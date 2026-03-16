use std::path::{Path, PathBuf};

use walkdir::{DirEntry, WalkDir};

const MANAGED_CONFIG_DIRS: [&str; 4] = ["home", "system", "hosts", "packages"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NixFileScanPolicy {
    include_hidden_paths: bool,
    include_default_nix: bool,
    include_common_nix: bool,
}

impl NixFileScanPolicy {
    pub(crate) const fn all_files() -> Self {
        Self {
            include_hidden_paths: true,
            include_default_nix: true,
            include_common_nix: true,
        }
    }

    pub(crate) const fn for_config_routing() -> Self {
        Self {
            include_hidden_paths: true,
            include_default_nix: false,
            include_common_nix: false,
        }
    }

    pub(crate) const fn for_package_scan() -> Self {
        Self {
            include_hidden_paths: true,
            include_default_nix: true,
            include_common_nix: false,
        }
    }

    pub(crate) const fn for_repo_manifest_scan() -> Self {
        Self {
            include_hidden_paths: false,
            include_default_nix: true,
            include_common_nix: true,
        }
    }

    #[cfg(test)]
    const fn new(
        include_hidden_paths: bool,
        include_default_nix: bool,
        include_common_nix: bool,
    ) -> Self {
        Self {
            include_hidden_paths,
            include_default_nix,
            include_common_nix,
        }
    }
}

pub(crate) fn collect_managed_nix_files(
    repo_root: &Path,
    policy: NixFileScanPolicy,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir_name in MANAGED_CONFIG_DIRS {
        let dir_path = repo_root.join(dir_name);
        if !dir_path.exists() {
            continue;
        }
        extend_nix_files_from_root(&mut out, &dir_path, repo_root, policy);
    }
    out.sort();
    out
}

pub(crate) fn collect_repo_nix_files(repo_root: &Path, policy: NixFileScanPolicy) -> Vec<PathBuf> {
    let mut out = Vec::new();
    extend_nix_files_from_root(&mut out, repo_root, repo_root, policy);
    out.sort();
    out
}

fn extend_nix_files_from_root(
    out: &mut Vec<PathBuf>,
    scan_root: &Path,
    repo_root: &Path,
    policy: NixFileScanPolicy,
) {
    for entry in WalkDir::new(scan_root)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| should_descend(entry, repo_root, policy))
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("nix") {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !policy.include_default_nix && file_name == "default.nix" {
            continue;
        }
        if !policy.include_common_nix && file_name == "common.nix" {
            continue;
        }

        out.push(path.to_path_buf());
    }
}

fn should_descend(entry: &DirEntry, repo_root: &Path, policy: NixFileScanPolicy) -> bool {
    policy.include_hidden_paths || !is_hidden_path(entry.path(), repo_root)
}

fn is_hidden_path(path: &Path, repo_root: &Path) -> bool {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(root: &Path, rel_path: &str) {
        let full = root.join(rel_path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, "{}").unwrap();
    }

    #[test]
    fn scans_only_managed_roots() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "home/shell.nix");
        write_file(tmp.path(), "misc/ignored.nix");

        let files = collect_managed_nix_files(tmp.path(), NixFileScanPolicy::for_package_scan());

        assert_eq!(files, vec![tmp.path().join("home/shell.nix")]);
    }

    #[test]
    fn config_routing_policy_skips_default_and_common() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "home/default.nix");
        write_file(tmp.path(), "home/common.nix");
        write_file(tmp.path(), "home/shell.nix");

        let files = collect_managed_nix_files(tmp.path(), NixFileScanPolicy::for_config_routing());

        assert_eq!(files, vec![tmp.path().join("home/shell.nix")]);
    }

    #[test]
    fn package_scan_policy_keeps_default_but_skips_common() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "home/default.nix");
        write_file(tmp.path(), "home/common.nix");
        write_file(tmp.path(), "home/shell.nix");

        let files = collect_managed_nix_files(tmp.path(), NixFileScanPolicy::for_package_scan());

        assert_eq!(
            files,
            vec![
                tmp.path().join("home/default.nix"),
                tmp.path().join("home/shell.nix"),
            ]
        );
    }

    #[test]
    fn hidden_paths_are_excluded_when_policy_disables_them() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "home/.private/secret.nix");
        write_file(tmp.path(), "home/visible.nix");

        let files =
            collect_managed_nix_files(tmp.path(), NixFileScanPolicy::new(false, true, true));

        assert_eq!(files, vec![tmp.path().join("home/visible.nix")]);
    }

    #[test]
    fn all_files_policy_keeps_default_and_common() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "home/default.nix");
        write_file(tmp.path(), "home/common.nix");

        let files = collect_managed_nix_files(tmp.path(), NixFileScanPolicy::all_files());

        assert_eq!(
            files,
            vec![
                tmp.path().join("home/common.nix"),
                tmp.path().join("home/default.nix"),
            ]
        );
    }

    #[test]
    fn repo_manifest_scan_includes_non_managed_roots() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "flake.nix");
        write_file(tmp.path(), "modules/cli.nix");

        let files = collect_repo_nix_files(tmp.path(), NixFileScanPolicy::for_repo_manifest_scan());

        assert_eq!(
            files,
            vec![
                tmp.path().join("flake.nix"),
                tmp.path().join("modules/cli.nix")
            ]
        );
    }

    #[test]
    fn repo_manifest_scan_skips_hidden_paths() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), ".git/hooks/pre-commit.nix");
        write_file(tmp.path(), "visible.nix");

        let files = collect_repo_nix_files(tmp.path(), NixFileScanPolicy::for_repo_manifest_scan());

        assert_eq!(files, vec![tmp.path().join("visible.nix")]);
    }
}
