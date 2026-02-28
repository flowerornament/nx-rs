use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::app::dirs_home;
use crate::output::printer::Printer;

const AUTO_REFRESH_ENV: &str = "NX_RS_AUTO_REFRESH";
const SOURCE_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Refresh a local cargo-installed `nx` binary before heavy system commands.
///
/// Returns:
/// - `Some(exit_code)` when command flow should stop now
/// - `None` when no refresh action is needed
pub fn maybe_refresh_before_system_command(needs_refresh: bool, printer: &Printer) -> Option<i32> {
    if !needs_refresh || !auto_refresh_enabled() {
        return None;
    }

    let Ok(current_exe) = std::env::current_exe() else {
        return None;
    };
    if !is_local_cargo_nx(&current_exe) {
        return None;
    }

    let source_root = PathBuf::from(SOURCE_ROOT);
    if !source_root.join("Cargo.toml").exists() {
        return None;
    }

    let stale = is_binary_stale(&current_exe, &source_root);
    if !stale {
        return None;
    }

    printer.action("Refreshing local nx binary");
    Printer::detail(&format!(
        "cargo install --path {} --force",
        source_root.display()
    ));

    match Command::new("cargo")
        .args(["install", "--path", SOURCE_ROOT, "--force"])
        .current_dir(&source_root)
        .status()
    {
        Ok(status) if status.success() => {
            printer.success("Local nx binary refreshed");
            Printer::detail("Re-run your command to continue");
            Some(0)
        }
        Ok(status) => {
            printer.error(&format!(
                "Failed to refresh local nx binary (exit code {})",
                status.code().unwrap_or(-1)
            ));
            Some(1)
        }
        Err(err) => {
            printer.error(&format!("Failed to run cargo install: {err:#}"));
            Some(1)
        }
    }
}

fn auto_refresh_enabled() -> bool {
    std::env::var(AUTO_REFRESH_ENV).map_or(true, |value| {
        !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no")
    })
}

fn is_local_cargo_nx(binary_path: &Path) -> bool {
    let expected = dirs_home().join(".local/share/cargo/bin/nx");
    paths_equivalent(binary_path, &expected)
}

fn paths_equivalent(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }

    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a_can), Ok(b_can)) => a_can == b_can,
        _ => false,
    }
}

fn is_binary_stale(binary_path: &Path, source_root: &Path) -> bool {
    let Ok(binary_modified) = fs::metadata(binary_path).and_then(|meta| meta.modified()) else {
        return false;
    };

    latest_source_modified(source_root)
        .is_some_and(|source_modified| source_modified > binary_modified)
}

fn latest_source_modified(source_root: &Path) -> Option<SystemTime> {
    let mut latest = None;

    for file in ["Cargo.toml", "Cargo.lock", "build.rs"] {
        update_latest(&mut latest, &source_root.join(file));
    }

    update_latest_recursive(&mut latest, &source_root.join("src"));
    latest
}

fn update_latest_recursive(latest: &mut Option<SystemTime>, root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            update_latest_recursive(latest, &path);
        } else {
            update_latest(latest, &path);
        }
    }
}

fn update_latest(latest: &mut Option<SystemTime>, path: &Path) {
    let Ok(modified) = fs::metadata(path).and_then(|meta| meta.modified()) else {
        return;
    };

    if latest.is_none_or(|curr| modified > curr) {
        *latest = Some(modified);
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::{is_binary_stale, latest_source_modified};

    #[test]
    fn latest_source_modified_reads_src_and_manifest_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").unwrap();

        assert!(latest_source_modified(root).is_some());
    }

    #[test]
    fn is_binary_stale_when_source_is_newer() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").unwrap();

        let binary = root.join("nx");
        std::fs::write(&binary, "binary").unwrap();
        thread::sleep(Duration::from_millis(20));
        std::fs::write(root.join("src/lib.rs"), "pub fn y() {}\n").unwrap();

        assert!(is_binary_stale(&binary, root));
    }
}
