mod support;

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

const PARITY_CAPTURE_ENV: &str = "NX_PARITY_CAPTURE";
const PARITY_TARGET_ENV: &str = "NX_PARITY_TARGET";
const PARITY_RUST_BIN_ENV: &str = "NX_RUST_PARITY_BIN";

#[derive(Debug, Deserialize)]
struct CaseSpec {
    id: String,
    args: Vec<String>,
    #[serde(default)]
    setup: CaseSetup,
    #[serde(default)]
    rust_parity: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CaseSetup {
    #[default]
    None,
    UntrackedNix,
    DefaultLaunchdService,
    StubSystemSuccess,
    StubUpdateFail,
    StubTestFail,
    StubTestMypyFail,
    StubTestUnittestFail,
    StubRebuildFlakeCheckFail,
    StubRebuildGitPreflightFail,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum HarnessTarget {
    Python,
    Rust,
}

impl HarnessTarget {
    fn from_env() -> Result<Self, Box<dyn Error>> {
        match env::var(PARITY_TARGET_ENV)
            .unwrap_or_else(|_| "python".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "python" => Ok(Self::Python),
            "rust" => Ok(Self::Rust),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid {PARITY_TARGET_ENV} value: {other}"),
            )
            .into()),
        }
    }

    fn includes_case(self, case: &CaseSpec) -> bool {
        match self {
            Self::Python => true,
            Self::Rust => case.rust_parity,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ParityOutcome {
    exit_code: i32,
    stdout: String,
    stderr: String,
    file_diff: FileDiff,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct FileDiff {
    added: BTreeMap<String, String>,
    removed: Vec<String>,
    modified: BTreeMap<String, String>,
}

#[test]
fn parity_harness() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures_root = workspace_root.join("tests/fixtures/parity");
    let cases_path = fixtures_root.join("cases.json");
    let baselines_dir = fixtures_root.join("baselines");
    let repo_base_dir = fixtures_root.join("repo_base");
    let reference_cli = workspace_root.join("reference/nx-python/nx");
    let target = HarnessTarget::from_env()?;
    let capture_mode = env::var_os(PARITY_CAPTURE_ENV).is_some();

    if capture_mode && target != HarnessTarget::Python {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{PARITY_CAPTURE_ENV} is only supported with {PARITY_TARGET_ENV}=python"),
        )
        .into());
    }

    let cases: Vec<CaseSpec> = read_cases(&cases_path)?
        .into_iter()
        .filter(|case| target.includes_case(case))
        .collect();
    if cases.is_empty() {
        return Err(io::Error::other("no parity cases selected for target").into());
    }

    let rust_cli = if target == HarnessTarget::Rust {
        Some(resolve_rust_cli(&workspace_root)?)
    } else {
        None
    };

    let mut updated = 0usize;
    for case in cases {
        let temp_repo = materialize_repo(&repo_base_dir, case.setup)?;
        let outcome = run_case(
            target,
            &reference_cli,
            rust_cli.as_deref(),
            &workspace_root,
            temp_repo.path(),
            &case,
        )?;
        let baseline_path = baselines_dir.join(format!("{}.json", case.id));

        if capture_mode {
            write_baseline(&baseline_path, &outcome)?;
            updated += 1;
            continue;
        }

        if !baseline_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "missing baseline for case '{}': {}. Run `just parity-capture`.",
                    case.id,
                    baseline_path.display()
                ),
            )
            .into());
        }

        let expected = read_baseline(&baseline_path)?;
        assert_eq!(outcome, expected, "parity mismatch for case {}", case.id);
    }

    if capture_mode {
        eprintln!("updated {updated} parity baseline files");
    }

    Ok(())
}

fn resolve_rust_cli(workspace_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = env::var_os(PARITY_RUST_BIN_ENV) {
        return Ok(PathBuf::from(path));
    }

    let candidate = workspace_root.join("target/debug/nx-rs");
    if candidate.exists() {
        return Ok(candidate);
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "Rust parity binary not found. Set {PARITY_RUST_BIN_ENV} or run `cargo build --bin nx-rs`."
        ),
    )
    .into())
}

fn run_case(
    target: HarnessTarget,
    reference_cli: &Path,
    rust_cli: Option<&Path>,
    workspace_root: &Path,
    repo_root: &Path,
    case: &CaseSpec,
) -> Result<ParityOutcome, Box<dyn Error>> {
    match target {
        HarnessTarget::Python => run_target_case(reference_cli, workspace_root, repo_root, case),
        HarnessTarget::Rust => {
            let cli = rust_cli.ok_or_else(|| io::Error::other("missing rust parity binary"))?;
            run_target_case(cli, workspace_root, repo_root, case)
        }
    }
}

fn read_cases(path: &Path) -> Result<Vec<CaseSpec>, Box<dyn Error>> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn read_baseline(path: &Path) -> Result<ParityOutcome, Box<dyn Error>> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_baseline(path: &Path, outcome: &ParityOutcome) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(outcome)?;
    fs::write(path, format!("{json}\n"))?;
    Ok(())
}

fn materialize_repo(repo_base_dir: &Path, setup: CaseSetup) -> Result<TempDir, Box<dyn Error>> {
    let temp = TempDir::new()?;
    support::copy_tree(repo_base_dir, temp.path())?;
    init_git_repo(temp.path())?;
    apply_setup(temp.path(), setup)?;
    Ok(temp)
}

fn init_git_repo(repo_root: &Path) -> Result<(), Box<dyn Error>> {
    run_checked(repo_root, ["git", "init", "-q"])?;
    run_checked(repo_root, ["git", "config", "user.name", "nx-rs parity"])?;
    run_checked(
        repo_root,
        ["git", "config", "user.email", "parity@example.invalid"],
    )?;
    run_checked(repo_root, ["git", "add", "."])?;
    run_checked(repo_root, ["git", "commit", "-q", "-m", "baseline fixture"])?;
    Ok(())
}

fn run_checked<I, S>(cwd: &Path, args: I) -> Result<(), Box<dyn Error>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut iter = args.into_iter();
    let program = iter
        .next()
        .ok_or_else(|| io::Error::other("empty command"))?;
    let mut cmd = Command::new(program.as_ref());
    cmd.current_dir(cwd).args(iter);
    let output = cmd.output()?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(io::Error::other(format!(
        "command failed in {}: status={} stdout='{}' stderr='{}'",
        cwd.display(),
        output.status,
        stdout.trim(),
        stderr.trim(),
    ))
    .into())
}

fn apply_setup(repo_root: &Path, setup: CaseSetup) -> Result<(), Box<dyn Error>> {
    match setup {
        CaseSetup::None => Ok(()),
        CaseSetup::UntrackedNix => {
            let path = repo_root.join("home/untracked-parity.nix");
            fs::write(path, "# nx: untracked parity fixture\n{ ... }:\n{\n}\n")?;
            Ok(())
        }
        CaseSetup::DefaultLaunchdService => {
            let darwin_dir = repo_root.join("home/darwin");
            fs::create_dir_all(&darwin_dir)?;
            fs::write(
                darwin_dir.join("default.nix"),
                r#"{ lib, ... }:
{
  launchd.agents.sops-nix.config.EnvironmentVariables.PATH =
    lib.mkForce "/usr/bin:/bin:/usr/sbin:/sbin";
}
"#,
            )?;
            Ok(())
        }
        CaseSetup::StubSystemSuccess => {
            install_system_stubs(repo_root)?;
            materialize_test_layout(repo_root, TestLayout::Passing)?;
            Ok(())
        }
        CaseSetup::StubTestUnittestFail => {
            install_system_stubs(repo_root)?;
            materialize_test_layout(repo_root, TestLayout::Failing)?;
            Ok(())
        }
        CaseSetup::StubUpdateFail
        | CaseSetup::StubTestFail
        | CaseSetup::StubTestMypyFail
        | CaseSetup::StubRebuildFlakeCheckFail
        | CaseSetup::StubRebuildGitPreflightFail => {
            install_system_stubs(repo_root)?;
            materialize_test_layout(repo_root, TestLayout::None)?;
            Ok(())
        }
    }
}

fn install_system_stubs(repo_root: &Path) -> Result<(), Box<dyn Error>> {
    let stub_bin = repo_root.join(".parity-bin");
    fs::create_dir_all(&stub_bin)?;

    support::write_executable(
        &stub_bin.join("git"),
        r#"#!/bin/sh
mode="${NX_PARITY_MODE:-stub_system_success}"

if [ "$1" = "-C" ]; then
  shift
  shift
fi

if [ "$1" = "ls-files" ]; then
  if [ "$mode" = "stub_rebuild_git_preflight_fail" ]; then
    echo "stub git ls-files failed" >&2
    exit 1
  fi
  exit 0
fi

if [ "$1" = "rev-parse" ] && [ "$2" = "--show-toplevel" ]; then
  pwd
  exit 0
fi

exit 0
"#,
    )?;

    support::write_executable(
        &stub_bin.join("nix"),
        r#"#!/bin/sh
mode="${NX_PARITY_MODE:-stub_system_success}"

if [ "$1" = "flake" ] && [ "$2" = "update" ]; then
  if [ "$mode" = "stub_update_fail" ]; then
    echo "stub nix flake update failed"
    exit 1
  fi
  echo "stub nix flake update ok"
  exit 0
fi

if [ "$1" = "flake" ] && [ "$2" = "check" ]; then
  if [ "$mode" = "stub_rebuild_flake_check_fail" ]; then
    echo "stub nix flake check failed" >&2
    exit 1
  fi
  echo "stub nix flake check ok"
  exit 0
fi

echo "stub nix unsupported: $*" >&2
exit 0
"#,
    )?;

    support::write_executable(
        &stub_bin.join("sudo"),
        r#"#!/bin/sh
echo "stub sudo $*"
exit 0
"#,
    )?;

    support::write_executable(
        &stub_bin.join("ruff"),
        r#"#!/bin/sh
if [ "${NX_PARITY_MODE:-}" = "stub_test_fail" ]; then
  echo "stub ruff failed" >&2
  exit 1
fi
echo "stub ruff ok"
exit 0
"#,
    )?;

    support::write_executable(
        &stub_bin.join("mypy"),
        r#"#!/bin/sh
if [ "${NX_PARITY_MODE:-}" = "stub_test_mypy_fail" ]; then
  echo "stub mypy failed" >&2
  exit 1
fi
echo "stub mypy ok"
exit 0
"#,
    )?;

    Ok(())
}

#[derive(Clone, Copy)]
enum TestLayout {
    None,
    Passing,
    Failing,
}

fn materialize_test_layout(repo_root: &Path, layout: TestLayout) -> Result<(), Box<dyn Error>> {
    let tests_dir = repo_root.join("scripts/nx/tests");
    fs::create_dir_all(&tests_dir)?;
    let test_file = tests_dir.join("test_stub.py");
    match layout {
        TestLayout::None => {}
        TestLayout::Passing => {
            fs::write(
                test_file,
                r#"import unittest


class StubTest(unittest.TestCase):
    def test_passes(self):
        self.assertTrue(True)


if __name__ == "__main__":
    unittest.main()
"#,
            )?;
        }
        TestLayout::Failing => {
            fs::write(
                test_file,
                r#"import unittest


class StubTest(unittest.TestCase):
    def test_fails(self):
        self.assertTrue(False)


if __name__ == "__main__":
    unittest.main()
"#,
            )?;
        }
    }
    Ok(())
}

fn run_target_case(
    cli_bin: &Path,
    workspace_root: &Path,
    repo_root: &Path,
    case: &CaseSpec,
) -> Result<ParityOutcome, Box<dyn Error>> {
    let before = snapshot_repo_files(repo_root)?;

    // Keep HOME outside the snapshotted repo tree to avoid cache artifacts
    // polluting file-diff baselines.
    let home_dir = TempDir::new()?;

    let mut command = Command::new(cli_bin);
    command
        .args(["--plain", "--minimal"])
        .args(&case.args)
        .current_dir(repo_root)
        .env("B2NIX_REPO_ROOT", repo_root)
        .env("HOME", home_dir.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb");

    if uses_system_stubs(case.setup) {
        let stub_bin = repo_root.join(".parity-bin");
        let mut path_value = stub_bin.to_string_lossy().to_string();
        if let Some(path) = env::var_os("PATH")
            && !path.is_empty()
        {
            path_value.push(':');
            path_value.push_str(&path.to_string_lossy());
        }
        command.env("PATH", path_value);
    }
    if let Some(mode) = setup_mode(case.setup) {
        command.env("NX_PARITY_MODE", mode);
    }

    let output = command.output()?;
    let after = snapshot_repo_files(repo_root)?;

    Ok(ParityOutcome {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: normalize_text(
            &String::from_utf8_lossy(&output.stdout),
            repo_root,
            workspace_root,
        ),
        stderr: normalize_text(
            &String::from_utf8_lossy(&output.stderr),
            repo_root,
            workspace_root,
        ),
        file_diff: diff_snapshots(&before, &after),
    })
}

fn uses_system_stubs(setup: CaseSetup) -> bool {
    matches!(
        setup,
        CaseSetup::StubSystemSuccess
            | CaseSetup::StubUpdateFail
            | CaseSetup::StubTestFail
            | CaseSetup::StubTestMypyFail
            | CaseSetup::StubTestUnittestFail
            | CaseSetup::StubRebuildFlakeCheckFail
            | CaseSetup::StubRebuildGitPreflightFail
    )
}

fn setup_mode(setup: CaseSetup) -> Option<&'static str> {
    match setup {
        CaseSetup::StubSystemSuccess => Some("stub_system_success"),
        CaseSetup::StubUpdateFail => Some("stub_update_fail"),
        CaseSetup::StubTestFail => Some("stub_test_fail"),
        CaseSetup::StubTestMypyFail => Some("stub_test_mypy_fail"),
        CaseSetup::StubTestUnittestFail => Some("stub_test_unittest_fail"),
        CaseSetup::StubRebuildFlakeCheckFail => Some("stub_rebuild_flake_check_fail"),
        CaseSetup::StubRebuildGitPreflightFail => Some("stub_rebuild_git_preflight_fail"),
        CaseSetup::None | CaseSetup::UntrackedNix | CaseSetup::DefaultLaunchdService => None,
    }
}

fn snapshot_repo_files(repo_root: &Path) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    support::snapshot_repo_files(repo_root, &|rel| {
        rel == ".git"
            || rel.starts_with(".git/")
            || rel == ".parity-home"
            || rel.starts_with(".parity-home/")
            || rel == ".tmp-home"
            || rel.starts_with(".tmp-home/")
    })
}

fn diff_snapshots(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> FileDiff {
    let mut added = BTreeMap::new();
    let mut modified = BTreeMap::new();
    let mut removed = Vec::new();

    for (path, before_content) in before {
        match after.get(path) {
            Some(after_content) => {
                if after_content != before_content {
                    modified.insert(path.clone(), after_content.clone());
                }
            }
            None => removed.push(path.clone()),
        }
    }

    for (path, after_content) in after {
        if !before.contains_key(path) {
            added.insert(path.clone(), after_content.clone());
        }
    }

    FileDiff {
        added,
        removed,
        modified,
    }
}

fn normalize_text(input: &str, repo_root: &Path, workspace_root: &Path) -> String {
    let stripped = ansi_regex().replace_all(input, "");
    let mut normalized = stripped.replace("\r\n", "\n");
    normalized = replace_path_tokens(normalized, repo_root, "<REPO_ROOT>");
    normalized = replace_path_tokens(normalized, workspace_root, "<WORKSPACE_ROOT>");
    normalized = normalize_unittest_timing(normalized);
    normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn normalize_unittest_timing(input: String) -> String {
    unittest_timing_regex()
        .replace_all(&input, |caps: &regex::Captures<'_>| {
            format!("{}0.000{}", &caps[1], &caps[2])
        })
        .into_owned()
}

fn replace_path_tokens(input: String, path: &Path, token: &str) -> String {
    let mut output = input;
    let raw = path.to_string_lossy();
    output = output.replace(raw.as_ref(), token);

    if let Ok(canonical) = fs::canonicalize(path) {
        let canonical = canonical.to_string_lossy();
        output = output.replace(canonical.as_ref(), token);
    }

    output
}

fn unittest_timing_regex() -> &'static Regex {
    static UNITTEST_TIMING_REGEX: OnceLock<Regex> = OnceLock::new();
    UNITTEST_TIMING_REGEX.get_or_init(|| {
        Regex::new(r"(Ran\s+\d+\s+tests?\s+in\s+)\d+\.\d+(s)")
            .expect("invalid unittest timing regex")
    })
}

fn ansi_regex() -> &'static Regex {
    static ANSI_REGEX: OnceLock<Regex> = OnceLock::new();
    ANSI_REGEX.get_or_init(|| Regex::new(r"\x1B\[[0-?]*[ -/]*[@-~]").expect("invalid ANSI regex"))
}

#[test]
fn normalize_unittest_timing_stabilizes_elapsed_seconds() {
    let input = "\
Ran 1 test in 0.001s
Ran 12 tests in 1.987s";
    let output = normalize_unittest_timing(input.to_string());
    assert_eq!(
        output,
        "\
Ran 1 test in 0.000s
Ran 12 tests in 0.000s"
    );
}

#[test]
fn normalize_unittest_timing_leaves_other_lines_unchanged() {
    let input = "running 1 test\npass";
    let output = normalize_unittest_timing(input.to_string());
    assert_eq!(output, input);
}
