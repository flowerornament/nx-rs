use crate::commands::context::SystemContext;
use crate::domain::manifest::PlatformKind;
use crate::infra::shell::{first_nonempty_output, run_captured_command, terminal_stdio_available};
use crate::output::printer::Printer;

const CACHE_MISS_THRESHOLD_ENV: &str = "NX_CACHE_MISS_THRESHOLD";
const DEFAULT_CACHE_MISS_THRESHOLD: usize = 5;
const MAX_LISTED_SOURCE_BUILDS: usize = 10;

/// How the cache preflight should react when source builds exceed the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CachePreflightMode {
    /// Report and warn without prompting.
    ReportOnly,
    /// Require usable coverage or explicit approval before continuing.
    Enforce,
    /// Proceed despite excessive or unavailable cache coverage.
    AllowSourceBuilds,
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
    /// Derivation names that will be built from source (cache misses).
    pub(super) to_build: Vec<String>,
    /// Number of store paths that will be fetched from a binary cache.
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

    let Some(host) = super::rebuild::darwin_host(ctx) else {
        ctx.printer
            .warn("Skipping binary cache preflight: could not resolve darwin host");
        return unavailable_outcome(mode);
    };

    ctx.printer.action("Checking binary cache coverage");
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
    report_dry_run_plan(ctx, &plan);

    let threshold = cache_miss_threshold();
    if plan.to_build.len() <= threshold {
        return CachePreflightOutcome::Admitted;
    }

    println!();
    ctx.printer.warn(&format!(
        "{} derivations will build from source (threshold: {threshold})",
        plan.to_build.len()
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
        CachePreflightOutcome::Admitted if mode == CachePreflightMode::AllowSourceBuilds => {
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
        CachePreflightMode::ReportOnly | CachePreflightMode::AllowSourceBuilds => {
            CachePreflightOutcome::Admitted
        }
        CachePreflightMode::Enforce if !interactive => CachePreflightOutcome::Failed,
        CachePreflightMode::Enforce if confirm() => CachePreflightOutcome::Admitted,
        CachePreflightMode::Enforce => CachePreflightOutcome::Cancelled,
    }
}

pub(super) fn unavailable_outcome(mode: CachePreflightMode) -> CachePreflightOutcome {
    match mode {
        CachePreflightMode::ReportOnly => {
            Printer::detail("Coverage is advisory for rebuild preflight.");
            CachePreflightOutcome::Admitted
        }
        CachePreflightMode::AllowSourceBuilds => {
            Printer::detail("Continuing because --allow-source-builds was passed.");
            CachePreflightOutcome::Admitted
        }
        CachePreflightMode::Enforce => {
            Printer::detail("Could not establish binary cache coverage; refusing the upgrade.");
            Printer::detail("Rerun with --allow-source-builds to proceed explicitly.");
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

fn report_dry_run_plan(ctx: &SystemContext<'_>, plan: &DryRunPlan) {
    if plan.to_build.is_empty() {
        ctx.printer.success(&format!(
            "Binary cache covers this build ({} paths to fetch)",
            plan.to_fetch
        ));
        return;
    }

    Printer::body(&format!("Source Builds ({})", plan.to_build.len()));
    for name in plan.to_build.iter().take(MAX_LISTED_SOURCE_BUILDS) {
        Printer::body(name);
    }
    if plan.to_build.len() > MAX_LISTED_SOURCE_BUILDS {
        Printer::detail(&format!(
            "... and {} more",
            plan.to_build.len() - MAX_LISTED_SOURCE_BUILDS
        ));
    }
    Printer::detail(&format!(
        "{} paths will be fetched from the binary cache",
        plan.to_fetch
    ));
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
    let mut recognized = true;

    for line in output.lines() {
        let trimmed = line.trim();
        if is_build_section_header(trimmed) {
            section = Section::Build;
        } else if is_fetch_section_header(trimmed) {
            section = Section::Fetch;
        } else if trimmed.starts_with("/nix/store/") {
            match section {
                Section::Build => plan.to_build.push(derivation_display_name(trimmed)),
                Section::Fetch => plan.to_fetch += 1,
                Section::None => recognized = false,
            }
        } else if trimmed.starts_with("warning:") || trimmed.starts_with("trace:") {
            section = Section::None;
        } else if !trimmed.is_empty() {
            recognized = false;
            section = Section::None;
        }
    }

    recognized.then_some(plan)
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
