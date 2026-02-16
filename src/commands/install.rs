use crate::cli::InstallArgs;
use crate::commands::context::AppContext;
use crate::commands::shared::{SnippetMode, relative_location, show_snippet};
use crate::domain::plan::{
    InsertionMode, InstallPlan, build_install_plan, nix_manifest_candidates,
};
use crate::domain::source::SourceResult;
use crate::infra::ai_engine::{AiEngine, build_routing_context, select_engine};
use crate::infra::file_edit::apply_edit;
use crate::infra::finder::find_package;

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

    let mut success_count = 0;

    for package in &args.packages {
        if install_one(package, args, ctx, engine.as_ref(), &routing_context) {
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
    engine: &dyn AiEngine,
    routing_context: &str,
) -> bool {
    // Check if already installed
    match find_package(package, &ctx.repo_root) {
        Ok(Some(location)) => {
            println!();
            ctx.printer.success(&format!(
                "{package} already installed ({})",
                relative_location(&location, &ctx.repo_root)
            ));
            return true;
        }
        Ok(None) => {} // not installed — proceed
        Err(err) => {
            ctx.printer.error(&format!("install lookup failed: {err}"));
            return false;
        }
    }

    let sr = source_result_from_args(package, args);

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

    execute_edit(&plan, &rel_target, ctx)
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

/// Apply the deterministic file edit and report outcome.
fn execute_edit(plan: &InstallPlan, rel_target: &str, ctx: &AppContext) -> bool {
    match apply_edit(plan) {
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

/// Build a `SourceResult` from CLI flags when search orchestration is not yet wired.
fn source_result_from_args(package: &str, args: &InstallArgs) -> SourceResult {
    let source = if args.cask {
        "cask"
    } else if args.mas {
        "mas"
    } else if args.nur {
        "nur"
    } else if let Some(ref s) = args.source {
        s.as_str()
    } else {
        "nxs"
    };

    SourceResult {
        attr: if source == "cask" || source == "mas" {
            None
        } else {
            Some(package.to_string())
        },
        requires_flake_mod: args.bleeding_edge || source == "flake-input",
        ..SourceResult::new(package, source)
    }
}
