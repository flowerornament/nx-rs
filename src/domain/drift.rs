use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::manifest::{Manifest, PlatformKind};
use super::manifest_scan::{ScannedRepo, manifest_from_scan, scan_repo};

#[derive(Debug, Clone)]
pub enum ManifestHealth {
    Missing,
    Invalid {
        error: String,
    },
    InSync {
        manifest: Manifest,
    },
    Drifted {
        effective_manifest: Manifest,
        report: DriftReport,
    },
}

impl ManifestHealth {
    pub fn from_scan(scanned: &ScannedRepo, repo_root: &Path) -> Self {
        match Manifest::load(repo_root) {
            Ok(Some(manifest)) => {
                let report = detect_manifest_drift_with_scan(scanned, repo_root, &manifest);
                let effective_manifest =
                    manifest_from_scan(scanned.clone(), repo_root, Some(&manifest));
                if report.is_empty() {
                    Self::InSync { manifest }
                } else {
                    Self::Drifted {
                        effective_manifest,
                        report,
                    }
                }
            }
            Ok(None) => Self::Missing,
            Err(err) => Self::Invalid {
                error: format!("{err:#}"),
            },
        }
    }

    pub fn routing_manifest(&self) -> Option<&Manifest> {
        match self {
            Self::InSync { manifest } => Some(manifest),
            Self::Drifted {
                effective_manifest, ..
            } => Some(effective_manifest),
            Self::Missing | Self::Invalid { .. } => None,
        }
    }

    pub fn blocks_system_commands(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }

    pub fn invalid_error(&self) -> Option<&str> {
        match self {
            Self::Invalid { error, .. } => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriftReport {
    pub issues: Vec<DriftIssue>,
}

impl DriftReport {
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SlotSignature {
    pub file: PathBuf,
    pub attr_path: String,
    pub kind: &'static str,
    pub runtime: Option<String>,
}

impl SlotSignature {
    fn from_slot(slot: &super::manifest::Slot) -> Self {
        Self {
            file: slot.file.clone(),
            attr_path: slot.attr_path.clone(),
            kind: slot.kind.as_str(),
            runtime: slot.runtime.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftIssue {
    MissingSlotFile {
        file: PathBuf,
    },
    StaleSlot {
        slot: SlotSignature,
    },
    NewSlot {
        slot: SlotSignature,
    },
    PlatformKindChanged {
        manifest: PlatformKind,
        scanned: PlatformKind,
    },
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn detect_manifest_drift(repo_root: &Path, manifest: &Manifest) -> DriftReport {
    let scanned = scan_repo(repo_root);
    detect_manifest_drift_with_scan(&scanned, repo_root, manifest)
}

fn detect_manifest_drift_with_scan(
    scanned: &ScannedRepo,
    repo_root: &Path,
    manifest: &Manifest,
) -> DriftReport {
    let mut issues = Vec::new();

    if manifest.platform.kind != scanned.platform.kind {
        issues.push(DriftIssue::PlatformKindChanged {
            manifest: manifest.platform.kind,
            scanned: scanned.platform.kind,
        });
    }

    let manifest_slots: BTreeSet<_> = manifest
        .slots
        .iter()
        .map(SlotSignature::from_slot)
        .collect();
    let scanned_slots: BTreeSet<_> = scanned.slots.iter().map(SlotSignature::from_slot).collect();

    let mut missing_files = BTreeSet::new();
    for slot in manifest_slots.difference(&scanned_slots) {
        let full_path = repo_root.join(&slot.file);
        if !full_path.exists() {
            missing_files.insert(slot.file.clone());
            continue;
        }
        issues.push(DriftIssue::StaleSlot { slot: slot.clone() });
    }

    for file in missing_files {
        issues.push(DriftIssue::MissingSlotFile { file });
    }

    for slot in scanned_slots.difference(&manifest_slots) {
        issues.push(DriftIssue::NewSlot { slot: slot.clone() });
    }

    issues.sort_by_key(issue_sort_key);
    DriftReport { issues }
}

fn issue_sort_key(issue: &DriftIssue) -> (u8, String, String) {
    match issue {
        DriftIssue::MissingSlotFile { file } => (0, file.display().to_string(), String::new()),
        DriftIssue::PlatformKindChanged { manifest, scanned } => (
            1,
            manifest.as_str().to_string(),
            scanned.as_str().to_string(),
        ),
        DriftIssue::StaleSlot { slot } => {
            (2, slot.file.display().to_string(), slot.attr_path.clone())
        }
        DriftIssue::NewSlot { slot } => {
            (3, slot.file.display().to_string(), slot.attr_path.clone())
        }
    }
}

pub fn format_issue(issue: &DriftIssue) -> String {
    match issue {
        DriftIssue::MissingSlotFile { file } => {
            format!("missing manifest file {}", file.display())
        }
        DriftIssue::StaleSlot { slot } => format!(
            "manifest slot no longer matches repo: {} ({})",
            slot.file.display(),
            describe_slot(slot)
        ),
        DriftIssue::NewSlot { slot } => format!(
            "new repo slot not captured in manifest: {} ({})",
            slot.file.display(),
            describe_slot(slot)
        ),
        DriftIssue::PlatformKindChanged { manifest, scanned } => {
            format!(
                "platform kind changed: manifest={} repo={}",
                manifest.as_str(),
                scanned.as_str()
            )
        }
    }
}

fn describe_slot(slot: &SlotSignature) -> String {
    match &slot.runtime {
        Some(runtime) => format!("{} {} runtime={runtime}", slot.kind, slot.attr_path),
        None => format!("{} {}", slot.kind, slot.attr_path),
    }
}

pub fn affects_routing(issue: &DriftIssue) -> bool {
    !matches!(issue, DriftIssue::PlatformKindChanged { .. })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::domain::manifest::{PlatformConfig, Slot, SlotKind};

    fn write_file(root: &Path, rel_path: &str, content: &str) {
        let full = root.join(rel_path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    fn manifest_with_slots(slots: Vec<Slot>) -> Manifest {
        Manifest {
            schema_version: 1,
            platform: Manifest::default_darwin(),
            slots,
            aliases: HashMap::from([("vim".to_string(), "helix".to_string())]),
            overlays: HashMap::from([(
                "firefox".to_string(),
                "nxs-mozilla:firefox-nightly-bin:Firefox Nightly".to_string(),
            )]),
        }
    }

    fn nix_slot(file: &str, attr_path: &str) -> Slot {
        Slot {
            kind: SlotKind::NixPackages,
            file: PathBuf::from(file),
            attr_path: attr_path.to_string(),
            tags: vec!["custom".to_string()],
            runtime: None,
            default_for: Some(vec!["install".to_string()]),
        }
    }

    #[test]
    fn clean_manifest_has_no_drift() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            "{ outputs = { self, nix-darwin, ... }: { darwinConfigurations.host = {}; }; }",
        );
        write_file(
            tmp.path(),
            "packages/cli.nix",
            "{ pkgs, ... }: { home.packages = with pkgs; [ ripgrep ]; }",
        );

        let manifest = manifest_with_slots(vec![nix_slot("packages/cli.nix", "home.packages")]);
        let report = detect_manifest_drift(tmp.path(), &manifest);

        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn removed_slot_file_is_reported_once() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            "{ outputs = { self, nix-darwin, ... }: { darwinConfigurations.host = {}; }; }",
        );

        let manifest = manifest_with_slots(vec![nix_slot("packages/cli.nix", "home.packages")]);
        let report = detect_manifest_drift(tmp.path(), &manifest);

        assert_eq!(
            report.issues,
            vec![DriftIssue::MissingSlotFile {
                file: PathBuf::from("packages/cli.nix")
            }]
        );
    }

    #[test]
    fn new_repo_slot_is_reported() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            "{ outputs = { self, nix-darwin, ... }: { darwinConfigurations.host = {}; }; }",
        );
        write_file(
            tmp.path(),
            "packages/cli.nix",
            "{ pkgs, ... }: { home.packages = with pkgs; [ ripgrep ]; }",
        );

        let manifest = manifest_with_slots(Vec::new());
        let report = detect_manifest_drift(tmp.path(), &manifest);

        assert_eq!(
            report.issues,
            vec![DriftIssue::NewSlot {
                slot: SlotSignature {
                    file: PathBuf::from("packages/cli.nix"),
                    attr_path: "home.packages".to_string(),
                    kind: "nix-packages",
                    runtime: None,
                }
            }]
        );
    }

    #[test]
    fn platform_changes_are_reported() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            "{ outputs = { self, nixpkgs, ... }: { nixosConfigurations.host = {}; }; }",
        );

        let manifest = Manifest {
            platform: PlatformConfig {
                kind: PlatformKind::Darwin,
                rebuild_command: "darwin-rebuild".to_string(),
                sudo: true,
                flake_root: ".".to_string(),
            },
            ..manifest_with_slots(Vec::new())
        };

        let report = detect_manifest_drift(tmp.path(), &manifest);

        assert_eq!(
            report.issues,
            vec![DriftIssue::PlatformKindChanged {
                manifest: PlatformKind::Darwin,
                scanned: PlatformKind::NixOS
            }]
        );
    }

    #[test]
    fn user_annotations_do_not_cause_drift() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            "{ outputs = { self, nix-darwin, ... }: { darwinConfigurations.host = {}; }; }",
        );
        write_file(
            tmp.path(),
            "packages/cli.nix",
            "{ pkgs, ... }: { home.packages = with pkgs; [ ripgrep ]; }",
        );

        let manifest = manifest_with_slots(vec![nix_slot("packages/cli.nix", "home.packages")]);
        let report = detect_manifest_drift(tmp.path(), &manifest);

        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn manifest_health_from_scan_reports_missing_without_manifest() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            "{ outputs = { self, nix-darwin, ... }: { darwinConfigurations.host = {}; }; }",
        );
        write_file(
            tmp.path(),
            "packages/cli.nix",
            "{ pkgs, ... }: { home.packages = with pkgs; [ ripgrep ]; }",
        );

        let scanned = scan_repo(tmp.path());

        assert!(matches!(
            ManifestHealth::from_scan(&scanned, tmp.path()),
            ManifestHealth::Missing
        ));
    }
}
