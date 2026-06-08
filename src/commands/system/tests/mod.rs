use super::*;
use std::fs;

use tempfile::TempDir;

use crate::cli::{RebuildArgs, UpgradeArgs, UpgradeFlowArgs, UpgradeSkipArgs};
use crate::domain::upgrade::{InputChange, github_owner_repo};
use crate::infra::shell::{CapturedCommand, run_captured_command};

use super::fixed_output_hash::{
    FixedOutputHashMismatch, apply_fixed_output_hash_repair, find_fixed_output_hash_targets,
    parse_fixed_output_hash_mismatch, path_is_clean,
};
use super::rebuild::{
    FailureOutputExcerpt, SplitBuildOutputMode, build_rebuild_command,
    build_rebuild_command_with_manifest, failure_output_excerpt, has_nix_extension,
    parse_system_config_path, quiet_activation_line, should_use_split_darwin,
    split_nix_build_command_with_log_format, sudo_password_required,
};
use super::undo::{git_diff_stat, git_modified_files};
use super::upgrade::{
    brew_compare_url, build_nix_command, flake_compare_endpoint, flake_compare_url,
    flake_prefetch_ref, is_cache_corruption, is_fd_exhaustion, maybe_ai_summary,
    parse_ai_summary_output, parse_brew_info_json, parse_brew_outdated_json, parse_compare_json,
    should_use_detailed_ai_summary, upgrade_requires_manifest_system_safety,
};
use crate::domain::upgrade::build_flake_update_args;

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
        prefetch_ref: Some(
            "github:nix-community/home-manager/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
        ),
    }
}
