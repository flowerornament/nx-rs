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
#[path = "support/tree.rs"]
mod support_tree;

use std::env;
use std::error::Error;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

use support_bin::resolve_nx_bin;
use support_command_io::{ensure_test_layout, run_command_with_optional_stdin};
use support_invocations::{
    EXPECTED_CWD_REPO_ROOT, ExpectedCall, REPO_ROOT_TOKEN, assert_invocations, read_invocations,
};
use support_snapshot::snapshot_repo_files;
use support_stubs::{LOG_FILE_NAME, STUB_DIR_NAME, install_stubs, prepend_path};
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
const REBUILD_TIMING_HEAD_ARGS: &[&str] = &["rev-parse", "HEAD"];
const REBUILD_FLAKE_ARGS: &[&str] = &["flake", "check", REPO_ROOT_TOKEN];
const TEST_CI_ARGS: &[&str] = &["ci"];

const UPDATE_PASSTHROUGH_ARGS: &[&str] = &["update", "--", "--commit-lock-file", "foo"];
const UPDATE_BASE_ARGS: &[&str] = &["update"];
const TEST_BASE_ARGS: &[&str] = &["test"];
const LINT_BASE_ARGS: &[&str] = &["lint"];
const REBUILD_PASSTHROUGH_ARGS: &[&str] = &["rebuild", "--", "--show-trace", "foo"];
const REBUILD_BASE_ARGS: &[&str] = &["rebuild"];
const REBUILD_CHECK_ONLY_ARGS: &[&str] = &["rebuild", "--preflight"];
const UNDO_BASE_ARGS: &[&str] = &["undo"];
const SUDO_SET_HOME_ARG: &str = "-H";
const ROOT_ENV_PROGRAM: &str = "/usr/bin/env";
const ROOT_HOME_ENV_ARG: &str = "HOME=/var/root";
const NIX_REMOTE_DAEMON_ENV_ARG: &str = "NIX_REMOTE=daemon";

const UPDATE_SUCCESS_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "nix",
    EXPECTED_CWD_REPO_ROOT,
    &["flake", "update", "--commit-lock-file", "foo"],
)];

const UPDATE_FAILURE_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "nix",
    EXPECTED_CWD_REPO_ROOT,
    &["flake", "update"],
)];

const TEST_SUCCESS_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "just",
    EXPECTED_CWD_REPO_ROOT,
    TEST_CI_ARGS,
)];

const TEST_FAILURE_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "just",
    EXPECTED_CWD_REPO_ROOT,
    TEST_CI_ARGS,
)];

const LINT_SUCCESS_CALLS: &[ExpectedCall] = &[];

const REBUILD_SUCCESS_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
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
            "--show-trace",
            "foo",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &["switch", "--flake", REPO_ROOT_TOKEN, "--show-trace", "foo"],
    ),
];

const REBUILD_GIT_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
];

const REBUILD_UNTRACKED_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
];

const REBUILD_FLAKE_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
];

const REBUILD_CHECK_ONLY_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
];

const REBUILD_DARWIN_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
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
            "--show-trace",
            "foo",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &["switch", "--flake", REPO_ROOT_TOKEN, "--show-trace", "foo"],
    ),
];

const SPLIT_REBUILD_BUILD_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "scutil",
        EXPECTED_CWD_REPO_ROOT,
        &["--get", "LocalHostName"],
    ),
    ExpectedCall::new(
        "nix",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "build",
            "--json",
            "--no-link",
            "<REPO_ROOT>#darwinConfigurations.test-host.system",
        ],
    ),
];

const SPLIT_REBUILD_SUDO_CHECK_CALL: ExpectedCall =
    ExpectedCall::new("sudo", EXPECTED_CWD_REPO_ROOT, &["-n", "true"]);

const SPLIT_REBUILD_SUDO_AUTH_CALL: ExpectedCall =
    ExpectedCall::new("sudo", EXPECTED_CWD_REPO_ROOT, &[SUDO_SET_HOME_ARG, "-v"]);

const SPLIT_REBUILD_PROFILE_SET_CALL: ExpectedCall = ExpectedCall::new(
    "sudo",
    EXPECTED_CWD_REPO_ROOT,
    &[
        SUDO_SET_HOME_ARG,
        ROOT_ENV_PROGRAM,
        ROOT_HOME_ENV_ARG,
        NIX_REMOTE_DAEMON_ENV_ARG,
        "nix-env",
        "-p",
        "/nix/var/nix/profiles/system",
        "--set",
        "/nix/store/new-system",
    ],
);

const SPLIT_REBUILD_ACTIVATE_CALL: ExpectedCall = ExpectedCall::new(
    "sudo",
    EXPECTED_CWD_REPO_ROOT,
    &[
        SUDO_SET_HOME_ARG,
        ROOT_ENV_PROGRAM,
        ROOT_HOME_ENV_ARG,
        NIX_REMOTE_DAEMON_ENV_ARG,
        "/nix/store/new-system/activate",
    ],
);

const ROOT_GIT_CACHE_CLEAR_CALL: ExpectedCall = ExpectedCall::new(
    "sudo",
    EXPECTED_CWD_REPO_ROOT,
    &["-n", "rm", "-rf", "/var/root/.cache/nix/gitv3"],
);

const ROOT_FETCHER_CACHE_CLEAR_CALL: ExpectedCall = ExpectedCall::new(
    "sudo",
    EXPECTED_CWD_REPO_ROOT,
    &[
        "-n",
        "rm",
        "-f",
        "/var/root/.cache/nix/fetcher-cache-v4.sqlite",
    ],
);

fn split_rebuild_calls(authorizes_sudo: bool) -> Vec<ExpectedCall> {
    let mut calls = Vec::with_capacity(SPLIT_REBUILD_BUILD_CALLS.len() + 4);
    calls.extend_from_slice(SPLIT_REBUILD_BUILD_CALLS);
    calls.push(SPLIT_REBUILD_SUDO_CHECK_CALL);
    if authorizes_sudo {
        calls.push(SPLIT_REBUILD_SUDO_AUTH_CALL);
    }
    calls.push(SPLIT_REBUILD_PROFILE_SET_CALL);
    calls.push(SPLIT_REBUILD_ACTIVATE_CALL);
    calls
}

fn split_rebuild_cache_retry_calls() -> Vec<ExpectedCall> {
    let mut calls = Vec::with_capacity(SPLIT_REBUILD_BUILD_CALLS.len() + 7);
    calls.extend_from_slice(SPLIT_REBUILD_BUILD_CALLS);
    calls.push(ROOT_GIT_CACHE_CLEAR_CALL);
    calls.push(ROOT_FETCHER_CACHE_CLEAR_CALL);
    calls.push(ExpectedCall::new(
        "scutil",
        EXPECTED_CWD_REPO_ROOT,
        &["--get", "LocalHostName"],
    ));
    calls.push(ExpectedCall::new(
        "nix",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "build",
            "--json",
            "--no-link",
            "<REPO_ROOT>#darwinConfigurations.test-host.system",
        ],
    ));
    calls.push(SPLIT_REBUILD_SUDO_CHECK_CALL);
    calls.push(SPLIT_REBUILD_PROFILE_SET_CALL);
    calls.push(SPLIT_REBUILD_ACTIVATE_CALL);
    calls
}

const UNDO_CLEAN_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "git",
    EXPECTED_CWD_REPO_ROOT,
    &["status", "--porcelain"],
)];

const UNDO_CONFIRMED_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, &["status", "--porcelain"]),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &["diff", "--stat", "packages/nix/cli.nix"],
    ),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &["checkout", "--", "packages/nix/cli.nix"],
    ),
];

const UNDO_CANCELLED_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, &["status", "--porcelain"]),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &["diff", "--stat", "packages/nix/cli.nix"],
    ),
];

struct CommandCase {
    id: &'static str,
    cli_args: &'static [&'static str],
    mode: &'static str,
    expected_exit: i32,
    expected_calls: &'static [ExpectedCall],
    stdout_contains: &'static [&'static str],
}

const COMMAND_CASES: &[CommandCase] = &[
    CommandCase {
        id: "undo_clean_noop",
        cli_args: UNDO_BASE_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UNDO_CLEAN_CALLS,
        stdout_contains: &["Nothing to undo."],
    },
    CommandCase {
        id: "undo_dirty_confirmed_reverts",
        cli_args: UNDO_BASE_ARGS,
        mode: "undo_dirty",
        expected_exit: 0,
        expected_calls: UNDO_CONFIRMED_CALLS,
        stdout_contains: &["Undo Changes (1 files)", "Reverted 1 files"],
    },
    CommandCase {
        id: "undo_dirty_cancelled_short_circuit",
        cli_args: UNDO_BASE_ARGS,
        mode: "undo_dirty",
        expected_exit: 0,
        expected_calls: UNDO_CANCELLED_CALLS,
        stdout_contains: &["Undo Changes (1 files)", "Cancelled."],
    },
    CommandCase {
        id: "update_success_passthrough",
        cli_args: UPDATE_PASSTHROUGH_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UPDATE_SUCCESS_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "update_failure_exit",
        cli_args: UPDATE_BASE_ARGS,
        mode: "update_fail",
        expected_exit: 1,
        expected_calls: UPDATE_FAILURE_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "test_success_sequence",
        cli_args: TEST_BASE_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: TEST_SUCCESS_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "test_ci_failure_exit",
        cli_args: TEST_BASE_ARGS,
        mode: "just_fail",
        expected_exit: 1,
        expected_calls: TEST_FAILURE_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "lint_success_sequence",
        cli_args: LINT_BASE_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: LINT_SUCCESS_CALLS,
        stdout_contains: &["nx routing metadata passed"],
    },
    CommandCase {
        id: "rebuild_success_passthrough",
        cli_args: REBUILD_PASSTHROUGH_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: REBUILD_SUCCESS_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "rebuild_preflight_short_circuits_before_rebuild",
        cli_args: REBUILD_CHECK_ONLY_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: REBUILD_CHECK_ONLY_CALLS,
        stdout_contains: &["Rebuild preflight passed"],
    },
    CommandCase {
        id: "rebuild_git_preflight_failure_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: "git_preflight_fail",
        expected_exit: 1,
        expected_calls: REBUILD_GIT_FAIL_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "rebuild_untracked_nix_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: "preflight_untracked",
        expected_exit: 1,
        expected_calls: REBUILD_UNTRACKED_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "rebuild_flake_check_failure_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: "flake_check_fail",
        expected_exit: 1,
        expected_calls: REBUILD_FLAKE_FAIL_CALLS,
        stdout_contains: &[],
    },
    CommandCase {
        id: "rebuild_darwin_failure_exit",
        cli_args: REBUILD_PASSTHROUGH_ARGS,
        mode: "darwin_rebuild_fail",
        expected_exit: 1,
        expected_calls: REBUILD_DARWIN_FAIL_CALLS,
        stdout_contains: &[],
    },
];

#[test]
fn system_command_flows() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    for case in COMMAND_CASES {
        run_command_case(&nx_bin, &repo_base, case)?;
    }

    Ok(())
}

#[test]
fn split_darwin_rebuild_runs_explicit_phases() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let expected_calls = split_rebuild_calls(false);

    let RunResult {
        home_dir,
        stdout,
        stderr,
    } = run_split_rebuild(
        &nx_bin,
        &repo_base,
        "split_rebuild_env",
        "success",
        &[("NX_SPLIT_DARWIN", "1")],
        &expected_calls,
    )?;

    assert!(
        stdout.contains("System rebuilt"),
        "stdout missing rebuild success\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_timing_children(
        home_dir.path(),
        "split_rebuild_env",
        &["build", "profile-compare", "profile-set", "activate"],
    )?;
    assert_activate_timing_children(
        home_dir.path(),
        "split_rebuild_env",
        &[
            "etc",
            "homebrew-bundle",
            "home-manager",
            "hm.link-generation",
        ],
    )?;

    Ok(())
}

#[test]
fn split_darwin_rebuild_skips_activation_when_system_is_current() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let RunResult {
        home_dir,
        stdout,
        stderr,
    } = run_split_rebuild(
        &nx_bin,
        &repo_base,
        "split_rebuild_current",
        "success",
        &[
            ("NX_SPLIT_DARWIN", "1"),
            (
                "NX_SYSTEM_IT_DARWIN_BUILD_OUTPUT",
                "/nix/store/current-system",
            ),
        ],
        SPLIT_REBUILD_BUILD_CALLS,
    )?;

    assert!(
        stdout.contains("System already current"),
        "stdout missing current-system success\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_timing_children(
        home_dir.path(),
        "split_rebuild_current",
        &["build", "profile-compare", "already-current"],
    )?;

    Ok(())
}

#[test]
fn split_darwin_rebuild_authorizes_sudo_when_prompt_is_needed() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let expected_calls = split_rebuild_calls(true);

    let RunResult { stdout, stderr, .. } = run_split_rebuild(
        &nx_bin,
        &repo_base,
        "split_rebuild_sudo_prompt",
        "split_sudo_prompt",
        &[("NX_SPLIT_DARWIN", "1")],
        &expected_calls,
    )?;

    assert!(
        stdout.contains("System rebuilt"),
        "stdout missing rebuild success\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Authorizing sudo"),
        "stdout missing sudo authorization phase\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("Falling back to darwin-rebuild switch"),
        "stdout should not fall back after sudo authorization\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    Ok(())
}

#[test]
fn split_darwin_rebuild_retries_source_cache_corruption() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let expected_calls = split_rebuild_cache_retry_calls();

    let RunResult { stdout, stderr, .. } = run_split_rebuild(
        &nx_bin,
        &repo_base,
        "split_rebuild_cache_corruption",
        "split_build_cache_corruption",
        &[("NX_SPLIT_DARWIN", "1")],
        &expected_calls,
    )?;

    assert!(
        stdout.contains("Nix git cache corruption detected, clearing cache and retrying"),
        "stdout missing cache-corruption retry warning\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("System rebuilt"),
        "stdout missing rebuild success\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    Ok(())
}

struct RunResult {
    home_dir: TempDir,
    stdout: String,
    stderr: String,
}

fn run_split_rebuild(
    nx_bin: &Path,
    repo_base: &Path,
    case_id: &str,
    mode: &str,
    extra_env: &[(&str, &str)],
    expected_calls: &[ExpectedCall],
) -> Result<RunResult, Box<dyn Error>> {
    let repo_root = TempDir::new()?;
    copy_tree(repo_base, repo_root.path())?;
    ensure_test_layout(repo_root.path())?;

    let stub_dir = repo_root.path().join(STUB_DIR_NAME);
    fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;

    let log_path = repo_root.path().join(LOG_FILE_NAME);
    let before = snapshot_repo_files(repo_root.path(), &should_ignore_snapshot_path)?;

    let home_dir = TempDir::new()?;
    let profile_link = home_dir.path().join("system-profile");
    symlink("/nix/store/current-system", &profile_link)?;
    let mut command = Command::new(nx_bin);
    command
        .args(["--plain", "--minimal", "rebuild"])
        .current_dir(repo_root.path())
        .env("NX_REPO_ROOT", repo_root.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NX_SYSTEM_PROFILE_PATH", &profile_link)
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("NX_SYSTEM_IT_MODE", mode)
        .env(
            "NX_SYSTEM_IT_DARWIN_REBUILD",
            stub_dir.join("darwin-rebuild"),
        )
        .env("PATH", prepend_path(&stub_dir));
    for (key, value) in extra_env {
        command.env(key, value);
    }

    let output = run_command_with_optional_stdin(&mut command, None)?;
    let after = snapshot_repo_files(repo_root.path(), &should_ignore_snapshot_path)?;
    let invocations = read_invocations(&log_path)?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        exit_code, 0,
        "case {case_id}: unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert_invocations(case_id, repo_root.path(), &invocations, expected_calls);
    assert_eq!(
        before, after,
        "case {case_id} mutated repository files\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );

    Ok(RunResult {
        home_dir,
        stdout,
        stderr,
    })
}

fn run_command_case(
    nx_bin: &Path,
    repo_base: &Path,
    case: &CommandCase,
) -> Result<(), Box<dyn Error>> {
    let repo_root = TempDir::new()?;
    copy_tree(repo_base, repo_root.path())?;
    ensure_test_layout(repo_root.path())?;

    let stub_dir = repo_root.path().join(STUB_DIR_NAME);
    fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;

    let log_path = repo_root.path().join(LOG_FILE_NAME);
    let before = snapshot_repo_files(repo_root.path(), &should_ignore_snapshot_path)?;

    let home_dir = TempDir::new()?;
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
        .env(
            "NX_SYSTEM_IT_DARWIN_REBUILD",
            stub_dir.join("darwin-rebuild"),
        )
        .env("PATH", prepend_path(&stub_dir));

    let output = run_command_with_optional_stdin(&mut command, case_stdin(case.id))?;
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
    if case.id == "rebuild_success_passthrough" {
        assert_activation_timing(home_dir.path(), case.id)?;
    }

    assert_eq!(
        before, after,
        "case {} mutated repository files\nstdout:\n{}\nstderr:\n{}",
        case.id, stdout, stderr
    );

    Ok(())
}

fn case_stdin(case_id: &str) -> Option<&'static str> {
    match case_id {
        "undo_dirty_confirmed_reverts" => Some("y\n"),
        "undo_dirty_cancelled_short_circuit" => Some("n\n"),
        _ => None,
    }
}

fn should_ignore_snapshot_path(rel_path: &str) -> bool {
    rel_path == LOG_FILE_NAME || rel_path == STUB_DIR_NAME || rel_path.starts_with(".system-stubs/")
}

fn assert_activation_timing(home_dir: &Path, case_id: &str) -> Result<(), Box<dyn Error>> {
    assert_timing_children(
        home_dir,
        case_id,
        &[
            "build",
            "nix-build",
            "fetches",
            "etc",
            "homebrew-bundle",
            "home-manager",
            "hm.link-generation",
        ],
    )
}

fn assert_timing_children(
    home_dir: &Path,
    case_id: &str,
    expected_children: &[&str],
) -> Result<(), Box<dyn Error>> {
    let path = home_dir.join(".local/state/nx/timings.jsonl");
    let raw = fs::read_to_string(&path)?;
    let record: Value = serde_json::from_str(raw.lines().last().unwrap_or_default())?;
    let phases = record["phases"]
        .as_array()
        .ok_or("timing record phases should be an array")?;
    let activation = phases
        .iter()
        .find(|phase| phase["name"] == "activation")
        .ok_or("timing record should include activation phase")?;
    let children = activation["children"]
        .as_array()
        .ok_or("activation phase should include children")?;
    let names = children
        .iter()
        .filter_map(|child| child["name"].as_str())
        .collect::<Vec<_>>();

    for expected in expected_children {
        assert!(
            names.contains(expected),
            "case {case_id}: activation child '{expected}' missing from {names:?}"
        );
    }

    Ok(())
}

fn assert_activate_timing_children(
    home_dir: &Path,
    case_id: &str,
    expected_children: &[&str],
) -> Result<(), Box<dyn Error>> {
    let path = home_dir.join(".local/state/nx/timings.jsonl");
    let raw = fs::read_to_string(&path)?;
    let record: Value = serde_json::from_str(raw.lines().last().unwrap_or_default())?;
    let phases = record["phases"]
        .as_array()
        .ok_or("timing record phases should be an array")?;
    let activation = phases
        .iter()
        .find(|phase| phase["name"] == "activation")
        .ok_or("timing record should include activation phase")?;
    let activation_children = activation["children"]
        .as_array()
        .ok_or("activation phase should include children")?;
    let activate = activation_children
        .iter()
        .find(|phase| phase["name"] == "activate")
        .ok_or("activation phase should include activate child")?;
    let nested_children = activate["children"]
        .as_array()
        .ok_or("activate phase should include nested children")?;
    let names = nested_children
        .iter()
        .filter_map(|child| child["name"].as_str())
        .collect::<Vec<_>>();

    for expected in expected_children {
        assert!(
            names.contains(expected),
            "case {case_id}: activate child '{expected}' missing from {names:?}"
        );
    }

    Ok(())
}
