#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/stubs.rs"]
mod support_stubs;
#[path = "support/tree.rs"]
mod support_tree;

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

use support_bin::resolve_nx_bin;
use support_stubs::{LOG_FILE_NAME, STUB_DIR_NAME, install_stubs, prepend_path};
use support_tree::copy_tree;

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

const INVALID_MANIFEST: &str = r#"
schema_version = 1

[platform
kind = "darwin"
"#;

fn write_stale_manifest(repo_root: &Path) {
    write_manifest(repo_root, STALE_MANIFEST);
}

fn write_invalid_manifest(repo_root: &Path) {
    write_manifest(repo_root, INVALID_MANIFEST);
}

fn write_manifest(repo_root: &Path, content: &str) {
    let nx_dir = repo_root.join(".nx");
    fs::create_dir_all(&nx_dir).expect("manifest dir should be created");
    fs::write(nx_dir.join("manifest.toml"), content.trim_start())
        .expect("manifest should be written");
}

fn write_minimal_repo(repo_root: &Path) {
    fs::write(
        repo_root.join("flake.nix"),
        "{ outputs = { self, nix-darwin, ... }: { darwinConfigurations.host = {}; }; }\n",
    )
    .expect("flake.nix should be written");
}

fn run_nx_minimal(args: &[&str]) -> Result<std::process::Output, Box<dyn Error>> {
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

fn write_install_cache(home_dir: &Path) {
    let cache_dir = home_dir.join(".cache").join("nx");
    fs::create_dir_all(&cache_dir).expect("cache dir should be created");
    let cache = serde_json::json!({
        "schema_version": 1,
        "entries": {
            "demo-mcp|nxs|unknown": {
                "attr": "demo-mcp",
                "version": null,
                "description": "Demo MCP package",
                "confidence": 0.9,
                "requires_flake_mod": false,
                "flake_url": null
            }
        }
    });
    fs::write(
        cache_dir.join("packages_v4.json"),
        serde_json::to_string_pretty(&cache).expect("cache json should serialize"),
    )
    .expect("cache file should be written");
}

fn run_nx_repo_base_with_stale_manifest(
    args: &[&str],
) -> Result<(TempDir, std::process::Output), Box<dyn Error>> {
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let nx_bin = resolve_nx_bin(&workspace_root)?;
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");

    let tmp = TempDir::new()?;
    copy_tree(&repo_base, tmp.path())?;
    write_stale_manifest(tmp.path());

    let home_dir = TempDir::new()?;
    let stub_dir = tmp.path().join(STUB_DIR_NAME);
    fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;
    let log_path = tmp.path().join(LOG_FILE_NAME);

    write_install_cache(home_dir.path());
    let output = Command::new(nx_bin)
        .args(["--plain", "--minimal"])
        .args(args)
        .current_dir(tmp.path())
        .env("NX_REPO_ROOT", tmp.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("PATH", prepend_path(&stub_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    Ok((tmp, output))
}

#[test]
fn status_reports_manifest_drift() -> Result<(), Box<dyn Error>> {
    let output = run_nx_minimal(&["status"])?;
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
    assert!(
        stdout
            .contains("Using live discovery fallback for routing until the manifest is refreshed")
    );
    assert!(stdout.contains("Run: nx init --refresh"));

    Ok(())
}

#[test]
fn install_uses_live_routing_when_manifest_is_stale() -> Result<(), Box<dyn Error>> {
    let (tmp, output) = run_nx_repo_base_with_stale_manifest(&["install", "--yes", "demo-mcp"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code().unwrap_or(-1),
        0,
        "unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.is_empty(), "stderr should be empty:\n{stderr}");
    assert!(stdout.contains("Installing demo-mcp"));
    assert!(stdout.contains("Added 'demo-mcp' to packages/nix/cli.nix"));

    let cli_nix = fs::read_to_string(tmp.path().join("packages/nix/cli.nix"))?;
    assert!(cli_nix.contains("demo-mcp"));

    Ok(())
}

#[test]
fn remove_uses_live_routing_when_manifest_is_stale() -> Result<(), Box<dyn Error>> {
    let (tmp, output) = run_nx_repo_base_with_stale_manifest(&["remove", "--yes", "ripgrep"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code().unwrap_or(-1),
        0,
        "unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.is_empty(), "stderr should be empty:\n{stderr}");
    assert!(stdout.contains("Removing ripgrep"));
    assert!(stdout.contains("ripgrep removed from cli.nix"));

    let cli_nix = fs::read_to_string(tmp.path().join("packages/nix/cli.nix"))?;
    assert!(!cli_nix.contains("ripgrep"));
    assert!(cli_nix.contains("fd"));

    Ok(())
}

#[test]
fn rebuild_blocks_when_manifest_is_invalid() -> Result<(), Box<dyn Error>> {
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let tmp = TempDir::new()?;
    write_minimal_repo(tmp.path());
    write_invalid_manifest(tmp.path());

    let home_dir = TempDir::new()?;
    let output = Command::new(nx_bin)
        .args(["--plain", "--minimal", "rebuild"])
        .current_dir(tmp.path())
        .env("NX_REPO_ROOT", tmp.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code().unwrap_or(-1),
        1,
        "unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.contains("Cannot rebuild while .nx/manifest.toml is unreadable"));
    assert!(
        stdout
            .contains("Run: nx init --refresh so custom platform settings can be recovered safely")
    );

    Ok(())
}

#[test]
fn upgrade_dry_run_skip_brew_works_when_manifest_is_invalid() -> Result<(), Box<dyn Error>> {
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let tmp = TempDir::new()?;
    write_minimal_repo(tmp.path());
    write_invalid_manifest(tmp.path());

    let home_dir = TempDir::new()?;
    let output = Command::new(nx_bin)
        .args([
            "--plain",
            "--minimal",
            "upgrade",
            "--dry-run",
            "--skip-brew",
        ])
        .current_dir(tmp.path())
        .env("NX_REPO_ROOT", tmp.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code().unwrap_or(-1),
        0,
        "unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.is_empty(), "stderr should be empty:\n{stderr}");
    assert!(stdout.contains("All flake inputs up to date"));
    assert!(stdout.contains("Dry run complete - no changes made"));
    assert!(!stdout.contains("Cannot upgrade while .nx/manifest.toml is unreadable"));

    Ok(())
}
