use std::path::Path;

use crate::cli::PassthroughArgs;
use crate::commands::context::AppContext;
use crate::infra::shell::{CapturedCommand, run_captured_command, run_indented_command};
use crate::output::printer::Printer;

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
