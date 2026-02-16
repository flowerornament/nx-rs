use std::path::Path;

use crate::cli::{PassthroughArgs, UpgradeArgs};
use crate::commands::context::AppContext;
use crate::domain::upgrade::{diff_locks, load_flake_lock, short_rev};
use crate::infra::shell::{
    CapturedCommand, run_captured_command, run_indented_command, run_indented_command_collecting,
};
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
    if !ctx.printer.confirm("Revert all changes?", false) {
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

// ─── upgrade ─────────────────────────────────────────────────────────────────

pub fn cmd_upgrade(args: &UpgradeArgs, ctx: &AppContext) -> i32 {
    if args.dry_run {
        ctx.printer.dry_run_banner();
    }

    // Phase 1: Flake update
    let flake_changed = match run_flake_phase(args, ctx) {
        Ok(changed) => changed,
        Err(code) => return code,
    };

    // Phase 2: Brew
    if !args.skip_brew {
        run_brew_phase(args, ctx);
    }

    if args.dry_run {
        ctx.printer.detail("Dry run complete - no changes made");
        return 0;
    }

    // Phase 3: Rebuild
    if !args.skip_rebuild {
        let passthrough = PassthroughArgs {
            passthrough: Vec::new(),
        };
        if cmd_rebuild(&passthrough, ctx) != 0 {
            return 1;
        }
    }

    // Phase 4: Commit
    if !args.skip_commit && flake_changed {
        commit_flake_lock(ctx);
    }

    0
}

/// Flake phase: load old lock → update → load new lock → diff → report.
///
/// Returns `Ok(true)` if flake inputs changed, `Ok(false)` if unchanged,
/// `Err(exit_code)` on failure.
fn run_flake_phase(args: &UpgradeArgs, ctx: &AppContext) -> Result<bool, i32> {
    let old_inputs = load_flake_lock(&ctx.repo_root).unwrap_or_default();

    let new_inputs = if args.dry_run {
        old_inputs.clone()
    } else {
        if !stream_nix_update(args, ctx) {
            ctx.printer.error("Flake update failed");
            return Err(1);
        }
        load_flake_lock(&ctx.repo_root).unwrap_or_default()
    };

    let diff = diff_locks(&old_inputs, &new_inputs);

    if diff.changed.is_empty() && diff.added.is_empty() && diff.removed.is_empty() {
        ctx.printer.success("All flake inputs up to date");
        return Ok(false);
    }

    if !diff.changed.is_empty() {
        ctx.printer
            .action(&format!("Flake Inputs Changed ({})", diff.changed.len()));
        for change in &diff.changed {
            println!(
                "  {} ({}/{}) {} \u{2192} {}",
                change.name,
                change.owner,
                change.repo,
                short_rev(&change.old_rev),
                short_rev(&change.new_rev),
            );
        }
    }

    if !diff.added.is_empty() {
        ctx.printer
            .detail(&format!("Added: {}", diff.added.join(", ")));
    }
    if !diff.removed.is_empty() {
        ctx.printer
            .detail(&format!("Removed: {}", diff.removed.join(", ")));
    }

    Ok(!diff.changed.is_empty())
}

/// Brew phase: check outdated packages, display, and upgrade.
fn run_brew_phase(args: &UpgradeArgs, ctx: &AppContext) {
    ctx.printer.action("Checking Homebrew updates");

    let outdated = brew_outdated();

    if outdated.is_empty() {
        ctx.printer.success("All Homebrew packages up to date");
        return;
    }

    ctx.printer.detail(&format!(
        "{} outdated package{}",
        outdated.len(),
        if outdated.len() == 1 { "" } else { "s" }
    ));

    for (name, installed, current) in &outdated {
        println!("  {name}: {installed} \u{2192} {current}");
    }

    if args.dry_run {
        return;
    }

    let pkg_names: Vec<&str> = outdated.iter().map(|(name, _, _)| name.as_str()).collect();
    ctx.printer
        .action(&format!("Upgrading {} Homebrew packages", pkg_names.len()));
    println!();

    let code = match run_indented_command("brew", &["upgrade"], None, &ctx.printer, "  ") {
        Ok(code) => code,
        Err(err) => {
            ctx.printer.error(&format!("{err:#}"));
            return;
        }
    };

    println!();
    if code == 0 {
        ctx.printer.success("Homebrew packages upgraded");
    } else {
        ctx.printer.warn("Some Homebrew upgrades may have failed");
    }
}

/// Fetch outdated brew packages via `brew outdated --json`.
fn brew_outdated() -> Vec<(String, String, String)> {
    let output = match run_captured_command("brew", &["outdated", "--json"], None) {
        Ok(cmd) if cmd.code == 0 => cmd.stdout,
        _ => return Vec::new(),
    };
    parse_brew_outdated_json(&output)
}

/// Parse brew outdated JSON into (name, installed, current) version tuples.
fn parse_brew_outdated_json(json_str: &str) -> Vec<(String, String, String)> {
    let data: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();

    // Formulae
    if let Some(formulae) = data.get("formulae").and_then(|v| v.as_array()) {
        for formula in formulae {
            let name = formula
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let installed = formula
                .get("installed_versions")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let current = formula
                .get("current_version")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !name.is_empty() && !installed.is_empty() && !current.is_empty() {
                results.push((name.to_string(), installed.to_string(), current.to_string()));
            }
        }
    }

    // Casks
    if let Some(casks) = data.get("casks").and_then(|v| v.as_array()) {
        for cask in casks {
            let name = cask
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let installed = cask
                .get("installed_versions")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let current = cask
                .get("current_version")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !name.is_empty() && !installed.is_empty() && !current.is_empty() {
                results.push((name.to_string(), installed.to_string(), current.to_string()));
            }
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

/// Build the nix flake update command, optionally wrapped with a ulimit raise.
fn build_nix_update_command(base_args: &[String], raise_nofile: Option<u32>) -> Vec<String> {
    match raise_nofile {
        Some(limit) => {
            let nix_cmd = std::iter::once("nix".to_string())
                .chain(base_args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            vec![
                "-lc".to_string(),
                format!("ulimit -n {limit} 2>/dev/null; exec {nix_cmd}"),
            ]
        }
        None => base_args.to_vec(),
    }
}

/// Detect file descriptor exhaustion in command output.
fn is_fd_exhaustion(output: &str) -> bool {
    output.contains("Too many open files") || output.contains("too many open files")
}

/// Execute `nix flake update` with GitHub token, ulimit raising, and retry.
fn stream_nix_update(args: &UpgradeArgs, ctx: &AppContext) -> bool {
    let token = gh_auth_token();

    let mut base_args: Vec<String> = vec!["flake".into(), "update".into()];
    base_args.extend(args.passthrough.clone());
    if !token.is_empty() {
        base_args.extend([
            "--option".into(),
            "access-tokens".into(),
            format!("github.com={token}"),
        ]);
    }

    // Proactively raise FD limit to avoid "Too many open files" from libgit2.
    let mut raise_nofile: Option<u32> = Some(8192);

    for attempt in 0..3 {
        if attempt == 0 {
            ctx.printer.action("Updating flake inputs");
        } else {
            ctx.printer.action("Retrying flake update");
        }

        let cmd_args = build_nix_update_command(&base_args, raise_nofile);
        let (program, arg_refs): (&str, Vec<&str>) = if raise_nofile.is_some() {
            ("bash", cmd_args.iter().map(String::as_str).collect())
        } else {
            ("nix", cmd_args.iter().map(String::as_str).collect())
        };

        let (code, output) = match run_indented_command_collecting(
            program,
            &arg_refs,
            Some(&ctx.repo_root),
            &ctx.printer,
            "  ",
        ) {
            Ok(result) => result,
            Err(err) => {
                ctx.printer.error(&format!("{err:#}"));
                return false;
            }
        };

        if code == 0 {
            return true;
        }

        if attempt >= 2 {
            return false;
        }

        // FD exhaustion: clear tarball pack cache, bump limit, retry
        if is_fd_exhaustion(&output) {
            ctx.printer
                .warn("Nix hit file descriptor limits, clearing cache and retrying");
            clear_tarball_pack_cache();
            clear_fetcher_cache();
            raise_nofile = Some(65536);
            continue;
        }

        // Cache corruption: clear and retry
        if clear_fetcher_cache() {
            ctx.printer
                .warn("Nix cache corruption detected, clearing cache");
            continue;
        }

        return false;
    }

    false
}

/// Get GitHub token from `gh auth token`.
fn gh_auth_token() -> String {
    run_captured_command("gh", &["auth", "token"], None)
        .map(|cmd| cmd.stdout.trim().to_string())
        .unwrap_or_default()
}

/// Clear the nix fetcher cache to fix corruption issues.
fn clear_fetcher_cache() -> bool {
    let cache_path = crate::app::dirs_home().join(".cache/nix/fetcher-cache-v4.sqlite");
    if cache_path.exists() {
        std::fs::remove_file(&cache_path).is_ok()
    } else {
        false
    }
}

/// Clear the nix tarball pack cache to fix FD exhaustion from stale packfiles.
/// Recreates the empty directory so nix can write new packfiles.
fn clear_tarball_pack_cache() {
    let pack_dir = crate::app::dirs_home().join(".cache/nix/tarball-cache-v2/objects/pack");
    if pack_dir.is_dir() {
        let _ = std::fs::remove_dir_all(&pack_dir);
        let _ = std::fs::create_dir_all(&pack_dir);
    }
}

/// Commit `flake.lock` after a successful upgrade.
fn commit_flake_lock(ctx: &AppContext) {
    let repo = ctx.repo_root.display().to_string();
    let _ = run_captured_command("git", &["-C", &repo, "add", "flake.lock"], None);
    let result = run_captured_command(
        "git",
        &["-C", &repo, "commit", "-m", "chore: update flake.lock"],
        None,
    );
    match result {
        Ok(cmd) if cmd.code == 0 => {
            ctx.printer.success("Committed flake.lock");
        }
        _ => {
            ctx.printer.warn("No flake.lock changes to commit");
        }
    }
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

    // --- parse_brew_outdated_json ---

    #[test]
    fn brew_parse_extracts_formulae() {
        let json = r#"{
            "formulae": [
                {
                    "name": "git",
                    "installed_versions": ["2.43.0"],
                    "current_version": "2.44.0"
                },
                {
                    "name": "jq",
                    "installed_versions": ["1.6"],
                    "current_version": "1.7.1"
                }
            ],
            "casks": []
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("git".into(), "2.43.0".into(), "2.44.0".into()));
        assert_eq!(result[1], ("jq".into(), "1.6".into(), "1.7.1".into()));
    }

    #[test]
    fn brew_parse_extracts_casks() {
        let json = r#"{
            "formulae": [],
            "casks": [
                {
                    "name": "firefox",
                    "installed_versions": "120.0",
                    "current_version": "121.0"
                }
            ]
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            ("firefox".into(), "120.0".into(), "121.0".into())
        );
    }

    #[test]
    fn brew_parse_mixed_formulae_and_casks_sorted() {
        let json = r#"{
            "formulae": [
                {
                    "name": "zsh",
                    "installed_versions": ["5.9"],
                    "current_version": "5.9.1"
                }
            ],
            "casks": [
                {
                    "name": "alacritty",
                    "installed_versions": "0.12",
                    "current_version": "0.13"
                }
            ]
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 2);
        // Sorted by name: alacritty < zsh
        assert_eq!(result[0].0, "alacritty");
        assert_eq!(result[1].0, "zsh");
    }

    #[test]
    fn brew_parse_skips_incomplete_entries() {
        let json = r#"{
            "formulae": [
                {
                    "name": "",
                    "installed_versions": ["1.0"],
                    "current_version": "2.0"
                },
                {
                    "name": "valid",
                    "installed_versions": ["1.0"],
                    "current_version": "2.0"
                }
            ],
            "casks": []
        }"#;

        let result = parse_brew_outdated_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "valid");
    }

    #[test]
    fn brew_parse_invalid_json_returns_empty() {
        let result = parse_brew_outdated_json("not json at all");
        assert!(result.is_empty());
    }

    #[test]
    fn brew_parse_empty_json_returns_empty() {
        let result = parse_brew_outdated_json("{}");
        assert!(result.is_empty());
    }

    #[test]
    fn brew_parse_empty_arrays_returns_empty() {
        let json = r#"{"formulae": [], "casks": []}"#;
        let result = parse_brew_outdated_json(json);
        assert!(result.is_empty());
    }

    // --- is_fd_exhaustion ---

    #[test]
    fn fd_exhaustion_detected() {
        assert!(is_fd_exhaustion(
            "error: creating git packfile indexer: Too many open files"
        ));
        assert!(is_fd_exhaustion("something too many open files here"));
    }

    #[test]
    fn fd_exhaustion_not_detected_for_other_errors() {
        assert!(!is_fd_exhaustion("error: attribute not found"));
        assert!(!is_fd_exhaustion(""));
    }

    // --- build_nix_update_command ---

    #[test]
    fn build_command_without_ulimit() {
        let args = vec!["flake".into(), "update".into()];
        let result = build_nix_update_command(&args, None);
        assert_eq!(result, vec!["flake", "update"]);
    }

    #[test]
    fn build_command_with_ulimit() {
        let args = vec!["flake".into(), "update".into()];
        let result = build_nix_update_command(&args, Some(8192));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "-lc");
        assert!(result[1].contains("ulimit -n 8192"));
        assert!(result[1].contains("exec nix flake update"));
    }
}
