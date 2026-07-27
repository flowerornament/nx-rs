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
    run_planned_command, snapshot_nix_disk_usage,
};
use crate::output::printer::Printer;

pub fn cmd_generations(args: &GenerationsArgs, ctx: &HostContext<'_>) -> i32 {
    match &args.command {
        GenerationsCommand::Status(status) => render_status(status, args.json, ctx),
        GenerationsCommand::Plan(plan) => render_plan(plan, args.json, ctx),
        GenerationsCommand::Prune(prune) => render_prune(prune, args.json, ctx),
    }
}

fn render_status(args: &GenerationsStatusArgs, json: bool, ctx: &HostContext<'_>) -> i32 {
    let summary = match load_command_summary(
        "status",
        retention_policy(&args.policy, true),
        CommandMode::ReadOnly,
        false,
        ctx,
    ) {
        Ok(summary) => summary,
        Err(code) => return code,
    };

    if json {
        return render_json(&summary, 0, ctx.printer);
    }

    ctx.printer.action("Inspecting host generations");
    println!();
    Printer::heading("Generations Status");
    render_summary_sections(&summary, false);
    if summary.has_prunable_generations() {
        Printer::detail("Run `nx generations plan` to review exact prune steps.");
    } else if summary.run_gc() {
        Printer::detail("Run `nx generations prune` to garbage collect now.");
    }
    0
}

fn render_plan(args: &GenerationsPlanArgs, json: bool, ctx: &HostContext<'_>) -> i32 {
    let summary = match load_command_summary(
        "plan",
        retention_policy(&args.policy, !args.no_gc),
        CommandMode::ReadOnly,
        true,
        ctx,
    ) {
        Ok(summary) => summary,
        Err(code) => return code,
    };

    if json {
        return render_json(&summary, 0, ctx.printer);
    }

    ctx.printer.action("Preparing generations plan");
    println!();
    Printer::heading("Generations Plan");
    render_summary_sections(&summary, true);
    0
}

fn render_prune(args: &GenerationsPruneArgs, json: bool, ctx: &HostContext<'_>) -> i32 {
    if args.dry_run {
        let plan_args = GenerationsPlanArgs {
            policy: args.policy.clone(),
            no_gc: args.no_gc,
        };
        return render_plan(&plan_args, json, ctx);
    }

    let summary = match load_command_summary(
        "prune",
        retention_policy(&args.policy, !args.no_gc),
        CommandMode::Mutating,
        true,
        ctx,
    ) {
        Ok(summary) => summary,
        Err(code) => return code,
    };

    let outcome = execute_prune_with(
        &summary,
        args.yes,
        ctx.printer,
        Printer::confirm,
        run_planned_command,
        snapshot_nix_disk_usage,
    );

    if json {
        return render_json(
            &PruneJsonOutput {
                summary: &summary,
                outcome: &outcome,
            },
            outcome.exit_code(),
            ctx.printer,
        );
    }

    render_prune_outcome(&summary, &outcome, ctx.printer);
    outcome.exit_code()
}

fn render_summary_sections(summary: &CommandSummary<'_>, show_plan_details: bool) {
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

    if !show_plan_details {
        return;
    }

    if summary.has_prunable_generations() {
        Printer::detail(&format!(
            "Planned prunes: {}={}, {}={}",
            GenerationKind::DarwinSystem.label(),
            summary.prune_count(GenerationKind::DarwinSystem),
            GenerationKind::HomeManager.label(),
            summary.prune_count(GenerationKind::HomeManager)
        ));
        render_prune_ids(summary);
    } else {
        Printer::detail("Planned prunes: none");
    }

    if !summary.commands.is_empty() {
        Printer::detail("Commands:");
        for command in &summary.commands {
            Printer::sub_detail(&format!("{} {}", command.program, command.args.join(" ")));
        }
    }
}

fn render_prune_ids(summary: &CommandSummary<'_>) {
    let darwin_ids = render_generation_ids(
        summary
            .plan
            .execution
            .prune_ids(GenerationKind::DarwinSystem),
    );
    if !darwin_ids.is_empty() {
        Printer::detail(&format!(
            "{} prune IDs: {darwin_ids}",
            GenerationKind::DarwinSystem.label()
        ));
    }

    let home_manager_ids = render_generation_ids(
        summary
            .plan
            .execution
            .prune_ids(GenerationKind::HomeManager),
    );
    if !home_manager_ids.is_empty() {
        Printer::detail(&format!(
            "{} prune IDs: {home_manager_ids}",
            GenerationKind::HomeManager.label()
        ));
    }
}

fn render_prune_outcome(summary: &CommandSummary<'_>, outcome: &PruneOutcome, printer: &Printer) {
    match outcome.status {
        PruneStatus::NoChanges => {
            printer.success("Nothing to prune");
            Printer::detail("No generations matched the selected retention policy.");
        }
        PruneStatus::Cancelled => {
            Printer::body("Cancelled.");
        }
        PruneStatus::Succeeded => {
            println!();
            printer.success("Generations pruned");
            Printer::heading("Prune Result");
            render_summary_sections(summary, true);
            if let Some(after) = &outcome.after_disk_usage {
                Printer::detail(&format!(
                    "Disk usage after: {} used of {} ({}) on {}",
                    after.used, after.size, after.capacity, after.mounted_on
                ));
            }
        }
        PruneStatus::CommandFailed => {
            println!();
            printer.error("Generations prune failed");
            render_execution_failure(outcome);
        }
        PruneStatus::RefreshFailed => {
            println!();
            printer.error("Generations pruned, but status refresh failed");
            render_execution_failure(outcome);
        }
    }
}

fn render_execution_failure(outcome: &PruneOutcome) {
    if !outcome.executed_commands.is_empty() {
        Printer::detail("Completed before failure:");
        for command in &outcome.executed_commands {
            if matches!(command.status, ExecutedCommandStatus::Completed) {
                Printer::sub_detail(&command.description);
            }
        }
    }

    if let Some(command) = &outcome.failed_command {
        Printer::detail(&format!("Failed step: {}", command.description));
    }
    if let Some(error) = &outcome.error {
        Printer::detail(&format!("Details: {error}"));
    }
}

fn render_json(value: &impl Serialize, exit_code: i32, printer: &Printer) -> i32 {
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            println!("{text}");
            exit_code
        }
        Err(err) => {
            printer.error(&format!("failed to render generations output: {err}"));
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PruneOutcome {
    status: PruneStatus,
    executed_commands: Vec<ExecutedCommand>,
    failed_command: Option<ExecutedCommand>,
    after_disk_usage: Option<DiskUsageSnapshot>,
    error: Option<String>,
}

impl PruneOutcome {
    const fn exit_code(&self) -> i32 {
        match self.status {
            PruneStatus::NoChanges | PruneStatus::Cancelled | PruneStatus::Succeeded => 0,
            PruneStatus::CommandFailed | PruneStatus::RefreshFailed => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PruneStatus {
    NoChanges,
    Cancelled,
    Succeeded,
    CommandFailed,
    RefreshFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ExecutedCommand {
    description: String,
    program: String,
    args: Vec<String>,
    status: ExecutedCommandStatus,
}

impl ExecutedCommand {
    fn from_planned(command: &PlannedCommand, status: ExecutedCommandStatus) -> Self {
        Self {
            description: command.description.to_string(),
            program: command.program.clone(),
            args: command.args.clone(),
            status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ExecutedCommandStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CommandMode {
    ReadOnly,
    Mutating,
}

#[derive(Debug, Serialize)]
struct PruneJsonOutput<'a> {
    summary: &'a CommandSummary<'a>,
    outcome: &'a PruneOutcome,
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
    include_commands: bool,
    ctx: &HostContext<'_>,
) -> Result<CommandSummary<'a>, i32> {
    let state = match discover_generation_state() {
        Ok(state) => state,
        Err(err) => {
            ctx.printer
                .error(&format!("failed to discover host generations: {err:#}"));
            return Err(1);
        }
    };
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
    })
}

fn execute_prune_with<C, R, S>(
    summary: &CommandSummary<'_>,
    yes: bool,
    printer: &Printer,
    confirm: C,
    mut run_command: R,
    snapshot_after: S,
) -> PruneOutcome
where
    C: FnOnce(&str, bool) -> bool,
    R: FnMut(&PlannedCommand, &Printer) -> anyhow::Result<()>,
    S: FnOnce() -> anyhow::Result<DiskUsageSnapshot>,
{
    if summary.commands.is_empty() {
        return PruneOutcome {
            status: PruneStatus::NoChanges,
            executed_commands: Vec::new(),
            failed_command: None,
            after_disk_usage: None,
            error: None,
        };
    }

    let prompt = prune_confirmation_prompt(summary);
    if !yes && !confirm(&prompt, false) {
        return PruneOutcome {
            status: PruneStatus::Cancelled,
            executed_commands: Vec::new(),
            failed_command: None,
            after_disk_usage: None,
            error: None,
        };
    }

    let mut executed_commands = Vec::new();
    for command in &summary.commands {
        printer.action(command.description);
        println!();
        match run_command(command, printer) {
            Ok(()) => executed_commands.push(ExecutedCommand::from_planned(
                command,
                ExecutedCommandStatus::Completed,
            )),
            Err(err) => {
                return PruneOutcome {
                    status: PruneStatus::CommandFailed,
                    executed_commands,
                    failed_command: Some(ExecutedCommand::from_planned(
                        command,
                        ExecutedCommandStatus::Failed,
                    )),
                    after_disk_usage: None,
                    error: Some(err.to_string()),
                };
            }
        }
    }

    match snapshot_after() {
        Ok(after_disk_usage) => PruneOutcome {
            status: PruneStatus::Succeeded,
            executed_commands,
            failed_command: None,
            after_disk_usage: Some(after_disk_usage),
            error: None,
        },
        Err(err) => PruneOutcome {
            status: PruneStatus::RefreshFailed,
            executed_commands,
            failed_command: None,
            after_disk_usage: None,
            error: Some(err.to_string()),
        },
    }
}

fn prune_confirmation_prompt(summary: &CommandSummary<'_>) -> String {
    let darwin = summary.prune_count(GenerationKind::DarwinSystem);
    let home_manager = summary.prune_count(GenerationKind::HomeManager);
    match (darwin, home_manager, summary.run_gc()) {
        (0, 0, true) => "Run garbage collection?".to_string(),
        (0, 0, false) => "Apply generations prune plan?".to_string(),
        (_, _, true) => format!(
            "Prune {} {} generation(s), {} {} generation(s), and run garbage collection?",
            darwin,
            GenerationKind::DarwinSystem.label(),
            home_manager,
            GenerationKind::HomeManager.label()
        ),
        (_, _, false) => format!(
            "Prune {} {} generation(s) and {} {} generation(s)?",
            darwin,
            GenerationKind::DarwinSystem.label(),
            home_manager,
            GenerationKind::HomeManager.label()
        ),
    }
}

fn render_current_generation(id: Option<GenerationId>) -> String {
    id.map_or_else(|| "none".to_string(), |id| id.get().to_string())
}

fn render_generation_ids(ids: &[GenerationId]) -> String {
    ids.iter()
        .map(|id| id.get().to_string())
        .collect::<Vec<_>>()
        .join(" ")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::style::OutputStyle;
    use anyhow::Error;

    fn test_summary(commands: Vec<PlannedCommand>) -> CommandSummary<'static> {
        let state = GenerationState {
            generations: Vec::new(),
            disk_usage: DiskUsageSnapshot {
                filesystem: "/dev/disk-test".to_string(),
                size: "100Gi".to_string(),
                used: "40Gi".to_string(),
                available: "60Gi".to_string(),
                capacity: "40%".to_string(),
                mounted_on: "/nix".to_string(),
                available_bytes: 60 * 1024 * 1024 * 1024,
            },
        };
        let policy = RetentionPolicy::all(10, true);
        let plan = plan_prune(&state.generations, policy);
        CommandSummary {
            command: "prune",
            overview: GenerationOverview::from_state(&state),
            state,
            plan,
            commands,
            mode: CommandMode::Mutating,
        }
    }

    fn test_printer() -> Printer {
        Printer::new(OutputStyle::from_flags(true, false, true))
    }

    #[test]
    fn execute_prune_with_no_changes_is_noop() {
        let summary = test_summary(Vec::new());
        let outcome = execute_prune_with(
            &summary,
            false,
            &test_printer(),
            |_prompt, _default| unreachable!("no-op should not prompt"),
            |_command, _printer| unreachable!("no-op should not run commands"),
            || unreachable!("no-op should not snapshot after"),
        );

        assert_eq!(outcome.status, PruneStatus::NoChanges);
        assert_eq!(outcome.exit_code(), 0);
    }

    #[test]
    fn execute_prune_with_cancelled_prompt_stops_before_running_commands() {
        let summary = test_summary(vec![PlannedCommand {
            description: "collect garbage",
            program: "sudo".to_string(),
            args: vec!["nix-collect-garbage".to_string(), "-d".to_string()],
        }]);

        let outcome = execute_prune_with(
            &summary,
            false,
            &test_printer(),
            |_prompt, _default| false,
            |_command, _printer| unreachable!("cancel should not run commands"),
            || unreachable!("cancel should not snapshot after"),
        );

        assert_eq!(outcome.status, PruneStatus::Cancelled);
        assert_eq!(outcome.exit_code(), 0);
    }

    #[test]
    fn execute_prune_with_command_failure_reports_partial_progress() {
        let summary = test_summary(vec![
            PlannedCommand {
                description: "step one",
                program: "one".to_string(),
                args: vec!["a".to_string()],
            },
            PlannedCommand {
                description: "step two",
                program: "two".to_string(),
                args: vec!["b".to_string()],
            },
        ]);

        let mut seen = Vec::new();
        let outcome = execute_prune_with(
            &summary,
            true,
            &test_printer(),
            |_prompt, _default| unreachable!("--yes should skip confirm"),
            |command, _printer| {
                seen.push(command.description.to_string());
                if command.description == "step two" {
                    Err(Error::msg("boom"))
                } else {
                    Ok(())
                }
            },
            || unreachable!("failed execution should not snapshot after"),
        );

        assert_eq!(seen, vec!["step one".to_string(), "step two".to_string()]);
        assert_eq!(outcome.status, PruneStatus::CommandFailed);
        assert_eq!(outcome.executed_commands.len(), 1);
        assert_eq!(
            outcome
                .failed_command
                .as_ref()
                .expect("failed command")
                .description,
            "step two"
        );
        assert_eq!(outcome.exit_code(), 1);
    }

    #[test]
    fn execute_prune_with_refresh_failure_returns_nonzero() {
        let summary = test_summary(vec![PlannedCommand {
            description: "collect garbage",
            program: "sudo".to_string(),
            args: vec!["nix-collect-garbage".to_string(), "-d".to_string()],
        }]);

        let outcome = execute_prune_with(
            &summary,
            true,
            &test_printer(),
            |_prompt, _default| unreachable!("--yes should skip confirm"),
            |_command, _printer| Ok(()),
            || Err(Error::msg("refresh failed")),
        );

        assert_eq!(outcome.status, PruneStatus::RefreshFailed);
        assert_eq!(outcome.exit_code(), 1);
    }
}
