use super::*;

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
    assert!(modified.is_empty());
}

#[test]
fn modified_files_ignores_untracked() {
    let tmp = init_git_repo();
    fs::write(tmp.path().join("new.txt"), "new\n").unwrap();

    let modified = git_modified_files(tmp.path()).unwrap();
    assert!(modified.is_empty());
}

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
