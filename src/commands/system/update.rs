use crate::cli::PassthroughArgs;
use crate::commands::context::RepoContext;
use crate::domain::upgrade::build_flake_update_args;
use crate::infra::shell::run_indented_command;
use crate::output::printer::Printer;

// ─── update ──────────────────────────────────────────────────────────────────

pub fn cmd_update(args: &PassthroughArgs, ctx: &RepoContext<'_>) -> i32 {
    ctx.printer.action("Updating flake inputs");

    let raw_args = build_flake_update_args(&[], &args.passthrough);
    let command_args = raw_args.iter().map(String::as_str).collect::<Vec<_>>();
    let return_code =
        match run_indented_command("nix", &command_args, Some(ctx.repo_root), ctx.printer, "  ") {
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
