use std::path::Path;

use crate::commands::context::AppContext;
use crate::infra::shell::run_captured_command;
use crate::output::printer::Printer;

// ─── undo ────────────────────────────────────────────────────────────────────

pub fn cmd_undo(ctx: &AppContext) -> i32 {
    let modified = match git_modified_files(&ctx.repo_root) {
        Ok(files) => files,
        Err(err) => {
            ctx.printer.error(&format!("git status failed: {err:#}"));
            return 0;
        }
    };

    if modified.is_empty() {
        println!();
        println!("  Nothing to undo.");
        return 0;
    }

    println!();
    println!("  Undo Changes ({} files)", modified.len());

    for file in &modified {
        println!("  {file}");
        if let Some(summary) = git_diff_stat(file, &ctx.repo_root) {
            println!("    {summary}");
        }
    }

    println!();
    if !Printer::confirm("Revert all changes?", false) {
        println!("  Cancelled.");
        return 0;
    }

    for file in &modified {
        let _ = run_captured_command("git", &["checkout", "--", file], Some(&ctx.repo_root));
    }

    ctx.printer
        .success(&format!("Reverted {} files", modified.len()));
    0
}

/// Parse `git status --porcelain` for unstaged modifications (` M` prefix).
pub(super) fn git_modified_files(repo_root: &Path) -> anyhow::Result<Vec<String>> {
    let output = run_captured_command("git", &["status", "--porcelain"], Some(repo_root))?;

    if output.stdout.trim().is_empty() {
        return Ok(Vec::new());
    }

    let modified = output
        .stdout
        .lines()
        .filter(|line| line.starts_with(" M"))
        .filter_map(|line| line.get(3..))
        .map(String::from)
        .collect();

    Ok(modified)
}

/// Get the diff stat summary line for a single file.
pub(super) fn git_diff_stat(file: &str, repo_root: &Path) -> Option<String> {
    let output = run_captured_command("git", &["diff", "--stat", file], Some(repo_root)).ok()?;

    output
        .stdout
        .lines()
        .find(|line| {
            line.contains("insertion") || line.contains("deletion") || line.contains("changed")
        })
        .map(|line| line.trim().to_string())
}
