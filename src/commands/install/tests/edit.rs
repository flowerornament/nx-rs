use super::*;

#[test]
fn execute_edit_codex_uses_deterministic_path() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = StubEngine {
        engine_name: "codex",
        supports_flake: false,
        run_edit_calls: calls.clone(),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: "unused".to_string(),
        },
    };

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let edited = fs::read_to_string(root.join("packages/nix/cli.nix"))
        .expect("edited file should be readable");
    assert!(edited.contains("ripgrep"));
}

#[test]
fn execute_edit_claude_uses_deterministic_path() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = StubEngine {
        engine_name: "claude",
        supports_flake: true,
        run_edit_calls: calls.clone(),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: "ok".to_string(),
        },
    };

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let edited = fs::read_to_string(root.join("packages/nix/cli.nix"))
        .expect("target file should be readable");
    assert!(edited.contains("ripgrep"));
}

#[test]
fn execute_edit_claude_is_idempotent_without_ai_fallback() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n    ripgrep\n  ];\n}\n",
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = StubEngine {
        engine_name: "claude",
        supports_flake: true,
        run_edit_calls: calls.clone(),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: "ok".to_string(),
        },
    };

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn execute_edit_claude_falls_back_to_ai_when_deterministic_unsupported() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  services = { };\n}\n",
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = StubEngine {
        engine_name: "claude",
        supports_flake: true,
        run_edit_calls: calls.clone(),
        run_edit_outcome: CommandOutcome {
            success: true,
            output: "ok".to_string(),
        },
    };

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn execute_edit_claude_fallback_failure_returns_false() {
    let tmp = TempDir::new().expect("temp dir should be created");
    let root = tmp.path();
    write_nix(
        root,
        "packages/nix/cli.nix",
        "{ pkgs, ... }:\n{\n  services = { };\n}\n",
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let engine = StubEngine {
        engine_name: "claude",
        supports_flake: true,
        run_edit_calls: calls.clone(),
        run_edit_outcome: CommandOutcome {
            success: false,
            output: "boom".to_string(),
        },
    };

    let ctx = test_context(root);
    let plan = test_plan(root, "ripgrep");

    assert!(!execute_edit(&plan, "packages/nix/cli.nix", &ctx, &engine));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
