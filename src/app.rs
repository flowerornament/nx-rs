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
use crate::infra::self_refresh::maybe_refresh_before_system_command;
use crate::output::printer::Printer;
use crate::output::style::OutputStyle;

pub fn execute(cli: Cli) -> i32 {
    let style = OutputStyle::from_flags(cli.plain, cli.unicode, cli.minimal);
    let printer = Printer::new(style);
    let needs_refresh = matches!(
        cli.command,
        CommandKind::Rebuild(_) | CommandKind::Upgrade(_)
    );
    if let Some(code) = maybe_refresh_before_system_command(needs_refresh, &printer) {
        return code;
    }

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
        let path = PathBuf::from(env_root);
        // Keep Python parity: accept unresolved paths when canonicalization fails.
        return Ok(std::fs::canonicalize(&path).unwrap_or(path));
    }

    let git_root = git_repo_root();
    let home_config = dirs_home().join(".nix-config");

    resolve_repo_root(None, git_root, home_config)
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

fn resolve_repo_root(
    env_root: Option<PathBuf>,
    git_root: Option<PathBuf>,
    home_config: PathBuf,
) -> anyhow::Result<PathBuf> {
    if let Some(env_path) = env_root {
        return Ok(env_path);
    }
    if let Some(root) = git_root {
        return Ok(root);
    }
    if home_config.exists() {
        return Ok(home_config);
    }
    bail!("Could not find nix-config repository")
}

pub fn dirs_home() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::resolve_repo_root;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn resolve_repo_root_prefers_env_var() {
        let home = TempDir::new().expect("temp dir should be created");
        let home_config = home.path().join(".nix-config");
        std::fs::create_dir_all(&home_config).expect("home config should exist");
        let env_root = PathBuf::from("/tmp/env-root");
        let git_root = PathBuf::from("/tmp/git-root");

        let resolved = resolve_repo_root(Some(env_root.clone()), Some(git_root), home_config)
            .expect("resolve");

        assert_eq!(resolved, env_root);
    }

    #[test]
    fn resolve_repo_root_prefers_git_root_over_home_default() {
        let home = TempDir::new().expect("temp dir should be created");
        let home_config = home.path().join(".nix-config");
        std::fs::create_dir_all(&home_config).expect("home config should exist");
        let git_root = PathBuf::from("/tmp/git-root");

        let resolved =
            resolve_repo_root(None, Some(git_root.clone()), home_config).expect("resolve");

        assert_eq!(resolved, git_root);
    }

    #[test]
    fn resolve_repo_root_falls_back_to_home_default() {
        let home = TempDir::new().expect("temp dir should be created");
        let home_config = home.path().join(".nix-config");
        std::fs::create_dir_all(&home_config).expect("home config should exist");

        let resolved = resolve_repo_root(None, None, home_config.clone()).expect("resolve");

        assert_eq!(resolved, home_config);
    }

    #[test]
    fn resolve_repo_root_errors_without_any_source() {
        let home = TempDir::new().expect("temp dir should be created");
        let missing_home_config = home.path().join(".nix-config");

        let err = resolve_repo_root(None, None, missing_home_config).expect_err("must fail");
        assert!(
            err.to_string()
                .contains("Could not find nix-config repository")
        );
    }
}
