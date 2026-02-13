// No consumers yet — downstream commands wire in via .12/.13
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::LazyLock;

/// Result from searching a package source.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceResult {
    pub name: String,
    pub source: String,
    pub attr: Option<String>,
    pub version: Option<String>,
    pub confidence: f64,
    pub description: String,
    pub requires_flake_mod: bool,
    pub flake_url: Option<String>,
}

impl SourceResult {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
            attr: None,
            version: None,
            confidence: 0.0,
            description: String::new(),
            requires_flake_mod: false,
            flake_url: None,
        }
    }
}

/// Case-insensitive alias map: common names → canonical nix attribute names.
static NAME_MAPPINGS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        // Numeric prefix packages
        ("1password-cli", "_1password-cli"),
        ("1password", "_1password-gui"),
        // Editor aliases
        ("nvim", "neovim"),
        ("vim", "neovim"),
        // Python aliases
        ("python", "python3"),
        ("python3", "python3"),
        ("py-yaml", "pyyaml"),
        ("py_yaml", "pyyaml"),
        // Node aliases
        ("node", "nodejs"),
        ("nodejs", "nodejs"),
        // Tool aliases
        ("rg", "ripgrep"),
        ("fd-find", "fd"),
        // GNU tools
        ("grep", "gnugrep"),
        ("sed", "gnused"),
        ("make", "gnumake"),
        ("tar", "gnutar"),
        ("find", "findutils"),
    ])
});

/// Normalize a package name through alias mapping (case-insensitive).
pub fn normalize_name(name: &str) -> String {
    let lower = name.to_lowercase();
    match NAME_MAPPINGS.get(lower.as_str()) {
        Some(mapped) => mapped.to_lowercase(),
        None => lower,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_aliases() {
        assert_eq!(normalize_name("py-yaml"), "pyyaml");
        assert_eq!(normalize_name("py_yaml"), "pyyaml");
        assert_eq!(normalize_name("nvim"), "neovim");
        assert_eq!(normalize_name("rg"), "ripgrep");
        assert_eq!(normalize_name("1password"), "_1password-gui");
    }

    #[test]
    fn normalize_passthrough() {
        assert_eq!(normalize_name("ripgrep"), "ripgrep");
        assert_eq!(normalize_name("firefox"), "firefox");
    }

    #[test]
    fn normalize_is_case_insensitive() {
        assert_eq!(normalize_name("Nvim"), "neovim");
        assert_eq!(normalize_name("PY-YAML"), "pyyaml");
    }
}
