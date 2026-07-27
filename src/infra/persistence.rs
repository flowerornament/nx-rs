use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tempfile::NamedTempFile;

/// Replace a file atomically after syncing its complete contents.
pub fn write_file_atomically(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    write_file_atomically_with(path, contents.as_ref(), |temp, target| {
        temp.persist(target).map(drop).map_err(|error| error.error)
    })
}

fn write_file_atomically_with(
    path: &Path,
    contents: &[u8],
    replace: impl FnOnce(NamedTempFile, &Path) -> io::Result<()>,
) -> Result<()> {
    let target = replacement_target(path)?;
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    target
        .file_name()
        .with_context(|| format!("atomic write target has no file name: {}", target.display()))?;
    let permissions = fs::metadata(&target)
        .map(|metadata| Some(metadata.permissions()))
        .or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            }
        })
        .with_context(|| format!("reading permissions for {}", target.display()))?;
    if permissions.as_ref().is_some_and(fs::Permissions::readonly) {
        bail!("refusing to replace read-only file {}", target.display());
    }

    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file for {}", target.display()))?;

    temp.write_all(contents)
        .with_context(|| format!("writing temporary file for {}", target.display()))?;
    if let Some(permissions) = permissions {
        temp.as_file()
            .set_permissions(permissions)
            .with_context(|| format!("preserving permissions for {}", target.display()))?;
    }
    temp.as_file()
        .sync_all()
        .with_context(|| format!("syncing temporary file for {}", target.display()))?;
    replace(temp, &target).with_context(|| format!("replacing {}", target.display()))
}

fn replacement_target(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::canonicalize(path).with_context(|| format!("resolving symlink {}", path.display()))
        }
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn creates_and_replaces_files_without_leaving_a_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.nix");

        write_file_atomically(&path, "first").unwrap();
        write_file_atomically(&path, "second").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn creates_new_files_with_restrictive_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.nix");

        write_file_atomically(&path, "new").unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn failed_replace_preserves_old_file_and_removes_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.nix");
        fs::write(&path, "old").unwrap();
        let mut temp_path = None;

        let error = write_file_atomically_with(&path, b"new", |temp, _| {
            assert_eq!(fs::read_to_string(temp.path()).unwrap(), "new");
            temp_path = Some(temp.path().to_path_buf());
            Err(io::Error::other("injected replace failure"))
        })
        .unwrap_err();

        assert!(error.to_string().contains("replacing"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        assert!(
            !temp_path
                .expect("replace callback should capture the temporary path")
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_existing_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.nix");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        write_file_atomically(&path, "new").unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_replace_a_read_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.nix");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();

        let error = write_file_atomically(&path, "new").unwrap_err();

        assert!(error.to_string().contains("read-only"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn follows_an_existing_symlink_without_replacing_it() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.nix");
        let link = dir.path().join("config.nix");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        symlink("target.nix", &link).unwrap();

        write_file_atomically(&link, "new").unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
