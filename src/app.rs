use std::env;
use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::cli::{Cli, CommandKind};
use crate::commands::completion::cmd_completion;
use crate::commands::context::{AppContext, HostContext};
use crate::commands::doctor::cmd_doctor;
use crate::commands::help::cmd_help;
use crate::commands::host::{cmd_clean_caches, cmd_generations};
use crate::commands::init::cmd_init;
use crate::commands::install::cmd_install;
use crate::commands::meta::cmd_version;
use crate::commands::profile::cmd_profile;
use crate::commands::query::{cmd_info, cmd_installed, cmd_list, cmd_status, cmd_where};
use crate::commands::remove::cmd_remove;
use crate::commands::search::cmd_search;
use crate::commands::secret::cmd_secret;
use crate::commands::system::{cmd_lint, cmd_rebuild, cmd_test, cmd_undo, cmd_update, cmd_upgrade};
use crate::domain::config::ConfigFiles;
use crate::domain::drift::ManifestHealth;
use crate::domain::manifest_scan::scan_repo;
use crate::infra::self_refresh::maybe_refresh_before_system_command;
use crate::output::printer::Printer;
use crate::output::style::OutputStyle;

const NX_REPO_ROOT_ENV: &str = "NX_REPO_ROOT";

pub fn execute(cli: Cli) -> i32 {
    let style = OutputStyle::from_flags(cli.plain(), cli.unicode(), cli.minimal());
    let printer = Printer::new(style);

    if let CommandKind::Version(args) = &cli.command {
        return cmd_version(args);
    }
    if let CommandKind::Help(args) = &cli.command {
        return cmd_help(args);
    }
    if let CommandKind::Completion(args) = &cli.command {
        return cmd_completion(args);
    }
    if let Some(args) = host_generations_args(&cli.command) {
        return cmd_generations(args, &HostContext::new(&printer));
    }
    if let CommandKind::CleanCaches(args) = &cli.command {
        return cmd_clean_caches(args, &HostContext::new(&printer));
    }
    if let CommandKind::Profile(args) = &cli.command {
        return cmd_profile(args);
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

    let scanned_repo = scan_repo(&repo_root);
    let manifest_health = ManifestHealth::from_scan(&scanned_repo, &repo_root);
    let config_files = config_files_for_manifest_health(&repo_root, &manifest_health);
    let ctx = AppContext::new(
        repo_root,
        printer,
        config_files,
        manifest_health,
        scanned_repo,
    );

    match cli.command {
        CommandKind::Version(_) => unreachable!("version handled before repo setup"),
        CommandKind::Help(_) => unreachable!("help handled before repo setup"),
        CommandKind::Completion(_) => unreachable!("completion handled before repo setup"),
        CommandKind::Generations(_) | CommandKind::CleanCaches(_) => {
            unreachable!("host commands handled before repo setup")
        }
        CommandKind::Doctor(args) => cmd_doctor(&args, &ctx),
        CommandKind::Init(args) => cmd_init(args.refresh, &ctx.init_context()),
        CommandKind::Install(args) => cmd_install(&args, &ctx),
        CommandKind::Remove(args) => cmd_remove(&args, &ctx),
        CommandKind::Secret(args) => cmd_secret(&args, &ctx),
        CommandKind::Search(args) => cmd_search(&args, &ctx.query_context()),
        CommandKind::Where(args) => cmd_where(&args, &ctx.query_context()),
        CommandKind::List(args) => cmd_list(&args, &ctx.query_context()),
        CommandKind::Info(args) => cmd_info(&args, &ctx.query_context()),
        CommandKind::Status(args) => cmd_status(&args, &ctx.query_context()),
        CommandKind::Installed(args) => cmd_installed(&args, &ctx.query_context()),
        CommandKind::Profile(_) => unreachable!("profile handled before repo setup"),
        CommandKind::Lint(args) => cmd_lint(&args, &ctx.system_context()),
        CommandKind::Undo(args) => cmd_undo(&args, &ctx.repo_context()),
        CommandKind::Update(args) => cmd_update(&args, &ctx.repo_context()),
        CommandKind::Test(_) => cmd_test(&ctx.repo_context()),
        CommandKind::Rebuild(args) => cmd_rebuild(&args, &ctx.system_context()),
        CommandKind::Upgrade(args) => cmd_upgrade(&args, &ctx),
    }
}

fn find_repo_root() -> anyhow::Result<PathBuf> {
    resolve_repo_root(
        env::var_os(NX_REPO_ROOT_ENV).map(PathBuf::from),
        env::current_dir().ok(),
    )
}

fn host_generations_args(command: &CommandKind) -> Option<&crate::cli::GenerationsArgs> {
    match command {
        CommandKind::Generations(args) => Some(args),
        _ => None,
    }
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

fn config_files_for_manifest_health(
    repo_root: &Path,
    manifest_health: &ManifestHealth,
) -> ConfigFiles {
    manifest_health.routing_manifest().map_or_else(
        || ConfigFiles::discover(repo_root),
        |manifest| ConfigFiles::from_manifest(manifest, repo_root),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        config_files_for_manifest_health, detect_repo_root, host_generations_args,
        resolve_repo_root,
    };
    use crate::cli::Cli;
    use crate::domain::drift::{DriftReport, ManifestHealth};
    use crate::domain::manifest::{Manifest, PlatformConfig, PlatformKind, Slot, SlotKind};
    use clap::Parser;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
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

    #[test]
    fn generations_status_is_routed_as_host_command() {
        let cli = Cli::try_parse_from(["nx", "generations", "status"]).expect("parse");
        assert!(host_generations_args(&cli.command).is_some());
    }

    #[test]
    fn generations_prune_dry_run_is_routed_as_host_command() {
        let cli = Cli::try_parse_from(["nx", "generations", "prune", "--dry-run"]).expect("parse");
        assert!(host_generations_args(&cli.command).is_some());
    }

    #[test]
    fn missing_manifest_uses_discovery_routing() {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path();
        fs::create_dir_all(root.join("packages/homebrew")).expect("create packages dir");
        fs::write(
            root.join("packages/homebrew/custom-taps.nix"),
            "# nx: taps manifest custom\n[]\n",
        )
        .expect("write taps file");

        let config = config_files_for_manifest_health(root, &ManifestHealth::Missing);

        assert!(config.manifest().is_none());
        assert_eq!(
            config.homebrew_taps(),
            root.join("packages/homebrew/custom-taps.nix")
        );
    }

    #[test]
    fn invalid_manifest_uses_discovery_routing() {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path();
        fs::create_dir_all(root.join("packages/homebrew")).expect("create packages dir");
        fs::write(
            root.join("packages/homebrew/custom-taps.nix"),
            "# nx: taps manifest custom\n[]\n",
        )
        .expect("write taps file");

        let config = config_files_for_manifest_health(
            root,
            &ManifestHealth::Invalid {
                error: "broken toml".to_string(),
            },
        );

        assert!(config.manifest().is_none());
        assert_eq!(
            config.homebrew_taps(),
            root.join("packages/homebrew/custom-taps.nix")
        );
    }

    #[test]
    fn drifted_manifest_uses_effective_manifest_routing() {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path();
        let effective_manifest = Manifest {
            schema_version: 1,
            platform: PlatformConfig {
                kind: PlatformKind::Darwin,
                rebuild_command: "darwin-rebuild".to_string(),
                sudo: true,
                flake_root: ".".to_string(),
                split_rebuild: false,
            },
            slots: vec![Slot {
                kind: SlotKind::NixPackages,
                file: PathBuf::from("modules/packages.nix"),
                attr_path: "home.packages".to_string(),
                tags: vec![],
                runtime: None,
                default_for: Some(vec!["install".to_string()]),
            }],
            aliases: HashMap::new(),
            overlays: HashMap::new(),
        };

        let config = config_files_for_manifest_health(
            root,
            &ManifestHealth::Drifted {
                effective_manifest,
                report: DriftReport::default(),
            },
        );

        assert!(config.manifest().is_some());
        assert_eq!(config.packages(), root.join("modules/packages.nix"));
    }
}
