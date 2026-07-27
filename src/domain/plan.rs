use std::path::PathBuf;

use anyhow::{Result, bail};

use super::config::ConfigFiles;
use super::source::{PackageSource, SourceResult, detect_language_package};

// --- Types

/// Complete shape of a deterministic config-file edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditSpec {
    /// Bare identifier into `home.packages = with pkgs; [ ... ]`
    NixPackages { token: String },
    /// Bare package name inside a `runtime.withPackages (ps: ...)` block.
    WithPackages {
        token: String,
        member: String,
        runtime: String,
    },
    /// Double-quoted string into a homebrew `[ "pkg" ... ]` list
    HomebrewList { token: String },
    /// `"Name" = <id>;` into `masApps = { ... }`
    MasApps { token: String },
}

impl EditSpec {
    pub fn nix_packages(token: impl Into<String>) -> Self {
        Self::NixPackages {
            token: token.into(),
        }
    }

    pub fn with_packages(
        token: impl Into<String>,
        member: impl Into<String>,
        runtime: impl Into<String>,
    ) -> Self {
        Self::WithPackages {
            token: token.into(),
            member: member.into(),
            runtime: runtime.into(),
        }
    }

    pub fn homebrew_list(token: impl Into<String>) -> Self {
        Self::HomebrewList {
            token: token.into(),
        }
    }

    pub fn mas_apps(token: impl Into<String>) -> Self {
        Self::MasApps {
            token: token.into(),
        }
    }

    pub fn token(&self) -> &str {
        match self {
            Self::NixPackages { token }
            | Self::WithPackages { token, .. }
            | Self::HomebrewList { token }
            | Self::MasApps { token } => token,
        }
    }
}

/// A fully-resolved install decision consumed by the editing engine.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub source_result: SourceResult,
    pub target_file: PathBuf,
    pub edit: EditSpec,
    pub routing_warning: Option<String>,
}

// --- Pure Functions

/// Build a deterministic install plan from a source result.
///
/// Routes to the correct target file and edit shape based on source type,
/// language detection, and MCP tool patterns. General nix packages fall back
/// to `cli.nix` with a routing warning; the command layer refines via AI engine.
pub fn build_install_plan(sr: SourceResult, config: &ConfigFiles) -> Result<InstallPlan> {
    // Safety: nix sources with missing attr → hard error
    if sr.source.requires_attr() && sr.attr.is_none() {
        bail!(
            "missing resolved attribute for '{}' (source: {}); refusing unsafe install",
            sr.name,
            sr.source,
        );
    }

    let package_token = install_name(&sr);
    let (target_file, edit, routing_warning) = match sr.source {
        PackageSource::Cask => (
            config.homebrew_casks(),
            EditSpec::homebrew_list(&package_token),
            None,
        ),
        PackageSource::Homebrew => (
            config.homebrew_brews(),
            EditSpec::homebrew_list(&package_token),
            None,
        ),
        PackageSource::Mas => (config.darwin(), EditSpec::mas_apps(&package_token), None),
        _ => {
            if let Some((member, runtime)) = detect_language_package(&package_token) {
                let target = config
                    .with_packages_for(runtime)
                    .filter(|p| p.exists())
                    .unwrap_or_else(|| config.languages());
                (
                    target,
                    EditSpec::with_packages(&package_token, member, runtime),
                    None,
                )
            } else {
                // Deterministic fallback: MCP tools and general nix → cli.nix
                let target = config.packages();
                let warning = if is_mcp_tool(&package_token) {
                    None
                } else {
                    Some(format!(
                        "routed '{}' to fallback {}; needs AI refinement",
                        package_token,
                        target.display(),
                    ))
                };
                (target, EditSpec::nix_packages(&package_token), warning)
            }
        }
    };

    Ok(InstallPlan {
        source_result: sr,
        target_file,
        edit,
        routing_warning,
    })
}

/// Collect nix manifest files that could host a package (for AI routing).
///
/// When a manifest is loaded, returns all `NixPackages` slots.
/// Otherwise, constrains to the fallback manifest's parent directory and
/// excludes the language manifest to preserve routing safety invariants.
pub fn nix_manifest_candidates(config: &ConfigFiles) -> Vec<PathBuf> {
    if let Some(manifest) = config.manifest() {
        return manifest
            .slots_by_kind(super::manifest::SlotKind::NixPackages)
            .into_iter()
            .filter(|slot| slot.attr_path == "home.packages")
            .map(|slot| config.repo_root().join(&slot.file))
            .collect();
    }

    let fallback = config.packages();
    let Some(parent) = fallback.parent() else {
        return vec![fallback];
    };
    let language_manifest = config.languages();

    config
        .all_files()
        .iter()
        .filter(|path| path.parent() == Some(parent) && **path != language_manifest)
        .cloned()
        .collect()
}

/// Detect MCP tool packages by naming convention (`*-mcp` or `mcp-*`).
pub fn is_mcp_tool(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with("-mcp") || lower.starts_with("mcp-")
}

/// Resolve the canonical install token from a source result.
///
/// Prefers `attr` (the resolved nix attribute) over the search `name`.
fn install_name(sr: &SourceResult) -> String {
    sr.attr.clone().unwrap_or_else(|| sr.name.clone())
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::ConfigFiles;
    use crate::domain::source::{PackageSource, SourceResult};
    use std::fs;
    use tempfile::TempDir;

    fn write_nix(dir: &std::path::Path, rel_path: &str, content: &str) {
        let full = dir.join(rel_path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
    }

    fn test_config() -> (TempDir, ConfigFiles) {
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
        write_nix(root, "packages/nix/editors.nix", "# nx: editors\n[]");
        write_nix(
            root,
            "packages/homebrew/brews.nix",
            "# nx: formula manifest\n[]",
        );
        write_nix(
            root,
            "packages/homebrew/casks.nix",
            "# nx: cask manifest\n[]",
        );
        write_nix(root, "system/darwin.nix", "# nx: macos system\n{}");
        write_nix(root, "home/services.nix", "# nx: services\n{}");

        let cf = ConfigFiles::discover(root);
        (tmp, cf)
    }

    fn sr(name: &str, source: PackageSource, attr: Option<&str>) -> SourceResult {
        SourceResult {
            attr: attr.map(String::from),
            ..SourceResult::new(name, source)
        }
    }

    // --- Routing: cask → casks.nix

    #[test]
    fn route_cask_to_casks_file() {
        let (_tmp, config) = test_config();
        let plan = build_install_plan(sr("firefox", PackageSource::Cask, Some("firefox")), &config)
            .unwrap();
        assert_eq!(plan.edit, EditSpec::homebrew_list("firefox"));
        assert!(plan.target_file.ends_with("packages/homebrew/casks.nix"));
        assert_eq!(plan.source_result.source, PackageSource::Cask);
    }

    // --- Routing: brew → brews.nix

    #[test]
    fn route_brew_to_brews_file() {
        let (_tmp, config) = test_config();
        let plan =
            build_install_plan(sr("htop", PackageSource::Homebrew, Some("htop")), &config).unwrap();
        assert_eq!(plan.edit, EditSpec::homebrew_list("htop"));
        assert!(plan.target_file.ends_with("packages/homebrew/brews.nix"));
        assert_eq!(plan.source_result.source, PackageSource::Homebrew);
    }

    // --- Routing: mas → darwin.nix

    #[test]
    fn route_mas_to_darwin() {
        let (_tmp, config) = test_config();
        let plan =
            build_install_plan(sr("Xcode", PackageSource::Mas, Some("Xcode")), &config).unwrap();
        assert_eq!(plan.edit, EditSpec::mas_apps("Xcode"));
        assert!(plan.target_file.ends_with("system/darwin.nix"));
        assert_eq!(plan.source_result.source, PackageSource::Mas);
    }

    // --- Routing: language → languages.nix

    #[test]
    fn route_python_package_to_languages() {
        let (_tmp, config) = test_config();
        let result = sr("pyyaml", PackageSource::Nxs, Some("python3Packages.pyyaml"));
        let plan = build_install_plan(result, &config).unwrap();
        assert_eq!(
            plan.edit,
            EditSpec::with_packages("python3Packages.pyyaml", "pyyaml", "python3")
        );
        assert!(plan.target_file.ends_with("packages/nix/languages.nix"));
    }

    #[test]
    fn route_lua_package_to_languages() {
        let (_tmp, config) = test_config();
        let result = sr("lpeg", PackageSource::Nxs, Some("luaPackages.lpeg"));
        let plan = build_install_plan(result, &config).unwrap();
        assert_eq!(
            plan.edit,
            EditSpec::with_packages("luaPackages.lpeg", "lpeg", "lua5_4")
        );
    }

    // --- Routing: MCP tool → cli.nix (no warning)

    #[test]
    fn route_mcp_tool_to_cli_no_warning() {
        let (_tmp, config) = test_config();
        let result = sr("server-mcp", PackageSource::Nxs, Some("server-mcp"));
        let plan = build_install_plan(result, &config).unwrap();
        assert_eq!(plan.edit, EditSpec::nix_packages("server-mcp"));
        assert!(plan.target_file.ends_with("packages/nix/cli.nix"));
        assert!(plan.routing_warning.is_none());
    }

    #[test]
    fn route_mcp_prefix_to_cli_no_warning() {
        let (_tmp, config) = test_config();
        let result = sr("mcp-server-git", PackageSource::Nxs, Some("mcp-server-git"));
        let plan = build_install_plan(result, &config).unwrap();
        assert!(plan.routing_warning.is_none());
    }

    // --- Routing: general nix → cli.nix (with warning)

    #[test]
    fn route_general_nix_to_cli_with_warning() {
        let (_tmp, config) = test_config();
        let result = sr("ripgrep", PackageSource::Nxs, Some("ripgrep"));
        let plan = build_install_plan(result, &config).unwrap();
        assert_eq!(plan.edit, EditSpec::nix_packages("ripgrep"));
        assert!(plan.target_file.ends_with("packages/nix/cli.nix"));
        assert!(plan.routing_warning.is_some());
        assert!(plan.routing_warning.as_ref().unwrap().contains("fallback"));
    }

    // --- Safety: missing attr for nix sources

    #[test]
    fn safety_nxs_missing_attr_errors() {
        let (_tmp, config) = test_config();
        let result = sr("ripgrep", PackageSource::Nxs, None);
        assert!(build_install_plan(result, &config).is_err());
    }

    #[test]
    fn safety_nur_missing_attr_errors() {
        let (_tmp, config) = test_config();
        let result = sr("pkg", PackageSource::Nur, None);
        assert!(build_install_plan(result, &config).is_err());
    }

    #[test]
    fn safety_flake_input_missing_attr_errors() {
        let (_tmp, config) = test_config();
        let result = sr("rust", PackageSource::FlakeInput, None);
        assert!(build_install_plan(result, &config).is_err());
    }

    // --- edit token resolution

    #[test]
    fn edit_token_prefers_attr() {
        let (_tmp, config) = test_config();
        let result = sr("rg", PackageSource::Nxs, Some("ripgrep"));
        let plan = build_install_plan(result, &config).unwrap();
        assert_eq!(plan.edit.token(), "ripgrep");
    }

    #[test]
    fn edit_token_falls_back_to_name() {
        let (_tmp, config) = test_config();
        let result = sr("firefox", PackageSource::Cask, None);
        let plan = build_install_plan(result, &config).unwrap();
        assert_eq!(plan.edit.token(), "firefox");
    }

    // --- is_mcp_tool

    #[test]
    fn mcp_suffix_detected() {
        assert!(is_mcp_tool("server-mcp"));
        assert!(is_mcp_tool("lua-mcp"));
    }

    #[test]
    fn mcp_prefix_detected() {
        assert!(is_mcp_tool("mcp-server-git"));
        assert!(is_mcp_tool("MCP-tools"));
    }

    #[test]
    fn mcp_not_detected_for_regular_packages() {
        assert!(!is_mcp_tool("ripgrep"));
        assert!(!is_mcp_tool("mcptools"));
        assert!(!is_mcp_tool("amcp"));
    }

    // --- nix_manifest_candidates

    #[test]
    fn manifest_candidates_filter_to_home_packages_only() {
        use crate::domain::manifest::{Manifest, PlatformConfig, PlatformKind, Slot, SlotKind};
        use std::collections::HashMap;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(root, "modules/cli.nix", "# cli tools\n[]");
        write_nix(root, "modules/editors.nix", "# editors\n[]");
        write_nix(root, "system/darwin.nix", "# system config\n{}");

        let manifest = Manifest {
            schema_version: 1,
            platform: PlatformConfig {
                kind: PlatformKind::Darwin,
                rebuild_command: "darwin-rebuild".to_string(),
                sudo: true,
                flake_root: ".".to_string(),
                split_rebuild: false,
            },
            slots: vec![
                Slot {
                    kind: SlotKind::NixPackages,
                    file: "modules/cli.nix".into(),
                    attr_path: "home.packages".to_string(),
                    tags: vec!["cli".to_string()],
                    runtime: None,
                    default_for: Some(vec!["install".to_string()]),
                },
                Slot {
                    kind: SlotKind::NixPackages,
                    file: "modules/editors.nix".into(),
                    attr_path: "home.packages".to_string(),
                    tags: vec!["editors".to_string()],
                    runtime: None,
                    default_for: None,
                },
                Slot {
                    kind: SlotKind::NixPackages,
                    file: "system/darwin.nix".into(),
                    attr_path: "environment.systemPackages".to_string(),
                    tags: vec![],
                    runtime: None,
                    default_for: None,
                },
            ],
            aliases: HashMap::new(),
            overlays: HashMap::new(),
        };

        let config = ConfigFiles::from_manifest(&manifest, root);
        let candidates = nix_manifest_candidates(&config);

        // Only home.packages slots returned — darwin.nix excluded
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|p| p.ends_with("modules/cli.nix")));
        assert!(
            candidates
                .iter()
                .any(|p| p.ends_with("modules/editors.nix"))
        );
        assert!(!candidates.iter().any(|p| p.ends_with("system/darwin.nix")));
    }

    #[test]
    fn candidates_include_cli_siblings_and_exclude_languages() {
        let (tmp, config) = test_config();
        let candidates = nix_manifest_candidates(&config);
        assert!(
            candidates
                .iter()
                .all(|p| p.starts_with(tmp.path().join("packages/nix")))
        );
        assert!(
            candidates
                .iter()
                .any(|p| p.ends_with("packages/nix/cli.nix"))
        );
        assert!(
            candidates
                .iter()
                .any(|p| p.ends_with("packages/nix/editors.nix"))
        );
        assert!(
            !candidates
                .iter()
                .any(|p| p.ends_with("packages/nix/languages.nix"))
        );
        assert!(candidates.iter().all(|p| p.extension().unwrap() == "nix"));
    }
}
