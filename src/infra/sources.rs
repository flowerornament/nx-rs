use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::domain::source::{
    ExplicitSourceTarget, NixSearchEntry, OVERLAY_PACKAGES, PackageSource, SourcePreferences,
    SourceResult, check_platforms, clean_attr_path, deduplicate_results, detect_language_package,
    get_current_system, mapped_name, parse_nix_search_results, score_match, search_name_variants,
    sort_results,
};
use crate::infra::cache::MultiSourceCache;
use crate::infra::shell::run_captured_command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnavailableSource {
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct SourceSearchOutcome {
    pub results: Vec<SourceResult>,
    pub unavailable_sources: Vec<UnavailableSource>,
}

#[derive(Debug, Clone, Default)]
struct SourceBackendOutcome {
    results: Vec<SourceResult>,
    unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum JsonCommandOutcome {
    Parsed(Value),
    Failed,
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NixAvailability {
    available: bool,
    reason: Option<String>,
    backend_unavailable: Option<String>,
}

impl SourceSearchOutcome {
    fn push_unavailable(&mut self, source: impl Into<String>, reason: impl Into<String>) {
        let candidate = UnavailableSource {
            source: source.into(),
            reason: reason.into(),
        };
        if !self.unavailable_sources.contains(&candidate) {
            self.unavailable_sources.push(candidate);
        }
    }

    fn extend_results(&mut self, results: Vec<SourceResult>) {
        self.results.extend(results);
    }
}

impl SourceBackendOutcome {
    fn from_results(results: Vec<SourceResult>) -> Self {
        Self {
            results,
            unavailable_reason: None,
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            results: Vec::new(),
            unavailable_reason: Some(reason.into()),
        }
    }
}

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

fn run_json_command(program: &str, args: &[&str]) -> JsonCommandOutcome {
    let output = match run_captured_command(program, args, None) {
        Ok(output) => output,
        Err(err) => {
            return JsonCommandOutcome::Unavailable(format!(
                "{program} command execution failed: {err:#}"
            ));
        }
    };

    if output.code != 0 {
        return JsonCommandOutcome::Failed;
    }

    match serde_json::from_str::<Value>(&output.stdout) {
        Ok(value) => JsonCommandOutcome::Parsed(value),
        Err(err) => {
            JsonCommandOutcome::Unavailable(format!("{program} returned invalid JSON: {err}"))
        }
    }
}

/// Evaluate a nix attribute, trying each target in order.
fn eval_nix_attr(targets: &[&str], attr_path: &str) -> JsonCommandOutcome {
    let mut unavailable_reason = None;
    for target in targets {
        let full_attr = format!("{target}#{attr_path}");
        match run_json_command("nix", &["eval", "--json", &full_attr]) {
            JsonCommandOutcome::Parsed(value) => return JsonCommandOutcome::Parsed(value),
            JsonCommandOutcome::Failed => {}
            JsonCommandOutcome::Unavailable(reason) => {
                unavailable_reason.get_or_insert(reason);
            }
        }
    }

    unavailable_reason.map_or(JsonCommandOutcome::Failed, JsonCommandOutcome::Unavailable)
}

/// Get a single entry from `brew info --json=v2`.
fn get_homebrew_info_entry(name: &str, is_cask: bool) -> JsonCommandOutcome {
    if !command_available("brew") {
        return JsonCommandOutcome::Unavailable("brew command unavailable".to_string());
    }

    let mut args = vec!["info", "--json=v2"];
    if is_cask {
        args.push("--cask");
    }
    args.push(name);

    let data = match run_json_command("brew", &args) {
        JsonCommandOutcome::Parsed(data) => data,
        JsonCommandOutcome::Failed => return JsonCommandOutcome::Failed,
        JsonCommandOutcome::Unavailable(reason) => return JsonCommandOutcome::Unavailable(reason),
    };
    let key = if is_cask { "casks" } else { "formulae" };
    let Some(entries) = data.get(key).and_then(Value::as_array) else {
        return JsonCommandOutcome::Unavailable(format!(
            "brew returned unexpected JSON structure for '{key}'"
        ));
    };
    let Some(entry) = entries.first() else {
        return JsonCommandOutcome::Failed;
    };

    if entry.is_object() {
        JsonCommandOutcome::Parsed(entry.clone())
    } else {
        JsonCommandOutcome::Unavailable("brew returned a non-object package entry".to_string())
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
) -> SourceBackendOutcome {
    if !command_available("nix") {
        return SourceBackendOutcome::unavailable("nix command unavailable");
    }

    let mut all_entries: Vec<NixSearchEntry> = Vec::new();
    let mut seen_attrs: HashSet<String> = HashSet::new();
    let resolved = mapped_name(name);
    let mut unavailable_reason = None;
    let mut saw_parsed_response = false;

    for search_name in search_name_variants(name) {
        for target in targets {
            match run_json_command("nix", &["search", "--json", target, &search_name]) {
                JsonCommandOutcome::Parsed(data) => {
                    saw_parsed_response = true;
                    for entry in parse_nix_search_results(&data) {
                        if !entry.attr_path.is_empty() && seen_attrs.insert(entry.attr_path.clone())
                        {
                            all_entries.push(entry);
                        }
                    }
                    break; // found results for this variant, try next
                }
                JsonCommandOutcome::Failed => {}
                JsonCommandOutcome::Unavailable(reason) => {
                    unavailable_reason.get_or_insert(reason);
                }
            }
        }
    }

    if all_entries.is_empty() {
        return if saw_parsed_response {
            SourceBackendOutcome::default()
        } else {
            unavailable_reason.map_or_else(
                SourceBackendOutcome::default,
                SourceBackendOutcome::unavailable,
            )
        };
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
    SourceBackendOutcome::from_results(results)
}

/// Search nixpkgs for a package.
fn search_nxs(name: &str, prefer_unstable: bool) -> SourceBackendOutcome {
    let targets: Vec<&str> = if prefer_unstable {
        vec!["github:nixos/nixpkgs/nixos-unstable", "nixpkgs"]
    } else {
        vec!["nixpkgs", "github:nixos/nixpkgs/nixos-unstable"]
    };
    search_nix_source(name, &targets, PackageSource::Nxs, false, None)
}

/// Search NUR (Nix User Repository) for a package.
fn search_nur(name: &str) -> SourceBackendOutcome {
    search_nix_source(
        name,
        &["github:nix-community/NUR"],
        PackageSource::Nur,
        true,
        Some("github:nix-community/NUR"),
    )
}

/// Check existing flake inputs for package overlays.
fn search_flake_inputs(name: &str, flake_lock_path: &Path) -> SourceBackendOutcome {
    let Ok(content) = fs::read_to_string(flake_lock_path) else {
        return SourceBackendOutcome::unavailable(format!(
            "failed to read flake.lock: {}",
            flake_lock_path.display()
        ));
    };

    let Ok(lock) = serde_json::from_str::<Value>(&content) else {
        return SourceBackendOutcome::unavailable("flake.lock is not valid JSON");
    };

    let Some(nodes) = lock.get("nodes").and_then(Value::as_object) else {
        return SourceBackendOutcome::unavailable("flake.lock missing 'nodes' object");
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

    SourceBackendOutcome::from_results(results)
}

/// Search Homebrew for a package (formula or cask).
fn search_homebrew(name: &str, is_cask: bool, allow_fallback: bool) -> SourceBackendOutcome {
    match get_homebrew_info_entry(name, is_cask) {
        JsonCommandOutcome::Failed => {
            if allow_fallback && !is_cask {
                search_homebrew(name, true, false)
            } else {
                SourceBackendOutcome::default()
            }
        }
        JsonCommandOutcome::Unavailable(reason) => SourceBackendOutcome::unavailable(reason),
        JsonCommandOutcome::Parsed(entry) => {
            let results = if is_cask {
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
            };
            SourceBackendOutcome::from_results(results)
        }
    }
}

// --- Platform / Language Validation

/// Check if a nix package is available on the current platform.
///
/// Shells out to `nix eval` then delegates to pure `check_platforms`.
/// Permissive when `nix` is missing or evaluation fails.
fn check_nix_available_status(attr: &str) -> NixAvailability {
    if !command_available("nix") {
        return NixAvailability {
            available: false,
            reason: None,
            backend_unavailable: Some("nix command unavailable".to_string()),
        };
    }

    let targets = &["nixpkgs"][..];
    let meta_attr = format!("{attr}.meta.platforms");

    match eval_nix_attr(targets, &meta_attr) {
        JsonCommandOutcome::Parsed(platforms) => {
            let (available, reason) = check_platforms(&platforms, get_current_system());
            NixAvailability {
                available,
                reason,
                backend_unavailable: None,
            }
        }
        JsonCommandOutcome::Failed => NixAvailability {
            available: true,
            reason: None,
            backend_unavailable: None,
        },
        JsonCommandOutcome::Unavailable(reason) => NixAvailability {
            available: false,
            reason: None,
            backend_unavailable: Some(reason),
        },
    }
}

/// Validate that a language package attr exists and is available on this platform.
fn validate_language_override(name: &str) -> (bool, Option<String>) {
    if !command_available("nix") {
        return (false, Some("nix command unavailable".to_string()));
    }

    let targets = &["nixpkgs", "github:nixos/nixpkgs/nixos-unstable"];
    let name_attr = format!("{name}.name");

    if !matches!(
        eval_nix_attr(targets, &name_attr),
        JsonCommandOutcome::Parsed(_)
    ) {
        return (false, Some("attribute not found in nixpkgs".to_string()));
    }

    let availability = check_nix_available_status(name);
    if let Some(reason) = availability.backend_unavailable {
        return (false, Some(reason));
    }
    if !availability.available {
        return (false, availability.reason);
    }

    (true, None)
}

pub fn check_nix_available(attr: &str) -> (bool, Option<String>) {
    let availability = check_nix_available_status(attr);
    if let Some(reason) = availability.backend_unavailable {
        return (false, Some(reason));
    }

    (availability.available, availability.reason)
}

// --- Search Shortcuts (forced / explicit / language override)

fn search_forced_source(name: &str, prefs: &SourcePreferences) -> Option<Vec<SourceResult>> {
    let source = prefs.force_source.as_deref()?;
    if source.eq_ignore_ascii_case("unstable") {
        return Some(search_nxs(name, true).results);
    }
    match PackageSource::parse(source) {
        Some(PackageSource::Nxs) => Some(search_nxs(name, false).results),
        Some(PackageSource::Nur) => Some(search_nur(name).results),
        Some(PackageSource::Homebrew) => Some(
            search_homebrew(
                name,
                matches!(prefs.explicit_target, ExplicitSourceTarget::Cask),
                true,
            )
            .results,
        ),
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
    let (_bare, runtime) = detect_language_package(name)?;

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
    id: usize,
    package: String,
    kind: SearchTaskKind,
    outcome: SourceBackendOutcome,
}

struct SearchTask {
    id: usize,
    package: String,
    kind: SearchTaskKind,
    search: Box<dyn FnOnce() -> SearchCallResult + Send>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchTaskKind {
    Source(PackageSource),
    LanguageOverride,
}

impl SearchTaskKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Source(source) => source.as_str(),
            Self::LanguageOverride => "language override",
        }
    }

    const fn source(self) -> PackageSource {
        match self {
            Self::Source(source) => source,
            Self::LanguageOverride => PackageSource::Nxs,
        }
    }
}

type SearchCallResult = SourceBackendOutcome;

type SearchByNameFn = fn(&str) -> SearchCallResult;
type SearchByNameAndPathFn = fn(&str, &Path) -> SearchCallResult;

#[derive(Clone, Copy)]
struct SearchFns {
    nxs: SearchByNameFn,
    flake_inputs: SearchByNameAndPathFn,
    nur: SearchByNameFn,
    homebrew_formula: SearchByNameFn,
    homebrew_cask: SearchByNameFn,
}

#[derive(Clone, Copy)]
struct SearchOptions {
    report_failures: bool,
    timeout: Duration,
    worker_limit: usize,
}

impl SearchOptions {
    fn standard(report_failures: bool) -> Self {
        Self {
            report_failures,
            timeout: Duration::from_secs(45),
            worker_limit: search_worker_limit(),
        }
    }
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

fn search_homebrew_formula(name: &str) -> SearchCallResult {
    search_homebrew(name, false, false)
}

fn search_homebrew_cask(name: &str) -> SearchCallResult {
    search_homebrew(name, true, false)
}

struct SearchExecution {
    completed: Vec<SearchBatch>,
    timed_out: Vec<(String, SearchTaskKind)>,
}

struct SearchExecutor {
    worker_limit: usize,
    timeout: Duration,
}

struct SearchQueue {
    tasks: VecDeque<SearchTask>,
    cancelled: bool,
}

impl SearchExecutor {
    fn execute(&self, tasks: Vec<SearchTask>) -> SearchExecution {
        let mut pending = tasks
            .iter()
            .map(|task| (task.id, (task.package.clone(), task.kind)))
            .collect::<HashMap<_, _>>();
        let expected = pending.len();
        let queue = Arc::new(Mutex::new(SearchQueue {
            tasks: VecDeque::from(tasks),
            cancelled: false,
        }));
        let (tx, rx) = mpsc::channel();

        for _ in 0..expected.min(self.worker_limit.max(1)) {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            thread::spawn(move || {
                loop {
                    let task = queue.lock().ok().and_then(|mut queue| {
                        (!queue.cancelled)
                            .then(|| queue.tasks.pop_front())
                            .flatten()
                    });
                    let Some(task) = task else {
                        break;
                    };
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(task.search));
                    let outcome = result.unwrap_or_else(|_| {
                        SourceBackendOutcome::unavailable("search worker panicked")
                    });
                    let _ = tx.send(SearchBatch {
                        id: task.id,
                        package: task.package,
                        kind: task.kind,
                        outcome,
                    });
                }
            });
        }
        drop(tx);

        let deadline = Instant::now() + self.timeout;
        let mut completed = Vec::with_capacity(expected);
        while !pending.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match rx.recv_timeout(remaining) {
                Ok(batch) => {
                    pending.remove(&batch.id);
                    completed.push(batch);
                }
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        if let Ok(mut queue) = queue.lock() {
            queue.cancelled = true;
            queue.tasks.clear();
        }

        completed.sort_unstable_by_key(|batch| batch.id);
        let mut timed_out = pending.into_values().collect::<Vec<_>>();
        timed_out.sort_unstable_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.label().cmp(right.1.label()))
        });
        SearchExecution {
            completed,
            timed_out,
        }
    }
}

fn default_search_fns() -> SearchFns {
    SearchFns {
        nxs: search_nxs_primary,
        flake_inputs: search_flake_inputs_primary,
        nur: search_nur_primary,
        homebrew_formula: search_homebrew_formula,
        homebrew_cask: search_homebrew_cask,
    }
}

fn search_many_with(
    names: &[String],
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
    options: SearchOptions,
    mut warn: impl FnMut(&str),
    search_fns: SearchFns,
) -> HashMap<String, SourceSearchOutcome> {
    let (mut outcomes, tasks) =
        build_search_tasks(names, prefs, flake_lock_path, options, search_fns);
    let execution = SearchExecutor {
        worker_limit: options.worker_limit,
        timeout: options.timeout,
    }
    .execute(tasks);

    collect_search_results(&mut outcomes, execution, prefs, options, &mut warn);
    outcomes
}

fn build_search_tasks(
    names: &[String],
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
    options: SearchOptions,
    search_fns: SearchFns,
) -> (HashMap<String, SourceSearchOutcome>, Vec<SearchTask>) {
    let mut outcomes = HashMap::with_capacity(names.len());
    let mut tasks = Vec::new();

    for name in names {
        if let Some(results) = search_explicit_source(name, prefs) {
            outcomes.insert(
                name.clone(),
                SourceSearchOutcome {
                    results,
                    unavailable_sources: Vec::new(),
                },
            );
            continue;
        }

        outcomes.entry(name.clone()).or_default();
        let mut push_task = |kind, search: Box<dyn FnOnce() -> SearchCallResult + Send>| {
            tasks.push(SearchTask {
                id: tasks.len(),
                package: name.clone(),
                kind,
                search,
            });
        };

        if let Some(source) = prefs.force_source.as_deref() {
            let source = if source.eq_ignore_ascii_case("unstable") {
                Some(PackageSource::Nxs)
            } else {
                PackageSource::parse(source)
            };
            if let Some(source) = source {
                let package = name.clone();
                let prefs = prefs.clone();
                push_task(
                    SearchTaskKind::Source(source),
                    Box::new(move || {
                        SourceBackendOutcome::from_results(
                            search_forced_source(&package, &prefs).unwrap_or_default(),
                        )
                    }),
                );
                continue;
            }
        }

        if detect_language_package(name).is_some() {
            let package = name.clone();
            let report_failures = options.report_failures;
            push_task(
                SearchTaskKind::LanguageOverride,
                Box::new(move || {
                    SourceBackendOutcome::from_results(
                        search_language_override(&package, report_failures).unwrap_or_default(),
                    )
                }),
            );
        }

        let package = name.clone();
        push_task(
            SearchTaskKind::Source(PackageSource::Nxs),
            Box::new(move || (search_fns.nxs)(&package)),
        );
        if let Some(lock_path) = flake_lock_path {
            let package = name.clone();
            let lock_path = lock_path.to_path_buf();
            push_task(
                SearchTaskKind::Source(PackageSource::FlakeInput),
                Box::new(move || (search_fns.flake_inputs)(&package, &lock_path)),
            );
        }
        if prefs.nur || prefs.bleeding_edge {
            let package = name.clone();
            push_task(
                SearchTaskKind::Source(PackageSource::Nur),
                Box::new(move || (search_fns.nur)(&package)),
            );
        }
        let package = name.clone();
        push_task(
            SearchTaskKind::Source(PackageSource::Homebrew),
            Box::new(move || (search_fns.homebrew_formula)(&package)),
        );
        let package = name.clone();
        push_task(
            SearchTaskKind::Source(PackageSource::Cask),
            Box::new(move || (search_fns.homebrew_cask)(&package)),
        );
    }

    (outcomes, tasks)
}

fn collect_search_results(
    outcomes: &mut HashMap<String, SourceSearchOutcome>,
    execution: SearchExecution,
    prefs: &SourcePreferences,
    options: SearchOptions,
    warn: &mut impl FnMut(&str),
) {
    let language_overrides = execution
        .completed
        .iter()
        .filter(|batch| {
            batch.kind == SearchTaskKind::LanguageOverride && !batch.outcome.results.is_empty()
        })
        .map(|batch| batch.package.clone())
        .collect::<HashSet<_>>();

    for batch in execution.completed {
        if language_overrides.contains(&batch.package)
            && batch.kind != SearchTaskKind::LanguageOverride
        {
            continue;
        }
        let outcome = outcomes.entry(batch.package.clone()).or_default();
        if let Some(reason) = batch.outcome.unavailable_reason {
            if options.report_failures {
                warn(&format!(
                    "warning: {source} search unavailable for '{package}': {reason}; using partial results",
                    source = batch.kind.label(),
                    package = batch.package,
                ));
            }
            outcome.push_unavailable(batch.kind.source().as_str(), reason);
        } else {
            outcome.extend_results(batch.outcome.results);
        }
    }

    let mut reported_timeouts = HashSet::new();
    for (package, kind) in execution.timed_out {
        if language_overrides.contains(&package)
            || !reported_timeouts.insert((package.clone(), kind.source()))
        {
            continue;
        }
        if options.report_failures {
            warn(&format!(
                "warning: timed out waiting for {source} search for '{package}'; using partial results",
                source = kind.label(),
            ));
        }
        outcomes.entry(package).or_default().push_unavailable(
            kind.source().as_str(),
            "timed out waiting for search response",
        );
    }

    for outcome in outcomes.values_mut() {
        sort_results(&mut outcome.results, prefs);
        outcome.results = deduplicate_results(std::mem::take(&mut outcome.results));
    }
}

#[cfg(test)]
fn search_one_with(
    name: &str,
    prefs: &SourcePreferences,
    flake_lock_path: Option<&Path>,
    options: SearchOptions,
    warn: impl FnMut(&str),
    search_fns: SearchFns,
) -> SourceSearchOutcome {
    search_many_with(
        &[name.to_string()],
        prefs,
        flake_lock_path,
        options,
        warn,
        search_fns,
    )
    .remove(name)
    .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct CachedSearchOutcome {
    pub outcome: SourceSearchOutcome,
    pub cache_hit: bool,
}

pub fn cached_search_many_with_status(
    names: &[String],
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
) -> HashMap<String, CachedSearchOutcome> {
    cached_search_many_with_status_reporting(names, prefs, repo_root, cache, true)
}

pub fn cached_search_many_with_status_quiet(
    names: &[String],
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
) -> HashMap<String, CachedSearchOutcome> {
    cached_search_many_with_status_reporting(names, prefs, repo_root, cache, false)
}

fn cached_search_many_with_status_reporting(
    names: &[String],
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    report_failures: bool,
) -> HashMap<String, CachedSearchOutcome> {
    cached_search_many_with_status_using(
        names,
        prefs,
        repo_root,
        cache,
        |names, prefs, lock_path| {
            search_many_with(
                names,
                prefs,
                lock_path,
                SearchOptions::standard(report_failures),
                |message| eprintln!("{message}"),
                default_search_fns(),
            )
        },
    )
}

#[cfg(test)]
pub fn cached_search_with<F>(
    name: &str,
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    mut search: F,
) -> SourceSearchOutcome
where
    F: FnMut(&str, &SourcePreferences, Option<&Path>) -> SourceSearchOutcome,
{
    if let Some(cached) = cached_search_result(name, prefs, cache.as_ref()) {
        return cached.outcome;
    }

    let outcome = search(name, prefs, flake_lock_path(repo_root).as_deref());
    cache_search_results(cache, &outcome.results);

    outcome
}

fn cached_search_many_with_status_using<F>(
    names: &[String],
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    search: F,
) -> HashMap<String, CachedSearchOutcome>
where
    F: Fn(&[String], &SourcePreferences, Option<&Path>) -> HashMap<String, SourceSearchOutcome>,
{
    let unique_names = unique_names(names);
    if unique_names.is_empty() {
        return HashMap::new();
    }

    let mut outcomes = HashMap::with_capacity(unique_names.len());
    let mut uncached_names = Vec::new();

    for name in unique_names {
        if let Some(cached) = cached_search_result(&name, prefs, cache.as_ref()) {
            outcomes.insert(name, cached);
            continue;
        }
        uncached_names.push(name);
    }

    if uncached_names.is_empty() {
        return outcomes;
    }

    let flake_lock_path = flake_lock_path(repo_root);
    let mut fresh = search(&uncached_names, prefs, flake_lock_path.as_deref());
    let mut fresh_results = Vec::new();
    for name in uncached_names {
        let outcome = fresh.remove(&name).unwrap_or_default();
        fresh_results.extend(outcome.results.iter().cloned());
        outcomes.insert(
            name,
            CachedSearchOutcome {
                outcome,
                cache_hit: false,
            },
        );
    }

    cache_search_results(cache, &fresh_results);

    outcomes
}

fn cached_search_result(
    name: &str,
    prefs: &SourcePreferences,
    cache: Option<&MultiSourceCache>,
) -> Option<CachedSearchOutcome> {
    let cached = cache?.get_all_with_prefs(name, prefs);
    (!cached.is_empty()).then_some(CachedSearchOutcome {
        outcome: SourceSearchOutcome {
            results: cached,
            unavailable_sources: Vec::new(),
        },
        cache_hit: true,
    })
}

fn flake_lock_path(repo_root: &Path) -> Option<std::path::PathBuf> {
    let flake_lock = repo_root.join("flake.lock");
    flake_lock.exists().then_some(flake_lock)
}

fn cache_search_results(cache: &mut Option<MultiSourceCache>, results: &[SourceResult]) {
    if !results.is_empty()
        && let Some(cache_ref) = cache.as_mut()
    {
        let _ = cache_ref.set_many(results);
    }
}

fn unique_names(names: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut unique = Vec::with_capacity(names.len());

    for name in names {
        if seen.insert(name.clone()) {
            unique.push(name.clone());
        }
    }

    unique
}

fn search_worker_limit() -> usize {
    thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get)
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as FmtWrite;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
        let outcome = search_flake_inputs("rust", &lock);
        assert!(
            !outcome.results.is_empty(),
            "should find rust in fenix overlay"
        );
        assert_eq!(outcome.results[0].source, PackageSource::FlakeInput);
    }

    #[test]
    fn flake_inputs_empty_for_unknown_package() {
        let dir = tempfile::tempdir().unwrap();
        let lock = make_flake_lock(&dir, &["fenix"]);
        let outcome = search_flake_inputs("obscure-pkg-xyz", &lock);
        assert!(outcome.results.is_empty());
        assert!(outcome.unavailable_reason.is_none());
    }

    #[test]
    fn flake_inputs_missing_lock_returns_empty() {
        let outcome = search_flake_inputs("rust", Path::new("/nonexistent/flake.lock"));
        assert!(outcome.results.is_empty());
        assert!(
            outcome
                .unavailable_reason
                .is_some_and(|reason| reason.contains("failed to read flake.lock"))
        );
    }

    #[test]
    fn flake_inputs_neovim_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let lock = make_flake_lock(&dir, &["neovim-nightly-overlay"]);
        let outcome = search_flake_inputs("neovim", &lock);
        assert!(!outcome.results.is_empty());
        assert!(outcome.results[0].confidence >= 0.7);
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

    // --- SearchExecutor ---

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
        SourceBackendOutcome::from_results(vec![stub_result(PackageSource::Nxs, "slow-nxs")])
    }

    fn stub_nur_fast(_name: &str) -> SearchCallResult {
        SourceBackendOutcome::from_results(vec![stub_result(PackageSource::Nur, "fast-nur")])
    }

    fn stub_nxs_failed(_name: &str) -> SearchCallResult {
        panic!("stub nxs failure");
    }

    fn stub_flake_empty(_name: &str, _path: &Path) -> SearchCallResult {
        SourceBackendOutcome::default()
    }

    fn stub_source_empty(_name: &str) -> SearchCallResult {
        SourceBackendOutcome::default()
    }

    #[test]
    fn executor_cancels_queued_work_at_the_deadline() {
        let starts = Arc::new(AtomicUsize::new(0));
        let first_starts = Arc::clone(&starts);
        let second_starts = Arc::clone(&starts);
        let task = |id, starts: Arc<AtomicUsize>, delay| SearchTask {
            id,
            package: "ripgrep".to_string(),
            kind: SearchTaskKind::Source(PackageSource::Nxs),
            search: Box::new(move || {
                starts.fetch_add(1, Ordering::SeqCst);
                sleep(delay);
                SourceBackendOutcome::default()
            }),
        };

        let started = Instant::now();
        let execution = SearchExecutor {
            worker_limit: 1,
            timeout: Duration::from_millis(30),
        }
        .execute(vec![
            task(0, first_starts, Duration::from_millis(120)),
            task(1, second_starts, Duration::ZERO),
        ]);

        assert!(started.elapsed() < Duration::from_millis(90));
        assert_eq!(execution.completed.len(), 0);
        assert_eq!(execution.timed_out.len(), 2);
        sleep(Duration::from_millis(120));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    fn cache_result(name: &str, source: PackageSource, attr: &str) -> SourceResult {
        SourceResult {
            name: name.to_string(),
            source,
            attr: Some(attr.to_string()),
            version: None,
            confidence: 1.0,
            description: "cached".to_string(),
            requires_flake_mod: false,
            flake_url: None,
        }
    }

    #[test]
    fn executor_timeout_returns_partial_results_and_warns() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();
        let started = Instant::now();

        let results = search_one_with(
            "ripgrep",
            &prefs,
            None,
            SearchOptions {
                report_failures: true,
                timeout: Duration::from_millis(40),
                worker_limit: 5,
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_slow,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
                homebrew_formula: stub_source_empty,
                homebrew_cask: stub_source_empty,
            },
        );

        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(results.results.len(), 1);
        assert_eq!(results.results[0].source, PackageSource::Nur);
        assert_eq!(results.unavailable_sources.len(), 1);
        assert_eq!(results.unavailable_sources[0].source, "nxs");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("timed out waiting for nxs search")),
            "expected timeout warning, got: {warnings:?}"
        );
    }

    #[test]
    fn cached_search_many_reuses_cache_hits_and_only_searches_misses() {
        let tmp = tempfile::tempdir().expect("temp dir should be created");
        let repo_root = tmp.path().join("repo");
        let cache_root = tmp.path().join("cache");
        fs::create_dir_all(&repo_root).expect("repo dir should be created");
        fs::create_dir_all(&cache_root).expect("cache dir should be created");

        let mut cache = Some(
            MultiSourceCache::load_with_cache_dir(&repo_root, &cache_root)
                .expect("cache should load"),
        );
        cache
            .as_mut()
            .expect("cache should exist")
            .set_many(&[cache_result("ripgrep", PackageSource::Nxs, "ripgrep")])
            .expect("cached result should save");

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let call_log = std::sync::Arc::clone(&calls);
        let prefs = SourcePreferences::default();
        let names = vec![
            "ripgrep".to_string(),
            "fd".to_string(),
            "ripgrep".to_string(),
        ];

        let outcomes = cached_search_many_with_status_using(
            &names,
            &prefs,
            &repo_root,
            &mut cache,
            move |names, _prefs, _flake_lock_path| {
                names
                    .iter()
                    .map(|name| {
                        call_log
                            .lock()
                            .expect("call log should not be poisoned")
                            .push(name.clone());
                        (
                            name.clone(),
                            SourceSearchOutcome {
                                results: vec![cache_result(name, PackageSource::Nxs, name)],
                                unavailable_sources: Vec::new(),
                            },
                        )
                    })
                    .collect()
            },
        );

        let logged_calls = calls.lock().expect("call log should not be poisoned");
        assert_eq!(logged_calls.as_slice(), ["fd"]);
        assert!(
            outcomes
                .get("ripgrep")
                .is_some_and(|outcome| outcome.cache_hit),
            "expected cached ripgrep lookup to stay a cache hit"
        );
        assert!(
            outcomes.get("fd").is_some_and(|outcome| !outcome.cache_hit),
            "expected uncached fd lookup to be searched live"
        );
        assert!(
            cache
                .as_ref()
                .expect("cache should exist")
                .get("fd", PackageSource::Nxs)
                .is_some(),
            "expected fresh miss results to be written back to cache"
        );
    }

    #[test]
    fn executor_timeout_quiet_suppresses_warning() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let results = search_one_with(
            "ripgrep",
            &prefs,
            None,
            SearchOptions {
                report_failures: false,
                timeout: Duration::from_millis(40),
                worker_limit: 5,
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_slow,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
                homebrew_formula: stub_source_empty,
                homebrew_cask: stub_source_empty,
            },
        );

        assert_eq!(results.results.len(), 1);
        assert!(warnings.is_empty(), "warnings should be suppressed");
    }

    #[test]
    fn executor_source_failure_keeps_other_results_and_warns() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let results = search_one_with(
            "ripgrep",
            &prefs,
            None,
            SearchOptions {
                report_failures: true,
                timeout: Duration::from_millis(200),
                worker_limit: 5,
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_failed,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
                homebrew_formula: stub_source_empty,
                homebrew_cask: stub_source_empty,
            },
        );

        assert_eq!(results.results.len(), 1);
        assert_eq!(results.results[0].source, PackageSource::Nur);
        assert_eq!(results.unavailable_sources.len(), 1);
        assert_eq!(results.unavailable_sources[0].source, "nxs");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("nxs search unavailable")),
            "expected source-failure warning, got: {warnings:?}"
        );
    }

    #[test]
    fn executor_source_failure_quiet_suppresses_warning() {
        let prefs = SourcePreferences {
            nur: true,
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let results = search_one_with(
            "ripgrep",
            &prefs,
            None,
            SearchOptions {
                report_failures: false,
                timeout: Duration::from_millis(200),
                worker_limit: 5,
            },
            |message| warnings.push(message.to_string()),
            SearchFns {
                nxs: stub_nxs_failed,
                flake_inputs: stub_flake_empty,
                nur: stub_nur_fast,
                homebrew_formula: stub_source_empty,
                homebrew_cask: stub_source_empty,
            },
        );

        assert_eq!(results.results.len(), 1);
        assert!(warnings.is_empty(), "warnings should be suppressed");
    }
}
