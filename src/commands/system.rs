use std::path::Path;

use crate::cli::PassthroughArgs;
use crate::commands::context::AppContext;
use crate::infra::shell::{CapturedCommand, run_captured_command, run_indented_command};
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
        println!("Nothing to undo.");
        return 0;
    }

    ctx.printer
        .action(&format!("Undo Changes ({} files)", modified.len()));

    for file in &modified {
        println!("  {file}");
        if let Some(summary) = git_diff_stat(file, &ctx.repo_root) {
            ctx.printer.detail(&summary);
        }
    }

    println!();
    if !ctx.printer.confirm("Revert all changes?", false) {
        println!("Cancelled.");
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
fn git_modified_files(repo_root: &Path) -> anyhow::Result<Vec<String>> {
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
fn git_diff_stat(file: &str, repo_root: &Path) -> Option<String> {
    let output = run_captured_command("git", &["diff", "--stat", file], Some(repo_root)).ok()?;

    output
        .stdout
        .lines()
        .find(|line| {
            line.contains("insertion") || line.contains("deletion") || line.contains("changed")
        })
        .map(|line| line.trim().to_string())
}

// ─── update ──────────────────────────────────────────────────────────────────

const DARWIN_REBUILD: &str = "/run/current-system/sw/bin/darwin-rebuild";

pub fn cmd_update(args: &PassthroughArgs, ctx: &AppContext) -> i32 {
    ctx.printer.action("Updating flake inputs");

    let mut command_args: Vec<&str> = vec!["flake", "update"];
    command_args.extend(args.passthrough.iter().map(String::as_str));
    let return_code = match run_indented_command(
        "nix",
        &command_args,
        Some(&ctx.repo_root),
        &ctx.printer,
        "  ",
    ) {
        Ok(code) => code,
        Err(err) => {
            ctx.printer.error(&format!("{err:#}"));
            return 1;
        }
    };

    if return_code == 0 {
        println!();
        ctx.printer.success("Flake inputs updated");
        ctx.printer
            .detail("Run 'nx rebuild' to rebuild, or 'nx upgrade' for full upgrade");
        return 0;
    }

    ctx.printer.error("Flake update failed");
    1
}

pub fn cmd_test(ctx: &AppContext) -> i32 {
    let scripts_nx = ctx.repo_root.join("scripts/nx");
    let steps: [(&str, &str, &[&str], Option<&Path>); 3] = [
        ("ruff", "ruff", &["check", "."], Some(&scripts_nx)),
        ("mypy", "mypy", &["."], Some(&scripts_nx)),
        (
            "tests",
            "python3",
            &["-m", "unittest", "discover", "-s", "scripts/nx/tests"],
            Some(&ctx.repo_root),
        ),
    ];

    for (label, program, args, cwd) in steps {
        if run_test_step(label, program, args, cwd, &ctx.printer).is_err() {
            return 1;
        }
    }

    0
}

fn run_test_step(
    label: &str,
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    printer: &Printer,
) -> Result<(), ()> {
    printer.action(&format!("Running {label}"));
    println!();

    let return_code = match run_indented_command(program, args, cwd, printer, "  ") {
        Ok(code) => code,
        Err(err) => {
            printer.error(&format!("{label} failed"));
            printer.error(&format!("{err:#}"));
            return Err(());
        }
    };

    if return_code != 0 {
        printer.error(&format!("{label} failed"));
        return Err(());
    }

    println!();
    printer.success(&format!("{label} passed"));
    Ok(())
}

pub fn cmd_rebuild(args: &PassthroughArgs, ctx: &AppContext) -> i32 {
    if let Err(code) = check_git_preflight(ctx) {
        return code;
    }
    if let Err(code) = check_flake(ctx) {
        return code;
    }
    do_rebuild(args, ctx)
}

/// Returns `stderr.trim()` if non-empty, otherwise `stdout.trim()`.
fn first_nonempty_output(output: &CapturedCommand) -> &str {
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        return stderr;
    }
    output.stdout.trim()
}

fn check_git_preflight(ctx: &AppContext) -> Result<(), i32> {
    ctx.printer.action("Checking tracked nix files");
    let repo = ctx.repo_root.display().to_string();
    let args = [
        "-C",
        &repo,
        "ls-files",
        "--others",
        "--exclude-standard",
        "--",
        "home",
        "packages",
        "system",
        "hosts",
    ];
    let output = match run_captured_command("git", &args, None) {
        Ok(output) => output,
        Err(err) => {
            ctx.printer.error(&format!("Git preflight failed: {err:#}"));
            return Err(1);
        }
    };

    if output.code != 0 {
        ctx.printer.error("Git preflight failed");
        let detail = first_nonempty_output(&output);
        if !detail.is_empty() {
            ctx.printer.detail(detail);
        }
        return Err(1);
    }

    #[allow(clippy::case_sensitive_file_extension_comparisons)] // .nix is always lowercase
    let mut untracked: Vec<&str> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.ends_with(".nix"))
        .collect();
    untracked.sort_unstable();

    if untracked.is_empty() {
        ctx.printer.success("Git preflight passed");
        return Ok(());
    }

    ctx.printer
        .error("Untracked .nix files would be ignored by flake evaluation");
    println!();
    ctx.printer.detail("Track these files before rebuild:");
    for rel_path in &untracked {
        ctx.printer.detail(&format!("- {rel_path}"));
    }
    println!();
    ctx.printer.detail(&format!(
        "Run: git -C \"{}\" add <files>",
        ctx.repo_root.display()
    ));
    Err(1)
}

fn check_flake(ctx: &AppContext) -> Result<(), i32> {
    ctx.printer.action("Checking flake");
    let repo = ctx.repo_root.display().to_string();
    let args = ["flake", "check", &repo];
    let output = match run_captured_command("nix", &args, None) {
        Ok(output) => output,
        Err(err) => {
            ctx.printer.error(&format!("Flake check failed: {err:#}"));
            return Err(1);
        }
    };

    if output.code != 0 {
        ctx.printer.error("Flake check failed");
        let err_text = first_nonempty_output(&output);
        if !err_text.is_empty() {
            println!("{err_text}");
        }
        return Err(1);
    }

    ctx.printer.success("Flake check passed");
    Ok(())
}

fn do_rebuild(args: &PassthroughArgs, ctx: &AppContext) -> i32 {
    ctx.printer.action("Rebuilding system");
    println!();
    let repo = ctx.repo_root.display().to_string();
    let mut rebuild_args: Vec<&str> = vec![DARWIN_REBUILD, "switch", "--flake", &repo];
    rebuild_args.extend(args.passthrough.iter().map(String::as_str));

    let return_code = match run_indented_command("sudo", &rebuild_args, None, &ctx.printer, "  ") {
        Ok(code) => code,
        Err(err) => {
            ctx.printer.error("Rebuild failed");
            ctx.printer.error(&format!("{err:#}"));
            return 1;
        }
    };

    if return_code == 0 {
        println!();
        ctx.printer.success("System rebuilt");
        return 0;
    }

    ctx.printer.error("Rebuild failed");
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a minimal git repo with one committed file.
    fn init_git_repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        run_captured_command("git", &["init"], Some(root)).unwrap();
        run_captured_command(
            "git",
            &["config", "user.email", "test@test.com"],
            Some(root),
        )
        .unwrap();
        run_captured_command("git", &["config", "user.name", "Test"], Some(root)).unwrap();

        fs::write(root.join("file.txt"), "initial\n").unwrap();
        run_captured_command("git", &["add", "file.txt"], Some(root)).unwrap();
        run_captured_command("git", &["commit", "-m", "init"], Some(root)).unwrap();

        tmp
    }

    // --- git_modified_files ---

    #[test]
    fn modified_files_empty_on_clean_tree() {
        let tmp = init_git_repo();
        let modified = git_modified_files(tmp.path()).unwrap();
        assert!(modified.is_empty());
    }

    #[test]
    fn modified_files_detects_unstaged_changes() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("file.txt"), "changed\n").unwrap();

        let modified = git_modified_files(tmp.path()).unwrap();
        assert_eq!(modified, vec!["file.txt"]);
    }

    #[test]
    fn modified_files_ignores_staged_only() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("file.txt"), "staged\n").unwrap();
        run_captured_command("git", &["add", "file.txt"], Some(tmp.path())).unwrap();

        let modified = git_modified_files(tmp.path()).unwrap();
        // Staged-only files have status `M ` not ` M`, so excluded
        assert!(modified.is_empty());
    }

    #[test]
    fn modified_files_ignores_untracked() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("new.txt"), "new\n").unwrap();

        let modified = git_modified_files(tmp.path()).unwrap();
        assert!(modified.is_empty());
    }

    // --- git_diff_stat ---

    #[test]
    fn diff_stat_returns_summary_for_modified_file() {
        let tmp = init_git_repo();
        fs::write(tmp.path().join("file.txt"), "changed\n").unwrap();

        let summary = git_diff_stat("file.txt", tmp.path());
        assert!(summary.is_some());
        let text = summary.unwrap();
        assert!(
            text.contains("changed") || text.contains("insertion") || text.contains("deletion"),
            "expected diff stat summary, got: {text}"
        );
    }

    #[test]
    fn diff_stat_returns_none_for_clean_file() {
        let tmp = init_git_repo();
        let summary = git_diff_stat("file.txt", tmp.path());
        assert!(summary.is_none());
    }
}
