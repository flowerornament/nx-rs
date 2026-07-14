use super::*;

fn sample_upgrade_args() -> UpgradeArgs {
    UpgradeArgs {
        flow: UpgradeFlowArgs {
            dry_run: false,
            verbose: false,
            no_ai: true,
        },
        skip: UpgradeSkipArgs::default(),
        targets: Vec::new(),
        passthrough: Vec::new(),
    }
}

#[test]
fn flake_update_selects_structured_or_verbose_nix_output() {
    let base = vec!["flake".to_string(), "update".to_string()];

    assert_eq!(
        nix_update_output_args(&base, false),
        ["--log-format", "internal-json", "flake", "update"]
    );
    assert_eq!(
        nix_update_output_args(&base, true),
        ["--log-format", "bar-with-logs", "flake", "update"]
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
fn fd_exhaustion_detected() {
    assert!(is_fd_exhaustion(
        "error: creating git packfile indexer: Too many open files"
    ));
    assert!(is_fd_exhaustion("something too many open files here"));
}

#[test]
fn fd_exhaustion_not_detected_for_other_errors() {
    assert!(!is_fd_exhaustion("error: attribute not found"));
    assert!(!is_fd_exhaustion(""));
}

#[test]
fn cache_corruption_detected() {
    assert!(is_cache_corruption(
        "error: failed to insert entry: invalid object specified"
    ));
    assert!(is_cache_corruption(
        "error: adding a file to a tree builder during nix fetch"
    ));
    assert!(is_cache_corruption(
        "error: looking up file '«github:owner/repo/rev»/README.md': object not found - no match for id (abc123)"
    ));
}

#[test]
fn cache_corruption_not_detected_for_other_errors() {
    assert!(!is_cache_corruption("error: something unrelated"));
    assert!(!is_cache_corruption(""));
}

#[test]
fn flake_prefetch_ref_uses_new_github_revision() {
    let change = sample_input_change();
    let prefetch = flake_prefetch_ref(&change).expect("prefetch ref");

    assert_eq!(prefetch.name, "home-manager");
    assert_eq!(prefetch.short_rev, "bbbbbbb");
    assert_eq!(
        prefetch.flake_ref,
        "github:nix-community/home-manager/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
}

#[test]
fn flake_prefetch_ref_skips_inputs_without_prefetch_ref() {
    let mut change = sample_input_change();
    change.prefetch_ref = None;

    assert!(flake_prefetch_ref(&change).is_none());
}

#[test]
fn build_command_without_ulimit() {
    let args = build_flake_update_args(&[], &[]);
    let result = build_nix_command(&args, None);
    assert_eq!(
        result,
        (
            "nix".to_string(),
            vec!["flake".to_string(), "update".to_string()]
        )
    );
}

#[test]
fn build_command_with_ulimit() {
    let args = build_flake_update_args(&[], &[]);
    let result = build_nix_command(&args, Some(8192));
    assert_eq!(
        result,
        (
            "bash".to_string(),
            vec![
                "-lc".to_string(),
                "ulimit -n 8192 2>/dev/null; exec \"$@\"".to_string(),
                "nx-nix-with-ulimit".to_string(),
                "nix".to_string(),
                "flake".to_string(),
                "update".to_string(),
            ],
        )
    );
}

#[test]
fn targeted_upgrade_builds_flake_update_input_args() {
    let args = build_flake_update_args(&["nx-rs".to_string(), "anneal".to_string()], &[]);
    assert_eq!(args, vec!["flake", "update", "nx-rs", "anneal",]);
}

#[test]
fn targeted_upgrade_command_with_ulimit_wraps_flake_update_input() {
    let args = build_flake_update_args(&["nx-rs".to_string()], &[]);
    let result = build_nix_command(&args, Some(8192));
    assert_eq!(
        result.1,
        vec![
            "-lc".to_string(),
            "ulimit -n 8192 2>/dev/null; exec \"$@\"".to_string(),
            "nx-nix-with-ulimit".to_string(),
            "nix".to_string(),
            "flake".to_string(),
            "update".to_string(),
            "nx-rs".to_string(),
        ]
    );
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
