use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rnix::ast;
use rowan::ast::AstNode;
use walkdir::WalkDir;

use crate::commands::context::AppContext;
use crate::domain::manifest::{CURRENT_SCHEMA_VERSION, Manifest, PlatformConfig, Slot, SlotKind};
use crate::output::printer::Printer;

// --- Public entry point

pub fn cmd_init(refresh: bool, ctx: &AppContext) -> i32 {
    ctx.printer.action(if refresh {
        "Rescanning repository"
    } else {
        "Scanning repository"
    });

    let existing = if refresh {
        match Manifest::load(&ctx.repo_root) {
            Ok(m) => m,
            Err(err) => {
                ctx.printer
                    .error(&format!("failed to load existing manifest: {err:#}"));
                return 1;
            }
        }
    } else {
        None
    };

    let platform = detect_platform(&ctx.repo_root).unwrap_or_else(Manifest::default_darwin);
    let mut slots = scan_repo_slots(&ctx.repo_root);

    // Pick the largest NixPackages slot as default install target if none is tagged.
    mark_default_install_slot(&mut slots, &ctx.repo_root);

    // On refresh, preserve user-added tags from the existing manifest.
    if let Some(ref existing) = existing {
        merge_user_annotations(&mut slots, &existing.slots);
    }

    let aliases = existing
        .as_ref()
        .map_or_else(HashMap::new, |m| m.aliases.clone());
    let overlays = existing
        .as_ref()
        .map_or_else(HashMap::new, |m| m.overlays.clone());

    let manifest = Manifest {
        schema_version: CURRENT_SCHEMA_VERSION,
        platform,
        slots,
        aliases,
        overlays,
    };

    print_summary(&manifest, &ctx.printer);

    if !Printer::confirm("Write .nx/manifest.toml?", true) {
        Printer::detail("Cancelled.");
        return 0;
    }

    if let Err(err) = manifest.save(&ctx.repo_root) {
        ctx.printer
            .error(&format!("failed to write manifest: {err:#}"));
        return 1;
    }

    ctx.printer.success("Manifest written to .nx/manifest.toml");
    0
}

// --- Platform detection

fn detect_platform(repo_root: &Path) -> Option<PlatformConfig> {
    let flake_path = repo_root.join("flake.nix");
    let content = fs::read_to_string(&flake_path).ok()?;
    let parse = rnix::Root::parse(&content);
    let root = parse.tree();
    let expr = root.expr()?;

    let outputs = find_attrpath_value_recursive(expr.syntax(), &["outputs"])?;

    // Walk AST identifiers to avoid matching comments or string literals.
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

// --- Slot scanning

fn scan_repo_slots(repo_root: &Path) -> Vec<Slot> {
    let mut slots = Vec::new();

    for nix_file in collect_all_nix_files(repo_root) {
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
    // Deduplicate by (file, attr_path) — multiple AST walks can yield duplicates.
    slots.dedup_by(|a, b| a.file == b.file && a.attr_path == b.attr_path);
    slots
}

fn scan_expr_for_slots(node: &rnix::SyntaxNode, rel_path: &Path, slots: &mut Vec<Slot>) {
    for descendant in node.descendants() {
        // Check for AttrpathValue (assignments like `home.packages = [...]`)
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

            // Check for launchd/systemd services in deeper paths
            if segments_str.len() >= 3
                && matches!(
                    segments_str[..2],
                    ["launchd", "agents" | "daemons"] | ["services", _] | ["systemd", "services"]
                )
            {
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

        // Check for withPackages calls
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

    // Build runtime from expr + attrpath segments (minus "withPackages").
    //
    // rnix AST splits differently depending on prefix depth:
    //   `python3.withPackages`                → expr=Ident("python3"), attrs=["withPackages"]
    //   `pkgs.python3.withPackages`           → expr=Ident("pkgs"),   attrs=["python3", "withPackages"]
    //   `haskellPackages.ghc.withPackages`    → expr=Ident("haskellPackages"), attrs=["ghc", "withPackages"]
    //   `pkgs.haskellPackages.ghc.withPackages` → expr=Ident("pkgs"), attrs=["haskellPackages", "ghc", "withPackages"]
    let expr_name = match sel.expr()? {
        ast::Expr::Ident(id) => Some(id.ident_token()?.text().to_string()),
        _ => None,
    };

    let runtime_attrs = &attrs[..attrs.len() - 1]; // everything before "withPackages"
    let mut segments: Vec<&str> = Vec::new();

    if let Some(ref name) = expr_name
        && name != "pkgs"
        && name != "self"
    {
        segments.push(name);
    }
    segments.extend(runtime_attrs.iter().map(String::as_str));

    if segments.is_empty() {
        return None; // bare `pkgs.withPackages` doesn't make sense
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

// --- File collection

fn collect_all_nix_files(repo_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in WalkDir::new(repo_root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();

        // Skip hidden dirs by checking the relative path components
        let rel = path.strip_prefix(repo_root).unwrap_or(path);
        if rel
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("nix") {
            continue;
        }
        files.push(path.to_path_buf());
    }
    files.sort();
    files
}

// --- Default slot marking

fn mark_default_install_slot(slots: &mut [Slot], repo_root: &Path) {
    // Skip if any slot already has default_for set
    if slots
        .iter()
        .any(|s| s.default_for.as_ref().is_some_and(|df| !df.is_empty()))
    {
        return;
    }

    // Find the NixPackages slot with the most items
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
    // Quick heuristic: count non-comment, non-empty lines in list blocks
    content
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with("//")
        })
        .count()
}

// --- Merge with existing manifest

fn merge_user_annotations(new_slots: &mut [Slot], existing_slots: &[Slot]) {
    for new_slot in new_slots.iter_mut() {
        if let Some(existing) = existing_slots
            .iter()
            .find(|e| e.file == new_slot.file && e.attr_path == new_slot.attr_path)
        {
            // Preserve user-added tags
            for tag in &existing.tags {
                if !new_slot.tags.contains(tag) {
                    new_slot.tags.push(tag.clone());
                }
            }
            // Preserve runtime
            if new_slot.runtime.is_none() {
                new_slot.runtime.clone_from(&existing.runtime);
            }
            // Preserve default_for
            if new_slot.default_for.is_none() {
                new_slot.default_for.clone_from(&existing.default_for);
            }
        }
    }
}

// --- Output

fn print_summary(manifest: &Manifest, printer: &Printer) {
    println!();
    printer.success(&format!(
        "Platform: {} ({})",
        manifest.platform.kind.as_str(),
        manifest.platform.rebuild_command
    ));
    println!();

    let nix_count = manifest
        .slots
        .iter()
        .filter(|s| s.kind == SlotKind::NixPackages)
        .count();
    let brew_count = manifest
        .slots
        .iter()
        .filter(|s| s.kind == SlotKind::HomebrewList)
        .count();
    let wp_count = manifest
        .slots
        .iter()
        .filter(|s| s.kind == SlotKind::WithPackages)
        .count();
    let mas_count = manifest
        .slots
        .iter()
        .filter(|s| s.kind == SlotKind::MasApps)
        .count();
    let svc_count = manifest
        .slots
        .iter()
        .filter(|s| s.kind == SlotKind::Services)
        .count();

    Printer::detail(&format!("Discovered {} slot(s):", manifest.slots.len()));
    if nix_count > 0 {
        Printer::detail(&format!("  {nix_count} nix package list(s)"));
    }
    if brew_count > 0 {
        Printer::detail(&format!("  {brew_count} homebrew list(s)"));
    }
    if wp_count > 0 {
        Printer::detail(&format!("  {wp_count} withPackages call(s)"));
    }
    if mas_count > 0 {
        Printer::detail(&format!("  {mas_count} mas app list(s)"));
    }
    if svc_count > 0 {
        Printer::detail(&format!("  {svc_count} service definition(s)"));
    }

    if let Some(default) = manifest.default_install_slot() {
        Printer::detail(&format!(
            "  Default install target: {}",
            default.file.display()
        ));
    }

    println!();
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::manifest::PlatformKind;
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
                .any(|s| s.kind == SlotKind::NixPackages && s.attr_path == "home.packages"),
            "should find home.packages slot, got: {slots:?}"
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
        assert!(slots.iter().any(
            |s| s.kind == SlotKind::NixPackages && s.attr_path == "environment.systemPackages"
        ));
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
                .any(|s| s.kind == SlotKind::HomebrewList && s.attr_path == "homebrew.brews")
        );
        assert!(
            slots
                .iter()
                .any(|s| s.kind == SlotKind::HomebrewList && s.attr_path == "homebrew.casks")
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
        assert!(slots.iter().any(|s| s.kind == SlotKind::MasApps));
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
        assert!(
            slots
                .iter()
                .any(|s| s.kind == SlotKind::WithPackages
                    && s.runtime == Some("python3".to_string())),
            "should find python3 withPackages, got: {slots:?}"
        );
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
        assert!(
            slots.iter().any(|s| s.kind == SlotKind::Services),
            "should find launchd service, got: {slots:?}"
        );
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

        let files = collect_all_nix_files(tmp.path());
        assert!(files.iter().all(|f| !f.to_string_lossy().contains(".git")));
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
        let platform = detect_platform(tmp.path());
        assert!(platform.is_none());
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
    fn scan_fixture_repo_finds_all_slot_kinds() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/system/repo_base");
        if !fixture.exists() {
            return; // skip if fixtures not present
        }

        let slots = scan_repo_slots(&fixture);
        let platform = detect_platform(&fixture);

        // Platform should detect Darwin from darwinConfigurations
        assert!(
            platform.is_some(),
            "should detect platform from fixture flake.nix"
        );
        assert_eq!(platform.unwrap().kind, PlatformKind::Darwin);

        // Verify all SlotKind variants are represented
        let has_kind = |kind: SlotKind| slots.iter().any(|s| s.kind == kind);
        assert!(
            has_kind(SlotKind::NixPackages),
            "missing NixPackages slot: {slots:?}"
        );
        assert!(
            has_kind(SlotKind::HomebrewList),
            "missing HomebrewList slot: {slots:?}"
        );
        assert!(
            has_kind(SlotKind::MasApps),
            "missing MasApps slot: {slots:?}"
        );
        assert!(
            has_kind(SlotKind::WithPackages),
            "missing WithPackages slot: {slots:?}"
        );
        assert!(
            has_kind(SlotKind::Services),
            "missing Services slot: {slots:?}"
        );

        // Verify specific expected files
        let file_matches = |s: &Slot, file: &str, attr: &str| {
            s.file.as_path() == Path::new(file) && s.attr_path == attr
        };
        assert!(
            slots
                .iter()
                .any(|s| file_matches(s, "packages/nix/cli.nix", "home.packages")),
            "missing cli.nix home.packages slot"
        );
        assert!(
            slots
                .iter()
                .any(|s| file_matches(s, "packages/homebrew/brews.nix", "homebrew.brews")),
            "missing brews.nix slot"
        );
        assert!(
            slots
                .iter()
                .any(|s| file_matches(s, "packages/homebrew/casks.nix", "homebrew.casks")),
            "missing casks.nix slot"
        );

        // Verify no duplicates
        let mut seen = std::collections::HashSet::new();
        for slot in &slots {
            let key = (slot.file.clone(), slot.attr_path.clone());
            assert!(
                seen.insert(key.clone()),
                "duplicate slot: {}:{}",
                slot.file.display(),
                slot.attr_path
            );
        }
    }

    // --- detect_with_packages: implicit scope (1-segment attrpath) ---

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
        assert!(
            slots
                .iter()
                .any(|s| s.kind == SlotKind::WithPackages
                    && s.runtime == Some("python3".to_string())),
            "should find python3 from implicit-scope `python3.withPackages`, got: {slots:?}"
        );
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
        assert!(
            slots.iter().any(
                |s| s.kind == SlotKind::WithPackages && s.runtime == Some("lua5_4".to_string())
            ),
            "should find lua5_4 from implicit-scope, got: {slots:?}"
        );
    }

    // --- detect_with_packages: multi-segment runtimes ---

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
        assert!(
            slots.iter().any(|s| s.kind == SlotKind::WithPackages
                && s.runtime == Some("haskellPackages.ghc".to_string())),
            "should find haskellPackages.ghc, got: {slots:?}"
        );
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
        assert!(
            slots.iter().any(|s| s.kind == SlotKind::WithPackages
                && s.runtime == Some("haskellPackages.ghc".to_string())),
            "should find haskellPackages.ghc with pkgs. prefix, got: {slots:?}"
        );
    }

    #[test]
    fn detect_platform_ignores_comments() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "flake.nix",
            r"{
  # darwinConfigurations would go here
  outputs = { self, nixpkgs, ... }: {
    packages = {};
  };
}",
        );

        // Should NOT detect Darwin — "darwinConfigurations" only appears in a comment
        let platform = detect_platform(tmp.path());
        assert!(
            platform.is_none(),
            "should not detect platform from comment, got: {platform:?}"
        );
    }
}
