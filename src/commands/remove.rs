use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::cli::RemoveArgs;
use crate::commands::context::AppContext;
use crate::commands::shared::{
    SnippetMode, location_path_and_line, relative_location, show_snippet,
};
use crate::infra::finder::find_package;

pub fn cmd_remove(args: &RemoveArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        ctx.printer.error("No packages specified");
        return 1;
    }

    if args.dry_run {
        ctx.printer.dry_run_banner();
    }

    for package in &args.packages {
        match find_package(package, &ctx.repo_root) {
            Ok(Some(location)) => {
                ctx.printer.action(&format!("Removing {package}"));
                ctx.printer.detail(&format!(
                    "Location: {}",
                    relative_location(&location, &ctx.repo_root)
                ));
                let (file_path, line_num) = location_path_and_line(&location);
                if let Some(line_num) = line_num {
                    show_snippet(file_path, line_num, 1, SnippetMode::Remove, args.dry_run);
                    if args.dry_run {
                        println!("\n- Would remove {package}");
                        continue;
                    }

                    if !args.yes {
                        println!();
                        if !ctx.printer.confirm(&format!("Remove {package}?"), false) {
                            ctx.printer.detail("Cancelled.");
                            continue;
                        }
                    }

                    if let Err(err) = remove_line_directly(file_path, line_num) {
                        ctx.printer
                            .error(&format!("Failed to remove {package}: {err}"));
                        return 1;
                    }

                    let file_name = Path::new(file_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(file_path);
                    println!("* {file_name}");
                    println!();
                    ctx.printer
                        .success(&format!("{package} removed from {file_name}"));
                }
            }
            Ok(None) => {
                ctx.printer.error(&format!("{package} not found"));
                println!();
                ctx.printer
                    .detail(&format!("Check installed: nx list | grep -i {package}"));
            }
            Err(err) => {
                ctx.printer.error(&format!("remove lookup failed: {err}"));
                return 1;
            }
        }
    }

    0
}

fn remove_line_directly(file_path: &str, line_num: usize) -> anyhow::Result<()> {
    anyhow::ensure!(line_num > 0, "invalid line number");

    let content = fs::read_to_string(file_path).with_context(|| format!("reading {file_path}"))?;
    let mut lines: Vec<&str> = content.lines().collect();
    anyhow::ensure!(
        line_num <= lines.len(),
        "line {line_num} out of range for {} lines",
        lines.len()
    );

    lines.remove(line_num - 1);
    let mut updated = lines.join("\n");
    if content.ends_with('\n') {
        updated.push('\n');
    }

    fs::write(file_path, updated).with_context(|| format!("writing {file_path}"))
}
