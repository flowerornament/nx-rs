use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;

use anyhow::{Context, Result};
use rustix::fs::{FlockOperation, flock};

use crate::cli::{RebuildArgs, UpgradeArgs};
use crate::commands::context::AppContext;
use crate::domain::upgrade::{
    FlakeLockInput, InputChange, LockDiff, build_flake_update_args, diff_locks, github_owner_repo,
    load_flake_lock, parse_flake_lock_content, short_rev,
};
use crate::infra::ai_engine::DEFAULT_CODEX_MODEL;
use crate::infra::nix_output::NixOutputMode;
use crate::infra::nix_runtime::{
    DeterminateFreshness, NixDistribution, detect_installed_nix, determinate_version_status,
};
use crate::infra::persistence::write_file_atomically;
use crate::infra::shell::{
    first_nonempty_output, first_unpresented_output, run_captured_command, run_indented_command,
    run_nix_command_with_stdout, terminal_stdio_available,
};
use crate::output::printer::Printer;

use crate::infra::text::truncate_with_ellipsis;
use crate::infra::timing::TimingCommand;

use super::cache_preflight::{CachePreflightMode, CachePreflightOutcome, check_cache_preflight};
use super::nix_diagnostics::{NixCacheHome, diagnose_nix_failure};
use super::rebuild::cmd_rebuild_with_command_result;

// ─── upgrade ─────────────────────────────────────────────────────────────────

pub fn cmd_upgrade(args: &UpgradeArgs, ctx: &AppContext) -> i32 {
    if args.dry_run() {
        ctx.printer.dry_run_banner();
    }

    check_determinate_version(args, ctx);

    if upgrade_requires_manifest_system_safety(args)
        && let Err(code) = ctx.require_manifest_system_safe("upgrade")
    {
        return code;
    }

    // Phase 1: Flake update and cache admission
    let prepared = match prepare_flake_update(args, ctx) {
        Ok(prepared) => prepared,
        Err(code) => return code,
    };

    // Phase 2: Brew, after the candidate system is admitted
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
        let rebuild = RebuildArgs {
            verbose: args.flow.verbose,
            ..RebuildArgs::default()
        };
        let system_ctx = ctx.system_context();
        let rebuild_result =
            cmd_rebuild_with_command_result(&rebuild, &system_ctx, TimingCommand::Upgrade);
        if rebuild_result.code != 0 {
            return 1;
        }
        repaired_paths = rebuild_result.repaired_paths;
    }

    // Phase 4: Commit
    if !args.skip_commit()
        && (!prepared.changes.is_empty() || !repaired_paths.is_empty())
        && let Err(code) = commit_flake_lock(ctx, &prepared.changes, &repaired_paths)
    {
        return code;
    }

    0
}

pub(super) struct UpgradeLock {
    directory: File,
}

impl UpgradeLock {
    fn acquire(repo_root: &std::path::Path) -> Result<Self> {
        let directory = File::open(repo_root)
            .with_context(|| format!("opening repository {}", repo_root.display()))?;
        flock(&directory, FlockOperation::NonBlockingLockExclusive)
            .with_context(|| format!("locking repository {}", repo_root.display()))?;
        Ok(Self { directory })
    }
}

impl Drop for UpgradeLock {
    fn drop(&mut self) {
        let _ = flock(&self.directory, FlockOperation::Unlock);
    }
}

impl std::fmt::Debug for UpgradeLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("UpgradeLock")
    }
}

#[derive(Debug)]
pub(super) struct FlakeLockTransaction {
    lock: Option<UpgradeLock>,
    path: PathBuf,
    original: Vec<u8>,
    candidate: Option<Vec<u8>>,
    armed: bool,
}

impl FlakeLockTransaction {
    pub(super) fn capture(repo_root: &std::path::Path) -> Result<Self> {
        let lock = UpgradeLock::acquire(repo_root)?;
        let path = repo_root.join("flake.lock");
        let original =
            fs::read(&path).with_context(|| format!("reading original {}", path.display()))?;
        Ok(Self {
            lock: Some(lock),
            path,
            original,
            candidate: None,
            armed: true,
        })
    }

    fn original_inputs(&self) -> Result<HashMap<String, FlakeLockInput>> {
        parse_flake_lock_content(&self.original).context("parsing original flake.lock")
    }

    pub(super) fn observe_candidate(&mut self, candidate: Vec<u8>) {
        self.candidate = Some(candidate);
    }

    pub(super) fn restore(mut self) -> Result<bool> {
        self.armed = false;
        self.restore_if_owned()
    }

    pub(super) fn admit(mut self) -> UpgradeLock {
        self.armed = false;
        self.lock
            .take()
            .expect("active transaction must own the upgrade lock")
    }

    fn restore_if_owned(&self) -> Result<bool> {
        let current =
            fs::read(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
        if current == self.original {
            return Ok(false);
        }
        if self
            .candidate
            .as_ref()
            .is_some_and(|candidate| current != *candidate)
        {
            anyhow::bail!(
                "{} changed after cache evaluation; refusing to overwrite it",
                self.path.display()
            );
        }
        write_file_atomically(&self.path, &self.original)
            .with_context(|| format!("restoring original {}", self.path.display()))?;
        Ok(true)
    }
}

impl Drop for FlakeLockTransaction {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.restore_if_owned();
        }
    }
}

struct PreparedFlakeUpdate {
    changes: Vec<InputChange>,
    _lock: Option<UpgradeLock>,
}

fn prepare_flake_update(args: &UpgradeArgs, ctx: &AppContext) -> Result<PreparedFlakeUpdate, i32> {
    if args.dry_run() {
        return run_flake_phase(args, ctx).map(|changes| PreparedFlakeUpdate {
            changes,
            _lock: None,
        });
    }
    if args.skip_rebuild() {
        let lock = acquire_upgrade_lock(ctx)?;
        return run_flake_phase(args, ctx).map(|changes| PreparedFlakeUpdate {
            changes,
            _lock: Some(lock),
        });
    }

    let mut transaction = FlakeLockTransaction::capture(&ctx.repo_root).map_err(|err| {
        ctx.printer
            .error(&format!("Could not start flake.lock transaction: {err:#}"));
        1
    })?;

    let old_inputs = transaction.original_inputs().map_err(|err| {
        ctx.printer
            .error(&format!("Could not load flake.lock before update: {err:#}"));
        1
    })?;
    let candidate = match update_flake_lock(args, ctx, &old_inputs) {
        Ok(candidate) => candidate,
        Err(code) => return Err(restore_rejected_lock(transaction, ctx, code)),
    };
    transaction.observe_candidate(candidate.bytes);

    let mode = if args.allow_source_builds {
        CachePreflightMode::AllowSourceBuilds
    } else {
        CachePreflightMode::Enforce
    };
    match check_cache_preflight(&ctx.system_context(), mode) {
        CachePreflightOutcome::Admitted => {
            let lock = transaction.admit();
            Ok(PreparedFlakeUpdate {
                changes: report_flake_diff(args, ctx, candidate.diff),
                _lock: Some(lock),
            })
        }
        CachePreflightOutcome::Cancelled => {
            Printer::body("Cancelled before rebuild.");
            Err(restore_rejected_lock(transaction, ctx, 0))
        }
        CachePreflightOutcome::Failed => Err(restore_rejected_lock(transaction, ctx, 1)),
    }
}

fn acquire_upgrade_lock(ctx: &AppContext) -> Result<UpgradeLock, i32> {
    UpgradeLock::acquire(&ctx.repo_root).map_err(|err| {
        ctx.printer
            .error(&format!("Could not lock repository for upgrade: {err:#}"));
        1
    })
}

fn restore_rejected_lock(transaction: FlakeLockTransaction, ctx: &AppContext, code: i32) -> i32 {
    match transaction.restore() {
        Ok(restored) => {
            if restored {
                ctx.printer.success("Restored original flake.lock");
            }
            code
        }
        Err(err) => {
            ctx.printer
                .error(&format!("Could not restore flake.lock: {err:#}"));
            Printer::detail(
                "The candidate flake.lock may still be present; inspect it before retrying.",
            );
            1
        }
    }
}

fn check_determinate_version(args: &UpgradeArgs, ctx: &AppContext) {
    if !detect_installed_nix().is_ok_and(|installed| {
        installed.is_some_and(|nix| nix.distribution == NixDistribution::Determinate)
    }) {
        return;
    }

    ctx.printer.action("Checking Determinate Nix");
    match determinate_version_status() {
        Ok(Some(status)) => match status.freshness {
            DeterminateFreshness::Current => ctx
                .printer
                .success(&format!("Determinate Nix {} is current", status.daemon)),
            DeterminateFreshness::UpdateAvailable(latest) => {
                ctx.printer.warn(&format!(
                    "Determinate Nix {} is behind {}",
                    status.daemon, latest
                ));
                Printer::detail("Run: sudo determinate-nixd upgrade");
            }
            DeterminateFreshness::DaemonClientMismatch => {
                ctx.printer.warn(&format!(
                    "Determinate Nix daemon {} does not match client {}",
                    status.daemon, status.client
                ));
            }
            DeterminateFreshness::Unknown => {
                ctx.printer
                    .warn("Could not determine whether Determinate Nix is current");
            }
        },
        Ok(None) => {
            ctx.printer
                .warn("Could not check the Determinate Nix version");
        }
        Err(err) => {
            ctx.printer
                .warn("Could not check the Determinate Nix version");
            if args.flow.verbose {
                Printer::detail(&format!("{err:#}"));
            }
        }
    }
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
    let diff = if args.dry_run() {
        diff_locks(&old_inputs, &old_inputs)
    } else {
        update_flake_lock(args, ctx, &old_inputs)?.diff
    };
    Ok(report_flake_diff(args, ctx, diff))
}

struct CandidateFlakeLock {
    diff: LockDiff,
    bytes: Vec<u8>,
}

fn update_flake_lock(
    args: &UpgradeArgs,
    ctx: &AppContext,
    old_inputs: &HashMap<String, FlakeLockInput>,
) -> Result<CandidateFlakeLock, i32> {
    if !stream_nix_update(args, ctx, &NixCommandEnv::from_gh()) {
        ctx.printer.error("Flake update failed");
        return Err(1);
    }
    let path = ctx.repo_root.join("flake.lock");
    let bytes = fs::read(&path).map_err(|err| {
        ctx.printer
            .error(&format!("Could not load flake.lock after update: {err}"));
        1
    })?;
    let inputs = parse_flake_lock_content(&bytes).map_err(|err| {
        ctx.printer
            .error(&format!("Could not load flake.lock after update: {err}"));
        1
    })?;

    Ok(CandidateFlakeLock {
        diff: diff_locks(old_inputs, &inputs),
        bytes,
    })
}

fn report_flake_diff(args: &UpgradeArgs, ctx: &AppContext, diff: LockDiff) -> Vec<InputChange> {
    if diff.changed.is_empty() && diff.added.is_empty() && diff.removed.is_empty() {
        ctx.printer.success("All flake inputs up to date");
        return Vec::new();
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

    diff.changed
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

/// Execute `nix flake update` with the GitHub token bridge.
fn stream_nix_update(args: &UpgradeArgs, ctx: &AppContext, nix_env: &NixCommandEnv) -> bool {
    let base_args = build_flake_update_args(&args.targets, &args.passthrough);
    ctx.printer.action("Updating flake inputs");
    let output_mode = NixOutputMode::for_terminal(args.flow.verbose, terminal_stdio_available());
    let nix_args = output_mode.command_args(&base_args);
    let arg_refs = nix_args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = match nix_env.with_command_env(|env| {
        run_nix_command_with_stdout("nix", &arg_refs, Some(&ctx.repo_root), env, output_mode)
    }) {
        Ok(result) => result,
        Err(err) => {
            ctx.printer.error(&format!("{err:#}"));
            return false;
        }
    };
    if output.code == 0 {
        return true;
    }

    diagnose_nix_failure(&output, NixCacheHome::User, &ctx.printer);
    print_command_failure_detail(&output);
    false
}

fn print_command_failure_detail(output: &crate::infra::shell::CapturedCommand) {
    let detail = first_unpresented_output(output);
    if !detail.is_empty() {
        Printer::detail(detail);
    }
}

/// Get GitHub token from `gh auth token`.
fn gh_auth_token() -> String {
    run_captured_command("gh", &["auth", "token"], None)
        .map(|cmd| cmd.stdout.trim().to_string())
        .unwrap_or_default()
}

fn nix_access_tokens_config(token: &str) -> Option<String> {
    (!token.is_empty()).then(|| format!("extra-access-tokens = github.com={token}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NixConfig(String);

impl NixConfig {
    fn inherited_with(setting: &str) -> Self {
        Self::compose(std::env::var("NIX_CONFIG").ok().as_deref(), setting)
    }

    pub(super) fn compose(inherited: Option<&str>, setting: &str) -> Self {
        let inherited = inherited
            .map(str::trim_end)
            .filter(|value| !value.is_empty());
        Self(inherited.map_or_else(
            || setting.to_string(),
            |value| format!("{value}\n{setting}"),
        ))
    }

    pub(super) fn command_env(&self) -> [(&str, &str); 1] {
        [("NIX_CONFIG", &self.0)]
    }
}

#[derive(Debug, Default)]
struct NixCommandEnv {
    nix_config: Option<NixConfig>,
}

impl NixCommandEnv {
    fn from_gh() -> Self {
        Self {
            nix_config: nix_access_tokens_config(&gh_auth_token())
                .map(|setting| NixConfig::inherited_with(&setting)),
        }
    }

    fn with_command_env<R>(&self, run: impl FnOnce(Option<&[(&str, &str)]>) -> R) -> R {
        let env_pairs = self.nix_config.as_ref().map(NixConfig::command_env);
        run(env_pairs.as_ref().map(<[(&str, &str); 1]>::as_slice))
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
