use std::path::Path;

use crate::cli::RebuildArgs;
use crate::commands::context::SystemContext;
use crate::infra::activation_profile::ActivationPhaseProfiler;
use crate::infra::shell::{
    StreamName, first_nonempty_output, run_captured_command,
    run_indented_command_collecting_with_observer,
};
use crate::infra::timing::{
    TimingCommand, TimingPhase, TimingRecord, TimingSession, append_timing, timing_detail_lines,
    timings_path,
};
use crate::output::printer::Printer;

use crate::domain::manifest::Manifest;

use super::{DARWIN_REBUILD, lint::run_routing_lint};

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
        println!();

        let rebuild_cmd = build_rebuild_command_with_manifest(&repo, args, manifest);

        let (runner, runner_args): (&str, Vec<&str>) = if use_sudo {
            let arg_refs: Vec<&str> = rebuild_cmd.iter().map(String::as_str).collect();
            ("sudo", arg_refs)
        } else {
            let (first, rest) = rebuild_cmd
                .split_first()
                .expect("non-empty rebuild command");
            (first.as_str(), rest.iter().map(String::as_str).collect())
        };

        let (code, output) = match run_indented_command_collecting_with_observer(
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
        ) {
            Ok(result) => result,
            Err(err) => {
                ctx.printer.error("Rebuild failed");
                ctx.printer.error(&format!("{err:#}"));
                return (1, profiler.finish());
            }
        };

        if code == 0 {
            println!();
            ctx.printer.success("System rebuilt");
            return (0, profiler.finish());
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
            ctx.printer
                .warn("Nix git cache corruption detected, clearing cache and retrying");
            clear_root_git_cache();
            continue;
        }

        break;
    }

    ctx.printer.error("Rebuild failed");
    (1, profiler.finish())
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
fn clear_root_git_cache() {
    let gitv3_dir = "/var/root/.cache/nix/gitv3";
    let fetcher_db = "/var/root/.cache/nix/fetcher-cache-v4.sqlite";
    let _ = run_captured_command("sudo", &["rm", "-rf", gitv3_dir], None);
    let _ = run_captured_command("sudo", &["rm", "-f", fetcher_db], None);
}
