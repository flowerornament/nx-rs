#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/command_io.rs"]
mod support_command_io;
#[path = "support/stubs.rs"]
mod support_stubs;
#[path = "support/tree.rs"]
mod support_tree;

use serde_json::Value;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn profile_command(root: &Path, traces: &Path) -> Result<Command, Box<dyn Error>> {
    let mut command = Command::new(support_bin::resolve_nx_bin(root)?);
    command
        .args(["profile", "--json"])
        .env("NX_PROFILE_PATH", traces.join("missing"))
        .env("NX_TRACE_DIR", traces)
        .env("NX_TRACE_KEEP", "1");
    Ok(command)
}

#[test]
fn routine_traces_are_unique_private_and_preserve_explicit_files() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let traces = TempDir::new()?;
    fs::write(traces.path().join("keep-me"), "unrelated")?;
    for _ in 0..3 {
        let output = support_command_io::run_command_with_optional_stdin(
            &mut profile_command(&root, traces.path())?,
            None,
        )?;
        assert!(output.status.success());
    }
    let managed = fs::read_dir(traces.path())?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("nx-"))
        .collect::<Vec<_>>();
    assert_eq!(managed.len(), 1);
    assert_eq!(managed[0].metadata()?.permissions().mode() & 0o777, 0o600);
    let rows = fs::read_to_string(managed[0].path())?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()?;
    assert_eq!(rows[0]["data"]["managed"], true);
    assert_eq!(rows.last().unwrap()["data"]["complete"], true);
    let explicit = traces.path().join("explicit.jsonl");
    let mut command = profile_command(&root, traces.path())?;
    command.env("NX_TRACE_PATH", &explicit);
    assert!(
        support_command_io::run_command_with_optional_stdin(&mut command, None)?
            .status
            .success()
    );
    assert!(explicit.exists());
    assert!(traces.path().join("keep-me").exists());
    assert_eq!(fs::read_dir(traces.path())?.count(), 3);
    Ok(())
}

#[test]
fn size_limited_trace_warns_without_changing_upgrade_result() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = TempDir::new()?;
    let home = TempDir::new()?;
    let traces = TempDir::new()?;
    support_tree::copy_tree(&root.join("tests/fixtures/system/repo_base"), repo.path())?;
    support_command_io::ensure_test_layout(repo.path())?;
    let stubs = repo.path().join(support_stubs::STUB_DIR_NAME);
    fs::create_dir(&stubs)?;
    support_stubs::install_stubs(&stubs)?;
    let mut command = Command::new(support_bin::resolve_nx_bin(&root)?);
    command
        .args([
            "--plain",
            "--minimal",
            "upgrade",
            "-y",
            "--no-ai",
            "--skip-brew",
        ])
        .current_dir(repo.path())
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path().join(".cache"))
        .env("NX_REPO_ROOT", repo.path())
        .env(
            "NX_SYSTEM_PROFILE_PATH",
            home.path().join("absent-system-profile"),
        )
        .env("NX_TRACE_DIR", traces.path())
        .env("NX_TRACE_MAX_BYTES", "4096")
        .env(
            "NX_SYSTEM_IT_LOG",
            repo.path().join(support_stubs::LOG_FILE_NAME),
        )
        .env("NX_SYSTEM_IT_MODE", "success")
        .env("NX_SYSTEM_IT_DARWIN_REBUILD", stubs.join("darwin-rebuild"))
        .env("PATH", support_stubs::prepend_path(&stubs));
    let output = support_command_io::run_command_with_optional_stdin(&mut command, None)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let trace = fs::read_dir(traces.path())?.next().unwrap()?.path();
    assert!(fs::metadata(&trace)?.len() <= 4096);
    let contents = fs::read_to_string(&trace)?;
    let last: Value = serde_json::from_str(contents.lines().last().unwrap())?;
    assert_eq!(last["event"], "run-end");
    assert_eq!(last["code"], 0);
    assert_eq!(last["data"]["complete"], false);
    assert!(String::from_utf8_lossy(&output.stderr).contains("incomplete"));
    let timing = fs::read_to_string(home.path().join(".local/state/nx/timings.jsonl"))?;
    let record: Value = serde_json::from_str(timing.lines().last().unwrap())?;
    assert_eq!(record["trace_path"], trace.to_string_lossy().as_ref());
    assert!(record["workload_start"]["logical_cpus"].as_u64().is_some());
    Ok(())
}
