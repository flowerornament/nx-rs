use crate::cli::PassthroughArgs;
use crate::commands::context::AppContext;
use crate::infra::shell::run_indented_command;
use crate::output::printer::Printer;

// ─── update ──────────────────────────────────────────────────────────────────

pub fn cmd_update(args: &PassthroughArgs, ctx: &AppContext) -> i32 {
    ctx.printer.action("Updating flake inputs");

    let mut command_args: Vec<&str> = vec!["flake", "update"];
    command_args.extend(args.passthrough.iter().map(String::as_str));
    let return_code = match run_indented_command(
        "nix",
        &command_args,
        Some(&ctx.repo_root),
        &ctx.printer,
        "  ",
    ) {
        Ok(code) => code,
        Err(err) => {
            ctx.printer.error(&format!("{err:#}"));
            return 1;
        }
    };

    if return_code == 0 {
        println!();
        ctx.printer.success("Flake inputs updated");
        Printer::detail("Run 'nx rebuild' to rebuild, or 'nx upgrade' for full upgrade");
        return 0;
    }

    ctx.printer.error("Flake update failed");
    1
}
