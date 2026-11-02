//! Owned bounded commands: one deadline covers execution and inherited pipes.
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{Pid, Signal, kill_process_group};

use super::shell::CapturedCommand;

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

struct OwnedChild {
    child: Child,
    armed: bool,
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // The leader remains unreaped until both pipes close, reserving its PID
        // while we still own the process group. Never signal after reaping it.
        if let Some(pid) = i32::try_from(self.child.id()).ok().and_then(Pid::from_raw) {
            let _ = kill_process_group(pid, Signal::KILL);
        }
        let _ = self.child.wait();
    }
}

fn nonblocking(pipe: &impl AsFd) -> Result<()> {
    fcntl_setfl(pipe, fcntl_getfl(pipe)? | OFlags::NONBLOCK)?;
    Ok(())
}

fn drain(
    pipe: &mut impl Read,
    bytes: &mut Vec<u8>,
    mut observe: impl FnMut(&[u8]),
) -> io::Result<bool> {
    let mut buffer = [0; 16 * 1024];
    // Bound work per pass so a continuous writer cannot defeat the deadline.
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if bytes.len() + count > OUTPUT_LIMIT {
                    return Err(io::Error::other("optional command exceeded output limit"));
                }
                observe(&buffer[..count]);
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub(crate) fn run(program: &str, args: &[&str], deadline: Instant) -> Result<CapturedCommand> {
    run_cancellable(program, args, deadline, None)
}

pub(crate) fn run_cancellable(
    program: &str,
    args: &[&str],
    deadline: Instant,
    cancel: Option<&AtomicBool>,
) -> Result<CapturedCommand> {
    let mut command = Command::new(program);
    command.args(args).env("HOMEBREW_NO_AUTO_UPDATE", "1");
    run_prepared(command, program, args, deadline, cancel, |_| {})
}

pub(crate) fn run_prepared(
    mut command: Command,
    program: &str,
    trace_args: &[&str],
    deadline: Instant,
    cancel: Option<&AtomicBool>,
    mut observe_stdout: impl FnMut(&[u8]),
) -> Result<CapturedCommand> {
    if Instant::now() >= deadline || cancelled(cancel) {
        return Ok(CapturedCommand::captured(
            124,
            String::new(),
            "command deadline reached".into(),
        ));
    }
    let mut trace = super::run_trace::command(program, trace_args);
    let mut child = OwnedChild {
        child: command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not start {program}"))?,
        armed: true,
    };
    let mut stdout = child.child.stdout.take().context("missing stdout pipe")?;
    let mut stderr = child.child.stderr.take().context("missing stderr pipe")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_done, mut err_done) = (false, false);
    let code = loop {
        if Instant::now() >= deadline || cancelled(cancel) {
            err.extend_from_slice(b"\ncommand deadline reached");
            break 124;
        }
        let drained = (|| -> io::Result<()> {
            if !out_done {
                out_done = drain(&mut stdout, &mut out, &mut observe_stdout)?;
            }
            if !err_done {
                err_done = drain(&mut stderr, &mut err, |_| {})?;
            }
            Ok(())
        })();
        if let Err(error) = drained {
            err.extend_from_slice(format!("\n{error}").as_bytes());
            break 125;
        }
        if out_done
            && err_done
            && let Some(status) = child.child.try_wait()?
        {
            // try_wait reaped the leader. Disarm without signaling its old PID.
            child.armed = false;
            break status.code().unwrap_or(1);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(child);
    if let Some(trace) = trace.as_mut() {
        trace.finish(code);
    }
    Ok(CapturedCommand::captured(
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    ))
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Output/status controls need scheduling room on loaded build hosts.
    const COMPLETION_BUDGET: Duration = Duration::from_secs(60);

    #[test]
    fn cancellation_stops_the_owned_process_without_waiting_for_its_deadline() {
        let cancel = AtomicBool::new(false);
        let mut command = Command::new("sh");
        command.args(["-c", "printf ready; sleep 60 & wait"]);
        let output = run_prepared(
            command,
            "sh",
            &[],
            Instant::now() + Duration::from_secs(60),
            Some(&cancel),
            |_| cancel.store(true, Ordering::Relaxed),
        )
        .unwrap();
        assert_eq!(output.code, 124);
        assert_eq!(output.stdout, "ready");
        assert_eq!(
            run_cancellable(
                "nonexistent-cancelled-provider",
                &[],
                Instant::now() + COMPLETION_BUDGET,
                Some(&cancel)
            )
            .unwrap()
            .code,
            124
        );
    }

    #[test]
    fn preserves_output_and_failure_status() {
        let output = run(
            "sh",
            &["-c", "printf hello; printf problem >&2; exit 7"],
            Instant::now() + COMPLETION_BUDGET,
        )
        .unwrap();
        assert_eq!(
            (output.code, output.stdout.as_str(), output.stderr.as_str()),
            (7, "hello", "problem")
        );
    }

    #[test]
    fn deadline_includes_descendant_pipes_and_running_leader() {
        for script in ["sleep 60 & exit 0", "sleep 60 & wait"] {
            let start = Instant::now();
            let output = run("sh", &["-c", script], start + Duration::from_millis(100)).unwrap();
            assert_eq!(output.code, 124);
            assert!(start.elapsed() < Duration::from_secs(3));
        }
    }

    #[test]
    fn fallback_does_not_receive_a_fresh_budget() {
        let deadline = Instant::now() + Duration::from_millis(50);
        assert_eq!(run("sh", &["-c", "sleep 60"], deadline).unwrap().code, 124);
        assert_eq!(
            run("nonexistent-optional-provider", &[], deadline)
                .unwrap()
                .code,
            124
        );
    }

    #[test]
    fn unlimited_output_is_rejected() {
        let output = run(
            "sh",
            &["-c", "yes flooding"],
            Instant::now() + COMPLETION_BUDGET,
        )
        .unwrap();
        assert_eq!(output.code, 125);
        assert!(output.stdout.len() <= OUTPUT_LIMIT);
    }
}
