use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rnix::ast;
use rowan::ast::AstNode;

use super::manifest::{CURRENT_SCHEMA_VERSION, Manifest, PlatformConfig, Slot, SlotKind};
use super::repo_scan::{NixFileScanPolicy, collect_repo_nix_files};

#[derive(Debug, Clone)]
pub struct ScannedRepo {
    pub platform: PlatformConfig,
    pub slots: Vec<Slot>,
}

pub fn scan_repo(repo_root: &Path) -> ScannedRepo {
    let platform = detect_platform(repo_root).unwrap_or_else(Manifest::default_darwin);
    let slots = scan_repo_slots(repo_root);
    ScannedRepo { platform, slots }
}

pub fn manifest_from_scan(
    mut scanned: ScannedRepo,
    repo_root: &Path,
    existing: Option<&Manifest>,
) -> Manifest {
    mark_default_install_slot(&mut scanned.slots, repo_root);

    if let Some(existing) = existing {
        merge_user_annotations(&mut scanned.slots, &existing.slots);
    }

    let aliases = existing.map_or_else(HashMap::new, |m| m.aliases.clone());
    let overlays = existing.map_or_else(HashMap::new, |m| m.overlays.clone());
    let platform = existing.map_or(scanned.platform.clone(), |manifest| {
        merge_platform_config(&manifest.platform, &scanned.platform)
    });

    Manifest {
        schema_version: CURRENT_SCHEMA_VERSION,
        platform,
        slots: scanned.slots,
        aliases,
        overlays,
    }
}

fn merge_platform_config(existing: &PlatformConfig, scanned: &PlatformConfig) -> PlatformConfig {
    if existing.kind == scanned.kind {
        existing.clone()
    } else {
        scanned.clone()
    }
}

fn detect_platform(repo_root: &Path) -> Option<PlatformConfig> {
    let flake_path = repo_root.join("flake.nix");
    let content = fs::read_to_string(&flake_path).ok()?;
    let parse = rnix::Root::parse(&content);
    let root = parse.tree();
    let expr = root.expr()?;

    let outputs = find_attrpath_value_recursive(expr.syntax(), &["outputs"])?;

    let has_ident = |name: &str| -> bool {
        outputs.syntax().descendants().any(|node| {
            ast::Ident::cast(node)
                .and_then(|id| id.ident_token())
                .is_some_and(|tok| tok.text() == name)
        })
    };

    if has_ident("darwinConfigurations") {
        Some(Manifest::default_darwin())
    } else if has_ident("nixosConfigurations") {
        Some(Manifest::default_nixos())
    } else if has_ident("homeConfigurations") {
        Some(Manifest::default_home_manager())
    } else {
        None
    }
}

fn scan_repo_slots(repo_root: &Path) -> Vec<Slot> {
    let mut slots = Vec::new();

    for nix_file in collect_repo_nix_files(repo_root, NixFileScanPolicy::for_repo_manifest_scan()) {
        let Ok(content) = fs::read_to_string(&nix_file) else {
            eprintln!(
                "warning: could not read {}",
                nix_file
                    .strip_prefix(repo_root)
                    .unwrap_or(&nix_file)
                    .display()
            );
            continue;
        };
        let rel_path = nix_file
            .strip_prefix(repo_root)
            .unwrap_or(&nix_file)
            .to_path_buf();

        let parse = rnix::Root::parse(&content);
        if let Some(err) = parse.errors().first() {
            eprintln!("warning: parse error in {}: {err}", rel_path.display());
        }
        let root = parse.tree();
        let Some(expr) = root.expr() else { continue };

        scan_expr_for_slots(expr.syntax(), &rel_path, &mut slots);
    }

    slots.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.attr_path.cmp(&b.attr_path))
    });
    slots.dedup_by(|a, b| a.file == b.file && a.attr_path == b.attr_path);
    slots
}

fn scan_expr_for_slots(node: &rnix::SyntaxNode, rel_path: &Path, slots: &mut Vec<Slot>) {
    for descendant in node.descendants() {
        if let Some(apv) = ast::AttrpathValue::cast(descendant.clone()) {
            let segments = attrpath_segments(&apv);
            let segments_str: Vec<&str> = segments.iter().map(String::as_str).collect();

            match segments_str.as_slice() {
                ["home", "packages"] | ["environment", "systemPackages"] => {
                    slots.push(Slot {
                        kind: SlotKind::NixPackages,
                        file: rel_path.to_path_buf(),
                        attr_path: segments.join("."),
                        tags: vec![],
                        runtime: None,
                        default_for: None,
                    });
                }
                ["homebrew", "brews"] => {
                    slots.push(Slot {
                        kind: SlotKind::HomebrewList,
                        file: rel_path.to_path_buf(),
                        attr_path: "homebrew.brews".to_string(),
                        tags: vec!["brews".to_string()],
                        runtime: None,
                        default_for: None,
                    });
                }
                ["homebrew", "casks"] => {
                    slots.push(Slot {
                        kind: SlotKind::HomebrewList,
                        file: rel_path.to_path_buf(),
                        attr_path: "homebrew.casks".to_string(),
                        tags: vec!["casks".to_string()],
                        runtime: None,
                        default_for: None,
                    });
                }
                ["homebrew", "masApps"] | ["masApps"] => {
                    slots.push(Slot {
                        kind: SlotKind::MasApps,
                        file: rel_path.to_path_buf(),
                        attr_path: segments.join("."),
                        tags: vec![],
                        runtime: None,
                        default_for: None,
                    });
                }
                _ => {}
            }

            if is_service_slot_attrpath(&segments_str) {
                slots.push(Slot {
                    kind: SlotKind::Services,
                    file: rel_path.to_path_buf(),
                    attr_path: segments.join("."),
                    tags: vec![],
                    runtime: None,
                    default_for: None,
                });
            }
        }

        if let Some(apply) = ast::Apply::cast(descendant.clone())
            && let Some((runtime, method)) = detect_with_packages(&apply)
        {
            slots.push(Slot {
                kind: SlotKind::WithPackages,
                file: rel_path.to_path_buf(),
                attr_path: format!("{runtime}.{method}"),
                tags: vec![],
                runtime: Some(runtime),
                default_for: None,
            });
        }
    }
}

fn is_service_slot_attrpath(segments: &[&str]) -> bool {
    matches!(
        segments,
        ["launchd", "agents", ..] | ["launchd", "user", "agents", ..]
    )
}

fn detect_with_packages(apply: &ast::Apply) -> Option<(String, String)> {
    let func = apply.lambda()?;
    let ast::Expr::Select(sel) = func else {
        return None;
    };

    let attrs: Vec<String> = sel
        .attrpath()?
        .attrs()
        .filter_map(|a| match a {
            ast::Attr::Ident(id) => Some(id.ident_token()?.text().to_string()),
            _ => None,
        })
        .collect();

    if attrs.last().map(String::as_str) != Some("withPackages") {
        return None;
    }

    let expr_name = match sel.expr()? {
        ast::Expr::Ident(id) => Some(id.ident_token()?.text().to_string()),
        _ => None,
    };

    let runtime_attrs = &attrs[..attrs.len() - 1];
    let mut segments: Vec<&str> = Vec::new();

    if let Some(ref name) = expr_name
        && name != "pkgs"
        && name != "self"
    {
        segments.push(name);
    }
    segments.extend(runtime_attrs.iter().map(String::as_str));

    if segments.is_empty() {
        return None;
    }

    Some((segments.join("."), "withPackages".to_string()))
}

fn attrpath_segments(apv: &ast::AttrpathValue) -> Vec<String> {
    apv.attrpath()
        .into_iter()
        .flat_map(|p| p.attrs())
        .filter_map(|attr| match attr {
            ast::Attr::Ident(id) => Some(id.ident_token()?.text().to_string()),
            ast::Attr::Str(s) => {
                let parts: String = s
                    .normalized_parts()
                    .into_iter()
                    .filter_map(|part| match part {
                        ast::InterpolPart::Literal(lit) => Some(lit),
                        ast::InterpolPart::Interpolation(_) => None,
                    })
                    .collect();
                if parts.is_empty() { None } else { Some(parts) }
            }
            ast::Attr::Dynamic(_) => None,
        })
        .collect()
}

fn find_attrpath_value_recursive(
    node: &rnix::SyntaxNode,
    target: &[&str],
) -> Option<ast::AttrpathValue> {
    for descendant in node.descendants() {
        if let Some(apv) = ast::AttrpathValue::cast(descendant) {
            let segments = attrpath_segments(&apv);
            let segments_str: Vec<&str> = segments.iter().map(String::as_str).collect();
            if segments_str == target {
                return Some(apv);
            }
        }
    }
    None
}

fn mark_default_install_slot(slots: &mut [Slot], repo_root: &Path) {
    if slots
        .iter()
        .any(|s| s.default_for.as_ref().is_some_and(|df| !df.is_empty()))
    {
        return;
    }

    let best = slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == SlotKind::NixPackages)
        .max_by_key(|(_, s)| count_list_items(&repo_root.join(&s.file)));

    if let Some((idx, _)) = best {
        slots[idx].default_for = Some(vec!["install".to_string()]);
    }
}

fn count_list_items(path: &Path) -> usize {
    let Ok(content) = fs::read_to_string(path) else {
        return 0;
    };
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("//")
        })
        .count()
}

fn merge_user_annotations(new_slots: &mut [Slot], existing_slots: &[Slot]) {
    for new_slot in new_slots.iter_mut() {
        if let Some(existing) = existing_slots.iter().find(|existing| {
            existing.file == new_slot.file && existing.attr_path == new_slot.attr_path
        }) {
            for tag in &existing.tags {
                if !new_slot.tags.contains(tag) {
                    new_slot.tags.push(tag.clone());
                }
            }
            if new_slot.runtime.is_none() {
                new_slot.runtime.clone_from(&existing.runtime);
            }
            if new_slot.default_for.is_none() {
                new_slot.default_for.clone_from(&existing.default_for);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::manifest::PlatformKind;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_file(root: &Path, rel_path: &str, content: &str) {
        let full = root.join(rel_path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
    }

    #[test]
    fn scan_finds_home_packages() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "home/packages.nix",
            r"{ pkgs, ... }: {
  home.packages = with pkgs; [
    ripgrep
    fd
  ];
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(
            slots
                .iter()
                .any(|slot| slot.kind == SlotKind::NixPackages && slot.attr_path == "home.packages")
        );
    }

    #[test]
    fn scan_finds_system_packages() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "system/packages.nix",
            r"{ pkgs, ... }: {
  environment.systemPackages = with pkgs; [
    git
  ];
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| {
            slot.kind == SlotKind::NixPackages && slot.attr_path == "environment.systemPackages"
        }));
    }

    #[test]
    fn scan_finds_homebrew_brews_and_casks() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/homebrew.nix",
            r#"{ ... }: {
  homebrew.brews = [
    "htop"
  ];
  homebrew.casks = [
    "firefox"
  ];
}"#,
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(
            slots
                .iter()
                .any(|slot| slot.kind == SlotKind::HomebrewList
                    && slot.attr_path == "homebrew.brews")
        );
        assert!(
            slots
                .iter()
                .any(|slot| slot.kind == SlotKind::HomebrewList
                    && slot.attr_path == "homebrew.casks")
        );
    }

    #[test]
    fn scan_finds_mas_apps() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "system/darwin.nix",
            r#"{ ... }: {
  homebrew.masApps = {
    "Xcode" = 497799835;
  };
}"#,
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| slot.kind == SlotKind::MasApps));
    }

    #[test]
    fn scan_finds_with_packages() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/languages.nix",
            r"{ pkgs, ... }: {
  home.packages = [
    (pkgs.python3.withPackages (ps: with ps; [ requests pyyaml ]))
  ];
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| {
            slot.kind == SlotKind::WithPackages && slot.runtime == Some("python3".to_string())
        }));
    }

    #[test]
    fn scan_finds_launchd_services() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "home/services.nix",
            r"{ ... }: {
  launchd.agents.sops-nix = {
    config = {};
  };
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| slot.kind == SlotKind::Services));
    }

    #[test]
    fn scan_finds_launchd_user_services() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "home/services.nix",
            r"{ ... }: {
  launchd.user.agents.syncthing = {
    config = {};
  };
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| {
            slot.kind == SlotKind::Services && slot.attr_path == "launchd.user.agents.syncthing"
        }));
    }

    #[test]
    fn scan_ignores_generic_services_attrpaths() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "system/darwin.nix",
            r"{ ... }: {
  services.yabai.enable = true;
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(!slots.iter().any(|slot| slot.kind == SlotKind::Services));
    }

    #[test]
    fn scan_ignores_systemd_service_attrpaths() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "hosts/workstation.nix",
            r#"{ ... }: {
  systemd.services.demo = {
    wantedBy = [ "multi-user.target" ];
  };
}"#,
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(!slots.iter().any(|slot| slot.kind == SlotKind::Services));
    }

    #[test]
    fn scan_skips_hidden_directories() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            ".git/config.nix",
            r"{ ... }: { home.packages = [ git ]; }",
        );
        write_file(
            tmp.path(),
            "packages/cli.nix",
            r"{ pkgs, ... }: { home.packages = with pkgs; [ ripgrep ]; }",
        );

        let files = collect_repo_nix_files(tmp.path(), NixFileScanPolicy::for_repo_manifest_scan());
        assert!(
            files
                .iter()
                .all(|file| !file.to_string_lossy().contains(".git"))
        );
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn detect_platform_darwin() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            r"{
  outputs = { self, nix-darwin, ... }: {
    darwinConfigurations.myhost = nix-darwin.lib.darwinSystem {};
  };
}",
        );

        let platform = detect_platform(tmp.path()).unwrap();
        assert_eq!(platform.kind, PlatformKind::Darwin);
    }

    #[test]
    fn detect_platform_nixos() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            r"{
  outputs = { self, nixpkgs, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {};
  };
}",
        );

        let platform = detect_platform(tmp.path()).unwrap();
        assert_eq!(platform.kind, PlatformKind::NixOS);
    }

    #[test]
    fn detect_platform_home_manager() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            r"{
  outputs = { self, home-manager, ... }: {
    homeConfigurations.user = home-manager.lib.homeManagerConfiguration {};
  };
}",
        );

        let platform = detect_platform(tmp.path()).unwrap();
        assert_eq!(platform.kind, PlatformKind::HomeManager);
    }

    #[test]
    fn detect_platform_missing_flake_returns_none() {
        let tmp = TempDir::new().unwrap();
        assert!(detect_platform(tmp.path()).is_none());
    }

    #[test]
    fn mark_default_picks_largest_nix_slot() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/small.nix",
            "{ pkgs }: {\n  home.packages = [ a ];\n}\n",
        );
        write_file(
            tmp.path(),
            "packages/big.nix",
            "{ pkgs }: {\n  home.packages = [\n    a\n    b\n    c\n    d\n    e\n  ];\n}\n",
        );

        let mut slots = vec![
            Slot {
                kind: SlotKind::NixPackages,
                file: PathBuf::from("packages/small.nix"),
                attr_path: "home.packages".to_string(),
                tags: vec![],
                runtime: None,
                default_for: None,
            },
            Slot {
                kind: SlotKind::NixPackages,
                file: PathBuf::from("packages/big.nix"),
                attr_path: "home.packages".to_string(),
                tags: vec![],
                runtime: None,
                default_for: None,
            },
        ];

        mark_default_install_slot(&mut slots, tmp.path());
        assert_eq!(slots[1].default_for, Some(vec!["install".to_string()]));
        assert!(slots[0].default_for.is_none());
    }

    #[test]
    fn merge_preserves_user_tags() {
        let existing = vec![Slot {
            kind: SlotKind::NixPackages,
            file: PathBuf::from("packages/cli.nix"),
            attr_path: "home.packages".to_string(),
            tags: vec!["my-custom-tag".to_string()],
            runtime: None,
            default_for: Some(vec!["install".to_string()]),
        }];

        let mut new_slots = vec![Slot {
            kind: SlotKind::NixPackages,
            file: PathBuf::from("packages/cli.nix"),
            attr_path: "home.packages".to_string(),
            tags: vec![],
            runtime: None,
            default_for: None,
        }];

        merge_user_annotations(&mut new_slots, &existing);
        assert!(new_slots[0].tags.contains(&"my-custom-tag".to_string()));
        assert_eq!(new_slots[0].default_for, Some(vec!["install".to_string()]));
    }

    #[test]
    fn merge_preserves_runtime() {
        let existing = vec![Slot {
            kind: SlotKind::WithPackages,
            file: PathBuf::from("packages/languages.nix"),
            attr_path: "python3.withPackages".to_string(),
            tags: vec![],
            runtime: Some("python3".to_string()),
            default_for: None,
        }];

        let mut new_slots = vec![Slot {
            kind: SlotKind::WithPackages,
            file: PathBuf::from("packages/languages.nix"),
            attr_path: "python3.withPackages".to_string(),
            tags: vec![],
            runtime: None,
            default_for: None,
        }];

        merge_user_annotations(&mut new_slots, &existing);
        assert_eq!(new_slots[0].runtime, Some("python3".to_string()));
    }

    #[test]
    fn manifest_from_scan_preserves_custom_platform_settings_when_kind_matches() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/cli.nix",
            "{ pkgs, ... }: { home.packages = with pkgs; [ ripgrep ]; }",
        );

        let scanned = ScannedRepo {
            platform: Manifest::default_darwin(),
            slots: vec![Slot {
                kind: SlotKind::NixPackages,
                file: PathBuf::from("packages/cli.nix"),
                attr_path: "home.packages".to_string(),
                tags: vec![],
                runtime: None,
                default_for: None,
            }],
        };
        let existing = Manifest {
            schema_version: CURRENT_SCHEMA_VERSION,
            platform: PlatformConfig {
                kind: PlatformKind::Darwin,
                rebuild_command: "custom-rebuild".to_string(),
                sudo: false,
                flake_root: "hosts/workstation".to_string(),
            },
            slots: Vec::new(),
            aliases: HashMap::new(),
            overlays: HashMap::new(),
        };

        let merged = manifest_from_scan(scanned, tmp.path(), Some(&existing));

        assert_eq!(merged.platform.rebuild_command, "custom-rebuild");
        assert!(!merged.platform.sudo);
        assert_eq!(merged.platform.flake_root, "hosts/workstation");
    }

    #[test]
    fn scan_fixture_repo_finds_all_slot_kinds() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/system/repo_base");
        if !fixture.exists() {
            return;
        }

        let scanned = scan_repo(&fixture);

        assert_eq!(scanned.platform.kind, PlatformKind::Darwin);

        let has_kind = |kind: SlotKind| scanned.slots.iter().any(|slot| slot.kind == kind);
        assert!(has_kind(SlotKind::NixPackages));
        assert!(has_kind(SlotKind::HomebrewList));
        assert!(has_kind(SlotKind::MasApps));
        assert!(has_kind(SlotKind::WithPackages));
        assert!(has_kind(SlotKind::Services));

        let file_matches = |slot: &Slot, file: &str, attr: &str| {
            slot.file.as_path() == Path::new(file) && slot.attr_path == attr
        };
        assert!(scanned.slots.iter().any(|slot| file_matches(
            slot,
            "packages/nix/cli.nix",
            "home.packages"
        )));
        assert!(scanned.slots.iter().any(|slot| file_matches(
            slot,
            "packages/homebrew/brews.nix",
            "homebrew.brews"
        )));
        assert!(scanned.slots.iter().any(|slot| file_matches(
            slot,
            "packages/homebrew/casks.nix",
            "homebrew.casks"
        )));

        let mut seen = std::collections::HashSet::new();
        for slot in &scanned.slots {
            let key = (slot.file.clone(), slot.attr_path.clone());
            assert!(seen.insert(key));
        }
    }

    #[test]
    fn scan_finds_with_packages_implicit_scope() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/languages.nix",
            r"{ pkgs, ... }: {
  home.packages = [
    (python3.withPackages (ps: with ps; [ requests pyyaml ]))
  ];
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| {
            slot.kind == SlotKind::WithPackages && slot.runtime == Some("python3".to_string())
        }));
    }

    #[test]
    fn scan_finds_with_packages_lua_implicit() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/languages.nix",
            r"{ pkgs, ... }: {
  home.packages = [
    (lua5_4.withPackages (ps: [ lpeg ]))
  ];
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| {
            slot.kind == SlotKind::WithPackages && slot.runtime == Some("lua5_4".to_string())
        }));
    }

    #[test]
    fn scan_finds_haskell_with_packages() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/languages.nix",
            r"{ pkgs, ... }: {
  home.packages = [
    (haskellPackages.ghc.withPackages (ps: [ ps.pandoc ]))
  ];
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| {
            slot.kind == SlotKind::WithPackages
                && slot.runtime == Some("haskellPackages.ghc".to_string())
        }));
    }

    #[test]
    fn scan_finds_haskell_with_pkgs_prefix() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "packages/languages.nix",
            r"{ pkgs, ... }: {
  home.packages = [
    (pkgs.haskellPackages.ghc.withPackages (ps: [ ps.pandoc ]))
  ];
}",
        );

        let slots = scan_repo_slots(tmp.path());
        assert!(slots.iter().any(|slot| {
            slot.kind == SlotKind::WithPackages
                && slot.runtime == Some("haskellPackages.ghc".to_string())
        }));
    }
}
