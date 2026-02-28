mod support;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

const LOG_FILE_NAME: &str = ".system-command-log.tsv";
const STUB_DIR_NAME: &str = ".system-stubs";
const REPO_ROOT_TOKEN: &str = "<REPO_ROOT>";

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
const TEST_RUFF_ARGS: &[&str] = &["check", "."];
const TEST_MYPY_ARGS: &[&str] = &["."];
const TEST_UNITTEST_ARGS: &[&str] = &["-m", "unittest", "discover", "-s", "scripts/nx/tests"];

const UPDATE_PASSTHROUGH_ARGS: &[&str] = &["update", "--", "--commit-lock-file", "foo"];
const UPDATE_BASE_ARGS: &[&str] = &["update"];
const TEST_BASE_ARGS: &[&str] = &["test"];
const REBUILD_PASSTHROUGH_ARGS: &[&str] = &["rebuild", "--", "--show-trace", "foo"];
const REBUILD_BASE_ARGS: &[&str] = &["rebuild"];
const UNDO_BASE_ARGS: &[&str] = &["undo"];
const INSTALL_MISSING_ARGS: &[&str] = &["install"];
const REMOVE_MISSING_ARGS: &[&str] = &["remove"];
const WHERE_MISSING_ARGS: &[&str] = &["where"];
const INFO_MISSING_ARGS: &[&str] = &["info"];
const INSTALLED_MISSING_ARGS: &[&str] = &["installed"];
const INFO_FOUND_ARGS: &[&str] = &["info", "ripgrep"];
const INFO_JSON_FOUND_ARGS: &[&str] = &["info", "ripgrep", "--json"];
const INFO_BLEEDING_EDGE_ARGS: &[&str] = &["info", "ripgrep", "--bleeding-edge"];
const INFO_JSON_HM_MODULE_ARGS: &[&str] = &["info", "git", "--json"];
const INFO_JSON_DARWIN_SERVICE_ARGS: &[&str] = &["info", "yabai", "--json"];
const UPGRADE_COMMIT_ARGS: &[&str] = &["upgrade", "--skip-brew", "--skip-rebuild", "--no-ai"];
const UPGRADE_FAILURE_ARGS: &[&str] = &["upgrade", "--no-ai"];
const UPGRADE_DRY_RUN_SKIP_BREW_ARGS: &[&str] = &["upgrade", "--dry-run", "--skip-brew", "--no-ai"];
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
const GH_AUTH_TOKEN_ARGS: &[&str] = &["auth", "token"];
const GH_NIXPKGS_COMPARE_ARGS: &[&str] = &["api", "repos/NixOS/nixpkgs/compare/aaaaaaa...bbbbbbb"];
const UPGRADE_TOKEN_OPTION: &str = "github.com=ghp_system_matrix_token";

const INFO_FOUND_STDOUT: &[&str] = &[
    "ripgrep (installed (nxs))",
    "Location: packages/nix/cli.nix:5",
];
const INFO_JSON_FOUND_STDOUT: &[&str] = &[
    "\"name\": \"ripgrep\"",
    "\"installed\": true",
    "\"sources\": []",
];
const INFO_JSON_HM_MODULE_STDOUT: &[&str] = &[
    "\"name\": \"git\"",
    "\"hm_module\": {",
    "\"path\": \"programs.git\"",
    "\"enabled\": false",
];
const INFO_JSON_DARWIN_SERVICE_STDOUT: &[&str] = &[
    "\"name\": \"yabai\"",
    "\"darwin_service\": {",
    "\"path\": \"services.yabai\"",
    "\"enabled\": false",
];

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
enum StubMode {
    Success,
    UpdateFail,
    RuffFail,
    MypyFail,
    UnittestFail,
    FlakeCheckFail,
    GitPreflightFail,
    PreflightUntracked,
    DarwinRebuildFail,
    UpgradeFlakeChanged,
    UpgradeWithToken,
    UpgradeCacheCorruption,
    UndoDirty,
}

impl StubMode {
    const fn as_env(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::UpdateFail => "update_fail",
            Self::RuffFail => "ruff_fail",
            Self::MypyFail => "mypy_fail",
            Self::UnittestFail => "unittest_fail",
            Self::FlakeCheckFail => "flake_check_fail",
            Self::GitPreflightFail => "git_preflight_fail",
            Self::PreflightUntracked => "preflight_untracked",
            Self::DarwinRebuildFail => "darwin_rebuild_fail",
            Self::UpgradeFlakeChanged => "upgrade_flake_changed",
            Self::UpgradeWithToken => "upgrade_with_token",
            Self::UpgradeCacheCorruption => "upgrade_cache_corruption",
            Self::UndoDirty => "undo_dirty",
        }
    }

    const fn expected_mutated_paths(self) -> &'static [&'static str] {
        match self {
            Self::UpgradeFlakeChanged => &["flake.lock"],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ExpectedCwd {
    RepoRoot,
    ScriptsNx,
}

#[derive(Debug, Clone, Copy)]
struct ExpectedCall {
    program: &'static str,
    cwd: ExpectedCwd,
    args: &'static [&'static str],
}

impl ExpectedCall {
    const fn new(program: &'static str, cwd: ExpectedCwd, args: &'static [&'static str]) -> Self {
        Self { program, cwd, args }
    }
}

#[derive(Debug, Clone, Copy)]
struct MatrixCase {
    id: &'static str,
    cli_args: &'static [&'static str],
    mode: StubMode,
    expected_exit: i32,
    expected_calls: Option<&'static [ExpectedCall]>,
    stdout_contains: &'static [&'static str],
}

#[derive(Debug)]
struct Invocation {
    program: String,
    cwd: PathBuf,
    args: Vec<String>,
}

const UPDATE_SUCCESS_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "nix",
    ExpectedCwd::RepoRoot,
    &["flake", "update", "--commit-lock-file", "foo"],
)];

const UPDATE_FAILURE_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "nix",
    ExpectedCwd::RepoRoot,
    &["flake", "update"],
)];

const TEST_SUCCESS_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("ruff", ExpectedCwd::ScriptsNx, TEST_RUFF_ARGS),
    ExpectedCall::new("mypy", ExpectedCwd::ScriptsNx, TEST_MYPY_ARGS),
    ExpectedCall::new("python3", ExpectedCwd::RepoRoot, TEST_UNITTEST_ARGS),
];

const TEST_RUFF_FAIL_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "ruff",
    ExpectedCwd::ScriptsNx,
    TEST_RUFF_ARGS,
)];

const TEST_MYPY_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("ruff", ExpectedCwd::ScriptsNx, TEST_RUFF_ARGS),
    ExpectedCall::new("mypy", ExpectedCwd::ScriptsNx, TEST_MYPY_ARGS),
];

const TEST_UNITTEST_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("ruff", ExpectedCwd::ScriptsNx, TEST_RUFF_ARGS),
    ExpectedCall::new("mypy", ExpectedCwd::ScriptsNx, TEST_MYPY_ARGS),
    ExpectedCall::new("python3", ExpectedCwd::RepoRoot, TEST_UNITTEST_ARGS),
];

const REBUILD_SUCCESS_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        ExpectedCwd::RepoRoot,
        &[
            "/run/current-system/sw/bin/darwin-rebuild",
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--show-trace",
            "foo",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        ExpectedCwd::RepoRoot,
        &["switch", "--flake", REPO_ROOT_TOKEN, "--show-trace", "foo"],
    ),
];

const REBUILD_GIT_FAIL_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "git",
    ExpectedCwd::RepoRoot,
    REBUILD_PREFLIGHT_ARGS,
)];

const REBUILD_UNTRACKED_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "git",
    ExpectedCwd::RepoRoot,
    REBUILD_PREFLIGHT_ARGS,
)];

const REBUILD_FLAKE_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, REBUILD_FLAKE_ARGS),
];

const REBUILD_DARWIN_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        ExpectedCwd::RepoRoot,
        &[
            "/run/current-system/sw/bin/darwin-rebuild",
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--show-trace",
            "foo",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        ExpectedCwd::RepoRoot,
        &["switch", "--flake", REPO_ROOT_TOKEN, "--show-trace", "foo"],
    ),
];

const UNDO_CLEAN_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "git",
    ExpectedCwd::RepoRoot,
    &["status", "--porcelain"],
)];

const UNDO_CONFIRMED_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, &["status", "--porcelain"]),
    ExpectedCall::new(
        "git",
        ExpectedCwd::RepoRoot,
        &["diff", "--stat", "packages/nix/cli.nix"],
    ),
    ExpectedCall::new(
        "git",
        ExpectedCwd::RepoRoot,
        &["checkout", "--", "packages/nix/cli.nix"],
    ),
];

const UNDO_CANCELLED_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, &["status", "--porcelain"]),
    ExpectedCall::new(
        "git",
        ExpectedCwd::RepoRoot,
        &["diff", "--stat", "packages/nix/cli.nix"],
    ),
];

const NO_CALLS: &[ExpectedCall] = &[];

const UPGRADE_COMMIT_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, &["flake", "update"]),
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_NIXPKGS_COMPARE_ARGS),
    ExpectedCall::new(
        "git",
        ExpectedCwd::RepoRoot,
        &["-C", REPO_ROOT_TOKEN, "add", "flake.lock"],
    ),
    ExpectedCall::new(
        "git",
        ExpectedCwd::RepoRoot,
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
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, &["flake", "update"]),
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_NIXPKGS_COMPARE_ARGS),
];

const UPGRADE_FAILURE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, &["flake", "update"]),
];

const UPGRADE_PASSTHROUGH_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new(
        "nix",
        ExpectedCwd::RepoRoot,
        &["flake", "update", "--commit-lock-file", "foo"],
    ),
];

const UPGRADE_TOKEN_MODE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new(
        "nix",
        ExpectedCwd::RepoRoot,
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
    ExpectedCall::new("gh", ExpectedCwd::RepoRoot, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, &["flake", "update"]),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, &["flake", "update"]),
];

const MATRIX_CASES: &[MatrixCase] = &[
    MatrixCase {
        id: "install_missing_args_parser_error",
        cli_args: INSTALL_MISSING_ARGS,
        mode: StubMode::Success,
        expected_exit: 2,
        expected_calls: Some(NO_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "remove_missing_args_parser_error",
        cli_args: REMOVE_MISSING_ARGS,
        mode: StubMode::Success,
        expected_exit: 2,
        expected_calls: Some(NO_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "where_missing_args_parser_error",
        cli_args: WHERE_MISSING_ARGS,
        mode: StubMode::Success,
        expected_exit: 2,
        expected_calls: Some(NO_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "info_missing_args_parser_error",
        cli_args: INFO_MISSING_ARGS,
        mode: StubMode::Success,
        expected_exit: 2,
        expected_calls: Some(NO_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "installed_missing_args_parser_error",
        cli_args: INSTALLED_MISSING_ARGS,
        mode: StubMode::Success,
        expected_exit: 2,
        expected_calls: Some(NO_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "undo_clean_noop",
        cli_args: UNDO_BASE_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: Some(UNDO_CLEAN_CALLS),
        stdout_contains: &["Nothing to undo."],
    },
    MatrixCase {
        id: "undo_dirty_confirmed_reverts",
        cli_args: UNDO_BASE_ARGS,
        mode: StubMode::UndoDirty,
        expected_exit: 0,
        expected_calls: Some(UNDO_CONFIRMED_CALLS),
        stdout_contains: &["Undo Changes (1 files)", "Reverted 1 files"],
    },
    MatrixCase {
        id: "undo_dirty_cancelled_short_circuit",
        cli_args: UNDO_BASE_ARGS,
        mode: StubMode::UndoDirty,
        expected_exit: 0,
        expected_calls: Some(UNDO_CANCELLED_CALLS),
        stdout_contains: &["Undo Changes (1 files)", "Cancelled."],
    },
    MatrixCase {
        id: "update_success_passthrough",
        cli_args: UPDATE_PASSTHROUGH_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: Some(UPDATE_SUCCESS_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "update_failure_exit",
        cli_args: UPDATE_BASE_ARGS,
        mode: StubMode::UpdateFail,
        expected_exit: 1,
        expected_calls: Some(UPDATE_FAILURE_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "test_success_sequence",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: Some(TEST_SUCCESS_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "test_ruff_failure_short_circuit",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::RuffFail,
        expected_exit: 1,
        expected_calls: Some(TEST_RUFF_FAIL_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "test_mypy_failure_short_circuit",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::MypyFail,
        expected_exit: 1,
        expected_calls: Some(TEST_MYPY_FAIL_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "test_unittest_failure_exit",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::UnittestFail,
        expected_exit: 1,
        expected_calls: Some(TEST_UNITTEST_FAIL_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "rebuild_success_passthrough",
        cli_args: REBUILD_PASSTHROUGH_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: Some(REBUILD_SUCCESS_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "rebuild_git_preflight_failure_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: StubMode::GitPreflightFail,
        expected_exit: 1,
        expected_calls: Some(REBUILD_GIT_FAIL_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "rebuild_untracked_nix_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: StubMode::PreflightUntracked,
        expected_exit: 1,
        expected_calls: Some(REBUILD_UNTRACKED_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "rebuild_flake_check_failure_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: StubMode::FlakeCheckFail,
        expected_exit: 1,
        expected_calls: Some(REBUILD_FLAKE_FAIL_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "rebuild_darwin_failure_exit",
        cli_args: REBUILD_PASSTHROUGH_ARGS,
        mode: StubMode::DarwinRebuildFail,
        expected_exit: 1,
        expected_calls: Some(REBUILD_DARWIN_FAIL_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "upgrade_flake_failure_short_circuit",
        cli_args: UPGRADE_FAILURE_ARGS,
        mode: StubMode::UpdateFail,
        expected_exit: 1,
        expected_calls: Some(UPGRADE_FAILURE_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "upgrade_dry_run_skip_brew_short_circuit",
        cli_args: UPGRADE_DRY_RUN_SKIP_BREW_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: Some(NO_CALLS),
        stdout_contains: &[
            "Dry Run (no changes will be made)",
            "Dry run complete - no changes made",
        ],
    },
    MatrixCase {
        id: "upgrade_flake_changed_commits_lockfile",
        cli_args: UPGRADE_COMMIT_ARGS,
        mode: StubMode::UpgradeFlakeChanged,
        expected_exit: 0,
        expected_calls: Some(UPGRADE_COMMIT_CALLS),
        stdout_contains: &["Committed: Update flake (nixpkgs)"],
    },
    MatrixCase {
        id: "upgrade_flake_changed_skip_commit_gate",
        cli_args: UPGRADE_SKIP_COMMIT_ARGS,
        mode: StubMode::UpgradeFlakeChanged,
        expected_exit: 0,
        expected_calls: Some(UPGRADE_SKIP_COMMIT_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "upgrade_passthrough_flake_update_args",
        cli_args: UPGRADE_PASSTHROUGH_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: Some(UPGRADE_PASSTHROUGH_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "upgrade_flake_update_injects_access_token_option",
        cli_args: UPGRADE_TOKEN_MODE_ARGS,
        mode: StubMode::UpgradeWithToken,
        expected_exit: 0,
        expected_calls: Some(UPGRADE_TOKEN_MODE_CALLS),
        stdout_contains: &[],
    },
    MatrixCase {
        id: "upgrade_flake_update_cache_corruption_retries_once",
        cli_args: UPGRADE_CACHE_RETRY_ARGS,
        mode: StubMode::UpgradeCacheCorruption,
        expected_exit: 0,
        expected_calls: Some(UPGRADE_CACHE_RETRY_CALLS),
        stdout_contains: &[
            "Nix cache corruption detected, clearing cache and retrying",
            "Retrying flake update",
        ],
    },
    MatrixCase {
        id: "info_found_installed_plain",
        cli_args: INFO_FOUND_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: None,
        stdout_contains: INFO_FOUND_STDOUT,
    },
    MatrixCase {
        id: "info_found_installed_json",
        cli_args: INFO_JSON_FOUND_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: None,
        stdout_contains: INFO_JSON_FOUND_STDOUT,
    },
    MatrixCase {
        id: "info_found_bleeding_edge_plain",
        cli_args: INFO_BLEEDING_EDGE_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: None,
        stdout_contains: INFO_FOUND_STDOUT,
    },
    MatrixCase {
        id: "info_json_hm_module_known_package",
        cli_args: INFO_JSON_HM_MODULE_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: None,
        stdout_contains: INFO_JSON_HM_MODULE_STDOUT,
    },
    MatrixCase {
        id: "info_json_darwin_service_known_package",
        cli_args: INFO_JSON_DARWIN_SERVICE_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: None,
        stdout_contains: INFO_JSON_DARWIN_SERVICE_STDOUT,
    },
];

#[test]
fn system_command_matrix() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/parity/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    for case in MATRIX_CASES {
        run_case(&nx_bin, &repo_base, case)?;
    }

    Ok(())
}

fn resolve_nx_bin(workspace_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = env::var_os("CARGO_BIN_EXE_nx") {
        return Ok(PathBuf::from(path));
    }

    let candidate = workspace_root.join("target/debug/nx");
    if candidate.exists() {
        return Ok(candidate);
    }

    Err(io::Error::new(io::ErrorKind::NotFound, "missing nx test binary").into())
}

fn run_case(nx_bin: &Path, repo_base: &Path, case: &MatrixCase) -> Result<(), Box<dyn Error>> {
    let repo_root = TempDir::new()?;
    support::copy_tree(repo_base, repo_root.path())?;
    ensure_test_layout(repo_root.path())?;
    seed_flake_lock_if_needed(repo_root.path(), case.mode)?;

    let stub_dir = repo_root.path().join(STUB_DIR_NAME);
    fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;

    let log_path = repo_root.path().join(LOG_FILE_NAME);
    let before = snapshot_repo_files(repo_root.path())?;

    let home_dir = TempDir::new()?;
    let mut command = Command::new(nx_bin);
    command
        .args(["--plain", "--minimal"])
        .args(case.cli_args)
        .current_dir(repo_root.path())
        .env("B2NIX_REPO_ROOT", repo_root.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("NX_SYSTEM_IT_MODE", case.mode.as_env())
        .env("NX_SYSTEM_IT_UPGRADE_NEW_LOCK", UPGRADE_FLAKE_LOCK_NEW)
        .env(
            "NX_SYSTEM_IT_DARWIN_REBUILD",
            stub_dir.join("darwin-rebuild"),
        )
        .env("PATH", prepend_path(&stub_dir));

    let output = run_command_with_optional_stdin(&mut command, case_stdin(case.id))?;
    let after = snapshot_repo_files(repo_root.path())?;
    let invocations = read_invocations(&log_path)?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        exit_code, case.expected_exit,
        "case {}: unexpected exit code\nstdout:\n{}\nstderr:\n{}",
        case.id, stdout, stderr
    );

    if let Some(expected_calls) = case.expected_calls {
        assert_invocations(case.id, repo_root.path(), &invocations, expected_calls);
    }
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

    Ok(())
}

fn case_stdin(case_id: &str) -> Option<&'static str> {
    match case_id {
        "undo_dirty_confirmed_reverts" => Some("y\n"),
        "undo_dirty_cancelled_short_circuit" => Some("n\n"),
        _ => None,
    }
}

fn run_command_with_optional_stdin(
    command: &mut Command,
    stdin: Option<&str>,
) -> Result<std::process::Output, Box<dyn Error>> {
    if let Some(input) = stdin {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(input.as_bytes())?;
        }
        return Ok(child.wait_with_output()?);
    }
    Ok(command.output()?)
}

fn prepend_path(stub_dir: &Path) -> String {
    let mut path_value = stub_dir.to_string_lossy().to_string();
    if let Some(existing) = env::var_os("PATH")
        && !existing.is_empty()
    {
        path_value.push(':');
        path_value.push_str(&existing.to_string_lossy());
    }
    path_value
}

fn assert_invocations(
    case_id: &str,
    repo_root: &Path,
    actual: &[Invocation],
    expected: &[ExpectedCall],
) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "case {case_id}: invocation count mismatch\nactual: {actual:?}"
    );

    for index in 0..expected.len() {
        let expected_call = expected[index];
        let actual_call = &actual[index];

        assert_eq!(
            actual_call.program, expected_call.program,
            "case {case_id}: unexpected program at step {index}: {actual_call:?}"
        );

        let expected_cwd = match expected_call.cwd {
            ExpectedCwd::RepoRoot => REPO_ROOT_TOKEN.to_string(),
            ExpectedCwd::ScriptsNx => format!("{REPO_ROOT_TOKEN}/scripts/nx"),
        };
        let actual_cwd = normalize_value(actual_call.cwd.to_string_lossy().as_ref(), repo_root);
        assert_eq!(
            actual_cwd, expected_cwd,
            "case {case_id}: unexpected cwd at step {index}: {actual_call:?}"
        );

        let mut actual_args = Vec::with_capacity(actual_call.args.len());
        for arg in &actual_call.args {
            actual_args.push(normalize_value(arg, repo_root));
        }

        let mut expected_args = Vec::with_capacity(expected_call.args.len());
        for arg in expected_call.args {
            expected_args.push((*arg).to_string());
        }

        assert_eq!(
            actual_args, expected_args,
            "case {case_id}: unexpected args at step {index}: {actual_call:?}"
        );
    }
}

fn read_invocations(path: &Path) -> Result<Vec<Invocation>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(path)?;
    let mut out = Vec::new();

    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let mut parts = line.split('\t');
        let program = parts.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("missing program in invocation line {}", index + 1),
            )
        })?;
        let cwd = parts.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("missing cwd in invocation line {}", index + 1),
            )
        })?;

        let mut args = Vec::new();
        for part in parts {
            args.push(part.to_string());
        }

        out.push(Invocation {
            program: program.to_string(),
            cwd: PathBuf::from(cwd),
            args,
        });
    }

    Ok(out)
}

fn ensure_test_layout(repo_root: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(repo_root.join("scripts/nx/tests"))?;
    Ok(())
}

fn seed_flake_lock_if_needed(repo_root: &Path, mode: StubMode) -> Result<(), Box<dyn Error>> {
    if matches!(mode, StubMode::UpgradeFlakeChanged) {
        fs::write(repo_root.join("flake.lock"), UPGRADE_FLAKE_LOCK_OLD)?;
    }
    Ok(())
}

fn install_stubs(stub_dir: &Path) -> Result<(), Box<dyn Error>> {
    for program in [
        "git",
        "nix",
        "gh",
        "brew",
        "sudo",
        "ruff",
        "mypy",
        "python3",
        "darwin-rebuild",
    ] {
        support::write_executable(&stub_dir.join(program), STUB_SCRIPT)?;
    }
    Ok(())
}

fn snapshot_repo_files(repo_root: &Path) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    support::snapshot_repo_files(repo_root, &|rel| should_ignore_snapshot_path(rel))
}

fn assert_repo_state(
    case: &MatrixCase,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    stdout: &str,
    stderr: &str,
) {
    let expected_paths = case.mode.expected_mutated_paths();
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

fn changed_paths(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed = BTreeSet::new();

    for (path, before_content) in before {
        match after.get(path) {
            Some(after_content) => {
                if after_content != before_content {
                    changed.insert(path.clone());
                }
            }
            None => {
                changed.insert(path.clone());
            }
        }
    }

    for path in after.keys() {
        if !before.contains_key(path) {
            changed.insert(path.clone());
        }
    }

    changed.into_iter().collect()
}

fn should_ignore_snapshot_path(rel_path: &str) -> bool {
    rel_path == LOG_FILE_NAME || rel_path == STUB_DIR_NAME || rel_path.starts_with(".system-stubs/")
}

fn normalize_value(input: &str, repo_root: &Path) -> String {
    let mut output = input.to_string();
    for candidate in repo_root_candidates(repo_root) {
        output = output.replace(candidate.as_str(), REPO_ROOT_TOKEN);
    }
    output
}

fn repo_root_candidates(repo_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    push_candidate(&mut out, repo_root.to_string_lossy().to_string());

    if let Ok(canonical) = fs::canonicalize(repo_root) {
        push_candidate(&mut out, canonical.to_string_lossy().to_string());
    }

    let mut aliases = Vec::new();
    for candidate in &out {
        if let Some(alias) = private_path_alias(candidate) {
            aliases.push(alias);
        }
    }
    for alias in aliases {
        push_candidate(&mut out, alias);
    }

    out.sort_by_key(|value| Reverse(value.len()));
    out
}

fn private_path_alias(path: &str) -> Option<String> {
    if let Some(stripped) = path.strip_prefix("/private") {
        return Some(stripped.to_string());
    }
    if path.starts_with("/var/") || path.starts_with("/tmp/") {
        return Some(format!("/private{path}"));
    }
    None
}

fn push_candidate(out: &mut Vec<String>, value: String) {
    if !value.is_empty() && !out.contains(&value) {
        out.push(value);
    }
}

const STUB_SCRIPT: &str = r#"#!/bin/sh
set -eu

program="$(basename "$0")"
log_path="${NX_SYSTEM_IT_LOG:?NX_SYSTEM_IT_LOG must be set}"
mode="${NX_SYSTEM_IT_MODE:-success}"

printf "%s\t%s" "$program" "$PWD" >> "$log_path"
for arg in "$@"; do
  printf "\t%s" "$arg" >> "$log_path"
done
printf "\n" >> "$log_path"

case "$program" in
  git)
    if [ "${1:-}" = "-C" ]; then
      shift
      shift
    fi

    if [ "${1:-}" = "ls-files" ]; then
      if [ "$mode" = "git_preflight_fail" ]; then
        echo "stub git ls-files failed" >&2
        exit 1
      fi
      if [ "$mode" = "preflight_untracked" ]; then
        echo "home/untracked-from-stub.nix"
        exit 0
      fi
      exit 0
    fi

    if [ "${1:-}" = "rev-parse" ] && [ "${2:-}" = "--show-toplevel" ]; then
      pwd
      exit 0
    fi

    if [ "${1:-}" = "status" ] && [ "${2:-}" = "--porcelain" ]; then
      if [ "$mode" = "undo_dirty" ]; then
        echo " M packages/nix/cli.nix"
      fi
      exit 0
    fi

    if [ "${1:-}" = "diff" ] && [ "${2:-}" = "--stat" ]; then
      if [ "$mode" = "undo_dirty" ]; then
        echo " 1 file changed, 1 insertion(+)"
      fi
      exit 0
    fi

    if [ "${1:-}" = "checkout" ] && [ "${2:-}" = "--" ]; then
      exit 0
    fi

    exit 0
    ;;
  nix)
    if [ "${1:-}" = "flake" ] && [ "${2:-}" = "update" ]; then
      if [ "$mode" = "update_fail" ]; then
        echo "stub nix flake update failed"
        exit 1
      fi
      if [ "$mode" = "upgrade_cache_corruption" ]; then
        marker="${HOME}/.nx-system-it-cache-corruption-once"
        if [ ! -f "$marker" ]; then
          : > "$marker"
          echo "error: failed to insert entry: invalid object specified"
          exit 1
        fi
      fi
      if [ "$mode" = "upgrade_flake_changed" ]; then
        printf '%s' "${NX_SYSTEM_IT_UPGRADE_NEW_LOCK:?NX_SYSTEM_IT_UPGRADE_NEW_LOCK must be set}" > flake.lock
      fi
      echo "stub nix flake update ok"
      exit 0
    fi

    if [ "${1:-}" = "flake" ] && [ "${2:-}" = "check" ]; then
      if [ "$mode" = "flake_check_fail" ]; then
        echo "stub nix flake check failed" >&2
        exit 1
      fi
      echo "stub nix flake check ok"
      exit 0
    fi

    echo "stub nix unsupported: $*" >&2
    exit 1
    ;;
  gh)
    if [ "${1:-}" = "auth" ] && [ "${2:-}" = "token" ]; then
      if [ "$mode" = "upgrade_with_token" ]; then
        echo "ghp_system_matrix_token"
        exit 0
      fi
      exit 1
    fi
    echo "stub gh unsupported: $*" >&2
    exit 1
    ;;
  brew)
    if [ "${1:-}" = "info" ] && [ "${2:-}" = "--json=v2" ]; then
      if [ "${3:-}" = "--cask" ]; then
        echo '{"casks":[]}'
        exit 0
      fi
      echo '{"formulae":[]}'
      exit 0
    fi
    echo "stub brew unsupported: $*" >&2
    exit 1
    ;;
  sudo)
    if [ "$mode" = "sudo_fail" ]; then
      echo "stub sudo failed" >&2
      exit 1
    fi

    # Handle bash -lc wrapper (ulimit + exec darwin-rebuild)
    if [ "${1:-}" = "bash" ] && [ "${2:-}" = "-lc" ]; then
      cmd="${3:-}"
      cmd="$(printf '%s' "$cmd" | sed "s|/run/current-system/sw/bin/darwin-rebuild|${NX_SYSTEM_IT_DARWIN_REBUILD:?}|g")"
      bash -c "$cmd"
      exit $?
    fi

    if [ "${1:-}" = "/run/current-system/sw/bin/darwin-rebuild" ]; then
      shift
      "${NX_SYSTEM_IT_DARWIN_REBUILD:?NX_SYSTEM_IT_DARWIN_REBUILD must be set}" "$@"
      exit $?
    fi

    echo "stub sudo $*"
    exit 0
    ;;
  ruff)
    if [ "$mode" = "ruff_fail" ]; then
      echo "stub ruff failed" >&2
      exit 1
    fi
    echo "stub ruff ok"
    exit 0
    ;;
  mypy)
    if [ "$mode" = "mypy_fail" ]; then
      echo "stub mypy failed" >&2
      exit 1
    fi
    echo "stub mypy ok"
    exit 0
    ;;
  python3)
    if [ "$mode" = "unittest_fail" ]; then
      echo "stub unittest failed" >&2
      exit 1
    fi
    echo "stub unittest ok"
    exit 0
    ;;
  darwin-rebuild)
    if [ "$mode" = "darwin_rebuild_fail" ]; then
      echo "stub darwin-rebuild failed" >&2
      exit 1
    fi
    echo "stub darwin-rebuild ok"
    exit 0
    ;;
  *)
    echo "unsupported stub program: $program" >&2
    exit 99
    ;;
esac
"#;
