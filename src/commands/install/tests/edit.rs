use super::*;

#[test]
fn execute_edit_codex_uses_deterministic_path() {
    let tmp = setup_install_root(DEFAULT_CLI_NIX);
    let root = tmp.path();

    let (engine, calls) = stub_engine("codex", false, true, "unused");

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, CLI_NIX_PATH, &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let edited =
        fs::read_to_string(root.join(CLI_NIX_PATH)).expect("edited file should be readable");
    assert!(edited.contains("ripgrep"));
}

#[test]
fn execute_edit_claude_uses_deterministic_path() {
    let tmp = setup_install_root(DEFAULT_CLI_NIX);
    let root = tmp.path();

    let (engine, calls) = stub_engine("claude", true, true, "ok");

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, CLI_NIX_PATH, &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let edited =
        fs::read_to_string(root.join(CLI_NIX_PATH)).expect("target file should be readable");
    assert!(edited.contains("ripgrep"));
}

#[test]
fn execute_edit_claude_is_idempotent_without_ai_fallback() {
    let tmp = setup_install_root(
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n    ripgrep\n  ];\n}\n",
    );
    let root = tmp.path();

    let (engine, calls) = stub_engine("claude", true, true, "ok");

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, CLI_NIX_PATH, &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn execute_edit_claude_falls_back_to_ai_when_deterministic_unsupported() {
    let tmp = setup_install_root("{ pkgs, ... }:\n{\n  services = { };\n}\n");
    let root = tmp.path();

    let (engine, calls) = stub_engine("claude", true, true, "ok");

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, CLI_NIX_PATH, &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn execute_edit_claude_fallback_failure_returns_false() {
    let tmp = setup_install_root("{ pkgs, ... }:\n{\n  services = { };\n}\n");
    let root = tmp.path();

    let (engine, calls) = stub_engine("claude", true, false, "boom");

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(!execute_edit(&plan, CLI_NIX_PATH, &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
