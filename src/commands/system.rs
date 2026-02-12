use std::path::{Path, PathBuf};

use crate::cli::PassthroughArgs;
use crate::infra::shell::{run_captured_command, run_indented_command};
use crate::output::printer::Printer;

const DARWIN_REBUILD: &str = "/run/current-system/sw/bin/darwin-rebuild";

pub fn cmd_update(args: &PassthroughArgs, repo_root: &Path, printer: &Printer) -> i32 {
    printer.action("Updating flake inputs");

    let mut command_args = vec!["flake".to_string(), "update".to_string()];
    command_args.extend(args.passthrough.iter().cloned());
    let return_code =
        match run_indented_command("nix", &command_args, Some(repo_root), printer, "  ") {
            Ok(code) => code,
            Err(err) => {
                printer.error(&err);
                return 1;
            }
        };

    if return_code == 0 {
        println!();
        printer.success("Flake inputs updated");
        printer.detail("Run 'nx rebuild' to rebuild, or 'nx upgrade' for full upgrade");
        return 0;
    }

    printer.error("Flake update failed");
    1
}

pub fn cmd_test(repo_root: &Path, printer: &Printer) -> i32 {
    let steps: [(&str, &str, Vec<String>, Option<PathBuf>); 3] = [
        (
            "ruff",
            "ruff",
            vec!["check".to_string(), ".".to_string()],
            Some(repo_root.join("scripts/nx")),
        ),
        (
            "mypy",
            "mypy",
            vec![".".to_string()],
            Some(repo_root.join("scripts/nx")),
        ),
        (
            "tests",
            "python3",
            vec![
                "-m".to_string(),
                "unittest".to_string(),
                "discover".to_string(),
                "-s".to_string(),
                "scripts/nx/tests".to_string(),
            ],
            Some(repo_root.to_path_buf()),
        ),
    ];

    for (label, program, args, cwd) in steps {
        if run_test_step(label, program, &args, cwd.as_deref(), printer).is_err() {
            return 1;
        }
    }

    0
}

fn run_test_step(
    label: &str,
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    printer: &Printer,
) -> Result<(), ()> {
    printer.action(&format!("Running {label}"));
    println!();

    let return_code = match run_indented_command(program, args, cwd, printer, "  ") {
        Ok(code) => code,
        Err(err) => {
            printer.error(&format!("{label} failed"));
            printer.error(&err);
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

pub fn cmd_rebuild(args: &PassthroughArgs, repo_root: &Path, printer: &Printer) -> i32 {
    printer.action("Checking tracked nix files");
    let preflight_args = vec![
        "-C".to_string(),
        repo_root.display().to_string(),
        "ls-files".to_string(),
        "--others".to_string(),
        "--exclude-standard".to_string(),
        "--".to_string(),
        "home".to_string(),
        "packages".to_string(),
        "system".to_string(),
        "hosts".to_string(),
    ];
    let output = match run_captured_command("git", &preflight_args, None) {
        Ok(output) => output,
        Err(_) => {
            printer.error("Git preflight failed");
            return 1;
        }
    };

    if output.code != 0 {
        printer.error("Git preflight failed");
        let stderr = output.stderr.trim().to_string();
        if !stderr.is_empty() {
            printer.detail(&stderr);
        } else {
            let stdout = output.stdout.trim().to_string();
            if !stdout.is_empty() {
                printer.detail(&stdout);
            }
        }
        return 1;
    }

    let mut untracked: Vec<String> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.ends_with(".nix"))
        .map(ToOwned::to_owned)
        .collect();
    untracked.sort();

    if untracked.is_empty() {
        printer.success("Git preflight passed");
    } else {
        printer.error("Untracked .nix files would be ignored by flake evaluation");
        println!("\n  Track these files before rebuild:");
        for rel_path in &untracked {
            println!("  - {rel_path}");
        }
        println!("\n  Run: git -C \"{}\" add <files>", repo_root.display());
        return 1;
    }

    printer.action("Checking flake");
    let flake_args = vec![
        "flake".to_string(),
        "check".to_string(),
        repo_root.display().to_string(),
    ];
    let flake_output = match run_captured_command("nix", &flake_args, None) {
        Ok(output) => output,
        Err(err) => {
            printer.error("Flake check failed");
            println!("{err}");
            return 1;
        }
    };
    if flake_output.code != 0 {
        printer.error("Flake check failed");
        let err_text = if flake_output.stderr.trim().is_empty() {
            flake_output.stdout.trim()
        } else {
            flake_output.stderr.trim()
        };
        if !err_text.is_empty() {
            println!("{err_text}");
        }
        return 1;
    }
    printer.success("Flake check passed");

    printer.action("Rebuilding system");
    println!();
    let mut rebuild_args = vec![
        DARWIN_REBUILD.to_string(),
        "switch".to_string(),
        "--flake".to_string(),
        repo_root.display().to_string(),
    ];
    rebuild_args.extend(args.passthrough.iter().cloned());

    let return_code = match run_indented_command("sudo", &rebuild_args, None, printer, "  ") {
        Ok(code) => code,
        Err(err) => {
            printer.error("Rebuild failed");
            printer.error(&err);
            return 1;
        }
    };
    if return_code == 0 {
        println!();
        printer.success("System rebuilt");
        return 0;
    }

    printer.error("Rebuild failed");
    1
}
