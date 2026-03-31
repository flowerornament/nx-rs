use crate::commands::context::SystemContext;
use crate::domain::routing::RoutingAudit;
use crate::output::printer::Printer;

pub fn cmd_lint(ctx: &SystemContext<'_>) -> i32 {
    match run_routing_lint(
        ctx,
        "Linting nx routing metadata",
        "nx routing metadata passed",
        "nx routing metadata failed",
        "Fix these issues:",
    ) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

pub(super) fn run_routing_lint(
    ctx: &SystemContext<'_>,
    action: &str,
    success: &str,
    failure: &str,
    detail_intro: &str,
) -> Result<(), i32> {
    ctx.printer.action(action);

    let audit = RoutingAudit::scan(ctx.repo_root);
    if audit.is_clean() {
        ctx.printer.success(success);
        return Ok(());
    }

    ctx.printer.error(failure);
    println!();
    Printer::detail(detail_intro);
    for issue in audit.issues() {
        Printer::detail(&format!("- {}", issue.summary(ctx.repo_root)));
    }
    Err(1)
}
