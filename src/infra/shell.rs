use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, anyhow};
use serde_json::Value;

use crate::output::printer::Printer;

type CommandEnv<'a> = &'a [(&'a str, &'a str)];

pub struct CapturedCommand {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Returns `stderr.trim()` if non-empty, otherwise `stdout.trim()`.
pub fn first_nonempty_output(output: &CapturedCommand) -> &str {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        output.stdout.trim()
    } else {
        stderr
    }
}

struct StreamedCommand {
    code: i32,
    collected: Option<String>,
}

/// Run a command and parse stdout as JSON while suppressing stderr noise.
pub fn run_json_command_quiet(program: &str, args: &[&str]) -> Option<Value> {
    let output = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    serde_json::from_slice(&output.stdout).ok()
}

/// Capture `git diff` output for change detection.
pub fn git_diff(cwd: &Path) -> String {
    run_captured_command("git", &["diff"], Some(cwd))
        .map(|cmd| cmd.stdout)
        .unwrap_or_default()
}

pub fn run_captured_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> anyhow::Result<CapturedCommand> {
    run_captured_command_with_env(program, args, cwd, None)
}

pub fn run_captured_command_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
) -> anyhow::Result<CapturedCommand> {
    let mut command = Command::new(program);
    configure_command(&mut command, args, cwd, env);

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
    run_indented_command_with_env(program, args, cwd, None, printer, indent)
}

pub fn run_indented_command_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    _printer: &Printer,
    indent: &str,
) -> anyhow::Result<i32> {
    Ok(run_streaming_command_with_env(program, args, cwd, env, indent, false)?.code)
}

/// Like `run_indented_command` but also returns collected output for error detection.
pub fn run_indented_command_collecting(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    printer: &Printer,
    indent: &str,
) -> anyhow::Result<(i32, String)> {
    run_indented_command_collecting_with_env(program, args, cwd, None, printer, indent)
}

pub fn run_indented_command_collecting_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    _printer: &Printer,
    indent: &str,
) -> anyhow::Result<(i32, String)> {
    let streamed = run_streaming_command_with_env(program, args, cwd, env, indent, true)?;
    Ok((streamed.code, streamed.collected.unwrap_or_default()))
}

fn run_streaming_command_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    indent: &str,
    collect_output: bool,
) -> anyhow::Result<StreamedCommand> {
    let mut command = Command::new(program);
    configure_command(&mut command, args, cwd, env);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

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

    let mut collected = collect_output.then(String::new);
    for line in rx {
        let trimmed = line.trim_end();
        if let Some(collected) = collected.as_mut() {
            if !collected.is_empty() {
                collected.push('\n');
            }
            collected.push_str(trimmed);
        }
        if trimmed.is_empty() {
            println!();
        } else {
            Printer::stream_line(trimmed, indent, 80);
        }
    }

    join_reader("stdout", stdout_handle)?;
    join_reader("stderr", stderr_handle)?;

    let status = child.wait().context("waiting for child process")?;
    Ok(StreamedCommand {
        code: status.code().unwrap_or(1),
        collected,
    })
}

fn configure_command(
    command: &mut Command,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
) {
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = env {
        command.envs(env.iter().copied());
    }
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
    use std::fs;
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

    #[test]
    fn run_json_command_quiet_parses_valid_json() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file should be created");
        fs::write(tmp.path(), "{\"ok\":true}\n").expect("fixture should be written");
        let path = tmp.path().to_str().expect("temp path should be utf-8");

        let parsed = run_json_command_quiet("cat", &[path]).expect("json should parse");
        assert_eq!(
            parsed.get("ok").and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn run_json_command_quiet_returns_none_on_invalid_json() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file should be created");
        fs::write(tmp.path(), "not-json\n").expect("fixture should be written");
        let path = tmp.path().to_str().expect("temp path should be utf-8");

        assert!(run_json_command_quiet("cat", &[path]).is_none());
    }

    #[test]
    fn run_json_command_quiet_returns_none_on_spawn_failure() {
        assert!(run_json_command_quiet("__nx_missing_command__", &[]).is_none());
    }

    #[test]
    fn run_indented_command_collecting_returns_streamed_output() {
        let printer = Printer::new(OutputStyle::from_flags(true, false, false));
        let args = ["-c", "printf 'one\\n\\nthree\\n'"];

        let (code, output) = run_indented_command_collecting("sh", &args, None, &printer, "  ")
            .expect("shell command should run");

        assert_eq!(code, 0);
        assert_eq!(output, "one\n\nthree");
    }
}
