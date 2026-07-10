use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cli::RebuildArgs;
use crate::commands::context::SystemContext;
use crate::domain::manifest::{Manifest, PlatformKind};
use crate::infra::activation_profile::ActivationPhaseProfiler;
use crate::infra::nix_output::classify_nix_chatter_line;
use crate::infra::shell::{
    StreamName, first_nonempty_output, run_captured_command, run_captured_command_with_env,
    run_indented_command_collecting_filtered_with_observer,
    run_indented_command_collecting_with_env, run_indented_command_collecting_with_observer,
    run_native_command_with_env, run_stdout_collecting_quiet_nix_stderr_with_env,
    run_stdout_collecting_tee_stderr_with_env, terminal_stderr_available, terminal_stdio_available,
};
use crate::infra::timing::{
    TimingCommand, TimingPhase, TimingRecord, TimingSession, append_timing, timing_detail_lines,
    timings_path,
};
use crate::output::printer::Printer;

use super::{
    DARWIN_REBUILD,
    cache_preflight::{CachePreflightMode, check_cache_preflight},
    fixed_output_hash::{
        FixedOutputHashMismatch, FixedOutputHashTarget, apply_fixed_output_hash_repair,
        find_fixed_output_hash_targets, parse_fixed_output_hash_mismatch, path_is_clean,
    },
    lint::run_routing_lint,
};

const SPLIT_DARWIN_ENV: &str = "NX_SPLIT_DARWIN";
const DARWIN_HOST_ENV: &str = "NX_DARWIN_HOST";
const SYSTEM_PROFILE_PATH_ENV: &str = "NX_SYSTEM_PROFILE_PATH";
const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system";
const RUN_CURRENT_DARWIN_REBUILD: &str = "/run/current-system/sw/bin/darwin-rebuild";
const SUDO_SET_HOME_ARG: &str = "-H";
const ROOT_HOME_ENV: &str = "HOME=/var/root";
const NIX_REMOTE_DAEMON_ENV: &str = "NIX_REMOTE=daemon";
const ENV_WRAPPER: &str = "/usr/bin/env";
const NIX_CONFIG_ENV_KEY: &str = "NIX_CONFIG";
const NIX_CONFIG_BAR: &str = "log-format = bar";
const NIX_CONFIG_BAR_WITH_LOGS: &str = "log-format = bar-with-logs";
const NIX_CONFIG_BAR_ASSIGNMENT: &str = "NIX_CONFIG=log-format = bar";
const NIX_CONFIG_BAR_WITH_LOGS_ASSIGNMENT: &str = "NIX_CONFIG=log-format = bar-with-logs";
const NO_AUTO_HASH_FIX_ENV: &str = "NX_NO_AUTO_HASH_FIX";
const MAX_AUTO_HASH_FIXES: usize = 3;
const MAX_REBUILD_ATTEMPTS: usize = 8;
const MAX_SOURCE_CACHE_RETRIES: usize = 3;
const MAX_FD_EXHAUSTION_RETRIES: usize = 2;
const SPLIT_NIX_BUILD_NOFILE_LIMIT: u32 = 65536;
const REBUILD_FAILURE_OUTPUT_LINES: usize = 80;

pub fn cmd_rebuild(args: &RebuildArgs, ctx: &SystemContext<'_>) -> i32 {
    cmd_rebuild_with_command(args, ctx, TimingCommand::Rebuild)
}

pub fn cmd_rebuild_with_command(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
    command: TimingCommand,
) -> i32 {
    cmd_rebuild_with_command_result(args, ctx, command).code
}

pub(super) struct RebuildCommandResult {
    pub(super) code: i32,
    pub(super) repaired_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
struct RebuildOutputMode {
    split_build: SplitBuildOutputMode,
    activation: ActivationOutputMode,
}

impl RebuildOutputMode {
    fn from_args(args: &RebuildArgs) -> Self {
        let native_activation = native_rebuild_output_enabled(args);
        Self {
            split_build: SplitBuildOutputMode::from_args(args, native_split_build_output_enabled()),
            activation: ActivationOutputMode::from_args(args, native_activation),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SplitBuildOutputMode {
    Captured,
    Native,
    NativeVerbose,
}

impl SplitBuildOutputMode {
    const fn from_args(args: &RebuildArgs, native_activation: bool) -> Self {
        if !native_activation {
            Self::Captured
        } else if args.verbose {
            Self::NativeVerbose
        } else {
            Self::Native
        }
    }

    pub(super) const fn log_format(self) -> Option<&'static str> {
        match self {
            Self::Captured => None,
            Self::Native => Some("bar"),
            Self::NativeVerbose => Some("bar-with-logs"),
        }
    }

    const fn is_native(self) -> bool {
        matches!(self, Self::Native | Self::NativeVerbose)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationOutputMode {
    Captured,
    Native(NixLogFormat),
}

impl ActivationOutputMode {
    const fn from_args(args: &RebuildArgs, native_enabled: bool) -> Self {
        if native_enabled {
            Self::Native(NixLogFormat::from_verbose(args.verbose))
        } else {
            Self::Captured
        }
    }

    const fn is_native(self) -> bool {
        matches!(self, Self::Native(_))
    }

    const fn nix_log_format(self) -> Option<NixLogFormat> {
        match self {
            Self::Captured => None,
            Self::Native(format) => Some(format),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NixLogFormat {
    Bar,
    BarWithLogs,
}

impl NixLogFormat {
    const fn from_verbose(verbose: bool) -> Self {
        if verbose {
            Self::BarWithLogs
        } else {
            Self::Bar
        }
    }

    pub(super) const fn as_arg(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::BarWithLogs => "bar-with-logs",
        }
    }

    const fn as_config(self) -> &'static str {
        match self {
            Self::Bar => NIX_CONFIG_BAR,
            Self::BarWithLogs => NIX_CONFIG_BAR_WITH_LOGS,
        }
    }

    const fn as_sudo_env_assignment(self) -> &'static str {
        match self {
            Self::Bar => NIX_CONFIG_BAR_ASSIGNMENT,
            Self::BarWithLogs => NIX_CONFIG_BAR_WITH_LOGS_ASSIGNMENT,
        }
    }
}

pub(super) fn cmd_rebuild_with_command_result(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
    command: TimingCommand,
) -> RebuildCommandResult {
    if let Err(code) = ctx.require_manifest_system_safe("rebuild") {
        return RebuildCommandResult {
            code,
            repaired_paths: Vec::new(),
        };
    }
    let mut timing = TimingSession::new(command, ctx.repo_root);

    let result = run_rebuild(args, ctx, &mut timing);
    let record = timing.finish(result.code);
    finish_timing(args, ctx, &record);
    result
}

fn run_rebuild(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
    timing: &mut TimingSession,
) -> RebuildCommandResult {
    if args.preflight {
        let routing_code =
            timing.record_result_phase("routing-preflight", || check_routing_preflight(ctx));
        if let Some(code) = routing_code {
            return RebuildCommandResult {
                code,
                repaired_paths: Vec::new(),
            };
        }
    }

    let git_code = timing.record_result_phase("git-preflight", || check_git_preflight(ctx));
    if let Some(code) = git_code {
        return RebuildCommandResult {
            code,
            repaired_paths: Vec::new(),
        };
    }

    let flake_code = timing.record_result_phase("flake-check", || check_flake(ctx));
    if let Some(code) = flake_code {
        return RebuildCommandResult {
            code,
            repaired_paths: Vec::new(),
        };
    }

    if args.preflight {
        let _ = timing.record_result_phase("cache-preflight", || {
            check_cache_preflight(ctx, CachePreflightMode::ReportOnly);
            Ok(())
        });
        println!();
        ctx.printer.success("Rebuild preflight passed");
        return RebuildCommandResult {
            code: 0,
            repaired_paths: Vec::new(),
        };
    }

    let mut repaired_paths = Vec::new();
    let code = timing.record_exit_phase_with_children("activation", || {
        let (code, phases, repairs) = do_rebuild(args, ctx);
        repaired_paths = repairs;
        (code, phases)
    });

    RebuildCommandResult {
        code,
        repaired_paths,
    }
}

fn finish_timing(args: &RebuildArgs, ctx: &SystemContext<'_>, record: &TimingRecord) {
    match append_timing(record) {
        Ok(path) => {
            if args.timing {
                print_timing(record, &path);
            }
        }
        Err(err) => {
            ctx.printer
                .warn(&format!("Failed to record rebuild timing: {err:#}"));
        }
    }
}

fn print_timing(record: &TimingRecord, path: &std::path::Path) {
    println!();
    Printer::heading("Rebuild Timing");
    Printer::detail(&format!("total: {}ms ({})", record.total_ms, record.status));
    for line in timing_detail_lines(record) {
        Printer::detail(&line);
    }
    Printer::detail(&format!("recorded: {}", path.display()));
    Printer::detail(&format!(
        "profile: NX_PROFILE_PATH=\"{}\" nx profile",
        timings_path().display()
    ));
}

fn check_routing_preflight(ctx: &SystemContext<'_>) -> Result<(), i32> {
    run_routing_lint(
        ctx,
        "Checking nx routing metadata",
        "Routing metadata passed",
        "Routing metadata failed",
        "Fix these issues before rebuild:",
    )
}

pub(super) fn has_nix_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "nix")
}

fn check_git_preflight(ctx: &SystemContext<'_>) -> Result<(), i32> {
    ctx.printer.action("Checking tracked nix files");
    let repo = ctx.repo_root.display().to_string();

    // Derive directories from manifest slots when available, fall back to hardcoded list.
    let slot_dirs = ctx.config_files.manifest().map(|m| {
        let mut extras: Vec<String> = m
            .slots
            .iter()
            .filter(|s| s.file.components().count() > 1)
            .filter_map(|s| {
                s.file
                    .components()
                    .next()
                    .and_then(|c| c.as_os_str().to_str())
                    .map(str::to_string)
            })
            .collect();
        extras.sort();
        extras.dedup();

        let mut out: Vec<String> = ["home", "packages", "system", "hosts"]
            .into_iter()
            .map(str::to_string)
            .collect();
        for dir in extras {
            if !out.contains(&dir) {
                out.push(dir);
            }
        }
        out
    });
    let default_dirs = ["home", "packages", "system", "hosts"];
    let dir_refs: Vec<&str> = slot_dirs.as_ref().map_or_else(
        || default_dirs.to_vec(),
        |dirs| dirs.iter().map(String::as_str).collect(),
    );

    let mut git_args = vec![
        "-C",
        &repo,
        "ls-files",
        "--others",
        "--exclude-standard",
        "--",
    ];
    git_args.extend_from_slice(&dir_refs);
    let args = git_args;
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

fn check_flake(ctx: &SystemContext<'_>) -> Result<(), i32> {
    let repo = ctx.repo_root.display().to_string();
    let args = ["flake", "check", &repo];

    for attempt in 0..2 {
        if attempt == 0 {
            ctx.printer.action("Checking flake");
        } else {
            ctx.printer.action("Retrying flake check");
        }

        let output = match run_captured_command("nix", &args, None) {
            Ok(output) => output,
            Err(err) => {
                ctx.printer.error(&format!("Flake check failed: {err:#}"));
                return Err(1);
            }
        };

        if output.code == 0 {
            ctx.printer.success("Flake check passed");
            return Ok(());
        }

        let err_text = first_nonempty_output(&output);
        if attempt == 0 && super::upgrade::is_cache_corruption(err_text) {
            super::upgrade::clear_user_source_caches();
            ctx.printer
                .warn("Nix cache corruption detected, clearing cache and retrying");
            continue;
        }

        ctx.printer.error("Flake check failed");
        if !err_text.is_empty() {
            println!("{err_text}");
        }
        return Err(1);
    }

    Err(1)
}

fn do_rebuild(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
) -> (i32, Vec<TimingPhase>, Vec<PathBuf>) {
    let repo = ctx.repo_root.display().to_string();
    let manifest = ctx.config_files.manifest();
    let use_sudo = manifest.is_none_or(|m| m.platform.sudo);
    let mut source_cache_retries = 0usize;
    let mut fd_exhaustion_retries = 0usize;
    let mut repaired_paths = Vec::new();
    let mut profiler = ActivationPhaseProfiler::new();
    let mut final_failure_output = None;
    let mut final_failure_phases = Vec::new();

    for attempt in 0..MAX_REBUILD_ATTEMPTS {
        if attempt == 0 {
            ctx.printer.action("Rebuilding system");
        } else {
            ctx.printer.action("Retrying rebuild");
        }

        let output_mode = RebuildOutputMode::from_args(args);
        let (code, output, split_phases, outcome) = match do_rebuild_once(
            args,
            ctx,
            manifest,
            &repo,
            use_sudo,
            output_mode,
            &mut profiler,
        ) {
            Ok(result) => result,
            Err(err) => {
                ctx.printer.error("Rebuild failed");
                ctx.printer.error(&format!("{err:#}"));
                let mut phases = profiler.finish();
                phases.push(failed_phase("rebuild-error"));
                return (1, phases, repaired_paths);
            }
        };

        if code == 0 {
            println!();
            match outcome {
                RebuildOutcome::AlreadyCurrent => ctx.printer.success("System already current"),
                RebuildOutcome::Rebuilt => ctx.printer.success("System rebuilt"),
            }
            return (
                0,
                split_phases_or_profile(split_phases, profiler.finish()),
                repaired_paths,
            );
        }

        if super::upgrade::is_fd_exhaustion(&output)
            && fd_exhaustion_retries < MAX_FD_EXHAUSTION_RETRIES
        {
            fd_exhaustion_retries += 1;
            ctx.printer
                .warn("Nix hit file descriptor limits, clearing tarball caches and retrying");
            super::upgrade::clear_user_tarball_pack_cache();
            super::upgrade::clear_user_fetcher_cache();
            if use_sudo {
                clear_root_tarball_pack_cache_noninteractive();
            }
            continue;
        }

        if super::upgrade::is_cache_corruption(&output)
            && source_cache_retries < MAX_SOURCE_CACHE_RETRIES
        {
            source_cache_retries += 1;
            super::upgrade::clear_user_source_caches();
            ctx.printer
                .warn("Nix source cache corruption detected, clearing cache and retrying");
            clear_root_git_cache_noninteractive();
            continue;
        }

        if let Some(path) = handle_fixed_output_hash_mismatch(ctx, &output, repaired_paths.len()) {
            if !repaired_paths.contains(&path) {
                repaired_paths.push(path);
            }
            continue;
        }

        if should_print_captured_rebuild_failure(output_mode, &split_phases) {
            final_failure_output = Some(output);
        }
        final_failure_phases = split_phases;
        break;
    }

    ctx.printer.error("Rebuild failed");
    if let Some(output) = final_failure_output.as_deref() {
        print_rebuild_failure_output(output);
    }
    let phases = if final_failure_phases.is_empty() {
        profiler.finish()
    } else {
        final_failure_phases
    };
    (1, phases, repaired_paths)
}

fn should_print_captured_rebuild_failure(
    output_mode: RebuildOutputMode,
    phases: &[TimingPhase],
) -> bool {
    output_mode.split_build == SplitBuildOutputMode::Captured
        && phases
            .first()
            .is_some_and(|phase| phase.name == "build" && phase.status != "ok")
}

fn print_rebuild_failure_output(output: &str) {
    let excerpt = failure_output_excerpt(output, REBUILD_FAILURE_OUTPUT_LINES);
    if excerpt.lines.is_empty() {
        return;
    }

    if excerpt.omitted == 0 {
        Printer::detail("Build failure output:");
    } else {
        Printer::detail(&format!(
            "Build failure output (last {REBUILD_FAILURE_OUTPUT_LINES} lines):"
        ));
        Printer::sub_detail(&format!("... omitted {} earlier lines", excerpt.omitted));
    }
    for line in excerpt.lines {
        Printer::sub_detail(line);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct FailureOutputExcerpt<'a> {
    pub(super) omitted: usize,
    pub(super) lines: Vec<&'a str>,
}

pub(super) fn failure_output_excerpt(output: &str, max_lines: usize) -> FailureOutputExcerpt<'_> {
    let output = output.trim_end_matches(['\n', '\r']);
    if output.is_empty() || max_lines == 0 {
        return FailureOutputExcerpt {
            omitted: 0,
            lines: Vec::new(),
        };
    }

    let lines: Vec<_> = output.lines().collect();
    let omitted = lines.len().saturating_sub(max_lines);
    FailureOutputExcerpt {
        omitted,
        lines: lines.into_iter().skip(omitted).collect(),
    }
}

fn handle_fixed_output_hash_mismatch(
    ctx: &SystemContext<'_>,
    output: &str,
    repaired_count: usize,
) -> Option<PathBuf> {
    let mismatch = parse_fixed_output_hash_mismatch(output)?;
    let targets = match find_fixed_output_hash_targets(ctx.repo_root, &mismatch.specified) {
        Ok(targets) => targets,
        Err(err) => {
            print_fixed_output_hash_hint(
                ctx,
                &mismatch,
                &[],
                Some(&format!("could not scan tracked .nix files: {err:#}")),
            );
            return None;
        }
    };

    let [target] = targets.as_slice() else {
        print_fixed_output_hash_hint(ctx, &mismatch, &targets, None);
        return None;
    };

    if env_flag(NO_AUTO_HASH_FIX_ENV) {
        print_fixed_output_hash_hint(
            ctx,
            &mismatch,
            &targets,
            Some("automatic hash repair is disabled by NX_NO_AUTO_HASH_FIX"),
        );
        return None;
    }

    if repaired_count >= MAX_AUTO_HASH_FIXES {
        print_fixed_output_hash_hint(
            ctx,
            &mismatch,
            &targets,
            Some("multiple fixed-output hash mismatches in one command need manual review"),
        );
        return None;
    }

    if !path_is_clean(ctx.repo_root, &target.path) {
        print_fixed_output_hash_hint(
            ctx,
            &mismatch,
            &targets,
            Some("matching file has pre-existing changes, so nx left it alone"),
        );
        return None;
    }

    match apply_fixed_output_hash_repair(ctx.repo_root, target, &mismatch) {
        Ok(()) => {
            ctx.printer.warn(&format!(
                "Auto-updated {}:{}: hash {} -> {} (FOD content drift); retrying",
                target.path.display(),
                target.line_number,
                mismatch.specified,
                mismatch.got
            ));
            Some(target.path.clone())
        }
        Err(err) => {
            print_fixed_output_hash_hint(
                ctx,
                &mismatch,
                &targets,
                Some(&format!("could not update matching file: {err:#}")),
            );
            None
        }
    }
}

fn print_fixed_output_hash_hint(
    ctx: &SystemContext<'_>,
    mismatch: &FixedOutputHashMismatch,
    targets: &[FixedOutputHashTarget],
    reason: Option<&str>,
) {
    ctx.printer
        .warn("Nix fixed-output hash changed during rebuild");
    if let Some(reason) = reason {
        Printer::detail(reason);
    }
    Printer::detail(&format!("specified: {}", mismatch.specified));
    Printer::detail(&format!("got:       {}", mismatch.got));

    match targets {
        [] => {
            Printer::detail("No unique tracked .nix hash assignment was found to update.");
        }
        [target] => {
            Printer::detail(&format!(
                "Update {}:{}:{} from the specified hash to the got hash, then rerun.",
                target.path.display(),
                target.line_number,
                target.column_number
            ));
        }
        _ => {
            Printer::detail("Multiple matching hash occurrences were found:");
            for target in targets {
                Printer::detail(&format!(
                    "- {}:{}:{}",
                    target.path.display(),
                    target.line_number,
                    target.column_number
                ));
            }
        }
    }
}

fn do_rebuild_once(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
    manifest: Option<&Manifest>,
    repo: &str,
    use_sudo: bool,
    output_mode: RebuildOutputMode,
    profiler: &mut ActivationPhaseProfiler,
) -> anyhow::Result<(i32, String, Vec<TimingPhase>, RebuildOutcome)> {
    if should_use_split_darwin(args, manifest) {
        match do_split_darwin_rebuild(ctx, repo, use_sudo, output_mode) {
            SplitDarwinResult::Handled(result) => return result,
            SplitDarwinResult::Fallback(rebuild_command_override) => {
                ctx.printer.action("Running darwin-rebuild switch");
                let (code, output) = run_legacy_rebuild(
                    args,
                    manifest,
                    repo,
                    use_sudo,
                    output_mode,
                    profiler,
                    rebuild_command_override.as_deref(),
                )?;
                return Ok((code, output, Vec::new(), RebuildOutcome::Rebuilt));
            }
        }
    }

    let (code, output) =
        run_legacy_rebuild(args, manifest, repo, use_sudo, output_mode, profiler, None)?;
    Ok((code, output, Vec::new(), RebuildOutcome::Rebuilt))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebuildOutcome {
    Rebuilt,
    AlreadyCurrent,
}

fn run_legacy_rebuild(
    args: &RebuildArgs,
    manifest: Option<&Manifest>,
    repo: &str,
    use_sudo: bool,
    output_mode: RebuildOutputMode,
    profiler: &mut ActivationPhaseProfiler,
    rebuild_command_override: Option<&str>,
) -> anyhow::Result<(i32, String)> {
    let rebuild_cmd = build_rebuild_command_with_manifest_and_log_format(
        repo,
        args,
        manifest,
        output_mode,
        rebuild_command_override,
    );

    let (runner, runner_args): (&str, Vec<&str>) = if use_sudo {
        let arg_refs: Vec<&str> = rebuild_cmd.iter().map(String::as_str).collect();
        ("sudo", arg_refs)
    } else {
        let (first, rest) = rebuild_cmd
            .split_first()
            .expect("non-empty rebuild command");
        (first.as_str(), rest.iter().map(String::as_str).collect())
    };

    if output_mode.activation.is_native() {
        let output = run_nix_native_output(
            runner,
            &runner_args,
            matches!(
                output_mode.activation,
                ActivationOutputMode::Native(NixLogFormat::BarWithLogs)
            ),
        )?;
        return Ok((
            output.code,
            combined_stream_output(&output.stdout, &output.stderr),
        ));
    }

    run_indented_command_collecting_filtered_with_observer(
        runner,
        &runner_args,
        None,
        None,
        "  ",
        |stream, line| {
            if stream == StreamName::Stderr {
                profiler.observe_stderr_line(line);
            }
        },
        quiet_activation_line,
    )
}

fn native_rebuild_output_enabled(args: &RebuildArgs) -> bool {
    !args.timing && terminal_stdio_available()
}

fn native_split_build_output_enabled() -> bool {
    terminal_stderr_available()
}

pub(super) fn should_use_split_darwin(args: &RebuildArgs, manifest: Option<&Manifest>) -> bool {
    if !args.passthrough.is_empty() {
        return false;
    }

    let Some(manifest) = manifest else {
        return env_flag(SPLIT_DARWIN_ENV);
    };

    manifest.platform.kind == PlatformKind::Darwin
        && manifest.platform.rebuild_command == DARWIN_REBUILD
        && (manifest.platform.split_rebuild || env_flag(SPLIT_DARWIN_ENV))
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn do_split_darwin_rebuild(
    ctx: &SystemContext<'_>,
    repo: &str,
    use_sudo: bool,
    output_mode: RebuildOutputMode,
) -> SplitDarwinResult {
    let Some(host) = darwin_host(ctx) else {
        ctx.printer
            .warn("Split darwin rebuild could not determine host; falling back");
        return SplitDarwinResult::Fallback(None);
    };

    let attr = format!("{repo}#darwinConfigurations.{host}.system");
    let mut phases = Vec::new();
    let build = match build_split_system_config(ctx, &attr, output_mode) {
        Ok(result) => result,
        Err(err) => return SplitDarwinResult::Handled(Err(err)),
    };
    phases.push(build.phase);
    if build.code != 0 {
        return SplitDarwinResult::Handled(Ok((
            build.code,
            combined_stream_output(&build.stdout, &build.stderr),
            phases,
            RebuildOutcome::Rebuilt,
        )));
    }

    let Some(system_config) = parse_system_config_path(&build.stdout) else {
        ctx.printer
            .warn("Split darwin rebuild could not parse nix build output; falling back");
        return SplitDarwinResult::Fallback(None);
    };

    let (current_system, compare_phase) =
        match timed_phase("profile-compare", || Ok(current_system_profile_target())) {
            Ok(result) => result,
            Err(err) => return SplitDarwinResult::Handled(Err(err)),
        };
    phases.push(compare_phase);

    if current_system.as_deref() == Some(system_config.as_str()) {
        phases.push(ok_phase("already-current", 0));
        return SplitDarwinResult::Handled(Ok((
            0,
            String::new(),
            phases,
            RebuildOutcome::AlreadyCurrent,
        )));
    }

    let split_activation_needs_auth = split_activation_would_prompt(use_sudo);
    if split_activation_needs_auth
        && let Some(rebuild_command) = legacy_darwin_rebuild_sudo_command(ctx.repo_root)
    {
        Printer::detail("activation: using passwordless darwin-rebuild");
        return SplitDarwinResult::Fallback(Some(rebuild_command.to_string()));
    }

    let sudo_auth = match authorize_split_sudo(ctx, split_activation_needs_auth) {
        Ok(result) => result,
        Err(err) => return SplitDarwinResult::Handled(Err(err)),
    };
    if let Some(((code, output), phase)) = sudo_auth {
        phases.push(phase);
        if code != 0 {
            return SplitDarwinResult::Handled(Ok((code, output, phases, RebuildOutcome::Rebuilt)));
        }
    }

    let (set_profile, set_profile_phase) = match set_system_profile(ctx, use_sudo, &system_config) {
        Ok(result) => result,
        Err(err) => return SplitDarwinResult::Handled(Err(err)),
    };
    phases.push(set_profile_phase);
    if set_profile.0 != 0 {
        return SplitDarwinResult::Handled(Ok((
            set_profile.0,
            set_profile.1,
            phases,
            RebuildOutcome::Rebuilt,
        )));
    }

    let (activate, activate_phase) =
        match activate_system(ctx, use_sudo, output_mode.activation, &system_config) {
            Ok(result) => result,
            Err(err) => return SplitDarwinResult::Handled(Err(err)),
        };
    phases.push(activate_phase);
    SplitDarwinResult::Handled(Ok((
        activate.0,
        activate.1,
        phases,
        RebuildOutcome::Rebuilt,
    )))
}

struct SplitBuildOutput {
    code: i32,
    stdout: String,
    stderr: String,
    phase: TimingPhase,
}

fn build_split_system_config(
    ctx: &SystemContext<'_>,
    attr: &str,
    output_mode: RebuildOutputMode,
) -> anyhow::Result<SplitBuildOutput> {
    let mut build_stderr = String::new();
    let (build_program, build_args) =
        split_nix_build_command_with_log_format(attr, output_mode.split_build.log_format());
    let build_arg_refs: Vec<&str> = build_args.iter().map(String::as_str).collect();
    let (build, mut phase) = timed_phase("build", || {
        ctx.printer.action("Building system configuration");
        if output_mode.split_build.is_native() {
            let output = run_nix_native_output(
                &build_program,
                &build_arg_refs,
                output_mode.split_build == SplitBuildOutputMode::NativeVerbose,
            )?;
            build_stderr = output.stderr;
            return Ok((output.code, output.stdout));
        }
        let output = run_captured_command_with_env(&build_program, &build_arg_refs, None, None)?;
        build_stderr = output.stderr;
        Ok((output.code, output.stdout))
    })?;
    phase.status = exit_status(build.0);
    Ok(SplitBuildOutput {
        code: build.0,
        stdout: build.1,
        stderr: build_stderr,
        phase,
    })
}

fn run_nix_native_output(
    program: &str,
    args: &[&str],
    verbose: bool,
) -> anyhow::Result<crate::infra::shell::CapturedCommand> {
    if verbose {
        run_stdout_collecting_tee_stderr_with_env(program, args, None, None)
    } else {
        run_stdout_collecting_quiet_nix_stderr_with_env(program, args, None, None)
    }
}

pub(super) fn split_nix_build_command_with_log_format(
    attr: &str,
    log_format: Option<&str>,
) -> (String, Vec<String>) {
    let mut args = vec![
        "build".to_string(),
        "--no-link".to_string(),
        "--print-out-paths".to_string(),
    ];
    if let Some(log_format) = log_format {
        args.extend(["--log-format".to_string(), log_format.to_string()]);
    }
    args.push(attr.to_string());
    super::upgrade::build_nix_command(&args, Some(SPLIT_NIX_BUILD_NOFILE_LIMIT))
}

enum SplitDarwinResult {
    Handled(anyhow::Result<(i32, String, Vec<TimingPhase>, RebuildOutcome)>),
    Fallback(Option<String>),
}

fn set_system_profile(
    ctx: &SystemContext<'_>,
    use_sudo: bool,
    system_config: &str,
) -> anyhow::Result<((i32, String), TimingPhase)> {
    let (output, mut phase) = timed_phase("profile-set", || {
        ctx.printer.action("Updating system profile");
        run_split_command(
            use_sudo,
            "nix-env",
            &["-p", SYSTEM_PROFILE, "--set", system_config],
            ctx,
        )
    })?;
    phase.status = exit_status(output.0);
    Ok((output, phase))
}

fn activate_system(
    ctx: &SystemContext<'_>,
    use_sudo: bool,
    output_mode: ActivationOutputMode,
    system_config: &str,
) -> anyhow::Result<((i32, String), TimingPhase)> {
    let mut profiler = ActivationPhaseProfiler::new();
    let (output, mut phase) = timed_phase("activate", || {
        ctx.printer.action("Activating system");
        if let Some(log_format) = output_mode.nix_log_format() {
            let code = run_split_command_native(
                use_sudo,
                &format!("{system_config}/activate"),
                &[],
                ctx,
                Some(log_format),
            )?;
            return Ok((code, String::new()));
        }
        run_split_command_filtered_with_observer(
            use_sudo,
            &format!("{system_config}/activate"),
            &[],
            None,
            |stream, line| {
                if stream == StreamName::Stderr {
                    profiler.observe_stderr_line(line);
                }
            },
            quiet_activation_line,
        )
    })?;
    phase.status = exit_status(output.0);
    phase.children = profiler.finish();
    Ok((output, phase))
}

fn run_split_command_native(
    use_sudo: bool,
    program: &str,
    args: &[&str],
    ctx: &SystemContext<'_>,
    log_format: Option<NixLogFormat>,
) -> anyhow::Result<i32> {
    let (runner, runner_args) = split_command_invocation(use_sudo, program, args, log_format);
    let env = (!use_sudo)
        .then_some(log_format)
        .flatten()
        .map(|format| [(NIX_CONFIG_ENV_KEY, format.as_config())]);
    run_native_command_with_env(
        runner,
        &runner_args,
        None,
        env.as_ref().map(<[(&str, &str); 1]>::as_slice),
        ctx.printer,
    )
}

fn run_split_command(
    use_sudo: bool,
    program: &str,
    args: &[&str],
    ctx: &SystemContext<'_>,
) -> anyhow::Result<(i32, String)> {
    run_split_command_with_observer(use_sudo, program, args, ctx, |_, _| {})
}

fn run_split_command_with_observer<F>(
    use_sudo: bool,
    program: &str,
    args: &[&str],
    ctx: &SystemContext<'_>,
    mut observer: F,
) -> anyhow::Result<(i32, String)>
where
    F: FnMut(StreamName, &str),
{
    let (runner, runner_args) = split_command_invocation(use_sudo, program, args, None);
    run_indented_command_collecting_with_observer(
        runner,
        &runner_args,
        None,
        None,
        ctx.printer,
        "  ",
        |stream, line| observer(stream, line),
    )
}

fn run_split_command_filtered_with_observer<F, G>(
    use_sudo: bool,
    program: &str,
    args: &[&str],
    log_format: Option<NixLogFormat>,
    mut observer: F,
    should_render: G,
) -> anyhow::Result<(i32, String)>
where
    F: FnMut(StreamName, &str),
    G: FnMut(StreamName, &str) -> bool,
{
    let (runner, runner_args) = split_command_invocation(use_sudo, program, args, log_format);
    let env = (!use_sudo)
        .then_some(log_format)
        .flatten()
        .map(|format| [(NIX_CONFIG_ENV_KEY, format.as_config())]);
    run_indented_command_collecting_filtered_with_observer(
        runner,
        &runner_args,
        None,
        env.as_ref().map(<[(&str, &str); 1]>::as_slice),
        "  ",
        |stream, line| observer(stream, line),
        should_render,
    )
}

pub(super) fn quiet_activation_line(stream: StreamName, line: &str) -> bool {
    stream != StreamName::Stderr || classify_nix_chatter_line(line).is_none()
}

fn split_command_invocation<'a>(
    use_sudo: bool,
    program: &'a str,
    args: &'a [&'a str],
    log_format: Option<NixLogFormat>,
) -> (&'a str, Vec<&'a str>) {
    if !use_sudo {
        return (program, args.to_vec());
    }

    let mut sudo_args = Vec::with_capacity(args.len() + 6);
    sudo_args.push(SUDO_SET_HOME_ARG);
    sudo_args.push(ENV_WRAPPER);
    sudo_args.push(ROOT_HOME_ENV);
    sudo_args.push(NIX_REMOTE_DAEMON_ENV);
    if let Some(format) = log_format {
        sudo_args.push(format.as_sudo_env_assignment());
    }
    sudo_args.push(program);
    sudo_args.extend(args.iter().copied());
    ("sudo", sudo_args)
}

fn sudo_noninteractive_available() -> bool {
    run_captured_command("sudo", &["-n", "true"], None).is_ok_and(|output| output.code == 0)
}

fn split_activation_would_prompt(use_sudo: bool) -> bool {
    use_sudo && !sudo_noninteractive_available()
}

fn legacy_darwin_rebuild_sudo_command(repo: &Path) -> Option<&'static str> {
    [DARWIN_REBUILD, RUN_CURRENT_DARWIN_REBUILD]
        .into_iter()
        .find(|command| legacy_darwin_rebuild_sudo_available(repo, command))
}

fn legacy_darwin_rebuild_sudo_available(repo: &Path, rebuild_command: &str) -> bool {
    let repo = repo.display().to_string();
    run_captured_command(
        "sudo",
        &["-n", "-l", rebuild_command, "switch", "--flake", &repo],
        None,
    )
    .is_ok_and(|output| output.code == 0 && !sudo_password_required(&output))
}

pub(super) fn sudo_password_required(output: &crate::infra::shell::CapturedCommand) -> bool {
    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    combined.contains("a password is required")
        || combined.contains("a terminal is required to read the password")
        || combined.contains("no tty present")
}

fn authorize_split_sudo(
    ctx: &SystemContext<'_>,
    split_activation_needs_auth: bool,
) -> anyhow::Result<Option<((i32, String), TimingPhase)>> {
    if !split_activation_needs_auth {
        return Ok(None);
    }

    let (output, mut phase) = timed_phase("sudo-auth", || {
        ctx.printer
            .action("Authorizing sudo for system profile update and activation");
        run_indented_command_collecting_with_env("sudo", &["-v"], None, None, ctx.printer, "  ")
    })?;
    phase.status = exit_status(output.0);
    Ok(Some((output, phase)))
}

pub(super) fn darwin_host(ctx: &SystemContext<'_>) -> Option<String> {
    std::env::var(DARWIN_HOST_ENV)
        .ok()
        .filter(|host| !host.trim().is_empty())
        .or_else(|| captured_trimmed("scutil", &["--get", "LocalHostName"], None))
        .or_else(|| captured_trimmed("hostname", &["-s"], None))
        .inspect(|host| Printer::detail(&format!("darwin host: {host}")))
        .or_else(|| {
            ctx.printer.warn("Unable to resolve darwin host");
            None
        })
}

fn current_system_profile_target() -> Option<String> {
    let profile_path = std::env::var_os(SYSTEM_PROFILE_PATH_ENV).map_or_else(
        || std::path::PathBuf::from(SYSTEM_PROFILE),
        std::path::PathBuf::from,
    );
    fs::canonicalize(&profile_path)
        .ok()
        .map(|path| path.display().to_string())
        .or_else(|| {
            fs::read_link(&profile_path)
                .ok()
                .map(|path| path.display().to_string())
        })
        .or_else(|| {
            profile_path
                .to_str()
                .and_then(|path| captured_trimmed("readlink", &[path], None))
        })
}

fn captured_trimmed(program: &str, args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let output = run_captured_command(program, args, cwd).ok()?;
    if output.code != 0 {
        return None;
    }
    let trimmed = output.stdout.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(super) fn parse_system_config_path(output: &str) -> Option<String> {
    if let Some(path) = parse_plain_system_config_path(output) {
        return Some(path);
    }

    let parsed = serde_json::from_str::<serde_json::Value>(output).ok()?;
    let items = parsed.as_array()?;
    if items.len() != 1 {
        return None;
    }
    let item = items.first()?;
    item.get("outputs")?
        .get("out")?
        .as_str()
        .map(str::to_string)
}

fn parse_plain_system_config_path(output: &str) -> Option<String> {
    let mut lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let path = lines.next()?;
    if lines.next().is_some() || !path.starts_with("/nix/store/") {
        return None;
    }
    Some(path.to_string())
}

fn combined_stream_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (true, false) => stderr.to_string(),
        (false, true) => stdout.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn timed_phase<T, F>(name: &str, run: F) -> anyhow::Result<(T, TimingPhase)>
where
    F: FnOnce() -> anyhow::Result<T>,
{
    let started = Instant::now();
    let result = run()?;
    let phase = TimingPhase {
        name: name.to_string(),
        duration_ms: started.elapsed().as_millis(),
        status: "ok".to_string(),
        children: Vec::new(),
    };
    Ok((result, phase))
}

fn exit_status(code: i32) -> String {
    if code == 0 {
        "ok".to_string()
    } else {
        format!("failed:{code}")
    }
}

fn ok_phase(name: &str, duration_ms: u128) -> TimingPhase {
    TimingPhase {
        name: name.to_string(),
        duration_ms,
        status: "ok".to_string(),
        children: Vec::new(),
    }
}

fn failed_phase(name: &str) -> TimingPhase {
    TimingPhase {
        name: name.to_string(),
        duration_ms: 0,
        status: "failed".to_string(),
        children: Vec::new(),
    }
}

fn split_phases_or_profile(
    split_phases: Vec<TimingPhase>,
    profiled_phases: Vec<TimingPhase>,
) -> Vec<TimingPhase> {
    if split_phases.is_empty() {
        profiled_phases
    } else {
        split_phases
    }
}

/// Build sudo args for rebuild command (backward-compat wrapper for tests).
#[cfg(test)]
pub(super) fn build_rebuild_command(repo: &str, args: &RebuildArgs) -> Vec<String> {
    build_rebuild_command_with_manifest(repo, args, None)
}

#[cfg(test)]
pub(super) fn build_rebuild_command_with_manifest(
    repo: &str,
    args: &RebuildArgs,
    manifest: Option<&Manifest>,
) -> Vec<String> {
    build_rebuild_command_with_manifest_and_log_format(
        repo,
        args,
        manifest,
        RebuildOutputMode::from_args(args),
        None,
    )
}

fn build_rebuild_command_with_manifest_and_log_format(
    repo: &str,
    args: &RebuildArgs,
    manifest: Option<&Manifest>,
    output_mode: RebuildOutputMode,
    rebuild_command_override: Option<&str>,
) -> Vec<String> {
    let rebuild_bin = rebuild_command_override
        .or_else(|| manifest.map(|m| m.platform.rebuild_command.as_str()))
        .unwrap_or(DARWIN_REBUILD);

    let mut rebuild_args = vec![
        rebuild_bin.to_string(),
        "switch".to_string(),
        "--flake".to_string(),
        repo.to_string(),
    ];
    if rebuild_supports_nix_log_format(rebuild_bin) && !has_log_format_arg(&args.passthrough) {
        let format = output_mode
            .activation
            .nix_log_format()
            .unwrap_or_else(|| NixLogFormat::from_verbose(args.verbose));
        rebuild_args.extend(["--log-format".to_string(), format.as_arg().to_string()]);
    }
    rebuild_args.extend(args.passthrough.iter().cloned());
    rebuild_args
}

fn rebuild_supports_nix_log_format(rebuild_bin: &str) -> bool {
    rebuild_bin.rsplit('/').next() == Some("darwin-rebuild")
}

fn has_log_format_arg(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--log-format")
}

/// Clear root's nix tarball pack cache to reduce open file pressure during rebuild.
fn clear_root_tarball_pack_cache_noninteractive() {
    let pack_dir = "/var/root/.cache/nix/tarball-cache-v2/objects/pack";
    let _ = run_captured_command("sudo", &["-n", "rm", "-rf", pack_dir], None);
    let _ = run_captured_command("sudo", &["-n", "mkdir", "-p", pack_dir], None);
}

/// Clear nix git caches under root to fix tree-builder corruption.
///
/// Removes both the gitv3 object store (where corrupt git trees live)
/// and the fetcher-cache sqlite (which indexes them).
fn clear_root_git_cache_noninteractive() {
    let gitv3_dir = "/var/root/.cache/nix/gitv3";
    let fetcher_db = "/var/root/.cache/nix/fetcher-cache-v4.sqlite";
    let _ = run_captured_command("sudo", &["-n", "rm", "-rf", gitv3_dir], None);
    let _ = run_captured_command("sudo", &["-n", "rm", "-f", fetcher_db], None);
}
