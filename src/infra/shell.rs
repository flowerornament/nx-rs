use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, anyhow};

use crate::output::printer::Printer;

pub struct CapturedCommand {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_captured_command(
    program: &str,
    args: &[&str],
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
    args: &[&str],
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

    let stdout = child
        .stdout
        .take()
        .context("failed to capture child stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture child stderr")?;
    let stdout_handle = spawn_line_reader("stdout", stdout, tx.clone());
    let stderr_handle = spawn_line_reader("stderr", stderr, tx);

    for line in rx {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            println!();
        } else {
            printer.stream_line(trimmed, indent, 80);
        }
    }

    join_reader("stdout", stdout_handle)?;
    join_reader("stderr", stderr_handle)?;

    let status = child.wait().context("waiting for child process")?;
    Ok(status.code().unwrap_or(1))
}

/// Like `run_indented_command` but also returns collected output for error detection.
pub fn run_indented_command_collecting(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    printer: &Printer,
    indent: &str,
) -> anyhow::Result<(i32, String)> {
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

    let stdout = child
        .stdout
        .take()
        .context("failed to capture child stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture child stderr")?;
    let stdout_handle = spawn_line_reader("stdout", stdout, tx.clone());
    let stderr_handle = spawn_line_reader("stderr", stderr, tx);

    let mut collected = String::new();
    for line in rx {
        let trimmed = line.trim_end();
        if !collected.is_empty() {
            collected.push('\n');
        }
        collected.push_str(trimmed);
        if trimmed.is_empty() {
            println!();
        } else {
            printer.stream_line(trimmed, indent, 80);
        }
    }

    join_reader("stdout", stdout_handle)?;
    join_reader("stderr", stderr_handle)?;

    let status = child.wait().context("waiting for child process")?;
    Ok((status.code().unwrap_or(1), collected))
}

fn spawn_line_reader(
    stream_name: &'static str,
    stream: impl Read + Send + 'static,
    tx: mpsc::Sender<String>,
) -> thread::JoinHandle<anyhow::Result<()>> {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let line = line.with_context(|| format!("reading {stream_name} stream"))?;
            if tx.send(line).is_err() {
                break;
            }
        }
        Ok(())
    })
}

fn join_reader(
    stream_name: &str,
    handle: thread::JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    handle
        .join()
        .map_err(|_| anyhow!("{stream_name} reader thread panicked"))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    use crate::output::style::OutputStyle;

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("boom"))
        }
    }

    #[test]
    fn run_indented_command_surfaces_spawn_failure() {
        let printer = Printer::new(OutputStyle::from_flags(true, false, false));
        let args: &[&str] = &[];
        let err = run_indented_command("__nx_missing_command__", args, None, &printer, "  ")
            .expect_err("missing command should fail to spawn");

        assert!(
            err.to_string()
                .contains("failed to spawn __nx_missing_command__")
        );
    }

    #[test]
    fn join_reader_surfaces_stream_read_error() {
        let (tx, rx) = mpsc::channel::<String>();
        drop(rx);
        let handle = spawn_line_reader("stderr", FailingReader, tx);

        let err = join_reader("stderr", handle).expect_err("read error should be surfaced");
        assert!(err.to_string().contains("reading stderr stream"));
    }

    #[test]
    fn join_reader_surfaces_thread_panic() {
        let handle = thread::spawn(|| -> anyhow::Result<()> {
            panic!("reader panic");
        });

        let err = join_reader("stdout", handle).expect_err("panic should be surfaced");
        assert!(err.to_string().contains("stdout reader thread panicked"));
    }
}
