use super::*;

#[test]
fn gate_flake_input_refuses_codex_engine() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
    );
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.yes = true;

    let mut plan = test_plan(root, "ripgrep");
    plan.source_result.requires_flake_mod = true;

    let engine = StubEngine {
        engine_name: "codex",
        supports_flake: false,
        run_edit_calls: Arc::new(AtomicUsize::new(0)),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: String::new(),
        },
    };

    assert!(!gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
}

#[test]
fn gate_flake_input_allows_claude_with_yes() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
    );
    write_nix(
        root,
        "flake.nix",
        "{\n  inputs = {\n    nixpkgs.url = \"github:NixOS/nixpkgs\";\n  };\n}\n",
    );
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.yes = true;

    let mut plan = test_plan(root, "ripgrep");
    plan.source_result.requires_flake_mod = true;
    plan.source_result.flake_url = Some("github:nix-community/NUR".to_string());

    let engine = StubEngine {
        engine_name: "claude",
        supports_flake: true,
        run_edit_calls: Arc::new(AtomicUsize::new(0)),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: String::new(),
        },
    };

    assert!(gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));

    let flake_content =
        fs::read_to_string(root.join("flake.nix")).expect("flake should be readable");
    assert!(flake_content.contains("nur.url = \"github:nix-community/NUR\";"));
}

#[test]
fn gate_flake_input_dry_run_reports_intent_and_allows() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
    );
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.dry_run = true;

    let mut plan = test_plan(root, "ripgrep");
    plan.source_result.requires_flake_mod = true;
    plan.source_result.flake_url = Some("github:nix-community/NUR".to_string());

    let engine = StubEngine {
        engine_name: "claude",
        supports_flake: true,
        run_edit_calls: Arc::new(AtomicUsize::new(0)),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: String::new(),
        },
    };

    assert!(gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
}

#[test]
fn gate_flake_input_errors_when_url_missing() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
    );
    let ctx = test_context(root);
    let mut args = install_args_template();
    args.flow.yes = true;

    let mut plan = test_plan(root, "ripgrep");
    plan.source_result.requires_flake_mod = true;
    plan.source_result.flake_url = None;

    let engine = StubEngine {
        engine_name: "claude",
        supports_flake: true,
        run_edit_calls: Arc::new(AtomicUsize::new(0)),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: String::new(),
        },
    };

    assert!(!gate_flake_input("ripgrep", &plan, &args, &ctx, &engine));
}
