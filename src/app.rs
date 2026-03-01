use std::env;
use std::path::PathBuf;

use anyhow::bail;

use crate::cli::{Cli, CommandKind};
use crate::commands::context::{AppContext, GlobalFlags};
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

const NX_REPO_ROOT_ENV: &str = "NX_REPO_ROOT";

pub fn execute(cli: Cli) -> i32 {
    // Parsed for SPEC compatibility; currently does not alter behavior.
    let _verbose_compat = cli.verbose_requested();
    let global_flags = GlobalFlags { json: cli.json() };
    let style = OutputStyle::from_flags(cli.plain(), cli.unicode(), cli.minimal());
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
    let ctx = AppContext::new(repo_root, printer, config_files, global_flags);

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
    resolve_repo_root(env::var_os(NX_REPO_ROOT_ENV).map(PathBuf::from))
}

fn resolve_repo_root(env_root: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(env_path) = env_root {
        return Ok(std::fs::canonicalize(&env_path).unwrap_or(env_path));
    }
    bail!(
        "Could not resolve repository root. Set {NX_REPO_ROOT_ENV} to your config repository path."
    )
}

pub fn dirs_home() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::resolve_repo_root;
    use tempfile::TempDir;

    #[test]
    fn resolve_repo_root_uses_env_path() {
        let repo = TempDir::new().expect("temp dir should be created");
        let resolved = resolve_repo_root(Some(repo.path().to_path_buf())).expect("resolve");
        let expected = std::fs::canonicalize(repo.path()).expect("canonical path");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_repo_root_keeps_missing_env_path_unmodified() {
        let repo = TempDir::new().expect("temp dir should be created");
        let missing = repo.path().join("missing-config-root");
        let resolved = resolve_repo_root(Some(missing.clone())).expect("resolve");
        assert_eq!(resolved, missing);
    }

    #[test]
    fn resolve_repo_root_errors_without_env_var() {
        let err = resolve_repo_root(None).expect_err("must fail");
        assert!(
            err.to_string()
                .contains("Set NX_REPO_ROOT to your config repository path")
        );
    }
}
