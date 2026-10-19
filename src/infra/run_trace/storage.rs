use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use rustix::fs::{FlockOperation, flock};

pub(super) struct TraceFile {
    pub file: File,
    pub path: PathBuf,
    pub max_bytes: Option<usize>,
    pub retention: Option<Retention>,
}

pub(super) struct Retention {
    pub directory: PathBuf,
    pub max_files: usize,
}

pub(super) fn complete(file: File, retention: Option<Retention>) -> anyhow::Result<()> {
    // Other threads may fork while the writer is open. Closing only our
    // descriptor can leave its lock alive in the child until that child execs.
    flock(&file, FlockOperation::Unlock)?;
    drop(file);
    if let Some(retention) = retention {
        retention.prune()?;
    }
    Ok(())
}

impl Retention {
    pub fn prune(&self) -> anyhow::Result<()> {
        let directory = File::open(&self.directory)?;
        // Another run may be pruning; it will enforce retention on completion.
        if flock(&directory, FlockOperation::NonBlockingLockExclusive).is_err() {
            return Ok(());
        }
        let mut completed = Vec::new();
        let mut active = 0usize;
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() || !managed_name(&entry.file_name().to_string_lossy())
            {
                continue;
            }
            let file = File::open(entry.path())?;
            if flock(&file, FlockOperation::NonBlockingLockExclusive).is_err() {
                active += 1;
                continue;
            }
            let mut header = String::new();
            BufReader::new((&file).take(4096)).read_line(&mut header)?;
            let managed = serde_json::from_str::<serde_json::Value>(&header)
                .is_ok_and(|row| row["data"]["managed"] == true);
            if managed {
                completed.push((entry.file_name(), entry.path(), file));
            }
        }
        completed.sort_by(|a, b| a.0.cmp(&b.0));
        let excess = completed
            .len()
            .saturating_sub(self.max_files.saturating_sub(active));
        for (_, path, _lock) in completed.into_iter().take(excess) {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn managed_name(name: &str) -> bool {
    name.strip_prefix("nx-")
        .and_then(|name| name.strip_suffix(".jsonl"))
        .and_then(|name| name.split_once('-'))
        .is_some_and(|(epoch, id)| {
            epoch.parse::<u128>().is_ok() && uuid::Uuid::parse_str(id).is_ok()
        })
}

pub(super) fn from_env() -> anyhow::Result<Option<TraceFile>> {
    if let Some(path) = std::env::var_os("NX_TRACE_PATH") {
        return open_explicit(PathBuf::from(path)).map(Some);
    }
    let Some(directory) = std::env::var_os("NX_TRACE_DIR") else {
        return Ok(None);
    };
    create_managed(
        Path::new(&directory),
        positive_env("NX_TRACE_KEEP", 20, 1)?,
        positive_env("NX_TRACE_MAX_BYTES", 8 * 1024 * 1024, 4096)?,
    )
    .map(Some)
}

fn positive_env(name: &str, default: usize, minimum: usize) -> anyhow::Result<usize> {
    match std::env::var(name) {
        Ok(value) => {
            let value: usize = value.parse().with_context(|| format!("invalid {name}"))?;
            if value < minimum {
                bail!("{name} must be at least {minimum}");
            }
            Ok(value)
        }
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("invalid {name}")),
    }
}

fn open_explicit(path: PathBuf) -> anyhow::Result<TraceFile> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("creating trace {}", path.display()))?;
    flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    Ok(TraceFile {
        file,
        path,
        max_bytes: None,
        retention: None,
    })
}

fn create_managed(
    directory: &Path,
    max_files: usize,
    max_bytes: usize,
) -> anyhow::Result<TraceFile> {
    if !directory.is_absolute() {
        bail!("NX_TRACE_DIR must be absolute");
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let path = directory.join(format!("nx-{epoch:020}-{}.jsonl", uuid::Uuid::new_v4()));
    let mut output = open_explicit(path)?;
    output.max_bytes = Some(max_bytes);
    output.retention = Some(Retention {
        directory: directory.to_path_buf(),
        max_files,
    });
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn retention_preserves_active_explicit_and_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("notes.jsonl"), "private").unwrap();
        let explicit_path = directory
            .path()
            .join(format!("nx-000-{}.jsonl", uuid::Uuid::new_v4()));
        fs::write(&explicit_path, "{\"data\":{\"managed\":false}}\n").unwrap();
        let mut active = create_managed(directory.path(), 2, 4096).unwrap();
        writeln!(active.file, "{{\"data\":{{\"managed\":true}}}}").unwrap();
        for _ in 0..4 {
            let mut finished = create_managed(directory.path(), 2, 4096).unwrap();
            writeln!(finished.file, "{{\"data\":{{\"managed\":true}}}}").unwrap();
            complete(finished.file, None).unwrap();
        }
        active.retention.as_ref().unwrap().prune().unwrap();
        assert!(active.path.exists());
        assert!(explicit_path.exists());
        assert!(directory.path().join("notes.jsonl").exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 4);
        assert_eq!(
            active.file.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn completion_releases_lock_even_when_a_duplicate_descriptor_survives() {
        let directory = tempfile::tempdir().unwrap();
        let finished = create_managed(directory.path(), 1, 4096).unwrap();
        // A fork can retain this same open file description until exec.
        let inherited = finished.file.try_clone().unwrap();
        let path = finished.path.clone();
        complete(finished.file, None).unwrap();
        let probe = File::open(path).unwrap();
        assert!(flock(&probe, FlockOperation::NonBlockingLockExclusive).is_ok());
        drop(inherited);
    }

    #[test]
    fn explicit_files_are_exclusive_and_relative_directories_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.jsonl");
        let _first = open_explicit(path.clone()).unwrap();
        assert!(open_explicit(path).is_err());
        assert!(create_managed(Path::new("relative"), 20, 4096).is_err());
    }
}
