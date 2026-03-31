use crate::commands::context::SystemContext;
use crate::domain::routing::RoutingAudit;
use crate::output::printer::Printer;

pub fn cmd_lint(ctx: &SystemContext<'_>) -> i32 {
    ctx.printer.action("Linting nx routing metadata");

    let audit = RoutingAudit::scan(ctx.repo_root);
    if audit.is_clean() {
        ctx.printer.success("nx routing metadata passed");
        return 0;
    }

    ctx.printer.error("nx routing metadata failed");
    println!();
    Printer::detail("Fix these issues:");
    for issue in audit.issues() {
        Printer::detail(&format!("- {}", issue.summary(ctx.repo_root)));
    }
    1
}
