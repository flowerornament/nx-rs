mod rebuild;
mod test_cmd;
mod undo;
mod update;
mod upgrade;

pub use rebuild::cmd_rebuild;
pub use test_cmd::cmd_test;
pub use undo::cmd_undo;
pub use update::cmd_update;
pub use upgrade::cmd_upgrade;

const DARWIN_REBUILD: &str = "/run/current-system/sw/bin/darwin-rebuild";

#[cfg(test)]
use crate::cli::PassthroughArgs;
#[cfg(test)]
use crate::domain::upgrade::InputChange;
#[cfg(test)]
use crate::infra::shell::run_captured_command;

#[cfg(test)]
use self::rebuild::{build_rebuild_command, has_nix_extension};
#[cfg(test)]
use self::undo::{git_diff_stat, git_modified_files};
#[cfg(test)]
use self::upgrade::{
    brew_compare_url, build_nix_update_command, flake_compare_endpoint, flake_compare_url,
    github_owner_repo, is_cache_corruption, is_fd_exhaustion, maybe_ai_summary,
    parse_ai_summary_output, parse_brew_info_json, parse_brew_outdated_json, parse_compare_json,
    should_use_detailed_ai_summary,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a minimal git repo with one committed file.
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

    // --- git_modified_files ---

    #[test]
    fn has_nix_extension_accepts_lowercase_nix_files() {
        assert!(has_nix_extension("home/default.nix"));
        assert!(has_nix_extension("packages/cli.nix"));
    }

    #[test]
    fn has_nix_extension_rejects_non_nix_or_uppercase_extensions() {
        assert!(!has_nix_extension("home/default.NIX"));
        assert!(!has_nix_extension("home/default.nix.bak"));
        assert!(!has_nix_extension("home/default"));
    }

    #[test]
    fn modified_files_empty_on_clean_tree() {
        let tmp = init_git_repo();
        let modified = git_modified_files(tmp.path()).unwrap();
        assert!(modified.is_empty());
    }

    #[test]
    fn modified_files_detects_unstaged_changes() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("file.txt"), "changed\n").unwrap();

        let modified = git_modified_files(tmp.path()).unwrap();
        assert_eq!(modified, vec!["file.txt"]);
    }

    #[test]
    fn modified_files_ignores_staged_only() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("file.txt"), "staged\n").unwrap();
        run_captured_command("git", &["add", "file.txt"], Some(tmp.path())).unwrap();

        let modified = git_modified_files(tmp.path()).unwrap();
        // Staged-only files have status `M ` not ` M`, so excluded
        assert!(modified.is_empty());
    }

    #[test]
    fn modified_files_ignores_untracked() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("new.txt"), "new\n").unwrap();

        let modified = git_modified_files(tmp.path()).unwrap();
        assert!(modified.is_empty());
    }

    // --- git_diff_stat ---

    #[test]
    fn diff_stat_returns_summary_for_modified_file() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("file.txt"), "changed\n").unwrap();

        let summary = git_diff_stat("file.txt", tmp.path());
        assert!(summary.is_some());
        let text = summary.unwrap();
        assert!(
            text.contains("changed") || text.contains("insertion") || text.contains("deletion"),
            "expected diff stat summary, got: {text}"
        );
    }

    #[test]
    fn diff_stat_returns_none_for_clean_file() {
        let tmp = init_git_repo();
        let summary = git_diff_stat("file.txt", tmp.path());
        assert!(summary.is_none());
    }

    // --- parse_brew_outdated_json ---

    #[test]
    fn brew_parse_extracts_formulae() {
        let json = r#"{
            "formulae": [
                {
                    "name": "git",
                    "installed_versions": ["2.43.0"],
                    "current_version": "2.44.0"
                },
                {
                    "name": "jq",
                    "installed_versions": ["1.6"],
                    "current_version": "1.7.1"
                }
            ],
            "casks": []
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "git");
        assert_eq!(result[0].installed_version, "2.43.0");
        assert_eq!(result[0].current_version, "2.44.0");
        assert!(!result[0].is_cask);
        assert_eq!(result[1].name, "jq");
        assert_eq!(result[1].installed_version, "1.6");
        assert_eq!(result[1].current_version, "1.7.1");
        assert!(!result[1].is_cask);
    }

    #[test]
    fn brew_parse_extracts_casks() {
        let json = r#"{
            "formulae": [],
            "casks": [
                {
                    "name": "firefox",
                    "installed_versions": "120.0",
                    "current_version": "121.0"
                }
            ]
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "firefox");
        assert_eq!(result[0].installed_version, "120.0");
        assert_eq!(result[0].current_version, "121.0");
        assert!(result[0].is_cask);
    }

    #[test]
    fn brew_parse_mixed_formulae_and_casks_sorted() {
        let json = r#"{
            "formulae": [
                {
                    "name": "zsh",
                    "installed_versions": ["5.9"],
                    "current_version": "5.9.1"
                }
            ],
            "casks": [
                {
                    "name": "alacritty",
                    "installed_versions": "0.12",
                    "current_version": "0.13"
                }
            ]
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 2);
        // Sorted by name: alacritty < zsh
        assert_eq!(result[0].name, "alacritty");
        assert!(result[0].is_cask);
        assert_eq!(result[1].name, "zsh");
        assert!(!result[1].is_cask);
    }

    #[test]
    fn brew_parse_skips_incomplete_entries() {
        let json = r#"{
            "formulae": [
                {
                    "name": "",
                    "installed_versions": ["1.0"],
                    "current_version": "2.0"
                },
                {
                    "name": "valid",
                    "installed_versions": ["1.0"],
                    "current_version": "2.0"
                }
            ],
            "casks": []
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "valid");
    }

    #[test]
    fn brew_parse_invalid_json_returns_empty() {
        let result = parse_brew_outdated_json("not json at all");
        assert!(result.is_empty());
    }

    #[test]
    fn brew_parse_empty_json_returns_empty() {
        let result = parse_brew_outdated_json("{}");
        assert!(result.is_empty());
    }

    #[test]
    fn brew_parse_empty_arrays_returns_empty() {
        let json = r#"{"formulae": [], "casks": []}"#;
        let result = parse_brew_outdated_json(json);
        assert!(result.is_empty());
    }

    // --- parse_brew_info_json ---

    #[test]
    fn brew_info_parse_extracts_formula_metadata() {
        let json = r#"{
            "formulae": [
                {
                    "name": "git",
                    "homepage": "https://github.com/git/git",
                    "desc": "Distributed revision control system"
                }
            ]
        }"#;

        let result = parse_brew_info_json(json, false);
        let metadata = result.get("git").expect("git metadata should exist");
        assert_eq!(
            metadata.homepage.as_deref(),
            Some("https://github.com/git/git")
        );
        assert_eq!(
            metadata.description.as_deref(),
            Some("Distributed revision control system")
        );
    }

    #[test]
    fn brew_info_parse_extracts_cask_metadata() {
        let json = r#"{
            "casks": [
                {
                    "token": "firefox",
                    "homepage": "https://www.mozilla.org/firefox/",
                    "desc": "Web browser"
                }
            ]
        }"#;

        let result = parse_brew_info_json(json, true);
        let metadata = result
            .get("firefox")
            .expect("firefox metadata should exist");
        assert_eq!(
            metadata.homepage.as_deref(),
            Some("https://www.mozilla.org/firefox/")
        );
        assert_eq!(metadata.description.as_deref(), Some("Web browser"));
    }

    #[test]
    fn brew_info_parse_invalid_json_returns_empty() {
        let result = parse_brew_info_json("oops", false);
        assert!(result.is_empty());
    }

    // --- flake changelog metadata ---

    fn sample_input_change() -> InputChange {
        InputChange {
            name: "home-manager".to_string(),
            owner: "nix-community".to_string(),
            repo: "home-manager".to_string(),
            old_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            new_rev: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        }
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

    // --- changelog URL derivation ---

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
    fn brew_compare_url_for_github_homepage() {
        let url = brew_compare_url(
            Some("https://github.com/BurntSushi/ripgrep"),
            "v14.1.0",
            "14.1.1",
        );
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/BurntSushi/ripgrep/compare/14.1.0...14.1.1")
        );
    }

    #[test]
    fn brew_compare_url_non_github_returns_none() {
        let url = brew_compare_url(Some("https://example.com/project"), "1.0.0", "1.1.0");
        assert!(url.is_none());
    }

    // --- is_fd_exhaustion ---

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

    // --- is_cache_corruption ---

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

    // --- build_nix_update_command ---

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

    // --- build_rebuild_command ---

    #[test]
    fn rebuild_command_includes_base_args() {
        let args = PassthroughArgs {
            passthrough: Vec::new(),
        };
        let result = build_rebuild_command("/Users/test/.nix-config", &args);
        assert_eq!(result[0], DARWIN_REBUILD);
        assert_eq!(result[1], "switch");
        assert_eq!(result[2], "--flake");
        assert_eq!(result[3], "/Users/test/.nix-config");
    }

    #[test]
    fn rebuild_command_includes_passthrough_args() {
        let args = PassthroughArgs {
            passthrough: vec!["--show-trace".into()],
        };
        let result = build_rebuild_command("/test", &args);
        assert_eq!(
            result,
            vec![
                DARWIN_REBUILD.to_string(),
                "switch".to_string(),
                "--flake".to_string(),
                "/test".to_string(),
                "--show-trace".to_string(),
            ]
        );
    }
}
