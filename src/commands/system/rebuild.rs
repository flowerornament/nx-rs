use std::path::Path;

use crate::cli::PassthroughArgs;
use crate::commands::context::AppContext;
use crate::infra::shell::{CapturedCommand, run_captured_command, run_indented_command_collecting};
use crate::output::printer::Printer;

use super::DARWIN_REBUILD;

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

pub(super) fn has_nix_extension(path: &str) -> bool {
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

        if attempt >= 2 || !super::upgrade::is_fd_exhaustion(&output) {
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
pub(super) fn build_rebuild_command(repo: &str, args: &PassthroughArgs) -> Vec<String> {
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
