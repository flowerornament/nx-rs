use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

// --- Types

/// Top-level manifest describing a Nix config repository's structure.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub schema_version: u32,
    pub platform: PlatformConfig,
    pub slots: Vec<Slot>,
    pub aliases: HashMap<String, String>,
    pub overlays: HashMap<String, String>,
}

/// Platform-specific rebuild configuration.
#[derive(Debug, Clone)]
pub struct PlatformConfig {
    pub kind: PlatformKind,
    pub rebuild_command: String,
    pub sudo: bool,
    pub flake_root: String,
}

/// Supported Nix platform types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Darwin,
    NixOS,
    HomeManager,
    Custom,
}

/// A discovered config file slot within the repository.
#[derive(Debug, Clone)]
pub struct Slot {
    pub kind: SlotKind,
    pub file: PathBuf,
    pub attr_path: String,
    pub tags: Vec<String>,
    pub runtime: Option<String>,
    pub default_for: Option<Vec<String>>,
}

/// The type of packages/items a slot contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    NixPackages,
    WithPackages,
    HomebrewList,
    MasApps,
    Services,
}

// --- Display impls

impl PlatformKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Darwin => "darwin",
            Self::NixOS => "nixos",
            Self::HomeManager => "home-manager",
            Self::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "darwin" => Some(Self::Darwin),
            "nixos" => Some(Self::NixOS),
            "home-manager" => Some(Self::HomeManager),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

impl SlotKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NixPackages => "nix-packages",
            Self::WithPackages => "with-packages",
            Self::HomebrewList => "homebrew-list",
            Self::MasApps => "mas-apps",
            Self::Services => "services",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "nix-packages" => Some(Self::NixPackages),
            "with-packages" => Some(Self::WithPackages),
            "homebrew-list" => Some(Self::HomebrewList),
            "mas-apps" => Some(Self::MasApps),
            "services" => Some(Self::Services),
            _ => None,
        }
    }
}

// --- Constants

const MANIFEST_DIR: &str = ".nx";
const MANIFEST_FILE: &str = "manifest.toml";
const CURRENT_SCHEMA_VERSION: u32 = 1;

// --- Load / Save

impl Manifest {
    fn manifest_path(repo_root: &Path) -> PathBuf {
        repo_root.join(MANIFEST_DIR).join(MANIFEST_FILE)
    }

    /// Load manifest from `.nx/manifest.toml`. Returns `None` if the file does not exist.
    pub fn load(repo_root: &Path) -> Result<Option<Self>> {
        let path = Self::manifest_path(repo_root);
        if !path.exists() {
            return Ok(None);
        }
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let doc: toml_edit::DocumentMut = content
            .parse()
            .with_context(|| format!("parsing {}", path.display()))?;
        let manifest = parse_document(&doc)?;
        Ok(Some(manifest))
    }

    /// Write manifest to `.nx/manifest.toml`, creating the directory if needed.
    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let dir = repo_root.join(MANIFEST_DIR);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = Self::manifest_path(repo_root);
        let content = serialize_manifest(self);
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Default platform config for Darwin.
    pub fn default_darwin() -> PlatformConfig {
        PlatformConfig {
            kind: PlatformKind::Darwin,
            rebuild_command: "/run/current-system/sw/bin/darwin-rebuild".to_string(),
            sudo: true,
            flake_root: ".".to_string(),
        }
    }

    /// Default platform config for NixOS.
    pub fn default_nixos() -> PlatformConfig {
        PlatformConfig {
            kind: PlatformKind::NixOS,
            rebuild_command: "nixos-rebuild".to_string(),
            sudo: true,
            flake_root: ".".to_string(),
        }
    }

    /// Default platform config for Home Manager.
    pub fn default_home_manager() -> PlatformConfig {
        PlatformConfig {
            kind: PlatformKind::HomeManager,
            rebuild_command: "home-manager".to_string(),
            sudo: false,
            flake_root: ".".to_string(),
        }
    }

    /// Return the first slot matching a given kind.
    pub fn slot_by_kind(&self, kind: SlotKind) -> Option<&Slot> {
        self.slots.iter().find(|s| s.kind == kind)
    }

    /// Return all slots matching a given kind.
    pub fn slots_by_kind(&self, kind: SlotKind) -> Vec<&Slot> {
        self.slots.iter().filter(|s| s.kind == kind).collect()
    }

    /// Return the default install target slot (the one tagged `default_for: ["install"]`),
    /// falling back to the first `NixPackages` slot.
    pub fn default_install_slot(&self) -> Option<&Slot> {
        self.slots
            .iter()
            .find(|s| {
                s.default_for
                    .as_ref()
                    .is_some_and(|df| df.iter().any(|d| d == "install"))
            })
            .or_else(|| self.slot_by_kind(SlotKind::NixPackages))
    }
}

// --- TOML Parsing

fn parse_document(doc: &toml_edit::DocumentMut) -> Result<Manifest> {
    let schema_version = doc
        .get("schema_version")
        .and_then(toml_edit::Item::as_integer)
        .map_or(CURRENT_SCHEMA_VERSION, |v| u32::try_from(v).unwrap_or(0));

    let platform = doc
        .get("platform")
        .and_then(toml_edit::Item::as_table)
        .map(parse_platform)
        .transpose()?
        .unwrap_or_else(Manifest::default_darwin);

    let slots = doc
        .get("slots")
        .and_then(toml_edit::Item::as_array_of_tables)
        .map(|arr| arr.iter().map(parse_slot).collect::<Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();

    let aliases = doc
        .get("aliases")
        .and_then(toml_edit::Item::as_table)
        .map(parse_string_map)
        .unwrap_or_default();

    let overlays = doc
        .get("overlays")
        .and_then(toml_edit::Item::as_table)
        .map(parse_string_map)
        .unwrap_or_default();

    Ok(Manifest {
        schema_version,
        platform,
        slots,
        aliases,
        overlays,
    })
}

fn parse_platform(table: &toml_edit::Table) -> Result<PlatformConfig> {
    let kind_str = table
        .get("kind")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or("darwin");
    let kind = PlatformKind::parse(kind_str)
        .ok_or_else(|| anyhow::anyhow!("unknown platform kind: {kind_str}"))?;

    let rebuild_command = table
        .get("rebuild_command")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or(match kind {
            PlatformKind::Darwin => "/run/current-system/sw/bin/darwin-rebuild",
            PlatformKind::NixOS => "nixos-rebuild",
            PlatformKind::HomeManager => "home-manager",
            PlatformKind::Custom => "echo",
        })
        .to_string();

    let sudo = table
        .get("sudo")
        .and_then(toml_edit::Item::as_bool)
        .unwrap_or(matches!(kind, PlatformKind::Darwin | PlatformKind::NixOS));

    let flake_root = table
        .get("flake_root")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or(".")
        .to_string();

    Ok(PlatformConfig {
        kind,
        rebuild_command,
        sudo,
        flake_root,
    })
}

fn parse_slot(table: &toml_edit::Table) -> Result<Slot> {
    let kind_str = table
        .get("kind")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or("nix-packages");
    let kind = SlotKind::parse(kind_str)
        .ok_or_else(|| anyhow::anyhow!("unknown slot kind: {kind_str}"))?;

    let file = table
        .get("file")
        .and_then(toml_edit::Item::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("slot missing 'file' field"))?;

    let attr_path = table
        .get("attr_path")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or("")
        .to_string();

    let tags = table
        .get("tags")
        .and_then(toml_edit::Item::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(toml_edit::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let runtime = table
        .get("runtime")
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);

    let default_for = table
        .get("default_for")
        .and_then(toml_edit::Item::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(toml_edit::Value::as_str)
                .map(str::to_string)
                .collect()
        });

    Ok(Slot {
        kind,
        file,
        attr_path,
        tags,
        runtime,
        default_for,
    })
}

fn parse_string_map(table: &toml_edit::Table) -> HashMap<String, String> {
    table
        .iter()
        .filter_map(|(key, val)| val.as_str().map(|v| (key.to_string(), v.to_string())))
        .collect()
}

// --- TOML Serialization

fn serialize_manifest(manifest: &Manifest) -> String {
    let mut doc = toml_edit::DocumentMut::new();

    doc.insert(
        "schema_version",
        toml_edit::value(i64::from(manifest.schema_version)),
    );

    // Platform table
    let mut platform = toml_edit::Table::new();
    platform.insert("kind", toml_edit::value(manifest.platform.kind.as_str()));
    platform.insert(
        "rebuild_command",
        toml_edit::value(&manifest.platform.rebuild_command),
    );
    platform.insert("sudo", toml_edit::value(manifest.platform.sudo));
    platform.insert(
        "flake_root",
        toml_edit::value(&manifest.platform.flake_root),
    );
    doc.insert("platform", toml_edit::Item::Table(platform));

    // Slots as array of tables
    let mut slots = toml_edit::ArrayOfTables::new();
    for slot in &manifest.slots {
        let mut table = toml_edit::Table::new();
        table.insert("kind", toml_edit::value(slot.kind.as_str()));
        table.insert(
            "file",
            toml_edit::value(slot.file.to_string_lossy().as_ref()),
        );
        if !slot.attr_path.is_empty() {
            table.insert("attr_path", toml_edit::value(&slot.attr_path));
        }
        if !slot.tags.is_empty() {
            let mut arr = toml_edit::Array::new();
            for tag in &slot.tags {
                arr.push(tag.as_str());
            }
            table.insert("tags", toml_edit::value(arr));
        }
        if let Some(runtime) = &slot.runtime {
            table.insert("runtime", toml_edit::value(runtime.as_str()));
        }
        if let Some(default_for) = &slot.default_for {
            let mut arr = toml_edit::Array::new();
            for d in default_for {
                arr.push(d.as_str());
            }
            table.insert("default_for", toml_edit::value(arr));
        }
        slots.push(table);
    }
    if !manifest.slots.is_empty() {
        doc.insert("slots", toml_edit::Item::ArrayOfTables(slots));
    }

    // Aliases
    if !manifest.aliases.is_empty() {
        let mut aliases = toml_edit::Table::new();
        let mut keys: Vec<_> = manifest.aliases.keys().collect();
        keys.sort();
        for key in keys {
            aliases.insert(key, toml_edit::value(manifest.aliases[key].as_str()));
        }
        doc.insert("aliases", toml_edit::Item::Table(aliases));
    }

    // Overlays
    if !manifest.overlays.is_empty() {
        let mut overlays = toml_edit::Table::new();
        let mut keys: Vec<_> = manifest.overlays.keys().collect();
        keys.sort();
        for key in keys {
            overlays.insert(key, toml_edit::value(manifest.overlays[key].as_str()));
        }
        doc.insert("overlays", toml_edit::Item::Table(overlays));
    }

    doc.to_string()
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_manifest() -> Manifest {
        Manifest {
            schema_version: CURRENT_SCHEMA_VERSION,
            platform: Manifest::default_darwin(),
            slots: vec![
                Slot {
                    kind: SlotKind::NixPackages,
                    file: PathBuf::from("packages/nix/cli.nix"),
                    attr_path: "home.packages".to_string(),
                    tags: vec!["cli".to_string(), "tools".to_string()],
                    runtime: None,
                    default_for: Some(vec!["install".to_string()]),
                },
                Slot {
                    kind: SlotKind::HomebrewList,
                    file: PathBuf::from("packages/homebrew/casks.nix"),
                    attr_path: "homebrew.casks".to_string(),
                    tags: vec!["gui".to_string()],
                    runtime: None,
                    default_for: None,
                },
                Slot {
                    kind: SlotKind::WithPackages,
                    file: PathBuf::from("packages/nix/languages.nix"),
                    attr_path: "python3.withPackages".to_string(),
                    tags: vec![],
                    runtime: Some("python3".to_string()),
                    default_for: None,
                },
            ],
            aliases: HashMap::from([
                ("vim".to_string(), "neovim".to_string()),
                ("rg".to_string(), "ripgrep".to_string()),
            ]),
            overlays: HashMap::from([("neovim".to_string(), "neovim-nightly-overlay".to_string())]),
        }
    }

    #[test]
    fn round_trip_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let manifest = sample_manifest();

        manifest.save(tmp.path()).unwrap();
        let loaded = Manifest::load(tmp.path()).unwrap().unwrap();

        assert_eq!(loaded.schema_version, manifest.schema_version);
        assert_eq!(loaded.platform.kind, PlatformKind::Darwin);
        assert!(loaded.platform.sudo);
        assert_eq!(loaded.slots.len(), 3);
        assert_eq!(loaded.slots[0].kind, SlotKind::NixPackages);
        assert_eq!(loaded.slots[0].file, PathBuf::from("packages/nix/cli.nix"));
        assert_eq!(loaded.slots[0].attr_path, "home.packages");
        assert_eq!(loaded.slots[0].tags, vec!["cli", "tools"]);
        assert_eq!(
            loaded.slots[0].default_for,
            Some(vec!["install".to_string()])
        );
        assert_eq!(loaded.slots[1].kind, SlotKind::HomebrewList);
        assert_eq!(loaded.slots[2].runtime, Some("python3".to_string()));
        assert_eq!(
            loaded.aliases.get("vim").map(String::as_str),
            Some("neovim")
        );
        assert_eq!(
            loaded.aliases.get("rg").map(String::as_str),
            Some("ripgrep")
        );
        assert_eq!(
            loaded.overlays.get("neovim").map(String::as_str),
            Some("neovim-nightly-overlay")
        );
    }

    #[test]
    fn load_returns_none_when_no_manifest() {
        let tmp = TempDir::new().unwrap();
        let result = Manifest::load(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn save_creates_nx_directory() {
        let tmp = TempDir::new().unwrap();
        let manifest = sample_manifest();
        manifest.save(tmp.path()).unwrap();
        assert!(tmp.path().join(".nx").is_dir());
        assert!(tmp.path().join(".nx/manifest.toml").is_file());
    }

    #[test]
    fn default_install_slot_finds_tagged_slot() {
        let manifest = sample_manifest();
        let slot = manifest.default_install_slot().unwrap();
        assert_eq!(slot.kind, SlotKind::NixPackages);
        assert_eq!(slot.file, PathBuf::from("packages/nix/cli.nix"));
    }

    #[test]
    fn default_install_slot_falls_back_to_first_nix_packages() {
        let manifest = Manifest {
            schema_version: 1,
            platform: Manifest::default_darwin(),
            slots: vec![Slot {
                kind: SlotKind::NixPackages,
                file: PathBuf::from("packages/main.nix"),
                attr_path: "environment.systemPackages".to_string(),
                tags: vec![],
                runtime: None,
                default_for: None,
            }],
            aliases: HashMap::new(),
            overlays: HashMap::new(),
        };
        let slot = manifest.default_install_slot().unwrap();
        assert_eq!(slot.file, PathBuf::from("packages/main.nix"));
    }

    #[test]
    fn slots_by_kind_returns_matching() {
        let manifest = sample_manifest();
        let nix_slots = manifest.slots_by_kind(SlotKind::NixPackages);
        assert_eq!(nix_slots.len(), 1);
        let service_slots = manifest.slots_by_kind(SlotKind::Services);
        assert!(service_slots.is_empty());
    }

    #[test]
    fn platform_kind_round_trip() {
        for kind in [
            PlatformKind::Darwin,
            PlatformKind::NixOS,
            PlatformKind::HomeManager,
            PlatformKind::Custom,
        ] {
            let parsed = PlatformKind::parse(kind.as_str()).unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn slot_kind_round_trip() {
        for kind in [
            SlotKind::NixPackages,
            SlotKind::WithPackages,
            SlotKind::HomebrewList,
            SlotKind::MasApps,
            SlotKind::Services,
        ] {
            let parsed = SlotKind::parse(kind.as_str()).unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn parse_minimal_manifest() {
        let tmp = TempDir::new().unwrap();
        let nx_dir = tmp.path().join(".nx");
        fs::create_dir_all(&nx_dir).unwrap();
        fs::write(nx_dir.join("manifest.toml"), "schema_version = 1\n").unwrap();

        let manifest = Manifest::load(tmp.path()).unwrap().unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.platform.kind, PlatformKind::Darwin);
        assert!(manifest.slots.is_empty());
        assert!(manifest.aliases.is_empty());
    }

    #[test]
    fn parse_unknown_platform_kind_errors() {
        let tmp = TempDir::new().unwrap();
        let nx_dir = tmp.path().join(".nx");
        fs::create_dir_all(&nx_dir).unwrap();
        fs::write(
            nx_dir.join("manifest.toml"),
            "schema_version = 1\n\n[platform]\nkind = \"haiku-os\"\n",
        )
        .unwrap();

        let err = Manifest::load(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("unknown platform kind"));
    }

    #[test]
    fn parse_unknown_slot_kind_errors() {
        let tmp = TempDir::new().unwrap();
        let nx_dir = tmp.path().join(".nx");
        fs::create_dir_all(&nx_dir).unwrap();
        fs::write(
            nx_dir.join("manifest.toml"),
            "schema_version = 1\n\n[[slots]]\nkind = \"magic\"\nfile = \"foo.nix\"\n",
        )
        .unwrap();

        let err = Manifest::load(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("unknown slot kind"));
    }

    #[test]
    fn serialized_manifest_is_valid_toml() {
        let manifest = sample_manifest();
        let content = serialize_manifest(&manifest);
        let _doc: toml_edit::DocumentMut = content.parse().expect("should be valid TOML");
    }

    #[test]
    fn nixos_default_platform() {
        let platform = Manifest::default_nixos();
        assert_eq!(platform.kind, PlatformKind::NixOS);
        assert_eq!(platform.rebuild_command, "nixos-rebuild");
        assert!(platform.sudo);
    }

    #[test]
    fn home_manager_default_platform() {
        let platform = Manifest::default_home_manager();
        assert_eq!(platform.kind, PlatformKind::HomeManager);
        assert!(!platform.sudo);
    }
}
