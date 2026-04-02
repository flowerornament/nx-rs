use serde::Serialize;

use crate::cli::{
    GenerationKindArg, GenerationsArgs, GenerationsCommand, GenerationsPlanArgs,
    GenerationsPolicyArgs, GenerationsPruneArgs, GenerationsStatusArgs,
};
use crate::commands::context::HostContext;
use crate::domain::generations::{GenerationKind, PrunePlan, RetentionPolicy, plan_prune};
use crate::infra::generations::{
    GenerationState, PlannedCommand, discover_generation_state, planned_commands,
};
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
    let state = match read_generation_state(ctx) {
        Ok(state) => state,
        Err(code) => return code,
    };
    let summary = StatusSummary {
        command: "status",
        plan: plan_prune(&state.generations, policy.clone()),
        policy,
        state,
        implementation: ImplementationState::Scaffolded,
    };
    render_json_or_text(
        &summary,
        ctx,
        false,
        "Generations Status",
        "Host-scoped generations discovery is active.",
        "Execution and richer rendering land in the next slice.",
    )
}

fn render_plan(args: &GenerationsPlanArgs, ctx: &HostContext<'_>) -> i32 {
    let policy = retention_policy(&args.policy, !args.no_gc);
    let state = match read_generation_state(ctx) {
        Ok(state) => state,
        Err(code) => return code,
    };
    let summary = PlanSummary::new(
        "plan",
        state,
        policy,
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
        let policy = retention_policy(&args.policy, !args.no_gc);
        let state = match read_generation_state(ctx) {
            Ok(state) => state,
            Err(code) => return code,
        };
        let summary = PlanSummary::new(
            "plan",
            state,
            policy,
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

    let policy = retention_policy(&args.policy, !args.no_gc);
    let state = match read_generation_state(ctx) {
        Ok(state) => state,
        Err(code) => return code,
    };
    let summary = PlanSummary::new(
        "prune",
        state,
        policy,
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
    Printer::detail(&format!(
        "Discovered generations: darwin={}, home-manager={}",
        summary.generation_count(GenerationKind::DarwinSystem),
        summary.generation_count(GenerationKind::HomeManager)
    ));
    Printer::detail(&format!(
        "Current generations: darwin={}, home-manager={}",
        summary.current_generation_label(GenerationKind::DarwinSystem),
        summary.current_generation_label(GenerationKind::HomeManager)
    ));
    Printer::detail(&format!(
        "Disk usage: {} used of {} ({}) on {}",
        summary.disk_usage().used,
        summary.disk_usage().size,
        summary.disk_usage().capacity,
        summary.disk_usage().mounted_on
    ));
    if !summary.commands().is_empty() {
        Printer::detail("Commands:");
        for command in summary.commands() {
            Printer::sub_detail(&format!("{} {}", command.program, command.args.join(" ")));
        }
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
    fn disk_usage(&self) -> &crate::domain::generations::DiskUsageSnapshot;
    fn generation_count(&self, kind: GenerationKind) -> usize;
    fn current_generation_label(&self, kind: GenerationKind) -> String;
    fn prune_count(&self, kind: GenerationKind) -> usize;
    fn has_prunable_generations(&self) -> bool;
    fn commands(&self) -> &[PlannedCommand];
}

#[derive(Debug, Serialize)]
struct StatusSummary {
    command: &'static str,
    policy: RetentionPolicy,
    plan: PrunePlan,
    state: GenerationState,
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

    fn disk_usage(&self) -> &crate::domain::generations::DiskUsageSnapshot {
        &self.state.disk_usage
    }

    fn generation_count(&self, kind: GenerationKind) -> usize {
        self.state
            .generations
            .iter()
            .filter(|generation| generation.kind == kind)
            .count()
    }

    fn current_generation_label(&self, kind: GenerationKind) -> String {
        current_generation_label(&self.state, kind)
    }

    fn prune_count(&self, kind: GenerationKind) -> usize {
        self.plan.execution.prune_ids(kind).len()
    }

    fn has_prunable_generations(&self) -> bool {
        self.plan.execution.has_prunable_generations()
    }

    fn commands(&self) -> &[PlannedCommand] {
        &[]
    }
}

#[derive(Debug, Serialize)]
struct PlanSummary<'a> {
    command: &'a str,
    state: GenerationState,
    plan: PrunePlan,
    commands: Vec<PlannedCommand>,
    mode: CommandMode,
    implementation: ImplementationState,
}

impl<'a> PlanSummary<'a> {
    fn new(
        command: &'a str,
        state: GenerationState,
        policy: RetentionPolicy,
        mode: CommandMode,
        implementation: ImplementationState,
    ) -> Self {
        let plan = plan_prune(&state.generations, policy);
        let commands = planned_commands(&plan);
        Self {
            command,
            state,
            plan,
            commands,
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

    fn disk_usage(&self) -> &crate::domain::generations::DiskUsageSnapshot {
        &self.state.disk_usage
    }

    fn generation_count(&self, kind: GenerationKind) -> usize {
        self.state
            .generations
            .iter()
            .filter(|generation| generation.kind == kind)
            .count()
    }

    fn current_generation_label(&self, kind: GenerationKind) -> String {
        current_generation_label(&self.state, kind)
    }

    fn prune_count(&self, kind: GenerationKind) -> usize {
        self.plan.execution.prune_ids(kind).len()
    }

    fn has_prunable_generations(&self) -> bool {
        self.plan.execution.has_prunable_generations()
    }

    fn commands(&self) -> &[PlannedCommand] {
        &self.commands
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

fn read_generation_state(ctx: &HostContext<'_>) -> Result<GenerationState, i32> {
    match discover_generation_state() {
        Ok(state) => Ok(state),
        Err(err) => {
            ctx.printer
                .error(&format!("failed to discover host generations: {err:#}"));
            Err(1)
        }
    }
}

fn current_generation_label(state: &GenerationState, kind: GenerationKind) -> String {
    state
        .generations
        .iter()
        .find(|generation| generation.kind == kind && generation.current)
        .map_or_else(
            || "none".to_string(),
            |generation| generation.id.get().to_string(),
        )
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
