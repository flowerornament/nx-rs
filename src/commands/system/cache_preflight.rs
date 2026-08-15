use crate::commands::context::SystemContext;
use crate::domain::manifest::PlatformKind;
use crate::infra::shell::{first_nonempty_output, run_captured_command, terminal_stdio_available};
use crate::output::printer::Printer;
use serde_json::{Map, Value};
use std::path::Path;

const CACHE_MISS_THRESHOLD_ENV: &str = "NX_CACHE_MISS_THRESHOLD";
const DEFAULT_CACHE_MISS_THRESHOLD: usize = 5;
const MAX_LISTED_SOURCE_BUILDS: usize = 10;

/// How the cache preflight should react when source builds exceed the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CachePreflightMode {
    /// Report and warn without prompting.
    ReportOnly,
    /// Require usable coverage or explicit approval before continuing.
    RequireApproval,
    /// Admit a recognized build plan without prompting.
    ApproveSourceBuilds,
    /// Proceed even when cache coverage cannot be established.
    Bypass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CachePreflightOutcome {
    Admitted,
    Cancelled,
    Failed,
}

/// Build plan parsed from `nix build --dry-run` output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DryRunPlan {
    /// Derivation paths that Nix plans to build locally.
    pub(super) to_build: Vec<String>,
    /// Number of store paths that will be fetched from a binary cache.
    pub(super) to_fetch: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CachePlan {
    pub(super) source_builds: Vec<String>,
    pub(super) local_builds: Vec<String>,
    pub(super) to_fetch: usize,
}

/// Dry-run the system build and warn when too many derivations miss the cache.
///
/// Report-only checks remain advisory. Enforced checks reject unavailable
/// coverage and excessive source builds unless the user explicitly approves.
pub(super) fn check_cache_preflight(
    ctx: &SystemContext<'_>,
    mode: CachePreflightMode,
) -> CachePreflightOutcome {
    if !cache_preflight_supported(ctx) {
        return CachePreflightOutcome::Admitted;
    }

    ctx.printer.action("Checking binary cache coverage");
    let Some(host) = super::rebuild::darwin_host(ctx) else {
        ctx.printer
            .warn("Skipping binary cache preflight: could not resolve darwin host");
        return unavailable_outcome(mode);
    };

    let attr = format!(
        "{}#darwinConfigurations.{host}.system",
        ctx.repo_root.display()
    );
    let output = ctx
        .printer
        .with_loading("Planning build with nix --dry-run", |_| {
            run_captured_command("nix", &["build", &attr, "--dry-run"], None)
        });

    let output = match output {
        Ok(output) if output.code == 0 => output,
        Ok(output) => {
            ctx.printer.warn("Cache preflight dry-run failed");
            let detail = first_nonempty_output(&output);
            if !detail.is_empty() {
                Printer::detail(detail);
            }
            return unavailable_outcome(mode);
        }
        Err(err) => {
            ctx.printer
                .warn(&format!("Cache preflight dry-run failed: {err:#}"));
            return unavailable_outcome(mode);
        }
    };

    let Some(plan) = parse_dry_run_plan(&format!("{}\n{}", output.stdout, output.stderr)) else {
        ctx.printer
            .warn("Cache preflight output was not recognized");
        return unavailable_outcome(mode);
    };
    let threshold = cache_miss_threshold();
    let plan = classify_dry_run_plan(&plan);
    report_cache_plan(ctx, &plan);

    if plan.source_builds.len() <= threshold {
        return CachePreflightOutcome::Admitted;
    }

    println!();
    ctx.printer.warn(&format!(
        "{} derivations will build from source (threshold: {threshold})",
        plan.source_builds.len()
    ));
    Printer::detail("The candidate system is not sufficiently covered by binary caches.");
    Printer::detail(&format!(
        "Adjust the policy threshold with {CACHE_MISS_THRESHOLD_ENV}=<count>."
    ));

    let interactive = terminal_stdio_available();
    let outcome = source_builds_outcome(mode, interactive, || {
        println!();
        Printer::confirm("Continue with rebuild?", false)
    });
    match outcome {
        CachePreflightOutcome::Admitted if mode == CachePreflightMode::ApproveSourceBuilds => {
            Printer::detail("Continuing because --yes pre-approved this source-build plan.");
        }
        CachePreflightOutcome::Admitted if mode == CachePreflightMode::Bypass => {
            Printer::detail("Continuing because --allow-source-builds was passed.");
        }
        CachePreflightOutcome::Failed => {
            Printer::detail("Non-interactive session; refusing unapproved source builds.");
            Printer::detail("Rerun with --allow-source-builds to proceed explicitly.");
        }
        _ => {}
    }
    outcome
}

pub(super) fn source_builds_outcome(
    mode: CachePreflightMode,
    interactive: bool,
    confirm: impl FnOnce() -> bool,
) -> CachePreflightOutcome {
    match mode {
        CachePreflightMode::ReportOnly
        | CachePreflightMode::ApproveSourceBuilds
        | CachePreflightMode::Bypass => CachePreflightOutcome::Admitted,
        CachePreflightMode::RequireApproval if !interactive => CachePreflightOutcome::Failed,
        CachePreflightMode::RequireApproval if confirm() => CachePreflightOutcome::Admitted,
        CachePreflightMode::RequireApproval => CachePreflightOutcome::Cancelled,
    }
}

pub(super) fn unavailable_outcome(mode: CachePreflightMode) -> CachePreflightOutcome {
    match mode {
        CachePreflightMode::ReportOnly => {
            Printer::detail("Coverage is advisory for rebuild preflight.");
            CachePreflightOutcome::Admitted
        }
        CachePreflightMode::Bypass => {
            Printer::detail("Continuing because --allow-source-builds was passed.");
            CachePreflightOutcome::Admitted
        }
        CachePreflightMode::RequireApproval | CachePreflightMode::ApproveSourceBuilds => {
            Printer::detail("Could not establish binary cache coverage; refusing the upgrade.");
            Printer::detail("Rerun once to confirm after Nix finishes realizing inputs.");
            Printer::detail(
                "Use --allow-source-builds only after independently verifying cache coverage.",
            );
            CachePreflightOutcome::Failed
        }
    }
}

/// The dry-run attribute path is darwin-specific.
fn cache_preflight_supported(ctx: &SystemContext<'_>) -> bool {
    ctx.config_files
        .manifest()
        .is_none_or(|manifest| manifest.platform.kind == PlatformKind::Darwin)
}

fn report_cache_plan(ctx: &SystemContext<'_>, plan: &CachePlan) {
    if plan.source_builds.is_empty() && plan.local_builds.is_empty() {
        ctx.printer.success(&format!(
            "Binary cache covers this build ({} paths to fetch)",
            plan.to_fetch
        ));
        return;
    }

    if !plan.source_builds.is_empty() {
        report_builds("Source Builds", &plan.source_builds);
    }

    if !plan.local_builds.is_empty() {
        report_builds("Local Builds", &plan.local_builds);
        Printer::detail("Nix marks these as cheap or required to build locally.");
    }

    Printer::detail(&format!(
        "{} paths will be fetched from the binary cache",
        plan.to_fetch
    ));
}

fn report_builds(label: &str, builds: &[String]) {
    Printer::body(&format!("{label} ({})", builds.len()));
    for name in builds.iter().take(MAX_LISTED_SOURCE_BUILDS) {
        Printer::body(name);
    }
    if builds.len() > MAX_LISTED_SOURCE_BUILDS {
        Printer::detail(&format!(
            "... and {} more",
            builds.len() - MAX_LISTED_SOURCE_BUILDS
        ));
    }
}

fn classify_dry_run_plan(plan: &DryRunPlan) -> CachePlan {
    if plan.to_build.is_empty() {
        return CachePlan {
            to_fetch: plan.to_fetch,
            ..CachePlan::default()
        };
    }

    let mut args = vec!["derivation", "show"];
    args.extend(plan.to_build.iter().map(String::as_str));
    run_captured_command("nix", &args, None)
        .ok()
        .filter(|output| output.code == 0)
        .and_then(|output| cache_plan_from_metadata(plan, &output.stdout))
        .unwrap_or_else(|| unclassified_cache_plan(plan))
}

pub(super) fn cache_plan_from_metadata(plan: &DryRunPlan, output: &str) -> Option<CachePlan> {
    let value: Value = serde_json::from_str(output).ok()?;
    let derivations = derivation_records(&value)?;
    let mut classified = CachePlan {
        to_fetch: plan.to_fetch,
        ..CachePlan::default()
    };

    for path in &plan.to_build {
        let key = derivation_key(path);
        let record = derivations.get(&key).or_else(|| derivations.get(path))?;
        let target = if is_nix_local_build(record)? {
            &mut classified.local_builds
        } else {
            &mut classified.source_builds
        };
        target.push(derivation_display_name(path));
    }

    Some(classified)
}

pub(super) fn unclassified_cache_plan(plan: &DryRunPlan) -> CachePlan {
    CachePlan {
        source_builds: plan
            .to_build
            .iter()
            .map(|path| derivation_display_name(path))
            .collect(),
        to_fetch: plan.to_fetch,
        ..CachePlan::default()
    }
}

fn derivation_records(value: &Value) -> Option<&Map<String, Value>> {
    value
        .get("derivations")
        .and_then(Value::as_object)
        .or_else(|| value.as_object())
}

fn is_nix_local_build(record: &Value) -> Option<bool> {
    let env = record.get("env").and_then(Value::as_object);
    let structured = record.get("structuredAttrs").and_then(Value::as_object);
    if env.is_none() && structured.is_none() {
        return None;
    }

    let allow_substitutes = nix_bool(structured, env, "allowSubstitutes").unwrap_or(true);
    let prefer_local = nix_bool(structured, env, "preferLocalBuild").unwrap_or(false);
    Some(!allow_substitutes || prefer_local)
}

fn nix_bool(
    structured: Option<&Map<String, Value>>,
    env: Option<&Map<String, Value>>,
    field: &str,
) -> Option<bool> {
    structured
        .and_then(|attrs| attrs.get(field))
        .and_then(Value::as_bool)
        .or_else(|| {
            env.and_then(|attrs| attrs.get(field))
                .and_then(Value::as_str)
                .map(|value| !value.is_empty())
        })
}

pub(super) fn derivation_key(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

/// Parse the `will be built` / `will be fetched` sections of dry-run output.
pub(super) fn parse_dry_run_plan(output: &str) -> Option<DryRunPlan> {
    #[derive(PartialEq, Eq)]
    enum Section {
        None,
        Build,
        Fetch,
    }

    let mut section = Section::None;
    let mut plan = DryRunPlan::default();
    let mut saw_plan_section = false;
    let mut saw_unrecognized = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if is_build_section_header(trimmed) {
            section = Section::Build;
            saw_plan_section = true;
        } else if is_fetch_section_header(trimmed) {
            section = Section::Fetch;
            saw_plan_section = true;
        } else if trimmed.starts_with("/nix/store/") {
            match section {
                Section::Build => plan.to_build.push(trimmed.to_string()),
                Section::Fetch => plan.to_fetch += 1,
                Section::None => saw_unrecognized = true,
            }
        } else if is_dry_run_diagnostic(trimmed) {
            section = Section::None;
        } else if !trimmed.is_empty() {
            saw_unrecognized = true;
            section = Section::None;
        }
    }

    (saw_plan_section || !saw_unrecognized).then_some(plan)
}

fn is_dry_run_diagnostic(line: &str) -> bool {
    line.starts_with("warning:")
        || line.starts_with("trace:")
        || (line.starts_with("unpacking '") && line.ends_with("' into the Git cache..."))
}

fn is_build_section_header(line: &str) -> bool {
    line.starts_with("this derivation will be built")
        || (line.starts_with("these ") && line.contains(" derivations will be built"))
}

fn is_fetch_section_header(line: &str) -> bool {
    line.starts_with("this path will be fetched")
        || (line.starts_with("these ") && line.contains(" paths will be fetched"))
}

/// Strip `/nix/store/<hash>-` prefix and `.drv` suffix from a store path.
pub(super) fn derivation_display_name(store_path: &str) -> String {
    crate::infra::nix_output::store_path_display_name(store_path)
}

fn cache_miss_threshold() -> usize {
    parse_cache_miss_threshold(std::env::var(CACHE_MISS_THRESHOLD_ENV).ok().as_deref())
}

/// Parse the source-build threshold, falling back to the default on bad input.
pub(super) fn parse_cache_miss_threshold(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_CACHE_MISS_THRESHOLD)
}
