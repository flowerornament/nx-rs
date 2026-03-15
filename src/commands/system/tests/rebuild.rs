use super::*;

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
        },
        slots: vec![],
        aliases: HashMap::default(),
        overlays: HashMap::default(),
    };

    let args = PassthroughArgs {
        passthrough: Vec::new(),
    };
    let result = build_rebuild_command_with_manifest("/test", &args, Some(&manifest));
    assert_eq!(result[0], "nixos-rebuild");
    assert_eq!(result[1], "switch");
}
