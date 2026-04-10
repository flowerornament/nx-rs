#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/stubs.rs"]
mod support_stubs;
#[path = "support/tree.rs"]
mod support_tree;

use std::cmp::Reverse;
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
const LIST_JSON_ARGS: &[&str] = &["list", "--json"];
const INFO_JSON_ARGS: &[&str] = &["info", "ripgrep", "--json"];
const INSTALLED_JSON_ARGS: &[&str] = &["installed", "ripgrep", "--json"];
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

struct QueryRun {
    output: std::process::Output,
    repo_root_candidates: Vec<String>,
}

fn run_query_command(
    nx_bin: &std::path::Path,
    repo_base: &std::path::Path,
    args: &[&str],
) -> Result<QueryRun, Box<dyn Error>> {
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

    Ok(QueryRun {
        output,
        repo_root_candidates: repo_root_candidates(tmp.path()),
    })
}

fn run_query_case(
    case_name: &str,
    nx_bin: &std::path::Path,
    repo_base: &std::path::Path,
    case: &QueryCase,
) -> Result<(), Box<dyn Error>> {
    let run = run_query_command(nx_bin, repo_base, case.args)?;
    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);

    assert_eq!(
        run.output.status.code().unwrap_or(-1),
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
    let run = run_query_command(nx_bin, repo_base, args)?;
    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);

    assert_eq!(
        run.output.status.code().unwrap_or(-1),
        0,
        "case {snapshot_name}: unexpected exit code\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let value = normalize_snapshot_json(
        serde_json::from_slice(&run.output.stdout)?,
        &run.repo_root_candidates,
    );
    assert_json_snapshot!(snapshot_name, value);

    Ok(())
}

fn normalize_snapshot_json(mut value: Value, repo_root_candidates: &[String]) -> Value {
    let normalized_location = value
        .get("location")
        .and_then(Value::as_str)
        .map(|location| normalize_repo_relative(location, repo_root_candidates));
    if let Some(location) = normalized_location {
        value["location"] = Value::String(location);
    }
    if value.get("elapsed_ms").is_some() {
        value["elapsed_ms"] = Value::from(0);
    }
    value
}

fn normalize_repo_relative(path: &str, repo_root_candidates: &[String]) -> String {
    repo_root_candidates
        .iter()
        .find_map(|candidate| path.strip_prefix(candidate))
        .map_or_else(
            || path.to_string(),
            |relative| relative.trim_start_matches('/').to_string(),
        )
}

fn repo_root_candidates(repo_root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    push_candidate(&mut out, repo_root.to_string_lossy().to_string());

    if let Ok(canonical) = fs::canonicalize(repo_root) {
        push_candidate(&mut out, canonical.to_string_lossy().to_string());
    }

    let aliases = out
        .iter()
        .filter_map(|candidate| private_path_alias(candidate))
        .collect::<Vec<_>>();
    for alias in aliases {
        push_candidate(&mut out, alias);
    }

    out.sort_by_key(|value| Reverse(value.len()));
    out
}

fn private_path_alias(path: &str) -> Option<String> {
    if let Some(stripped) = path.strip_prefix("/private") {
        return Some(stripped.to_string());
    }
    if path.starts_with("/var/") || path.starts_with("/tmp/") {
        return Some(format!("/private{path}"));
    }
    None
}

fn push_candidate(out: &mut Vec<String>, value: String) {
    if !value.is_empty() && !out.contains(&value) {
        out.push(value);
    }
}

#[test]
fn normalize_snapshot_json_strips_repo_root_from_any_tracked_file() {
    let candidates = vec!["/tmp/nx-repo".to_string()];
    let value = serde_json::json!({
        "location": "/tmp/nx-repo/home/services.nix:7",
    });

    let normalized = normalize_snapshot_json(value, &candidates);

    assert_eq!(normalized["location"], "home/services.nix:7");
}

#[test]
fn normalize_snapshot_json_supports_private_tmp_aliases() {
    let candidates = vec![
        "/tmp/nx-repo".to_string(),
        "/private/tmp/nx-repo".to_string(),
    ];
    let value = serde_json::json!({
        "location": "/private/tmp/nx-repo/system/darwin.nix:11",
    });

    let normalized = normalize_snapshot_json(value, &candidates);

    assert_eq!(normalized["location"], "system/darwin.nix:11");
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
            "info_local_json_flag_renders_json",
            QueryCase {
                args: INFO_JSON_ARGS,
                expected_exit: 0,
                stdout_contains: INFO_JSON_FOUND_STDOUT,
            },
        ),
        (
            "installed_local_json_flag_renders_json",
            QueryCase {
                args: INSTALLED_JSON_ARGS,
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
        "system_query_list_local_json_flag_renders_json",
        &nx_bin,
        &repo_base,
        LIST_JSON_ARGS,
    )
}
