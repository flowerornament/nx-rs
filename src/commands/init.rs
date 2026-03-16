use crate::commands::context::AppContext;
use crate::domain::manifest::{Manifest, SlotKind};
use crate::domain::manifest_scan::manifest_from_scan;
use crate::output::printer::Printer;

pub fn cmd_init(refresh: bool, ctx: &AppContext) -> i32 {
    ctx.printer.action(if refresh {
        "Rescanning repository"
    } else {
        "Scanning repository"
    });

    let existing = if refresh {
        match Manifest::load(&ctx.repo_root) {
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

    let manifest = manifest_from_scan(ctx.scanned_repo.clone(), &ctx.repo_root, existing.as_ref());

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

    println!();
}
