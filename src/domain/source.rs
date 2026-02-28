use std::collections::HashMap;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

// --- Types

/// The source a package was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageSource {
    Nxs,
    Nur,
    FlakeInput,
    Homebrew,
    Cask,
    Mas,
}

impl fmt::Display for PackageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PackageSource {
    /// Canonical string form used in display, cache keys, and JSON output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nxs => "nxs",
            Self::Nur => "nur",
            Self::FlakeInput => "flake-input",
            Self::Homebrew => "homebrew",
            Self::Cask => "cask",
            Self::Mas => "mas",
        }
    }

    /// Parse from user-facing or serialized string (case-insensitive).
    #[allow(dead_code)] // retained for source parsing from external cache/CLI strings
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "nxs" => Some(Self::Nxs),
            "nur" => Some(Self::Nur),
            "flake-input" => Some(Self::FlakeInput),
            "homebrew" | "brew" => Some(Self::Homebrew),
            "cask" => Some(Self::Cask),
            "mas" => Some(Self::Mas),
            _ => None,
        }
    }

    /// Whether this source requires a resolved nix attribute.
    pub const fn requires_attr(self) -> bool {
        matches!(self, Self::Nxs | Self::Nur | Self::FlakeInput)
    }
}

/// Result from searching a package source.
#[derive(Debug, Clone)]
pub struct SourceResult {
    pub name: String,
    pub source: PackageSource,
    pub attr: Option<String>,
    pub version: Option<String>,
    pub confidence: f64,
    pub description: String,
    pub requires_flake_mod: bool,
    pub flake_url: Option<String>,
}

impl SourceResult {
    pub fn new(name: impl Into<String>, source: PackageSource) -> Self {
        Self {
            name: name.into(),
            source,
            attr: None,
            version: None,
            confidence: 0.0,
            description: String::new(),
            requires_flake_mod: false,
            flake_url: None,
        }
    }
}

/// User preferences for source selection.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // Source selection is modeled as orthogonal switches.
pub struct SourcePreferences {
    pub bleeding_edge: bool,
    pub nur: bool,
    pub force_source: Option<String>,
    pub is_cask: bool,
    pub is_mas: bool,
}

/// Typed intermediate from `nix search --json` output.
#[derive(Debug, Clone)]
pub struct NixSearchEntry {
    pub attr_path: String,
    pub pname: String,
    pub version: String,
    pub description: String,
}

// --- Constants

/// Case-insensitive alias map: common names -> canonical nix attribute names.
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

/// Language package prefixes that need `withPackages` treatment.
/// Maps attr prefix -> (runtime, method).
pub const LANG_PACKAGE_PREFIXES: &[(&str, &str, &str)] = &[
    ("python3Packages.", "python3", "withPackages"),
    ("python311Packages.", "python3", "withPackages"),
    ("python312Packages.", "python3", "withPackages"),
    ("python313Packages.", "python3", "withPackages"),
    ("python314Packages.", "python3", "withPackages"),
    ("luaPackages.", "lua5_4", "withPackages"),
    ("lua51Packages.", "lua5_1", "withPackages"),
    ("lua52Packages.", "lua5_2", "withPackages"),
    ("lua53Packages.", "lua5_3", "withPackages"),
    ("lua54Packages.", "lua5_4", "withPackages"),
    ("perlPackages.", "perl", "withPackages"),
    ("rubyPackages.", "ruby", "withPackages"),
    ("haskellPackages.", "haskellPackages.ghc", "withPackages"),
];

/// Known overlays and the packages they replace/provide.
/// Maps `package_name` -> `(overlay_name, attr_in_overlay, description)`.
pub static OVERLAY_PACKAGES: LazyLock<
    HashMap<&'static str, (&'static str, &'static str, &'static str)>,
> = LazyLock::new(|| {
    HashMap::from([
        (
            "neovim",
            ("neovim-nightly-overlay", "default", "Neovim nightly build"),
        ),
        (
            "nvim",
            ("neovim-nightly-overlay", "default", "Neovim nightly build"),
        ),
        (
            "rust",
            ("fenix", "default.toolchain", "Rust nightly toolchain"),
        ),
        (
            "cargo",
            ("fenix", "default.toolchain", "Rust nightly toolchain"),
        ),
        (
            "rustc",
            ("fenix", "default.toolchain", "Rust nightly toolchain"),
        ),
        (
            "rust-analyzer",
            ("fenix", "rust-analyzer", "Rust analyzer nightly"),
        ),
        (
            "emacs",
            ("emacs-overlay", "emacs-git", "Emacs from git master"),
        ),
        ("zig", ("zig-overlay", "master", "Zig nightly build")),
        (
            "firefox",
            ("nxs-mozilla", "firefox-nightly-bin", "Firefox Nightly"),
        ),
        (
            "firefox-nightly",
            ("nxs-mozilla", "firefox-nightly-bin", "Firefox Nightly"),
        ),
        (
            "rust-bin",
            ("rust-overlay", "rust", "Rust from rust-overlay"),
        ),
    ])
});

// --- Pure Functions

/// Normalize a package name through alias mapping (case-insensitive).
pub fn normalize_name(name: &str) -> String {
    let lower = name.to_lowercase();
    NAME_MAPPINGS
        .get(lower.as_str())
        .map_or(lower, |mapped| mapped.to_lowercase())
}

/// Resolve common aliases case-insensitively (returns mapped or original).
pub fn mapped_name(name: &str) -> String {
    let lower = name.to_lowercase();
    NAME_MAPPINGS
        .get(lower.as_str())
        .map_or_else(|| name.to_string(), |mapped| (*mapped).to_string())
}

/// Detect if a package is a language-specific package.
///
/// Returns `(bare_name, runtime, method)` or `None`.
pub fn detect_language_package(name: &str) -> Option<(&str, &str, &str)> {
    for &(prefix, runtime, method) in LANG_PACKAGE_PREFIXES {
        if let Some(bare) = name.strip_prefix(prefix)
            && !bare.is_empty()
        {
            return Some((bare, runtime, method));
        }
    }
    None
}

/// Strip the `legacyPackages.<arch>` prefix from a nix attribute path.
pub fn clean_attr_path(attr: &str) -> &str {
    if let Some(rest) = attr.strip_prefix("legacyPackages.") {
        // Skip the arch segment: find the second dot after the prefix
        if let Some(idx) = rest.find('.') {
            return &rest[idx + 1..];
        }
    }
    attr
}

/// Strip non-alphanumeric characters for normalized comparison.
fn strip_separators(s: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
    RE.replace_all(&s.to_lowercase(), "").into_owned()
}

// --- Scoring constants for score_match ---
//
// Hierarchy: exact pname > exact tail > case-insensitive > prefix > substring.
// Root-level packages score higher than nested ones (e.g. pkgs.redis > pkgs.foo.redis).

/// Exact pname match at root level (best possible).
const SCORE_EXACT_ROOT_PNAME: f64 = 1.0;
/// Exact tail match at root level.
const SCORE_EXACT_ROOT_TAIL: f64 = 0.98;
/// Exact tail match, normalized, at root level.
const SCORE_NORM_ROOT_TAIL: f64 = 0.95;
/// Exact pname/norm match when nested.
const SCORE_EXACT_NESTED_PNAME: f64 = 0.85;
/// Exact tail/norm match when nested.
const SCORE_EXACT_NESTED_TAIL: f64 = 0.82;
/// Exact tail match, case-insensitive only.
const SCORE_TAIL_CASE_INSENSITIVE: f64 = 0.80;
/// Case-insensitive exact tail match.
const SCORE_CASE_INSENSITIVE: f64 = 0.75;
/// Normalized prefix match.
const SCORE_NORM_PREFIX: f64 = 0.68;
/// Case-sensitive prefix match.
const SCORE_PREFIX: f64 = 0.65;
/// Case-insensitive prefix match.
const SCORE_PREFIX_CI: f64 = 0.60;
/// Normalized substring match.
const SCORE_NORM_SUBSTRING: f64 = 0.52;
/// Case-insensitive substring match.
const SCORE_SUBSTRING_CI: f64 = 0.45;
/// Minimum score: no exact/prefix/substring match found.
const SCORE_FLOOR: f64 = 0.3;
/// Maximum nesting penalty.
const MAX_NESTING_PENALTY: f64 = 0.3;
/// Per-level nesting penalty.
const NESTING_PENALTY_PER_LEVEL: f64 = 0.1;

/// Score how well an attribute matches the search name.
///
/// Prefers root-level packages (pkgs.redis) over nested ones
/// (`pkgs.chickenPackages.eggs.redis`). Returns 0.0-1.0.
pub fn score_match(search_name: &str, attr: &str, pname: &str) -> f64 {
    let parts: Vec<&str> = attr.split('.').collect();
    let tail = parts.last().copied().unwrap_or(attr);

    let is_root = if attr.starts_with("legacyPackages.") {
        parts.len() == 3
    } else {
        parts.len() == 1
    };

    let nesting_penalty = if is_root {
        0.0
    } else {
        let depth = if attr.starts_with("legacyPackages.") {
            parts.len().saturating_sub(3)
        } else {
            parts.len().saturating_sub(1)
        };
        #[allow(clippy::cast_precision_loss)] // depth is always small (< 10)
        f64::min(
            MAX_NESTING_PENALTY,
            depth as f64 * NESTING_PENALTY_PER_LEVEL,
        )
    };

    let search_lower = search_name.to_lowercase();
    let tail_lower = tail.to_lowercase();
    let search_norm = strip_separators(search_name);
    let tail_norm = strip_separators(tail);
    let pname_norm = strip_separators(pname);

    // Exact matches (highest priority, checked first)
    let exact_score: f64 = if pname == search_name {
        if is_root {
            SCORE_EXACT_ROOT_PNAME
        } else {
            SCORE_EXACT_NESTED_PNAME
        }
    } else if tail == search_name {
        if is_root {
            SCORE_EXACT_ROOT_TAIL
        } else {
            SCORE_TAIL_CASE_INSENSITIVE
        }
    } else if tail_lower == search_lower {
        SCORE_CASE_INSENSITIVE
    } else if tail.starts_with(search_name) {
        SCORE_PREFIX
    } else if tail_lower.starts_with(&search_lower) {
        SCORE_PREFIX_CI
    } else if tail_lower.contains(&search_lower) {
        SCORE_SUBSTRING_CI
    } else {
        SCORE_FLOOR
    };

    // Separator-normalized comparison (e.g. py-yaml vs pyyaml)
    let norm_score = if search_norm.is_empty() {
        0.0
    } else if pname_norm == search_norm {
        if is_root {
            SCORE_EXACT_ROOT_PNAME
        } else {
            SCORE_EXACT_NESTED_PNAME
        }
    } else if tail_norm == search_norm {
        if is_root {
            SCORE_NORM_ROOT_TAIL
        } else {
            SCORE_EXACT_NESTED_TAIL
        }
    } else if tail_norm.starts_with(&search_norm) {
        SCORE_NORM_PREFIX
    } else if tail_norm.contains(&search_norm) {
        SCORE_NORM_SUBSTRING
    } else {
        0.0
    };

    exact_score.max(norm_score) - nesting_penalty
}

/// Extract a `NixSearchEntry` from a JSON object map.
fn entry_from_obj(obj: &serde_json::Map<String, Value>, fallback_attr: &str) -> NixSearchEntry {
    let str_field = |key| {
        obj.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    NixSearchEntry {
        attr_path: obj
            .get("attrPath")
            .and_then(Value::as_str)
            .unwrap_or(fallback_attr)
            .to_string(),
        pname: str_field("pname"),
        version: str_field("version"),
        description: str_field("description"),
    }
}

/// Parse nix search JSON output into typed entries.
///
/// Handles both dict format (`attrPath -> {pname, description, ...}`)
/// and list format.
pub fn parse_nix_search_results(data: &Value) -> Vec<NixSearchEntry> {
    match data {
        Value::Object(map) => map
            .iter()
            .map(|(key, val)| {
                let obj = val.as_object();
                obj.map_or_else(
                    || entry_from_obj(&serde_json::Map::new(), key),
                    |o| entry_from_obj(o, key),
                )
            })
            .collect(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(Value::as_object)
            .map(|obj| entry_from_obj(obj, ""))
            .collect(),
        _ => Vec::new(),
    }
}

/// Generate search name variants: mapped name, original, compact (max 3).
pub fn search_name_variants(name: &str) -> Vec<String> {
    let mut variants = Vec::with_capacity(3);
    let mapped = mapped_name(name);

    for candidate in [&mapped, &name.to_string()] {
        if !candidate.is_empty() && !variants.contains(candidate) {
            variants.push(candidate.clone());
        }
        let compact = strip_separators(candidate);
        if !compact.is_empty() && !variants.contains(&compact) {
            variants.push(compact);
        }
    }

    variants.truncate(3);
    variants
}

/// Return the current Nix system identifier (e.g., `aarch64-darwin`).
pub const fn get_current_system() -> &'static str {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64-darwin"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        "x86_64-darwin"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64-linux"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64-linux"
    }
}

/// Sort results in-place by source priority and confidence.
pub fn sort_results(results: &mut [SourceResult], prefs: &SourcePreferences) {
    let priority = |source: PackageSource| -> u8 {
        if prefs.bleeding_edge {
            match source {
                PackageSource::FlakeInput => 0,
                PackageSource::Nur => 1,
                PackageSource::Nxs => 2,
                PackageSource::Homebrew => 3,
                PackageSource::Cask => 4,
                PackageSource::Mas => 5,
            }
        } else {
            match source {
                PackageSource::FlakeInput => 0,
                PackageSource::Nxs => 1,
                PackageSource::Nur => 2,
                PackageSource::Homebrew => 3,
                PackageSource::Cask => 4,
                PackageSource::Mas => 5,
            }
        }
    };

    results.sort_by(|a, b| {
        let pa = priority(a.source);
        let pb = priority(b.source);
        pa.cmp(&pb).then_with(|| {
            // Higher confidence first (reverse order)
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
}

/// Remove duplicate results by `(source, attr)`, preserving order.
pub fn deduplicate_results(results: Vec<SourceResult>) -> Vec<SourceResult> {
    let mut seen = std::collections::HashSet::new();
    results
        .into_iter()
        .filter(|r| seen.insert((r.source, r.attr.clone())))
        .collect()
}

/// Check if a platform list includes the current system.
///
/// Returns `(available, reason)`. Permissive when platforms is not a
/// string list or is empty.
pub fn check_platforms(platforms: &Value, current_system: &str) -> (bool, Option<String>) {
    let arr = match platforms.as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return (true, None),
    };

    let strings: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();

    // If no entries are plain strings, treat permissively
    if strings.is_empty() {
        return (true, None);
    }

    if strings.contains(&current_system) {
        (true, None)
    } else {
        (
            false,
            Some(format!(
                "not available on {current_system} (only: {})",
                strings.join(", ")
            )),
        )
    }
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- normalize_name ---

    #[test]
    fn normalize_aliases() {
        assert_eq!(normalize_name("py-yaml"), "pyyaml");
        assert_eq!(normalize_name("py_yaml"), "pyyaml");
        assert_eq!(normalize_name("nvim"), "neovim");
        assert_eq!(normalize_name("python"), "python3");
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

    // --- score_match ---

    #[test]
    fn score_match_exact_root() {
        let s = score_match(
            "ripgrep",
            "legacyPackages.aarch64-darwin.ripgrep",
            "ripgrep",
        );
        assert!(
            (s - 1.0).abs() < f64::EPSILON,
            "exact root pname match should be 1.0, got {s}"
        );
    }

    #[test]
    fn score_match_nested_penalty() {
        let root = score_match("redis", "legacyPackages.aarch64-darwin.redis", "redis");
        let nested = score_match(
            "redis",
            "legacyPackages.aarch64-darwin.chickenPackages.eggs.redis",
            "redis",
        );
        assert!(
            root > nested,
            "root ({root}) should score higher than nested ({nested})"
        );
    }

    #[test]
    fn score_match_separator_normalization() {
        let s = score_match("py-yaml", "legacyPackages.aarch64-darwin.pyyaml", "pyyaml");
        assert!(
            s >= 0.8,
            "separator-normalized match should score high, got {s}"
        );
    }

    #[test]
    fn score_match_exact_bare() {
        let s = score_match("ripgrep", "ripgrep", "ripgrep");
        assert!(
            (s - 1.0).abs() < f64::EPSILON,
            "bare exact should be 1.0, got {s}"
        );
    }

    #[test]
    fn score_match_substring() {
        let s = score_match("grep", "legacyPackages.aarch64-darwin.ripgrep", "ripgrep");
        assert!(s >= 0.3, "substring match should pass threshold, got {s}");
        assert!(s < 0.7, "substring match should not be high, got {s}");
    }

    // --- clean_attr_path ---

    #[test]
    fn clean_attr_path_with_prefix() {
        assert_eq!(
            clean_attr_path("legacyPackages.aarch64-darwin.ripgrep"),
            "ripgrep"
        );
    }

    #[test]
    fn clean_attr_path_nested() {
        assert_eq!(
            clean_attr_path("legacyPackages.x86_64-linux.python3Packages.requests"),
            "python3Packages.requests"
        );
    }

    #[test]
    fn clean_attr_path_without_prefix() {
        assert_eq!(clean_attr_path("ripgrep"), "ripgrep");
    }

    // --- parse_nix_search_results ---

    #[test]
    fn parse_dict_format() {
        let data = json!({
            "legacyPackages.aarch64-darwin.ripgrep": {
                "pname": "ripgrep",
                "version": "14.1.0",
                "description": "fast grep"
            }
        });
        let entries = parse_nix_search_results(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].pname, "ripgrep");
        assert_eq!(entries[0].version, "14.1.0");
    }

    #[test]
    fn parse_empty_input() {
        let entries = parse_nix_search_results(&json!({}));
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_dict_uses_key_as_attr_path() {
        let data = json!({
            "legacyPackages.x86_64-linux.fd": {
                "pname": "fd",
                "version": "9.0.0",
                "description": "find alternative"
            }
        });
        let entries = parse_nix_search_results(&data);
        assert_eq!(entries[0].attr_path, "legacyPackages.x86_64-linux.fd");
    }

    // --- detect_language_package ---

    #[test]
    fn detect_python_package() {
        let result = detect_language_package("python3Packages.rich");
        assert_eq!(result, Some(("rich", "python3", "withPackages")));
    }

    #[test]
    fn detect_lua_package() {
        let result = detect_language_package("luaPackages.lpeg");
        assert_eq!(result, Some(("lpeg", "lua5_4", "withPackages")));
    }

    #[test]
    fn detect_not_language_package() {
        assert!(detect_language_package("ripgrep").is_none());
    }

    #[test]
    fn detect_versioned_python() {
        let result = detect_language_package("python312Packages.requests");
        assert_eq!(result, Some(("requests", "python3", "withPackages")));
    }

    // --- search_name_variants ---

    #[test]
    fn variants_dedup() {
        let v = search_name_variants("ripgrep");
        assert!(v.len() <= 3);
        let unique: std::collections::HashSet<_> = v.iter().collect();
        assert_eq!(unique.len(), v.len(), "variants should be unique");
    }

    #[test]
    fn variants_max_three() {
        let v = search_name_variants("py-yaml");
        assert!(
            v.len() <= 3,
            "should produce at most 3 variants, got {}",
            v.len()
        );
    }

    #[test]
    fn variants_includes_mapped() {
        let v = search_name_variants("rg");
        assert!(
            v.contains(&"ripgrep".to_string()),
            "should include mapped name"
        );
    }

    // --- sort_results ---

    #[test]
    fn sort_normal_priority() {
        let prefs = SourcePreferences::default();
        let mut results = vec![
            SourceResult {
                confidence: 0.9,
                ..SourceResult::new("x", PackageSource::Homebrew)
            },
            SourceResult {
                confidence: 0.8,
                ..SourceResult::new("x", PackageSource::Nxs)
            },
            SourceResult {
                confidence: 0.7,
                ..SourceResult::new("x", PackageSource::FlakeInput)
            },
        ];
        sort_results(&mut results, &prefs);
        assert_eq!(results[0].source, PackageSource::FlakeInput);
        assert_eq!(results[1].source, PackageSource::Nxs);
        assert_eq!(results[2].source, PackageSource::Homebrew);
    }

    #[test]
    fn sort_bleeding_edge_swaps_nur_nxs() {
        let prefs = SourcePreferences {
            bleeding_edge: true,
            ..Default::default()
        };
        let mut results = vec![
            SourceResult {
                confidence: 0.9,
                ..SourceResult::new("x", PackageSource::Nxs)
            },
            SourceResult {
                confidence: 0.8,
                ..SourceResult::new("x", PackageSource::Nur)
            },
        ];
        sort_results(&mut results, &prefs);
        assert_eq!(results[0].source, PackageSource::Nur);
        assert_eq!(results[1].source, PackageSource::Nxs);
    }

    // --- deduplicate_results ---

    #[test]
    fn dedup_preserves_first() {
        let results = vec![
            SourceResult {
                attr: Some("ripgrep".into()),
                confidence: 0.9,
                ..SourceResult::new("rg", PackageSource::Nxs)
            },
            SourceResult {
                attr: Some("ripgrep".into()),
                confidence: 0.7,
                ..SourceResult::new("rg", PackageSource::Nxs)
            },
        ];
        let deduped = deduplicate_results(results);
        assert_eq!(deduped.len(), 1);
        assert!((deduped[0].confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn dedup_keeps_different_sources() {
        let results = vec![
            SourceResult {
                attr: Some("ripgrep".into()),
                ..SourceResult::new("rg", PackageSource::Nxs)
            },
            SourceResult {
                attr: Some("ripgrep".into()),
                ..SourceResult::new("rg", PackageSource::Homebrew)
            },
        ];
        let deduped = deduplicate_results(results);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn dedup_handles_none_attr() {
        let results = vec![
            SourceResult::new("x", PackageSource::Nxs),
            SourceResult::new("x", PackageSource::Nxs),
        ];
        let deduped = deduplicate_results(results);
        assert_eq!(deduped.len(), 1);
    }

    // --- check_platforms ---

    #[test]
    fn platforms_includes_current() {
        let platforms = json!(["aarch64-darwin", "x86_64-linux"]);
        let (avail, reason) = check_platforms(&platforms, "aarch64-darwin");
        assert!(avail);
        assert!(reason.is_none());
    }

    #[test]
    fn platforms_excludes_current() {
        let platforms = json!(["x86_64-linux", "aarch64-linux"]);
        let (avail, reason) = check_platforms(&platforms, "aarch64-darwin");
        assert!(!avail);
        assert!(reason.unwrap().contains("not available on aarch64-darwin"));
    }

    #[test]
    fn platforms_non_list_permissive() {
        let (avail, _) = check_platforms(&json!("all"), "aarch64-darwin");
        assert!(avail);
    }

    #[test]
    fn platforms_empty_permissive() {
        let (avail, _) = check_platforms(&json!([]), "aarch64-darwin");
        assert!(avail);
    }
}
