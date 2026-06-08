use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, anyhow};
use serde_json::Value;

use crate::output::printer::Printer;

type CommandEnv<'a> = &'a [(&'a str, &'a str)];
type StreamObserver<'a> = &'a mut dyn FnMut(StreamName, &str);
type StreamFilter<'a> = &'a mut dyn FnMut(StreamName, &str) -> bool;
const STDERR_TEE_CAPTURE_LIMIT: usize = 256 * 1024;

pub struct CapturedCommand {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn command_path(name: &str) -> Option<String> {
    let output = Command::new("which")
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
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

struct StreamingOptions<'a> {
    collect_output: bool,
    observer: Option<StreamObserver<'a>>,
    should_render: Option<StreamFilter<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamName {
    Stdout,
    Stderr,
}

#[derive(Debug)]
struct StreamedLine {
    stream: StreamName,
    line: String,
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
    Ok(run_streaming_command_with_env(
        program,
        args,
        cwd,
        env,
        indent,
        StreamingOptions {
            collect_output: false,
            observer: None,
            should_render: None,
        },
    )?
    .code)
}

pub fn run_indented_command_collecting_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    _printer: &Printer,
    indent: &str,
) -> anyhow::Result<(i32, String)> {
    let streamed = run_streaming_command_with_env(
        program,
        args,
        cwd,
        env,
        indent,
        StreamingOptions {
            collect_output: true,
            observer: None,
            should_render: None,
        },
    )?;
    Ok((streamed.code, streamed.collected.unwrap_or_default()))
}

pub fn run_indented_command_collecting_with_observer<F>(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    _printer: &Printer,
    indent: &str,
    mut observer: F,
) -> anyhow::Result<(i32, String)>
where
    F: FnMut(StreamName, &str),
{
    let streamed = run_streaming_command_with_env(
        program,
        args,
        cwd,
        env,
        indent,
        StreamingOptions {
            collect_output: true,
            observer: Some(&mut observer),
            should_render: None,
        },
    )?;
    Ok((streamed.code, streamed.collected.unwrap_or_default()))
}

pub fn run_indented_command_collecting_filtered_with_observer<F, G>(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    indent: &str,
    mut observer: F,
    mut should_render: G,
) -> anyhow::Result<(i32, String)>
where
    F: FnMut(StreamName, &str),
    G: FnMut(StreamName, &str) -> bool,
{
    let streamed = run_streaming_command_with_env(
        program,
        args,
        cwd,
        env,
        indent,
        StreamingOptions {
            collect_output: true,
            observer: Some(&mut observer),
            should_render: Some(&mut should_render),
        },
    )?;
    Ok((streamed.code, streamed.collected.unwrap_or_default()))
}

pub fn terminal_stdio_available() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
}

pub fn terminal_stderr_available() -> bool {
    io::stderr().is_terminal()
}

pub fn run_native_command_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    _printer: &Printer,
) -> anyhow::Result<i32> {
    let mut command = Command::new(program);
    configure_command(&mut command, args, cwd, env);
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = command
        .status()
        .with_context(|| format!("failed to spawn {program}"))?;

    Ok(status.code().unwrap_or(1))
}

pub fn run_stdout_collecting_tee_stderr_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
) -> anyhow::Result<CapturedCommand> {
    let mut command = Command::new(program);
    configure_command(&mut command, args, cwd, env);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn {program}"))?;
    let stdout = child
        .stdout
        .take()
        .context("failed to capture child stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture child stderr")?;

    let stdout_handle = thread::spawn(move || collect_stream("stdout", stdout));
    let stderr_handle = thread::spawn(move || tee_stderr_stream(stderr));
    let status = child.wait().context("waiting for child process")?;
    let stdout = String::from_utf8_lossy(&join_collector("stdout", stdout_handle)?).into_owned();
    let stderr = String::from_utf8_lossy(&join_collector("stderr", stderr_handle)?).into_owned();

    Ok(CapturedCommand {
        code: status.code().unwrap_or(1),
        stdout,
        stderr,
    })
}

fn run_streaming_command_with_env(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    indent: &str,
    options: StreamingOptions<'_>,
) -> anyhow::Result<StreamedCommand> {
    let StreamingOptions {
        collect_output,
        mut observer,
        mut should_render,
    } = options;
    let mut command = Command::new(program);
    configure_command(&mut command, args, cwd, env);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn {program}"))?;

    let (tx, rx) = mpsc::channel::<StreamedLine>();

    let stdout = child
        .stdout
        .take()
        .context("failed to capture child stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture child stderr")?;
    let stdout_handle = spawn_line_reader("stdout", StreamName::Stdout, stdout, tx.clone());
    let stderr_handle = spawn_line_reader("stderr", StreamName::Stderr, stderr, tx);

    let mut collected = collect_output.then(String::new);
    for event in rx {
        let trimmed = visible_stream_line(&event.line);
        let trimmed = trimmed.as_ref();
        if let Some(observer) = observer.as_deref_mut() {
            observer(event.stream, trimmed);
        }
        if let Some(collected) = collected.as_mut() {
            if !collected.is_empty() {
                collected.push('\n');
            }
            collected.push_str(trimmed);
        }
        let render = should_render
            .as_deref_mut()
            .is_none_or(|filter| filter(event.stream, trimmed));
        if render {
            if trimmed.is_empty() {
                println!();
            } else {
                Printer::stream_line(trimmed, indent, 80);
            }
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
    stream_kind: StreamName,
    stream: impl Read + Send + 'static,
    tx: mpsc::Sender<StreamedLine>,
) -> thread::JoinHandle<anyhow::Result<()>> {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let line = line.with_context(|| format!("reading {stream_name} stream"))?;
            if tx
                .send(StreamedLine {
                    stream: stream_kind,
                    line,
                })
                .is_err()
            {
                break;
            }
        }
        Ok(())
    })
}

fn collect_stream(
    stream_name: &'static str,
    mut stream: impl Read + Send + 'static,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {stream_name} stream"))?;
    Ok(bytes)
}

fn tee_stderr_stream(mut stream: impl Read + Send + 'static) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    let mut stderr = io::stderr().lock();
    let should_tee = terminal_stdio_available();
    loop {
        let count = stream.read(&mut buf).context("reading stderr stream")?;
        if count == 0 {
            break;
        }
        if should_tee {
            stderr
                .write_all(&buf[..count])
                .context("writing child stderr")?;
            stderr.flush().context("flushing child stderr")?;
        }
        append_tail(&mut bytes, &buf[..count], STDERR_TEE_CAPTURE_LIMIT);
    }
    Ok(bytes)
}

fn append_tail(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) {
    if chunk.len() >= limit {
        bytes.clear();
        bytes.extend_from_slice(&chunk[chunk.len() - limit..]);
        return;
    }

    let excess = bytes
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(limit);
    if excess > 0 {
        bytes.drain(..excess);
    }
    bytes.extend_from_slice(chunk);
}

fn visible_stream_line(line: &str) -> Cow<'_, str> {
    if !line.contains('\r') {
        return Cow::Borrowed(line.trim_end());
    }

    let visible = line.rsplit('\r').next().unwrap_or_default().trim_end();
    Cow::Owned(visible.to_string())
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

fn join_collector(
    stream_name: &str,
    handle: thread::JoinHandle<anyhow::Result<Vec<u8>>>,
) -> anyhow::Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| anyhow!("{stream_name} reader thread panicked"))?
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
        let (tx, rx) = mpsc::channel::<StreamedLine>();
        drop(rx);
        let handle = spawn_line_reader("stderr", StreamName::Stderr, FailingReader, tx);

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

        let (code, output) =
            run_indented_command_collecting_with_env("sh", &args, None, None, &printer, "  ")
                .expect("shell command should run");

        assert_eq!(code, 0);
        assert_eq!(output, "one\n\nthree");
    }

    #[test]
    fn run_indented_command_observer_receives_stream_names() {
        let printer = Printer::new(OutputStyle::from_flags(true, false, false));
        let args = ["-c", "printf 'out\\n'; printf 'err\\n' >&2"];
        let mut seen = Vec::new();

        let (code, output) = run_indented_command_collecting_with_observer(
            "sh",
            &args,
            None,
            None,
            &printer,
            "  ",
            |stream, line| seen.push((stream, line.to_string())),
        )
        .expect("shell command should run");

        assert_eq!(code, 0);
        assert!(output.contains("out"));
        assert!(output.contains("err"));
        assert!(seen.contains(&(StreamName::Stdout, "out".to_string())));
        assert!(seen.contains(&(StreamName::Stderr, "err".to_string())));
    }

    #[test]
    fn run_indented_command_collapses_carriage_return_progress() {
        let printer = Printer::new(OutputStyle::from_flags(true, false, false));
        let args = [
            "-c",
            "printf 'remote: Counting objects:  86%%\\rremote: Counting objects: 100%% (72/72), done.\\n'",
        ];

        let (code, output) =
            run_indented_command_collecting_with_env("sh", &args, None, None, &printer, "  ")
                .expect("shell command should run");

        assert_eq!(code, 0);
        assert_eq!(output, "remote: Counting objects: 100% (72/72), done.");
    }

    #[test]
    fn run_indented_command_respects_carriage_return_clear() {
        let printer = Printer::new(OutputStyle::from_flags(true, false, false));
        let args = ["-c", "printf 'progress 99%%\\r          \\r\\n'"];

        let (code, output) =
            run_indented_command_collecting_with_env("sh", &args, None, None, &printer, "  ")
                .expect("shell command should run");

        assert_eq!(code, 0);
        assert_eq!(output, "");
    }

    #[test]
    fn run_stdout_collecting_tee_stderr_collects_both_streams() {
        let args = [
            "-c",
            "printf 'json\\n'; printf 'progress\\rprogress done\\n' >&2",
        ];

        let output = run_stdout_collecting_tee_stderr_with_env("sh", &args, None, None)
            .expect("shell command should run");

        assert_eq!(output.code, 0);
        assert_eq!(output.stdout, "json\n");
        assert!(output.stderr.contains("progress\rprogress done\n"));
    }

    #[test]
    fn run_stdout_collecting_tee_stderr_keeps_bounded_tail() {
        let args = [
            "-c",
            "printf 'json\\n'; i=0; while [ $i -lt 20000 ]; do printf '0123456789abcdef' >&2; i=$((i + 1)); done; printf 'tail-marker\\n' >&2",
        ];

        let output = run_stdout_collecting_tee_stderr_with_env("sh", &args, None, None)
            .expect("shell command should run");

        assert_eq!(output.code, 0);
        assert_eq!(output.stdout, "json\n");
        assert!(output.stderr.len() <= STDERR_TEE_CAPTURE_LIMIT);
        assert!(output.stderr.ends_with("tail-marker\n"));
    }

    #[test]
    fn terminal_stdio_detection_is_available_for_native_runner_gate() {
        let _ = terminal_stdio_available();
    }
}
