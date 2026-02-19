use std::env;
use std::path::PathBuf;
use std::process::Command;

use anyhow::bail;

use crate::cli::{Cli, CommandKind};
use crate::commands::context::AppContext;
use crate::commands::install::cmd_install;
use crate::commands::query::{cmd_info, cmd_installed, cmd_list, cmd_status, cmd_where};
use crate::commands::remove::cmd_remove;
use crate::commands::search::cmd_search;
use crate::commands::secret::cmd_secret;
use crate::commands::system::{cmd_rebuild, cmd_test, cmd_undo, cmd_update, cmd_upgrade};
use crate::domain::config::ConfigFiles;
use crate::output::printer::Printer;
use crate::output::style::OutputStyle;

pub fn execute(cli: Cli) -> i32 {
    let style = OutputStyle::from_flags(cli.plain, cli.unicode, cli.minimal);
    let printer = Printer::new(style);

    let repo_root = match find_repo_root() {
        Ok(path) => path,
        Err(err) => {
            printer.error(&format!("{err:#}"));
            return 1;
        }
    };

    let config_files = ConfigFiles::discover(&repo_root);
    let ctx = AppContext::new(repo_root, printer, config_files);

    match cli.command {
        CommandKind::Install(args) => cmd_install(&args, &ctx),
        CommandKind::Remove(args) => cmd_remove(&args, &ctx),
        CommandKind::Secret(args) => cmd_secret(&args, &ctx),
        CommandKind::Search(args) => cmd_search(&args, &ctx),
        CommandKind::Where(args) => cmd_where(&args, &ctx),
        CommandKind::List(args) => cmd_list(&args, &ctx),
        CommandKind::Info(args) => cmd_info(&args, &ctx),
        CommandKind::Status => cmd_status(&ctx),
        CommandKind::Installed(args) => cmd_installed(&args, &ctx),
        CommandKind::Undo => cmd_undo(&ctx),
        CommandKind::Update(args) => cmd_update(&args, &ctx),
        CommandKind::Test => cmd_test(&ctx),
        CommandKind::Rebuild(args) => cmd_rebuild(&args, &ctx),
        CommandKind::Upgrade(args) => cmd_upgrade(&args, &ctx),
    }
}

fn find_repo_root() -> anyhow::Result<PathBuf> {
    if let Some(env_root) = env::var_os("B2NIX_REPO_ROOT") {
        let env_path = PathBuf::from(env_root);
        return Ok(std::fs::canonicalize(&env_path).unwrap_or(env_path));
    }

    let home_config = dirs_home().join(".nix-config");
    if home_config.exists() {
        return Ok(home_config);
    }

    if let Some(root) = git_repo_root() {
        return Ok(root);
    }

    bail!("Could not find nix-config repository")
}

fn git_repo_root() -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let candidate = PathBuf::from(&root);
    candidate.join("flake.nix").exists().then_some(candidate)
}

pub fn dirs_home() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}
