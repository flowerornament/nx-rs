use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;

fn normalize_file_content(input: &str) -> String {
    input
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// Snapshot all non-ignored files under `repo_root` with normalized text content.
///
/// # Errors
///
/// Returns an error if walking directories or reading file contents fails.
pub fn snapshot_repo_files(
    repo_root: &Path,
    ignore: &dyn Fn(&str) -> bool,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut files = BTreeMap::new();
    snapshot_dir(repo_root, repo_root, &mut files, ignore)?;
    Ok(files)
}

fn snapshot_dir(
    repo_root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, String>,
    ignore: &dyn Fn(&str) -> bool,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(repo_root)
            .map_err(|err| io::Error::other(format!("strip_prefix failed: {err}")))?;
        let rel_key = rel.to_string_lossy().replace('\\', "/");

        if ignore(&rel_key) {
            continue;
        }

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            snapshot_dir(repo_root, &path, out, ignore)?;
            continue;
        }

        if file_type.is_file() {
            let bytes = fs::read(&path)?;
            let text = String::from_utf8_lossy(&bytes);
            out.insert(rel_key, normalize_file_content(&text));
        }
    }
    Ok(())
}
