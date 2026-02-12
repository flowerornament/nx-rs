use std::env;
use std::path::PathBuf;
use std::process::Command;

use crate::cli::{Cli, CommandKind};
use crate::commands::context::AppContext;
use crate::commands::install::cmd_install;
use crate::commands::query::{cmd_info, cmd_installed, cmd_list, cmd_status, cmd_where};
use crate::commands::remove::cmd_remove;
use crate::commands::system::{cmd_rebuild, cmd_test, cmd_update};
use crate::output::printer::Printer;
use crate::output::style::OutputStyle;

pub fn execute(cli: Cli) -> i32 {
    let style = OutputStyle::from_flags(cli.plain, cli.unicode, cli.minimal);
    let printer = Printer::new(style);

    let repo_root = match find_repo_root() {
        Ok(path) => path,
        Err(message) => {
            printer.error(&message);
            return 1;
        }
    };

    let ctx = AppContext::new(repo_root, printer);

    match cli.command {
        CommandKind::Install(args) => cmd_install(&args, &ctx.repo_root, &ctx.printer),
        CommandKind::Remove(args) => cmd_remove(&args, &ctx.repo_root, &ctx.printer),
        CommandKind::Where(args) => cmd_where(&args, &ctx.repo_root),
        CommandKind::List(args) => cmd_list(&args, &ctx.repo_root),
        CommandKind::Info(args) => cmd_info(&args, &ctx.repo_root),
        CommandKind::Status => cmd_status(&ctx.repo_root),
        CommandKind::Installed(args) => cmd_installed(&args, &ctx.repo_root),
        CommandKind::Undo => 0,
        CommandKind::Update(args) => cmd_update(&args, &ctx.repo_root, &ctx.printer),
        CommandKind::Test => cmd_test(&ctx.repo_root, &ctx.printer),
        CommandKind::Rebuild(args) => cmd_rebuild(&args, &ctx.repo_root, &ctx.printer),
        CommandKind::Upgrade(_args) => 0,
    }
}

fn find_repo_root() -> Result<PathBuf, String> {
    if let Some(env_root) = env::var_os("B2NIX_REPO_ROOT") {
        let env_path = PathBuf::from(env_root);
        return Ok(std::fs::canonicalize(&env_path).unwrap_or(env_path));
    }

    let git_output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|err| format!("git root detection failed: {err}"))?;

    if git_output.status.success() {
        let root = String::from_utf8_lossy(&git_output.stdout)
            .trim()
            .to_string();
        if !root.is_empty() {
            let candidate = PathBuf::from(&root);
            if candidate.join("flake.nix").exists() {
                return Ok(candidate);
            }
        }
    }

    let fallback = dirs_home().join(".nix-config");
    if fallback.exists() {
        return Ok(fallback);
    }

    Err("Could not find nix-config repository".to_string())
}

fn dirs_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
