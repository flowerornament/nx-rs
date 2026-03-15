use std::env;
use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::cli::{Cli, CommandKind};
use crate::commands::context::{AppContext, GlobalFlags};
use crate::commands::help::cmd_help;
use crate::commands::init::cmd_init;
use crate::commands::install::cmd_install;
use crate::commands::query::{cmd_info, cmd_installed, cmd_list, cmd_status, cmd_where};
use crate::commands::remove::cmd_remove;
use crate::commands::search::cmd_search;
use crate::commands::secret::cmd_secret;
use crate::commands::system::{cmd_rebuild, cmd_test, cmd_undo, cmd_update, cmd_upgrade};
use crate::domain::config::ConfigFiles;
use crate::domain::drift::ManifestHealth;
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

    if let CommandKind::Help(args) = &cli.command {
        return cmd_help(args);
    }

    let needs_refresh = matches!(
        &cli.command,
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

    let manifest_health = ManifestHealth::load(&repo_root);
    let config_files = manifest_health.manifest().map_or_else(
        || ConfigFiles::discover(&repo_root),
        |manifest| ConfigFiles::from_manifest(manifest, &repo_root),
    );
    let ctx = AppContext::new(
        repo_root,
        printer,
        config_files,
        manifest_health,
        global_flags,
    );

    match cli.command {
        CommandKind::Help(_) => unreachable!("help handled before repo setup"),
        CommandKind::Init(args) => cmd_init(args.refresh, &ctx),
        CommandKind::Install(args) => cmd_install(&args, &ctx),
        CommandKind::Remove(args) => cmd_remove(&args, &ctx),
        CommandKind::Secret(args) => cmd_secret(&args, &ctx),
        CommandKind::Search(args) => cmd_search(&args, &ctx),
        CommandKind::Where(args) => cmd_where(&args, &ctx),
        CommandKind::List(args) => cmd_list(&args, &ctx),
        CommandKind::Info(args) => cmd_info(&args, &ctx),
        CommandKind::Status => cmd_status(&ctx),
        CommandKind::Installed(args) => cmd_installed(&args, &ctx),
        CommandKind::Undo(args) => cmd_undo(&args, &ctx),
        CommandKind::Update(args) => cmd_update(&args, &ctx),
        CommandKind::Test => cmd_test(&ctx),
        CommandKind::Rebuild(args) => cmd_rebuild(&args, &ctx),
        CommandKind::Upgrade(args) => cmd_upgrade(&args, &ctx),
    }
}

fn find_repo_root() -> anyhow::Result<PathBuf> {
    resolve_repo_root(
        env::var_os(NX_REPO_ROOT_ENV).map(PathBuf::from),
        env::current_dir().ok(),
    )
}

fn resolve_repo_root(env_root: Option<PathBuf>, cwd: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(env_path) = env_root {
        return Ok(std::fs::canonicalize(&env_path).unwrap_or(env_path));
    }
    if let Some(detected) = cwd.and_then(|d| detect_repo_root(&d)) {
        return Ok(detected);
    }
    bail!(
        "Could not resolve repository root. Set {NX_REPO_ROOT_ENV} or run from inside a directory containing flake.nix."
    )
}

/// Walk up from `start` looking for a directory containing `flake.nix`.
fn detect_repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("flake.nix").is_file())
        .map(Path::to_path_buf)
}

pub fn dirs_home() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::{detect_repo_root, resolve_repo_root};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn resolve_repo_root_uses_env_path() {
        let repo = TempDir::new().expect("temp dir should be created");
        let resolved = resolve_repo_root(Some(repo.path().to_path_buf()), None).expect("resolve");
        let expected = fs::canonicalize(repo.path()).expect("canonical path");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_repo_root_keeps_missing_env_path_unmodified() {
        let repo = TempDir::new().expect("temp dir should be created");
        let missing = repo.path().join("missing-config-root");
        let resolved = resolve_repo_root(Some(missing.clone()), None).expect("resolve");
        assert_eq!(resolved, missing);
    }

    #[test]
    fn resolve_repo_root_errors_without_env_var() {
        let empty = TempDir::new().expect("temp dir");
        let err = resolve_repo_root(None, Some(empty.path().to_path_buf())).expect_err("must fail");
        assert!(
            err.to_string()
                .contains("Set NX_REPO_ROOT or run from inside a directory containing flake.nix")
        );
    }

    #[test]
    fn detect_finds_flake_in_dir() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("flake.nix"), "{}").expect("write flake.nix");
        assert_eq!(detect_repo_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn detect_walks_up_to_parent() {
        let parent = TempDir::new().expect("temp dir");
        fs::write(parent.path().join("flake.nix"), "{}").expect("write flake.nix");
        let child = parent.path().join("subdir");
        fs::create_dir(&child).expect("create subdir");
        assert_eq!(detect_repo_root(&child), Some(parent.path().to_path_buf()));
    }

    #[test]
    fn detect_returns_none_without_flake() {
        let dir = TempDir::new().expect("temp dir");
        // Temp dirs are typically under /tmp which has no flake.nix ancestors
        assert_eq!(detect_repo_root(dir.path()), None);
    }
}
