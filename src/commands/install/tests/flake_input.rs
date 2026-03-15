use super::*;

#[test]
fn gate_flake_input_refuses_codex_engine() {
    let tmp = setup_install_root(DEFAULT_CLI_NIX);
    let root = tmp.path();
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.yes = true;

    let plan = flake_input_plan(root, "ripgrep", None);
    let (engine, _) = stub_engine("codex", false, true, "");

    assert!(!gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
}

#[test]
fn gate_flake_input_allows_claude_with_yes() {
    let tmp = setup_install_root(DEFAULT_CLI_NIX);
    let root = tmp.path();
    write_nix(
        root,
        "flake.nix",
        "{\n  inputs = {\n    nixpkgs.url = \"github:NixOS/nixpkgs\";\n  };\n}\n",
    );
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.yes = true;

    let plan = flake_input_plan(root, "ripgrep", Some("github:nix-community/NUR"));
    let (engine, _) = stub_engine("claude", true, true, "");

    assert!(gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));

    let flake_content =
        fs::read_to_string(root.join("flake.nix")).expect("flake should be readable");
    assert!(flake_content.contains("nur.url = \"github:nix-community/NUR\";"));
}

#[test]
fn gate_flake_input_dry_run_reports_intent_and_allows() {
    let tmp = setup_install_root(DEFAULT_CLI_NIX);
    let root = tmp.path();
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.dry_run = true;

    let plan = flake_input_plan(root, "ripgrep", Some("github:nix-community/NUR"));
    let (engine, _) = stub_engine("claude", true, true, "");

    assert!(gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
}

#[test]
fn gate_flake_input_errors_when_url_missing() {
    let tmp = setup_install_root(DEFAULT_CLI_NIX);
    let root = tmp.path();
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.yes = true;

    let plan = flake_input_plan(root, "ripgrep", None);
    let (engine, _) = stub_engine("claude", true, true, "");

    assert!(!gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
}
