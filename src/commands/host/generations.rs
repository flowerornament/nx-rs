use serde::Serialize;

use crate::cli::{
    GenerationKindArg, GenerationsArgs, GenerationsCommand, GenerationsPlanArgs,
    GenerationsPolicyArgs, GenerationsPruneArgs, GenerationsStatusArgs,
};
use crate::commands::context::HostContext;
use crate::output::printer::Printer;

pub fn cmd_generations(args: &GenerationsArgs, ctx: &HostContext<'_>) -> i32 {
    match &args.command {
        GenerationsCommand::Status(args) => render_status(args, ctx),
        GenerationsCommand::Plan(args) => render_plan(args, ctx),
        GenerationsCommand::Prune(args) => render_prune(args, ctx),
    }
}

fn render_status(args: &GenerationsStatusArgs, ctx: &HostContext<'_>) -> i32 {
    let summary = CommandSummary::new("status", &args.policy, true, false, false);
    render_json_or_text(
        &summary,
        ctx,
        false,
        "Generations Status",
        "Host-scoped generations support is scaffolded.",
        "Discovery and retention planning land in the next slice.",
    )
}

fn render_plan(args: &GenerationsPlanArgs, ctx: &HostContext<'_>) -> i32 {
    let summary = CommandSummary::new("plan", &args.policy, !args.no_gc, false, false);
    render_json_or_text(
        &summary,
        ctx,
        false,
        "Generations Plan",
        "Host-scoped generations planning is scaffolded.",
        "Discovery adapters and exact keep/prune decisions land in the next slice.",
    )
}

fn render_prune(args: &GenerationsPruneArgs, ctx: &HostContext<'_>) -> i32 {
    if args.dry_run {
        let summary = CommandSummary::new("plan", &args.policy, !args.no_gc, false, true);
        return render_json_or_text(
            &summary,
            ctx,
            false,
            "Generations Plan",
            "Dry run mapped to the generations plan scaffold.",
            "Live pruning is blocked until discovery and execution slices land.",
        );
    }

    let summary = CommandSummary::new("prune", &args.policy, !args.no_gc, true, false);
    render_json_or_text(
        &summary,
        ctx,
        true,
        "Generations Prune",
        "Live generations pruning is not implemented yet.",
        "Use `nx generations plan` or `nx generations prune --dry-run` for now.",
    )
}

fn render_json_or_text(
    summary: &CommandSummary<'_>,
    ctx: &HostContext<'_>,
    error: bool,
    heading: &str,
    body: &str,
    detail: &str,
) -> i32 {
    if ctx.wants_json(false) {
        return render_json(summary, error, ctx);
    }

    if error {
        ctx.printer.warn(body);
    } else {
        ctx.printer.action(body);
    }
    println!();
    Printer::heading(heading);
    Printer::body(&format!(
        "Policy: keep newest {} generation(s) for {}",
        summary.keep, summary.kind
    ));
    Printer::detail(&format!(
        "Garbage collection: {}",
        if summary.run_gc {
            "enabled"
        } else {
            "disabled"
        }
    ));
    if matches!(summary.implementation, ImplementationState::DryRunAlias) {
        Printer::detail("Invocation: prune --dry-run");
    }
    Printer::detail(detail);

    i32::from(error)
}

fn render_json(summary: &CommandSummary<'_>, error: bool, ctx: &HostContext<'_>) -> i32 {
    match serde_json::to_string_pretty(&summary) {
        Ok(text) => {
            println!("{text}");
            i32::from(error)
        }
        Err(err) => {
            ctx.printer
                .error(&format!("failed to render generations output: {err}"));
            1
        }
    }
}

#[derive(Debug, Serialize)]
struct CommandSummary<'a> {
    command: &'a str,
    keep: usize,
    kind: &'a str,
    run_gc: bool,
    mode: CommandMode,
    implementation: ImplementationState,
}

impl<'a> CommandSummary<'a> {
    fn new(
        command: &'a str,
        policy: &'a GenerationsPolicyArgs,
        run_gc: bool,
        mutating: bool,
        dry_run_alias: bool,
    ) -> Self {
        Self {
            command,
            keep: policy.keep,
            kind: generation_kind_label(policy.kind),
            run_gc,
            mode: if mutating {
                CommandMode::Mutating
            } else {
                CommandMode::ReadOnly
            },
            implementation: if dry_run_alias {
                ImplementationState::DryRunAlias
            } else {
                ImplementationState::Scaffolded
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CommandMode {
    ReadOnly,
    Mutating,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ImplementationState {
    Scaffolded,
    DryRunAlias,
}

const fn generation_kind_label(kind: GenerationKindArg) -> &'static str {
    match kind {
        GenerationKindArg::All => "all",
        GenerationKindArg::Darwin => "darwin",
        GenerationKindArg::HomeManager => "home-manager",
    }
}
