use serde::Serialize;

use crate::cli::{
    GenerationKindArg, GenerationsArgs, GenerationsCommand, GenerationsPlanArgs,
    GenerationsPolicyArgs, GenerationsPruneArgs, GenerationsStatusArgs,
};
use crate::commands::context::HostContext;
use crate::domain::generations::{
    DiskUsageSnapshot, GenerationId, GenerationKind, PrunePlan, RetentionPolicy, plan_prune,
};
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
    let summary = match load_command_summary(
        "status",
        retention_policy(&args.policy, true),
        CommandMode::ReadOnly,
        ImplementationState::Scaffolded,
        false,
        ctx,
    ) {
        Ok(summary) => summary,
        Err(code) => return code,
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
    let summary = match load_command_summary(
        "plan",
        retention_policy(&args.policy, !args.no_gc),
        CommandMode::ReadOnly,
        ImplementationState::Scaffolded,
        true,
        ctx,
    ) {
        Ok(summary) => summary,
        Err(code) => return code,
    };

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
        let summary = match load_command_summary(
            "plan",
            retention_policy(&args.policy, !args.no_gc),
            CommandMode::ReadOnly,
            ImplementationState::DryRunAlias,
            true,
            ctx,
        ) {
            Ok(summary) => summary,
            Err(code) => return code,
        };

        return render_json_or_text(
            &summary,
            ctx,
            false,
            "Generations Plan",
            "Dry run mapped to the generations plan scaffold.",
            "Live pruning is blocked until discovery and execution slices land.",
        );
    }

    let summary = match load_command_summary(
        "prune",
        retention_policy(&args.policy, !args.no_gc),
        CommandMode::Mutating,
        ImplementationState::Scaffolded,
        true,
        ctx,
    ) {
        Ok(summary) => summary,
        Err(code) => return code,
    };

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
            "Planned prunes: {}={}, {}={}",
            GenerationKind::DarwinSystem.label(),
            summary.prune_count(GenerationKind::DarwinSystem),
            GenerationKind::HomeManager.label(),
            summary.prune_count(GenerationKind::HomeManager)
        ));
    } else {
        Printer::detail("Planned prunes: none");
    }
    Printer::detail(&format!(
        "Discovered generations: {}={}, {}={}",
        GenerationKind::DarwinSystem.label(),
        summary.generation_count(GenerationKind::DarwinSystem),
        GenerationKind::HomeManager.label(),
        summary.generation_count(GenerationKind::HomeManager)
    ));
    Printer::detail(&format!(
        "Current generations: {}={}, {}={}",
        GenerationKind::DarwinSystem.label(),
        summary.current_generation_label(GenerationKind::DarwinSystem),
        GenerationKind::HomeManager.label(),
        summary.current_generation_label(GenerationKind::HomeManager)
    ));
    Printer::detail(&format!(
        "Disk usage: {} used of {} ({}) on {}",
        summary.disk_usage().used,
        summary.disk_usage().size,
        summary.disk_usage().capacity,
        summary.disk_usage().mounted_on
    ));
    if !summary.commands.is_empty() {
        Printer::detail("Commands:");
        for command in &summary.commands {
            Printer::sub_detail(&format!("{} {}", command.program, command.args.join(" ")));
        }
    }
    if matches!(summary.implementation, ImplementationState::DryRunAlias) {
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

#[derive(Debug, Serialize)]
struct CommandSummary<'a> {
    command: &'a str,
    state: GenerationState,
    overview: GenerationOverview,
    plan: PrunePlan,
    commands: Vec<PlannedCommand>,
    mode: CommandMode,
    implementation: ImplementationState,
}

impl CommandSummary<'_> {
    fn keep_newest(&self) -> usize {
        self.plan.policy.keep_newest
    }

    fn kind_label(&self) -> &'static str {
        policy_kind_label(&self.plan.policy)
    }

    fn run_gc(&self) -> bool {
        self.plan.policy.run_gc
    }

    fn prune_count(&self, kind: GenerationKind) -> usize {
        self.plan.execution.prune_ids(kind).len()
    }

    fn has_prunable_generations(&self) -> bool {
        self.plan.execution.has_prunable_generations()
    }

    fn generation_count(&self, kind: GenerationKind) -> usize {
        match kind {
            GenerationKind::DarwinSystem => self.overview.darwin_count,
            GenerationKind::HomeManager => self.overview.home_manager_count,
        }
    }

    fn current_generation_label(&self, kind: GenerationKind) -> String {
        match kind {
            GenerationKind::DarwinSystem => render_current_generation(self.overview.darwin_current),
            GenerationKind::HomeManager => {
                render_current_generation(self.overview.home_manager_current)
            }
        }
    }

    fn disk_usage(&self) -> &DiskUsageSnapshot {
        &self.state.disk_usage
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
struct GenerationOverview {
    darwin_count: usize,
    home_manager_count: usize,
    darwin_current: Option<GenerationId>,
    home_manager_current: Option<GenerationId>,
}

impl GenerationOverview {
    fn from_state(state: &GenerationState) -> Self {
        let mut overview = Self {
            darwin_count: 0,
            home_manager_count: 0,
            darwin_current: None,
            home_manager_current: None,
        };

        for generation in &state.generations {
            match generation.kind {
                GenerationKind::DarwinSystem => {
                    overview.darwin_count += 1;
                    if generation.current {
                        overview.darwin_current = Some(generation.id);
                    }
                }
                GenerationKind::HomeManager => {
                    overview.home_manager_count += 1;
                    if generation.current {
                        overview.home_manager_current = Some(generation.id);
                    }
                }
            }
        }

        overview
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

fn load_command_summary<'a>(
    command: &'a str,
    policy: RetentionPolicy,
    mode: CommandMode,
    implementation: ImplementationState,
    include_commands: bool,
    ctx: &HostContext<'_>,
) -> Result<CommandSummary<'a>, i32> {
    let state = read_generation_state(ctx)?;
    let overview = GenerationOverview::from_state(&state);
    let plan = plan_prune(&state.generations, policy);
    let commands = if include_commands {
        planned_commands(&plan)
    } else {
        Vec::new()
    };

    Ok(CommandSummary {
        command,
        state,
        overview,
        plan,
        commands,
        mode,
        implementation,
    })
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

fn render_current_generation(id: Option<GenerationId>) -> String {
    id.map_or_else(|| "none".to_string(), |id| id.get().to_string())
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
