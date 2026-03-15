#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/command_io.rs"]
mod support_command_io;
#[path = "support/invocations.rs"]
mod support_invocations;
#[path = "support/snapshot.rs"]
mod support_snapshot;
#[path = "support/stubs.rs"]
mod support_stubs;
#[path = "support/system.rs"]
mod support_system;
#[path = "support/tree.rs"]
mod support_tree;

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use support_bin::resolve_nx_bin;
use support_command_io::{ensure_test_layout, run_command_with_optional_stdin};
use support_invocations::{
    EXPECTED_CWD_REPO_ROOT, ExpectedCall, REPO_ROOT_TOKEN, assert_invocations, read_invocations,
};
use support_snapshot::snapshot_repo_files;
use support_stubs::{LOG_FILE_NAME, STUB_DIR_NAME, install_stubs, prepend_path};
use support_system::{changed_paths, fetcher_cache_path};
use support_tree::copy_tree;

const REBUILD_PREFLIGHT_ARGS: &[&str] = &[
    "-C",
    REPO_ROOT_TOKEN,
    "ls-files",
    "--others",
    "--exclude-standard",
    "--",
    "home",
    "packages",
    "system",
    "hosts",
];
const REBUILD_FLAKE_ARGS: &[&str] = &["flake", "check", REPO_ROOT_TOKEN];

const UPGRADE_COMMIT_ARGS: &[&str] = &["upgrade", "--skip-brew", "--skip-rebuild", "--no-ai"];
const UPGRADE_FAILURE_ARGS: &[&str] = &["upgrade", "--no-ai"];
const UPGRADE_DRY_RUN_SKIP_BREW_ARGS: &[&str] = &["upgrade", "--dry-run", "--skip-brew", "--no-ai"];
const UPGRADE_REBUILD_ARGS: &[&str] = &["upgrade", "--skip-brew", "--skip-commit", "--no-ai"];
const UPGRADE_REBUILD_FAILURE_ARGS: &[&str] =
    &["upgrade", "--skip-brew", "--skip-commit", "--no-ai"];
const UPGRADE_SKIP_COMMIT_ARGS: &[&str] = &[
    "upgrade",
    "--skip-brew",
    "--skip-rebuild",
    "--skip-commit",
    "--no-ai",
];
const UPGRADE_PASSTHROUGH_ARGS: &[&str] = &[
    "upgrade",
    "--skip-brew",
    "--skip-rebuild",
    "--skip-commit",
    "--no-ai",
    "--",
    "--commit-lock-file",
    "foo",
];
const UPGRADE_TOKEN_MODE_ARGS: &[&str] = &[
    "upgrade",
    "--skip-brew",
    "--skip-rebuild",
    "--skip-commit",
    "--no-ai",
];
const UPGRADE_CACHE_RETRY_ARGS: &[&str] = &[
    "upgrade",
    "--skip-brew",
    "--skip-rebuild",
    "--skip-commit",
    "--no-ai",
];
const UPGRADE_BREW_ARGS: &[&str] = &["upgrade", "--skip-rebuild", "--skip-commit", "--no-ai"];
const UPGRADE_DRY_RUN_BREW_ARGS: &[&str] = &[
    "upgrade",
    "--dry-run",
    "--skip-rebuild",
    "--skip-commit",
    "--no-ai",
];
const GH_AUTH_TOKEN_ARGS: &[&str] = &["auth", "token"];
const GH_NIXPKGS_COMPARE_ARGS: &[&str] = &["api", "repos/NixOS/nixpkgs/compare/aaaaaaa...bbbbbbb"];
const UPGRADE_TOKEN_OPTION: &str = "github.com=ghp_system_matrix_token";

const UPGRADE_FLAKE_LOCK_OLD: &str = r#"{
  "nodes": {
    "root": {
      "inputs": {
        "nixpkgs": "nixpkgs"
      }
    },
    "nixpkgs": {
      "locked": {
        "lastModified": 1700000000,
        "owner": "NixOS",
        "repo": "nixpkgs",
        "rev": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "type": "github"
      }
    }
  }
}
"#;

const UPGRADE_FLAKE_LOCK_NEW: &str = r#"{
  "nodes": {
    "root": {
      "inputs": {
        "nixpkgs": "nixpkgs"
      }
    },
    "nixpkgs": {
      "locked": {
        "lastModified": 1700000001,
        "owner": "NixOS",
        "repo": "nixpkgs",
        "rev": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "type": "github"
      }
    }
  }
}
"#;

#[derive(Debug, Clone, Copy)]
struct UpgradeCase {
    id: &'static str,
    cli_args: &'static [&'static str],
    mode: &'static str,
    expected_exit: i32,
    expected_calls: &'static [ExpectedCall],
    stdout_contains: &'static [&'static str],
}

const UPGRADE_COMMIT_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_NIXPKGS_COMPARE_ARGS),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &["-C", REPO_ROOT_TOKEN, "add", "flake.lock"],
    ),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "-C",
            REPO_ROOT_TOKEN,
            "commit",
            "-m",
            "Update flake (nixpkgs)",
        ],
    ),
];

const UPGRADE_SKIP_COMMIT_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_NIXPKGS_COMPARE_ARGS),
];

const UPGRADE_FAILURE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
];

const UPGRADE_PASSTHROUGH_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new(
        "nix",
        EXPECTED_CWD_REPO_ROOT,
        &["flake", "update", "--commit-lock-file", "foo"],
    ),
];

const UPGRADE_TOKEN_MODE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new(
        "nix",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "flake",
            "update",
            "--option",
            "access-tokens",
            UPGRADE_TOKEN_OPTION,
        ],
    ),
];

const UPGRADE_CACHE_RETRY_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
];

const UPGRADE_NO_CHANGE_NO_COMMIT_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
];

const UPGRADE_BREW_NO_UPDATES_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
    ExpectedCall::new("brew", EXPECTED_CWD_REPO_ROOT, &["outdated", "--json"]),
];

const UPGRADE_REBUILD_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "/run/current-system/sw/bin/darwin-rebuild",
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &["switch", "--flake", REPO_ROOT_TOKEN],
    ),
];

const UPGRADE_REBUILD_FAILURE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "/run/current-system/sw/bin/darwin-rebuild",
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &["switch", "--flake", REPO_ROOT_TOKEN],
    ),
];

const UPGRADE_BREW_WITH_UPDATES_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, &["flake", "update"]),
    ExpectedCall::new("brew", EXPECTED_CWD_REPO_ROOT, &["outdated", "--json"]),
    ExpectedCall::new(
        "brew",
        EXPECTED_CWD_REPO_ROOT,
        &["info", "--json=v2", "ripgrep"],
    ),
    ExpectedCall::new("brew", EXPECTED_CWD_REPO_ROOT, &["upgrade", "ripgrep"]),
];

const UPGRADE_DRY_RUN_BREW_WITH_UPDATES_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("brew", EXPECTED_CWD_REPO_ROOT, &["outdated", "--json"]),
    ExpectedCall::new(
        "brew",
        EXPECTED_CWD_REPO_ROOT,
        &["info", "--json=v2", "ripgrep"],
    ),
];

const UPGRADE_CASES: &[UpgradeCase] = &[
    UpgradeCase {
        id: "upgrade_flake_failure_short_circuit",
        cli_args: UPGRADE_FAILURE_ARGS,
        mode: "update_fail",
        expected_exit: 1,
        expected_calls: UPGRADE_FAILURE_CALLS,
        stdout_contains: &[],
    },
    UpgradeCase {
        id: "upgrade_dry_run_skip_brew_short_circuit",
        cli_args: UPGRADE_DRY_RUN_SKIP_BREW_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: &[],
        stdout_contains: &[
            "Dry Run (no changes will be made)",
            "Dry run complete - no changes made",
        ],
    },
    UpgradeCase {
        id: "upgrade_runs_rebuild_when_not_skipped",
        cli_args: UPGRADE_REBUILD_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UPGRADE_REBUILD_CALLS,
        stdout_contains: &[],
    },
    UpgradeCase {
        id: "upgrade_rebuild_failure_exits_nonzero",
        cli_args: UPGRADE_REBUILD_FAILURE_ARGS,
        mode: "darwin_rebuild_fail",
        expected_exit: 1,
        expected_calls: UPGRADE_REBUILD_FAILURE_CALLS,
        stdout_contains: &[],
    },
    UpgradeCase {
        id: "upgrade_flake_changed_commits_lockfile",
        cli_args: UPGRADE_COMMIT_ARGS,
        mode: "upgrade_flake_changed",
        expected_exit: 0,
        expected_calls: UPGRADE_COMMIT_CALLS,
        stdout_contains: &["Committed: Update flake (nixpkgs)"],
    },
    UpgradeCase {
        id: "upgrade_flake_changed_skip_commit_gate",
        cli_args: UPGRADE_SKIP_COMMIT_ARGS,
        mode: "upgrade_flake_changed",
        expected_exit: 0,
        expected_calls: UPGRADE_SKIP_COMMIT_CALLS,
        stdout_contains: &[],
    },
    UpgradeCase {
        id: "upgrade_no_flake_changes_skips_commit",
        cli_args: UPGRADE_COMMIT_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UPGRADE_NO_CHANGE_NO_COMMIT_CALLS,
        stdout_contains: &["All flake inputs up to date"],
    },
    UpgradeCase {
        id: "upgrade_passthrough_flake_update_args",
        cli_args: UPGRADE_PASSTHROUGH_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UPGRADE_PASSTHROUGH_CALLS,
        stdout_contains: &[],
    },
    UpgradeCase {
        id: "upgrade_flake_update_injects_access_token_option",
        cli_args: UPGRADE_TOKEN_MODE_ARGS,
        mode: "upgrade_with_token",
        expected_exit: 0,
        expected_calls: UPGRADE_TOKEN_MODE_CALLS,
        stdout_contains: &[],
    },
    UpgradeCase {
        id: "upgrade_flake_update_cache_corruption_retries_once",
        cli_args: UPGRADE_CACHE_RETRY_ARGS,
        mode: "upgrade_cache_corruption",
        expected_exit: 0,
        expected_calls: UPGRADE_CACHE_RETRY_CALLS,
        stdout_contains: &[
            "Nix cache corruption detected, clearing cache and retrying",
            "Retrying flake update",
        ],
    },
    UpgradeCase {
        id: "upgrade_brew_no_updates_short_circuit",
        cli_args: UPGRADE_BREW_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UPGRADE_BREW_NO_UPDATES_CALLS,
        stdout_contains: &["All Homebrew packages up to date"],
    },
    UpgradeCase {
        id: "upgrade_brew_with_updates_runs_upgrade",
        cli_args: UPGRADE_BREW_ARGS,
        mode: "upgrade_brew_outdated",
        expected_exit: 0,
        expected_calls: UPGRADE_BREW_WITH_UPDATES_CALLS,
        stdout_contains: &["Homebrew Outdated (1)", "Homebrew packages upgraded"],
    },
    UpgradeCase {
        id: "upgrade_brew_with_updates_dry_run_skips_upgrade",
        cli_args: UPGRADE_DRY_RUN_BREW_ARGS,
        mode: "upgrade_brew_outdated",
        expected_exit: 0,
        expected_calls: UPGRADE_DRY_RUN_BREW_WITH_UPDATES_CALLS,
        stdout_contains: &["Homebrew Outdated (1)"],
    },
];

#[test]
fn system_upgrade_flows() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    for case in UPGRADE_CASES {
        run_case(&nx_bin, &repo_base, case)?;
    }

    Ok(())
}

fn run_case(nx_bin: &Path, repo_base: &Path, case: &UpgradeCase) -> Result<(), Box<dyn Error>> {
    let repo_root = TempDir::new()?;
    copy_tree(repo_base, repo_root.path())?;
    ensure_test_layout(repo_root.path())?;
    seed_flake_lock_if_needed(repo_root.path(), case.mode)?;

    let stub_dir = repo_root.path().join(STUB_DIR_NAME);
    fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;

    let log_path = repo_root.path().join(LOG_FILE_NAME);
    let before = snapshot_repo_files(repo_root.path(), &should_ignore_snapshot_path)?;

    let home_dir = TempDir::new()?;
    seed_home_state_if_needed(home_dir.path(), case.mode)?;
    let mut command = Command::new(nx_bin);
    command
        .args(["--plain", "--minimal"])
        .args(case.cli_args)
        .current_dir(repo_root.path())
        .env("NX_REPO_ROOT", repo_root.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("NX_SYSTEM_IT_MODE", case.mode)
        .env("NX_SYSTEM_IT_UPGRADE_NEW_LOCK", UPGRADE_FLAKE_LOCK_NEW)
        .env(
            "NX_SYSTEM_IT_DARWIN_REBUILD",
            stub_dir.join("darwin-rebuild"),
        )
        .env("PATH", prepend_path(&stub_dir));

    let output = run_command_with_optional_stdin(&mut command, None)?;
    let after = snapshot_repo_files(repo_root.path(), &should_ignore_snapshot_path)?;
    let invocations = read_invocations(&log_path)?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        exit_code, case.expected_exit,
        "case {}: unexpected exit code\nstdout:\n{}\nstderr:\n{}",
        case.id, stdout, stderr
    );

    assert_invocations(case.id, repo_root.path(), &invocations, case.expected_calls);
    for expected in case.stdout_contains {
        assert!(
            stdout.contains(expected),
            "case {}: stdout missing expected fragment '{}'\nstdout:\n{}\nstderr:\n{}",
            case.id,
            expected,
            stdout,
            stderr
        );
    }

    assert_repo_state(case, &before, &after, &stdout, &stderr);
    assert_home_state(case, home_dir.path(), &stdout, &stderr);

    Ok(())
}

fn seed_flake_lock_if_needed(repo_root: &Path, mode: &str) -> Result<(), Box<dyn Error>> {
    if mode == "upgrade_flake_changed" {
        fs::write(repo_root.join("flake.lock"), UPGRADE_FLAKE_LOCK_OLD)?;
    }
    Ok(())
}

fn seed_home_state_if_needed(home_dir: &Path, mode: &str) -> Result<(), Box<dyn Error>> {
    if mode == "upgrade_cache_corruption" {
        let cache_path = fetcher_cache_path(home_dir);
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(cache_path, "cache placeholder\n")?;
    }
    Ok(())
}

fn assert_repo_state(
    case: &UpgradeCase,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    stdout: &str,
    stderr: &str,
) {
    let expected_paths = expected_mutated_paths(case.mode);
    if expected_paths.is_empty() {
        assert_eq!(
            before, after,
            "case {} mutated repository files\nstdout:\n{}\nstderr:\n{}",
            case.id, stdout, stderr
        );
        return;
    }

    let actual_paths = changed_paths(before, after);
    let expected = expected_paths
        .iter()
        .map(|path| (*path).to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        actual_paths, expected,
        "case {} mutated unexpected repository files\nstdout:\n{}\nstderr:\n{}",
        case.id, stdout, stderr
    );
}

fn expected_mutated_paths(mode: &str) -> &'static [&'static str] {
    match mode {
        "upgrade_flake_changed" => &["flake.lock"],
        _ => &[],
    }
}

fn assert_home_state(case: &UpgradeCase, home_dir: &Path, stdout: &str, stderr: &str) {
    if case.id != "upgrade_flake_update_cache_corruption_retries_once" {
        return;
    }

    let cache_path = fetcher_cache_path(home_dir);
    assert!(
        !cache_path.exists(),
        "case {} did not clear fetcher cache at {}\nstdout:\n{}\nstderr:\n{}",
        case.id,
        cache_path.display(),
        stdout,
        stderr
    );
}

fn should_ignore_snapshot_path(rel_path: &str) -> bool {
    rel_path == LOG_FILE_NAME || rel_path == STUB_DIR_NAME || rel_path.starts_with(".system-stubs/")
}
