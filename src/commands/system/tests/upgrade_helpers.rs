use super::*;

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
}

#[test]
fn cache_corruption_not_detected_for_other_errors() {
    assert!(!is_cache_corruption("error: something unrelated"));
    assert!(!is_cache_corruption(""));
}

#[test]
fn build_command_without_ulimit() {
    let args = vec!["flake".into(), "update".into()];
    let result = build_nix_update_command(&args, None);
    assert_eq!(result, vec!["flake", "update"]);
}

#[test]
fn build_command_with_ulimit() {
    let args = vec!["flake".into(), "update".into()];
    let result = build_nix_update_command(&args, Some(8192));
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], "-lc");
    assert!(result[1].contains("ulimit -n 8192"));
    assert!(result[1].contains("exec nix flake update"));
}
