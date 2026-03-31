use super::*;
use std::fs;

use tempfile::TempDir;

use crate::cli::{RebuildArgs, UpgradeArgs, UpgradeFlowArgs, UpgradeSkipArgs};
use crate::domain::upgrade::InputChange;
use crate::infra::shell::run_captured_command;

use super::rebuild::{
    build_rebuild_command, build_rebuild_command_with_manifest, has_nix_extension,
};
use super::undo::{git_diff_stat, git_modified_files};
use super::upgrade::{
    brew_compare_url, build_nix_update_command, flake_compare_endpoint, flake_compare_url,
    github_owner_repo, is_cache_corruption, is_fd_exhaustion, maybe_ai_summary,
    parse_ai_summary_output, parse_brew_info_json, parse_brew_outdated_json, parse_compare_json,
    should_use_detailed_ai_summary, upgrade_requires_manifest_system_safety,
};

mod brew;
mod git;
mod rebuild;
mod upgrade_helpers;

fn init_git_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    run_captured_command("git", &["init"], Some(root)).unwrap();
    run_captured_command(
        "git",
        &["config", "user.email", "test@test.com"],
        Some(root),
    )
    .unwrap();
    run_captured_command("git", &["config", "user.name", "Test"], Some(root)).unwrap();

    fs::write(root.join("file.txt"), "initial\n").unwrap();
    run_captured_command("git", &["add", "file.txt"], Some(root)).unwrap();
    run_captured_command("git", &["commit", "-m", "init"], Some(root)).unwrap();

    tmp
}

fn sample_input_change() -> InputChange {
    InputChange {
        name: "home-manager".to_string(),
        owner: "nix-community".to_string(),
        repo: "home-manager".to_string(),
        old_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        new_rev: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
    }
}
