use std::fs;
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

    if args.dry_run {
        printer.dry_run_banner();
    } else if !args.yes {
        printer.error("Removal requires --yes (or use --dry-run)");
        return 1;
    }

    for package in &args.packages {
        match find_package(package, repo_root) {
            Ok(Some(location)) => {
                printer.action(&format!("Removing {package}"));
                printer.detail(&format!(
                    "Location: {}",
                    relative_location(&location, repo_root)
                ));
                let (file_path, line_num) = location_path_and_line(&location);
                if let Some(line_num) = line_num {
                    show_snippet(file_path, line_num, 1, SnippetMode::Remove, args.dry_run);
                    if args.dry_run {
                        println!("\n- Would remove {package}");
                        continue;
                    }

                    if let Err(err) = remove_line_directly(file_path, line_num) {
                        printer.error(&format!("Failed to remove {package}: {err}"));
                        return 1;
                    }

                    let file_name = Path::new(file_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(file_path);
                    println!("* {file_name}");
                    println!();
                    printer.success(&format!("{package} removed from {file_name}"));
                }
            }
            Ok(None) => {
                printer.error(&format!("{package} not found"));
                println!();
                printer.detail(&format!("Check installed: nx list | grep -i {package}"));
            }
            Err(err) => {
                printer.error(&format!("remove lookup failed: {err}"));
                return 1;
            }
        }
    }

    0
}

fn remove_line_directly(file_path: &str, line_num: usize) -> Result<(), String> {
    if line_num == 0 {
        return Err("invalid line number".to_string());
    }

    let content = fs::read_to_string(file_path).map_err(|err| err.to_string())?;
    let mut lines: Vec<&str> = content.lines().collect();
    if line_num > lines.len() {
        return Err(format!(
            "line {} out of range for {} lines",
            line_num,
            lines.len()
        ));
    }

    lines.remove(line_num - 1);
    let mut updated = lines.join("\n");
    if content.ends_with('\n') {
        updated.push('\n');
    }

    fs::write(file_path, updated).map_err(|err| err.to_string())
}
