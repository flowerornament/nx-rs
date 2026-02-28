use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::cli::RemoveArgs;
use crate::commands::context::AppContext;
use crate::commands::shared::{
    SnippetMode, missing_argument_error, relative_location, show_snippet,
};
use crate::domain::plan::{InsertionMode, InstallPlan, LanguageInfo};
use crate::domain::source::{PackageSource, SourceResult, detect_language_package};
use crate::infra::ai_engine::{
    ClaudeEngine, CommandOutcome, build_remove_prompt, run_edit_with_callback,
};
use crate::infra::file_edit::{EditOutcome, apply_removal};
use crate::infra::finder::find_package;
use crate::infra::shell::git_diff;
use crate::output::printer::Printer;

pub fn cmd_remove(args: &RemoveArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        return missing_argument_error("remove", "PACKAGES...");
    }

    if args.dry_run {
        ctx.printer.dry_run_banner();
    }

    for package in &args.packages {
        if let Err(code) = remove_single_package(package, args, ctx) {
            return code;
        }
    }

    0
}

fn remove_single_package(package: &str, args: &RemoveArgs, ctx: &AppContext) -> Result<(), i32> {
    let location = match find_package(package, &ctx.repo_root) {
        Ok(Some(location)) => location,
        Ok(None) => {
            ctx.printer.error(&format!("{package} not found"));
            println!();
            Printer::detail(&format!("Check installed: nx list | grep -i {package}"));
            return Ok(());
        }
        Err(err) => {
            ctx.printer.error(&format!("remove lookup failed: {err}"));
            return Err(1);
        }
    };

    ctx.printer.action(&format!("Removing {package}"));
    Printer::detail(&format!(
        "Location: {}",
        relative_location(&location, &ctx.repo_root)
    ));

    location.line().map_or_else(
        || remove_via_ai(package, location.path(), args, ctx),
        |line_num| remove_with_line(package, location.path(), line_num, args, ctx),
    )
}

/// Direct removal when the finder resolved an exact line number.
fn remove_with_line(
    package: &str,
    file_path: &Path,
    line_num: usize,
    args: &RemoveArgs,
    ctx: &AppContext,
) -> Result<(), i32> {
    show_snippet(file_path, line_num, 1, SnippetMode::Remove, args.dry_run);

    if args.dry_run {
        println!("\n- Would remove {package}");
        return Ok(());
    }

    if !args.yes {
        println!();
        if !Printer::confirm(&format!("Remove {package}?"), false) {
            Printer::detail("Cancelled.");
            return Ok(());
        }
    }

    if let Err(err) = remove_line_directly(file_path, line_num) {
        ctx.printer
            .error(&format!("Failed to remove {package}: {err}"));
        return Err(1);
    }

    report_success(package, file_path, ctx);
    Ok(())
}

/// AI fallback when the finder located the file but not an exact line.
fn remove_via_ai(
    package: &str,
    file_path: &Path,
    args: &RemoveArgs,
    ctx: &AppContext,
) -> Result<(), i32> {
    let rel_path = file_path
        .strip_prefix(&ctx.repo_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();
    let prompt = build_remove_prompt(package, &rel_path);

    if args.dry_run {
        Printer::detail(&format!("[DRY RUN] Would run AI to remove {package}"));
        println!("\n- Would remove {package}");
        return Ok(());
    }

    if !args.yes {
        println!();
        if !Printer::confirm(&format!("Remove {package}?"), false) {
            Printer::detail("Cancelled.");
            return Ok(());
        }
    }

    let before_diff = git_diff(&ctx.repo_root);

    Printer::detail(&format!("Analyzing removal of {package}"));

    let engine = ClaudeEngine::new(args.model.as_deref());
    let mut deterministic_edit: Option<EditOutcome> = None;
    let execution = run_edit_with_callback(&engine, &prompt, &ctx.repo_root, || {
        deterministic_edit = try_deterministic_remove(package, file_path);
        deterministic_edit.as_ref().map(|_| CommandOutcome {
            success: true,
            output: "deterministic removal applied".to_string(),
        })
    });
    let outcome = execution.outcome;

    if !outcome.success {
        ctx.printer
            .error(&format!("Failed to remove {package}: {}", outcome.output));
        return Err(1);
    }

    if deterministic_edit.is_some() {
        report_success(package, file_path, ctx);
        return Ok(());
    }

    let after_diff = git_diff(&ctx.repo_root);
    if after_diff == before_diff {
        ctx.printer.warn(&format!("No changes made for {package}"));
    } else {
        report_success(package, file_path, ctx);
    }

    Ok(())
}

fn report_success(package: &str, file_path: &Path, ctx: &AppContext) {
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| file_path.display().to_string(), str::to_string);
    println!("* {file_name}");
    println!();
    ctx.printer
        .success(&format!("{package} removed from {file_name}"));
}

fn remove_line_directly(file_path: &Path, line_num: usize) -> anyhow::Result<()> {
    anyhow::ensure!(line_num > 0, "invalid line number");

    let content = fs::read_to_string(file_path)
        .with_context(|| format!("reading {}", file_path.display()))?;
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

    fs::write(file_path, updated).with_context(|| format!("writing {}", file_path.display()))
}

fn try_deterministic_remove(package: &str, file_path: &Path) -> Option<EditOutcome> {
    deterministic_remove_plans(package, file_path)
        .into_iter()
        .find_map(|plan| match apply_removal(&plan) {
            Ok(outcome) if outcome.file_changed => Some(outcome),
            Ok(_) | Err(_) => None,
        })
}

fn deterministic_remove_plans(package: &str, file_path: &Path) -> Vec<InstallPlan> {
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    let mut plans = Vec::new();

    if matches!(file_name, "brews.nix" | "casks.nix" | "taps.nix") {
        let source = if file_name == "casks.nix" {
            PackageSource::Cask
        } else {
            PackageSource::Homebrew
        };
        plans.push(make_remove_plan(
            package,
            file_path,
            InsertionMode::HomebrewManifest,
            None,
            source,
        ));
    }

    if file_name == "darwin.nix" {
        plans.push(make_remove_plan(
            package,
            file_path,
            InsertionMode::MasApps,
            None,
            PackageSource::Mas,
        ));
    }

    if file_name == "languages.nix"
        && let Some((bare_name, runtime, method)) = detect_language_package(package)
    {
        plans.push(make_remove_plan(
            package,
            file_path,
            InsertionMode::LanguageWithPackages,
            Some(LanguageInfo {
                bare_name: bare_name.to_string(),
                runtime: runtime.to_string(),
                method: method.to_string(),
            }),
            PackageSource::Nxs,
        ));
    }

    // Generic nix manifests are tried last.
    plans.push(make_remove_plan(
        package,
        file_path,
        InsertionMode::NixManifest,
        None,
        PackageSource::Nxs,
    ));

    plans
}

fn make_remove_plan(
    package: &str,
    file_path: &Path,
    insertion_mode: InsertionMode,
    language_info: Option<LanguageInfo>,
    source: PackageSource,
) -> InstallPlan {
    InstallPlan {
        source_result: SourceResult::new(package, source),
        package_token: package.to_string(),
        target_file: file_path.to_path_buf(),
        insertion_mode,
        language_info,
        routing_warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // --- remove_line_directly ---

    #[test]
    fn remove_line_removes_target_line() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.nix");
        fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

        remove_line_directly(&file, 2).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
    }

    #[test]
    fn remove_line_first_line() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.nix");
        fs::write(&file, "first\nsecond\nthird\n").unwrap();

        remove_line_directly(&file, 1).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "second\nthird\n");
    }

    #[test]
    fn remove_line_last_line() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.nix");
        fs::write(&file, "first\nsecond\nthird\n").unwrap();

        remove_line_directly(&file, 3).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "first\nsecond\n");
    }

    #[test]
    fn remove_line_preserves_no_trailing_newline() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.nix");
        fs::write(&file, "alpha\nbeta\ngamma").unwrap();

        remove_line_directly(&file, 2).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma");
    }

    #[test]
    fn remove_line_out_of_range_errors() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.nix");
        fs::write(&file, "only one line\n").unwrap();

        let err = remove_line_directly(&file, 5).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn remove_line_zero_errors() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.nix");
        fs::write(&file, "content\n").unwrap();

        let err = remove_line_directly(&file, 0).unwrap_err();
        assert!(err.to_string().contains("invalid line number"));
    }

    // --- git_diff ---

    #[test]
    fn git_diff_returns_empty_for_non_repo() {
        let tmp = TempDir::new().unwrap();
        let result = git_diff(tmp.path());
        // Non-repo: git diff fails → empty string fallback
        assert!(result.is_empty());
    }

    // --- deterministic callback helpers ---

    #[test]
    fn deterministic_remove_handles_homebrew_manifest() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("brews.nix");
        fs::write(
            &file,
            r#"[
  "htop"
  "ripgrep"
]
"#,
        )
        .unwrap();

        let outcome = try_deterministic_remove("htop", &file);

        assert!(outcome.is_some());
        assert!(!fs::read_to_string(&file).unwrap().contains("\"htop\""));
    }

    #[test]
    fn deterministic_remove_handles_mas_apps() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("darwin.nix");
        fs::write(
            &file,
            r#"{ ... }:
{
  homebrew.masApps = {
    "Slack" = 803453959;
    "Xcode" = 497799835;
  };
}
"#,
        )
        .unwrap();

        let outcome = try_deterministic_remove("Xcode", &file);

        assert!(outcome.is_some());
        assert!(!fs::read_to_string(&file).unwrap().contains("\"Xcode\""));
    }

    #[test]
    fn deterministic_remove_returns_none_for_unsupported_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("custom.nix");
        fs::write(&file, "{ }\n").unwrap();

        let outcome = try_deterministic_remove("ripgrep", &file);

        assert!(outcome.is_none());
    }
}
