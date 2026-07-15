use crate::cli::UpdateArgs;
use crate::commands::context::RepoContext;
use crate::domain::upgrade::build_flake_update_args;
use crate::infra::nix_output::NixOutputMode;
use crate::infra::shell::{first_unpresented_output, run_stdout_collecting_nix_stderr_with_env};
use crate::output::printer::Printer;

// ─── update ──────────────────────────────────────────────────────────────────

pub fn cmd_update(args: &UpdateArgs, ctx: &RepoContext<'_>) -> i32 {
    ctx.printer.action("Updating flake inputs");

    let base_args = build_flake_update_args(&[], &args.passthrough);
    let nix_args = NixOutputMode::Structured.command_args(&base_args);
    let command_args = nix_args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = match run_stdout_collecting_nix_stderr_with_env(
        "nix",
        &command_args,
        Some(ctx.repo_root),
        None,
    ) {
        Ok(output) => output,
        Err(err) => {
            ctx.printer.error(&format!("{err:#}"));
            return 1;
        }
    };

    if output.code == 0 {
        println!();
        ctx.printer.success("Flake inputs updated");
        Printer::detail("Run 'nx rebuild' to rebuild, or 'nx upgrade' for full upgrade");
        return 0;
    }

    ctx.printer.error("Flake update failed");
    let detail = first_unpresented_output(&output);
    if !detail.is_empty() {
        Printer::detail(detail);
    }
    1
}
