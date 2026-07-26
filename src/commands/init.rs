use std::collections::hash_map::Entry;
use std::path::Path;

use crate::commands::context::InitContext;
use crate::domain::manifest::{Manifest, SlotKind};
use crate::domain::manifest_scan::manifest_from_scan;
use crate::domain::package::PackageBuckets;
use crate::domain::usage::default_usage_aliases_for_packages;
use crate::infra::config_scan::scan_packages;
use crate::output::printer::Printer;

pub fn cmd_init(refresh: bool, ctx: &InitContext<'_>) -> i32 {
    ctx.printer.action(if refresh {
        "Rescanning repository"
    } else {
        "Scanning repository"
    });

    let existing = if refresh {
        match Manifest::load(ctx.repo_root) {
            Ok(manifest) => manifest,
            Err(err) => {
                ctx.printer
                    .error(&format!("failed to load existing manifest: {err:#}"));
                return 1;
            }
        }
    } else {
        None
    };

    let mut manifest =
        manifest_from_scan(ctx.scanned_repo.clone(), ctx.repo_root, existing.as_ref());
    if let Err(err) = seed_usage_alias_hints(&mut manifest, ctx.repo_root) {
        ctx.printer
            .warn(&format!("package alias hints skipped: {err:#}"));
    }

    print_summary(&manifest, ctx.printer);

    if !Printer::confirm("Write .nx/manifest.toml?", true) {
        Printer::detail("Cancelled.");
        return 0;
    }

    if let Err(err) = manifest.save(ctx.repo_root) {
        ctx.printer
            .error(&format!("failed to write manifest: {err:#}"));
        return 1;
    }

    ctx.printer.success("Manifest written to .nx/manifest.toml");
    0
}

fn seed_usage_alias_hints(manifest: &mut Manifest, repo_root: &Path) -> anyhow::Result<usize> {
    let buckets = scan_packages(repo_root)?.buckets();
    let mut added = 0;

    for (alias, package) in default_usage_aliases_for_packages(package_names(&buckets)) {
        if let Entry::Vacant(entry) = manifest.aliases.entry(alias) {
            entry.insert(package);
            added += 1;
        }
    }

    Ok(added)
}

fn package_names(buckets: &PackageBuckets) -> impl Iterator<Item = &str> {
    buckets
        .nxs
        .iter()
        .chain(&buckets.brews)
        .chain(&buckets.casks)
        .chain(&buckets.mas)
        .chain(&buckets.services)
        .map(String::as_str)
}

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
        .filter(|slot| slot.kind == SlotKind::NixPackages)
        .count();
    let brew_count = manifest
        .slots
        .iter()
        .filter(|slot| slot.kind == SlotKind::HomebrewList)
        .count();
    let wp_count = manifest
        .slots
        .iter()
        .filter(|slot| slot.kind == SlotKind::WithPackages)
        .count();
    let mas_count = manifest
        .slots
        .iter()
        .filter(|slot| slot.kind == SlotKind::MasApps)
        .count();
    let svc_count = manifest
        .slots
        .iter()
        .filter(|slot| slot.kind == SlotKind::Services)
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
    if !manifest.aliases.is_empty() {
        Printer::detail(&format!(
            "  {} package alias hint(s)",
            manifest.aliases.len()
        ));
    }

    println!();
}
