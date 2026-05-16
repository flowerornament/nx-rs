#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/tree.rs"]
mod support_tree;

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use tempfile::TempDir;

use support_bin::resolve_nx_bin;
use support_tree::copy_tree;

fn run_nx(
    nx_bin: &std::path::Path,
    repo_root: &std::path::Path,
    home_dir: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    Command::new(nx_bin)
        .args(["--plain", "--minimal"])
        .args(args)
        .current_dir(repo_root)
        .env("NX_REPO_ROOT", repo_root)
        .env("HOME", home_dir)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to execute nx binary")
}

#[test]
fn init_creates_valid_manifest() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let tmp = TempDir::new()?;
    copy_tree(&repo_base, tmp.path())?;
    let home_dir = TempDir::new()?;

    let output = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["init"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code().unwrap_or(-1),
        0,
        "init failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Manifest written"),
        "stdout missing 'Manifest written'\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let manifest_path = tmp.path().join(".nx/manifest.toml");
    assert!(manifest_path.is_file(), "manifest.toml was not created");

    let raw = fs::read_to_string(&manifest_path)?;
    let doc: toml_edit::DocumentMut = raw.parse()?;

    assert_eq!(
        doc.get("schema_version")
            .and_then(toml_edit::Item::as_integer),
        Some(1),
        "unexpected schema_version"
    );

    let platform = doc.get("platform").expect("missing [platform]");
    assert_eq!(
        platform.get("kind").and_then(toml_edit::Item::as_str),
        Some("darwin"),
        "unexpected platform.kind"
    );

    let slots = doc
        .get("slots")
        .and_then(toml_edit::Item::as_array_of_tables)
        .expect("missing [[slots]]");

    let kind_values: Vec<&str> = slots
        .iter()
        .filter_map(|t| t.get("kind").and_then(toml_edit::Item::as_str))
        .collect();

    for expected_kind in &[
        "nix-packages",
        "homebrew-list",
        "mas-apps",
        "with-packages",
        "services",
    ] {
        assert!(
            kind_values.contains(expected_kind),
            "missing slot kind '{expected_kind}' in manifest\nkinds: {kind_values:?}"
        );
    }

    let has_cli_slot = slots
        .iter()
        .any(|t| t.get("file").and_then(toml_edit::Item::as_str) == Some("packages/nix/cli.nix"));
    assert!(has_cli_slot, "missing packages/nix/cli.nix slot");

    let has_default_install = slots.iter().any(|t| {
        t.get("default_for")
            .and_then(toml_edit::Item::as_array)
            .is_some_and(|arr| {
                arr.iter()
                    .any(|v| v.as_str().is_some_and(|s| s == "install"))
            })
    });
    assert!(
        has_default_install,
        "no slot has default_for = [\"install\"]"
    );

    Ok(())
}

#[test]
fn init_refresh_is_idempotent() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let tmp = TempDir::new()?;
    copy_tree(&repo_base, tmp.path())?;
    let home_dir = TempDir::new()?;

    let out1 = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["init"]);
    assert_eq!(out1.status.code().unwrap_or(-1), 0, "first init failed");

    let manifest1 = fs::read(tmp.path().join(".nx/manifest.toml"))?;

    let out2 = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["init", "--refresh"]);
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert_eq!(
        out2.status.code().unwrap_or(-1),
        0,
        "refresh failed\nstdout:\n{stdout2}\nstderr:\n{stderr2}"
    );

    let manifest2 = fs::read(tmp.path().join(".nx/manifest.toml"))?;
    assert_eq!(manifest1, manifest2, "refresh changed manifest bytes");

    Ok(())
}

#[test]
fn init_refresh_migrates_legacy_darwin_rebuild_command() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let tmp = TempDir::new()?;
    copy_tree(&repo_base, tmp.path())?;
    let home_dir = TempDir::new()?;

    let out1 = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["init"]);
    assert_eq!(out1.status.code().unwrap_or(-1), 0, "first init failed");

    let manifest_path = tmp.path().join(".nx/manifest.toml");
    let old_manifest = fs::read_to_string(&manifest_path)?.replace(
        "/nix/var/nix/profiles/system/sw/bin/darwin-rebuild",
        "/run/current-system/sw/bin/darwin-rebuild",
    );
    fs::write(&manifest_path, old_manifest)?;

    let out2 = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["init", "--refresh"]);
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert_eq!(
        out2.status.code().unwrap_or(-1),
        0,
        "refresh failed\nstdout:\n{stdout2}\nstderr:\n{stderr2}"
    );

    let refreshed = fs::read_to_string(&manifest_path)?;
    assert!(refreshed.contains("/nix/var/nix/profiles/system/sw/bin/darwin-rebuild"));
    assert!(!refreshed.contains("/run/current-system/sw/bin/darwin-rebuild"));

    Ok(())
}

#[test]
fn list_parity_with_manifest() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let tmp = TempDir::new()?;
    copy_tree(&repo_base, tmp.path())?;
    let home_dir = TempDir::new()?;

    let list_before = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["list", "--json"]);
    assert_eq!(
        list_before.status.code().unwrap_or(-1),
        0,
        "list (pre-init) failed: {}",
        String::from_utf8_lossy(&list_before.stderr)
    );

    let init_out = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["init"]);
    assert_eq!(
        init_out.status.code().unwrap_or(-1),
        0,
        "init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let list_after = run_nx(&nx_bin, tmp.path(), home_dir.path(), &["list", "--json"]);
    assert_eq!(
        list_after.status.code().unwrap_or(-1),
        0,
        "list (post-init) failed: {}",
        String::from_utf8_lossy(&list_after.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&list_before.stdout),
        String::from_utf8_lossy(&list_after.stdout),
        "list output changed after init"
    );

    Ok(())
}
