use std::error::Error;
use std::fs;
use std::path::Path;

/// Recursively copy a directory tree from `src` into `dst`.
///
/// # Errors
///
/// Returns an error if any directory entry cannot be read, created, or copied.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            fs::create_dir_all(&dst_path)?;
            copy_tree(&src_path, &dst_path)?;
            continue;
        }

        if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
