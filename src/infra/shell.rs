use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, anyhow};
use serde_json::Value;

use crate::infra::activation_profile::ActivationPhaseProfiler;
use crate::infra::nix_output::{NixOutputMode, NixRecord, decode_nix_record, feed_nix_output};
use crate::infra::timing::TimingPhase;
use crate::output::printer::Printer;

type CommandEnv<'a> = &'a [(&'a str, &'a str)];
const STDERR_TEE_CAPTURE_LIMIT: usize = 256 * 1024;

pub struct CapturedCommand {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    stderr_presented: bool,
}

impl CapturedCommand {
    pub(crate) fn captured(code: i32, stdout: String, stderr: String) -> Self {
        Self {
            code,
            stdout,
            stderr,
            stderr_presented: false,
        }
    }

    pub(crate) const fn stderr_was_presented(&self) -> bool {
        self.stderr_presented
    }

    pub(crate) fn presented(code: i32, stdout: String, stderr: String) -> Self {
        Self {
            code,
            stdout,
            stderr,
            stderr_presented: true,
        }
    }
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

/// Returns command output that has not already been shown to the user.
pub fn first_unpresented_output(output: &CapturedCommand) -> &str {
    let stderr = output.stderr.trim();
    if !output.stderr_was_presented() && !stderr.is_empty() {
        stderr
    } else {
        output.stdout.trim()
    }
}

struct StreamedCommand {
    code: i32,
    collected: Option<String>,
}

struct StderrCapture {
    bytes: Vec<u8>,
    phases: Vec<TimingPhase>,
    presented: bool,
}

#[derive(Debug)]
struct StreamedLine {
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

    Ok(CapturedCommand::captured(
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
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

pub fn terminal_stdio_available() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
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

pub fn run_nix_command_with_stdout(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    mode: NixOutputMode,
) -> anyhow::Result<CapturedCommand> {
    Ok(run_nix_command_with_stdout_profiled(program, args, cwd, env, mode)?.0)
}

pub fn run_nix_command_with_stdout_profiled(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
    mode: NixOutputMode,
) -> anyhow::Result<(CapturedCommand, Vec<TimingPhase>)> {
    if mode.is_native() {
        return Ok((
            run_stdout_collecting_native_stderr(program, args, cwd, env)?,
            Vec::new(),
        ));
    }

    run_stdout_collecting_stderr_with_env_profiled(program, args, cwd, env)
}

fn run_stdout_collecting_native_stderr(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
) -> anyhow::Result<CapturedCommand> {
    let mut command = Command::new(program);
    configure_command(&mut command, args, cwd, env);
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let output = command
        .output()
        .with_context(|| format!("failed to spawn {program}"))?;

    Ok(CapturedCommand::presented(
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::new(),
    ))
}

fn run_stdout_collecting_stderr_with_env_profiled(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: Option<CommandEnv<'_>>,
) -> anyhow::Result<(CapturedCommand, Vec<TimingPhase>)> {
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
    let stderr_handle = thread::spawn(move || tee_nix_stderr_stream(stderr));
    let status = child.wait().context("waiting for child process")?;
    let stdout = String::from_utf8_lossy(&join_collector("stdout", stdout_handle)?).into_owned();
    let stderr = join_stderr_collector(stderr_handle)?;
    let replayed = replay_success_diagnostics(
        status.success(),
        stderr.presented,
        &stderr.bytes,
        &mut io::stderr(),
    )?;
    let stderr_presented = stderr.presented || replayed;

    Ok((
        CapturedCommand {
            code: status.code().unwrap_or(1),
            stdout,
            stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
            stderr_presented,
        },
        stderr.phases,
    ))
}

fn replay_success_diagnostics(
    success: bool,
    presented: bool,
    diagnostics: &[u8],
    stderr: &mut impl Write,
) -> anyhow::Result<bool> {
    let replay = success && !presented && !diagnostics.is_empty();
    if replay {
        for line in String::from_utf8_lossy(diagnostics).lines() {
            writeln!(stderr, "  {line}").context("writing captured Nix diagnostics")?;
        }
        stderr
            .flush()
            .context("flushing captured Nix diagnostics")?;
    }
    Ok(replay)
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

    let (tx, rx) = mpsc::channel::<StreamedLine>();

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
    for event in rx {
        let trimmed = visible_stream_line(&event.line);
        let trimmed = trimmed.as_ref();
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
    tx: mpsc::Sender<StreamedLine>,
) -> thread::JoinHandle<anyhow::Result<()>> {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let line = line.with_context(|| format!("reading {stream_name} stream"))?;
            if tx.send(StreamedLine { line }).is_err() {
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

fn tee_nix_stderr_stream(mut stream: impl Read + Send + 'static) -> anyhow::Result<StderrCapture> {
    let mut diagnostics = Vec::new();
    let mut buf = [0u8; 8192];
    let mut pending = Vec::new();
    let mut profiler = ActivationPhaseProfiler::new();

    loop {
        let count = stream.read(&mut buf).context("reading stderr stream")?;
        if count == 0 {
            break;
        }
        feed_nix_output(&buf[..count], &mut pending, |record| {
            handle_nix_record(decode_nix_record(record), &mut diagnostics, &mut profiler);
            Ok(())
        })?;
    }

    if !pending.is_empty() {
        handle_nix_record(decode_nix_record(&pending), &mut diagnostics, &mut profiler);
    }
    Ok(StderrCapture {
        bytes: diagnostics,
        phases: profiler.finish(),
        presented: false,
    })
}

fn handle_nix_record(
    record: NixRecord,
    diagnostics: &mut Vec<u8>,
    profiler: &mut ActivationPhaseProfiler,
) {
    match record {
        NixRecord::Activity(activity) => profiler.observe_nix_activity(activity),
        NixRecord::Diagnostic(diagnostic) => {
            append_tail(
                diagnostics,
                diagnostic.message.as_bytes(),
                STDERR_TEE_CAPTURE_LIMIT,
            );
            append_tail(diagnostics, b"\n", STDERR_TEE_CAPTURE_LIMIT);
            for line in diagnostic.message.lines() {
                profiler.observe_stderr_line(line);
            }
        }
        NixRecord::Ignored => {}
    }
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

fn join_stderr_collector(
    handle: thread::JoinHandle<anyhow::Result<StderrCapture>>,
) -> anyhow::Result<StderrCapture> {
    handle
        .join()
        .map_err(|_| anyhow!("stderr reader thread panicked"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io;

    use crate::output::style::OutputStyle;

    struct FailingReader;

    #[test]
    fn failure_detail_skips_stderr_that_was_already_presented() {
        let output = CapturedCommand {
            code: 1,
            stdout: "stdout detail\n".to_string(),
            stderr: "stderr detail\n".to_string(),
            stderr_presented: true,
        };

        assert_eq!(first_unpresented_output(&output), "stdout detail");
    }

    #[test]
    fn failure_detail_prefers_unpresented_stderr() {
        let output = CapturedCommand::captured(
            1,
            "stdout detail\n".to_string(),
            "stderr detail\n".to_string(),
        );

        assert_eq!(first_unpresented_output(&output), "stderr detail");
    }

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

        let (code, output) =
            run_indented_command_collecting_with_env("sh", &args, None, None, &printer, "  ")
                .expect("shell command should run");

        assert_eq!(code, 0);
        assert_eq!(output, "one\n\nthree");
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
    fn native_nix_output_captures_only_stdout() {
        let args = [
            "-c",
            "printf 'json\\n'; printf 'progress\\rprogress done\\n' >&2",
        ];

        let output = run_nix_command_with_stdout("sh", &args, None, None, NixOutputMode::NativeBar)
            .expect("shell command should run");

        assert_eq!(output.code, 0);
        assert_eq!(output.stdout, "json\n");
        assert!(output.stderr.is_empty());
        assert!(output.stderr_was_presented());
    }

    #[test]
    fn structured_nix_output_keeps_diagnostic_tail() {
        let args = [
            "-c",
            "printf 'json\\n'; printf '%s\\n' '@nix {\"action\":\"start\",\"id\":1,\"level\":0,\"parent\":0,\"text\":\"\",\"type\":104}' '@nix {\"action\":\"result\",\"fields\":[1,2,1,0],\"id\":1,\"type\":105}' '@nix {\"action\":\"msg\",\"level\":0,\"msg\":\"error: Cannot build\"}' >&2; exit 1",
        ];

        let output =
            run_nix_command_with_stdout("sh", &args, None, None, NixOutputMode::Structured)
                .expect("shell command should run");

        assert_eq!(output.code, 1);
        assert_eq!(output.stdout, "json\n");
        assert_eq!(output.stderr, "error: Cannot build\n");
    }

    #[test]
    fn suppressed_progress_replays_success_diagnostics_after_completion() {
        let mut diagnostics = Vec::new();
        let mut profiler = ActivationPhaseProfiler::new();
        let mut stderr = Vec::new();

        let activity = decode_nix_record(
            br#"@nix {"action":"start","id":1,"level":0,"parent":0,"text":"building","type":104}"#,
        );
        handle_nix_record(activity, &mut diagnostics, &mut profiler);
        let warning =
            decode_nix_record(br#"@nix {"action":"msg","level":1,"msg":"warning: check this"}"#);
        handle_nix_record(warning, &mut diagnostics, &mut profiler);

        assert!(stderr.is_empty());
        replay_success_diagnostics(true, false, &diagnostics, &mut stderr)
            .expect("replay diagnostics");
        assert_eq!(stderr, b"  warning: check this\n");
    }

    #[test]
    fn terminal_stdio_detection_is_available_for_native_runner_gate() {
        let _ = terminal_stdio_available();
    }
}
