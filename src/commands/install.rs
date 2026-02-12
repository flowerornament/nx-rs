use std::path::Path;

use crate::cli::InstallArgs;
use crate::commands::shared::relative_location;
use crate::nix_scan::find_package;
use crate::output::printer::Printer;

pub fn cmd_install(args: &InstallArgs, repo_root: &Path, printer: &Printer) -> i32 {
    if args.packages.is_empty() {
        printer.error("No packages specified");
        return 1;
    }

    if args.dry_run {
        printer.dry_run_banner();
    }

    printer.action(&format!("Installing {}", args.packages[0]));

    for package in &args.packages {
        match find_package(package, repo_root) {
            Ok(Some(location)) => {
                println!();
                printer.success(&format!(
                    "{package} already installed ({})",
                    relative_location(&location, repo_root)
                ));
            }
            Ok(None) => {
                printer.error(&format!("{package} not found"));
                return 1;
            }
            Err(err) => {
                printer.error(&format!("install lookup failed: {err}"));
                return 1;
            }
        }
    }

    0
}
