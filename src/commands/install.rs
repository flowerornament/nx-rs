use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

use crate::cli::{InstallArgs, RebuildArgs};
use crate::commands::context::AppContext;
use crate::commands::shared::{
    SnippetMode, missing_argument_error, relative_location, show_dry_run_preview, show_snippet,
};
use crate::commands::system::cmd_rebuild_with_command;
use crate::domain::location::PackageLocation;
use crate::domain::plan::{
    InsertionMode, InstallPlan, build_install_plan, nix_manifest_candidates,
};
use crate::domain::source::{
    ExplicitSourceTarget, PackageSource, SourcePreferences, SourceResult, detect_language_package,
};
use crate::infra::ai_engine::{
    AiEngine, ClaudeCodeEngine, CommandOutcome, build_edit_prompt, build_routing_context,
    run_edit_with_callback, select_engine,
};
use crate::infra::cache::MultiSourceCache;
use crate::infra::file_edit::{EditOutcome, analyse_manifest_for_preview, apply_edit};
use crate::infra::finder::{find_first_package, find_package};
use crate::infra::flake_input::{FlakeInputEdit, add_flake_input};
use crate::infra::package_query::{PackageQueryReport, query_package, query_packages};
use crate::infra::shell::git_diff;
use crate::infra::sources::check_nix_available;
use crate::infra::text::truncate_with_ellipsis as truncate_text;
use crate::infra::timing::TimingCommand;
use crate::output::printer::Printer;

pub fn cmd_install(args: &InstallArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        return missing_argument_error("install", "PACKAGES...");
    }

    if args.dry_run() {
        ctx.printer.dry_run_banner();
    }

    let pkg_list = if args.packages.len() <= 3 {
        args.packages.join(", ")
    } else {
        format!(
            "{}, ... ({} total)",
            args.packages[..3].join(", "),
            args.packages.len()
        )
    };
    ctx.printer.action(&format!("Installing {pkg_list}"));

    let engine = select_engine(args.engine(), args.model(), ctx.printer.style());
    let routing_context = InstallRoutingContext::build(ctx);
    let mut cache = load_cache(ctx);
    let prefetched_searches = prefetch_install_searches(args, ctx, &mut cache);

    let mut success_count = 0;

    for package in &args.packages {
        if install_one(
            package,
            args,
            ctx,
            &mut cache,
            &prefetched_searches,
            engine.as_ref(),
            &routing_context,
        ) {
            success_count += 1;
        }
    }

    run_post_install_actions(success_count, args, ctx, || {
        let rebuild = RebuildArgs::default();
        let system_ctx = ctx.system_context();
        cmd_rebuild_with_command(&rebuild, &system_ctx, TimingCommand::Install)
    });

    i32::from(success_count != args.packages.len())
}

fn run_post_install_actions<F>(
    success_count: usize,
    args: &InstallArgs,
    _ctx: &AppContext,
    rebuild: F,
) where
    F: FnOnce() -> i32,
{
    if success_count == 0 || args.dry_run() {
        return;
    }

    println!();
    Printer::detail("Run: nx rebuild");

    if args.rebuild() {
        let _ = rebuild();
    }
}

/// Install a single package. Returns `true` on success.
fn install_one(
    package: &str,
    args: &InstallArgs,
    ctx: &AppContext,
    cache: &mut Option<MultiSourceCache>,
    prefetched_searches: &InstallSearchPrefetch,
    engine: &dyn AiEngine,
    routing_context: &InstallRoutingContext,
) -> bool {
    let resolved = match start_install_resolution(package, args, ctx, cache, prefetched_searches) {
        InstallStart::Proceed(resolved) => resolved,
        InstallStart::Completed => return true,
        InstallStart::Failed => return false,
    };

    announce_install_phase(args, ctx, resolved.platform_warning.as_deref());

    let Some(prepared) = prepare_install_phase(
        package,
        resolved.source_result,
        args,
        ctx,
        engine,
        routing_context,
    ) else {
        return false;
    };

    apply_install_phase(&prepared, args, ctx, engine)
}

fn start_install_resolution(
    package: &str,
    args: &InstallArgs,
    ctx: &AppContext,
    cache: &mut Option<MultiSourceCache>,
    prefetched_searches: &InstallSearchPrefetch,
) -> InstallStart {
    if let Some(prefetched) = prefetched_searches.get(package) {
        return finish_install_resolution(
            package,
            ctx,
            search_for_package(
                package,
                args,
                ctx,
                cache,
                match prefetched {
                    InstallPrefetchEntry::AlreadyInstalled(location) => {
                        report_already_installed(package, location, ctx);
                        return InstallStart::Completed;
                    }
                    InstallPrefetchEntry::Search(cached) => Some(cached),
                },
            ),
        );
    }

    match find_package(package, &ctx.repo_root) {
        Ok(Some(location)) => {
            report_already_installed(package, &location, ctx);
            return InstallStart::Completed;
        }
        Ok(None) => {}
        Err(err) => {
            ctx.printer.error(&format!("install lookup failed: {err}"));
            return InstallStart::Failed;
        }
    }

    finish_install_resolution(
        package,
        ctx,
        search_for_package(package, args, ctx, cache, None),
    )
}

fn finish_install_resolution(
    package: &str,
    ctx: &AppContext,
    resolution: Option<SearchResolution>,
) -> InstallStart {
    let Some(resolution) = resolution else {
        return InstallStart::Failed;
    };

    match resolution {
        SearchResolution::Install {
            result,
            platform_warning,
        } => InstallStart::Proceed(ResolvedInstall {
            source_result: result,
            platform_warning,
        }),
        SearchResolution::AlreadyInstalled(location) => {
            report_already_installed(package, &location, ctx);
            InstallStart::Completed
        }
        SearchResolution::Skipped => InstallStart::Completed,
    }
}

fn announce_install_phase(args: &InstallArgs, ctx: &AppContext, platform_warning: Option<&str>) {
    println!();
    if args.dry_run() {
        Printer::detail("Analyzing (1)");
    } else {
        Printer::detail("Installing (1)");
    }

    if let Some(warning) = platform_warning {
        ctx.printer.warn(warning);
    }
}

fn prepare_install_phase(
    package: &str,
    source_result: SourceResult,
    args: &InstallArgs,
    ctx: &AppContext,
    engine: &dyn AiEngine,
    routing_context: &InstallRoutingContext,
) -> Option<PreparedInstall> {
    let mut plan = match build_install_plan(&source_result, &ctx.config_files) {
        Ok(plan) => plan,
        Err(err) => {
            ctx.printer.error(&format!("{package}: {err}"));
            return None;
        }
    };

    refine_routing(&mut plan, engine, routing_context, ctx);

    if !gate_flake_input(package, &plan, args, ctx, engine) {
        return None;
    }

    ctx.printer
        .action(&format!("Routing {}", source_result.name));

    if let Some(ref warning) = plan.routing_warning {
        ctx.printer.warn(warning);
    }

    let rel_target = plan
        .target_file
        .strip_prefix(&ctx.repo_root)
        .unwrap_or(&plan.target_file)
        .display()
        .to_string();

    Some(PreparedInstall {
        source_name: source_result.name,
        source_description: source_result.description,
        plan,
        rel_target,
    })
}

fn apply_install_phase(
    prepared: &PreparedInstall,
    args: &InstallArgs,
    ctx: &AppContext,
    engine: &dyn AiEngine,
) -> bool {
    if args.dry_run() {
        render_dry_run_install(prepared, ctx);
        maybe_setup_service(&prepared.source_name, args, ctx);
        return true;
    }

    let installed = execute_edit(&prepared.plan, &prepared.rel_target, ctx, engine);
    if installed {
        maybe_setup_service(&prepared.source_name, args, ctx);
    }
    installed
}

fn render_dry_run_install(prepared: &PreparedInstall, ctx: &AppContext) {
    if prepared.plan.insertion_mode == InsertionMode::NixManifest
        && let Ok(content) = fs::read_to_string(&prepared.plan.target_file)
        && let Some(info) = analyse_manifest_for_preview(&content, &prepared.plan.package_token)
    {
        let simulated_line = build_simulated_preview_line(
            &prepared.plan.package_token,
            &prepared.source_description,
            info.comment_column,
        );
        show_dry_run_preview(
            &prepared.plan.target_file,
            info.insert_after_line,
            &simulated_line,
            1,
        );
    }

    println!();
    if let Some(language_info) = &prepared.plan.language_info {
        ctx.printer.success(&format!(
            "Would add '{}' to {}.withPackages in {}",
            language_info.bare_name, language_info.runtime, prepared.rel_target
        ));
    } else {
        ctx.printer.success(&format!(
            "Would add {} to {}",
            prepared.plan.package_token, prepared.rel_target
        ));
    }
}

/// Refine routing for general nix packages via AI engine.
fn refine_routing(
    plan: &mut InstallPlan,
    engine: &dyn AiEngine,
    routing_context: &InstallRoutingContext,
    ctx: &AppContext,
) {
    if plan.routing_warning.is_none() || plan.insertion_mode != InsertionMode::NixManifest {
        return;
    }

    let fallback = plan
        .target_file
        .strip_prefix(&ctx.repo_root)
        .unwrap_or(&plan.target_file)
        .to_string_lossy()
        .to_string();

    let decision = engine.route_package(
        &plan.package_token,
        &plan.source_result.description,
        routing_context.route_context(),
        &routing_context.candidates,
        &fallback,
        &ctx.repo_root,
    );

    plan.target_file = ctx.repo_root.join(&decision.target_file);
    plan.routing_warning = decision.warning;
}

/// Handle flake input gating (SPEC 7.5). Returns `true` to proceed, `false` to skip.
fn gate_flake_input(
    package: &str,
    plan: &InstallPlan,
    args: &InstallArgs,
    ctx: &AppContext,
    engine: &dyn AiEngine,
) -> bool {
    if !plan.source_result.requires_flake_mod {
        return true;
    }
    if !engine.supports_flake_input() {
        ctx.printer.warn(&format!(
            "{package} requires flake.nix modification - use --engine=claude"
        ));
        return false;
    }

    ctx.printer
        .warn(&format!("{package} requires flake.nix modification"));
    let Some(flake_url) = plan.source_result.flake_url.as_deref() else {
        ctx.printer.error(&format!(
            "Failed to add flake input: missing flake URL for {package}"
        ));
        return false;
    };
    Printer::detail(&format!("URL: {flake_url}"));

    if args.dry_run() {
        Printer::detail(&format!("[DRY RUN] Would add flake input for {package}"));
        return true; // counted as success in dry-run
    }
    if !args.yes() && !Printer::confirm("Add flake input?", true) {
        ctx.printer.warn(&format!("Skipping {package}"));
        return false;
    }

    let flake_path = ctx.repo_root.join("flake.nix");
    match add_flake_input(&flake_path, flake_url, None) {
        Ok(FlakeInputEdit::Added { input_name }) => {
            Printer::detail(&format!("added input '{input_name}'"));
            true
        }
        Ok(FlakeInputEdit::AlreadyExists { input_name }) => {
            Printer::detail(&format!("input '{input_name}' already exists"));
            true
        }
        Err(err) => {
            ctx.printer
                .error(&format!("Failed to add flake input: {err}"));
            false
        }
    }
}

/// Execute install edits per engine semantics (SPEC 7.7).
fn execute_edit(
    plan: &InstallPlan,
    rel_target: &str,
    ctx: &AppContext,
    engine: &dyn AiEngine,
) -> bool {
    let prompt = build_edit_prompt(plan);
    let before_diff = git_diff(&ctx.repo_root);
    let mut deterministic: Option<anyhow::Result<EditOutcome>> = None;

    let execution =
        run_edit_with_callback(engine, &prompt, &ctx.repo_root, || match apply_edit(plan) {
            Ok(outcome) => {
                deterministic = Some(Ok(outcome));
                Some(CommandOutcome {
                    success: true,
                    output: "deterministic edit applied".to_string(),
                })
            }
            Err(err) if should_fallback_to_ai(engine, &err) => None,
            Err(err) => {
                let message = err.to_string();
                deterministic = Some(Err(err));
                Some(CommandOutcome {
                    success: false,
                    output: message,
                })
            }
        });

    if let Some(result) = deterministic {
        return report_deterministic_edit(result, plan, rel_target, ctx);
    }

    if !execution.outcome.success {
        ctx.printer.error(&format!(
            "failed to edit {rel_target}: {}",
            execution.outcome.output
        ));
        return false;
    }

    let after_diff = git_diff(&ctx.repo_root);
    if after_diff == before_diff {
        println!();
        ctx.printer.success(&format!(
            "'{}' already present in {rel_target}",
            plan.package_token,
        ));
        return true;
    }

    println!();
    ctx.printer
        .success(&format!("Added '{}' to {rel_target}", plan.package_token));
    if let Ok(Some(location)) = find_package(&plan.package_token, &ctx.repo_root)
        && let Some(line) = location.line()
    {
        show_snippet(location.path(), line, 2, SnippetMode::Add, false);
    }
    true
}

fn should_fallback_to_ai(engine: &dyn AiEngine, err: &anyhow::Error) -> bool {
    engine.name() == "claude" && is_unsupported_edit_shape(err)
}

fn is_unsupported_edit_shape(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.starts_with("no ")
        && (message.contains("list found") || message.contains("block found"))
}

fn report_deterministic_edit(
    result: anyhow::Result<EditOutcome>,
    plan: &InstallPlan,
    rel_target: &str,
    ctx: &AppContext,
) -> bool {
    match result {
        Ok(outcome) => {
            println!();
            if outcome.file_changed {
                ctx.printer
                    .success(&format!("Added '{}' to {rel_target}", plan.package_token));
                if let Some(line) = outcome.line_number {
                    show_snippet(&plan.target_file, line, 2, SnippetMode::Add, false);
                }
            } else {
                ctx.printer.success(&format!(
                    "'{}' already present in {rel_target}",
                    plan.package_token,
                ));
            }
            true
        }
        Err(err) => {
            ctx.printer
                .error(&format!("failed to edit {rel_target}: {err}"));
            false
        }
    }
}

fn maybe_setup_service(package_name: &str, args: &InstallArgs, ctx: &AppContext) {
    maybe_setup_service_with(package_name, args, ctx, |prompt| {
        let service_engine = ClaudeCodeEngine::new(args.model(), ctx.printer.style());
        service_engine.run_edit(prompt, &ctx.repo_root)
    });
}

fn maybe_setup_service_with<F>(
    package_name: &str,
    args: &InstallArgs,
    ctx: &AppContext,
    mut run_service_edit: F,
) where
    F: FnMut(&str) -> CommandOutcome,
{
    if !args.service() {
        return;
    }

    if args.dry_run() {
        Printer::detail(&format!(
            "[DRY RUN] Would add launchd.agents.{package_name}"
        ));
        return;
    }

    let services_path = ctx.config_files.services();
    let services_target = services_path
        .strip_prefix(&ctx.repo_root)
        .unwrap_or(services_path.as_path())
        .display()
        .to_string();
    let prompt = build_service_prompt(package_name, &services_target);
    let outcome = run_service_edit(&prompt);

    if outcome.success {
        ctx.printer
            .success(&format!("launchd.agents.{package_name} added"));
        return;
    }

    let message = outcome.output.trim();
    if message.is_empty() {
        ctx.printer.warn("Service setup failed: unknown error");
    } else {
        ctx.printer
            .warn(&format!("Service setup failed: {message}"));
    }
}

fn build_service_prompt(name: &str, services_file: &str) -> String {
    format!(
        "Add a launchd agent for {name} to {services_file}.\n\n\
         Read the existing file to understand the pattern, then create a service configuration.\n\
         The binary is likely at /opt/homebrew/opt/{name}/bin/{name} or in the nix store.\n\n\
         Use the Edit tool to add the configuration."
    )
}

/// Map CLI flags to source preferences for search.
fn source_prefs_from_args(args: &InstallArgs) -> SourcePreferences {
    SourcePreferences {
        bleeding_edge: args.bleeding_edge(),
        nur: args.nur(),
        force_source: args.source().map(str::to_owned),
        explicit_target: ExplicitSourceTarget::from_flags(args.cask(), args.mas()),
    }
}

fn load_cache(ctx: &AppContext) -> Option<MultiSourceCache> {
    match MultiSourceCache::load(&ctx.repo_root) {
        Ok(cache) => Some(cache),
        Err(err) => {
            ctx.printer.warn(&format!(
                "cache unavailable; continuing without cache: {err}"
            ));
            None
        }
    }
}

fn report_already_installed(package: &str, location: &PackageLocation, ctx: &AppContext) {
    println!();
    ctx.printer.success(&format!(
        "{package} already installed ({})",
        relative_location(location, &ctx.repo_root)
    ));
}

enum SearchResolution {
    Install {
        result: SourceResult,
        platform_warning: Option<String>,
    },
    AlreadyInstalled(PackageLocation),
    Skipped,
}

enum InstallStart {
    Proceed(ResolvedInstall),
    Completed,
    Failed,
}

struct ResolvedInstall {
    source_result: SourceResult,
    platform_warning: Option<String>,
}

struct PreparedInstall {
    source_name: String,
    source_description: String,
    plan: InstallPlan,
    rel_target: String,
}

#[derive(Debug, Default)]
struct InstallSearchPrefetch {
    by_package: HashMap<String, InstallPrefetchEntry>,
}

impl InstallSearchPrefetch {
    fn get(&self, package: &str) -> Option<&InstallPrefetchEntry> {
        self.by_package.get(package)
    }
}

#[derive(Debug)]
enum InstallPrefetchEntry {
    AlreadyInstalled(PackageLocation),
    Search(PackageQueryReport),
}

struct InstallRoutingContext {
    base: String,
    enriched: Option<String>,
    candidates: Vec<String>,
}

impl InstallRoutingContext {
    fn build(ctx: &AppContext) -> Self {
        use crate::infra::file_edit::list_bracket_entries;

        let base = build_routing_context(&ctx.config_files);
        let candidates: Vec<String> = nix_manifest_candidates(&ctx.config_files)
            .iter()
            .filter_map(|path| {
                path.strip_prefix(&ctx.repo_root)
                    .ok()
                    .and_then(|rel| rel.to_str())
                    .map(String::from)
            })
            .collect();

        let content_lines: Vec<String> = candidates
            .iter()
            .filter_map(|rel| {
                let text = fs::read_to_string(ctx.repo_root.join(rel)).ok()?;
                let display = if let Some(entries) = list_bracket_entries(&text) {
                    let total = entries.len();
                    let sample: Vec<&str> = entries.iter().take(6).map(String::as_str).collect();
                    if total > sample.len() {
                        format!("{}, ... ({total} packages)", sample.join(", "))
                    } else if total > 0 {
                        format!("{} ({total} packages)", sample.join(", "))
                    } else {
                        "(empty)".to_string()
                    }
                } else {
                    "(unparsed list format)".to_string()
                };
                Some(format!("- {rel}: {display}"))
            })
            .collect();

        let enriched = (!content_lines.is_empty())
            .then(|| format!("{base}\n\nFile contents:\n{}", content_lines.join("\n")));

        Self {
            base,
            enriched,
            candidates,
        }
    }

    fn route_context(&self) -> &str {
        self.enriched.as_deref().unwrap_or(&self.base)
    }
}

#[derive(Debug)]
enum PlatformResolution {
    Primary(SourceResult),
    Fallback {
        candidate: SourceResult,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateSelection {
    Selected(usize),
    Skipped,
}

/// Search all sources for a package. Returns `None` with error printed if not found.
fn search_for_package(
    package: &str,
    args: &InstallArgs,
    ctx: &AppContext,
    cache: &mut Option<MultiSourceCache>,
    prefetched: Option<&PackageQueryReport>,
) -> Option<SearchResolution> {
    // Explicit --cask / --mas skip search (instant, no ambiguity)
    if args.cask() || args.mas() {
        let prefs = source_prefs_from_args(args);
        if let Some(prefetched) = prefetched {
            return resolve_search_candidates(
                package,
                &prefetched.outcome.results,
                args,
                &ctx.repo_root,
                ctx,
            );
        }

        let report = query_install_package(package, &prefs, &ctx.repo_root, cache, &ctx.printer);
        if args.verbose() {
            Printer::detail(&format!(
                "Query diagnostics: cache={}, elapsed={}ms, unavailable_backends={}",
                if report.cache_hit { "hit" } else { "miss" },
                report.elapsed.as_millis(),
                report.outcome.unavailable_sources.len()
            ));
        }
        return resolve_search_candidates(
            package,
            &report.outcome.results,
            args,
            &ctx.repo_root,
            ctx,
        );
    }

    let prefs = source_prefs_from_args(args);
    let live_lookup;
    let cached = if let Some(prefetched) = prefetched {
        prefetched
    } else {
        live_lookup = query_install_package(package, &prefs, &ctx.repo_root, cache, &ctx.printer);
        &live_lookup
    };
    let outcome = &cached.outcome;

    if args.verbose() || (args.explain() && cached.cache_hit) {
        Printer::detail(&format!(
            "Query diagnostics for '{package}': cache={}, elapsed={}ms, sources={}",
            if cached.cache_hit { "hit" } else { "miss" },
            cached.elapsed.as_millis(),
            outcome.results.len()
        ));
    }

    if outcome.results.is_empty() {
        show_unknown_group(package, ctx);
        if outcome.unavailable_sources.is_empty() {
            ctx.printer
                .error(&format!("{package}: not found in any source"));
        } else {
            ctx.printer
                .error(&format!("{package}: not found in any available source"));
            for source in &outcome.unavailable_sources {
                Printer::detail(&format!("- {}: {}", source.source, source.reason));
            }
        }
        return None;
    }

    resolve_search_candidates(package, &outcome.results, args, &ctx.repo_root, ctx)
}

fn resolve_search_candidates(
    package: &str,
    candidates: &[SourceResult],
    args: &InstallArgs,
    repo_root: &Path,
    ctx: &AppContext,
) -> Option<SearchResolution> {
    if candidates.is_empty() {
        return None;
    }

    let display_indices = unique_source_candidate_indices(candidates);
    let display_candidates: Vec<&SourceResult> = display_indices
        .iter()
        .map(|&idx| &candidates[idx])
        .collect();

    match find_existing_for_candidates(candidates, repo_root) {
        Ok(Some(location)) => {
            show_resolution_groups(package, &[], Some(&location), ctx);
            Some(SearchResolution::AlreadyInstalled(location))
        }
        Ok(None) => {
            show_resolution_groups(package, &display_candidates, None, ctx);
            if !args.yes() && !args.dry_run() && !display_candidates.is_empty() {
                println!();
            }

            match choose_candidate_selection(args, &display_candidates, ctx) {
                CandidateSelection::Selected(choice) => {
                    let selected_index = display_indices[choice];
                    resolve_platform_candidate(&candidates[selected_index], candidates, ctx)
                }
                CandidateSelection::Skipped => {
                    Printer::detail("Cancelled.");
                    Some(SearchResolution::Skipped)
                }
            }
        }
        Err(err) => {
            ctx.printer.error(&format!("install lookup failed: {err}"));
            None
        }
    }
}

fn choose_candidate_selection(
    args: &InstallArgs,
    candidates: &[&SourceResult],
    _ctx: &AppContext,
) -> CandidateSelection {
    select_candidate_index(
        args,
        candidates.len(),
        || {
            let candidate = &candidates[0];
            let attr = candidate.attr.as_deref().unwrap_or(&candidate.name);
            Printer::confirm(&format!("Install {attr} ({})?", candidate.source), true)
        },
        prompt_source_choice,
    )
}

fn select_candidate_index(
    args: &InstallArgs,
    candidate_count: usize,
    confirm_single: impl FnOnce() -> bool,
    prompt_choice: impl FnOnce(usize) -> Option<usize>,
) -> CandidateSelection {
    if candidate_count == 0 {
        return CandidateSelection::Skipped;
    }
    if args.yes() || args.dry_run() {
        return CandidateSelection::Selected(0);
    }
    if candidate_count == 1 {
        return if confirm_single() {
            CandidateSelection::Selected(0)
        } else {
            CandidateSelection::Skipped
        };
    }
    prompt_choice(candidate_count).map_or(CandidateSelection::Skipped, CandidateSelection::Selected)
}

fn resolve_platform_candidate(
    selected: &SourceResult,
    candidates: &[SourceResult],
    ctx: &AppContext,
) -> Option<SearchResolution> {
    match resolve_platform_candidate_with(selected, candidates, check_nix_available) {
        Ok(PlatformResolution::Primary(primary)) => Some(SearchResolution::Install {
            result: primary,
            platform_warning: None,
        }),
        Ok(PlatformResolution::Fallback { candidate, reason }) => {
            let fallback_desc = candidate
                .attr
                .as_deref()
                .unwrap_or(&candidate.name)
                .to_string();
            Some(SearchResolution::Install {
                result: candidate,
                platform_warning: Some(format!(
                    "{}: {reason}; trying {fallback_desc}",
                    selected.name
                )),
            })
        }
        Err(reason) => {
            ctx.printer.error(&format!("{}: {reason}", selected.name));
            None
        }
    }
}

fn resolve_platform_candidate_with<F>(
    selected: &SourceResult,
    candidates: &[SourceResult],
    mut check_available: F,
) -> Result<PlatformResolution, String>
where
    F: FnMut(&str) -> (bool, Option<String>),
{
    if !selected.source.requires_attr() {
        return Ok(PlatformResolution::Primary(selected.clone()));
    }

    let Some(primary_attr) = selected.attr.as_deref() else {
        return Ok(PlatformResolution::Primary(selected.clone()));
    };

    let (available, reason) = check_available(primary_attr);
    if available {
        return Ok(PlatformResolution::Primary(selected.clone()));
    }

    let reason = reason.unwrap_or_else(|| "not available on current platform".to_string());

    for candidate in candidates {
        if candidate.source != selected.source || candidate.attr == selected.attr {
            continue;
        }

        let Some(attr) = candidate.attr.as_deref() else {
            continue;
        };

        if check_available(attr).0 {
            return Ok(PlatformResolution::Fallback {
                candidate: candidate.clone(),
                reason,
            });
        }
    }

    Err(reason)
}

fn unique_source_candidate_indices(candidates: &[SourceResult]) -> Vec<usize> {
    let mut seen = HashSet::new();
    let mut indices = Vec::new();
    for (idx, candidate) in candidates.iter().enumerate() {
        if seen.insert(candidate.source) {
            indices.push(idx);
        }
    }
    indices
}

fn show_unknown_group(package: &str, _ctx: &AppContext) {
    println!();
    Printer::detail("unknown/not found:");
    Printer::detail(&format!("  - {package}"));
}

fn show_resolution_groups(
    package: &str,
    installable: &[&SourceResult],
    installed: Option<&PackageLocation>,
    ctx: &AppContext,
) {
    if !installable.is_empty() {
        println!();
        Printer::detail("Found (1)");

        if installable.len() == 1 {
            let candidate = installable[0];
            let source = format_source_display(candidate.source, candidate.attr.as_deref());
            let detail = if candidate.description.is_empty() {
                String::new()
            } else {
                format!(" - {}", truncate_text(&candidate.description, 50))
            };
            Printer::detail(&format!("{package} via {source}{detail}"));
        } else {
            Printer::detail(package);
            for (idx, candidate) in installable.iter().enumerate() {
                let source = format_source_display(candidate.source, candidate.attr.as_deref());
                Printer::detail(&format!("  {}. {source}", idx + 1));
                if let Some(version) = candidate.version.as_deref() {
                    Printer::detail(&format!("         Version:     {version}"));
                }
                if !candidate.description.is_empty() {
                    Printer::detail(&format!(
                        "         Description: {}",
                        truncate_text(&candidate.description, 60)
                    ));
                }
            }
        }
    }

    if let Some(location) = installed {
        Printer::detail("already installed:");
        Printer::detail(&format!(
            "  - {package} ({})",
            relative_location(location, &ctx.repo_root)
        ));
    }
}

fn format_source_display(source: PackageSource, attr: Option<&str>) -> String {
    match source {
        PackageSource::Nxs => {
            attr.map_or_else(|| "nxs".to_string(), |value| format!("nxs (pkgs.{value})"))
        }
        PackageSource::Nur => "NUR".to_string(),
        PackageSource::FlakeInput => "Flake overlay".to_string(),
        PackageSource::Homebrew => "Homebrew formula".to_string(),
        PackageSource::Cask => "Homebrew cask".to_string(),
        PackageSource::Mas => "Mac App Store".to_string(),
    }
}

fn build_simulated_preview_line(
    package_token: &str,
    description: &str,
    comment_col: Option<usize>,
) -> String {
    if description.is_empty() {
        return package_token.to_string();
    }
    let truncated = truncate_text(description, 40);
    if let Some(col) = comment_col
        && package_token.len() < col
    {
        let pad = col - package_token.len();
        return format!("{package_token}{:pad$}# {truncated}", "");
    }
    format!("{package_token}  # {truncated}")
}

fn prompt_source_choice(count: usize) -> Option<usize> {
    let nums = (1..=count)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("/");
    print!("  Install? [{nums}/n]: ");
    let _ = io::stdout().flush();

    let mut line = String::new();
    let read_result = io::stdin().lock().read_line(&mut line);
    match read_result {
        Ok(0) | Err(_) => Some(0),
        Ok(_) => parse_source_choice(&line, count),
    }
}

fn parse_source_choice(response: &str, count: usize) -> Option<usize> {
    let trimmed = response.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Some(0);
    }
    if trimmed == "n" || trimmed == "no" {
        return None;
    }

    trimmed.parse::<usize>().ok().and_then(|n| {
        if (1..=count).contains(&n) {
            Some(n - 1)
        } else {
            None
        }
    })
}

fn find_existing_for_candidates(
    candidates: &[SourceResult],
    repo_root: &Path,
) -> anyhow::Result<Option<PackageLocation>> {
    let mut names = Vec::new();

    for candidate in candidates {
        for name in lookup_names(candidate) {
            names.push(name);
        }
    }

    find_first_package(&names, repo_root)
}

fn prefetch_install_searches(
    args: &InstallArgs,
    ctx: &AppContext,
    cache: &mut Option<MultiSourceCache>,
) -> InstallSearchPrefetch {
    let mut by_package = HashMap::new();
    let packages =
        packages_needing_search_prefetch(&args.packages, &ctx.repo_root, &mut by_package);

    if packages.len() >= 2 {
        let prefs = source_prefs_from_args(args);
        let outcomes = ctx.printer.with_loading(
            &format!("Resolving sources for {} packages", packages.len()),
            |_| query_packages(&packages, &prefs, &ctx.repo_root, cache),
        );
        for (package, outcome) in outcomes {
            by_package.insert(package, InstallPrefetchEntry::Search(outcome));
        }
    }

    InstallSearchPrefetch { by_package }
}

fn query_install_package(
    package: &str,
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    printer: &Printer,
) -> PackageQueryReport {
    printer.with_loading(&format!("Resolving source for {package}"), |_| {
        query_package(package, prefs, repo_root, cache)
    })
}

fn packages_needing_search_prefetch(
    packages: &[String],
    repo_root: &Path,
    prefetched: &mut HashMap<String, InstallPrefetchEntry>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut pending = Vec::new();

    for package in packages {
        if !seen.insert(package.clone()) {
            continue;
        }

        match find_package(package, repo_root) {
            Ok(Some(location)) => {
                prefetched.insert(
                    package.clone(),
                    InstallPrefetchEntry::AlreadyInstalled(location),
                );
            }
            Ok(None) | Err(_) => pending.push(package.clone()),
        }
    }

    pending
}

fn lookup_names(candidate: &SourceResult) -> Vec<String> {
    let mut names = Vec::new();

    push_unique(&mut names, candidate.name.clone());

    if let Some(attr) = candidate.attr.as_deref() {
        push_unique(&mut names, attr.to_string());
        if let Some((bare, _runtime, _method)) = detect_language_package(attr) {
            push_unique(&mut names, bare.to_string());
        }
    }

    names
}

fn push_unique(items: &mut Vec<String>, value: String) {
    if !value.is_empty() && !items.contains(&value) {
        items.push(value);
    }
}

#[cfg(test)]
mod tests;
