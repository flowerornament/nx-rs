use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::cli::RemoveArgs;
use crate::commands::context::AppContext;
use crate::commands::shared::{
    SnippetMode, missing_argument_error, relative_location, show_snippet,
};
use crate::domain::manifest::{Manifest, SlotKind};
use crate::domain::plan::EditSpec;
use crate::domain::source::detect_language_package;
use crate::infra::ai_engine::{
    ClaudeCodeEngine, CommandOutcome, build_remove_prompt, run_edit_with_callback,
};
use crate::infra::file_edit::{EditOutcome, remove_first_edit};
use crate::infra::finder::find_package;
use crate::infra::persistence::write_file_atomically;
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
            return Err(1);
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
        ctx.printer.removal(&format!("Would remove {package}"));
        return Ok(());
    }

    if !args.yes {
        println!();
        if !Printer::confirm(&format!("Remove {package}?"), false) {
            Printer::body("Cancelled.");
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
        ctx.printer.removal(&format!("Would remove {package}"));
        return Ok(());
    }

    if !args.yes {
        println!();
        if !Printer::confirm(&format!("Remove {package}?"), false) {
            Printer::body("Cancelled.");
            return Ok(());
        }
    }

    let before_diff = git_diff(&ctx.repo_root);

    Printer::detail(&format!("Analyzing removal of {package}"));

    let engine = ClaudeCodeEngine::new(args.model.as_deref(), ctx.printer.style());
    let manifest = ctx.config_files.manifest();
    let mut deterministic_edit = false;
    let execution =
        run_edit_with_callback(
            &engine,
            &prompt,
            &ctx.repo_root,
            || match try_deterministic_remove(package, file_path, manifest) {
                Ok(Some(_)) => {
                    deterministic_edit = true;
                    Some(CommandOutcome {
                        success: true,
                        output: "deterministic removal applied".to_string(),
                    })
                }
                Ok(None) => None,
                Err(err) => Some(CommandOutcome {
                    success: false,
                    output: err.to_string(),
                }),
            },
        );
    let outcome = execution.outcome;

    if !outcome.success {
        ctx.printer
            .error(&format!("Failed to remove {package}: {}", outcome.output));
        return Err(1);
    }

    if deterministic_edit {
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
    println!();
    ctx.printer
        .removal(&format!("{package} removed from {file_name}"));
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

    write_file_atomically(file_path, updated)
}

fn try_deterministic_remove(
    package: &str,
    file_path: &Path,
    manifest: Option<&Manifest>,
) -> anyhow::Result<Option<EditOutcome>> {
    let specs = deterministic_remove_specs(package, file_path, manifest);
    match remove_first_edit(file_path, &specs)? {
        changed @ EditOutcome::Changed { .. } => Ok(Some(changed)),
        EditOutcome::Unchanged => Ok(None),
    }
}

fn deterministic_remove_specs(
    package: &str,
    file_path: &Path,
    manifest: Option<&Manifest>,
) -> Vec<EditSpec> {
    // When a manifest is available, use its slot kind to choose the edit shape.
    if let Some(m) = manifest
        && let Some(specs) = specs_from_manifest(package, file_path, m)
    {
        return specs;
    }

    // Fallback: filename-based heuristic (no manifest or file not in manifest).
    specs_from_filename(package, file_path)
}

/// Build removal edits by looking up the file in manifest slots.
fn specs_from_manifest(
    package: &str,
    file_path: &Path,
    manifest: &Manifest,
) -> Option<Vec<EditSpec>> {
    let kind = manifest
        .slots
        .iter()
        .find(|slot| file_path.ends_with(&slot.file))
        .map(|slot| slot.kind)?;
    Some(specs_for_kind(package, kind))
}

/// Filename-based heuristic for when no manifest is available.
fn specs_from_filename(package: &str, file_path: &Path) -> Vec<EditSpec> {
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    let kind = match file_name {
        "brews.nix" | "casks.nix" | "taps.nix" => SlotKind::HomebrewList,
        "darwin.nix" => SlotKind::MasApps,
        "languages.nix" => SlotKind::WithPackages,
        _ => SlotKind::NixPackages,
    };
    specs_for_kind(package, kind)
}

fn specs_for_kind(package: &str, kind: SlotKind) -> Vec<EditSpec> {
    let primary = match kind {
        SlotKind::HomebrewList => EditSpec::homebrew_list(package),
        SlotKind::MasApps => EditSpec::mas_apps(package),
        SlotKind::WithPackages => detect_language_package(package).map_or_else(
            || EditSpec::nix_packages(package),
            |(member, runtime)| EditSpec::with_packages(package, member, runtime),
        ),
        SlotKind::NixPackages | SlotKind::Services => EditSpec::nix_packages(package),
    };

    if matches!(primary, EditSpec::NixPackages { .. }) {
        vec![primary]
    } else {
        vec![primary, EditSpec::nix_packages(package)]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

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

        let outcome = try_deterministic_remove("htop", &file, None).unwrap();

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

        let outcome = try_deterministic_remove("Xcode", &file, None).unwrap();

        assert!(outcome.is_some());
        assert!(!fs::read_to_string(&file).unwrap().contains("\"Xcode\""));
    }

    #[test]
    fn deterministic_remove_returns_none_for_unsupported_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("custom.nix");
        fs::write(&file, "{ }\n").unwrap();

        let outcome = try_deterministic_remove("ripgrep", &file, None).unwrap();

        assert!(outcome.is_none());
    }

    #[test]
    fn deterministic_remove_propagates_file_errors() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing.nix");

        let error = try_deterministic_remove("ripgrep", &missing, None).unwrap_err();

        assert!(error.to_string().contains("cannot read"));
    }

    #[test]
    fn deterministic_remove_uses_manifest_slot_kind() {
        let tmp = TempDir::new().unwrap();
        // File with a non-standard name but contains homebrew-style content.
        let file = tmp.path().join("my-brews.nix");
        fs::write(
            &file,
            r#"[
  "htop"
  "ripgrep"
]
"#,
        )
        .unwrap();

        // Without manifest: filename doesn't match, so only NixPackages is tried.
        let outcome_no_manifest = try_deterministic_remove("htop", &file, None).unwrap();
        assert!(outcome_no_manifest.is_none());

        // Restore file content after failed attempt.
        fs::write(
            &file,
            r#"[
  "htop"
  "ripgrep"
]
"#,
        )
        .unwrap();

        // With manifest: the HomebrewList edit shape is tried first.
        let manifest = Manifest {
            schema_version: 1,
            platform: crate::domain::manifest::PlatformConfig {
                kind: crate::domain::manifest::PlatformKind::Darwin,
                rebuild_command: "darwin-rebuild switch".to_string(),
                sudo: false,
                flake_root: ".".to_string(),
                split_rebuild: false,
            },
            slots: vec![crate::domain::manifest::Slot {
                kind: SlotKind::HomebrewList,
                file: PathBuf::from("my-brews.nix"),
                attr_path: "homebrew.brews".to_string(),
                tags: vec![],
                runtime: None,
                default_for: None,
            }],
            aliases: HashMap::default(),
            overlays: HashMap::default(),
        };

        let outcome_with_manifest =
            try_deterministic_remove("htop", &file, Some(&manifest)).unwrap();
        assert!(outcome_with_manifest.is_some());
        assert!(!fs::read_to_string(&file).unwrap().contains("\"htop\""));
    }
}
