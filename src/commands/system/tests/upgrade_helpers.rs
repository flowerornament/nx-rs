use super::*;
use std::os::unix::fs::PermissionsExt;

fn sample_upgrade_args() -> UpgradeArgs {
    UpgradeArgs {
        flow: UpgradeFlowArgs {
            dry_run: false,
            verbose: false,
            no_ai: true,
        },
        skip: UpgradeSkipArgs::default(),
        allow_source_builds: false,
        targets: Vec::new(),
        passthrough: Vec::new(),
    }
}

fn seed_upgrade_commit_repo() -> TempDir {
    let tmp = init_git_repo();
    fs::write(tmp.path().join("flake.lock"), "old lock\n").unwrap();
    run_captured_command("git", &["add", "flake.lock"], Some(tmp.path())).unwrap();
    run_captured_command(
        "git",
        &["commit", "-m", "seed flake lock"],
        Some(tmp.path()),
    )
    .unwrap();
    tmp
}

#[test]
fn upgrade_commit_owns_only_requested_paths() {
    let tmp = seed_upgrade_commit_repo();
    fs::write(tmp.path().join("flake.lock"), "new lock\n").unwrap();
    fs::write(tmp.path().join("file.txt"), "unrelated dirty change\n").unwrap();

    let outcome = commit_upgrade_paths(
        tmp.path(),
        &["flake.lock".to_string()],
        "Update flake (anneal)",
    )
    .unwrap();

    assert_eq!(outcome, CommitOutcome::Committed);
    let committed = run_captured_command(
        "git",
        &["show", "--format=", "--name-only", "HEAD"],
        Some(tmp.path()),
    )
    .unwrap();
    assert_eq!(committed.stdout.trim(), "flake.lock");
    let status = run_captured_command("git", &["status", "--short"], Some(tmp.path())).unwrap();
    assert_eq!(status.stdout.trim(), "M file.txt");
}

#[test]
fn upgrade_commit_preserves_unrelated_staged_changes() {
    let tmp = seed_upgrade_commit_repo();
    fs::write(tmp.path().join("flake.lock"), "new lock\n").unwrap();
    fs::write(tmp.path().join("file.txt"), "unrelated staged change\n").unwrap();
    run_captured_command("git", &["add", "file.txt"], Some(tmp.path())).unwrap();

    let outcome = commit_upgrade_paths(
        tmp.path(),
        &["flake.lock".to_string()],
        "Update flake (anneal)",
    )
    .unwrap();

    assert_eq!(outcome, CommitOutcome::Committed);
    let committed = run_captured_command(
        "git",
        &["show", "--format=", "--name-only", "HEAD"],
        Some(tmp.path()),
    )
    .unwrap();
    assert_eq!(committed.stdout.trim(), "flake.lock");
    let staged = run_captured_command(
        "git",
        &["diff", "--cached", "--name-only"],
        Some(tmp.path()),
    )
    .unwrap();
    assert_eq!(staged.stdout.trim(), "file.txt");
}

#[test]
fn upgrade_commit_supersedes_staged_lock_with_candidate() {
    let tmp = seed_upgrade_commit_repo();
    fs::write(tmp.path().join("flake.lock"), "staged lock\n").unwrap();
    run_captured_command("git", &["add", "flake.lock"], Some(tmp.path())).unwrap();
    fs::write(tmp.path().join("flake.lock"), "candidate lock\n").unwrap();

    let outcome = commit_upgrade_paths(
        tmp.path(),
        &["flake.lock".to_string()],
        "Update flake (anneal)",
    )
    .unwrap();

    assert_eq!(outcome, CommitOutcome::Committed);
    let committed =
        run_captured_command("git", &["show", "HEAD:flake.lock"], Some(tmp.path())).unwrap();
    assert_eq!(committed.stdout, "candidate lock\n");
    let status = run_captured_command("git", &["status", "--short"], Some(tmp.path())).unwrap();
    assert!(status.stdout.is_empty());
}

#[test]
fn upgrade_commit_treats_already_committed_paths_as_success() {
    let tmp = seed_upgrade_commit_repo();
    let outcome = commit_upgrade_paths(
        tmp.path(),
        &["flake.lock".to_string()],
        "Update flake (anneal)",
    )
    .unwrap();

    assert_eq!(outcome, CommitOutcome::NoChanges);
}

#[test]
fn upgrade_commit_failure_leaves_the_index_unchanged() {
    let tmp = seed_upgrade_commit_repo();
    fs::write(tmp.path().join("flake.lock"), "new lock\n").unwrap();
    fs::write(tmp.path().join("file.txt"), "unrelated staged change\n").unwrap();
    run_captured_command("git", &["add", "file.txt"], Some(tmp.path())).unwrap();

    let hook = tmp.path().join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    let mut permissions = fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).unwrap();

    let result = commit_upgrade_paths(
        tmp.path(),
        &["flake.lock".to_string()],
        "Update flake (anneal)",
    );

    assert!(result.is_err());
    let staged = run_captured_command(
        "git",
        &["diff", "--cached", "--name-only"],
        Some(tmp.path()),
    )
    .unwrap();
    assert_eq!(staged.stdout.trim(), "file.txt");
}

#[test]
fn upgrade_commit_message_uses_all_root_input_change_kinds() {
    let root_inputs = RootInputChanges {
        changed: vec![sample_input_change()],
        added: vec!["anneal".to_string()],
        removed: vec!["nixpkgs".to_string()],
    };

    assert_eq!(
        build_upgrade_commit_message(true, &root_inputs, &[]),
        "Update flake (anneal, home-manager, nixpkgs)"
    );
}

#[test]
fn upgrade_commit_message_falls_back_for_unreportable_lock_changes() {
    let root_inputs = RootInputChanges {
        changed: Vec::new(),
        added: Vec::new(),
        removed: Vec::new(),
    };

    assert_eq!(
        build_upgrade_commit_message(true, &root_inputs, &[]),
        "Update flake inputs"
    );
}

#[test]
fn flake_update_selects_native_or_structured_nix_output() {
    let base = vec!["flake".to_string(), "update".to_string()];

    assert_eq!(
        NixOutputMode::for_terminal(false, false).command_args(&base),
        ["--log-format", "internal-json", "flake", "update"]
    );
    assert_eq!(
        NixOutputMode::for_terminal(false, true).command_args(&base),
        ["--log-format", "bar", "flake", "update"]
    );
    assert_eq!(
        NixOutputMode::for_terminal(true, true).command_args(&base),
        ["--log-format", "bar-with-logs", "flake", "update"]
    );
}

#[test]
fn nix_config_appends_token_without_replacing_inherited_settings() {
    let inherited = NixConfig::compose(
        Some("substituters = https://cache.example\n\n"),
        "extra-access-tokens = github.com=secret",
    );
    assert_eq!(
        inherited.command_env()[0].1,
        "substituters = https://cache.example\nextra-access-tokens = github.com=secret"
    );

    let standalone = NixConfig::compose(None, "extra-access-tokens = github.com=secret");
    assert_eq!(
        standalone.command_env()[0].1,
        "extra-access-tokens = github.com=secret"
    );
}

#[test]
fn flake_compare_url_uses_short_revs() {
    let url = flake_compare_url(&sample_input_change());
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/nix-community/home-manager/compare/aaaaaaa...bbbbbbb")
    );
}

#[test]
fn flake_compare_endpoint_uses_short_revs() {
    let endpoint = flake_compare_endpoint(&sample_input_change());
    assert_eq!(
        endpoint.as_deref(),
        Some("repos/nix-community/home-manager/compare/aaaaaaa...bbbbbbb")
    );
}

#[test]
fn parse_compare_json_extracts_commit_summary() {
    let json = r#"{
            "total_commits": 4,
            "commits": [
                {"commit": {"message": "feat: first line\n\nbody"}},
                {"commit": {"message": "fix: second line"}},
                {"commit": {"message": "chore: third line"}},
                {"commit": {"message": "docs: fourth line"}}
            ]
        }"#;

    let summary = parse_compare_json(json).expect("summary should parse");
    assert_eq!(summary.total_commits, 4);
    assert_eq!(
        summary.commit_subjects,
        vec![
            "feat: first line".to_string(),
            "fix: second line".to_string(),
            "chore: third line".to_string(),
        ]
    );
}

#[test]
fn parse_compare_json_invalid_returns_none() {
    let summary = parse_compare_json("not json");
    assert!(summary.is_none());
}

#[test]
fn maybe_ai_summary_respects_no_ai_gate() {
    let mut called = false;
    let summary = maybe_ai_summary(true, || {
        called = true;
        Some("should not run".to_string())
    });
    assert!(summary.is_none());
    assert!(!called);
}

#[test]
fn maybe_ai_summary_runs_when_enabled() {
    let mut called = false;
    let summary = maybe_ai_summary(false, || {
        called = true;
        Some("ok".to_string())
    });
    assert_eq!(summary.as_deref(), Some("ok"));
    assert!(called);
}

#[test]
fn detailed_ai_summary_for_key_input() {
    assert!(should_use_detailed_ai_summary("home-manager", 1));
    assert!(should_use_detailed_ai_summary("custom-input", 51));
    assert!(!should_use_detailed_ai_summary("custom-input", 10));
}

#[test]
fn parse_ai_summary_output_compacts_and_truncates() {
    let output = "Summary: first line\n\n- second line\nthird line";
    let parsed = parse_ai_summary_output(output, 2, 30).expect("summary should parse");
    assert!(parsed.starts_with("Summary: first line second"));
    assert!(parsed.len() <= 30);
}

#[test]
fn github_owner_repo_extracts_standard_url() {
    let result = github_owner_repo("https://github.com/BurntSushi/ripgrep");
    assert_eq!(
        result,
        Some(("BurntSushi".to_string(), "ripgrep".to_string()))
    );
}

#[test]
fn github_owner_repo_handles_git_suffix() {
    let result = github_owner_repo("https://github.com/nix-community/nixvim.git");
    assert_eq!(
        result,
        Some(("nix-community".to_string(), "nixvim".to_string()))
    );
}

#[test]
fn targeted_upgrade_builds_flake_update_input_args() {
    let args = build_flake_update_args(&["nx-rs".to_string(), "anneal".to_string()], &[]);
    assert_eq!(args, vec!["flake", "update", "nx-rs", "anneal",]);
}

#[test]
fn brew_phase_runs_for_repo_wide_upgrade() {
    let args = sample_upgrade_args();

    assert!(args.should_run_brew_phase());
}

#[test]
fn brew_phase_skipped_for_targeted_upgrade() {
    let mut args = sample_upgrade_args();
    args.targets = vec!["nx-rs".to_string()];

    assert!(!args.should_run_brew_phase());
}

#[test]
fn upgrade_manifest_guard_not_required_for_dry_run() {
    let mut args = sample_upgrade_args();
    args.flow.dry_run = true;

    assert!(!upgrade_requires_manifest_system_safety(&args));
}

#[test]
fn upgrade_manifest_guard_not_required_when_rebuild_is_skipped() {
    let mut args = sample_upgrade_args();
    args.skip.skip_rebuild = true;

    assert!(!upgrade_requires_manifest_system_safety(&args));
}

#[test]
fn upgrade_manifest_guard_required_for_full_upgrade() {
    let args = sample_upgrade_args();

    assert!(upgrade_requires_manifest_system_safety(&args));
}

#[test]
fn flake_lock_transaction_restores_original_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flake.lock");
    std::fs::write(&path, b"original\n").unwrap();
    let transaction = FlakeLockTransaction::capture(dir.path()).unwrap();
    std::fs::write(&path, b"candidate\n").unwrap();

    assert!(transaction.restore().unwrap());

    assert_eq!(std::fs::read(path).unwrap(), b"original\n");
}

#[cfg(unix)]
#[test]
fn flake_lock_transaction_reports_rollback_failure_without_clobbering_candidate() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flake.lock");
    std::fs::write(&path, b"original\n").unwrap();
    let transaction = FlakeLockTransaction::capture(dir.path()).unwrap();
    std::fs::write(&path, b"candidate\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

    let error = transaction.restore().unwrap_err();

    assert!(error.to_string().contains("restoring original"));
    assert_eq!(std::fs::read(path).unwrap(), b"candidate\n");
}

#[test]
fn flake_lock_transaction_rolls_back_when_dropped_while_armed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flake.lock");
    std::fs::write(&path, b"original\n").unwrap();
    let mut transaction = FlakeLockTransaction::capture(dir.path()).unwrap();
    std::fs::write(&path, b"candidate\n").unwrap();
    transaction.observe_candidate(b"candidate\n".to_vec());

    drop(transaction);

    assert_eq!(std::fs::read(path).unwrap(), b"original\n");
}

#[test]
fn flake_lock_transaction_refuses_concurrent_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("flake.lock"), b"original\n").unwrap();
    let transaction = FlakeLockTransaction::capture(dir.path()).unwrap();

    let error = FlakeLockTransaction::capture(dir.path()).unwrap_err();

    assert!(error.to_string().contains("locking repository"));
    drop(transaction);
}

#[test]
fn admitted_transaction_transfers_lock_ownership() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("flake.lock"), b"original\n").unwrap();
    let transaction = FlakeLockTransaction::capture(dir.path()).unwrap();
    let lock = transaction.admit();

    assert!(FlakeLockTransaction::capture(dir.path()).is_err());
    drop(lock);
    FlakeLockTransaction::capture(dir.path()).unwrap();
}

#[test]
fn flake_lock_transaction_does_not_overwrite_changed_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flake.lock");
    std::fs::write(&path, b"original\n").unwrap();
    let mut transaction = FlakeLockTransaction::capture(dir.path()).unwrap();
    std::fs::write(&path, b"candidate\n").unwrap();
    transaction.observe_candidate(b"candidate\n".to_vec());
    std::fs::write(&path, b"external\n").unwrap();

    let error = transaction.restore().unwrap_err();

    assert!(error.to_string().contains("refusing to overwrite"));
    assert_eq!(std::fs::read(path).unwrap(), b"external\n");
}
