use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

use crate::cli::{InstallArgs, PassthroughArgs};
use crate::commands::context::AppContext;
use crate::commands::shared::{
    SnippetMode, missing_argument_error, relative_location, show_dry_run_preview, show_snippet,
};
use crate::commands::system::cmd_rebuild;
use crate::domain::location::PackageLocation;
use crate::domain::plan::{
    InsertionMode, InstallPlan, build_install_plan, nix_manifest_candidates,
};
use crate::domain::source::{
    PackageSource, SourcePreferences, SourceResult, detect_language_package,
};
use crate::infra::ai_engine::{
    AiEngine, ClaudeEngine, CommandOutcome, build_edit_prompt, build_routing_context,
    run_edit_with_callback, select_engine,
};
use crate::infra::cache::MultiSourceCache;
use crate::infra::file_edit::{EditOutcome, apply_edit};
use crate::infra::finder::find_package;
use crate::infra::flake_input::{FlakeInputEdit, add_flake_input};
use crate::infra::shell::run_captured_command;
use crate::infra::sources::{check_nix_available, search_all_sources};

pub fn cmd_install(args: &InstallArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        return missing_argument_error("install", "PACKAGES...");
    }

    if args.dry_run {
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

    let engine = select_engine(args.engine.as_deref(), args.model.as_deref());
    let routing_context = build_routing_context(&ctx.config_files);
    let mut cache = load_cache(ctx);

    let mut success_count = 0;

    for package in &args.packages {
        if install_one(
            package,
            args,
            ctx,
            &mut cache,
            engine.as_ref(),
            &routing_context,
        ) {
            success_count += 1;
        }
    }

    run_post_install_actions(success_count, args, ctx, || {
        let passthrough = PassthroughArgs {
            passthrough: Vec::new(),
        };
        cmd_rebuild(&passthrough, ctx)
    });

    i32::from(success_count != args.packages.len())
}

fn run_post_install_actions<F>(
    success_count: usize,
    args: &InstallArgs,
    ctx: &AppContext,
    rebuild: F,
) where
    F: FnOnce() -> i32,
{
    if success_count == 0 || args.dry_run {
        return;
    }

    println!();
    ctx.printer.detail("Run: nx rebuild");

    if args.rebuild {
        let _ = rebuild();
    }
}

/// Install a single package. Returns `true` on success.
fn install_one(
    package: &str,
    args: &InstallArgs,
    ctx: &AppContext,
    cache: &mut Option<MultiSourceCache>,
    engine: &dyn AiEngine,
    routing_context: &str,
) -> bool {
    // Check if already installed
    match find_package(package, &ctx.repo_root) {
        Ok(Some(location)) => {
            report_already_installed(package, &location, ctx);
            return true;
        }
        Ok(None) => {} // not installed — proceed
        Err(err) => {
            ctx.printer.error(&format!("install lookup failed: {err}"));
            return false;
        }
    }

    let Some(resolution) = search_for_package(package, args, ctx, cache) else {
        return false;
    };
    let (sr, platform_warning) = match resolution {
        SearchResolution::Install {
            result,
            platform_warning,
        } => (result, platform_warning),
        SearchResolution::AlreadyInstalled(location) => {
            report_already_installed(package, &location, ctx);
            return true;
        }
        SearchResolution::Skipped => return true,
    };

    println!();
    if args.dry_run {
        ctx.printer.detail("Analyzing (1)");
    } else {
        ctx.printer.detail("Installing (1)");
    }

    if let Some(warning) = platform_warning {
        ctx.printer.warn(&warning);
    }

    let mut plan = match build_install_plan(&sr, &ctx.config_files) {
        Ok(p) => p,
        Err(err) => {
            ctx.printer.error(&format!("{package}: {err}"));
            return false;
        }
    };

    refine_routing(&mut plan, engine, routing_context, ctx);

    if !gate_flake_input(package, &plan, args, ctx, engine) {
        return false;
    }

    println!("> Routing {}", sr.name);

    if let Some(ref warning) = plan.routing_warning {
        ctx.printer.warn(warning);
    }

    let rel_target = plan
        .target_file
        .strip_prefix(&ctx.repo_root)
        .unwrap_or(&plan.target_file)
        .display()
        .to_string();

    if args.dry_run {
        if plan.insertion_mode == InsertionMode::NixManifest
            && let Some(insert_after_line) = find_preview_insert_after_line(&plan.target_file)
        {
            let simulated_line = build_simulated_preview_line(&plan.package_token, &sr.description);
            show_dry_run_preview(&plan.target_file, insert_after_line, &simulated_line, 1);
        }

        println!();
        if let Some(language_info) = &plan.language_info {
            ctx.printer.success(&format!(
                "Would add '{}' to {}.withPackages in {rel_target}",
                language_info.bare_name, language_info.runtime
            ));
        } else {
            ctx.printer
                .success(&format!("Would add {} to {rel_target}", plan.package_token));
        }
        maybe_setup_service(&sr.name, args, ctx);
        return true;
    }

    let installed = execute_edit(&plan, &rel_target, ctx, engine);
    if installed {
        maybe_setup_service(&sr.name, args, ctx);
    }
    installed
}

/// Refine routing for general nix packages via AI engine.
fn refine_routing(
    plan: &mut InstallPlan,
    engine: &dyn AiEngine,
    routing_context: &str,
    ctx: &AppContext,
) {
    if plan.routing_warning.is_none() || plan.insertion_mode != InsertionMode::NixManifest {
        return;
    }

    let candidates: Vec<String> = nix_manifest_candidates(&ctx.config_files)
        .iter()
        .filter_map(|p| {
            p.strip_prefix(&ctx.repo_root)
                .ok()
                .and_then(|r| r.to_str())
                .map(String::from)
        })
        .collect();

    let fallback = plan
        .target_file
        .strip_prefix(&ctx.repo_root)
        .unwrap_or(&plan.target_file)
        .to_string_lossy()
        .to_string();

    let decision = engine.route_package(
        &plan.package_token,
        routing_context,
        &candidates,
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
    ctx.printer.detail(&format!("URL: {flake_url}"));

    if args.dry_run {
        ctx.printer
            .detail(&format!("[DRY RUN] Would add flake input for {package}"));
        return true; // counted as success in dry-run
    }
    if !args.yes && !ctx.printer.confirm("Add flake input?", true) {
        ctx.printer.warn(&format!("Skipping {package}"));
        return false;
    }

    let flake_path = ctx.repo_root.join("flake.nix");
    match add_flake_input(&flake_path, flake_url, None) {
        Ok(FlakeInputEdit::Added { input_name }) => {
            ctx.printer.detail(&format!("added input '{input_name}'"));
            true
        }
        Ok(FlakeInputEdit::AlreadyExists { input_name }) => {
            ctx.printer
                .detail(&format!("input '{input_name}' already exists"));
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
        let service_engine = ClaudeEngine::new(args.model.as_deref());
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
    if !args.service {
        return;
    }

    if args.dry_run {
        ctx.printer.detail(&format!(
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

fn git_diff(cwd: &Path) -> String {
    run_captured_command("git", &["diff"], Some(cwd))
        .map(|cmd| cmd.stdout)
        .unwrap_or_default()
}

/// Map CLI flags to source preferences for search.
fn source_prefs_from_args(args: &InstallArgs) -> SourcePreferences {
    SourcePreferences {
        bleeding_edge: args.bleeding_edge,
        nur: args.nur,
        force_source: args.source.clone(),
        is_cask: args.cask,
        is_mas: args.mas,
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
) -> Option<SearchResolution> {
    // Explicit --cask / --mas skip search (instant, no ambiguity)
    if args.cask || args.mas {
        let prefs = source_prefs_from_args(args);
        let results = search_all_sources(package, &prefs, None);
        return resolve_search_candidates(package, &results, args, &ctx.repo_root, ctx);
    }

    if let Some(cache) = cache.as_mut() {
        let cached = cache.get_all(package);
        if !cached.is_empty() {
            if args.explain {
                ctx.printer.detail(&format!(
                    "Cache hit for '{package}' ({} sources)",
                    cached.len()
                ));
            }
            return resolve_search_candidates(package, &cached, args, &ctx.repo_root, ctx);
        }
    }

    let prefs = source_prefs_from_args(args);
    let flake_lock = ctx.repo_root.join("flake.lock");
    let flake_lock_path = flake_lock.exists().then_some(flake_lock.as_path());

    let results = search_all_sources(package, &prefs, flake_lock_path);

    if results.is_empty() {
        show_unknown_group(package, ctx);
        ctx.printer
            .error(&format!("{package}: not found in any source"));
        return None;
    }

    if let Some(cache) = cache.as_mut()
        && let Err(err) = cache.set_many(&results)
    {
        ctx.printer.warn(&format!(
            "failed to update search cache for {package}: {err}"
        ));
    }

    resolve_search_candidates(package, &results, args, &ctx.repo_root, ctx)
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
            if !args.yes && !args.dry_run && !display_candidates.is_empty() {
                println!();
            }

            match choose_candidate_selection(args, &display_candidates, ctx) {
                CandidateSelection::Selected(choice) => {
                    let selected_index = display_indices[choice];
                    resolve_platform_candidate(&candidates[selected_index], candidates, ctx)
                }
                CandidateSelection::Skipped => {
                    ctx.printer.detail("Cancelled.");
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
    ctx: &AppContext,
) -> CandidateSelection {
    select_candidate_index(
        args,
        candidates.len(),
        || {
            let candidate = &candidates[0];
            let attr = candidate.attr.as_deref().unwrap_or(&candidate.name);
            ctx.printer
                .confirm(&format!("Install {attr} ({})?", candidate.source), true)
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
    if args.yes || args.dry_run {
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

fn show_unknown_group(package: &str, ctx: &AppContext) {
    println!();
    ctx.printer.detail("unknown/not found:");
    ctx.printer.detail(&format!("  - {package}"));
}

fn show_resolution_groups(
    package: &str,
    installable: &[&SourceResult],
    installed: Option<&PackageLocation>,
    ctx: &AppContext,
) {
    if !installable.is_empty() {
        println!();
        ctx.printer.detail("Found (1)");

        if installable.len() == 1 {
            let candidate = installable[0];
            let source = format_source_display(candidate.source, candidate.attr.as_deref());
            let detail = if candidate.description.is_empty() {
                String::new()
            } else {
                format!(" - {}", truncate_text(&candidate.description, 50))
            };
            ctx.printer
                .detail(&format!("{package} via {source}{detail}"));
        } else {
            ctx.printer.detail(package);
            for (idx, candidate) in installable.iter().enumerate() {
                let source = format_source_display(candidate.source, candidate.attr.as_deref());
                ctx.printer.detail(&format!("  {}. {source}", idx + 1));
                if let Some(version) = candidate.version.as_deref() {
                    ctx.printer
                        .detail(&format!("         Version:     {version}"));
                }
                if !candidate.description.is_empty() {
                    ctx.printer.detail(&format!(
                        "         Description: {}",
                        truncate_text(&candidate.description, 60)
                    ));
                }
            }
        }
    }

    if let Some(location) = installed {
        ctx.printer.detail("already installed:");
        ctx.printer.detail(&format!(
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

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let take = max_chars.saturating_sub(3);
    format!("{}...", text.chars().take(take).collect::<String>())
}

fn build_simulated_preview_line(package_token: &str, description: &str) -> String {
    if description.is_empty() {
        return package_token.to_string();
    }
    let truncated = description.chars().take(40).collect::<String>();
    format!("{package_token}  # {truncated}...")
}

fn find_preview_insert_after_line(file_path: &Path) -> Option<usize> {
    let content = fs::read_to_string(file_path).ok()?;
    let mut insert_after = None;
    for (idx, line) in content.lines().enumerate() {
        if is_preview_manifest_entry(line) {
            insert_after = Some(idx + 1);
        }
    }
    insert_after
}

fn is_preview_manifest_entry(line: &str) -> bool {
    if !line.starts_with("    ") {
        return false;
    }
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with('[')
        || trimmed.starts_with(']')
        || trimmed.starts_with('{')
        || trimmed.starts_with('}')
    {
        return false;
    }

    let token = trimmed.split_whitespace().next().unwrap_or_default();
    !token.is_empty()
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
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
    for candidate in candidates {
        if let Some(existing) = find_existing_for_result(candidate, repo_root)? {
            return Ok(Some(existing));
        }
    }
    Ok(None)
}

fn find_existing_for_result(
    candidate: &SourceResult,
    repo_root: &Path,
) -> anyhow::Result<Option<PackageLocation>> {
    for name in lookup_names(candidate) {
        if let Some(location) = find_package(&name, repo_root)? {
            return Ok(Some(location));
        }
    }
    Ok(None)
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
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::domain::config::ConfigFiles;
    use crate::domain::source::PackageSource;
    use crate::infra::ai_engine::RouteDecision;
    use crate::output::printer::Printer;
    use crate::output::style::OutputStyle;
    use tempfile::TempDir;

    fn source_result(name: &str, source: PackageSource, attr: Option<&str>) -> SourceResult {
        SourceResult {
            name: name.to_string(),
            source,
            attr: attr.map(str::to_string),
            version: None,
            confidence: 1.0,
            description: String::new(),
            requires_flake_mod: false,
            flake_url: None,
        }
    }

    fn write_nix(root: &Path, rel_path: &str, content: &str) {
        let full = root.join(rel_path);
        fs::create_dir_all(full.parent().expect("nix file should have parent dirs"))
            .expect("nix parent dirs should be created");
        fs::write(full, content).expect("nix content should be written");
    }

    fn test_context(root: &Path) -> AppContext {
        AppContext::new(
            root.to_path_buf(),
            Printer::new(OutputStyle::from_flags(true, false, false)),
            ConfigFiles::discover(root),
        )
    }

    fn test_plan(root: &Path, token: &str) -> InstallPlan {
        InstallPlan {
            source_result: SourceResult::new(token, PackageSource::Nxs),
            package_token: token.to_string(),
            target_file: root.join("packages/nix/cli.nix"),
            insertion_mode: InsertionMode::NixManifest,
            language_info: None,
            routing_warning: None,
        }
    }

    struct StubEngine {
        engine_name: &'static str,
        supports_flake: bool,
        run_edit_calls: Arc<AtomicUsize>,
        run_edit_outcome: CommandOutcome,
    }

    impl AiEngine for StubEngine {
        fn route_package(
            &self,
            _package: &str,
            _context: &str,
            _candidates: &[String],
            fallback: &str,
            _cwd: &Path,
        ) -> RouteDecision {
            RouteDecision {
                target_file: fallback.to_string(),
                warning: None,
            }
        }

        fn run_edit(&self, _prompt: &str, _cwd: &Path) -> CommandOutcome {
            self.run_edit_calls.fetch_add(1, Ordering::SeqCst);
            self.run_edit_outcome.clone()
        }

        fn supports_flake_input(&self) -> bool {
            self.supports_flake
        }

        fn name(&self) -> &'static str {
            self.engine_name
        }
    }

    #[test]
    fn source_prefs_defaults_match_no_flags() {
        let args = InstallArgs {
            packages: vec![],
            yes: false,
            dry_run: false,
            cask: false,
            mas: false,
            service: false,
            rebuild: false,
            bleeding_edge: false,
            nur: false,
            source: None,
            explain: false,
            engine: None,
            model: None,
        };
        let prefs = source_prefs_from_args(&args);
        assert!(!prefs.bleeding_edge);
        assert!(!prefs.nur);
        assert!(!prefs.is_cask);
        assert!(!prefs.is_mas);
        assert!(prefs.force_source.is_none());
    }

    #[test]
    fn source_prefs_maps_cask_flag() {
        let args = InstallArgs {
            packages: vec![],
            yes: false,
            dry_run: false,
            cask: true,
            mas: false,
            service: false,
            rebuild: false,
            bleeding_edge: false,
            nur: false,
            source: None,
            explain: false,
            engine: None,
            model: None,
        };
        let prefs = source_prefs_from_args(&args);
        assert!(prefs.is_cask);
    }

    #[test]
    fn source_prefs_maps_source_and_bleeding_edge() {
        let args = InstallArgs {
            packages: vec![],
            yes: false,
            dry_run: false,
            cask: false,
            mas: false,
            service: false,
            rebuild: false,
            bleeding_edge: true,
            nur: true,
            source: Some("unstable".to_string()),
            explain: false,
            engine: None,
            model: None,
        };
        let prefs = source_prefs_from_args(&args);
        assert!(prefs.bleeding_edge);
        assert!(prefs.nur);
        assert_eq!(prefs.force_source.as_deref(), Some("unstable"));
    }

    #[test]
    fn lookup_names_includes_attr_and_language_bare_name() {
        let result = source_result(
            "py-yaml",
            PackageSource::Nxs,
            Some("python3Packages.pyyaml"),
        );

        let names = lookup_names(&result);
        assert_eq!(
            names,
            vec![
                "py-yaml".to_string(),
                "python3Packages.pyyaml".to_string(),
                "pyyaml".to_string()
            ]
        );
    }

    #[test]
    fn find_existing_for_candidates_checks_alternates() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            r"{ pkgs }:
[
  ripgrep
]
",
        );

        let candidates = vec![
            source_result("rg", PackageSource::Nxs, Some("fd")),
            source_result("rg", PackageSource::Nxs, Some("ripgrep")),
        ];

        let location = find_existing_for_candidates(&candidates, root)
            .expect("finder should not error")
            .expect("alternate candidate should resolve as installed");
        assert!(
            location.path().ends_with(Path::new("packages/nix/cli.nix")),
            "expected installed location to resolve to packages/nix/cli.nix, got {}",
            location.path().display()
        );
    }

    #[test]
    fn parse_source_choice_empty_defaults_to_first() {
        assert_eq!(parse_source_choice("", 3), Some(0));
        assert_eq!(parse_source_choice("   ", 3), Some(0));
    }

    #[test]
    fn parse_source_choice_accepts_valid_number() {
        assert_eq!(parse_source_choice("2", 3), Some(1));
    }

    #[test]
    fn parse_source_choice_rejects_cancel_and_invalid() {
        assert_eq!(parse_source_choice("n", 3), None);
        assert_eq!(parse_source_choice("no", 3), None);
        assert_eq!(parse_source_choice("0", 3), None);
        assert_eq!(parse_source_choice("9", 3), None);
        assert_eq!(parse_source_choice("abc", 3), None);
    }

    #[test]
    fn select_candidate_index_yes_bypasses_prompts() {
        let mut args = install_args_template();
        args.yes = true;

        let mut confirm_calls = 0usize;
        let mut prompt_calls = 0usize;

        let selection = select_candidate_index(
            &args,
            3,
            || {
                confirm_calls += 1;
                true
            },
            |_| {
                prompt_calls += 1;
                Some(2)
            },
        );

        assert_eq!(selection, CandidateSelection::Selected(0));
        assert_eq!(confirm_calls, 0);
        assert_eq!(prompt_calls, 0);
    }

    #[test]
    fn select_candidate_index_dry_run_bypasses_prompts() {
        let mut args = install_args_template();
        args.dry_run = true;

        let mut confirm_calls = 0usize;
        let mut prompt_calls = 0usize;

        let selection = select_candidate_index(
            &args,
            2,
            || {
                confirm_calls += 1;
                true
            },
            |_| {
                prompt_calls += 1;
                Some(1)
            },
        );

        assert_eq!(selection, CandidateSelection::Selected(0));
        assert_eq!(confirm_calls, 0);
        assert_eq!(prompt_calls, 0);
    }

    #[test]
    fn select_candidate_index_single_requires_confirmation() {
        let args = install_args_template();

        let mut confirm_calls = 0usize;
        let mut prompt_calls = 0usize;
        let declined = select_candidate_index(
            &args,
            1,
            || {
                confirm_calls += 1;
                false
            },
            |_| {
                prompt_calls += 1;
                Some(0)
            },
        );

        assert_eq!(declined, CandidateSelection::Skipped);
        assert_eq!(confirm_calls, 1);
        assert_eq!(prompt_calls, 0);
    }

    #[test]
    fn select_candidate_index_multi_uses_numbered_prompt() {
        let args = install_args_template();

        let mut confirm_calls = 0usize;
        let mut prompt_calls = 0usize;
        let selected = select_candidate_index(
            &args,
            3,
            || {
                confirm_calls += 1;
                true
            },
            |_| {
                prompt_calls += 1;
                Some(2)
            },
        );

        let skipped = select_candidate_index(
            &args,
            3,
            || {
                confirm_calls += 1;
                true
            },
            |_| {
                prompt_calls += 1;
                None
            },
        );

        assert_eq!(selected, CandidateSelection::Selected(2));
        assert_eq!(skipped, CandidateSelection::Skipped);
        assert_eq!(confirm_calls, 0);
        assert_eq!(prompt_calls, 2);
    }

    fn install_args_template() -> InstallArgs {
        InstallArgs {
            packages: vec!["ripgrep".to_string()],
            yes: false,
            dry_run: false,
            cask: false,
            mas: false,
            service: false,
            rebuild: false,
            bleeding_edge: false,
            nur: false,
            source: None,
            explain: false,
            engine: None,
            model: None,
        }
    }

    #[test]
    fn post_install_runs_rebuild_when_requested() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let ctx = test_context(tmp.path());
        let mut args = install_args_template();
        args.rebuild = true;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        run_post_install_actions(1, &args, &ctx, move || {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            0
        });

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn post_install_skips_rebuild_without_flag() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let ctx = test_context(tmp.path());
        let args = install_args_template();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        run_post_install_actions(1, &args, &ctx, move || {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            0
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn post_install_skips_rebuild_in_dry_run() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let ctx = test_context(tmp.path());
        let mut args = install_args_template();
        args.rebuild = true;
        args.dry_run = true;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        run_post_install_actions(1, &args, &ctx, move || {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            0
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn post_install_skips_rebuild_when_nothing_installed() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let ctx = test_context(tmp.path());
        let mut args = install_args_template();
        args.rebuild = true;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        run_post_install_actions(0, &args, &ctx, move || {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            0
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn service_setup_skips_when_flag_disabled() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(root, "home/services.nix", "# nx: services\n{}\n");
        let ctx = test_context(root);
        let args = install_args_template();

        let calls = Arc::new(AtomicUsize::new(0));
        maybe_setup_service_with("ripgrep", &args, &ctx, |_prompt| {
            calls.fetch_add(1, Ordering::SeqCst);
            CommandOutcome {
                success: true,
                output: String::new(),
            }
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn service_setup_dry_run_reports_without_edit_call() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(root, "home/services.nix", "# nx: services\n{}\n");
        let ctx = test_context(root);
        let mut args = install_args_template();
        args.service = true;
        args.dry_run = true;

        let calls = Arc::new(AtomicUsize::new(0));
        maybe_setup_service_with("ripgrep", &args, &ctx, |_prompt| {
            calls.fetch_add(1, Ordering::SeqCst);
            CommandOutcome {
                success: true,
                output: String::new(),
            }
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn service_setup_calls_editor_with_services_target() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(root, "home/services.nix", "# nx: services\n{}\n");
        let ctx = test_context(root);
        let mut args = install_args_template();
        args.service = true;

        let calls = Arc::new(AtomicUsize::new(0));
        let prompt = Arc::new(Mutex::new(String::new()));
        maybe_setup_service_with("ripgrep", &args, &ctx, |edit_prompt| {
            calls.fetch_add(1, Ordering::SeqCst);
            *prompt.lock().expect("prompt lock should succeed") = edit_prompt.to_string();
            CommandOutcome {
                success: true,
                output: String::new(),
            }
        });

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let captured = prompt.lock().expect("prompt lock should succeed");
        assert!(captured.contains("launchd agent for ripgrep"));
        assert!(captured.contains("home/services.nix"));
    }

    #[test]
    fn gate_flake_input_refuses_codex_engine() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
        );
        let ctx = test_context(root);
        let mut args = install_args_template();
        args.yes = true;

        let mut plan = test_plan(root, "ripgrep");
        plan.source_result.requires_flake_mod = true;

        let engine = StubEngine {
            engine_name: "codex",
            supports_flake: false,
            run_edit_calls: Arc::new(AtomicUsize::new(0)),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: String::new(),
            },
        };

        assert!(!gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
    }

    #[test]
    fn gate_flake_input_allows_claude_with_yes() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
        );
        write_nix(
            root,
            "flake.nix",
            "{\n  inputs = {\n    nixpkgs.url = \"github:NixOS/nixpkgs\";\n  };\n}\n",
        );
        let ctx = test_context(root);
        let mut args = install_args_template();
        args.yes = true;

        let mut plan = test_plan(root, "ripgrep");
        plan.source_result.requires_flake_mod = true;
        plan.source_result.flake_url = Some("github:nix-community/NUR".to_string());

        let engine = StubEngine {
            engine_name: "claude",
            supports_flake: true,
            run_edit_calls: Arc::new(AtomicUsize::new(0)),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: String::new(),
            },
        };

        assert!(gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));

        let flake_content =
            fs::read_to_string(root.join("flake.nix")).expect("flake should be readable");
        assert!(flake_content.contains("nur.url = \"github:nix-community/NUR\";"));
    }

    #[test]
    fn gate_flake_input_dry_run_reports_intent_and_allows() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
        );
        let ctx = test_context(root);
        let mut args = install_args_template();
        args.dry_run = true;

        let mut plan = test_plan(root, "ripgrep");
        plan.source_result.requires_flake_mod = true;
        plan.source_result.flake_url = Some("github:nix-community/NUR".to_string());

        let engine = StubEngine {
            engine_name: "claude",
            supports_flake: true,
            run_edit_calls: Arc::new(AtomicUsize::new(0)),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: String::new(),
            },
        };

        assert!(gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
    }

    #[test]
    fn gate_flake_input_errors_when_url_missing() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
        );
        let ctx = test_context(root);
        let mut args = install_args_template();
        args.yes = true;

        let mut plan = test_plan(root, "ripgrep");
        plan.source_result.requires_flake_mod = true;
        plan.source_result.flake_url = None;

        let engine = StubEngine {
            engine_name: "claude",
            supports_flake: true,
            run_edit_calls: Arc::new(AtomicUsize::new(0)),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: String::new(),
            },
        };

        assert!(!gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
    }

    #[test]
    fn execute_edit_codex_uses_deterministic_path() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = StubEngine {
            engine_name: "codex",
            supports_flake: false,
            run_edit_calls: calls.clone(),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: "unused".to_string(),
            },
        };

        let ctx = test_context(root);
        let plan = test_plan(root, "ripgrep");

        assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let edited = fs::read_to_string(root.join("packages/nix/cli.nix"))
            .expect("edited file should be readable");
        assert!(edited.contains("ripgrep"));
    }

    #[test]
    fn execute_edit_claude_uses_deterministic_path() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = StubEngine {
            engine_name: "claude",
            supports_flake: true,
            run_edit_calls: calls.clone(),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: "ok".to_string(),
            },
        };

        let ctx = test_context(root);
        let plan = test_plan(root, "ripgrep");

        assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let edited = fs::read_to_string(root.join("packages/nix/cli.nix"))
            .expect("target file should be readable");
        assert!(edited.contains("ripgrep"));
    }

    #[test]
    fn execute_edit_claude_is_idempotent_without_ai_fallback() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n    ripgrep\n  ];\n}\n",
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = StubEngine {
            engine_name: "claude",
            supports_flake: true,
            run_edit_calls: calls.clone(),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: "ok".to_string(),
            },
        };

        let ctx = test_context(root);
        let plan = test_plan(root, "ripgrep");

        assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn execute_edit_claude_falls_back_to_ai_when_deterministic_unsupported() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  services = { };\n}\n",
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = StubEngine {
            engine_name: "claude",
            supports_flake: true,
            run_edit_calls: calls.clone(),
            run_edit_outcome: CommandOutcome {
                success: true,
                output: "ok".to_string(),
            },
        };

        let ctx = test_context(root);
        let plan = test_plan(root, "ripgrep");

        assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn execute_edit_claude_fallback_failure_returns_false() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_nix(
            root,
            "packages/nix/cli.nix",
            "{ pkgs, ... }:\n{\n  services = { };\n}\n",
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let engine = StubEngine {
            engine_name: "claude",
            supports_flake: true,
            run_edit_calls: calls.clone(),
            run_edit_outcome: CommandOutcome {
                success: false,
                output: "boom".to_string(),
            },
        };

        let ctx = test_context(root);
        let plan = test_plan(root, "ripgrep");

        assert!(!execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn platform_resolution_uses_primary_when_available() {
        let primary = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"));
        let candidates = vec![primary.clone()];
        let mut checks = 0usize;

        let outcome = resolve_platform_candidate_with(&primary, &candidates, |_attr| {
            checks += 1;
            (true, None)
        })
        .expect("platform resolution should succeed");

        match outcome {
            PlatformResolution::Primary(sr) => {
                assert_eq!(sr.attr.as_deref(), Some("ripgrep"));
            }
            PlatformResolution::Fallback { .. } => panic!("expected primary candidate"),
        }
        assert_eq!(checks, 1);
    }

    #[test]
    fn platform_resolution_uses_same_source_fallback_when_primary_unavailable() {
        let primary = source_result(
            "py-yaml",
            PackageSource::Nxs,
            Some("python3Packages.aspy-yaml"),
        );
        let fallback = source_result(
            "py-yaml",
            PackageSource::Nxs,
            Some("python3Packages.pyyaml"),
        );
        let homebrew = source_result("py-yaml", PackageSource::Homebrew, Some("pyyaml"));
        let candidates = vec![primary.clone(), homebrew, fallback];

        let outcome = resolve_platform_candidate_with(&primary, &candidates, |attr| {
            if attr == "python3Packages.aspy-yaml" {
                return (
                    false,
                    Some("not available on aarch64-darwin (only: x86_64-linux)".to_string()),
                );
            }
            (true, None)
        })
        .expect("fallback should resolve");

        match outcome {
            PlatformResolution::Fallback { candidate, reason } => {
                assert_eq!(candidate.attr.as_deref(), Some("python3Packages.pyyaml"));
                assert!(reason.contains("not available on aarch64-darwin"));
            }
            PlatformResolution::Primary(_) => panic!("expected fallback candidate"),
        }
    }

    #[test]
    fn platform_resolution_errors_without_same_source_fallback() {
        let primary = source_result("roc", PackageSource::Nxs, Some("roc"));
        let other_source = source_result("roc", PackageSource::Homebrew, Some("roc"));
        let candidates = vec![primary.clone(), other_source];

        let outcome = resolve_platform_candidate_with(&primary, &candidates, |attr| {
            if attr == "roc" {
                return (
                    false,
                    Some("not available on aarch64-darwin (only: x86_64-linux)".to_string()),
                );
            }
            (true, None)
        });

        let err = outcome.expect_err("resolution should fail");
        assert!(err.contains("not available on aarch64-darwin"));
    }

    #[test]
    fn platform_resolution_skips_unavailable_same_source_and_uses_later_fallback() {
        let primary = source_result(
            "py-yaml",
            PackageSource::Nxs,
            Some("python3Packages.aspy-yaml"),
        );
        let unavailable_fallback = source_result(
            "py-yaml",
            PackageSource::Nxs,
            Some("python3Packages.bad-alt"),
        );
        let available_fallback = source_result(
            "py-yaml",
            PackageSource::Nxs,
            Some("python3Packages.pyyaml"),
        );
        let candidates = vec![
            primary.clone(),
            unavailable_fallback,
            available_fallback.clone(),
        ];

        let outcome = resolve_platform_candidate_with(&primary, &candidates, |attr| match attr {
            "python3Packages.aspy-yaml" | "python3Packages.bad-alt" => (
                false,
                Some("not available on aarch64-darwin (only: x86_64-linux)".to_string()),
            ),
            _ => (true, None),
        })
        .expect("later fallback should resolve");

        match outcome {
            PlatformResolution::Fallback { candidate, reason } => {
                assert_eq!(candidate.attr, available_fallback.attr);
                assert!(reason.contains("not available on aarch64-darwin"));
            }
            PlatformResolution::Primary(_) => panic!("expected fallback candidate"),
        }
    }
}
