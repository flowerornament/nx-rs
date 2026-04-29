use super::*;

#[test]
fn rebuild_command_includes_base_args() {
    let args = RebuildArgs {
        preflight: false,
        timing: false,
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
    let args = RebuildArgs {
        preflight: false,
        timing: false,
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

#[test]
fn rebuild_command_uses_manifest_rebuild_command() {
    use std::collections::HashMap;

    use crate::domain::manifest::{Manifest, PlatformConfig, PlatformKind};

    let manifest = Manifest {
        schema_version: 1,
        platform: PlatformConfig {
            kind: PlatformKind::NixOS,
            rebuild_command: "nixos-rebuild".to_string(),
            sudo: true,
            flake_root: ".".to_string(),
            split_rebuild: false,
        },
        slots: vec![],
        aliases: HashMap::default(),
        overlays: HashMap::default(),
    };

    let args = RebuildArgs {
        preflight: false,
        timing: false,
        passthrough: Vec::new(),
    };
    let result = build_rebuild_command_with_manifest("/test", &args, Some(&manifest));
    assert_eq!(result[0], "nixos-rebuild");
    assert_eq!(result[1], "switch");
}

#[test]
fn split_darwin_json_parser_extracts_system_output() {
    let output = r#"[{"outputs":{"out":"/nix/store/system-config"}}]"#;
    assert_eq!(
        parse_system_config_path(output),
        Some("/nix/store/system-config".to_string())
    );
}

#[test]
fn split_darwin_json_parser_rejects_missing_output() {
    assert_eq!(parse_system_config_path("[]"), None);
    assert_eq!(
        parse_system_config_path(r#"[{"outputs":{"bin":"/nix/store/bin"}}]"#),
        None
    );
    assert_eq!(
        parse_system_config_path(
            r#"[{"outputs":{"out":"/nix/store/one"}},{"outputs":{"out":"/nix/store/two"}}]"#
        ),
        None
    );
    assert_eq!(parse_system_config_path("not-json"), None);
}

#[test]
fn split_nix_build_raises_file_descriptor_limit() {
    let (program, args) =
        split_nix_build_command("git+file:///repo#darwinConfigurations.host.system");

    assert_eq!(
        (program, args),
        (
            "bash".to_string(),
            vec![
                "-lc".to_string(),
                "ulimit -n 65536 2>/dev/null; exec \"$@\"".to_string(),
                "nx-nix-with-ulimit".to_string(),
                "nix".to_string(),
                "build".to_string(),
                "--json".to_string(),
                "--no-link".to_string(),
                "git+file:///repo#darwinConfigurations.host.system".to_string(),
            ],
        )
    );
}

#[test]
fn split_darwin_defaults_on_for_darwin_and_allows_opt_out() {
    use std::collections::HashMap;

    use crate::domain::manifest::{Manifest, PlatformConfig, PlatformKind};

    let args = RebuildArgs {
        preflight: false,
        timing: false,
        passthrough: Vec::new(),
    };
    let manifest = Manifest {
        schema_version: 1,
        platform: PlatformConfig {
            kind: PlatformKind::Darwin,
            rebuild_command: DARWIN_REBUILD.to_string(),
            sudo: true,
            flake_root: ".".to_string(),
            split_rebuild: Manifest::default_darwin().split_rebuild,
        },
        slots: vec![],
        aliases: HashMap::default(),
        overlays: HashMap::default(),
    };

    assert!(should_use_split_darwin(&args, Some(&manifest)));

    let opted_out = Manifest {
        platform: PlatformConfig {
            split_rebuild: false,
            ..manifest.platform.clone()
        },
        ..manifest.clone()
    };
    assert!(!should_use_split_darwin(&args, Some(&opted_out)));

    let passthrough = RebuildArgs {
        passthrough: vec!["--show-trace".to_string()],
        ..args
    };
    assert!(!should_use_split_darwin(&passthrough, Some(&manifest)));
}

#[test]
fn fixed_output_hash_parser_extracts_specified_and_got_hashes() {
    let output = "\
error: hash mismatch in fixed-output derivation '/nix/store/example-npm-deps.drv':
         specified: sha256-D6HjBFzg2HxHZNjm8XMSHCuhMqXdJWKpEtfUc5rkYxo=
            got:    sha256-oNLh9Oc29XvLzMqMMmIbkTNz88zdrvyrANaNOFucmts=
";

    assert_eq!(
        parse_fixed_output_hash_mismatch(output),
        Some(FixedOutputHashMismatch {
            specified: "sha256-D6HjBFzg2HxHZNjm8XMSHCuhMqXdJWKpEtfUc5rkYxo=".to_string(),
            got: "sha256-oNLh9Oc29XvLzMqMMmIbkTNz88zdrvyrANaNOFucmts=".to_string(),
        })
    );
}

#[test]
fn fixed_output_hash_repair_updates_unique_clean_tracked_nix_file() {
    let tmp = init_git_repo();
    let root = tmp.path();
    let rel_path = std::path::Path::new("home/agent-sync.nix");
    let full_path = root.join(rel_path);
    fs::create_dir_all(full_path.parent().unwrap()).unwrap();
    fs::write(
        &full_path,
        "# nx: agent sync\n  npmDepsHash = \"sha256-old\";\n",
    )
    .unwrap();
    run_captured_command("git", &["add", "home/agent-sync.nix"], Some(root)).unwrap();
    run_captured_command("git", &["commit", "-m", "add agent sync"], Some(root)).unwrap();

    let targets = find_fixed_output_hash_targets(root, "sha256-old").unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].path, rel_path);
    assert_eq!(targets[0].line_number, 2);
    assert_eq!(targets[0].column_number, 18);
    assert!(path_is_clean(root, rel_path));

    apply_fixed_output_hash_repair(
        root,
        &targets[0],
        &FixedOutputHashMismatch {
            specified: "sha256-old".to_string(),
            got: "sha256-new".to_string(),
        },
    )
    .unwrap();

    let updated = fs::read_to_string(full_path).unwrap();
    assert!(updated.contains("npmDepsHash = \"sha256-new\";"));
    assert!(!updated.contains("sha256-old"));
    assert!(!path_is_clean(root, rel_path));
}

#[test]
fn fixed_output_hash_target_allows_plain_sha256_assignment() {
    let tmp = init_git_repo();
    let root = tmp.path();
    fs::write(root.join("pkg.nix"), "sha256 = \"sha256-old\";\n").unwrap();
    run_captured_command("git", &["add", "pkg.nix"], Some(root)).unwrap();
    run_captured_command("git", &["commit", "-m", "add package"], Some(root)).unwrap();

    let targets = find_fixed_output_hash_targets(root, "sha256-old").unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].path, std::path::Path::new("pkg.nix"));
    assert_eq!(targets[0].line_number, 1);
    assert_eq!(targets[0].column_number, 11);
}

#[test]
fn fixed_output_hash_target_counts_each_exact_occurrence() {
    let tmp = init_git_repo();
    let root = tmp.path();
    fs::write(
        root.join("pkg.nix"),
        "# old hash sha256-old\nsha256 = \"sha256-old\";\n",
    )
    .unwrap();
    run_captured_command("git", &["add", "pkg.nix"], Some(root)).unwrap();
    run_captured_command("git", &["commit", "-m", "add package"], Some(root)).unwrap();

    let targets = find_fixed_output_hash_targets(root, "sha256-old").unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].line_number, 1);
    assert_eq!(targets[1].line_number, 2);
}
