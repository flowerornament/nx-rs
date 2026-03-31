use super::*;

#[test]
fn start_install_resolution_completes_when_package_already_installed() {
    let tmp = setup_install_root("{ pkgs, ... }:\n[\n  ripgrep\n]\n");
    let root = tmp.path();
    let ctx = test_context(root);
    let args = install_args_template();
    let mut cache = None;

    let state = start_install_resolution("ripgrep", &args, &ctx, &mut cache);
    assert!(matches!(state, InstallStart::Completed));
}

#[test]
fn prepare_install_phase_stops_when_flake_input_engine_is_unsupported() {
    let tmp = setup_install_root("{ pkgs, ... }:\n[\n  bat\n]\n");
    let root = tmp.path();
    let ctx = test_context(root);
    let args = install_args_template();
    let mut result = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"));
    result.requires_flake_mod = true;
    result.flake_url = Some("github:nix-community/NUR".to_string());

    let (engine, _) = stub_engine("codex", false, true, "");

    let routing_context = test_routing_context();
    let prepared = prepare_install_phase("ripgrep", result, &args, &ctx, &engine, &routing_context);
    assert!(prepared.is_none());
}

#[test]
fn platform_resolution_uses_primary_when_available() {
    let primary = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"));
    let candidates = vec![primary.clone()];
    let mut checks = 0usize;

    let outcome = resolve_platform_candidate_with(&primary, &candidates, |_attr| {
        checks += 1;
        (true, None)
    })
    .expect("platform resolution should succeed");

    match outcome {
        PlatformResolution::Primary(sr) => {
            assert_eq!(sr.attr.as_deref(), Some("ripgrep"));
        }
        PlatformResolution::Fallback { .. } => panic!("expected primary candidate"),
    }
    assert_eq!(checks, 1);
}

#[test]
fn platform_resolution_uses_same_source_fallback_when_primary_unavailable() {
    let primary = source_result(
        "py-yaml",
        PackageSource::Nxs,
        Some("python3Packages.aspy-yaml"),
    );
    let fallback = source_result(
        "py-yaml",
        PackageSource::Nxs,
        Some("python3Packages.pyyaml"),
    );
    let homebrew = source_result("py-yaml", PackageSource::Homebrew, Some("pyyaml"));
    let candidates = vec![primary.clone(), homebrew, fallback];

    let outcome = resolve_platform_candidate_with(&primary, &candidates, |attr| {
        if attr == "python3Packages.aspy-yaml" {
            return (
                false,
                Some("not available on aarch64-darwin (only: x86_64-linux)".to_string()),
            );
        }
        (true, None)
    })
    .expect("fallback should resolve");

    match outcome {
        PlatformResolution::Fallback { candidate, reason } => {
            assert_eq!(candidate.attr.as_deref(), Some("python3Packages.pyyaml"));
            assert!(reason.contains("not available on aarch64-darwin"));
        }
        PlatformResolution::Primary(_) => panic!("expected fallback candidate"),
    }
}

#[test]
fn platform_resolution_errors_without_same_source_fallback() {
    let primary = source_result("roc", PackageSource::Nxs, Some("roc"));
    let other_source = source_result("roc", PackageSource::Homebrew, Some("roc"));
    let candidates = vec![primary.clone(), other_source];

    let outcome = resolve_platform_candidate_with(&primary, &candidates, |attr| {
        if attr == "roc" {
            return (
                false,
                Some("not available on aarch64-darwin (only: x86_64-linux)".to_string()),
            );
        }
        (true, None)
    });

    let err = outcome.expect_err("resolution should fail");
    assert!(err.contains("not available on aarch64-darwin"));
}

#[test]
fn platform_resolution_skips_unavailable_same_source_and_uses_later_fallback() {
    let primary = source_result(
        "py-yaml",
        PackageSource::Nxs,
        Some("python3Packages.aspy-yaml"),
    );
    let unavailable_fallback = source_result(
        "py-yaml",
        PackageSource::Nxs,
        Some("python3Packages.bad-alt"),
    );
    let available_fallback = source_result(
        "py-yaml",
        PackageSource::Nxs,
        Some("python3Packages.pyyaml"),
    );
    let candidates = vec![
        primary.clone(),
        unavailable_fallback,
        available_fallback.clone(),
    ];

    let outcome = resolve_platform_candidate_with(&primary, &candidates, |attr| match attr {
        "python3Packages.aspy-yaml" | "python3Packages.bad-alt" => (
            false,
            Some("not available on aarch64-darwin (only: x86_64-linux)".to_string()),
        ),
        _ => (true, None),
    })
    .expect("later fallback should resolve");

    match outcome {
        PlatformResolution::Fallback { candidate, reason } => {
            assert_eq!(candidate.attr, available_fallback.attr);
            assert!(reason.contains("not available on aarch64-darwin"));
        }
        PlatformResolution::Primary(_) => panic!("expected fallback candidate"),
    }
}
