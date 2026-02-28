use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::domain::source::{
    ExplicitSourceTarget, NixSearchEntry, OVERLAY_PACKAGES, PackageSource, SourcePreferences,
    SourceResult, check_platforms, clean_attr_path, deduplicate_results, detect_language_package,
    get_current_system, mapped_name, parse_nix_search_results, score_match, search_name_variants,
    sort_results,
};
use crate::infra::shell::run_json_command_quiet;

// --- Shell Helpers

/// Check if a program is available on PATH.
fn command_available(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Evaluate a nix attribute, trying each target in order.
fn eval_nix_attr(targets: &[&str], attr_path: &str) -> Option<Value> {
    for target in targets {
        let full_attr = format!("{target}#{attr_path}");
        if let Some(val) = run_json_command_quiet("nix", &["eval", "--json", &full_attr]) {
            return Some(val);
        }
    }
    None
}

/// Get a single entry from `brew info --json=v2`.
fn get_homebrew_info_entry(name: &str, is_cask: bool) -> Option<Value> {
    if !command_available("brew") {
        return None;
    }

    let mut args = vec!["info", "--json=v2"];
    if is_cask {
        args.push("--cask");
    }
    args.push(name);

    let data = run_json_command_quiet("brew", &args)?;
    let key = if is_cask { "casks" } else { "formulae" };
    let entries = data.get(key)?.as_array()?;
    let entry = entries.first()?;

    if entry.is_object() {
        Some(entry.clone())
    } else {
        None
    }
}

// --- Individual Source Searches

/// Shared nix search helper used by both nxs and NUR.
fn search_nix_source(
    name: &str,
    targets: &[&str],
    source: PackageSource,
    requires_flake_mod: bool,
    flake_url: Option<&str>,
) -> Vec<SourceResult> {
    if !command_available("nix") {
        return Vec::new();
    }

    let mut all_entries: Vec<NixSearchEntry> = Vec::new();
    let mut seen_attrs: HashSet<String> = HashSet::new();
    let resolved = mapped_name(name);

    for search_name in search_name_variants(name) {
        for target in targets {
            if let Some(data) =
                run_json_command_quiet("nix", &["search", "--json", target, &search_name])
            {
                for entry in parse_nix_search_results(&data) {
                    if !entry.attr_path.is_empty() && seen_attrs.insert(entry.attr_path.clone()) {
                        all_entries.push(entry);
                    }
                }
                break; // found results for this variant, try next
            }
        }
    }

    if all_entries.is_empty() {
        return Vec::new();
    }

    let mut results: Vec<SourceResult> = all_entries
        .iter()
        .filter_map(|entry| {
            let score = score_match(&resolved, &entry.attr_path, &entry.pname);
            if score < 0.3 {
                return None;
            }

            let attr_clean = clean_attr_path(&entry.attr_path).to_string();
            let description = if entry.description.len() > 100 {
                format!("{}...", &entry.description[..97])
            } else {
                entry.description.clone()
            };

            Some(SourceResult {
                name: name.to_string(),
                source,
                attr: Some(attr_clean),
                version: if entry.version.is_empty() {
                    None
                } else {
                    Some(entry.version.clone())
                },
                confidence: score,
                description,
                requires_flake_mod,
                flake_url: flake_url.map(String::from),
            })
        })
        .collect();

    results.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(5);
    results
}

/// Search nixpkgs for a package.
pub fn search_nxs(name: &str, prefer_unstable: bool) -> Vec<SourceResult> {
    let targets: Vec<&str> = if prefer_unstable {
        vec!["github:nixos/nixpkgs/nixos-unstable", "nixpkgs"]
    } else {
        vec!["nixpkgs", "github:nixos/nixpkgs/nixos-unstable"]
    };
    search_nix_source(name, &targets, PackageSource::Nxs, false, None)
}

/// Search NUR (Nix User Repository) for a package.
pub fn search_nur(name: &str) -> Vec<SourceResult> {
    search_nix_source(
        name,
        &["github:nix-community/NUR"],
        PackageSource::Nur,
        true,
        Some("github:nix-community/NUR"),
    )
}

/// Check existing flake inputs for package overlays.
pub fn search_flake_inputs(name: &str, flake_lock_path: &Path) -> Vec<SourceResult> {
    let Ok(content) = fs::read_to_string(flake_lock_path) else {
        return Vec::new();
    };

    let Ok(lock) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };

    let Some(nodes) = lock.get("nodes").and_then(Value::as_object) else {
        return Vec::new();
    };

    // Build overlay->packages index from domain OVERLAY_PACKAGES (package->overlay).
    let mut overlay_to_pkgs: HashMap<&str, Vec<&str>> = HashMap::new();
    for (&pkg, &(overlay, _, _)) in OVERLAY_PACKAGES.iter() {
        overlay_to_pkgs.entry(overlay).or_default().push(pkg);
    }

    let search_name = mapped_name(name).to_lowercase();
    let mut results = Vec::new();

    for input_name in nodes.keys() {
        if input_name == "root" {
            continue;
        }

        let Some(provided) = overlay_to_pkgs.get(input_name.as_str()) else {
            continue;
        };

        for &pkg in provided {
            let pkg_lower = pkg.to_lowercase();
            if search_name.contains(&pkg_lower) || pkg_lower.contains(&search_name) {
                let confidence = if pkg_lower == search_name { 0.9 } else { 0.7 };
                results.push(SourceResult {
                    name: name.to_string(),
                    source: PackageSource::FlakeInput,
                    attr: Some(pkg.to_string()),
                    version: None,
                    confidence,
                    description: format!("From {input_name} overlay"),
                    requires_flake_mod: false,
                    flake_url: None,
                });
            }
        }
    }

    results
}

/// Search Homebrew for a package (formula or cask).
pub fn search_homebrew(name: &str, is_cask: bool, allow_fallback: bool) -> Vec<SourceResult> {
    let entry = get_homebrew_info_entry(name, is_cask);

    entry.map_or_else(
        || {
            // Try the opposite (cask vs formula) as fallback
            if allow_fallback && !is_cask {
                search_homebrew(name, true, false)
            } else {
                Vec::new()
            }
        },
        |entry| {
            if is_cask {
                vec![SourceResult {
                    name: name.to_string(),
                    source: PackageSource::Cask,
                    attr: Some(
                        entry
                            .get("token")
                            .and_then(Value::as_str)
                            .unwrap_or(name)
                            .to_string(),
                    ),
                    version: entry
                        .get("version")
                        .and_then(Value::as_str)
                        .map(String::from),
                    confidence: 1.0,
                    description: entry
                        .get("desc")
                        .and_then(Value::as_str)
                        .unwrap_or("GUI application")
                        .to_string(),
                    requires_flake_mod: false,
                    flake_url: None,
                }]
            } else {
                vec![SourceResult {
                    name: name.to_string(),
                    source: PackageSource::Homebrew,
                    attr: Some(
                        entry
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(name)
                            .to_string(),
                    ),
                    version: entry
                        .get("versions")
                        .and_then(|v| v.get("stable"))
                        .and_then(Value::as_str)
                        .map(String::from),
                    confidence: 0.8,
                    description: entry
                        .get("desc")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    requires_flake_mod: false,
                    flake_url: None,
                }]
            }
        },
    )
}

// --- Platform / Language Validation

/// Check if a nix package is available on the current platform.
///
/// Shells out to `nix eval` then delegates to pure `check_platforms`.
/// Permissive when `nix` is missing or evaluation fails.
pub fn check_nix_available(attr: &str) -> (bool, Option<String>) {
    if !command_available("nix") {
        return (true, None);
    }

    let targets = &["nixpkgs"][..];
    let meta_attr = format!("{attr}.meta.platforms");

    eval_nix_attr(targets, &meta_attr).map_or((true, None), |platforms| {
        check_platforms(&platforms, get_current_system())
    })
}

/// Validate that a language package attr exists and is available on this platform.
fn validate_language_override(name: &str) -> (bool, Option<String>) {
    if !command_available("nix") {
        return (false, Some("nix command unavailable".to_string()));
    }

    let targets = &["nixpkgs", "github:nixos/nixpkgs/nixos-unstable"];
    let name_attr = format!("{name}.name");

    if eval_nix_attr(targets, &name_attr).is_none() {
        return (false, Some("attribute not found in nixpkgs".to_string()));
    }

    let (available, reason) = check_nix_available(name);
    if !available {
        return (false, reason);
    }

    (true, None)
}

// --- Search Shortcuts (forced / explicit / language override)

fn search_forced_source(name: &str, prefs: &SourcePreferences) -> Option<Vec<SourceResult>> {
    let source = prefs.force_source.as_deref()?;
    if source.eq_ignore_ascii_case("unstable") {
        return Some(search_nxs(name, true));
    }
    match PackageSource::parse(source) {
        Some(PackageSource::Nxs) => Some(search_nxs(name, false)),
        Some(PackageSource::Nur) => Some(search_nur(name)),
        Some(PackageSource::Homebrew) => Some(search_homebrew(
            name,
            matches!(prefs.explicit_target, ExplicitSourceTarget::Cask),
            true,
        )),
        _ => None,
    }
}

fn search_explicit_source(name: &str, prefs: &SourcePreferences) -> Option<Vec<SourceResult>> {
    match prefs.explicit_target {
        ExplicitSourceTarget::Any => None,
        ExplicitSourceTarget::Cask => Some(vec![SourceResult {
            name: name.to_string(),
            source: PackageSource::Cask,
            attr: Some(name.to_string()),
            version: None,
            confidence: 1.0,
            description: "GUI application (cask)".to_string(),
            requires_flake_mod: false,
            flake_url: None,
        }]),
        ExplicitSourceTarget::Mas => Some(vec![SourceResult {
            name: name.to_string(),
            source: PackageSource::Mas,
            attr: Some(name.to_string()),
            version: None,
            confidence: 1.0,
            description: "Mac App Store app".to_string(),
            requires_flake_mod: false,
            flake_url: None,
        }]),
    }
}

fn search_language_override(name: &str, warn: bool) -> Option<Vec<SourceResult>> {
    let (_bare, runtime, _method) = detect_language_package(name)?;

    let (valid, reason) = validate_language_override(name);
    if !valid {
        if warn
            && let Some(r) = &reason
            && r != "nix command unavailable"
        {
            eprintln!("warning: skipping language override '{name}': {r}");
        }
        return None;
    }

    Some(vec![SourceResult {
        name: name.to_string(),
        source: PackageSource::Nxs,
        attr: Some(name.to_string()),
        version: None,
        confidence: 1.0,
        description: format!("{runtime} package"),
        requires_flake_mod: false,
        flake_url: None,
    }])
}

// --- Parallel Search + Orchestration

#[derive(Debug)]
struct SearchBatch {
    source: &'static str,
    results: Vec<SourceResult>,
    failed: bool,
}

type SearchCallResult = Vec<SourceResult>;

type SearchByNameFn = fn(&str) -> SearchCallResult;
type SearchByNameAndPathFn = fn(&str, &Path) -> SearchCallResult;

#[derive(Clone, Copy)]
struct SearchFns {
    nxs: SearchByNameFn,
    flake_inputs: SearchByNameAndPathFn,
    nur: SearchByNameFn,
}

#[derive(Clone, Copy)]
struct ParallelSearchOptions {
    warn_on_timeout: bool,
    timeout: Duration,
}

fn search_nxs_primary(name: &str) -> SearchCallResult {
    search_nxs(name, false)
}

fn search_flake_inputs_primary(name: &str, lock_path: &Path) -> SearchCallResult {
    search_flake_inputs(name, lock_path)
}

fn search_nur_primary(name: &str) -> SearchCallResult {
    search_nur(name)
}

fn spawn_search_worker(
    tx: mpsc::Sender<SearchBatch>,
    source: &'static str,
    search: impl FnOnce() -> SearchCallResult + Send + 'static,
) {
    let _join_handle = thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(search));
        let batch = match result {
            Ok(results) => SearchBatch {
                source,
                results,
                failed: false,
            },
            Err(_) => SearchBatch {
                source,
                results: Vec::new(),
                failed: true,
            },
        };
        let _ = tx.send(batch);
    });
}

/// Execute parallel searches across enabled sources.
///
/// Uses detached workers + `mpsc::channel` + `recv_timeout`.
/// Individual source failures are logged but don't fail the whole search.
fn parallel_search(
    name: &str,
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
    warn_on_timeout: bool,
) -> Vec<SourceResult> {
    let options = ParallelSearchOptions {
        warn_on_timeout,
        timeout: Duration::from_secs(45),
    };
    let search_fns = SearchFns {
        nxs: search_nxs_primary,
        flake_inputs: search_flake_inputs_primary,
        nur: search_nur_primary,
    };

    parallel_search_with(
        name,
        prefs,
        flake_lock_path,
        options,
        |message| eprintln!("{message}"),
        search_fns,
    )
}

fn parallel_search_with(
    name: &str,
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
    options: ParallelSearchOptions,
    mut warn: impl FnMut(&str),
    search_fns: SearchFns,
) -> Vec<SourceResult> {
    let (tx, rx) = mpsc::channel::<SearchBatch>();
    let mut expected = 0_usize;
    let source_name = name.to_string();

    // Always search nxs
    {
        let tx_nxs = tx.clone();
        let name = source_name.clone();
        spawn_search_worker(tx_nxs, "nxs", move || (search_fns.nxs)(&name));
        expected += 1;
    }

    // Optional flake-input search
    if let Some(lock_path) = flake_lock_path {
        let tx_flake = tx.clone();
        let name = source_name.clone();
        let lock_path = lock_path.to_path_buf();
        spawn_search_worker(tx_flake, "flake-input", move || {
            (search_fns.flake_inputs)(&name, &lock_path)
        });
        expected += 1;
    }

    // Optional NUR search
    if prefs.nur || prefs.bleeding_edge {
        let tx_nur = tx.clone();
        let name = source_name;
        spawn_search_worker(tx_nur, "nur", move || (search_fns.nur)(&name));
        expected += 1;
    }

    drop(tx);

    let mut all_results = Vec::new();
    for _ in 0..expected {
        match rx.recv_timeout(options.timeout) {
            Ok(batch) => {
                if batch.failed {
                    if options.warn_on_timeout {
                        warn(&format!(
                            "warning: {src} search failed for '{name}'; using partial results",
                            src = batch.source
                        ));
                    }
                    continue;
                }
                all_results.extend(batch.results);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if options.warn_on_timeout {
                    warn(&format!(
                        "warning: timed out waiting for one or more search sources for '{name}'; using partial results"
                    ));
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    all_results
}

/// Search all enabled sources for a package.
///
/// Returns results sorted by preference and confidence.
pub fn search_all_sources(
    name: &str,
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
) -> Vec<SourceResult> {
    search_all_sources_with_timeout_reporting(name, prefs, flake_lock_path, true)
}

/// Search all enabled sources for a package without timeout warnings.
///
/// Used by `info --json` to avoid stderr drift in parity-sensitive read paths.
pub fn search_all_sources_quiet(
    name: &str,
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
) -> Vec<SourceResult> {
    search_all_sources_with_timeout_reporting(name, prefs, flake_lock_path, false)
}

fn search_all_sources_with_timeout_reporting(
    name: &str,
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
    warn_on_timeout: bool,
) -> Vec<SourceResult> {
    // 1. Forced source shortcut
    if let Some(results) = search_forced_source(name, prefs) {
        return results;
    }

    // 2. Explicit --cask / --mas
    if let Some(results) = search_explicit_source(name, prefs) {
        return results;
    }

    // 3. Language override
    if let Some(results) = search_language_override(name, warn_on_timeout) {
        return results;
    }

    // 4. Parallel primary search
    let mut results = parallel_search(name, prefs, flake_lock_path, warn_on_timeout);

    // 5. Always append homebrew formula + cask alternatives
    results.extend(search_homebrew(name, false, false));
    results.extend(search_homebrew(name, true, false));

    // 6. Sort by source priority + confidence
    sort_results(&mut results, prefs);

    // 7. Deduplicate by (source, attr)
    deduplicate_results(results)
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as FmtWrite;
    use std::io::Write;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    // --- search_flake_inputs ---

    fn make_flake_lock(dir: &tempfile::TempDir, nodes: &[&str]) -> std::path::PathBuf {
        let lock_path = dir.path().join("flake.lock");
        let mut node_entries = String::new();
        for (i, name) in nodes.iter().enumerate() {
            if i > 0 {
                node_entries.push_str(", ");
            }
            write!(
                node_entries,
                r#""{name}": {{"locked": {{"type": "github"}}}}"#
            )
            .unwrap();
        }
        let content = format!(r#"{{"version": 7, "nodes": {{"root": {{}}, {node_entries}}}}}"#);
        let mut f = fs::File::create(&lock_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        lock_path
    }

    #[test]
    fn flake_inputs_finds_overlay_package() {
        let dir = tempfile::tempdir().unwrap();
        let lock = make_flake_lock(&dir, &["fenix"]);
        let results = search_flake_inputs("rust", &lock);
        assert!(!results.is_empty(), "should find rust in fenix overlay");
        assert_eq!(results[0].source, PackageSource::FlakeInput);
    }

    #[test]
    fn flake_inputs_empty_for_unknown_package() {
        let dir = tempfile::tempdir().unwrap();
        let lock = make_flake_lock(&dir, &["fenix"]);
        let results = search_flake_inputs("obscure-pkg-xyz", &lock);
        assert!(results.is_empty());
    }

    #[test]
    fn flake_inputs_missing_lock_returns_empty() {
        let results = search_flake_inputs("rust", Path::new("/nonexistent/flake.lock"));
        assert!(results.is_empty());
    }

    #[test]
    fn flake_inputs_neovim_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let lock = make_flake_lock(&dir, &["neovim-nightly-overlay"]);
        let results = search_flake_inputs("neovim", &lock);
        assert!(!results.is_empty());
        assert!(results[0].confidence >= 0.7);
    }

    // --- search_forced_source ---

    #[test]
    fn forced_source_none_when_not_set() {
        let prefs = SourcePreferences::default();
        assert!(search_forced_source("ripgrep", &prefs).is_none());
    }

    #[test]
    fn forced_source_unknown_returns_none() {
        let prefs = SourcePreferences {
            force_source: Some("flakehub".to_string()),
            ..Default::default()
        };
        assert!(search_forced_source("ripgrep", &prefs).is_none());
    }

    #[test]
    fn forced_source_brew_alias_is_parsed() {
        let prefs = SourcePreferences {
            force_source: Some("BrEw".to_string()),
            ..Default::default()
        };
        assert!(search_forced_source("ripgrep", &prefs).is_some());
    }

    #[test]
    fn forced_source_unstable_is_case_insensitive() {
        let prefs = SourcePreferences {
            force_source: Some("UnStable".to_string()),
            ..Default::default()
        };
        assert!(search_forced_source("ripgrep", &prefs).is_some());
    }

    // --- search_explicit_source ---

    #[test]
    fn explicit_cask_shortcut() {
        let prefs = SourcePreferences {
            explicit_target: ExplicitSourceTarget::Cask,
            ..Default::default()
        };
        let results = search_explicit_source("firefox", &prefs).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, PackageSource::Cask);
        assert!((results[0].confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn explicit_mas_shortcut() {
        let prefs = SourcePreferences {
            explicit_target: ExplicitSourceTarget::Mas,
            ..Default::default()
        };
        let results = search_explicit_source("Xcode", &prefs).unwrap();
        assert_eq!(results[0].source, PackageSource::Mas);
    }

    #[test]
    fn explicit_source_none_for_default_prefs() {
        let prefs = SourcePreferences::default();
        assert!(search_explicit_source("ripgrep", &prefs).is_none());
    }

    // --- command_available ---

    #[test]
    fn command_available_finds_cat() {
        // `cat` (coreutils) is available in all environments including nix sandbox
        assert!(command_available("cat"));
    }

    #[test]
    fn command_available_missing_program() {
        assert!(!command_available("__nx_definitely_not_a_command__"));
    }

    // --- parallel_search_with ---

    fn stub_result(source: PackageSource, attr: &str) -> SourceResult {
        SourceResult {
            name: "ripgrep".to_string(),
            source,
            attr: Some(attr.to_string()),
            version: None,
            confidence: 1.0,
            description: "stub".to_string(),
            requires_flake_mod: false,
            flake_url: None,
        }
    }

    fn stub_nxs_slow(_name: &str) -> SearchCallResult {
        sleep(Duration::from_millis(250));
        vec![stub_result(PackageSource::Nxs, "slow-nxs")]
    }

    fn stub_nur_fast(_name: &str) -> SearchCallResult {
        vec![stub_result(PackageSource::Nur, "fast-nur")]
    }

    fn stub_nxs_failed(_name: &str) -> SearchCallResult {
        panic!("stub nxs failure");
    }

    fn stub_flake_empty(_name: &str, _path: &Path) -> SearchCallResult {
        Vec::new()
    }

    #[test]
    fn parallel_search_timeout_returns_partial_results_and_warns() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();
        let started = Instant::now();

        let results = parallel_search_with(
            "ripgrep",
            &prefs,
            None,
            ParallelSearchOptions {
                warn_on_timeout: true,
                timeout: Duration::from_millis(40),
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_slow,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
            },
        );

        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, PackageSource::Nur);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("timed out waiting")),
            "expected timeout warning, got: {warnings:?}"
        );
    }

    #[test]
    fn parallel_search_timeout_quiet_suppresses_warning() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let results = parallel_search_with(
            "ripgrep",
            &prefs,
            None,
            ParallelSearchOptions {
                warn_on_timeout: false,
                timeout: Duration::from_millis(40),
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_slow,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
            },
        );

        assert_eq!(results.len(), 1);
        assert!(warnings.is_empty(), "warnings should be suppressed");
    }

    #[test]
    fn parallel_search_source_failure_keeps_other_results_and_warns() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let results = parallel_search_with(
            "ripgrep",
            &prefs,
            None,
            ParallelSearchOptions {
                warn_on_timeout: true,
                timeout: Duration::from_millis(200),
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_failed,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
            },
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, PackageSource::Nur);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("nxs search failed")),
            "expected source-failure warning, got: {warnings:?}"
        );
    }

    #[test]
    fn parallel_search_source_failure_quiet_suppresses_warning() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let results = parallel_search_with(
            "ripgrep",
            &prefs,
            None,
            ParallelSearchOptions {
                warn_on_timeout: false,
                timeout: Duration::from_millis(200),
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_failed,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
            },
        );

        assert_eq!(results.len(), 1);
        assert!(warnings.is_empty(), "warnings should be suppressed");
    }
}
