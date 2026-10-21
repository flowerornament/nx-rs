#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/command_io.rs"]
mod support_command_io;

use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};
use rustix::process::{Pid, Signal, kill_process};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;

struct Run(Child);

impl Run {
    fn finish(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "nx did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
        let stdout = self.0.stdout.take().unwrap();
        let stderr = self.0.stderr.take().unwrap();
        let mut output = Output {
            status: self.0.wait().unwrap(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        std::io::BufReader::new(stdout)
            .read_to_end(&mut output.stdout)
            .unwrap();
        std::io::BufReader::new(stderr)
            .read_to_end(&mut output.stderr)
            .unwrap();
        output
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none()
            && let Some(pid) = i32::try_from(self.0.id()).ok().and_then(Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, Signal::KILL);
        }
        let _ = self.0.wait();
    }
}

struct Fixture {
    dir: TempDir,
    binary: PathBuf,
    lock: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(self.dir.path()) {
            for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                let _ = fs::write(entry.path().join("proceed"), "");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let binary = support_bin::resolve_nx_bin(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let lock = dir.path().join("operation.lock");
        Self { dir, binary, lock }
    }

    fn repo(&self, name: &str, kind: &str) -> PathBuf {
        let repo = self.dir.path().join(name);
        fs::create_dir_all(repo.join(".nx")).unwrap();
        let configuration = if kind == "nixos" {
            "nixosConfigurations"
        } else {
            "darwinConfigurations"
        };
        fs::write(
            repo.join("flake.nix"),
            format!("{{ outputs = {{ ... }}: {{ {configuration} = {{}}; }}; }}\n"),
        )
        .unwrap();
        fs::write(repo.join(".nx/manifest.toml"), format!(
            "schema_version = 1\n[platform]\nkind = \"{kind}\"\nrebuild_command = \"{}/switch\"\nsudo = false\nflake_root = \".\"\nsplit_rebuild = false\n",
            repo.display())).unwrap();
        executable(&repo.join("nix"), "#!/bin/sh\nexit 0\n");
        executable(&repo.join("git"), "#!/bin/sh\nexit 0\n");
        executable(
            &repo.join("sudo"),
            "#!/bin/sh\necho 'unexpected sudo in isolated lock test' >&2\nexit 1\n",
        );
        // Close inherited descriptors like sudo, to exercise the waiting shell.
        executable(
            &repo.join("switch"),
            r"#!/usr/bin/env python3
import os, pathlib, signal, time
signal.signal(signal.SIGINT, signal.SIG_IGN)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
signal.signal(signal.SIGHUP, signal.SIG_IGN)
signal.signal(signal.SIGQUIT, signal.SIG_IGN)
os.closerange(3, 1024)
root = pathlib.Path(os.environ['NX_REPO_ROOT'])
(root / 'entered').touch()
while not (root / 'proceed').exists():
    time.sleep(0.02)
(root / 'finished').touch()
",
        );
        repo
    }

    fn command(&self, binary: &Path, repo: &Path, args: &[&str]) -> Command {
        let home = repo.join("home");
        fs::create_dir_all(&home).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "umask 077; exec \"$@\"", "nx-lock-test"])
            .arg(binary)
            .args(["--plain", "--minimal"])
            .args(args)
            .current_dir(repo)
            .env("NX_REPO_ROOT", repo)
            .env("HOME", home)
            .env("NX_OPERATION_LOCK_PATH", &self.lock)
            .env("NX_PROFILE_PATH", repo.join("timings.jsonl"))
            .env("NX_SPLIT_DARWIN", "0")
            .env_remove("NX_TRACE_DIR")
            .env_remove("NX_TRACE_PATH")
            .env_remove("NX_TRACE_KEEP")
            .env_remove("NX_TRACE_MAX_BYTES")
            .env_remove("NX_SUMMARY_TIMEOUT_SECONDS")
            .env_remove("XDG_STATE_HOME")
            .env("NX_RS_AUTO_REFRESH", "0")
            .env(
                "PATH",
                format!("{}:{}", repo.display(), std::env::var("PATH").unwrap()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        command
    }

    fn spawn(&self, repo: &Path, args: &[&str]) -> Run {
        Run(self.command(&self.binary, repo, args).spawn().unwrap())
    }
}

fn executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_until_queued(run: &mut Run) {
    let stdout = run.0.stdout.as_mut().unwrap();
    let flags = rustix::fs::fcntl_getfl(&*stdout).unwrap();
    rustix::fs::fcntl_setfl(&*stdout, flags | rustix::fs::OFlags::NONBLOCK).unwrap();
    let mut output = Vec::new();
    until(|| {
        let mut bytes = [0; 4096];
        match stdout.read(&mut bytes) {
            Ok(count) => output.extend_from_slice(&bytes[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("reading waiter: {error}"),
        }
        String::from_utf8_lossy(&output).contains("Waiting for")
    });
    rustix::fs::fcntl_setfl(&*stdout, flags).unwrap();
}

#[test]
fn rebuilds_wait_across_repos_and_binary_names_then_read_fresh_state() -> TestResult {
    let fixture = Fixture::new();
    let first = fixture.repo("first", "darwin");
    let second = fixture.repo("second", "nixos");
    let owner = fixture.spawn(&first, &["rebuild"]);
    until(|| first.join("entered").exists());
    let dev = fixture.dir.path().join("nx-dev");
    symlink(&fixture.binary, &dev)?;
    let mut waiter = Run(fixture.command(&dev, &second, &["rebuild"]).spawn()?);
    std::thread::sleep(Duration::from_millis(250));
    assert!(waiter.0.try_wait()?.is_none());
    assert!(!second.join("entered").exists());
    assert!(!second.join("timings.jsonl").exists());
    // Queries and preflight do not wait for the operation lock.
    assert_ok(&fixture.spawn(&second, &["version"]).finish());
    assert_ok(&fixture.spawn(&second, &["rebuild", "--preflight"]).finish());
    let manifest = second.join(".nx/manifest.toml");
    let current = fs::read_to_string(&manifest)?;
    fs::rename(second.join("switch"), second.join("new-switch"))?;
    fs::write(manifest, current.replace("/switch\"", "/new-switch\""))?;
    fs::write(second.join("proceed"), "")?;
    fs::write(first.join("proceed"), "")?;
    assert_ok(&owner.finish());
    let output = waiter.finish();
    assert_ok(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Waiting for nx rebuild (PID"));
    assert!(stdout.contains("Operation lock acquired after"));
    assert!(second.join("finished").exists());
    let rows = fs::read_to_string(second.join("timings.jsonl"))?;
    let record: serde_json::Value = serde_json::from_str(rows.lines().last().unwrap())?;
    assert_eq!(record["phases"][0]["name"], "operation-wait");
    assert!(record["phases"][0]["duration_ms"].as_u64().unwrap() >= 100);
    Ok(())
}

#[test]
fn killed_owner_keeps_exclusion_until_its_activation_child_finishes() -> TestResult {
    exercise_orphan(None, false)
}

#[test]
fn process_group_signals_do_not_release_a_live_activation_lease() -> TestResult {
    for signal in [Signal::INT, Signal::TERM, Signal::HUP, Signal::QUIT] {
        exercise_orphan(Some(signal), false)?;
    }
    Ok(())
}

#[test]
fn sops_children_keep_the_lease_after_nx_is_killed() -> TestResult {
    exercise_orphan(None, true)
}

fn exercise_orphan(group_signal: Option<Signal>, secret: bool) -> TestResult {
    let fixture = Fixture::new();
    let first = fixture.repo("owner", "nixos");
    let second = fixture.repo("waiter", "darwin");
    let mut command = if secret {
        fs::create_dir_all(first.join("home"))?;
        fs::create_dir_all(first.join("secrets"))?;
        fs::write(
            first.join("home/secrets.nix"),
            "{\n  secretNames = [\n  ];\n}\n",
        )?;
        fs::write(first.join("secrets/secrets.yaml"), "{}\n")?;
        let mut command = fixture.command(
            &fixture.binary,
            &first,
            &["secret", "add", "test_key", "--value", "test-value"],
        );
        command.env("NX_RS_SOPS_BIN", first.join("switch"));
        command
    } else {
        fixture.command(&fixture.binary, &first, &["rebuild"])
    };
    let mut owner = Run(command.spawn()?);
    until(|| first.join("entered").exists());
    let mut waiter = fixture.spawn(&second, &["init"]);
    std::thread::sleep(Duration::from_millis(150));
    // Cancelling a waiter cannot release the owner's lease.
    let mut cancelled = fixture.spawn(&second, &["init"]);
    wait_until_queued(&mut cancelled);
    kill_process(
        Pid::from_raw(i32::try_from(cancelled.0.id())?).unwrap(),
        Signal::INT,
    )?;
    assert!(!cancelled.finish().status.success());
    if let Some(signal) = group_signal {
        rustix::process::kill_process_group(
            Pid::from_raw(i32::try_from(owner.0.id())?).unwrap(),
            signal,
        )?;
    } else {
        owner.0.kill()?;
    }
    std::thread::sleep(Duration::from_millis(150));
    assert!(waiter.0.try_wait()?.is_none());
    assert!(!first.join("finished").exists());
    fs::write(first.join("proceed"), "")?;
    assert_ok(&waiter.finish());
    let _ = owner.0.wait()?;
    assert!(first.join("finished").exists());
    Ok(())
}

#[test]
fn lock_rejects_links_and_does_not_touch_their_target() -> TestResult {
    for hard_link in [false, true] {
        let fixture = Fixture::new();
        let repo = fixture.repo("repo", "nixos");
        let target = fixture.dir.path().join("important");
        fs::write(&target, "preserve me")?;
        if hard_link {
            fs::hard_link(&target, &fixture.lock)?;
        } else {
            symlink(&target, &fixture.lock)?;
        }
        let output = fixture.spawn(&repo, &["init"]).finish();
        assert!(!output.status.success());
        assert_eq!(fs::read_to_string(target)?, "preserve me");
        assert!(!repo.join("entered").exists());
    }
    Ok(())
}

#[test]
fn relative_lock_overrides_fail_before_any_mutation() -> TestResult {
    let fixture = Fixture::new();
    let repo = fixture.repo("repo", "nixos");
    let output = Run(fixture
        .command(&fixture.binary, &repo, &["init"])
        .env("NX_OPERATION_LOCK_PATH", "relative.lock")
        .spawn()?)
    .finish();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be an absolute path"));
    assert!(!repo.join("relative.lock").exists());
    Ok(())
}

#[test]
fn held_lock_does_not_block_dry_runs_and_stale_metadata_is_not_a_lock() -> TestResult {
    let fixture = Fixture::new();
    let repo = fixture.repo("repo", "nixos");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&fixture.lock)?;
    file.write_all(b"stale owner metadata")?;
    flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    let output = fixture
        .spawn(&repo, &["remove", "missing", "--dry-run"])
        .finish();
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Waiting for"));
    drop(file);
    assert_ok(&fixture.spawn(&repo, &["init"]).finish());
    assert!(File::open(&fixture.lock)?.metadata()?.len() == 0);
    Ok(())
}

#[test]
fn concurrent_creators_publish_shared_permissions_despite_private_umask() -> TestResult {
    let fixture = Fixture::new();
    let first = fixture.repo("first", "nixos");
    let second = fixture.repo("second", "darwin");
    let left = fixture.spawn(&first, &["init"]);
    let right = fixture.spawn(&second, &["init"]);
    assert_ok(&left.finish());
    assert_ok(&right.finish());
    assert_eq!(
        fs::metadata(&fixture.lock)?.permissions().mode() & 0o777,
        0o666
    );
    Ok(())
}
