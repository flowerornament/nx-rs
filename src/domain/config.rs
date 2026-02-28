use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Purpose-based routing to `.nix` config files.
///
/// Discovers files by scanning `# nx:` comment tags on the first line,
/// then provides accessors that resolve by keyword match with deterministic fallbacks.
pub struct ConfigFiles {
    repo_root: PathBuf,
    by_purpose: BTreeMap<String, PathBuf>,
    all_files: Vec<PathBuf>,
}

impl ConfigFiles {
    /// Scan the repo for `.nix` files and read their `# nx:` purpose tags.
    ///
    /// Skips `default.nix` and `common.nix` per SPEC 3.2 — these are not
    /// routing targets. Note: `config_scan::collect_nix_files` intentionally
    /// includes `default.nix` for package/service scanning.
    /// Silently skips files that can't be read.
    pub fn discover(repo_root: &Path) -> Self {
        let mut by_purpose = BTreeMap::new();
        let mut all_files = Vec::new();

        for dir_name in ["home", "system", "hosts", "packages"] {
            let dir_path = repo_root.join(dir_name);
            if !dir_path.exists() {
                continue;
            }

            for entry in WalkDir::new(&dir_path)
                .sort_by_file_name()
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("nix") {
                    continue;
                }
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if file_name == "default.nix" || file_name == "common.nix" {
                    continue;
                }

                all_files.push(path.to_path_buf());

                if let Some(purpose) = read_nx_comment(path) {
                    by_purpose.insert(purpose, path.to_path_buf());
                }
            }
        }

        all_files.sort();

        Self {
            repo_root: repo_root.to_path_buf(),
            by_purpose,
            all_files,
        }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn all_files(&self) -> &[PathBuf] {
        &self.all_files
    }

    pub const fn by_purpose(&self) -> &BTreeMap<String, PathBuf> {
        &self.by_purpose
    }

    // -- Primary accessors --

    pub fn packages(&self) -> PathBuf {
        self.find_by_keywords(&["cli tools", "utilities"])
            .unwrap_or_else(|| self.repo_root.join("packages/nix/cli.nix"))
    }

    pub fn languages(&self) -> PathBuf {
        self.find_by_keywords(&["language", "runtimes", "toolchains"])
            .unwrap_or_else(|| self.repo_root.join("packages/nix/languages.nix"))
    }

    pub fn services(&self) -> PathBuf {
        self.find_by_keywords(&["services", "daemons"])
            .unwrap_or_else(|| self.repo_root.join("home/services.nix"))
    }

    pub fn darwin(&self) -> PathBuf {
        self.find_by_keywords(&["macos system"])
            .unwrap_or_else(|| self.repo_root.join("system/darwin.nix"))
    }

    pub fn homebrew_brews(&self) -> PathBuf {
        self.find_by_keywords(&["formula manifest", "brews"])
            .unwrap_or_else(|| self.repo_root.join("packages/homebrew/brews.nix"))
    }

    pub fn homebrew_casks(&self) -> PathBuf {
        self.find_by_keywords(&["cask manifest", "gui apps"])
            .unwrap_or_else(|| self.repo_root.join("packages/homebrew/casks.nix"))
    }

    #[allow(dead_code)] // retained for explicit tap routing outside current command paths
    pub fn homebrew_taps(&self) -> PathBuf {
        self.find_by_keywords(&["taps manifest"])
            .unwrap_or_else(|| self.repo_root.join("packages/homebrew/taps.nix"))
    }

    // -- Secondary accessors --

    #[allow(dead_code)] // retained for explicit shell config routing helpers
    pub fn shell(&self) -> PathBuf {
        self.find_by_keywords(&["shell"])
            .unwrap_or_else(|| self.repo_root.join("home/shell.nix"))
    }

    #[allow(dead_code)] // retained for explicit editor config routing helpers
    pub fn editors(&self) -> PathBuf {
        self.find_by_keywords(&["editor"])
            .unwrap_or_else(|| self.repo_root.join("home/editors.nix"))
    }

    #[allow(dead_code)] // retained for explicit git config routing helpers
    pub fn git(&self) -> PathBuf {
        self.find_by_keywords(&["git", "version control"])
            .unwrap_or_else(|| self.repo_root.join("home/git.nix"))
    }

    #[allow(dead_code)] // retained for explicit terminal config routing helpers
    pub fn terminal(&self) -> PathBuf {
        self.find_by_keywords(&["terminal", "multiplexer"])
            .unwrap_or_else(|| self.repo_root.join("home/terminal.nix"))
    }

    // -- Internal --

    fn find_by_keywords(&self, keywords: &[&str]) -> Option<PathBuf> {
        for keyword in keywords {
            let keyword_lower = keyword.to_lowercase();
            for (purpose, path) in &self.by_purpose {
                if purpose.to_lowercase().contains(&keyword_lower) {
                    return Some(path.clone());
                }
            }
        }
        None
    }
}

/// Read the `# nx:` purpose comment from the first line of a file.
fn read_nx_comment(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let trimmed = first_line.trim();
    trimmed
        .strip_prefix("# nx:")
        .map(|rest| rest.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_nix(dir: &Path, rel_path: &str, content: &str) {
        let full = dir.join(rel_path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
    }

    #[test]
    fn discover_finds_tagged_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            "# nx: cli tools and utilities\n{ pkgs }: []",
        );
        write_nix(
            root,
            "home/services.nix",
            "# nx: services and daemons\n{ ... }: {}",
        );
        write_nix(root, "home/shell.nix", "{ ... }: {}");

        let cf = ConfigFiles::discover(root);

        assert_eq!(cf.by_purpose().len(), 2);
        assert!(cf.by_purpose().contains_key("cli tools and utilities"));
        assert!(cf.by_purpose().contains_key("services and daemons"));
    }

    #[test]
    fn keyword_matching_resolves_correct_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            "# nx: cli tools and utilities\n[]",
        );
        write_nix(
            root,
            "packages/nix/languages.nix",
            "# nx: language runtimes\n[]",
        );

        let cf = ConfigFiles::discover(root);

        assert_eq!(cf.packages(), root.join("packages/nix/cli.nix"));
        assert_eq!(cf.languages(), root.join("packages/nix/languages.nix"));
    }

    #[test]
    fn ambiguous_keyword_matches_use_deterministic_winner() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(root, "home/shell-a.nix", "# nx: shell aliases\n{}");
        write_nix(root, "home/shell-z.nix", "# nx: shell profile\n{}");

        let cf = ConfigFiles::discover(root);

        // BTreeMap ordering yields a stable winner for ambiguous keyword matches.
        assert_eq!(cf.shell(), root.join("home/shell-a.nix"));
    }

    #[test]
    fn fallback_when_no_tags_match() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // No files at all — every accessor should return its fallback
        let cf = ConfigFiles::discover(root);

        assert_eq!(cf.packages(), root.join("packages/nix/cli.nix"));
        assert_eq!(cf.languages(), root.join("packages/nix/languages.nix"));
        assert_eq!(cf.services(), root.join("home/services.nix"));
        assert_eq!(cf.darwin(), root.join("system/darwin.nix"));
        assert_eq!(
            cf.homebrew_brews(),
            root.join("packages/homebrew/brews.nix")
        );
        assert_eq!(
            cf.homebrew_casks(),
            root.join("packages/homebrew/casks.nix")
        );
        assert_eq!(cf.homebrew_taps(), root.join("packages/homebrew/taps.nix"));
        assert_eq!(cf.shell(), root.join("home/shell.nix"));
        assert_eq!(cf.editors(), root.join("home/editors.nix"));
        assert_eq!(cf.git(), root.join("home/git.nix"));
        assert_eq!(cf.terminal(), root.join("home/terminal.nix"));
    }

    #[test]
    fn default_nix_and_common_nix_excluded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(root, "home/default.nix", "# nx: should be ignored\n{}");
        write_nix(root, "home/common.nix", "# nx: also ignored\n{}");
        write_nix(root, "home/shell.nix", "# nx: shell config\n{}");

        let cf = ConfigFiles::discover(root);

        assert_eq!(cf.all_files().len(), 1);
        assert!(cf.all_files()[0].ends_with("home/shell.nix"));
        assert_eq!(cf.by_purpose().len(), 1);
    }

    #[test]
    fn read_nx_comment_extracts_purpose() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.nix");
        fs::write(&path, "# nx: formula manifest for homebrew\n{ ... }: {}").unwrap();

        assert_eq!(
            read_nx_comment(&path),
            Some("formula manifest for homebrew".to_string())
        );
    }

    #[test]
    fn read_nx_comment_returns_none_without_tag() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.nix");
        fs::write(&path, "{ pkgs, ... }:\n{}").unwrap();

        assert_eq!(read_nx_comment(&path), None);
    }

    #[test]
    fn keyword_match_is_case_insensitive() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(
            root,
            "system/darwin.nix",
            "# nx: MacOS System Configuration\n{}",
        );

        let cf = ConfigFiles::discover(root);

        // "macos system" should match "MacOS System Configuration"
        assert_eq!(cf.darwin(), root.join("system/darwin.nix"));
    }
}
