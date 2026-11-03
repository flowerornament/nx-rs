//! A private, bounded JSON side channel leaves Nix's terminal renderer in charge.
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;
use tempfile::TempDir;

const MAX_CLIENTS: usize = 16;
const RECORD_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct CaptureSummary {
    connections: usize,
    activities: usize,
    complete: bool,
}

pub(crate) struct Capture {
    _directory: TempDir,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<CaptureSummary>>,
}

impl Capture {
    pub(crate) fn configure(
        command: &mut Command,
        program: &str,
        args: &[&str],
    ) -> io::Result<Option<Self>> {
        let Some(start) =
            nix_argument_start(program, args).filter(|_| super::ACTIVE.load(Ordering::Relaxed))
        else {
            command.args(args);
            return Ok(None);
        };
        if args
            .iter()
            .any(|arg| *arg == "json-log-path" || arg.starts_with("--json-log-path"))
        {
            command.args(args);
            return Ok(None);
        }
        let directory = tempfile::Builder::new()
            .prefix("nx-nix-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let path = directory.path().join("log");
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let reader = std::thread::Builder::new()
            .name("nx-nix-log".into())
            .spawn(super::with_parent(move || collect(&listener, &stopping)))?;
        let boundary = args
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, arg)| **arg == "--")
            .map_or(args.len(), |(index, _)| index);
        command
            .args(&args[..boundary])
            .args(["--option", "json-log-path"])
            .arg(path)
            .args(&args[boundary..]);
        Ok(Some(Self {
            _directory: directory,
            stop,
            reader: Some(reader),
        }))
    }

    pub(crate) fn finish(mut self) -> CaptureSummary {
        self.stop()
    }

    fn stop(&mut self) -> CaptureSummary {
        self.stop.store(true, Ordering::Release);
        self.reader
            .take()
            .map_or_else(CaptureSummary::default, |reader| {
                reader.thread().unpark();
                reader.join().unwrap_or_default()
            })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if self.reader.is_some() {
            self.stop();
        }
    }
}

fn nix_argument_start(program: &str, args: &[&str]) -> Option<usize> {
    let name = |value: &str| {
        matches!(
            Path::new(value).file_name().and_then(|name| name.to_str()),
            Some("nix" | "nix-env" | "nix-store" | "darwin-rebuild" | "nixos-rebuild")
        )
    };
    if name(program) {
        return Some(0);
    }
    if Path::new(program)
        .file_name()
        .is_some_and(|name| name == "sudo")
    {
        // Rebuild's root profile update uses this explicit environment wrapper.
        if matches!(args, ["-H", "env", "HOME=/var/root", "NIX_REMOTE=daemon", target, ..] if name(target))
        {
            return Some(5);
        }
        return args
            .iter()
            .position(|arg| !arg.starts_with('-'))
            .filter(|index| name(args[*index]))
            .map(|index| index + 1);
    }
    None
}

#[derive(Default)]
struct Record {
    pending: Vec<u8>,
    oversized: bool,
}

impl Record {
    fn push(&mut self, bytes: &[u8], summary: &mut CaptureSummary) {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            if self.pending.len() + part.len() > RECORD_LIMIT {
                self.pending.clear();
                self.oversized = true;
                summary.complete = false;
            }
            if !self.oversized {
                self.pending.extend_from_slice(part);
            }
            if part.last() == Some(&b'\n') {
                self.finish(summary);
            }
        }
    }

    fn finish(&mut self, summary: &mut CaptureSummary) {
        if !self.oversized && super::nix_json_activity(&self.pending) {
            summary.activities += 1;
        }
        self.pending.clear();
        self.oversized = false;
    }
}

struct Client {
    stream: UnixStream,
    record: Record,
}

impl Client {
    fn drain(&mut self, summary: &mut CaptureSummary) -> bool {
        let mut bytes = [0; 16 * 1024];
        // Bounded work per pass also bounds joining after the command exits.
        for _ in 0..64 {
            match self.stream.read(&mut bytes) {
                Ok(0) => {
                    self.record.finish(summary);
                    return false;
                }
                Ok(count) => self.record.push(&bytes[..count], summary),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    summary.complete = false;
                    return false;
                }
            }
        }
        true
    }
}

fn collect(listener: &UnixListener, stop: &AtomicBool) -> CaptureSummary {
    let mut summary = CaptureSummary {
        complete: true,
        ..CaptureSummary::default()
    };
    let mut clients = Vec::new();
    loop {
        for _ in 0..MAX_CLIENTS {
            match listener.accept() {
                Ok((stream, _)) if clients.len() < MAX_CLIENTS => {
                    summary.connections += 1;
                    if stream.set_nonblocking(true).is_ok() {
                        clients.push(Client {
                            stream,
                            record: Record::default(),
                        });
                    } else {
                        summary.complete = false;
                    }
                }
                Ok(_) => summary.complete = false,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    summary.complete = false;
                    break;
                }
            }
        }
        clients.retain_mut(|client| client.drain(&mut summary));
        if stop.load(Ordering::Acquire) {
            summary.complete &= clients.is_empty() && summary.connections > 0;
            return summary;
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests;
