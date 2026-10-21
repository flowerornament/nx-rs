//! Serialize nx mutations across repositories, binaries and Unix users.
use std::cell::Cell;
use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, ensure};
use rustix::fs::{CWD, FlockOperation, OFlags, RenameFlags, flock, renameat_with};
use rustix::io::{FdFlags, fcntl_setfd};
use serde::{Deserialize, Serialize};

use crate::output::printer::Printer;

const DEFAULT_PATH: &str = "/var/tmp/nx-operation.lock";
pub(crate) const PATH_ENV: &str = "NX_OPERATION_LOCK_PATH";
static ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

thread_local! {
    static WAIT: Cell<Option<WaitTiming>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
pub(crate) struct WaitTiming {
    pub started: Instant,
    pub started_at: SystemTime,
    pub duration: Duration,
}

pub(crate) fn wait_timing() -> Option<WaitTiming> {
    WAIT.get()
}

#[derive(Serialize, Deserialize)]
struct Owner {
    pid: u32,
    command: String,
    repo: String,
}

pub(crate) struct OperationGuard {
    file: File,
}

impl OperationGuard {
    pub(crate) fn acquire(command: &str, repo: &Path, printer: &Printer) -> Result<Self> {
        let path =
            std::env::var_os(PATH_ENV).map_or_else(|| PathBuf::from(DEFAULT_PATH), PathBuf::from);
        ensure!(path.is_absolute(), "{PATH_ENV} must be an absolute path");
        Self::acquire_at(&path, command, repo, printer)
    }

    fn acquire_at(path: &Path, command: &str, repo: &Path, printer: &Printer) -> Result<Self> {
        let file = open_lock(path)
            .with_context(|| format!("opening operation lock {}", path.display()))?;
        let started = Instant::now();
        let started_at = SystemTime::now();
        let mut trace = crate::infra::run_trace::Span::start("phase", "operation-wait".into());
        let mut waited = false;
        let mut loading = None;
        loop {
            match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                Err(rustix::io::Errno::WOULDBLOCK) => {
                    if !waited {
                        let message = wait_message(&file);
                        printer.action(&message);
                        loading = Some(printer.loading(&message));
                        waited = true;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error).context("acquiring operation lock"),
            }
        }
        drop(loading);
        if let Some(trace) = trace.as_mut() {
            trace.finish(0);
        }
        if waited {
            let duration = started.elapsed();
            printer.success(&format!(
                "Operation lock acquired after {:.1}s",
                duration.as_secs_f64()
            ));
            WAIT.set(Some(WaitTiming {
                started,
                started_at,
                duration,
            }));
        }
        // shell::operation_command retains this descriptor while waiting for
        // children, including sudo commands which close inherited descriptors.
        fcntl_setfd(&file, FdFlags::empty())?;
        let guard = Self { file };
        let owner = Owner {
            pid: std::process::id(),
            command: command.into(),
            repo: repo.display().to_string(),
        };
        guard.file.set_len(0)?;
        serde_json::to_writer(&guard.file, &owner)?;
        (&guard.file).flush()?;
        ACTIVE.store(true, Ordering::Relaxed);
        Ok(guard)
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        // Never unlink the file: waiters must keep locking the same inode.
        let _ = self.file.set_len(0);
        ACTIVE.store(false, Ordering::Relaxed);
        WAIT.set(None);
    }
}

fn open_lock(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(i32::try_from((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits())?);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Publish only after chmod, so another UID never observes umask's
            // private mode, even if this process dies during initialization.
            let temporary =
                tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
            temporary
                .as_file()
                .set_permissions(Permissions::from_mode(0o666))?;
            match renameat_with(CWD, temporary.path(), CWD, path, RenameFlags::NOREPLACE) {
                Ok(()) => temporary.into_file(),
                Err(rustix::io::Errno::EXIST) => options.open(path)?,
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1,
        "operation lock must be a regular file with no hard links"
    );
    Ok(file)
}

fn wait_message(file: &File) -> String {
    let mut bytes = [0; 4096];
    let owner = file
        .read_at(&mut bytes, 0)
        .ok()
        .and_then(|size| serde_json::from_slice::<Owner>(&bytes[..size]).ok());
    let text = owner.map_or_else(
        || "Waiting for another nx operation".into(),
        |owner| {
            format!(
                "Waiting for nx {} (PID {}) in {}",
                owner.command, owner.pid, owner.repo
            )
        },
    );
    text.chars()
        .filter(|character| !character.is_control())
        .collect()
}
