use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use super::manifest::{Manifest, SlotKind};

/// Purpose-based routing to `.nix` config files.
///
/// Discovers files by scanning `# nx:` comment tags on the first line,
/// then provides SPEC-defined accessors that resolve by keyword match with deterministic fallbacks.
/// When constructed from a manifest, resolves from manifest slots instead.
pub struct ConfigFiles {
    repo_root: PathBuf,
    by_purpose: BTreeMap<String, PathBuf>,
    all_files: Vec<PathBuf>,
    manifest: Option<Manifest>,
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
            manifest: None,
        }
    }

    /// Construct from a manifest, resolving slot files to absolute paths.
    pub fn from_manifest(manifest: &Manifest, repo_root: &Path) -> Self {
        let all_files: Vec<PathBuf> = manifest
            .slots
            .iter()
            .map(|slot| repo_root.join(&slot.file))
            .collect();

        let mut by_purpose = BTreeMap::new();
        for slot in &manifest.slots {
            for tag in &slot.tags {
                by_purpose.insert(tag.clone(), repo_root.join(&slot.file));
            }
        }

        Self {
            repo_root: repo_root.to_path_buf(),
            by_purpose,
            all_files,
            manifest: Some(manifest.clone()),
        }
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
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
    //
    // When a manifest is loaded, these resolve from manifest slots.
    // Otherwise they fall back to keyword matching and hardcoded paths.

    pub fn packages(&self) -> PathBuf {
        if let Some(slot) = self.manifest_slot_for(SlotKind::NixPackages, Some("install")) {
            return self.repo_root.join(&slot.file);
        }
        self.find_by_keywords(&["cli tools", "utilities"])
            .unwrap_or_else(|| self.repo_root.join("packages/nix/cli.nix"))
    }

    pub fn languages(&self) -> PathBuf {
        if let Some(slot) = self.manifest_slot_for(SlotKind::WithPackages, None) {
            return self.repo_root.join(&slot.file);
        }
        self.find_by_keywords(&["language", "runtimes", "toolchains"])
            .unwrap_or_else(|| self.repo_root.join("packages/nix/languages.nix"))
    }

    /// Find the file containing a specific runtime's withPackages block.
    pub fn with_packages_for(&self, runtime: &str) -> Option<PathBuf> {
        let manifest = self.manifest.as_ref()?;
        let slot = manifest
            .slots
            .iter()
            .find(|s| s.kind == SlotKind::WithPackages && s.runtime.as_deref() == Some(runtime))?;
        Some(self.repo_root.join(&slot.file))
    }

    pub fn services(&self) -> PathBuf {
        if let Some(slot) = self.manifest_slot_for(SlotKind::Services, None) {
            return self.repo_root.join(&slot.file);
        }
        self.find_by_keywords(&["services", "daemons"])
            .unwrap_or_else(|| self.repo_root.join("home/services.nix"))
    }

    pub fn darwin(&self) -> PathBuf {
        if let Some(slot) = self.manifest_slot_for(SlotKind::MasApps, None) {
            return self.repo_root.join(&slot.file);
        }
        self.find_by_keywords(&["macos system"])
            .unwrap_or_else(|| self.repo_root.join("system/darwin.nix"))
    }

    pub fn homebrew_brews(&self) -> PathBuf {
        if let Some(slot) = self.manifest_slot_for_tag(SlotKind::HomebrewList, "brews") {
            return self.repo_root.join(&slot.file);
        }
        self.find_by_keywords(&["formula manifest", "brews"])
            .unwrap_or_else(|| self.repo_root.join("packages/homebrew/brews.nix"))
    }

    pub fn homebrew_casks(&self) -> PathBuf {
        if let Some(slot) = self.manifest_slot_for_tag(SlotKind::HomebrewList, "casks") {
            return self.repo_root.join(&slot.file);
        }
        self.find_by_keywords(&["cask manifest", "gui apps"])
            .unwrap_or_else(|| self.repo_root.join("packages/homebrew/casks.nix"))
    }

    pub fn homebrew_taps(&self) -> PathBuf {
        // Taps are not a manifest slot kind — they share a file with brews or live in
        // a separate file matched by keyword only.
        self.find_by_keywords(&["taps manifest"])
            .unwrap_or_else(|| self.repo_root.join("packages/homebrew/taps.nix"))
    }

    // -- Internal --

    fn manifest_slot_for(
        &self,
        kind: SlotKind,
        default_for: Option<&str>,
    ) -> Option<&super::manifest::Slot> {
        let manifest = self.manifest.as_ref()?;
        if let Some(target) = default_for
            && let Some(slot) = manifest.slots.iter().find(|s| {
                s.kind == kind
                    && s.default_for
                        .as_ref()
                        .is_some_and(|df| df.iter().any(|d| d == target))
            })
        {
            return Some(slot);
        }
        manifest.slot_by_kind(kind)
    }

    fn manifest_slot_for_tag(&self, kind: SlotKind, tag: &str) -> Option<&super::manifest::Slot> {
        let manifest = self.manifest.as_ref()?;
        manifest
            .slots
            .iter()
            .find(|s| s.kind == kind && s.tags.iter().any(|t| t == tag))
            .or_else(|| manifest.slot_by_kind(kind))
    }

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
    use crate::domain::manifest::{Manifest, PlatformKind, Slot};
    use std::collections::HashMap;
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
    fn ambiguous_taps_keyword_matches_use_deterministic_winner() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(
            root,
            "packages/homebrew/taps-a.nix",
            "# nx: taps manifest alpha\n[]",
        );
        write_nix(
            root,
            "packages/homebrew/taps-z.nix",
            "# nx: taps manifest zulu\n[]",
        );

        let cf = ConfigFiles::discover(root);

        // BTreeMap ordering yields a stable winner for ambiguous keyword matches.
        assert_eq!(
            cf.homebrew_taps(),
            root.join("packages/homebrew/taps-a.nix")
        );
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

    // --- Manifest-aware routing ---

    fn test_manifest() -> Manifest {
        Manifest {
            schema_version: 1,
            platform: crate::domain::manifest::PlatformConfig {
                kind: PlatformKind::Darwin,
                rebuild_command: "/run/current-system/sw/bin/darwin-rebuild".to_string(),
                sudo: true,
                flake_root: ".".to_string(),
            },
            slots: vec![
                Slot {
                    kind: SlotKind::NixPackages,
                    file: PathBuf::from("modules/packages.nix"),
                    attr_path: "home.packages".to_string(),
                    tags: vec![],
                    runtime: None,
                    default_for: Some(vec!["install".to_string()]),
                },
                Slot {
                    kind: SlotKind::HomebrewList,
                    file: PathBuf::from("modules/brews.nix"),
                    attr_path: "homebrew.brews".to_string(),
                    tags: vec!["brews".to_string()],
                    runtime: None,
                    default_for: None,
                },
                Slot {
                    kind: SlotKind::HomebrewList,
                    file: PathBuf::from("modules/casks.nix"),
                    attr_path: "homebrew.casks".to_string(),
                    tags: vec!["casks".to_string()],
                    runtime: None,
                    default_for: None,
                },
                Slot {
                    kind: SlotKind::WithPackages,
                    file: PathBuf::from("modules/langs.nix"),
                    attr_path: "python3.withPackages".to_string(),
                    tags: vec![],
                    runtime: Some("python3".to_string()),
                    default_for: None,
                },
                Slot {
                    kind: SlotKind::Services,
                    file: PathBuf::from("modules/services.nix"),
                    attr_path: "launchd.agents".to_string(),
                    tags: vec![],
                    runtime: None,
                    default_for: None,
                },
                Slot {
                    kind: SlotKind::MasApps,
                    file: PathBuf::from("modules/mas.nix"),
                    attr_path: "homebrew.masApps".to_string(),
                    tags: vec![],
                    runtime: None,
                    default_for: None,
                },
            ],
            aliases: HashMap::new(),
            overlays: HashMap::new(),
        }
    }

    #[test]
    fn from_manifest_routes_packages_to_manifest_slot() {
        let tmp = TempDir::new().unwrap();
        let manifest = test_manifest();
        let cf = ConfigFiles::from_manifest(&manifest, tmp.path());

        assert_eq!(cf.packages(), tmp.path().join("modules/packages.nix"));
    }

    #[test]
    fn from_manifest_routes_languages_to_with_packages_slot() {
        let tmp = TempDir::new().unwrap();
        let manifest = test_manifest();
        let cf = ConfigFiles::from_manifest(&manifest, tmp.path());

        assert_eq!(cf.languages(), tmp.path().join("modules/langs.nix"));
    }

    #[test]
    fn with_packages_for_finds_matching_runtime() {
        let tmp = TempDir::new().unwrap();
        let manifest = test_manifest();
        let cf = ConfigFiles::from_manifest(&manifest, tmp.path());

        assert_eq!(
            cf.with_packages_for("python3"),
            Some(tmp.path().join("modules/langs.nix"))
        );
    }

    #[test]
    fn with_packages_for_returns_none_for_unknown_runtime() {
        let tmp = TempDir::new().unwrap();
        let manifest = test_manifest();
        let cf = ConfigFiles::from_manifest(&manifest, tmp.path());

        assert_eq!(cf.with_packages_for("perl"), None);
    }

    #[test]
    fn from_manifest_routes_brews_by_tag() {
        let tmp = TempDir::new().unwrap();
        let manifest = test_manifest();
        let cf = ConfigFiles::from_manifest(&manifest, tmp.path());

        assert_eq!(cf.homebrew_brews(), tmp.path().join("modules/brews.nix"));
        assert_eq!(cf.homebrew_casks(), tmp.path().join("modules/casks.nix"));
    }

    #[test]
    fn from_manifest_routes_services_and_darwin() {
        let tmp = TempDir::new().unwrap();
        let manifest = test_manifest();
        let cf = ConfigFiles::from_manifest(&manifest, tmp.path());

        assert_eq!(cf.services(), tmp.path().join("modules/services.nix"));
        assert_eq!(cf.darwin(), tmp.path().join("modules/mas.nix"));
    }

    #[test]
    fn from_manifest_populates_all_files() {
        let tmp = TempDir::new().unwrap();
        let manifest = test_manifest();
        let cf = ConfigFiles::from_manifest(&manifest, tmp.path());

        assert_eq!(cf.all_files().len(), manifest.slots.len());
    }

    #[test]
    fn from_manifest_keeps_manifest_routing_when_present() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let manifest = test_manifest();
        manifest.save(root).unwrap();

        let loaded = Manifest::load(root).unwrap().unwrap();
        let cf = ConfigFiles::from_manifest(&loaded, root);
        assert!(cf.manifest().is_some());
        assert_eq!(cf.packages(), root.join("modules/packages.nix"));
    }

    #[test]
    fn discover_falls_back_without_manifest() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(root, "packages/nix/cli.nix", "# nx: cli tools\n[]");

        let cf = ConfigFiles::discover(root);
        assert!(cf.manifest().is_none());
        assert_eq!(cf.packages(), root.join("packages/nix/cli.nix"));
    }
}
