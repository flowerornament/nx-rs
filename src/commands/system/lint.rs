use serde::Serialize;

use crate::cli::LintArgs;
use crate::commands::context::SystemContext;
use crate::domain::routing::RoutingAudit;
use crate::output::printer::Printer;

#[derive(Debug, Serialize)]
struct RoutingLintJson {
    ok: bool,
    issues: Vec<String>,
}

pub fn cmd_lint(args: &LintArgs, ctx: &SystemContext<'_>) -> i32 {
    if args.json {
        let audit = RoutingAudit::scan(ctx.repo_root);
        let output = RoutingLintJson {
            ok: audit.is_clean(),
            issues: audit
                .issues()
                .iter()
                .map(|issue| issue.summary(ctx.repo_root))
                .collect(),
        };
        return match serde_json::to_string_pretty(&output) {
            Ok(text) => {
                println!("{text}");
                i32::from(!output.ok)
            }
            Err(err) => {
                ctx.printer
                    .error(&format!("lint json rendering failed: {err}"));
                1
            }
        };
    }

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
