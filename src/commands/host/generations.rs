use serde::Serialize;

use crate::cli::{
    GenerationKindArg, GenerationsArgs, GenerationsCommand, GenerationsPlanArgs,
    GenerationsPolicyArgs, GenerationsPruneArgs, GenerationsStatusArgs,
};
use crate::commands::context::HostContext;
use crate::domain::generations::{GenerationKind, PrunePlan, RetentionPolicy, plan_prune};
use crate::output::printer::Printer;

pub fn cmd_generations(args: &GenerationsArgs, ctx: &HostContext<'_>) -> i32 {
    match &args.command {
        GenerationsCommand::Status(args) => render_status(args, ctx),
        GenerationsCommand::Plan(args) => render_plan(args, ctx),
        GenerationsCommand::Prune(args) => render_prune(args, ctx),
    }
}

fn render_status(args: &GenerationsStatusArgs, ctx: &HostContext<'_>) -> i32 {
    let policy = retention_policy(&args.policy, true);
    let summary = StatusSummary {
        command: "status",
        policy,
        implementation: ImplementationState::Scaffolded,
    };
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
    let summary = PlanSummary::new(
        "plan",
        retention_policy(&args.policy, !args.no_gc),
        CommandMode::ReadOnly,
        ImplementationState::Scaffolded,
    );
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
        let summary = PlanSummary::new(
            "plan",
            retention_policy(&args.policy, !args.no_gc),
            CommandMode::ReadOnly,
            ImplementationState::DryRunAlias,
        );
        return render_json_or_text(
            &summary,
            ctx,
            false,
            "Generations Plan",
            "Dry run mapped to the generations plan scaffold.",
            "Live pruning is blocked until discovery and execution slices land.",
        );
    }

    let summary = PlanSummary::new(
        "prune",
        retention_policy(&args.policy, !args.no_gc),
        CommandMode::Mutating,
        ImplementationState::Scaffolded,
    );
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
    summary: &impl GenerationsSummary,
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
        summary.keep_newest(),
        summary.kind_label()
    ));
    Printer::detail(&format!(
        "Garbage collection: {}",
        if summary.run_gc() {
            "enabled"
        } else {
            "disabled"
        }
    ));
    if summary.has_prunable_generations() {
        Printer::detail(&format!(
            "Planned prunes: darwin={}, home-manager={}",
            summary.prune_count(GenerationKind::DarwinSystem),
            summary.prune_count(GenerationKind::HomeManager)
        ));
    } else {
        Printer::detail("Planned prunes: none");
    }
    if matches!(
        summary.implementation_state(),
        ImplementationState::DryRunAlias
    ) {
        Printer::detail("Invocation: prune --dry-run");
    }
    Printer::detail(detail);

    i32::from(error)
}

fn render_json(summary: &impl Serialize, error: bool, ctx: &HostContext<'_>) -> i32 {
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

trait GenerationsSummary: Serialize {
    fn keep_newest(&self) -> usize;
    fn kind_label(&self) -> &'static str;
    fn run_gc(&self) -> bool;
    fn implementation_state(&self) -> ImplementationState;
    fn prune_count(&self, kind: GenerationKind) -> usize;
    fn has_prunable_generations(&self) -> bool;
}

#[derive(Debug, Serialize)]
struct StatusSummary {
    command: &'static str,
    policy: RetentionPolicy,
    implementation: ImplementationState,
}

impl GenerationsSummary for StatusSummary {
    fn keep_newest(&self) -> usize {
        self.policy.keep_newest
    }

    fn kind_label(&self) -> &'static str {
        policy_kind_label(&self.policy)
    }

    fn run_gc(&self) -> bool {
        self.policy.run_gc
    }

    fn implementation_state(&self) -> ImplementationState {
        self.implementation
    }

    fn prune_count(&self, _kind: GenerationKind) -> usize {
        0
    }

    fn has_prunable_generations(&self) -> bool {
        false
    }
}

#[derive(Debug, Serialize)]
struct PlanSummary<'a> {
    command: &'a str,
    plan: PrunePlan,
    mode: CommandMode,
    implementation: ImplementationState,
}

impl<'a> PlanSummary<'a> {
    fn new(
        command: &'a str,
        policy: RetentionPolicy,
        mode: CommandMode,
        implementation: ImplementationState,
    ) -> Self {
        Self {
            command,
            plan: plan_prune(&[], policy),
            mode,
            implementation,
        }
    }
}

impl GenerationsSummary for PlanSummary<'_> {
    fn keep_newest(&self) -> usize {
        self.plan.policy.keep_newest
    }

    fn kind_label(&self) -> &'static str {
        policy_kind_label(&self.plan.policy)
    }

    fn run_gc(&self) -> bool {
        self.plan.policy.run_gc
    }

    fn implementation_state(&self) -> ImplementationState {
        self.implementation
    }

    fn prune_count(&self, kind: GenerationKind) -> usize {
        self.plan.execution.prune_ids(kind).len()
    }

    fn has_prunable_generations(&self) -> bool {
        self.plan.execution.has_prunable_generations()
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CommandMode {
    ReadOnly,
    Mutating,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ImplementationState {
    Scaffolded,
    DryRunAlias,
}

fn retention_policy(args: &GenerationsPolicyArgs, run_gc: bool) -> RetentionPolicy {
    match args.kind {
        GenerationKindArg::All => RetentionPolicy::all(args.keep, run_gc),
        GenerationKindArg::Darwin => {
            RetentionPolicy::single(args.keep, GenerationKind::DarwinSystem, run_gc)
        }
        GenerationKindArg::HomeManager => {
            RetentionPolicy::single(args.keep, GenerationKind::HomeManager, run_gc)
        }
    }
}

fn policy_kind_label(policy: &RetentionPolicy) -> &'static str {
    match (
        policy.includes(GenerationKind::DarwinSystem),
        policy.includes(GenerationKind::HomeManager),
    ) {
        (true, true) => "all",
        (true, false) => GenerationKind::DarwinSystem.label(),
        (false, true) => GenerationKind::HomeManager.label(),
        (false, false) => "none",
    }
}
