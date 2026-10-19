//! Owned optional commands: one deadline covers execution and inherited pipes.
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
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

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>) -> io::Result<bool> {
    let mut buffer = [0; 16 * 1024];
    // Bound work per pass so a continuous writer cannot defeat the deadline.
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if bytes.len() + count > OUTPUT_LIMIT {
                    return Err(io::Error::other("optional command exceeded output limit"));
                }
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
    if Instant::now() >= deadline {
        return Ok(CapturedCommand::captured(
            124,
            String::new(),
            "optional command deadline reached".into(),
        ));
    }
    let mut trace = super::run_trace::command(program, args);
    let mut child = OwnedChild {
        child: Command::new(program)
            .args(args)
            .process_group(0)
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
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
        if Instant::now() >= deadline {
            err.extend_from_slice(b"\noptional command deadline reached");
            break 124;
        }
        let drained = (|| -> io::Result<()> {
            if !out_done {
                out_done = drain(&mut stdout, &mut out)?;
            }
            if !err_done {
                err_done = drain(&mut stderr, &mut err)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    // Output/status controls need scheduling room on loaded build hosts.
    const COMPLETION_BUDGET: Duration = Duration::from_secs(60);

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
