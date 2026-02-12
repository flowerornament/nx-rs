use std::path::Path;

use crate::cli::RemoveArgs;
use crate::commands::shared::{
    SnippetMode, location_path_and_line, relative_location, show_snippet,
};
use crate::nix_scan::find_package;
use crate::output::printer::Printer;

pub fn cmd_remove(args: &RemoveArgs, repo_root: &Path, printer: &Printer) -> i32 {
    if args.packages.is_empty() {
        printer.error("No packages specified");
        return 1;
    }

    if !args.dry_run {
        printer.error("remove is not implemented yet");
        return 1;
    }

    for package in &args.packages {
        match find_package(package, repo_root) {
            Ok(Some(location)) => {
                printer.dry_run_banner();
                printer.action(&format!("Removing {package}"));
                printer.detail(&format!(
                    "Location: {}",
                    relative_location(&location, repo_root)
                ));
                let (file_path, line_num) = location_path_and_line(&location);
                if let Some(line_num) = line_num {
                    show_snippet(file_path, line_num, 1, SnippetMode::Remove, true);
                }
                println!("\n- Would remove {package}");
            }
            Ok(None) => {
                printer.error(&format!("{package} not found"));
            }
            Err(err) => {
                printer.error(&format!("remove lookup failed: {err}"));
                return 1;
            }
        }
    }

    0
}
