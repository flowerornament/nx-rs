#[path = "support/bin.rs"]
mod support_bin;
#[allow(dead_code)]
#[path = "support/command_io.rs"]
mod support_command_io;
#[allow(dead_code)]
#[path = "support/invocations.rs"]
mod support_invocations;
#[path = "support/stubs.rs"]
mod support_stubs;

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

use support_bin::resolve_nx_bin;
use support_command_io::run_command_with_optional_stdin;
use support_invocations::read_invocations;
use support_stubs::{LOG_FILE_NAME, STUB_DIR_NAME, install_stubs, prepend_path};

#[test]
fn generations_plan_surfaces_stubbed_home_manager_prunes() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let cwd = TempDir::new()?;
    let home_dir = TempDir::new()?;
    let stub_dir = cwd.path().join(STUB_DIR_NAME);
    std::fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;
    let log_path = cwd.path().join(LOG_FILE_NAME);

    let mut command = Command::new(nx_bin);
    command
        .args(["--plain", "--minimal"])
        .args(["generations", "plan", "--kind", "home-manager", "--keep", "1", "--no-gc"])
        .current_dir(cwd.path())
        .env("PATH", prepend_path(&stub_dir))
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("NX_SYSTEM_IT_HOME_MANAGER_GENERATIONS", "2026-04-02 13:00 : id 6 -> /nix/store/old-home-manager-generation\n2026-04-02 14:00 : id 7 -> /nix/store/current-home-manager-generation (current)")
        .env("NX_SYSTEM_IT_DF_OUTPUT", "Filesystem      Size    Used   Avail Capacity Mounted on\n/dev/disk-test  100Gi   40Gi   60Gi   40% /nix")
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1");

    let output = run_command_with_optional_stdin(&mut command, None)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(0), "stderr:\n{stderr}");
    assert!(stdout.contains("Generations Plan"));
    assert!(stdout.contains("home-manager prune IDs: 6"));
    assert!(stdout.contains("home-manager remove-generations 6"));

    let invocations = read_invocations(&log_path)?;
    assert!(
        invocations
            .iter()
            .any(|call| call.program == "home-manager"
                && call.args == vec!["generations".to_string()]),
        "expected home-manager generations call, got {invocations:?}"
    );
    assert!(
        invocations
            .iter()
            .any(|call| call.program == "df"
                && call.args == vec!["-h".to_string(), "/nix".to_string()]),
        "expected df call, got {invocations:?}"
    );

    Ok(())
}

#[test]
fn generations_prune_runs_stubbed_home_manager_removal() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let cwd = TempDir::new()?;
    let home_dir = TempDir::new()?;
    let stub_dir = cwd.path().join(STUB_DIR_NAME);
    std::fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;
    let log_path = cwd.path().join(LOG_FILE_NAME);

    let mut command = Command::new(nx_bin);
    command
        .args(["--plain", "--minimal"])
        .args(["generations", "prune", "--kind", "home-manager", "--keep", "1", "--no-gc", "-y"])
        .current_dir(cwd.path())
        .env("PATH", prepend_path(&stub_dir))
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("NX_SYSTEM_IT_HOME_MANAGER_GENERATIONS", "2026-04-02 13:00 : id 6 -> /nix/store/old-home-manager-generation\n2026-04-02 14:00 : id 7 -> /nix/store/current-home-manager-generation (current)")
        .env("NX_SYSTEM_IT_DF_OUTPUT", "Filesystem      Size    Used   Avail Capacity Mounted on\n/dev/disk-test  100Gi   40Gi   60Gi   40% /nix")
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1");

    let output = run_command_with_optional_stdin(&mut command, None)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(0), "stderr:\n{stderr}");
    assert!(stdout.contains("Generations pruned"));
    assert!(stdout.contains("home-manager prune IDs: 6"));

    let invocations = read_invocations(&log_path)?;
    assert!(
        invocations.iter().any(|call| {
            call.program == "home-manager"
                && call.args == vec!["remove-generations".to_string(), "6".to_string()]
        }),
        "expected home-manager removal call, got {invocations:?}"
    );

    Ok(())
}
