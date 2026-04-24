use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::app::dirs_home;
use crate::infra::hash::short_hash;
use crate::infra::shell::run_captured_command;

const TIMING_PATH_ENV: &str = "NX_PROFILE_PATH";
const DEFAULT_TIMING_RELATIVE_PATH: &str = ".local/state/nx/timings.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingCommand {
    Rebuild,
    Upgrade,
    Install,
}

impl TimingCommand {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Rebuild => "rebuild",
            Self::Upgrade => "upgrade",
            Self::Install => "install",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimingRecord {
    pub command: String,
    pub status: String,
    pub exit_code: i32,
    pub repo_head: Option<String>,
    pub flake_lock_hash: Option<String>,
    pub started_at_ms: u128,
    pub total_ms: u128,
    pub phases: Vec<TimingPhase>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimingPhase {
    pub name: String,
    pub duration_ms: u128,
    pub status: String,
}

#[derive(Debug)]
pub struct TimingSession {
    command: String,
    repo_head: Option<String>,
    flake_lock_hash: Option<String>,
    started_at_ms: u128,
    started: std::time::Instant,
    phases: Vec<TimingPhase>,
}

impl TimingSession {
    #[must_use]
    pub fn new(command: TimingCommand, repo_root: &Path) -> Self {
        Self {
            command: command.as_str().to_string(),
            repo_head: git_head(repo_root),
            flake_lock_hash: flake_lock_hash(repo_root),
            started_at_ms: now_ms(),
            started: std::time::Instant::now(),
            phases: Vec::new(),
        }
    }

    pub fn record_result_phase<F>(&mut self, name: &str, run: F) -> Option<i32>
    where
        F: FnOnce() -> Result<(), i32>,
    {
        self.record_phase(name, || {
            let result = run();
            let code = result.err();
            (code, phase_status(code))
        })
    }

    pub fn record_exit_phase<F>(&mut self, name: &str, run: F) -> i32
    where
        F: FnOnce() -> i32,
    {
        self.record_phase(name, || {
            let code = run();
            (code, exit_status(code))
        })
    }

    fn record_phase<T, F>(&mut self, name: &str, run: F) -> T
    where
        F: FnOnce() -> (T, String),
    {
        let started = std::time::Instant::now();
        let (result, status) = run();
        self.phases.push(TimingPhase {
            name: name.to_string(),
            duration_ms: duration_ms(started.elapsed()),
            status,
        });
        result
    }

    #[must_use]
    pub fn finish(self, exit_code: i32) -> TimingRecord {
        TimingRecord {
            command: self.command,
            status: if exit_code == 0 { "ok" } else { "failed" }.to_string(),
            exit_code,
            repo_head: self.repo_head,
            flake_lock_hash: self.flake_lock_hash,
            started_at_ms: self.started_at_ms,
            total_ms: duration_ms(self.started.elapsed()),
            phases: self.phases,
        }
    }
}

pub fn append_timing(record: &TimingRecord) -> anyhow::Result<PathBuf> {
    let path = timings_path();
    append_timing_to_path(record, &path)?;
    Ok(path)
}

fn append_timing_to_path(record: &TimingRecord, path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut line = serde_json::to_vec(record).context("serializing timing record")?;
    line.push(b'\n');
    file.write_all(&line).context("writing timing record")?;
    Ok(())
}

pub fn read_recent_timings(limit: usize) -> anyhow::Result<Vec<TimingRecord>> {
    let path = timings_path();
    read_recent_timings_from_path(&path, limit)
}

fn read_recent_timings_from_path(path: &Path, limit: usize) -> anyhow::Result<Vec<TimingRecord>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("opening {}", path.display())),
    };
    let reader = BufReader::new(file);
    let mut records = VecDeque::with_capacity(limit);
    for (index, line) in reader.lines().enumerate() {
        let line = line.context("reading timing record")?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str::<TimingRecord>(&line)
            .with_context(|| format!("parsing timing record line {}", index + 1))?;
        if records.len() == limit {
            records.pop_front();
        }
        records.push_back(record);
    }
    Ok(records.into_iter().collect())
}

#[must_use]
pub fn timings_path() -> PathBuf {
    std::env::var_os(TIMING_PATH_ENV).map_or_else(
        || dirs_home().join(DEFAULT_TIMING_RELATIVE_PATH),
        PathBuf::from,
    )
}

#[must_use]
pub fn timing_summary_line(record: &TimingRecord) -> String {
    format!(
        "{} {}ms ({})",
        record.command, record.total_ms, record.status
    )
}

#[must_use]
pub fn timing_detail_lines(record: &TimingRecord) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(head) = &record.repo_head {
        lines.push(format!("git: {}", short_hash(head)));
    }
    if let Some(hash) = &record.flake_lock_hash {
        lines.push(format!("flake.lock: {hash}"));
    }
    lines.extend(
        record
            .phases
            .iter()
            .map(|phase| format!("{}: {}ms ({})", phase.name, phase.duration_ms, phase.status)),
    );
    lines
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn duration_ms(duration: Duration) -> u128 {
    duration.as_millis()
}

fn exit_status(code: i32) -> String {
    if code == 0 {
        "ok".to_string()
    } else {
        "failed".to_string()
    }
}

fn phase_status(code: Option<i32>) -> String {
    code.map_or_else(|| "ok".to_string(), |code| format!("failed:{code}"))
}

fn git_head(repo_root: &Path) -> Option<String> {
    let output = run_captured_command("git", &["rev-parse", "HEAD"], Some(repo_root)).ok()?;
    (output.code == 0).then(|| output.stdout.trim().to_string())
}

fn flake_lock_hash(repo_root: &Path) -> Option<String> {
    let mut file = File::open(repo_root.join("flake.lock")).ok()?;
    let mut buffer = [0; 8192];
    let mut hash = FNV_OFFSET;
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hash = fnv1a64_update(hash, &buffer[..count]);
    }
    Some(format!("{hash:016x}"))
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a64_update(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn timing_session_records_phase_and_finish() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let mut session = TimingSession::new(TimingCommand::Rebuild, tmp.path());
        let value = session.record_exit_phase("flake-check", || 0);
        let record = session.finish(0);

        assert_eq!(value, 0);
        assert_eq!(record.command, "rebuild");
        assert_eq!(record.status, "ok");
        assert_eq!(record.phases.len(), 1);
        assert_eq!(record.phases[0].name, "flake-check");
        assert_eq!(record.phases[0].status, "ok");
    }

    #[test]
    fn flake_lock_hash_changes_with_content() {
        let tmp = TempDir::new().expect("temp dir should be created");
        fs::write(tmp.path().join("flake.lock"), "one").expect("write flake lock");
        let first = flake_lock_hash(tmp.path()).expect("hash");
        fs::write(tmp.path().join("flake.lock"), "two").expect("write flake lock");
        let second = flake_lock_hash(tmp.path()).expect("hash");

        assert_ne!(first, second);
    }

    #[test]
    fn timing_jsonl_roundtrip_keeps_recent_records() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let path = tmp.path().join("timings.jsonl");

        for command in [
            TimingCommand::Install,
            TimingCommand::Upgrade,
            TimingCommand::Rebuild,
        ] {
            let record = TimingSession::new(command, tmp.path()).finish(0);
            append_timing_to_path(&record, &path).expect("append timing");
        }

        let records = read_recent_timings_from_path(&path, 2).expect("read timings");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].command, "upgrade");
        assert_eq!(records[1].command, "rebuild");
    }
}
