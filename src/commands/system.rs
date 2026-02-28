use std::collections::HashMap;
use std::path::Path;

use crate::cli::{PassthroughArgs, UpgradeArgs};
use crate::commands::context::AppContext;
use crate::domain::upgrade::{InputChange, diff_locks, load_flake_lock, short_rev};
use crate::infra::ai_engine::DEFAULT_CODEX_MODEL;
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
    let flake_changes = match run_flake_phase(args, ctx) {
        Ok(changes) => changes,
        Err(code) => return code,
    };

    // Phase 2: Brew
    if !args.skip_brew {
        run_brew_phase(args, ctx);
    }

    if args.dry_run {
        Printer::detail("Dry run complete - no changes made");
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
    if !args.skip_commit && !flake_changes.is_empty() {
        commit_flake_lock(ctx, &flake_changes);
    }

    0
}

/// Flake phase: load old lock → update → load new lock → diff → report.
///
/// Returns changed flake inputs when any changed,
/// `Err(exit_code)` on failure.
fn run_flake_phase(args: &UpgradeArgs, ctx: &AppContext) -> Result<Vec<InputChange>, i32> {
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
        return Ok(Vec::new());
    }

    if !diff.changed.is_empty() {
        println!("\n  Flake Inputs Changed ({})", diff.changed.len());
        for change in &diff.changed {
            println!("\n  {}", change.name);
            println!(
                "    {}/{} {} \u{2192} {}",
                change.owner,
                change.repo,
                short_rev(&change.old_rev),
                short_rev(&change.new_rev),
            );

            if let Some(summary) = fetch_flake_compare_summary(change) {
                println!("    summary: {}", format_compare_summary(&summary));
                if let Some(ai_summary) =
                    maybe_ai_summary(args.no_ai, || summarize_flake_change_ai(change, &summary))
                {
                    println!("    ai summary: {ai_summary}");
                }
            } else {
                ctx.printer.warn("Failed to fetch comparison from GitHub");
            }
        }
    }

    if !diff.added.is_empty() {
        Printer::detail(&format!("Added: {}", diff.added.join(", ")));
    }
    if !diff.removed.is_empty() {
        Printer::detail(&format!("Removed: {}", diff.removed.join(", ")));
    }

    Ok(diff.changed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompareSummary {
    total_commits: usize,
    commit_subjects: Vec<String>,
}

fn fetch_flake_compare_summary(change: &InputChange) -> Option<CompareSummary> {
    // Keep URL and API endpoint helpers exercised together to avoid drift.
    let _ = flake_compare_url(change);
    let endpoint = flake_compare_endpoint(change)?;
    fetch_compare_summary(&endpoint)
}

fn fetch_brew_compare_summary(package: &BrewOutdatedPackage) -> Option<CompareSummary> {
    let endpoint = brew_compare_endpoint(package)?;
    fetch_compare_summary(&endpoint)
}

fn fetch_compare_summary(endpoint: &str) -> Option<CompareSummary> {
    let output = run_captured_command("gh", &["api", endpoint], None).ok()?;
    if output.code != 0 {
        return None;
    }
    parse_compare_json(&output.stdout)
}

fn parse_compare_json(json_str: &str) -> Option<CompareSummary> {
    let data: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let commits = data.get("commits")?.as_array()?;
    if commits.is_empty() {
        return None;
    }

    let total_commits = data
        .get("total_commits")
        .and_then(serde_json::Value::as_u64)
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(commits.len());

    let commit_subjects = commits
        .iter()
        .filter_map(|commit| {
            commit
                .get("commit")
                .and_then(|value| value.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(first_commit_line)
        })
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .take(3)
        .collect();

    Some(CompareSummary {
        total_commits,
        commit_subjects,
    })
}

fn format_compare_summary(summary: &CompareSummary) -> String {
    let suffix = if summary.total_commits == 1 { "" } else { "s" };
    if summary.commit_subjects.is_empty() {
        format!("{} commit{suffix}", summary.total_commits)
    } else {
        format!(
            "{} commit{suffix}: {}",
            summary.total_commits,
            summary.commit_subjects.join(" | "),
        )
    }
}

fn maybe_ai_summary<F>(no_ai: bool, summarize: F) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if no_ai { None } else { summarize() }
}

const KEY_INPUTS: &[&str] = &["nxs", "home-manager", "nix-darwin"];

fn should_use_detailed_ai_summary(input_name: &str, commit_count: usize) -> bool {
    KEY_INPUTS.contains(&input_name) || commit_count > 50
}

fn summarize_flake_change_ai(change: &InputChange, summary: &CompareSummary) -> Option<String> {
    let target = format!(
        "flake input {} ({}/{})",
        change.name, change.owner, change.repo
    );
    let detailed = should_use_detailed_ai_summary(&change.name, summary.total_commits);
    summarize_with_ai(&target, &summary.commit_subjects, detailed, 2, 400)
}

fn summarize_brew_change_ai(
    package: &BrewOutdatedPackage,
    summary: &CompareSummary,
) -> Option<String> {
    let target = format!(
        "Homebrew package {} ({} -> {})",
        package.name, package.installed_version, package.current_version
    );
    summarize_with_ai(&target, &summary.commit_subjects, false, 1, 180)
}

fn summarize_with_ai(
    target: &str,
    commits: &[String],
    detailed: bool,
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
    if commits.is_empty() {
        return None;
    }

    if detailed {
        summarize_with_claude(target, commits, max_lines, max_chars)
            .or_else(|| summarize_with_codex(target, commits, max_lines, max_chars))
    } else {
        summarize_with_codex(target, commits, max_lines, max_chars)
            .or_else(|| summarize_with_claude(target, commits, max_lines, max_chars))
    }
}

fn summarize_with_codex(
    target: &str,
    commits: &[String],
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
    let prompt = build_codex_summary_prompt(target, commits);
    run_ai_summary(
        "codex",
        &["exec", "-m", DEFAULT_CODEX_MODEL, "--full-auto", &prompt],
        max_lines,
        max_chars,
    )
}

fn summarize_with_claude(
    target: &str,
    commits: &[String],
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
    let prompt = build_claude_summary_prompt(target, commits);
    run_ai_summary("claude", &["--print", "-p", &prompt], max_lines, max_chars)
}

fn build_codex_summary_prompt(target: &str, commits: &[String]) -> String {
    let commit_text = commits
        .iter()
        .take(30)
        .map(|commit| format!("- {commit}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Summarize these software update commits for {target} in 1 sentence.\n\
Focus on user-visible features, fixes, security updates, and breaking changes.\n\
Ignore minor refactors and dependency churn.\n\n\
Commits:\n\
{commit_text}\n\n\
Summary:"
    )
}

fn build_claude_summary_prompt(target: &str, commits: &[String]) -> String {
    let commit_text = commits
        .iter()
        .take(40)
        .map(|commit| format!("- {commit}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Summarize the key upgrade impact for {target} in 2 short sentences.\n\
Focus on behavior changes users will notice, important fixes, and any risks.\n\
Skip internal-only refactors.\n\n\
Commits:\n\
{commit_text}\n\n\
Summary:"
    )
}

fn run_ai_summary(
    program: &str,
    args: &[&str],
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
    let output = run_captured_command(program, args, None).ok()?;
    if output.code != 0 {
        return None;
    }
    parse_ai_summary_output(&output.stdout, max_lines, max_chars)
}

fn parse_ai_summary_output(output: &str, max_lines: usize, max_chars: usize) -> Option<String> {
    let lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(trim_summary_prefix)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .collect::<Vec<_>>();

    if lines.is_empty() {
        return None;
    }

    let joined = lines.join(" ");
    Some(truncate_summary(joined.trim(), max_chars))
}

fn trim_summary_prefix(line: &str) -> &str {
    line.trim_start_matches(['-', '*', ' ']).trim()
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }

    let keep = max_chars.saturating_sub(3);
    let mut shortened = chars.into_iter().take(keep).collect::<String>();
    while shortened.ends_with(' ') {
        shortened.pop();
    }
    shortened.push_str("...");
    shortened
}

fn first_commit_line(message: &str) -> &str {
    message.lines().next().map_or("", str::trim)
}

fn flake_compare_endpoint(change: &InputChange) -> Option<String> {
    let old = short_rev(&change.old_rev);
    let new = short_rev(&change.new_rev);
    if old.is_empty() || new.is_empty() {
        return None;
    }
    Some(format!(
        "repos/{}/{}/compare/{old}...{new}",
        change.owner, change.repo
    ))
}

fn flake_compare_url(change: &InputChange) -> Option<String> {
    let old = short_rev(&change.old_rev);
    let new = short_rev(&change.new_rev);
    if old.is_empty() || new.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/{}/{}/compare/{old}...{new}",
        change.owner, change.repo
    ))
}

fn brew_compare_endpoint(package: &BrewOutdatedPackage) -> Option<String> {
    let homepage = package.homepage.as_deref()?;
    let (owner, repo) = github_owner_repo(homepage)?;
    let old = normalize_version(&package.installed_version);
    let new = normalize_version(&package.current_version);
    if old.is_empty() || new.is_empty() {
        return None;
    }
    Some(format!("repos/{owner}/{repo}/compare/{old}...{new}"))
}

/// Brew phase: check outdated packages, display, and upgrade.
fn run_brew_phase(args: &UpgradeArgs, ctx: &AppContext) {
    ctx.printer.action("Checking Homebrew updates");

    let outdated = enrich_brew_outdated(brew_outdated());

    if outdated.is_empty() {
        ctx.printer.success("All Homebrew packages up to date");
        return;
    }

    println!();
    println!("  Homebrew Outdated ({})", outdated.len());

    for package in &outdated {
        println!();
        println!("  {}", package.name);
        println!(
            "    {} \u{2192} {}",
            package.installed_version, package.current_version
        );

        if let Some(changelog_url) = &package.changelog_url {
            println!("    {changelog_url}");
        } else if let Some(homepage) = &package.homepage {
            println!("    {homepage}");
        }

        if let Some(ai_summary) = maybe_ai_summary(args.no_ai, || {
            fetch_brew_compare_summary(package)
                .and_then(|summary| summarize_brew_change_ai(package, &summary))
        }) {
            println!("    ai summary: {ai_summary}");
        }
    }

    if args.dry_run {
        return;
    }

    ctx.printer
        .action(&format!("Upgrading {} Homebrew packages", outdated.len()));
    println!();

    let mut upgrade_args = vec!["upgrade"];
    upgrade_args.extend(outdated.iter().map(|package| package.name.as_str()));
    let code = match run_indented_command("brew", &upgrade_args, None, &ctx.printer, "  ") {
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
fn brew_outdated() -> Vec<BrewOutdatedPackage> {
    let output = match run_captured_command("brew", &["outdated", "--json"], None) {
        Ok(cmd) if cmd.code == 0 => cmd.stdout,
        _ => return Vec::new(),
    };
    parse_brew_outdated_json(&output)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrewOutdatedPackage {
    name: String,
    installed_version: String,
    current_version: String,
    is_cask: bool,
    homepage: Option<String>,
    description: Option<String>,
    changelog_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrewPackageMetadata {
    homepage: Option<String>,
    description: Option<String>,
}

/// Parse brew outdated JSON into package version tuples with source kind.
fn parse_brew_outdated_json(json_str: &str) -> Vec<BrewOutdatedPackage> {
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
                results.push(BrewOutdatedPackage {
                    name: name.to_string(),
                    installed_version: installed.to_string(),
                    current_version: current.to_string(),
                    is_cask: false,
                    homepage: None,
                    description: None,
                    changelog_url: None,
                });
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
                results.push(BrewOutdatedPackage {
                    name: name.to_string(),
                    installed_version: installed.to_string(),
                    current_version: current.to_string(),
                    is_cask: true,
                    homepage: None,
                    description: None,
                    changelog_url: None,
                });
            }
        }
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
}

/// Enrich outdated packages with homepage/description and changelog URL hints.
fn enrich_brew_outdated(packages: Vec<BrewOutdatedPackage>) -> Vec<BrewOutdatedPackage> {
    if packages.is_empty() {
        return packages;
    }

    let formulae = packages
        .iter()
        .filter(|package| !package.is_cask)
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    let casks = packages
        .iter()
        .filter(|package| package.is_cask)
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();

    let formula_metadata = brew_info_metadata(&formulae, false);
    let cask_metadata = brew_info_metadata(&casks, true);

    packages
        .into_iter()
        .map(|mut package| {
            let metadata = if package.is_cask {
                cask_metadata.get(&package.name)
            } else {
                formula_metadata.get(&package.name)
            };

            if let Some(metadata) = metadata {
                package.homepage = metadata.homepage.clone();
                package.description = metadata.description.clone();
            }

            package.changelog_url = brew_compare_url(
                package.homepage.as_deref(),
                &package.installed_version,
                &package.current_version,
            );
            package
        })
        .collect()
}

fn brew_info_metadata(
    package_names: &[&str],
    is_cask: bool,
) -> HashMap<String, BrewPackageMetadata> {
    if package_names.is_empty() {
        return HashMap::new();
    }

    let mut args = vec!["info", "--json=v2"];
    if is_cask {
        args.push("--cask");
    }
    args.extend(package_names.iter().copied());

    let output = match run_captured_command("brew", &args, None) {
        Ok(cmd) if cmd.code == 0 => cmd.stdout,
        _ => return HashMap::new(),
    };

    parse_brew_info_json(&output, is_cask)
}

fn parse_brew_info_json(json_str: &str, is_cask: bool) -> HashMap<String, BrewPackageMetadata> {
    let data: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };

    let entries_key = if is_cask { "casks" } else { "formulae" };
    let name_key = if is_cask { "token" } else { "name" };

    data.get(entries_key)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get(name_key).and_then(serde_json::Value::as_str)?;
            if name.is_empty() {
                return None;
            }

            Some((
                name.to_string(),
                BrewPackageMetadata {
                    homepage: entry
                        .get("homepage")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    description: entry
                        .get("desc")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                },
            ))
        })
        .collect()
}

fn brew_compare_url(
    homepage: Option<&str>,
    installed_version: &str,
    current_version: &str,
) -> Option<String> {
    let homepage = homepage?;
    let (owner, repo) = github_owner_repo(homepage)?;
    let old = normalize_version(installed_version);
    let new = normalize_version(current_version);

    if old.is_empty() || new.is_empty() {
        return None;
    }

    Some(format!(
        "https://github.com/{owner}/{repo}/compare/{old}...{new}"
    ))
}

fn github_owner_repo(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))?;
    let path = without_scheme.strip_prefix("github.com/")?;

    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let repo_part = parts.next()?.trim();

    if owner.is_empty() || repo_part.is_empty() {
        return None;
    }

    let repo = repo_part
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(".git")
        .trim()
        .to_string();

    if repo.is_empty() {
        return None;
    }

    Some((owner.to_string(), repo))
}

fn normalize_version(version: &str) -> &str {
    let trimmed = version.trim();
    trimmed.strip_prefix('v').unwrap_or(trimmed)
}

/// Build the nix flake update command, optionally wrapped with a ulimit raise.
fn build_nix_update_command(base_args: &[String], raise_nofile: Option<u32>) -> Vec<String> {
    raise_nofile.map_or_else(
        || base_args.to_vec(),
        |limit| {
            let nix_cmd = std::iter::once("nix".to_string())
                .chain(base_args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            vec![
                "-lc".to_string(),
                format!("ulimit -n {limit} 2>/dev/null; exec {nix_cmd}"),
            ]
        },
    )
}

/// Detect file descriptor exhaustion in command output.
fn is_fd_exhaustion(output: &str) -> bool {
    output.contains("Too many open files") || output.contains("too many open files")
}

/// Detect known nix fetcher-cache corruption signatures.
fn is_cache_corruption(output: &str) -> bool {
    const INDICATORS: [&str; 2] = [
        "failed to insert entry: invalid object specified",
        "error: adding a file to a tree builder",
    ];

    INDICATORS
        .iter()
        .any(|indicator| output.contains(indicator))
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
    let mut retried_cache_corruption = false;

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

        // Cache corruption: clear fetcher cache and retry once.
        if !retried_cache_corruption && is_cache_corruption(&output) {
            retried_cache_corruption = true;
            let _ = clear_fetcher_cache();
            ctx.printer
                .warn("Nix cache corruption detected, clearing cache and retrying");
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
fn commit_flake_lock(ctx: &AppContext, flake_changes: &[InputChange]) {
    let repo = ctx.repo_root.display().to_string();
    let message = build_upgrade_commit_message(flake_changes);
    let _ = run_captured_command("git", &["-C", &repo, "add", "flake.lock"], None);
    let result = run_captured_command("git", &["-C", &repo, "commit", "-m", &message], None);
    match result {
        Ok(cmd) if cmd.code == 0 => {
            ctx.printer.success(&format!("Committed: {message}"));
        }
        Ok(cmd)
            if cmd
                .stdout
                .to_ascii_lowercase()
                .contains("nothing to commit")
                || cmd
                    .stderr
                    .to_ascii_lowercase()
                    .contains("nothing to commit") =>
        {
            Printer::detail("No changes to commit");
        }
        _ => {
            ctx.printer.error("Commit failed");
        }
    }
}

fn build_upgrade_commit_message(flake_changes: &[InputChange]) -> String {
    if flake_changes.is_empty() {
        return "Update flake inputs".to_string();
    }

    let mut names = flake_changes
        .iter()
        .map(|change| change.name.as_str())
        .take(5)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if flake_changes.len() > 5 {
        names.push(format!("+{} more", flake_changes.len() - 5));
    }
    format!("Update flake ({})", names.join(", "))
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
        Printer::detail("Run 'nx rebuild' to rebuild, or 'nx upgrade' for full upgrade");
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

fn has_nix_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "nix")
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
            Printer::detail(detail);
        }
        return Err(1);
    }

    let mut untracked: Vec<&str> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| has_nix_extension(line))
        .collect();
    untracked.sort_unstable();

    if untracked.is_empty() {
        ctx.printer.success("Git preflight passed");
        return Ok(());
    }

    ctx.printer
        .error("Untracked .nix files would be ignored by flake evaluation");
    println!();
    Printer::detail("Track these files before rebuild:");
    for rel_path in &untracked {
        Printer::detail(&format!("- {rel_path}"));
    }
    println!();
    Printer::detail(&format!(
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
    let repo = ctx.repo_root.display().to_string();

    for attempt in 0..3 {
        if attempt == 0 {
            ctx.printer.action("Rebuilding system");
        } else {
            ctx.printer.action("Retrying rebuild");
        }
        println!();

        let rebuild_cmd = build_rebuild_command(&repo, args);
        let arg_refs: Vec<&str> = rebuild_cmd.iter().map(String::as_str).collect();

        let (code, output) =
            match run_indented_command_collecting("sudo", &arg_refs, None, &ctx.printer, "  ") {
                Ok(result) => result,
                Err(err) => {
                    ctx.printer.error("Rebuild failed");
                    ctx.printer.error(&format!("{err:#}"));
                    return 1;
                }
            };

        if code == 0 {
            println!();
            ctx.printer.success("System rebuilt");
            return 0;
        }

        if attempt >= 2 || !is_fd_exhaustion(&output) {
            break;
        }

        ctx.printer
            .warn("Nix hit file descriptor limits, clearing cache and retrying");
        clear_root_tarball_pack_cache();
    }

    ctx.printer.error("Rebuild failed");
    1
}

/// Build sudo args for `darwin-rebuild switch --flake`.
fn build_rebuild_command(repo: &str, args: &PassthroughArgs) -> Vec<String> {
    let mut rebuild_args = vec![
        DARWIN_REBUILD.to_string(),
        "switch".to_string(),
        "--flake".to_string(),
        repo.to_string(),
    ];
    rebuild_args.extend(args.passthrough.iter().cloned());
    rebuild_args
}

/// Clear root's nix tarball pack cache to reduce open file pressure during rebuild.
fn clear_root_tarball_pack_cache() {
    let pack_dir = "/var/root/.cache/nix/tarball-cache-v2/objects/pack";
    let _ = run_captured_command("sudo", &["rm", "-rf", pack_dir], None);
    let _ = run_captured_command("sudo", &["mkdir", "-p", pack_dir], None);
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
    fn has_nix_extension_accepts_lowercase_nix_files() {
        assert!(has_nix_extension("home/default.nix"));
        assert!(has_nix_extension("packages/cli.nix"));
    }

    #[test]
    fn has_nix_extension_rejects_non_nix_or_uppercase_extensions() {
        assert!(!has_nix_extension("home/default.NIX"));
        assert!(!has_nix_extension("home/default.nix.bak"));
        assert!(!has_nix_extension("home/default"));
    }

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
        assert_eq!(result[0].name, "git");
        assert_eq!(result[0].installed_version, "2.43.0");
        assert_eq!(result[0].current_version, "2.44.0");
        assert!(!result[0].is_cask);
        assert_eq!(result[1].name, "jq");
        assert_eq!(result[1].installed_version, "1.6");
        assert_eq!(result[1].current_version, "1.7.1");
        assert!(!result[1].is_cask);
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
        assert_eq!(result[0].name, "firefox");
        assert_eq!(result[0].installed_version, "120.0");
        assert_eq!(result[0].current_version, "121.0");
        assert!(result[0].is_cask);
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
        assert_eq!(result[0].name, "alacritty");
        assert!(result[0].is_cask);
        assert_eq!(result[1].name, "zsh");
        assert!(!result[1].is_cask);
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
        assert_eq!(result[0].name, "valid");
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

    // --- parse_brew_info_json ---

    #[test]
    fn brew_info_parse_extracts_formula_metadata() {
        let json = r#"{
            "formulae": [
                {
                    "name": "git",
                    "homepage": "https://github.com/git/git",
                    "desc": "Distributed revision control system"
                }
            ]
        }"#;

        let result = parse_brew_info_json(json, false);
        let metadata = result.get("git").expect("git metadata should exist");
        assert_eq!(
            metadata.homepage.as_deref(),
            Some("https://github.com/git/git")
        );
        assert_eq!(
            metadata.description.as_deref(),
            Some("Distributed revision control system")
        );
    }

    #[test]
    fn brew_info_parse_extracts_cask_metadata() {
        let json = r#"{
            "casks": [
                {
                    "token": "firefox",
                    "homepage": "https://www.mozilla.org/firefox/",
                    "desc": "Web browser"
                }
            ]
        }"#;

        let result = parse_brew_info_json(json, true);
        let metadata = result
            .get("firefox")
            .expect("firefox metadata should exist");
        assert_eq!(
            metadata.homepage.as_deref(),
            Some("https://www.mozilla.org/firefox/")
        );
        assert_eq!(metadata.description.as_deref(), Some("Web browser"));
    }

    #[test]
    fn brew_info_parse_invalid_json_returns_empty() {
        let result = parse_brew_info_json("oops", false);
        assert!(result.is_empty());
    }

    // --- flake changelog metadata ---

    fn sample_input_change() -> InputChange {
        InputChange {
            name: "home-manager".to_string(),
            owner: "nix-community".to_string(),
            repo: "home-manager".to_string(),
            old_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            new_rev: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        }
    }

    #[test]
    fn flake_compare_url_uses_short_revs() {
        let url = flake_compare_url(&sample_input_change());
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/nix-community/home-manager/compare/aaaaaaa...bbbbbbb")
        );
    }

    #[test]
    fn flake_compare_endpoint_uses_short_revs() {
        let endpoint = flake_compare_endpoint(&sample_input_change());
        assert_eq!(
            endpoint.as_deref(),
            Some("repos/nix-community/home-manager/compare/aaaaaaa...bbbbbbb")
        );
    }

    #[test]
    fn parse_compare_json_extracts_commit_summary() {
        let json = r#"{
            "total_commits": 4,
            "commits": [
                {"commit": {"message": "feat: first line\n\nbody"}},
                {"commit": {"message": "fix: second line"}},
                {"commit": {"message": "chore: third line"}},
                {"commit": {"message": "docs: fourth line"}}
            ]
        }"#;

        let summary = parse_compare_json(json).expect("summary should parse");
        assert_eq!(summary.total_commits, 4);
        assert_eq!(
            summary.commit_subjects,
            vec![
                "feat: first line".to_string(),
                "fix: second line".to_string(),
                "chore: third line".to_string(),
            ]
        );
    }

    #[test]
    fn parse_compare_json_invalid_returns_none() {
        let summary = parse_compare_json("not json");
        assert!(summary.is_none());
    }

    #[test]
    fn maybe_ai_summary_respects_no_ai_gate() {
        let mut called = false;
        let summary = maybe_ai_summary(true, || {
            called = true;
            Some("should not run".to_string())
        });
        assert!(summary.is_none());
        assert!(!called);
    }

    #[test]
    fn maybe_ai_summary_runs_when_enabled() {
        let mut called = false;
        let summary = maybe_ai_summary(false, || {
            called = true;
            Some("ok".to_string())
        });
        assert_eq!(summary.as_deref(), Some("ok"));
        assert!(called);
    }

    #[test]
    fn detailed_ai_summary_for_key_input() {
        assert!(should_use_detailed_ai_summary("home-manager", 1));
        assert!(should_use_detailed_ai_summary("custom-input", 51));
        assert!(!should_use_detailed_ai_summary("custom-input", 10));
    }

    #[test]
    fn parse_ai_summary_output_compacts_and_truncates() {
        let output = "Summary: first line\n\n- second line\nthird line";
        let parsed = parse_ai_summary_output(output, 2, 30).expect("summary should parse");
        assert!(parsed.starts_with("Summary: first line second"));
        assert!(parsed.len() <= 30);
    }

    // --- changelog URL derivation ---

    #[test]
    fn github_owner_repo_extracts_standard_url() {
        let result = github_owner_repo("https://github.com/BurntSushi/ripgrep");
        assert_eq!(
            result,
            Some(("BurntSushi".to_string(), "ripgrep".to_string()))
        );
    }

    #[test]
    fn github_owner_repo_handles_git_suffix() {
        let result = github_owner_repo("https://github.com/nix-community/nixvim.git");
        assert_eq!(
            result,
            Some(("nix-community".to_string(), "nixvim".to_string()))
        );
    }

    #[test]
    fn brew_compare_url_for_github_homepage() {
        let url = brew_compare_url(
            Some("https://github.com/BurntSushi/ripgrep"),
            "v14.1.0",
            "14.1.1",
        );
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/BurntSushi/ripgrep/compare/14.1.0...14.1.1")
        );
    }

    #[test]
    fn brew_compare_url_non_github_returns_none() {
        let url = brew_compare_url(Some("https://example.com/project"), "1.0.0", "1.1.0");
        assert!(url.is_none());
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

    // --- is_cache_corruption ---

    #[test]
    fn cache_corruption_detected() {
        assert!(is_cache_corruption(
            "error: failed to insert entry: invalid object specified"
        ));
        assert!(is_cache_corruption(
            "error: adding a file to a tree builder during nix fetch"
        ));
    }

    #[test]
    fn cache_corruption_not_detected_for_other_errors() {
        assert!(!is_cache_corruption("error: something unrelated"));
        assert!(!is_cache_corruption(""));
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

    // --- build_rebuild_command ---

    #[test]
    fn rebuild_command_includes_base_args() {
        let args = PassthroughArgs {
            passthrough: Vec::new(),
        };
        let result = build_rebuild_command("/Users/test/.nix-config", &args);
        assert_eq!(result[0], DARWIN_REBUILD);
        assert_eq!(result[1], "switch");
        assert_eq!(result[2], "--flake");
        assert_eq!(result[3], "/Users/test/.nix-config");
    }

    #[test]
    fn rebuild_command_includes_passthrough_args() {
        let args = PassthroughArgs {
            passthrough: vec!["--show-trace".into()],
        };
        let result = build_rebuild_command("/test", &args);
        assert_eq!(
            result,
            vec![
                DARWIN_REBUILD.to_string(),
                "switch".to_string(),
                "--flake".to_string(),
                "/test".to_string(),
                "--show-trace".to_string(),
            ]
        );
    }
}
