use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::cli::RebuildArgs;
use crate::commands::context::SystemContext;
use crate::domain::manifest::{Manifest, PlatformKind};
use crate::infra::activation_profile::ActivationPhaseProfiler;
use crate::infra::shell::{
    StreamName, first_nonempty_output, run_captured_command,
    run_indented_command_collecting_stdout_with_observer, run_indented_command_collecting_with_env,
    run_indented_command_collecting_with_observer,
};
use crate::infra::timing::{
    TimingCommand, TimingPhase, TimingRecord, TimingSession, append_timing, timing_detail_lines,
    timings_path,
};
use crate::output::printer::Printer;

use super::{DARWIN_REBUILD, lint::run_routing_lint};

const SPLIT_DARWIN_ENV: &str = "NX_SPLIT_DARWIN";
const DARWIN_HOST_ENV: &str = "NX_DARWIN_HOST";
const SYSTEM_PROFILE_PATH_ENV: &str = "NX_SYSTEM_PROFILE_PATH";
const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system";
const SUDO_SET_HOME_ARG: &str = "-H";
const ROOT_HOME_ENV: &str = "HOME=/var/root";
const NIX_REMOTE_DAEMON_ENV: &str = "NIX_REMOTE=daemon";
const ROOT_ENV_WRAPPER: &[&str] = &["/usr/bin/env", ROOT_HOME_ENV, NIX_REMOTE_DAEMON_ENV];

pub fn cmd_rebuild(args: &RebuildArgs, ctx: &SystemContext<'_>) -> i32 {
    cmd_rebuild_with_command(args, ctx, TimingCommand::Rebuild)
}

pub fn cmd_rebuild_with_command(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
    command: TimingCommand,
) -> i32 {
    if let Err(code) = ctx.require_manifest_system_safe("rebuild") {
        return code;
    }
    let mut timing = TimingSession::new(command, ctx.repo_root);

    let code = run_rebuild(args, ctx, &mut timing);
    let record = timing.finish(code);
    finish_timing(args, ctx, &record);
    code
}

fn run_rebuild(args: &RebuildArgs, ctx: &SystemContext<'_>, timing: &mut TimingSession) -> i32 {
    if args.preflight {
        let routing_code =
            timing.record_result_phase("routing-preflight", || check_routing_preflight(ctx));
        if let Some(code) = routing_code {
            return code;
        }
    }

    let git_code = timing.record_result_phase("git-preflight", || check_git_preflight(ctx));
    if let Some(code) = git_code {
        return code;
    }

    let flake_code = timing.record_result_phase("flake-check", || check_flake(ctx));
    if let Some(code) = flake_code {
        return code;
    }

    if args.preflight {
        println!();
        ctx.printer.success("Rebuild preflight passed");
        return 0;
    }

    timing.record_exit_phase_with_children("activation", || do_rebuild(args, ctx))
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
            super::upgrade::clear_user_git_cache();
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

fn do_rebuild(args: &RebuildArgs, ctx: &SystemContext<'_>) -> (i32, Vec<TimingPhase>) {
    let repo = ctx.repo_root.display().to_string();
    let manifest = ctx.config_files.manifest();
    let use_sudo = manifest.is_none_or(|m| m.platform.sudo);
    let mut retried_cache_corruption = false;
    let mut profiler = ActivationPhaseProfiler::new();

    for attempt in 0..3 {
        if attempt == 0 {
            ctx.printer.action("Rebuilding system");
        } else {
            ctx.printer.action("Retrying rebuild");
        }

        let (code, output, split_phases, outcome) =
            match do_rebuild_once(args, ctx, manifest, &repo, use_sudo, &mut profiler) {
                Ok(result) => result,
                Err(err) => {
                    ctx.printer.error("Rebuild failed");
                    ctx.printer.error(&format!("{err:#}"));
                    let mut phases = profiler.finish();
                    phases.push(failed_phase("rebuild-error"));
                    return (1, phases);
                }
            };

        if code == 0 {
            println!();
            match outcome {
                RebuildOutcome::AlreadyCurrent => ctx.printer.success("System already current"),
                RebuildOutcome::Rebuilt => ctx.printer.success("System rebuilt"),
            }
            return (0, split_phases_or_profile(split_phases, profiler.finish()));
        }

        if attempt >= 2 {
            break;
        }

        if super::upgrade::is_fd_exhaustion(&output) {
            ctx.printer
                .warn("Nix hit file descriptor limits, clearing cache and retrying");
            clear_root_tarball_pack_cache();
            continue;
        }

        if !retried_cache_corruption && super::upgrade::is_cache_corruption(&output) {
            retried_cache_corruption = true;
            super::upgrade::clear_user_git_cache();
            ctx.printer
                .warn("Nix git cache corruption detected, clearing cache and retrying");
            clear_root_git_cache_noninteractive();
            continue;
        }

        break;
    }

    ctx.printer.error("Rebuild failed");
    (1, profiler.finish())
}

fn do_rebuild_once(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
    manifest: Option<&Manifest>,
    repo: &str,
    use_sudo: bool,
    profiler: &mut ActivationPhaseProfiler,
) -> anyhow::Result<(i32, String, Vec<TimingPhase>, RebuildOutcome)> {
    if should_use_split_darwin(args, manifest) {
        match do_split_darwin_rebuild(ctx, repo, use_sudo) {
            SplitDarwinResult::Handled(result) => return result,
            SplitDarwinResult::Fallback => {
                ctx.printer.warn("Falling back to darwin-rebuild switch");
            }
        }
    }

    let (code, output) = run_legacy_rebuild(args, ctx, manifest, repo, use_sudo, profiler)?;
    Ok((code, output, Vec::new(), RebuildOutcome::Rebuilt))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebuildOutcome {
    Rebuilt,
    AlreadyCurrent,
}

fn run_legacy_rebuild(
    args: &RebuildArgs,
    ctx: &SystemContext<'_>,
    manifest: Option<&Manifest>,
    repo: &str,
    use_sudo: bool,
    profiler: &mut ActivationPhaseProfiler,
) -> anyhow::Result<(i32, String)> {
    let rebuild_cmd = build_rebuild_command_with_manifest(repo, args, manifest);

    let (runner, runner_args): (&str, Vec<&str>) = if use_sudo {
        let arg_refs: Vec<&str> = rebuild_cmd.iter().map(String::as_str).collect();
        ("sudo", arg_refs)
    } else {
        let (first, rest) = rebuild_cmd
            .split_first()
            .expect("non-empty rebuild command");
        (first.as_str(), rest.iter().map(String::as_str).collect())
    };

    run_indented_command_collecting_with_observer(
        runner,
        &runner_args,
        None,
        None,
        ctx.printer,
        "  ",
        |stream, line| {
            if stream == StreamName::Stderr {
                profiler.observe_stderr_line(line);
            }
        },
    )
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
) -> SplitDarwinResult {
    let Some(host) = darwin_host(ctx) else {
        ctx.printer
            .warn("Split darwin rebuild could not determine host; falling back");
        return SplitDarwinResult::Fallback;
    };

    let attr = format!("{repo}#darwinConfigurations.{host}.system");
    let mut phases = Vec::new();
    let mut build_stderr = String::new();
    let (build, mut build_phase) = match timed_phase("build", || {
        ctx.printer.action("Building system configuration");
        run_indented_command_collecting_stdout_with_observer(
            "nix",
            &["build", "--json", "--no-link", &attr],
            None,
            None,
            ctx.printer,
            "  ",
            |stream, line| {
                if stream == StreamName::Stderr {
                    push_stream_line(&mut build_stderr, line);
                }
            },
        )
    }) {
        Ok(result) => result,
        Err(err) => return SplitDarwinResult::Handled(Err(err)),
    };
    build_phase.status = exit_status(build.0);
    phases.push(build_phase);
    if build.0 != 0 {
        return SplitDarwinResult::Handled(Ok((
            build.0,
            combined_stream_output(&build.1, &build_stderr),
            phases,
            RebuildOutcome::Rebuilt,
        )));
    }

    let Some(system_config) = parse_system_config_path(&build.1) else {
        ctx.printer
            .warn("Split darwin rebuild could not parse nix build output; falling back");
        return SplitDarwinResult::Fallback;
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

    let sudo_auth = match authorize_split_sudo(ctx, use_sudo) {
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

    let (activate, activate_phase) = match activate_system(ctx, use_sudo, &system_config) {
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

enum SplitDarwinResult {
    Handled(anyhow::Result<(i32, String, Vec<TimingPhase>, RebuildOutcome)>),
    Fallback,
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
    system_config: &str,
) -> anyhow::Result<((i32, String), TimingPhase)> {
    let mut profiler = ActivationPhaseProfiler::new();
    let (output, mut phase) = timed_phase("activate", || {
        ctx.printer.action("Activating system");
        run_split_command_with_observer(
            use_sudo,
            &format!("{system_config}/activate"),
            &[],
            ctx,
            |stream, line| {
                if stream == StreamName::Stderr {
                    profiler.observe_stderr_line(line);
                }
            },
        )
    })?;
    phase.status = exit_status(output.0);
    phase.children = profiler.finish();
    Ok((output, phase))
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
    if use_sudo {
        let mut sudo_args = Vec::with_capacity(args.len() + ROOT_ENV_WRAPPER.len() + 2);
        sudo_args.push(SUDO_SET_HOME_ARG);
        sudo_args.extend(ROOT_ENV_WRAPPER);
        sudo_args.push(program);
        sudo_args.extend(args.iter().copied());
        run_indented_command_collecting_with_observer(
            "sudo",
            &sudo_args,
            None,
            None,
            ctx.printer,
            "  ",
            |stream, line| observer(stream, line),
        )
    } else {
        run_indented_command_collecting_with_observer(
            program,
            args,
            None,
            None,
            ctx.printer,
            "  ",
            |stream, line| observer(stream, line),
        )
    }
}

fn sudo_noninteractive_available() -> bool {
    run_captured_command("sudo", &["-n", "true"], None).is_ok_and(|output| output.code == 0)
}

fn authorize_split_sudo(
    ctx: &SystemContext<'_>,
    use_sudo: bool,
) -> anyhow::Result<Option<((i32, String), TimingPhase)>> {
    if !use_sudo || sudo_noninteractive_available() {
        return Ok(None);
    }

    let (output, mut phase) = timed_phase("sudo-auth", || {
        ctx.printer.action("Authorizing sudo");
        run_indented_command_collecting_with_env(
            "sudo",
            &[SUDO_SET_HOME_ARG, "-v"],
            None,
            None,
            ctx.printer,
            "  ",
        )
    })?;
    phase.status = exit_status(output.0);
    Ok(Some((output, phase)))
}

fn darwin_host(ctx: &SystemContext<'_>) -> Option<String> {
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

fn push_stream_line(output: &mut String, line: &str) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(line);
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

pub(super) fn build_rebuild_command_with_manifest(
    repo: &str,
    args: &RebuildArgs,
    manifest: Option<&Manifest>,
) -> Vec<String> {
    let rebuild_bin = manifest.map_or(DARWIN_REBUILD, |m| m.platform.rebuild_command.as_str());

    let mut rebuild_args = vec![
        rebuild_bin.to_string(),
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
