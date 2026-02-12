use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

const LOG_FILE_NAME: &str = ".system-command-log.tsv";
const STUB_DIR_NAME: &str = ".system-stubs";
const REPO_ROOT_TOKEN: &str = "<REPO_ROOT>";

const REBUILD_PREFLIGHT_ARGS: &[&str] = &[
    "-C",
    REPO_ROOT_TOKEN,
    "ls-files",
    "--others",
    "--exclude-standard",
    "--",
    "home",
    "packages",
    "system",
    "hosts",
];
const REBUILD_FLAKE_ARGS: &[&str] = &["flake", "check", REPO_ROOT_TOKEN];
const TEST_RUFF_ARGS: &[&str] = &["check", "."];
const TEST_MYPY_ARGS: &[&str] = &["."];
const TEST_UNITTEST_ARGS: &[&str] = &["-m", "unittest", "discover", "-s", "scripts/nx/tests"];

const UPDATE_PASSTHROUGH_ARGS: &[&str] = &["update", "--", "--commit-lock-file", "foo"];
const UPDATE_BASE_ARGS: &[&str] = &["update"];
const TEST_BASE_ARGS: &[&str] = &["test"];
const REBUILD_PASSTHROUGH_ARGS: &[&str] = &["rebuild", "--", "--show-trace", "foo"];
const REBUILD_BASE_ARGS: &[&str] = &["rebuild"];

#[derive(Debug, Clone, Copy)]
enum StubMode {
    Success,
    UpdateFail,
    RuffFail,
    MypyFail,
    UnittestFail,
    FlakeCheckFail,
    GitPreflightFail,
    PreflightUntracked,
    DarwinRebuildFail,
}

impl StubMode {
    const fn as_env(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::UpdateFail => "update_fail",
            Self::RuffFail => "ruff_fail",
            Self::MypyFail => "mypy_fail",
            Self::UnittestFail => "unittest_fail",
            Self::FlakeCheckFail => "flake_check_fail",
            Self::GitPreflightFail => "git_preflight_fail",
            Self::PreflightUntracked => "preflight_untracked",
            Self::DarwinRebuildFail => "darwin_rebuild_fail",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ExpectedCwd {
    RepoRoot,
    ScriptsNx,
}

#[derive(Debug, Clone, Copy)]
struct ExpectedCall {
    program: &'static str,
    cwd: ExpectedCwd,
    args: &'static [&'static str],
}

impl ExpectedCall {
    const fn new(program: &'static str, cwd: ExpectedCwd, args: &'static [&'static str]) -> Self {
        Self { program, cwd, args }
    }
}

#[derive(Debug, Clone, Copy)]
struct MatrixCase {
    id: &'static str,
    cli_args: &'static [&'static str],
    mode: StubMode,
    expected_exit: i32,
    expected_calls: &'static [ExpectedCall],
}

#[derive(Debug)]
struct Invocation {
    program: String,
    cwd: PathBuf,
    args: Vec<String>,
}

const UPDATE_SUCCESS_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "nix",
    ExpectedCwd::RepoRoot,
    &["flake", "update", "--commit-lock-file", "foo"],
)];

const UPDATE_FAILURE_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "nix",
    ExpectedCwd::RepoRoot,
    &["flake", "update"],
)];

const TEST_SUCCESS_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("ruff", ExpectedCwd::ScriptsNx, TEST_RUFF_ARGS),
    ExpectedCall::new("mypy", ExpectedCwd::ScriptsNx, TEST_MYPY_ARGS),
    ExpectedCall::new("python3", ExpectedCwd::RepoRoot, TEST_UNITTEST_ARGS),
];

const TEST_RUFF_FAIL_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "ruff",
    ExpectedCwd::ScriptsNx,
    TEST_RUFF_ARGS,
)];

const TEST_MYPY_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("ruff", ExpectedCwd::ScriptsNx, TEST_RUFF_ARGS),
    ExpectedCall::new("mypy", ExpectedCwd::ScriptsNx, TEST_MYPY_ARGS),
];

const TEST_UNITTEST_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("ruff", ExpectedCwd::ScriptsNx, TEST_RUFF_ARGS),
    ExpectedCall::new("mypy", ExpectedCwd::ScriptsNx, TEST_MYPY_ARGS),
    ExpectedCall::new("python3", ExpectedCwd::RepoRoot, TEST_UNITTEST_ARGS),
];

const REBUILD_SUCCESS_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        ExpectedCwd::RepoRoot,
        &[
            "/run/current-system/sw/bin/darwin-rebuild",
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--show-trace",
            "foo",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        ExpectedCwd::RepoRoot,
        &["switch", "--flake", REPO_ROOT_TOKEN, "--show-trace", "foo"],
    ),
];

const REBUILD_GIT_FAIL_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "git",
    ExpectedCwd::RepoRoot,
    REBUILD_PREFLIGHT_ARGS,
)];

const REBUILD_UNTRACKED_CALLS: &[ExpectedCall] = &[ExpectedCall::new(
    "git",
    ExpectedCwd::RepoRoot,
    REBUILD_PREFLIGHT_ARGS,
)];

const REBUILD_FLAKE_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, REBUILD_FLAKE_ARGS),
];

const REBUILD_DARWIN_FAIL_CALLS: &[ExpectedCall] = &[
    ExpectedCall::new("git", ExpectedCwd::RepoRoot, REBUILD_PREFLIGHT_ARGS),
    ExpectedCall::new("nix", ExpectedCwd::RepoRoot, REBUILD_FLAKE_ARGS),
    ExpectedCall::new(
        "sudo",
        ExpectedCwd::RepoRoot,
        &[
            "/run/current-system/sw/bin/darwin-rebuild",
            "switch",
            "--flake",
            REPO_ROOT_TOKEN,
            "--show-trace",
            "foo",
        ],
    ),
    ExpectedCall::new(
        "darwin-rebuild",
        ExpectedCwd::RepoRoot,
        &["switch", "--flake", REPO_ROOT_TOKEN, "--show-trace", "foo"],
    ),
];

const MATRIX_CASES: &[MatrixCase] = &[
    MatrixCase {
        id: "update_success_passthrough",
        cli_args: UPDATE_PASSTHROUGH_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: UPDATE_SUCCESS_CALLS,
    },
    MatrixCase {
        id: "update_failure_exit",
        cli_args: UPDATE_BASE_ARGS,
        mode: StubMode::UpdateFail,
        expected_exit: 1,
        expected_calls: UPDATE_FAILURE_CALLS,
    },
    MatrixCase {
        id: "test_success_sequence",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: TEST_SUCCESS_CALLS,
    },
    MatrixCase {
        id: "test_ruff_failure_short_circuit",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::RuffFail,
        expected_exit: 1,
        expected_calls: TEST_RUFF_FAIL_CALLS,
    },
    MatrixCase {
        id: "test_mypy_failure_short_circuit",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::MypyFail,
        expected_exit: 1,
        expected_calls: TEST_MYPY_FAIL_CALLS,
    },
    MatrixCase {
        id: "test_unittest_failure_exit",
        cli_args: TEST_BASE_ARGS,
        mode: StubMode::UnittestFail,
        expected_exit: 1,
        expected_calls: TEST_UNITTEST_FAIL_CALLS,
    },
    MatrixCase {
        id: "rebuild_success_passthrough",
        cli_args: REBUILD_PASSTHROUGH_ARGS,
        mode: StubMode::Success,
        expected_exit: 0,
        expected_calls: REBUILD_SUCCESS_CALLS,
    },
    MatrixCase {
        id: "rebuild_git_preflight_failure_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: StubMode::GitPreflightFail,
        expected_exit: 1,
        expected_calls: REBUILD_GIT_FAIL_CALLS,
    },
    MatrixCase {
        id: "rebuild_untracked_nix_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: StubMode::PreflightUntracked,
        expected_exit: 1,
        expected_calls: REBUILD_UNTRACKED_CALLS,
    },
    MatrixCase {
        id: "rebuild_flake_check_failure_short_circuit",
        cli_args: REBUILD_BASE_ARGS,
        mode: StubMode::FlakeCheckFail,
        expected_exit: 1,
        expected_calls: REBUILD_FLAKE_FAIL_CALLS,
    },
    MatrixCase {
        id: "rebuild_darwin_failure_exit",
        cli_args: REBUILD_PASSTHROUGH_ARGS,
        mode: StubMode::DarwinRebuildFail,
        expected_exit: 1,
        expected_calls: REBUILD_DARWIN_FAIL_CALLS,
    },
];

#[test]
fn system_command_matrix() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/parity/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    for case in MATRIX_CASES {
        run_case(&nx_bin, &repo_base, case)?;
    }

    Ok(())
}

fn resolve_nx_bin(workspace_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = env::var_os("CARGO_BIN_EXE_nx-rs") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("CARGO_BIN_EXE_nx_rs") {
        return Ok(PathBuf::from(path));
    }

    let candidate = workspace_root.join("target/debug/nx-rs");
    if candidate.exists() {
        return Ok(candidate);
    }

    Err(io::Error::new(io::ErrorKind::NotFound, "missing nx-rs test binary").into())
}

fn run_case(nx_bin: &Path, repo_base: &Path, case: &MatrixCase) -> Result<(), Box<dyn Error>> {
    let repo_root = TempDir::new()?;
    copy_tree(repo_base, repo_root.path())?;
    ensure_test_layout(repo_root.path())?;

    let stub_dir = repo_root.path().join(STUB_DIR_NAME);
    fs::create_dir_all(&stub_dir)?;
    install_stubs(&stub_dir)?;

    let log_path = repo_root.path().join(LOG_FILE_NAME);
    let before = snapshot_repo_files(repo_root.path())?;

    let home_dir = TempDir::new()?;
    let mut command = Command::new(nx_bin);
    command
        .args(["--plain", "--minimal"])
        .args(case.cli_args)
        .current_dir(repo_root.path())
        .env("B2NIX_REPO_ROOT", repo_root.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NX_SYSTEM_IT_LOG", &log_path)
        .env("NX_SYSTEM_IT_MODE", case.mode.as_env())
        .env(
            "NX_SYSTEM_IT_DARWIN_REBUILD",
            stub_dir.join("darwin-rebuild"),
        )
        .env("PATH", prepend_path(&stub_dir));

    let output = command.output()?;
    let after = snapshot_repo_files(repo_root.path())?;
    let invocations = read_invocations(&log_path)?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        exit_code, case.expected_exit,
        "case {}: unexpected exit code\nstdout:\n{}\nstderr:\n{}",
        case.id, stdout, stderr
    );

    assert_invocations(case.id, repo_root.path(), &invocations, case.expected_calls);

    assert_eq!(
        before, after,
        "case {} mutated repository files\nstdout:\n{}\nstderr:\n{}",
        case.id, stdout, stderr
    );

    Ok(())
}

fn prepend_path(stub_dir: &Path) -> String {
    let mut path_value = stub_dir.to_string_lossy().to_string();
    if let Some(existing) = env::var_os("PATH")
        && !existing.is_empty()
    {
        path_value.push(':');
        path_value.push_str(&existing.to_string_lossy());
    }
    path_value
}

fn assert_invocations(
    case_id: &str,
    repo_root: &Path,
    actual: &[Invocation],
    expected: &[ExpectedCall],
) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "case {case_id}: invocation count mismatch\nactual: {actual:?}"
    );

    for index in 0..expected.len() {
        let expected_call = expected[index];
        let actual_call = &actual[index];

        assert_eq!(
            actual_call.program, expected_call.program,
            "case {case_id}: unexpected program at step {index}: {actual_call:?}"
        );

        let expected_cwd = match expected_call.cwd {
            ExpectedCwd::RepoRoot => REPO_ROOT_TOKEN.to_string(),
            ExpectedCwd::ScriptsNx => format!("{REPO_ROOT_TOKEN}/scripts/nx"),
        };
        let actual_cwd = normalize_value(actual_call.cwd.to_string_lossy().as_ref(), repo_root);
        assert_eq!(
            actual_cwd, expected_cwd,
            "case {case_id}: unexpected cwd at step {index}: {actual_call:?}"
        );

        let mut actual_args = Vec::with_capacity(actual_call.args.len());
        for arg in &actual_call.args {
            actual_args.push(normalize_value(arg, repo_root));
        }

        let mut expected_args = Vec::with_capacity(expected_call.args.len());
        for arg in expected_call.args {
            expected_args.push((*arg).to_string());
        }

        assert_eq!(
            actual_args, expected_args,
            "case {case_id}: unexpected args at step {index}: {actual_call:?}"
        );
    }
}

fn read_invocations(path: &Path) -> Result<Vec<Invocation>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(path)?;
    let mut out = Vec::new();

    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let mut parts = line.split('\t');
        let program = parts.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("missing program in invocation line {}", index + 1),
            )
        })?;
        let cwd = parts.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("missing cwd in invocation line {}", index + 1),
            )
        })?;

        let mut args = Vec::new();
        for part in parts {
            args.push(part.to_string());
        }

        out.push(Invocation {
            program: program.to_string(),
            cwd: PathBuf::from(cwd),
            args,
        });
    }

    Ok(out)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            fs::create_dir_all(&dst_path)?;
            copy_tree(&src_path, &dst_path)?;
            continue;
        }

        if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

fn ensure_test_layout(repo_root: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(repo_root.join("scripts/nx/tests"))?;
    Ok(())
}

fn install_stubs(stub_dir: &Path) -> Result<(), Box<dyn Error>> {
    for program in [
        "git",
        "nix",
        "sudo",
        "ruff",
        "mypy",
        "python3",
        "darwin-rebuild",
    ] {
        write_executable(&stub_dir.join(program), STUB_SCRIPT)?;
    }
    Ok(())
}

fn write_executable(path: &Path, content: &str) -> Result<(), Box<dyn Error>> {
    fs::write(path, content)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn snapshot_repo_files(repo_root: &Path) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut files = BTreeMap::new();
    snapshot_dir(repo_root, repo_root, &mut files)?;
    Ok(files)
}

fn snapshot_dir(
    repo_root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, String>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(repo_root)
            .map_err(|err| io::Error::other(format!("strip_prefix failed: {err}")))?;
        let rel_key = rel.to_string_lossy().replace('\\', "/");

        if should_ignore_snapshot_path(&rel_key) {
            continue;
        }

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            snapshot_dir(repo_root, &path, out)?;
            continue;
        }

        if file_type.is_file() {
            let bytes = fs::read(&path)?;
            let text = String::from_utf8_lossy(&bytes);
            out.insert(rel_key, normalize_file_content(&text));
        }
    }

    Ok(())
}

fn should_ignore_snapshot_path(rel_path: &str) -> bool {
    rel_path == LOG_FILE_NAME || rel_path == STUB_DIR_NAME || rel_path.starts_with(".system-stubs/")
}

fn normalize_file_content(input: &str) -> String {
    input
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn normalize_value(input: &str, repo_root: &Path) -> String {
    let mut output = input.to_string();
    for candidate in repo_root_candidates(repo_root) {
        output = output.replace(candidate.as_str(), REPO_ROOT_TOKEN);
    }
    output
}

fn repo_root_candidates(repo_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    push_candidate(&mut out, repo_root.to_string_lossy().to_string());

    if let Ok(canonical) = fs::canonicalize(repo_root) {
        push_candidate(&mut out, canonical.to_string_lossy().to_string());
    }

    let mut aliases = Vec::new();
    for candidate in &out {
        if let Some(alias) = private_path_alias(candidate) {
            aliases.push(alias);
        }
    }
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

const STUB_SCRIPT: &str = r#"#!/bin/sh
set -eu

program="$(basename "$0")"
log_path="${NX_SYSTEM_IT_LOG:?NX_SYSTEM_IT_LOG must be set}"
mode="${NX_SYSTEM_IT_MODE:-success}"

printf "%s\t%s" "$program" "$PWD" >> "$log_path"
for arg in "$@"; do
  printf "\t%s" "$arg" >> "$log_path"
done
printf "\n" >> "$log_path"

case "$program" in
  git)
    if [ "${1:-}" = "-C" ]; then
      shift
      shift
    fi

    if [ "${1:-}" = "ls-files" ]; then
      if [ "$mode" = "git_preflight_fail" ]; then
        echo "stub git ls-files failed" >&2
        exit 1
      fi
      if [ "$mode" = "preflight_untracked" ]; then
        echo "home/untracked-from-stub.nix"
        exit 0
      fi
      exit 0
    fi

    if [ "${1:-}" = "rev-parse" ] && [ "${2:-}" = "--show-toplevel" ]; then
      pwd
      exit 0
    fi

    exit 0
    ;;
  nix)
    if [ "${1:-}" = "flake" ] && [ "${2:-}" = "update" ]; then
      if [ "$mode" = "update_fail" ]; then
        echo "stub nix flake update failed"
        exit 1
      fi
      echo "stub nix flake update ok"
      exit 0
    fi

    if [ "${1:-}" = "flake" ] && [ "${2:-}" = "check" ]; then
      if [ "$mode" = "flake_check_fail" ]; then
        echo "stub nix flake check failed" >&2
        exit 1
      fi
      echo "stub nix flake check ok"
      exit 0
    fi

    echo "stub nix unsupported: $*" >&2
    exit 1
    ;;
  sudo)
    if [ "$mode" = "sudo_fail" ]; then
      echo "stub sudo failed" >&2
      exit 1
    fi

    if [ "${1:-}" = "/run/current-system/sw/bin/darwin-rebuild" ]; then
      shift
      "${NX_SYSTEM_IT_DARWIN_REBUILD:?NX_SYSTEM_IT_DARWIN_REBUILD must be set}" "$@"
      exit $?
    fi

    echo "stub sudo $*"
    exit 0
    ;;
  ruff)
    if [ "$mode" = "ruff_fail" ]; then
      echo "stub ruff failed" >&2
      exit 1
    fi
    echo "stub ruff ok"
    exit 0
    ;;
  mypy)
    if [ "$mode" = "mypy_fail" ]; then
      echo "stub mypy failed" >&2
      exit 1
    fi
    echo "stub mypy ok"
    exit 0
    ;;
  python3)
    if [ "$mode" = "unittest_fail" ]; then
      echo "stub unittest failed" >&2
      exit 1
    fi
    echo "stub unittest ok"
    exit 0
    ;;
  darwin-rebuild)
    if [ "$mode" = "darwin_rebuild_fail" ]; then
      echo "stub darwin-rebuild failed" >&2
      exit 1
    fi
    echo "stub darwin-rebuild ok"
    exit 0
    ;;
  *)
    echo "unsupported stub program: $program" >&2
    exit 99
    ;;
esac
"#;
