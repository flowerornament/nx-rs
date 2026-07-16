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
use std::os::unix::fs::symlink;
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

const DARWIN_REBUILD_CMD: &str = "/nix/var/nix/profiles/system/sw/bin/darwin-rebuild";
const RUN_CURRENT_DARWIN_REBUILD_CMD: &str = "/run/current-system/sw/bin/darwin-rebuild";
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
const CACHE_PREFLIGHT_HOST_ARGS: &[&str] = &["--get", "LocalHostName"];
const CACHE_PREFLIGHT_BUILD_ARGS: &[&str] = &[
    "build",
    "<REPO_ROOT>#darwinConfigurations.test-host.system",
    "--dry-run",
];
const REBUILD_FLAKE_ARGS: &[&str] = &[
    "--log-format",
    "internal-json",
    "flake",
    "check",
    REPO_ROOT_TOKEN,
];
const SUDO_SET_HOME_ARG: &str = "-H";
const ROOT_ENV_PROGRAM: &str = "/usr/bin/env";
const ROOT_HOME_ENV_ARG: &str = "HOME=/var/root";
const NIX_REMOTE_DAEMON_ENV_ARG: &str = "NIX_REMOTE=daemon";
const FLAKE_UPDATE_ARGS: &[&str] = &["--log-format", "internal-json", "flake", "update"];

const UPGRADE_COMMIT_ARGS: &[&str] = &["upgrade", "--skip-brew", "--skip-rebuild", "--no-ai"];
const UPGRADE_FAILURE_ARGS: &[&str] = &["upgrade", "--no-ai"];
const UPGRADE_DRY_RUN_SKIP_BREW_ARGS: &[&str] = &["upgrade", "--dry-run", "--skip-brew", "--no-ai"];
const UPGRADE_REBUILD_ARGS: &[&str] = &["upgrade", "--skip-brew", "--skip-commit", "--no-ai"];
const UPGRADE_REBUILD_FAILURE_ARGS: &[&str] =
    &["upgrade", "--skip-brew", "--skip-commit", "--no-ai"];
const UPGRADE_HASH_REPAIR_ARGS: &[&str] = &["upgrade", "--skip-brew", "--no-ai"];
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
const UPGRADE_TARGETED_ARGS: &[&str] = &[
    "upgrade",
    "nx-rs",
    "anneal",
    "--skip-rebuild",
    "--skip-commit",
    "--no-ai",
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
const NIXPKGS_PREFETCH_ARGS: &[&str] = &[
    "flake",
    "prefetch",
    "--json",
    "github:NixOS/nixpkgs/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
];
const UPGRADE_NIX_CONFIG: &str = "extra-access-tokens = github.com=ghp_system_matrix_token";

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
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, NIXPKGS_PREFETCH_ARGS),
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_NIXPKGS_COMPARE_ARGS),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &["-C", REPO_ROOT_TOKEN, "add", "--", "flake.lock"],
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
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, NIXPKGS_PREFETCH_ARGS),
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_NIXPKGS_COMPARE_ARGS),
];

const UPGRADE_FAILURE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
];

const UPGRADE_PASSTHROUGH_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new(
        "nix",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "--log-format",
            "internal-json",
            "flake",
            "update",
            "--commit-lock-file",
            "foo",
        ],
    ),
];

const UPGRADE_TOKEN_MODE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS)
        .with_env(&[("NIX_CONFIG", UPGRADE_NIX_CONFIG)]),
];

const UPGRADE_TARGETED_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new(
        "nix",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "--log-format",
            "internal-json",
            "flake",
            "update",
            "nx-rs",
            "anneal",
        ],
    ),
];

const UPGRADE_CACHE_RETRY_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
];

const UPGRADE_PREFETCH_CACHE_RETRY_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, NIXPKGS_PREFETCH_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, NIXPKGS_PREFETCH_ARGS),
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_NIXPKGS_COMPARE_ARGS),
];

const UPGRADE_NO_CHANGE_NO_COMMIT_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
];

const UPGRADE_BREW_NO_UPDATES_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("brew", EXPECTED_CWD_REPO_ROOT, &["outdated", "--json"]),
];

const UPGRADE_REBUILD_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("scutil", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_HOST_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_BUILD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            DARWIN_REBUILD_CMD,
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
];

const UPGRADE_SPLIT_REBUILD_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("scutil", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_HOST_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_BUILD_ARGS),
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
            "--no-link",
            "--print-out-paths",
            "--log-format",
            "internal-json",
            "<REPO_ROOT>#darwinConfigurations.test-host.system",
        ],
    ),
    ExpectedCall::new("sudo", EXPECTED_CWD_REPO_ROOT, &["-n", "true"]),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            SUDO_SET_HOME_ARG,
            ROOT_ENV_PROGRAM,
            ROOT_HOME_ENV_ARG,
            NIX_REMOTE_DAEMON_ENV_ARG,
            "nix-env",
            "--log-format",
            "internal-json",
            "-p",
            "/nix/var/nix/profiles/system",
            "--set",
            "/nix/store/new-system",
        ],
    ),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            SUDO_SET_HOME_ARG,
            ROOT_ENV_PROGRAM,
            ROOT_HOME_ENV_ARG,
            NIX_REMOTE_DAEMON_ENV_ARG,
            "/nix/store/new-system/activate",
        ],
    ),
];

const UPGRADE_SPLIT_REBUILD_FAILURE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("scutil", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_HOST_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_BUILD_ARGS),
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
            "--no-link",
            "--print-out-paths",
            "--log-format",
            "internal-json",
            "<REPO_ROOT>#darwinConfigurations.test-host.system",
        ],
    ),
];

const UPGRADE_SPLIT_REBUILD_RUN_CURRENT_LEGACY_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("scutil", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_HOST_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_BUILD_ARGS),
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
            "--no-link",
            "--print-out-paths",
            "--log-format",
            "internal-json",
            "<REPO_ROOT>#darwinConfigurations.test-host.system",
        ],
    ),
    ExpectedCall::new("sudo", EXPECTED_CWD_REPO_ROOT, &["-n", "true"]),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "-n",
            "-l",
            DARWIN_REBUILD_CMD,
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
        ],
    ),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "-n",
            "-l",
            RUN_CURRENT_DARWIN_REBUILD_CMD,
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
        ],
    ),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            RUN_CURRENT_DARWIN_REBUILD_CMD,
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
];

const UPGRADE_REBUILD_FAILURE_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("scutil", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_HOST_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_BUILD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            DARWIN_REBUILD_CMD,
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
];

const UPGRADE_HASH_REPAIR_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, NIXPKGS_PREFETCH_ARGS),
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_NIXPKGS_COMPARE_ARGS),
    ExpectedCall::new("scutil", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_HOST_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, CACHE_PREFLIGHT_BUILD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_TIMING_HEAD_ARGS),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            DARWIN_REBUILD_CMD,
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
    ExpectedCall::new("git", EXPECTED_CWD_REPO_ROOT, &["ls-files", "--", "*.nix"]),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &["status", "--porcelain=v1", "--", "home/agent-sync.nix"],
    ),
    ExpectedCall::new(
        "sudo",
        EXPECTED_CWD_REPO_ROOT,
        &[
            DARWIN_REBUILD_CMD,
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--log-format",
            "internal-json",
        ],
    ),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "-C",
            REPO_ROOT_TOKEN,
            "add",
            "--",
            "flake.lock",
            "home/agent-sync.nix",
        ],
    ),
    ExpectedCall::new(
        "git",
        EXPECTED_CWD_REPO_ROOT,
        &[
            "-C",
            REPO_ROOT_TOKEN,
            "commit",
            "-m",
            "Update flake (nixpkgs) + fix FOD hash drift in home/agent-sync.nix",
        ],
    ),
];

const UPGRADE_BREW_WITH_UPDATES_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("gh", EXPECTED_CWD_REPO_ROOT, GH_AUTH_TOKEN_ARGS),
    ExpectedCall::new("nix", EXPECTED_CWD_REPO_ROOT, FLAKE_UPDATE_ARGS),
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

const UPGRADE_INVALID_LOCK_CASES: &[UpgradeCase] = &[
    UpgradeCase {
        id: "upgrade_malformed_lock_before_update_short_circuits",
        cli_args: UPGRADE_FAILURE_ARGS,
        mode: "upgrade_lock_malformed_pre",
        expected_exit: 1,
        expected_calls: &[],
        stdout_contains: &[],
    },
    UpgradeCase {
        id: "upgrade_unreadable_lock_after_update_short_circuits",
        cli_args: UPGRADE_FAILURE_ARGS,
        mode: "upgrade_lock_unreadable_post",
        expected_exit: 1,
        expected_calls: UPGRADE_NO_CHANGE_NO_COMMIT_CALLS,
        stdout_contains: &[],
    },
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
        id: "upgrade_rebuild_hash_mismatch_repairs_and_retries",
        cli_args: UPGRADE_HASH_REPAIR_ARGS,
        mode: "upgrade_hash_repair",
        expected_exit: 0,
        expected_calls: UPGRADE_HASH_REPAIR_CALLS,
        stdout_contains: &[
            "Auto-updated home/agent-sync.nix:4: hash sha256-old -> sha256-new (FOD content drift); retrying",
            "System rebuilt",
            "Committed: Update flake (nixpkgs) + fix FOD hash drift in home/agent-sync.nix",
        ],
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
        id: "upgrade_targeted_inputs_skip_brew_by_default",
        cli_args: UPGRADE_TARGETED_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UPGRADE_TARGETED_CALLS,
        stdout_contains: &["All flake inputs up to date"],
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
        id: "upgrade_prefetch_cache_corruption_retries_once",
        cli_args: UPGRADE_SKIP_COMMIT_ARGS,
        mode: "upgrade_prefetch_cache_corruption",
        expected_exit: 0,
        expected_calls: UPGRADE_PREFETCH_CACHE_RETRY_CALLS,
        stdout_contains: &["Flake sources ready"],
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

#[test]
fn upgrade_invalid_locks_fail_at_phase_boundaries() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    for case in UPGRADE_INVALID_LOCK_CASES {
        let output = run_case_with_extra_env(&nx_bin, &repo_base, case, &[])?;
        let stage = if case.mode.ends_with("_pre") {
            "before"
        } else {
            "after"
        };
        assert!(
            output
                .stderr
                .contains(&format!("Could not load flake.lock {stage} update")),
            "case {} did not explain lock failure\nstderr:\n{}",
            case.id,
            output.stderr
        );
    }

    Ok(())
}

#[test]
fn upgrade_split_rebuild_uses_structured_nix_output() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let case = UpgradeCase {
        id: "upgrade_split_rebuild_structured",
        cli_args: UPGRADE_REBUILD_ARGS,
        mode: "success",
        expected_exit: 0,
        expected_calls: UPGRADE_SPLIT_REBUILD_CALLS,
        stdout_contains: &["System rebuilt"],
    };

    let output = run_case_with_extra_env(&nx_bin, &repo_base, &case, &[("NX_SPLIT_DARWIN", "1")])?;

    assert_structured_split_build_output(&output.stdout, &output.stderr);

    Ok(())
}

#[test]
fn upgrade_split_rebuild_preserves_run_current_passwordless_legacy_sudo()
-> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let case = UpgradeCase {
        id: "upgrade_split_rebuild_run_current_passwordless_legacy_sudo",
        cli_args: UPGRADE_REBUILD_ARGS,
        mode: "split_sudo_prompt_run_current_legacy_available",
        expected_exit: 0,
        expected_calls: UPGRADE_SPLIT_REBUILD_RUN_CURRENT_LEGACY_CALLS,
        stdout_contains: &[
            "activation: using passwordless darwin-rebuild",
            "Running darwin-rebuild switch",
            "System rebuilt",
        ],
    };

    let output = run_case_with_extra_env(&nx_bin, &repo_base, &case, &[("NX_SPLIT_DARWIN", "1")])?;

    assert!(
        !output
            .stdout
            .contains("Authorizing sudo for system profile update and activation"),
        "stdout should not prompt when /run/current-system darwin-rebuild is passwordless\nstdout:\n{}\nstderr:\n{}",
        output.stdout,
        output.stderr
    );
    assert_structured_split_build_output(&output.stdout, &output.stderr);

    Ok(())
}

#[test]
fn upgrade_split_rebuild_failure_surfaces_structured_diagnostics() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let case = UpgradeCase {
        id: "upgrade_split_rebuild_build_failure",
        cli_args: UPGRADE_REBUILD_ARGS,
        mode: "split_build_fail",
        expected_exit: 1,
        expected_calls: UPGRADE_SPLIT_REBUILD_FAILURE_CALLS,
        stdout_contains: &[
            "Failure output:",
            "anneal-0.13.1",
            "git [\"init\"] failed to run: No such file or directory (os error 2)",
        ],
    };

    let output = run_case_with_extra_env(&nx_bin, &repo_base, &case, &[("NX_SPLIT_DARWIN", "1")])?;

    assert_structured_split_build_output(&output.stdout, &output.stderr);

    Ok(())
}

fn run_case(nx_bin: &Path, repo_base: &Path, case: &UpgradeCase) -> Result<(), Box<dyn Error>> {
    run_case_with_extra_env(nx_bin, repo_base, case, &[]).map(|_| ())
}

struct CaseOutput {
    stdout: String,
    stderr: String,
}

fn run_case_with_extra_env(
    nx_bin: &Path,
    repo_base: &Path,
    case: &UpgradeCase,
    extra_env: &[(&str, &str)],
) -> Result<CaseOutput, Box<dyn Error>> {
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
    let profile_link = home_dir.path().join("system-profile");
    symlink("/nix/store/current-system", &profile_link)?;
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
        .env("NX_SYSTEM_PROFILE_PATH", &profile_link)
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("NX_SYSTEM_IT_MODE", case.mode)
        .env("NX_SYSTEM_IT_UPGRADE_NEW_LOCK", UPGRADE_FLAKE_LOCK_NEW)
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
    assert!(
        !stdout.contains("stub nix flake command ok"),
        "case {}: successful structured Nix progress should not leak into captured output\nstdout:\n{}\nstderr:\n{}",
        case.id,
        stdout,
        stderr
    );

    assert_repo_state(case, &before, &after, &stdout, &stderr);
    assert_home_state(case, home_dir.path(), &stdout, &stderr);
    assert_no_anonymous_boundaries(case.id, &stdout, &stderr);

    Ok(CaseOutput {
        stdout: stdout.into_owned(),
        stderr: stderr.into_owned(),
    })
}

fn assert_structured_split_build_output(stdout: &str, stderr: &str) {
    for fragment in [
        "copying path '/nix/store/split-example-one'",
        "copying path '/nix/store/activation-example'",
        "copying path '/nix/store/example-one'",
        "building '/nix/store/split-example.drv'",
        "building /nix/store/activation-example.drv",
        "building /nix/store/example.drv",
    ] {
        assert!(
            !stdout.contains(fragment) && !stderr.contains(fragment),
            "default rebuild leaked noisy Nix output '{fragment}'\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}

fn assert_no_anonymous_boundaries(case_id: &str, stdout: &str, stderr: &str) {
    assert!(
        !stdout.contains("-------------------------")
            && !stderr.contains("-------------------------"),
        "case {case_id}: output included anonymous separator lines\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

fn seed_flake_lock_if_needed(repo_root: &Path, mode: &str) -> Result<(), Box<dyn Error>> {
    if mode == "upgrade_lock_malformed_pre" {
        fs::write(repo_root.join("flake.lock"), r#"{"nodes":{"root":{}}}"#)?;
        return Ok(());
    }
    if matches!(
        mode,
        "upgrade_flake_changed"
            | "upgrade_prefetch_cache_corruption"
            | "upgrade_hash_repair"
            | "upgrade_lock_unreadable_post"
    ) {
        fs::write(repo_root.join("flake.lock"), UPGRADE_FLAKE_LOCK_OLD)?;
    }
    if mode == "upgrade_hash_repair" {
        fs::write(
            repo_root.join("home/agent-sync.nix"),
            "# nx: agent sync\n{ ... }:\n{\n  npmDepsHash = \"sha256-old\";\n}\n",
        )?;
    }
    Ok(())
}

fn seed_home_state_if_needed(home_dir: &Path, mode: &str) -> Result<(), Box<dyn Error>> {
    if matches!(
        mode,
        "upgrade_cache_corruption" | "upgrade_prefetch_cache_corruption"
    ) {
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

    if case.mode == "upgrade_hash_repair" {
        let content = after
            .get("home/agent-sync.nix")
            .expect("hash repair fixture should be snapshotted");
        assert!(content.contains("npmDepsHash = \"sha256-new\";"));
    }
}

fn expected_mutated_paths(mode: &str) -> &'static [&'static str] {
    match mode {
        "upgrade_flake_changed"
        | "upgrade_prefetch_cache_corruption"
        | "upgrade_lock_unreadable_post" => &["flake.lock"],
        "upgrade_hash_repair" => &["flake.lock", "home/agent-sync.nix"],
        _ => &[],
    }
}

fn assert_home_state(case: &UpgradeCase, home_dir: &Path, stdout: &str, stderr: &str) {
    if !matches!(
        case.id,
        "upgrade_flake_update_cache_corruption_retries_once"
            | "upgrade_prefetch_cache_corruption_retries_once"
    ) {
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
