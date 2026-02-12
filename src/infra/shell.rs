use std::path::Path;
use std::process::Command;

use crate::output::printer::Printer;

pub struct CapturedCommand {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_captured_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> Result<CapturedCommand, String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let output = command
        .output()
        .map_err(|err| format!("command execution failed ({program}): {err}"))?;

    Ok(CapturedCommand {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub fn run_indented_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    printer: &Printer,
    indent: &str,
) -> Result<i32, String> {
    let output = run_captured_command(program, args, cwd)?;
    let mut merged = output.stdout;
    merged.push_str(&output.stderr);

    for raw_line in merged.replace("\r\n", "\n").lines() {
        let trimmed = raw_line.trim_end();
        if trimmed.is_empty() {
            println!();
            continue;
        }
        printer.stream_line(trimmed, indent, 80);
    }

    Ok(output.code)
}
