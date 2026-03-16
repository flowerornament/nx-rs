use crate::commands::context::RepoContext;
use crate::infra::shell::run_indented_command;

pub fn cmd_test(ctx: &RepoContext<'_>) -> i32 {
    ctx.printer.action("Running just ci");
    println!();

    let return_code =
        match run_indented_command("just", &["ci"], Some(ctx.repo_root), ctx.printer, "  ") {
            Ok(code) => code,
            Err(err) => {
                ctx.printer.error("just ci failed");
                ctx.printer.error(&format!("{err:#}"));
                return 1;
            }
        };

    if return_code != 0 {
        ctx.printer.error("just ci failed");
        return 1;
    }

    println!();
    ctx.printer.success("just ci passed");
    0
}
