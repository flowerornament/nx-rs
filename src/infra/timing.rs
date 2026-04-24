use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::app::dirs_home;
use crate::infra::shell::run_captured_command;

const TIMING_PATH_ENV: &str = "NX_PROFILE_PATH";
const DEFAULT_TIMING_RELATIVE_PATH: &str = ".local/state/nx/timings.jsonl";

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
    pub fn new(command: &str, repo_root: &Path) -> Self {
        Self {
            command: command.to_string(),
            repo_head: git_head(repo_root),
            flake_lock_hash: flake_lock_hash(repo_root),
            started_at_ms: now_ms(),
            started: std::time::Instant::now(),
            phases: Vec::new(),
        }
    }

    pub fn record_phase<T, F>(&mut self, name: &str, run: F) -> T
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    serde_json::to_writer(&mut file, record).context("serializing timing record")?;
    writeln!(file).context("writing timing record newline")?;
    Ok(path)
}

pub fn read_recent_timings(limit: usize) -> anyhow::Result<Vec<TimingRecord>> {
    let path = timings_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = OpenOptions::new()
        .read(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line.context("reading timing record")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<TimingRecord>(&line) {
            records.push(record);
        }
    }
    let keep_from = records.len().saturating_sub(limit);
    Ok(records.split_off(keep_from))
}

#[must_use]
pub fn timings_path() -> PathBuf {
    std::env::var_os(TIMING_PATH_ENV).map_or_else(
        || dirs_home().join(DEFAULT_TIMING_RELATIVE_PATH),
        PathBuf::from,
    )
}

#[must_use]
pub fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
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

fn git_head(repo_root: &Path) -> Option<String> {
    let repo = repo_root.to_str()?;
    let output = run_captured_command("git", &["-C", repo, "rev-parse", "HEAD"], None).ok()?;
    (output.code == 0).then(|| output.stdout.trim().to_string())
}

fn flake_lock_hash(repo_root: &Path) -> Option<String> {
    let bytes = fs::read(repo_root.join("flake.lock")).ok()?;
    Some(format!("{:016x}", fnv1a64(&bytes)))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    bytes.iter().fold(FNV_OFFSET, |hash, byte| {
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
        let mut session = TimingSession::new("rebuild", tmp.path());
        let value = session.record_phase("flake-check", || (42, "ok".to_string()));
        let record = session.finish(0);

        assert_eq!(value, 42);
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
}
