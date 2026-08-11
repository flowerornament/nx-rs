use super::*;

/// Captured (abbreviated) stderr from `nix build <flake>#darwinConfigurations.<host>.system --dry-run`
/// against a nixpkgs revision the binary cache had not caught up with.
const SAMPLE_DRY_RUN_STDERR: &str = "\
warning: Git tree '/Users/morgan/.nix-config' is dirty
these 6 derivations will be built:
  /nix/store/0kfh6g5wl8vvbmjmm6zkbz4nqhyfqhb0-starship-1.23.0.drv
  /nix/store/1kq06fzk5f7jvvj0472pfcgyzcnl90ap-terminal-notifier-2.0.0.drv
  /nix/store/8m7wpjm3v0dz8sq9m6a0y6b2r7ln3c14-python3.12-httpx-0.28.1.drv
  /nix/store/9qk3xw3nx6l0y5vjq3f9crw8z0l70y3s-darwin-system-26.05pre.drv
  /nix/store/c2m0qapmzr5r1a6ml7d3sswy3l7d7nhy-home-manager-generation.drv
  /nix/store/f9v0b39sslq7dxvzq3mfr5cvxrjr1c2j-nix-2.24.9.drv
these 4 paths will be fetched (27.61 MiB download, 116.86 MiB unpacked):
  /nix/store/2r7ll9xxsvvbl8rd77rkyjqa0ha0dn28-bash-5.2p37
  /nix/store/5j8kwhs62vp6cvy3nc0mkr2v0y1qjqcx-coreutils-9.7
  /nix/store/awxn5jrhbjyvzr3s0r0dj0dznax9qsw3-ripgrep-14.1.1
  /nix/store/x4y3wq3vh0cf6z2q28pfjvvyn4hkk0kk-zsh-5.9
";

#[test]
fn parse_dry_run_plan_extracts_builds_and_fetches() {
    let plan = parse_dry_run_plan(SAMPLE_DRY_RUN_STDERR);

    assert_eq!(
        plan,
        Some(DryRunPlan {
            to_build: vec![
                "starship-1.23.0".to_string(),
                "terminal-notifier-2.0.0".to_string(),
                "python3.12-httpx-0.28.1".to_string(),
                "darwin-system-26.05pre".to_string(),
                "home-manager-generation".to_string(),
                "nix-2.24.9".to_string(),
            ],
            to_fetch: 4,
        })
    );
}

#[test]
fn parse_dry_run_plan_handles_fully_cached_output() {
    let output = "\
these 12 paths will be fetched (94.53 MiB download, 486.36 MiB unpacked):
  /nix/store/2r7ll9xxsvvbl8rd77rkyjqa0ha0dn28-bash-5.2p37
  /nix/store/awxn5jrhbjyvzr3s0r0dj0dznax9qsw3-ripgrep-14.1.1
";

    let plan = parse_dry_run_plan(output).unwrap();
    assert!(plan.to_build.is_empty());
    assert_eq!(plan.to_fetch, 2);
}

#[test]
fn parse_dry_run_plan_handles_singular_headers() {
    let output = "\
this derivation will be built:
  /nix/store/0kfh6g5wl8vvbmjmm6zkbz4nqhyfqhb0-starship-1.23.0.drv
this path will be fetched (1.02 MiB download, 4.51 MiB unpacked):
  /nix/store/x4y3wq3vh0cf6z2q28pfjvvyn4hkk0kk-zsh-5.9
";

    let plan = parse_dry_run_plan(output).unwrap();
    assert_eq!(plan.to_build, vec!["starship-1.23.0".to_string()]);
    assert_eq!(plan.to_fetch, 1);
}

#[test]
fn parse_dry_run_plan_rejects_store_paths_outside_sections() {
    let output = "\
evaluating derivation '/nix/store/abc-flake.drv'
/nix/store/0kfh6g5wl8vvbmjmm6zkbz4nqhyfqhb0-starship-1.23.0
";

    assert_eq!(parse_dry_run_plan(output), None);
}

#[test]
fn parse_dry_run_plan_of_empty_output_is_empty() {
    assert_eq!(parse_dry_run_plan(""), Some(DryRunPlan::default()));
}

#[test]
fn parse_dry_run_plan_rejects_unrecognized_success_output() {
    assert_eq!(
        parse_dry_run_plan("future nix plan format: 6 local builds\n"),
        None
    );
}

#[test]
fn parse_dry_run_plan_allows_warning_only_no_work_output() {
    assert_eq!(
        parse_dry_run_plan("warning: Git tree is dirty\n"),
        Some(DryRunPlan::default())
    );
}

#[test]
fn parse_dry_run_plan_allows_flake_unpacking_only_no_work_output() {
    let output = "unpacking 'https://api.flakehub.com/f/pinned/example/source.tar.gz' into the Git cache...\n";

    assert_eq!(parse_dry_run_plan(output), Some(DryRunPlan::default()));
}

#[test]
fn parse_dry_run_plan_ignores_progress_before_recognized_plan() {
    let output = "\
evaluating candidate system closure...
this derivation will be built:
  /nix/store/0kfh6g5wl8vvbmjmm6zkbz4nqhyfqhb0-starship-1.23.0.drv
";

    assert_eq!(
        parse_dry_run_plan(output),
        Some(DryRunPlan {
            to_build: vec!["starship-1.23.0".to_string()],
            to_fetch: 0,
        })
    );
}

#[test]
fn parse_dry_run_plan_ends_section_at_unrecognized_output() {
    let output = "\
this derivation will be built:
  /nix/store/0kfh6g5wl8vvbmjmm6zkbz4nqhyfqhb0-starship-1.23.0.drv
evaluating another input...
  /nix/store/1kq06fzk5f7jvvj0472pfcgyzcnl90ap-terminal-notifier-2.0.0.drv
";

    assert_eq!(
        parse_dry_run_plan(output),
        Some(DryRunPlan {
            to_build: vec!["starship-1.23.0".to_string()],
            to_fetch: 0,
        })
    );
}

#[test]
fn derivation_display_name_strips_store_prefix_and_drv_suffix() {
    assert_eq!(
        derivation_display_name("/nix/store/0kfh6g5wl8vvbmjmm6zkbz4nqhyfqhb0-starship-1.23.0.drv"),
        "starship-1.23.0"
    );
}

#[test]
fn derivation_display_name_keeps_non_drv_paths_readable() {
    assert_eq!(
        derivation_display_name("/nix/store/2r7ll9xxsvvbl8rd77rkyjqa0ha0dn28-bash-5.2p37"),
        "bash-5.2p37"
    );
}

#[test]
fn derivation_display_name_tolerates_unexpected_shapes() {
    assert_eq!(derivation_display_name("weird.drv"), "weird");
}

#[test]
fn cache_miss_threshold_defaults_and_parses() {
    assert_eq!(parse_cache_miss_threshold(None), 5);
    assert_eq!(parse_cache_miss_threshold(Some("12")), 12);
    assert_eq!(parse_cache_miss_threshold(Some(" 0 ")), 0);
    assert_eq!(parse_cache_miss_threshold(Some("not-a-number")), 5);
    assert_eq!(parse_cache_miss_threshold(Some("")), 5);
}

#[test]
fn unavailable_coverage_is_advisory_only_when_requested() {
    assert_eq!(
        unavailable_outcome(CachePreflightMode::ReportOnly),
        CachePreflightOutcome::Admitted
    );
}

#[test]
fn unavailable_coverage_fails_closed_by_default() {
    assert_eq!(
        unavailable_outcome(CachePreflightMode::RequireApproval),
        CachePreflightOutcome::Failed
    );
}

#[test]
fn preapproval_does_not_accept_unavailable_coverage() {
    assert_eq!(
        unavailable_outcome(CachePreflightMode::ApproveSourceBuilds),
        CachePreflightOutcome::Failed
    );
}

#[test]
fn explicit_bypass_accepts_unavailable_coverage() {
    assert_eq!(
        unavailable_outcome(CachePreflightMode::Bypass),
        CachePreflightOutcome::Admitted
    );
}

#[test]
fn interactive_source_builds_follow_explicit_acceptance() {
    let mode = CachePreflightMode::RequireApproval;

    assert_eq!(
        source_builds_outcome(mode, true, || true),
        CachePreflightOutcome::Admitted
    );
    assert_eq!(
        source_builds_outcome(mode, true, || false),
        CachePreflightOutcome::Cancelled
    );
}

#[test]
fn noninteractive_source_builds_fail_without_prompting() {
    let mut prompted = false;
    let outcome = source_builds_outcome(CachePreflightMode::RequireApproval, false, || {
        prompted = true;
        true
    });

    assert_eq!(outcome, CachePreflightOutcome::Failed);
    assert!(!prompted);
}

#[test]
fn source_build_preapproval_never_prompts() {
    let mut prompted = false;
    let outcome = source_builds_outcome(CachePreflightMode::ApproveSourceBuilds, false, || {
        prompted = true;
        false
    });

    assert_eq!(outcome, CachePreflightOutcome::Admitted);
    assert!(!prompted);
}
