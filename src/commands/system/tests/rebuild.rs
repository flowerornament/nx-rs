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
fn split_darwin_is_opt_in_and_darwin_only() {
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
            split_rebuild: true,
        },
        slots: vec![],
        aliases: HashMap::default(),
        overlays: HashMap::default(),
    };

    assert!(should_use_split_darwin(&args, Some(&manifest)));

    let passthrough = RebuildArgs {
        passthrough: vec!["--show-trace".to_string()],
        ..args
    };
    assert!(!should_use_split_darwin(&passthrough, Some(&manifest)));
}
