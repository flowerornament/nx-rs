use super::*;

#[test]
fn source_prefs_defaults_match_no_flags() {
    let args = InstallArgs::default();
    let prefs = source_prefs_from_args(&args);
    assert!(!prefs.bleeding_edge);
    assert!(!prefs.nur);
    assert_eq!(prefs.explicit_target, ExplicitSourceTarget::Any);
    assert!(prefs.force_source.is_none());
}

#[test]
fn source_prefs_maps_cask_flag() {
    let mut args = InstallArgs::default();
    args.target.cask = true;
    let prefs = source_prefs_from_args(&args);
    assert_eq!(prefs.explicit_target, ExplicitSourceTarget::Cask);
}

#[test]
fn source_prefs_cask_wins_when_both_flags_set() {
    let mut args = InstallArgs::default();
    args.target.cask = true;
    args.target.mas = true;
    let prefs = source_prefs_from_args(&args);
    assert_eq!(prefs.explicit_target, ExplicitSourceTarget::Cask);
}

#[test]
fn source_prefs_maps_source_and_bleeding_edge() {
    let mut args = InstallArgs::default();
    args.source.bleeding_edge = true;
    args.source.nur = true;
    args.source.source = Some("unstable".to_string());
    let prefs = source_prefs_from_args(&args);
    assert!(prefs.bleeding_edge);
    assert!(prefs.nur);
    assert_eq!(prefs.force_source.as_deref(), Some("unstable"));
}

#[test]
fn lookup_names_includes_attr_and_language_bare_name() {
    let result = source_result(
        "py-yaml",
        PackageSource::Nxs,
        Some("python3Packages.pyyaml"),
    );

    let names = lookup_names(&result);
    assert_eq!(
        names,
        vec![
            "py-yaml".to_string(),
            "python3Packages.pyyaml".to_string(),
            "pyyaml".to_string()
        ]
    );
}

#[test]
fn find_existing_for_candidates_checks_alternates() {
    let tmp = temp_root();
    let root = tmp.path();

    write_nix(
        root,
        "packages/nix/cli.nix",
        r"{ pkgs }:
[
  ripgrep
]
",
    );

    let candidates = vec![
        source_result("rg", PackageSource::Nxs, Some("fd")),
        source_result("rg", PackageSource::Nxs, Some("ripgrep")),
    ];

    let location = find_existing_for_candidates(&candidates, root)
        .expect("finder should not error")
        .expect("alternate candidate should resolve as installed");
    assert!(
        location.path().ends_with(Path::new("packages/nix/cli.nix")),
        "expected installed location to resolve to packages/nix/cli.nix, got {}",
        location.path().display()
    );
}

#[test]
fn packages_needing_search_prefetch_skips_already_installed_packages() {
    let tmp = temp_root();
    let root = tmp.path();

    write_nix(
        root,
        "packages/nix/cli.nix",
        r"{ pkgs }:
[
  ripgrep
]
",
    );

    let packages = vec!["ripgrep".to_string(), "fd".to_string(), "fd".to_string()];
    let prefetched = packages_needing_search_prefetch(&packages, root);

    assert_eq!(prefetched, vec!["fd".to_string()]);
}
