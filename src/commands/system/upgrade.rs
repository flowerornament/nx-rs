use std::collections::HashMap;
use std::path::PathBuf;

use crate::cli::{RebuildArgs, UpgradeArgs};
use crate::commands::context::AppContext;
use crate::domain::upgrade::{
    InputChange, build_flake_update_args, diff_locks, github_owner_repo, load_flake_lock, short_rev,
};
use crate::infra::ai_engine::DEFAULT_CODEX_MODEL;
use crate::infra::nix_output::NixOutputMode;
use crate::infra::shell::{
    first_nonempty_output, first_unpresented_output, run_captured_command,
    run_captured_command_with_env, run_indented_command, run_nix_command_with_stdout,
    terminal_stdio_available,
};
use crate::output::printer::Printer;

use crate::infra::text::truncate_with_ellipsis;
use crate::infra::timing::TimingCommand;

use super::cache_preflight::{CachePreflightMode, CachePreflightOutcome, check_cache_preflight};
use super::rebuild::cmd_rebuild_with_command_result;

// ─── upgrade ─────────────────────────────────────────────────────────────────

pub fn cmd_upgrade(args: &UpgradeArgs, ctx: &AppContext) -> i32 {
    if args.dry_run() {
        ctx.printer.dry_run_banner();
    }

    // Phase 1: Flake update
    let flake_changes = match run_flake_phase(args, ctx) {
        Ok(changes) => changes,
        Err(code) => return code,
    };

    // Phase 2: Brew
    if args.should_run_brew_phase() {
        run_brew_phase(args, ctx);
    }

    if args.dry_run() {
        Printer::detail("Dry run complete - no changes made");
        return 0;
    }

    let mut repaired_paths = Vec::new();

    // Phase 3: Rebuild
    if !args.skip_rebuild() {
        if upgrade_requires_manifest_system_safety(args)
            && let Err(code) = ctx.require_manifest_system_safe("upgrade")
        {
            return code;
        }
        let rebuild = RebuildArgs {
            verbose: args.flow.verbose,
            ..RebuildArgs::default()
        };
        let system_ctx = ctx.system_context();
        if check_cache_preflight(&system_ctx, CachePreflightMode::Prompt)
            == CachePreflightOutcome::Abort
        {
            Printer::body("Cancelled before rebuild.");
            Printer::detail("flake.lock keeps the updated inputs.");
            Printer::detail("Run `git checkout flake.lock` to revert, or `nx upgrade` to retry.");
            return 0;
        }
        let rebuild_result =
            cmd_rebuild_with_command_result(&rebuild, &system_ctx, TimingCommand::Upgrade);
        if rebuild_result.code != 0 {
            return 1;
        }
        repaired_paths = rebuild_result.repaired_paths;
    }

    // Phase 4: Commit
    if !args.skip_commit()
        && (!flake_changes.is_empty() || !repaired_paths.is_empty())
        && let Err(code) = commit_flake_lock(ctx, &flake_changes, &repaired_paths)
    {
        return code;
    }

    0
}

pub(super) const fn upgrade_requires_manifest_system_safety(args: &UpgradeArgs) -> bool {
    !args.dry_run() && !args.skip_rebuild()
}

/// Flake phase: load old lock → update → load new lock → diff → report.
///
/// Returns changed flake inputs when any changed,
/// `Err(exit_code)` on failure.
fn run_flake_phase(args: &UpgradeArgs, ctx: &AppContext) -> Result<Vec<InputChange>, i32> {
    let old_inputs = load_flake_lock(&ctx.repo_root).map_err(|err| {
        ctx.printer
            .error(&format!("Could not load flake.lock before update: {err}"));
        1
    })?;
    let nix_env = if args.dry_run() {
        NixCommandEnv::default()
    } else {
        NixCommandEnv::from_gh()
    };

    let new_inputs = if args.dry_run() {
        old_inputs.clone()
    } else {
        if !stream_nix_update(args, ctx, &nix_env) {
            ctx.printer.error("Flake update failed");
            return Err(1);
        }
        load_flake_lock(&ctx.repo_root).map_err(|err| {
            ctx.printer
                .error(&format!("Could not load flake.lock after update: {err}"));
            1
        })?
    };

    let diff = diff_locks(&old_inputs, &new_inputs);

    if diff.changed.is_empty() && diff.added.is_empty() && diff.removed.is_empty() {
        ctx.printer.success("All flake inputs up to date");
        return Ok(Vec::new());
    }

    if !args.dry_run() && !diff.changed.is_empty() {
        realize_changed_flake_sources(ctx, &diff.changed, &nix_env);
    }

    if !diff.changed.is_empty() {
        Printer::heading(&format!("Flake Inputs Changed ({})", diff.changed.len()));

        // Fetch summaries and AI descriptions in parallel across all inputs.
        let no_ai = args.no_ai();
        let enriched: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = diff
                .changed
                .iter()
                .map(|change| {
                    s.spawn(move || {
                        let summary = fetch_flake_compare_summary(change);
                        let ai_summary = summary.as_ref().and_then(|sum| {
                            maybe_ai_summary(no_ai, || summarize_flake_change_ai(change, sum))
                        });
                        (change, summary, ai_summary)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for (change, summary, ai_summary) in &enriched {
            println!();
            Printer::body(&change.name);
            Printer::sub_detail(&format!(
                "{}/{} {} \u{2192} {}",
                change.owner,
                change.repo,
                short_rev(&change.old_rev),
                short_rev(&change.new_rev),
            ));

            if let Some(summary) = summary {
                Printer::sub_detail(&format_compare_summary(summary));
                if let Some(ai_summary) = ai_summary {
                    Printer::sub_detail(ai_summary);
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
pub(super) struct CompareSummary {
    pub(super) total_commits: usize,
    pub(super) commit_subjects: Vec<String>,
}

fn fetch_flake_compare_summary(change: &InputChange) -> Option<CompareSummary> {
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

pub(super) fn parse_compare_json(json_str: &str) -> Option<CompareSummary> {
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

pub(super) fn maybe_ai_summary<F>(no_ai: bool, summarize: F) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if no_ai { None } else { summarize() }
}

const KEY_INPUTS: &[&str] = &["nxs", "home-manager", "nix-darwin"];

pub(super) fn should_use_detailed_ai_summary(input_name: &str, commit_count: usize) -> bool {
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
    _detailed: bool,
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
    if commits.is_empty() {
        return None;
    }

    // Prefer Claude (Max auth) for all summaries, fall back to Codex.
    summarize_with_claude(target, commits, max_lines, max_chars)
        .or_else(|| summarize_with_codex(target, commits, max_lines, max_chars))
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

pub(super) fn parse_ai_summary_output(
    output: &str,
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
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
    Some(truncate_with_ellipsis(joined.trim(), max_chars))
}

fn trim_summary_prefix(line: &str) -> &str {
    line.trim_start_matches(['-', '*', ' ']).trim()
}

fn first_commit_line(message: &str) -> &str {
    message.lines().next().map_or("", str::trim)
}

pub(super) fn flake_compare_endpoint(change: &InputChange) -> Option<String> {
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

#[cfg(test)]
pub(super) fn flake_compare_url(change: &InputChange) -> Option<String> {
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

    let outdated = ctx
        .printer
        .with_loading("Querying Homebrew outdated packages", |_| {
            enrich_brew_outdated(brew_outdated())
        });

    if outdated.is_empty() {
        ctx.printer.success("All Homebrew packages up to date");
        return;
    }

    Printer::heading(&format!("Homebrew Outdated ({})", outdated.len()));

    // Fetch summaries and AI descriptions in parallel across all packages.
    let no_ai = args.no_ai();
    let enriched: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = outdated
            .iter()
            .map(|package| {
                s.spawn(move || {
                    let ai_summary = maybe_ai_summary(no_ai, || {
                        fetch_brew_compare_summary(package)
                            .and_then(|summary| summarize_brew_change_ai(package, &summary))
                    });
                    (package, ai_summary)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    for (package, ai_summary) in &enriched {
        println!();
        Printer::body(&package.name);
        Printer::sub_detail(&format!(
            "{} \u{2192} {}",
            package.installed_version, package.current_version
        ));

        if let Some(changelog_url) = &package.changelog_url {
            Printer::sub_detail(changelog_url);
        } else if let Some(homepage) = &package.homepage {
            Printer::sub_detail(homepage);
        }

        if let Some(ai_summary) = ai_summary {
            Printer::sub_detail(ai_summary);
        }
    }

    if args.dry_run() {
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
pub(super) struct BrewOutdatedPackage {
    pub(super) name: String,
    pub(super) installed_version: String,
    pub(super) current_version: String,
    pub(super) is_cask: bool,
    pub(super) homepage: Option<String>,
    pub(super) description: Option<String>,
    pub(super) changelog_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BrewPackageMetadata {
    pub(super) homepage: Option<String>,
    pub(super) description: Option<String>,
}

/// Parse brew outdated JSON into package version tuples with source kind.
pub(super) fn parse_brew_outdated_json(json_str: &str) -> Vec<BrewOutdatedPackage> {
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

pub(super) fn parse_brew_info_json(
    json_str: &str,
    is_cask: bool,
) -> HashMap<String, BrewPackageMetadata> {
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

pub(super) fn brew_compare_url(
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

fn normalize_version(version: &str) -> &str {
    let trimmed = version.trim();
    trimmed.strip_prefix('v').unwrap_or(trimmed)
}

/// Build a nix command, optionally wrapped with a ulimit raise.
pub(super) fn build_nix_command(
    base_args: &[String],
    raise_nofile: Option<u32>,
) -> (String, Vec<String>) {
    raise_nofile.map_or_else(
        || ("nix".to_string(), base_args.to_vec()),
        |limit| {
            let mut args = vec![
                "-lc".to_string(),
                format!("ulimit -n {limit} 2>/dev/null; exec \"$@\""),
                "nx-nix-with-ulimit".to_string(),
                "nix".to_string(),
            ];
            args.extend(base_args.iter().cloned());
            ("bash".to_string(), args)
        },
    )
}

/// Detect file descriptor exhaustion in command output.
pub(super) fn is_fd_exhaustion(output: &str) -> bool {
    output.contains("Too many open files") || output.contains("too many open files")
}

/// Detect known Nix source-cache corruption signatures.
pub(super) fn is_cache_corruption(output: &str) -> bool {
    const INDICATORS: [&str; 3] = [
        "failed to insert entry: invalid object specified",
        "error: adding a file to a tree builder",
        "object not found - no match for id",
    ];
    let output = output.to_ascii_lowercase();

    INDICATORS
        .iter()
        .any(|indicator| output.contains(indicator))
}

/// Execute `nix flake update` with GitHub token, ulimit raising, and retry.
fn stream_nix_update(args: &UpgradeArgs, ctx: &AppContext, nix_env: &NixCommandEnv) -> bool {
    let base_args = build_flake_update_args(&args.targets, &args.passthrough);

    // Proactively raise FD limit to avoid "Too many open files" from libgit2.
    let mut raise_nofile: Option<u32> = Some(8192);
    let mut retried_cache_corruption = false;

    for attempt in 0..3 {
        if attempt == 0 {
            ctx.printer.action("Updating flake inputs");
        } else {
            ctx.printer.action("Retrying flake update");
        }

        let output_mode =
            NixOutputMode::for_terminal(args.flow.verbose, terminal_stdio_available());
        let nix_args = output_mode.command_args(&base_args);
        let (program, cmd_args) = build_nix_command(&nix_args, raise_nofile);
        let arg_refs: Vec<&str> = cmd_args.iter().map(String::as_str).collect();
        let output = match nix_env.with_command_env(|env| {
            run_nix_command_with_stdout(&program, &arg_refs, Some(&ctx.repo_root), env, output_mode)
        }) {
            Ok(result) => result,
            Err(err) => {
                ctx.printer.error(&format!("{err:#}"));
                return false;
            }
        };
        let combined_output = combined_command_output(&output);

        if output.code == 0 {
            return true;
        }

        if attempt >= 2 {
            print_command_failure_detail(&output);
            return false;
        }

        // FD exhaustion: clear tarball pack cache, bump limit, retry
        if is_fd_exhaustion(&combined_output) {
            ctx.printer
                .warn("Nix hit file descriptor limits, clearing cache and retrying");
            clear_user_tarball_pack_cache();
            clear_user_fetcher_cache();
            raise_nofile = Some(65536);
            continue;
        }

        // Source-cache corruption: clear user source caches and retry once.
        if !retried_cache_corruption && is_cache_corruption(&combined_output) {
            retried_cache_corruption = true;
            clear_user_source_caches();
            ctx.printer
                .warn("Nix cache corruption detected, clearing cache and retrying");
            continue;
        }

        print_command_failure_detail(&output);
        return false;
    }

    false
}

fn combined_command_output(output: &crate::infra::shell::CapturedCommand) -> String {
    match (
        output.stdout.trim().is_empty(),
        output.stderr.trim().is_empty(),
    ) {
        (true, true) => String::new(),
        (false, true) => output.stdout.clone(),
        (true, false) => output.stderr.clone(),
        (false, false) => format!("{}\n{}", output.stdout, output.stderr),
    }
}

fn print_command_failure_detail(output: &crate::infra::shell::CapturedCommand) {
    let detail = first_unpresented_output(output);
    if !detail.is_empty() {
        Printer::detail(detail);
    }
}

fn realize_changed_flake_sources(
    ctx: &AppContext,
    changes: &[InputChange],
    nix_env: &NixCommandEnv,
) {
    let prefetches: Vec<_> = changes.iter().filter_map(flake_prefetch_ref).collect();
    if prefetches.is_empty() {
        return;
    }

    ctx.printer.action("Realizing updated flake sources");
    let mut failed = false;

    for prefetch in prefetches {
        Printer::detail(&format!("{} {}", prefetch.name, prefetch.short_rev));
        if !prefetch_flake_source_with_retry(&ctx.repo_root, &prefetch.flake_ref, nix_env) {
            failed = true;
            ctx.printer.warn(&format!(
                "Could not prefetch {}; rebuild will retry if nix reports cache corruption",
                prefetch.name
            ));
        }
    }

    if !failed {
        ctx.printer.success("Flake sources ready");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FlakePrefetch {
    pub(super) name: String,
    pub(super) short_rev: String,
    pub(super) flake_ref: String,
}

pub(super) fn flake_prefetch_ref(change: &InputChange) -> Option<FlakePrefetch> {
    let flake_ref = change.prefetch_ref.clone()?;

    Some(FlakePrefetch {
        name: change.name.clone(),
        short_rev: short_rev(&change.new_rev).to_string(),
        flake_ref,
    })
}

fn prefetch_flake_source_with_retry(
    repo_root: &std::path::Path,
    flake_ref: &str,
    nix_env: &NixCommandEnv,
) -> bool {
    let args = ["flake", "prefetch", "--json", flake_ref];
    let first = nix_env
        .with_command_env(|env| run_captured_command_with_env("nix", &args, Some(repo_root), env));
    let Ok(output) = first else {
        return false;
    };

    if output.code == 0 {
        return true;
    }

    if !is_cache_corruption(first_nonempty_output(&output)) {
        return false;
    }

    clear_user_source_caches();
    nix_env
        .with_command_env(|env| run_captured_command_with_env("nix", &args, Some(repo_root), env))
        .is_ok_and(|retry| retry.code == 0)
}

/// Get GitHub token from `gh auth token`.
fn gh_auth_token() -> String {
    run_captured_command("gh", &["auth", "token"], None)
        .map(|cmd| cmd.stdout.trim().to_string())
        .unwrap_or_default()
}

fn nix_access_tokens_config(token: &str) -> Option<String> {
    (!token.is_empty()).then(|| format!("access-tokens = github.com={token}"))
}

#[derive(Debug, Default)]
struct NixCommandEnv {
    nix_config: Option<String>,
}

impl NixCommandEnv {
    fn from_gh() -> Self {
        Self {
            nix_config: nix_access_tokens_config(&gh_auth_token()),
        }
    }

    fn with_command_env<R>(&self, run: impl FnOnce(Option<&[(&str, &str)]>) -> R) -> R {
        let env_pairs = self
            .nix_config
            .as_deref()
            .map(|config| [("NIX_CONFIG", config)]);
        run(env_pairs.as_ref().map(<[(&str, &str); 1]>::as_slice))
    }
}

/// Clear user-owned nix source caches to fix lazy git/tarball source corruption.
pub(super) fn clear_user_source_caches() {
    let cache_dir = crate::app::dirs_home().join(".cache/nix");
    let _ = std::fs::remove_dir_all(cache_dir.join("gitv3"));
    let _ = std::fs::remove_dir_all(cache_dir.join("tarball-cache-v2"));
    let _ = std::fs::remove_file(cache_dir.join("fetcher-cache-v4.sqlite"));
}

pub(super) fn clear_user_fetcher_cache() {
    let cache_dir = crate::app::dirs_home().join(".cache/nix");
    let _ = std::fs::remove_file(cache_dir.join("fetcher-cache-v4.sqlite"));
}

/// Clear the nix tarball pack cache to fix FD exhaustion from stale packfiles.
/// Recreates the empty directory so nix can write new packfiles.
pub(super) fn clear_user_tarball_pack_cache() {
    let pack_dir = crate::app::dirs_home().join(".cache/nix/tarball-cache-v2/objects/pack");
    if pack_dir.is_dir() {
        let _ = std::fs::remove_dir_all(&pack_dir);
        let _ = std::fs::create_dir_all(&pack_dir);
    }
}

/// Commit `flake.lock` and any auto-repaired files after a successful upgrade.
fn commit_flake_lock(
    ctx: &AppContext,
    flake_changes: &[InputChange],
    extra_paths: &[PathBuf],
) -> Result<(), i32> {
    let repo = ctx.repo_root.display().to_string();
    let message = build_upgrade_commit_message(flake_changes, extra_paths);
    let mut paths = Vec::new();
    if !flake_changes.is_empty() {
        paths.push("flake.lock".to_string());
    }
    for path in extra_paths {
        if let Some(path) = path.to_str() {
            paths.push(path.to_string());
        }
    }

    let mut add_args = vec!["-C", repo.as_str(), "add", "--"];
    add_args.extend(paths.iter().map(String::as_str));
    let add_result = run_captured_command("git", &add_args, None);
    match add_result {
        Ok(cmd) if cmd.code == 0 => {}
        Ok(cmd) => {
            ctx.printer.error("Commit failed");
            Printer::detail("Could not stage upgrade changes");
            let detail = first_nonempty_output(&cmd);
            if !detail.is_empty() {
                Printer::detail(detail);
            }
            return Err(1);
        }
        Err(err) => {
            ctx.printer.error("Commit failed");
            Printer::detail(&format!("Could not stage upgrade changes: {err:#}"));
            return Err(1);
        }
    }

    let result = run_captured_command("git", &["-C", &repo, "commit", "-m", &message], None);
    match result {
        Ok(cmd) if cmd.code == 0 => {
            ctx.printer.success(&format!("Committed: {message}"));
            Ok(())
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
            Ok(())
        }
        Ok(cmd) => {
            ctx.printer.error("Commit failed");
            let detail = first_nonempty_output(&cmd);
            if !detail.is_empty() {
                Printer::detail(detail);
            }
            Err(1)
        }
        Err(err) => {
            ctx.printer.error("Commit failed");
            Printer::detail(&format!("{err:#}"));
            Err(1)
        }
    }
}

fn build_upgrade_commit_message(
    flake_changes: &[InputChange],
    repaired_paths: &[PathBuf],
) -> String {
    let flake_part = if flake_changes.is_empty() {
        None
    } else {
        let mut names = flake_changes
            .iter()
            .map(|change| change.name.as_str())
            .take(5)
            .map(str::to_string)
            .collect::<Vec<_>>();
        if flake_changes.len() > 5 {
            names.push(format!("+{} more", flake_changes.len() - 5));
        }
        Some(format!("Update flake ({})", names.join(", ")))
    };

    let repair_part = format_repaired_paths(repaired_paths);
    match (flake_part, repair_part) {
        (Some(flake), Some(repair)) => format!("{flake} + fix FOD hash drift in {repair}"),
        (Some(flake), None) => flake,
        (None, Some(repair)) => format!("Fix FOD hash drift in {repair}"),
        (None, None) => "Update flake inputs".to_string(),
    }
}

fn format_repaired_paths(repaired_paths: &[PathBuf]) -> Option<String> {
    match repaired_paths {
        [] => None,
        [path] => Some(path.display().to_string()),
        [first, rest @ ..] => Some(format!("{} +{} more", first.display(), rest.len())),
    }
}
