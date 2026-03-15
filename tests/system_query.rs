#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/stubs.rs"]
mod support_stubs;
#[path = "support/tree.rs"]
mod support_tree;

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use insta::assert_json_snapshot;
use serde_json::Value;
use tempfile::TempDir;

use support_bin::resolve_nx_bin;
use support_stubs::{LOG_FILE_NAME, STUB_DIR_NAME, install_stubs, prepend_path};
use support_tree::copy_tree;

const WHERE_MISSING_ARGS: &[&str] = &["where"];
const INFO_MISSING_ARGS: &[&str] = &["info"];
const INSTALLED_MISSING_ARGS: &[&str] = &["installed"];
const INFO_FOUND_ARGS: &[&str] = &["info", "ripgrep"];
const INFO_JSON_FOUND_ARGS: &[&str] = &["info", "ripgrep", "--json"];
const LIST_JSON_GLOBAL_ARGS: &[&str] = &["--json", "list"];
const INFO_JSON_GLOBAL_ARGS: &[&str] = &["--json", "info", "ripgrep"];
const INSTALLED_JSON_GLOBAL_ARGS: &[&str] = &["--json", "installed", "ripgrep"];
const INFO_BLEEDING_EDGE_ARGS: &[&str] = &["info", "ripgrep", "--bleeding-edge"];
const INFO_JSON_HM_MODULE_ARGS: &[&str] = &["info", "git", "--json"];
const INFO_JSON_DARWIN_SERVICE_ARGS: &[&str] = &["info", "yabai", "--json"];

const INFO_FOUND_STDOUT: &[&str] = &[
    "ripgrep  installed (nxs)",
    "Location: packages/nix/cli.nix:5",
];
const INFO_JSON_FOUND_STDOUT: &[&str] = &[
    "\"name\": \"ripgrep\"",
    "\"installed\": true",
    "\"sources\": []",
];
const INSTALLED_JSON_GLOBAL_STDOUT: &[&str] =
    &["\"ripgrep\": {\"match\": \"ripgrep\", \"location\": \""];

struct QueryCase {
    args: &'static [&'static str],
    expected_exit: i32,
    stdout_contains: &'static [&'static str],
}

fn run_query_command(
    nx_bin: &std::path::Path,
    repo_base: &std::path::Path,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn Error>> {
    let tmp = TempDir::new()?;
    copy_tree(repo_base, tmp.path())?;
    let stub_dir = tmp.path().join(STUB_DIR_NAME);
    fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;
    let log_path = tmp.path().join(LOG_FILE_NAME);
    let home_dir = TempDir::new()?;

    let output = Command::new(nx_bin)
        .args(["--plain", "--minimal"])
        .args(args)
        .current_dir(tmp.path())
        .env("NX_REPO_ROOT", tmp.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("PATH", prepend_path(&stub_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to execute nx binary");

    Ok(output)
}

fn run_query_case(
    case_name: &str,
    nx_bin: &std::path::Path,
    repo_base: &std::path::Path,
    case: &QueryCase,
) -> Result<(), Box<dyn Error>> {
    let output = run_query_command(nx_bin, repo_base, case.args)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code().unwrap_or(-1),
        case.expected_exit,
        "case {case_name}: unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    for fragment in case.stdout_contains {
        assert!(
            stdout.contains(fragment),
            "case {case_name}: stdout missing expected fragment '{fragment}'\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    Ok(())
}

fn assert_query_json_snapshot(
    snapshot_name: &str,
    nx_bin: &std::path::Path,
    repo_base: &std::path::Path,
    args: &[&str],
) -> Result<(), Box<dyn Error>> {
    let output = run_query_command(nx_bin, repo_base, args)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code().unwrap_or(-1),
        0,
        "case {snapshot_name}: unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let value = normalize_snapshot_json(serde_json::from_slice(&output.stdout)?);
    assert_json_snapshot!(snapshot_name, value);

    Ok(())
}

fn normalize_snapshot_json(mut value: Value) -> Value {
    let normalized_location = value
        .get("location")
        .and_then(Value::as_str)
        .and_then(|location| {
            location
                .find("packages/")
                .map(|index| location[index..].to_string())
        });
    if let Some(location) = normalized_location {
        value["location"] = Value::String(location);
    }
    value
}

#[test]
fn system_query_surface() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    let cases = [
        (
            "where_missing_args_parser_error",
            QueryCase {
                args: WHERE_MISSING_ARGS,
                expected_exit: 2,
                stdout_contains: &[],
            },
        ),
        (
            "info_missing_args_parser_error",
            QueryCase {
                args: INFO_MISSING_ARGS,
                expected_exit: 2,
                stdout_contains: &[],
            },
        ),
        (
            "installed_missing_args_parser_error",
            QueryCase {
                args: INSTALLED_MISSING_ARGS,
                expected_exit: 2,
                stdout_contains: &[],
            },
        ),
        (
            "info_found_installed_plain",
            QueryCase {
                args: INFO_FOUND_ARGS,
                expected_exit: 0,
                stdout_contains: INFO_FOUND_STDOUT,
            },
        ),
        (
            "info_found_bleeding_edge_plain",
            QueryCase {
                args: INFO_BLEEDING_EDGE_ARGS,
                expected_exit: 0,
                stdout_contains: INFO_FOUND_STDOUT,
            },
        ),
        (
            "info_global_json_flag_renders_json",
            QueryCase {
                args: INFO_JSON_GLOBAL_ARGS,
                expected_exit: 0,
                stdout_contains: INFO_JSON_FOUND_STDOUT,
            },
        ),
        (
            "installed_global_json_flag_renders_json",
            QueryCase {
                args: INSTALLED_JSON_GLOBAL_ARGS,
                expected_exit: 0,
                stdout_contains: INSTALLED_JSON_GLOBAL_STDOUT,
            },
        ),
    ];

    for (case_name, case) in &cases {
        run_query_case(case_name, &nx_bin, &repo_base, case)?;
    }

    Ok(())
}

#[test]
fn system_query_info_json_snapshots() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    assert_query_json_snapshot(
        "system_query_info_found_installed_json",
        &nx_bin,
        &repo_base,
        INFO_JSON_FOUND_ARGS,
    )?;
    assert_query_json_snapshot(
        "system_query_info_json_hm_module_known_package",
        &nx_bin,
        &repo_base,
        INFO_JSON_HM_MODULE_ARGS,
    )?;
    assert_query_json_snapshot(
        "system_query_info_json_darwin_service_known_package",
        &nx_bin,
        &repo_base,
        INFO_JSON_DARWIN_SERVICE_ARGS,
    )?;

    Ok(())
}

#[test]
fn system_query_list_json_snapshot() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    assert_query_json_snapshot(
        "system_query_list_global_json_flag_renders_json",
        &nx_bin,
        &repo_base,
        LIST_JSON_GLOBAL_ARGS,
    )
}
