use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::Context;

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
) -> anyhow::Result<CapturedCommand> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let output = command
        .output()
        .with_context(|| format!("command execution failed ({program})"))?;

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
) -> anyhow::Result<i32> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn {program}"))?;

    let (tx, rx) = mpsc::channel::<String>();

    let stdout_handle = spawn_line_reader(child.stdout.take(), tx.clone());
    let stderr_handle = spawn_line_reader(child.stderr.take(), tx);

    for line in rx {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            println!();
        } else {
            printer.stream_line(trimmed, indent, 80);
        }
    }

    if let Some(h) = stdout_handle {
        let _ = h.join();
    }
    if let Some(h) = stderr_handle {
        let _ = h.join();
    }

    let status = child.wait().context("waiting for child process")?;
    Ok(status.code().unwrap_or(1))
}

fn spawn_line_reader(
    stream: Option<impl Read + Send + 'static>,
    tx: mpsc::Sender<String>,
) -> Option<thread::JoinHandle<()>> {
    stream.map(|s| {
        thread::spawn(move || {
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
    })
}
