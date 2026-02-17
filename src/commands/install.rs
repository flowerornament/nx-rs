use std::io::{self, BufRead, Write};
use std::path::Path;

use crate::cli::InstallArgs;
use crate::commands::context::AppContext;
use crate::commands::shared::{SnippetMode, relative_location, show_snippet};
use crate::domain::location::PackageLocation;
use crate::domain::plan::{
    InsertionMode, InstallPlan, build_install_plan, nix_manifest_candidates,
};
use crate::domain::source::{SourcePreferences, SourceResult, detect_language_package};
use crate::infra::ai_engine::{
    AiEngine, CommandOutcome, build_edit_prompt, build_routing_context, run_edit_with_callback,
    select_engine,
};
use crate::infra::cache::MultiSourceCache;
use crate::infra::file_edit::{EditOutcome, apply_edit};
use crate::infra::finder::find_package;
use crate::infra::shell::run_captured_command;
use crate::infra::sources::{check_nix_available, search_all_sources};

pub fn cmd_install(args: &InstallArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        ctx.printer.error("No packages specified");
        return 1;
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

    if success_count > 0 && !args.dry_run {
        println!();
        ctx.printer.detail("Run: nx rebuild");
    }

    i32::from(success_count != args.packages.len())
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
    let sr = match resolution {
        SearchResolution::Install(sr) => sr,
        SearchResolution::AlreadyInstalled(location) => {
            report_already_installed(package, &location, ctx);
            return true;
        }
        SearchResolution::Skipped => return true,
    };

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
        ctx.printer.detail(&format!(
            "[DRY RUN] Would add '{}' to {rel_target}",
            plan.package_token
        ));
        return true;
    }

    execute_edit(&plan, &rel_target, ctx, engine)
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
            "{package} requires flake.nix modification \u{2014} use --engine=claude"
        ));
        return false;
    }
    if args.dry_run {
        ctx.printer
            .detail(&format!("[DRY RUN] Would add flake input for {package}"));
        return true; // counted as success in dry-run
    }
    if !args.yes && !ctx.printer.confirm("Add flake input?", false) {
        ctx.printer.detail("Skipped");
        return false;
    }
    true
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

    let execution = run_edit_with_callback(engine, &prompt, &ctx.repo_root, || {
        if engine.name() != "codex" {
            return None;
        }

        deterministic = Some(apply_edit(plan));
        deterministic.as_ref().map(|result| match result {
            Ok(_) => CommandOutcome {
                success: true,
                output: "deterministic edit applied".to_string(),
            },
            Err(err) => CommandOutcome {
                success: false,
                output: err.to_string(),
            },
        })
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

fn report_deterministic_edit(
    result: anyhow::Result<EditOutcome>,
    plan: &InstallPlan,
    rel_target: &str,
    ctx: &AppContext,
) -> bool {
    match result {
        Ok(outcome) => {
            if outcome.file_changed {
                println!();
                ctx.printer
                    .success(&format!("Added '{}' to {rel_target}", plan.package_token));
                if let Some(line) = outcome.line_number {
                    show_snippet(&plan.target_file, line, 2, SnippetMode::Add, false);
                }
            } else {
                println!();
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
    Install(SourceResult),
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

    ctx.printer.searching(package);
    let results = search_all_sources(package, &prefs, flake_lock_path);
    ctx.printer.searching_done();

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

    match find_existing_for_candidates(candidates, repo_root) {
        Ok(Some(location)) => {
            show_resolution_groups(package, &[], Some(&location), ctx);
            Some(SearchResolution::AlreadyInstalled(location))
        }
        Ok(None) => {
            show_resolution_groups(package, candidates, None, ctx);

            if args.yes || args.dry_run || candidates.len() == 1 {
                return candidates
                    .first()
                    .and_then(|selected| resolve_platform_candidate(selected, candidates, ctx));
            }

            if let Some(choice) = prompt_source_choice(candidates.len()) {
                resolve_platform_candidate(&candidates[choice], candidates, ctx)
            } else {
                ctx.printer.detail("Cancelled.");
                Some(SearchResolution::Skipped)
            }
        }
        Err(err) => {
            ctx.printer.error(&format!("install lookup failed: {err}"));
            None
        }
    }
}

fn resolve_platform_candidate(
    selected: &SourceResult,
    candidates: &[SourceResult],
    ctx: &AppContext,
) -> Option<SearchResolution> {
    match resolve_platform_candidate_with(selected, candidates, check_nix_available) {
        Ok(PlatformResolution::Primary(primary)) => Some(SearchResolution::Install(primary)),
        Ok(PlatformResolution::Fallback { candidate, reason }) => {
            let fallback_desc = candidate.attr.as_deref().unwrap_or(&candidate.name);
            ctx.printer.warn(&format!(
                "{}: {reason}; trying {fallback_desc}",
                selected.name
            ));
            Some(SearchResolution::Install(candidate))
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

fn show_unknown_group(package: &str, ctx: &AppContext) {
    println!();
    ctx.printer.action(&format!("Results for '{package}'"));
    ctx.printer.detail("unknown/not found:");
    ctx.printer.detail(&format!("  - {package}"));
}

fn show_resolution_groups(
    package: &str,
    installable: &[SourceResult],
    installed: Option<&PackageLocation>,
    ctx: &AppContext,
) {
    println!();
    ctx.printer.action(&format!("Results for '{package}'"));

    if !installable.is_empty() {
        ctx.printer.detail("installable:");
        for (idx, candidate) in installable.iter().enumerate() {
            let attr = candidate.attr.as_deref().unwrap_or(&candidate.name);
            let detail = if candidate.description.is_empty() {
                String::new()
            } else {
                format!(" - {}", candidate.description)
            };
            ctx.printer.detail(&format!(
                "  {}. {} ({}){}",
                idx + 1,
                attr,
                candidate.source,
                detail
            ));
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

fn prompt_source_choice(count: usize) -> Option<usize> {
    let nums = (1..=count)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("/");
    print!("  Install? [{nums}/n]: ");
    let _ = io::stdout().flush();

    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
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
        let ctx = test_context(root);
        let mut args = install_args_template();
        args.yes = true;

        let mut plan = test_plan(root, "ripgrep");
        plan.source_result.requires_flake_mod = true;

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
    fn execute_edit_claude_uses_ai_path() {
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
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let edited = fs::read_to_string(root.join("packages/nix/cli.nix"))
            .expect("target file should be readable");
        assert!(!edited.contains("ripgrep"));
    }

    #[test]
    fn execute_edit_claude_failure_returns_false() {
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
        let candidates = vec![primary.clone(), homebrew, fallback.clone()];

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
}
