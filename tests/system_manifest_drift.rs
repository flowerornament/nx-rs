#[path = "support/bin.rs"]
mod support_bin;

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

use support_bin::resolve_nx_bin;

const STALE_MANIFEST: &str = r#"
schema_version = 1

[platform]
kind = "darwin"
rebuild_command = "/run/current-system/sw/bin/darwin-rebuild"
sudo = true
flake_root = "."

[[slots]]
kind = "nix-packages"
file = "packages/moved-cli.nix"
attr_path = "home.packages"
tags = []
default_for = ["install"]
"#;

fn write_stale_manifest(repo_root: &Path) {
    let nx_dir = repo_root.join(".nx");
    fs::create_dir_all(&nx_dir).expect("manifest dir should be created");
    fs::write(nx_dir.join("manifest.toml"), STALE_MANIFEST.trim_start())
        .expect("manifest should be written");
}

fn write_minimal_repo(repo_root: &Path) {
    fs::write(
        repo_root.join("flake.nix"),
        "{ outputs = { self, nix-darwin, ... }: { darwinConfigurations.host = {}; }; }\n",
    )
    .expect("flake.nix should be written");
}

fn run_nx(args: &[&str]) -> Result<std::process::Output, Box<dyn Error>> {
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let tmp = TempDir::new()?;
    write_minimal_repo(tmp.path());
    write_stale_manifest(tmp.path());

    let home_dir = TempDir::new()?;
    let output = Command::new(nx_bin)
        .args(["--plain", "--minimal"])
        .args(args)
        .current_dir(tmp.path())
        .env("NX_REPO_ROOT", tmp.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    Ok(output)
}

#[test]
fn status_reports_manifest_drift() -> Result<(), Box<dyn Error>> {
    let output = run_nx(&["status"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code().unwrap_or(-1),
        0,
        "unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Manifest Health"));
    assert!(stdout.contains("Manifest drift detected (1 issue(s))"));
    assert!(stdout.contains("missing manifest file packages/moved-cli.nix"));
    assert!(stdout.contains("Write-routing commands are blocked until the manifest is refreshed"));
    assert!(stdout.contains("Run: nx init --refresh"));

    Ok(())
}

#[test]
fn install_refuses_to_run_when_manifest_is_stale() -> Result<(), Box<dyn Error>> {
    let output = run_nx(&["install", "ripgrep"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code().unwrap_or(-1),
        1,
        "unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.contains("Cannot install while manifest drift is detected"));
    assert!(stdout.contains("missing manifest file packages/moved-cli.nix"));
    assert!(stdout.contains("Run: nx init --refresh"));

    Ok(())
}
