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
    /// Ask the user whether to continue (interactive sessions only).
    Prompt,
    /// Report and warn without prompting.
    ReportOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CachePreflightOutcome {
    Continue,
    Abort,
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
/// Never fails the surrounding command: dry-run errors are warnings because
/// the real rebuild remains authoritative. Returns `Abort` only when the user
/// declines the interactive prompt in [`CachePreflightMode::Prompt`].
pub(super) fn check_cache_preflight(
    ctx: &SystemContext<'_>,
    mode: CachePreflightMode,
) -> CachePreflightOutcome {
    if !cache_preflight_supported(ctx) {
        return CachePreflightOutcome::Continue;
    }

    let Some(host) = super::rebuild::darwin_host(ctx) else {
        ctx.printer
            .warn("Skipping binary cache preflight: could not resolve darwin host");
        return CachePreflightOutcome::Continue;
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
            ctx.printer
                .warn("Cache preflight dry-run failed; rebuild will report details");
            let detail = first_nonempty_output(&output);
            if !detail.is_empty() {
                Printer::detail(detail);
            }
            return CachePreflightOutcome::Continue;
        }
        Err(err) => {
            ctx.printer
                .warn(&format!("Cache preflight dry-run failed: {err:#}"));
            return CachePreflightOutcome::Continue;
        }
    };

    let plan = parse_dry_run_plan(&format!("{}\n{}", output.stdout, output.stderr));
    report_dry_run_plan(ctx, &plan);

    let threshold = cache_miss_threshold();
    if plan.to_build.len() <= threshold {
        return CachePreflightOutcome::Continue;
    }

    println!();
    ctx.printer.warn(&format!(
        "{} derivations will build from source (threshold: {threshold})",
        plan.to_build.len()
    ));
    Printer::detail("The binary cache has likely not caught up with the new nixpkgs revision.");
    Printer::detail(&format!(
        "Raise the threshold with {CACHE_MISS_THRESHOLD_ENV}=<count> to silence this warning."
    ));

    if mode == CachePreflightMode::ReportOnly {
        return CachePreflightOutcome::Continue;
    }
    if !terminal_stdio_available() {
        Printer::detail("Non-interactive session; continuing with source builds");
        return CachePreflightOutcome::Continue;
    }

    println!();
    if Printer::confirm("Continue with rebuild?", false) {
        CachePreflightOutcome::Continue
    } else {
        CachePreflightOutcome::Abort
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
pub(super) fn parse_dry_run_plan(output: &str) -> DryRunPlan {
    #[derive(PartialEq, Eq)]
    enum Section {
        None,
        Build,
        Fetch,
    }

    let mut section = Section::None;
    let mut plan = DryRunPlan::default();

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
                Section::None => {}
            }
        } else if !trimmed.is_empty() {
            section = Section::None;
        }
    }

    plan
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
