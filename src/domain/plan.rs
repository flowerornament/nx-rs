use std::path::PathBuf;

use anyhow::{Result, bail};

use super::config::ConfigFiles;
use super::source::{SourceResult, detect_language_package};

// --- Types

/// How a package should be inserted into a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertionMode {
    /// Bare identifier into `home.packages = with pkgs; [ ... ]`
    NixManifest,
    /// Bare name into a `runtime.withPackages (ps: ...)` block
    LanguageWithPackages,
    /// Double-quoted string into a homebrew `[ "pkg" ... ]` list
    HomebrewManifest,
    /// `"Name" = <id>;` into `masApps = { ... }`
    MasApps,
}

/// Language-specific routing metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageInfo {
    pub bare_name: String,
    pub runtime: String,
    pub method: String,
}

/// A fully-resolved install decision consumed by the editing engine.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub source_result: SourceResult,
    pub package_token: String,
    pub target_file: PathBuf,
    pub insertion_mode: InsertionMode,
    #[allow(dead_code)] // consumed by ai_engine::build_edit_prompt (.15)
    pub is_brew: bool,
    #[allow(dead_code)] // consumed by ai_engine::build_edit_prompt (.15)
    pub is_cask: bool,
    #[allow(dead_code)] // consumed by ai_engine::build_edit_prompt (.15)
    pub is_mas: bool,
    pub language_info: Option<LanguageInfo>,
    pub routing_warning: Option<String>,
}

// --- Pure Functions

/// Build a deterministic install plan from a source result.
///
/// Routes to the correct target file and insertion mode based on source type,
/// language detection, and MCP tool patterns. General nix packages fall back
/// to `cli.nix` with a routing warning; the command layer refines via AI engine.
pub fn build_install_plan(sr: &SourceResult, config: &ConfigFiles) -> Result<InstallPlan> {
    // Safety: nxs/nur/flake-input with missing attr → hard error
    if matches!(sr.source.as_str(), "nxs" | "nur" | "flake-input") && sr.attr.is_none() {
        bail!(
            "missing resolved attribute for '{}' (source: {}); refusing unsafe install",
            sr.name,
            sr.source,
        );
    }

    let package_token = install_name(sr);
    let is_cask = sr.source == "cask";
    let is_brew = sr.source == "homebrew" || sr.source == "brew";
    let is_mas = sr.source == "mas";
    let language_info =
        detect_language_package(&package_token).map(|(bare, runtime, method)| LanguageInfo {
            bare_name: bare.to_string(),
            runtime: runtime.to_string(),
            method: method.to_string(),
        });

    let (target_file, insertion_mode, routing_warning) = if is_cask {
        (
            config.homebrew_casks(),
            InsertionMode::HomebrewManifest,
            None,
        )
    } else if is_brew {
        (
            config.homebrew_brews(),
            InsertionMode::HomebrewManifest,
            None,
        )
    } else if is_mas {
        (config.darwin(), InsertionMode::MasApps, None)
    } else if language_info.is_some() {
        (
            config.languages(),
            InsertionMode::LanguageWithPackages,
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
        (target, InsertionMode::NixManifest, warning)
    };

    Ok(InstallPlan {
        source_result: sr.clone(),
        package_token,
        target_file,
        insertion_mode,
        is_brew,
        is_cask,
        is_mas,
        language_info,
        routing_warning,
    })
}

/// Collect nix manifest files that could host a package (for AI routing).
pub fn nix_manifest_candidates(config: &ConfigFiles) -> Vec<PathBuf> {
    config
        .all_files()
        .iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e == "nix")
        })
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
    use crate::domain::source::SourceResult;
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

    fn sr(name: &str, source: &str, attr: Option<&str>) -> SourceResult {
        SourceResult {
            attr: attr.map(String::from),
            ..SourceResult::new(name, source)
        }
    }

    // --- Routing: cask → casks.nix

    #[test]
    fn route_cask_to_casks_file() {
        let (_tmp, config) = test_config();
        let plan = build_install_plan(&sr("firefox", "cask", Some("firefox")), &config).unwrap();
        assert_eq!(plan.insertion_mode, InsertionMode::HomebrewManifest);
        assert!(plan.target_file.ends_with("packages/homebrew/casks.nix"));
        assert!(plan.is_cask);
        assert!(!plan.is_brew);
    }

    // --- Routing: brew → brews.nix

    #[test]
    fn route_brew_to_brews_file() {
        let (_tmp, config) = test_config();
        let plan = build_install_plan(&sr("htop", "homebrew", Some("htop")), &config).unwrap();
        assert_eq!(plan.insertion_mode, InsertionMode::HomebrewManifest);
        assert!(plan.target_file.ends_with("packages/homebrew/brews.nix"));
        assert!(plan.is_brew);
    }

    #[test]
    fn route_brew_source_variant() {
        let (_tmp, config) = test_config();
        let plan = build_install_plan(&sr("htop", "brew", Some("htop")), &config).unwrap();
        assert!(plan.is_brew);
    }

    // --- Routing: mas → darwin.nix

    #[test]
    fn route_mas_to_darwin() {
        let (_tmp, config) = test_config();
        let plan = build_install_plan(&sr("Xcode", "mas", Some("Xcode")), &config).unwrap();
        assert_eq!(plan.insertion_mode, InsertionMode::MasApps);
        assert!(plan.target_file.ends_with("system/darwin.nix"));
        assert!(plan.is_mas);
    }

    // --- Routing: language → languages.nix

    #[test]
    fn route_python_package_to_languages() {
        let (_tmp, config) = test_config();
        let result = sr("pyyaml", "nxs", Some("python3Packages.pyyaml"));
        let plan = build_install_plan(&result, &config).unwrap();
        assert_eq!(plan.insertion_mode, InsertionMode::LanguageWithPackages);
        assert!(plan.target_file.ends_with("packages/nix/languages.nix"));
        let lang = plan.language_info.as_ref().unwrap();
        assert_eq!(lang.bare_name, "pyyaml");
        assert_eq!(lang.runtime, "python3");
    }

    #[test]
    fn route_lua_package_to_languages() {
        let (_tmp, config) = test_config();
        let result = sr("lpeg", "nxs", Some("luaPackages.lpeg"));
        let plan = build_install_plan(&result, &config).unwrap();
        assert_eq!(plan.insertion_mode, InsertionMode::LanguageWithPackages);
        let lang = plan.language_info.as_ref().unwrap();
        assert_eq!(lang.bare_name, "lpeg");
        assert_eq!(lang.runtime, "lua5_4");
    }

    // --- Routing: MCP tool → cli.nix (no warning)

    #[test]
    fn route_mcp_tool_to_cli_no_warning() {
        let (_tmp, config) = test_config();
        let result = sr("server-mcp", "nxs", Some("server-mcp"));
        let plan = build_install_plan(&result, &config).unwrap();
        assert_eq!(plan.insertion_mode, InsertionMode::NixManifest);
        assert!(plan.target_file.ends_with("packages/nix/cli.nix"));
        assert!(plan.routing_warning.is_none());
    }

    #[test]
    fn route_mcp_prefix_to_cli_no_warning() {
        let (_tmp, config) = test_config();
        let result = sr("mcp-server-git", "nxs", Some("mcp-server-git"));
        let plan = build_install_plan(&result, &config).unwrap();
        assert!(plan.routing_warning.is_none());
    }

    // --- Routing: general nix → cli.nix (with warning)

    #[test]
    fn route_general_nix_to_cli_with_warning() {
        let (_tmp, config) = test_config();
        let result = sr("ripgrep", "nxs", Some("ripgrep"));
        let plan = build_install_plan(&result, &config).unwrap();
        assert_eq!(plan.insertion_mode, InsertionMode::NixManifest);
        assert!(plan.target_file.ends_with("packages/nix/cli.nix"));
        assert!(plan.routing_warning.is_some());
        assert!(plan.routing_warning.as_ref().unwrap().contains("fallback"));
    }

    // --- Safety: missing attr for nix sources

    #[test]
    fn safety_nxs_missing_attr_errors() {
        let (_tmp, config) = test_config();
        let result = sr("ripgrep", "nxs", None);
        assert!(build_install_plan(&result, &config).is_err());
    }

    #[test]
    fn safety_nur_missing_attr_errors() {
        let (_tmp, config) = test_config();
        let result = sr("pkg", "nur", None);
        assert!(build_install_plan(&result, &config).is_err());
    }

    #[test]
    fn safety_flake_input_missing_attr_errors() {
        let (_tmp, config) = test_config();
        let result = sr("rust", "flake-input", None);
        assert!(build_install_plan(&result, &config).is_err());
    }

    // --- package_token resolution

    #[test]
    fn package_token_prefers_attr() {
        let (_tmp, config) = test_config();
        let result = sr("rg", "nxs", Some("ripgrep"));
        let plan = build_install_plan(&result, &config).unwrap();
        assert_eq!(plan.package_token, "ripgrep");
    }

    #[test]
    fn package_token_falls_back_to_name() {
        let (_tmp, config) = test_config();
        let result = sr("firefox", "cask", None);
        let plan = build_install_plan(&result, &config).unwrap();
        assert_eq!(plan.package_token, "firefox");
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
    fn candidates_lists_all_nix_files() {
        let (_tmp, config) = test_config();
        let candidates = nix_manifest_candidates(&config);
        assert!(candidates.len() >= 5);
        assert!(candidates.iter().all(|p| p.extension().unwrap() == "nix"));
    }
}
