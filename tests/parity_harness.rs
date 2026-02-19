mod support;

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
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
    #[serde(default = "default_true")]
    python_parity: bool,
    #[serde(default)]
    python_parity_reason: Option<String>,
    #[serde(default)]
    rust_parity: bool,
    #[serde(default)]
    rust_parity_reason: Option<String>,
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
    StubInfoSources,
    StubInfoSourcesBleedingEdge,
    StubInstallSources,
    StubInstallSourcesCacheHit,
    ModifiedTrackedFile,
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
            Self::Python => case.python_parity,
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
    let reference_cli = env::var_os("NX_PYTHON_CLI")
        .map_or_else(|| home_dir().join("code/nx-python/nx"), PathBuf::from);
    let target = HarnessTarget::from_env()?;
    let capture_mode = env::var_os(PARITY_CAPTURE_ENV).is_some();

    let all_cases = read_cases(&cases_path)?;
    validate_case_annotations(&all_cases)?;
    let cases: Vec<CaseSpec> = all_cases
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

    let candidate = workspace_root.join("target/debug/nx");
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

fn validate_case_annotations(cases: &[CaseSpec]) -> Result<(), Box<dyn Error>> {
    for case in cases {
        if !case.python_parity {
            let Some(reason) = case.python_parity_reason.as_deref() else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "case '{}' has python_parity=false without python_parity_reason",
                        case.id
                    ),
                )
                .into());
            };
            if reason.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("case '{}' has empty python_parity_reason", case.id),
                )
                .into());
            }
        }

        if !case.rust_parity {
            let Some(reason) = case.rust_parity_reason.as_deref() else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "case '{}' has rust_parity=false without rust_parity_reason",
                        case.id
                    ),
                )
                .into());
            };
            if reason.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("case '{}' has empty rust_parity_reason", case.id),
                )
                .into());
            }
        }

        if !case.python_parity && !case.rust_parity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("case '{}' is disabled for both parity targets", case.id),
            )
            .into());
        }
    }
    Ok(())
}

const fn default_true() -> bool {
    true
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
        CaseSetup::ModifiedTrackedFile => {
            let target = repo_root.join("packages/nix/cli.nix");
            fs::OpenOptions::new()
                .append(true)
                .open(target)?
                .write_all(b"# parity-modified\n")?;
            Ok(())
        }
        CaseSetup::StubUpdateFail
        | CaseSetup::StubTestFail
        | CaseSetup::StubTestMypyFail
        | CaseSetup::StubRebuildFlakeCheckFail
        | CaseSetup::StubRebuildGitPreflightFail
        | CaseSetup::StubInfoSources
        | CaseSetup::StubInfoSourcesBleedingEdge
        | CaseSetup::StubInstallSources
        | CaseSetup::StubInstallSourcesCacheHit => {
            install_system_stubs(repo_root)?;
            materialize_test_layout(repo_root, TestLayout::None)?;
            Ok(())
        }
    }
}

fn install_system_stubs(repo_root: &Path) -> Result<(), Box<dyn Error>> {
    let stub_bin = repo_root.join(".parity-bin");
    fs::create_dir_all(&stub_bin)?;

    write_git_stub(&stub_bin)?;
    write_nix_stub(&stub_bin)?;
    write_sudo_stub(&stub_bin)?;
    write_brew_stub(&stub_bin)?;
    write_ruff_stub(&stub_bin)?;
    write_mypy_stub(&stub_bin)?;
    support::write_executable(&stub_bin.join("gh"), "#!/bin/sh\nexit 1\n")?;

    Ok(())
}

fn write_git_stub(stub_bin: &Path) -> Result<(), Box<dyn Error>> {
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
    Ok(())
}

#[allow(clippy::too_many_lines)] // embedded shell script keeps stub behavior in one place
fn write_nix_stub(stub_bin: &Path) -> Result<(), Box<dyn Error>> {
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

if [ "$mode" = "stub_info_sources" ] || [ "$mode" = "stub_info_sources_bleeding_edge" ]; then
  if [ "$1" = "search" ] && [ "$2" = "--json" ]; then
    query="$4"
    if [ "$query" = "python3Packages.requests" ] || [ "$query" = "requests" ]; then
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.python3Packages.requests":{"pname":"requests","version":"1.0.0","description":"Stub Python requests package"}}
JSON
      exit 0
    fi
    if [ "$query" = "ripgrep" ]; then
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.ripgrep":{"pname":"ripgrep","version":"14.1.1","description":"Stub ripgrep package"}}
JSON
      exit 0
    fi
    echo "{}"
    exit 0
  fi

  if [ "$1" = "eval" ] && [ "$2" = "--json" ]; then
    attr="$3"
    case "$attr" in
      *python3Packages.requests.name)
        echo "\"python3Packages.requests\""
        ;;
      *python3Packages.requests.version)
        echo "\"1.0.0\""
        ;;
      *python3Packages.requests.meta)
        cat <<'JSON'
{"description":"Stub Python requests package","homepage":"https://example.test/requests","license":{"spdxId":"MIT"},"broken":false,"insecure":false}
JSON
        ;;
      *ripgrep.name)
        echo "\"ripgrep\""
        ;;
      *ripgrep.version)
        echo "\"14.1.1\""
        ;;
      *ripgrep.meta)
        cat <<'JSON'
{"description":"Stub ripgrep package","homepage":"https://example.test/ripgrep","license":{"spdxId":"MIT"},"broken":false,"insecure":false}
JSON
        ;;
      *)
        echo "null"
        ;;
    esac
    exit 0
  fi
fi

if [ "$mode" = "stub_install_sources" ] || [ "$mode" = "stub_install_sources_cache_hit" ]; then
  if [ "$1" = "search" ] && [ "$2" = "--json" ]; then
    target="$3"
    query="$4"
    if [ "$query" = "nxrsmulti" ]; then
      if [ "$target" = "github:nix-community/NUR" ]; then
        cat <<'JSON'
{"legacyPackages.aarch64-darwin.nxrsmultinur":{"pname":"nxrsmultinur","version":"1.0.0","description":"Stub NUR multi-source candidate"}}
JSON
        exit 0
      fi
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.nxrsmultinxs":{"pname":"nxrsmultinxs","version":"1.0.0","description":"Stub nixpkgs multi-source candidate"}}
JSON
      exit 0
    fi
    if [ "$query" = "nxrsdryrun" ]; then
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.nxrsdryrun":{"pname":"nxrsdryrun","version":"1.0.0","description":"Stub dry-run install candidate"}}
JSON
      exit 0
    fi
    if [ "$query" = "nxrsplatform" ]; then
      cat <<'JSON'
{
  "legacyPackages.aarch64-darwin.nxrsplatformincompatible": {
    "pname": "nxrsplatformincompatible",
    "version": "1.0.0",
    "description": "Stub platform-incompatible candidate"
  },
  "legacyPackages.aarch64-darwin.nxrsplatformfallback": {
    "pname": "nxrsplatformfallback",
    "version": "1.0.0",
    "description": "Stub platform-compatible fallback"
  }
}
JSON
      exit 0
    fi
    echo "{}"
    exit 0
  fi

  if [ "$1" = "eval" ] && [ "$2" = "--json" ]; then
    attr="$3"
    case "$attr" in
      *nxrsplatformincompatible.meta.platforms)
        echo '["x86_64-windows","aarch64-windows"]'
        ;;
      *.meta.platforms)
        echo '["aarch64-darwin","x86_64-darwin","x86_64-linux","aarch64-linux"]'
        ;;
      *)
        echo "null"
        ;;
    esac
    exit 0
  fi
fi

echo "stub nix unsupported: $*" >&2
exit 0
"#,
    )?;
    Ok(())
}

fn write_sudo_stub(stub_bin: &Path) -> Result<(), Box<dyn Error>> {
    support::write_executable(
        &stub_bin.join("sudo"),
        r#"#!/bin/sh
echo "stub sudo $*"
exit 0
"#,
    )?;
    Ok(())
}

fn write_brew_stub(stub_bin: &Path) -> Result<(), Box<dyn Error>> {
    support::write_executable(
        &stub_bin.join("brew"),
        r#"#!/bin/sh
mode="${NX_PARITY_MODE:-stub_system_success}"

if [ "$1" = "info" ] && [ "$2" = "--json=v2" ]; then
  if [ "$mode" = "stub_install_sources" ] || [ "$mode" = "stub_install_sources_cache_hit" ]; then
    if [ "$3" = "--cask" ]; then
      query="$4"
      if [ "$query" = "nxrsmulti" ]; then
        echo '{"casks":[{"token":"nxrsmulticask","version":"1.0.0","desc":"Stub cask multi-source candidate"}]}'
        exit 0
      fi
      echo '{"casks":[]}'
      exit 0
    fi

    query="$3"
    if [ "$query" = "nxrsmulti" ]; then
      echo '{"formulae":[{"name":"nxrsmultibrew","versions":{"stable":"1.0.0"},"desc":"Stub Homebrew multi-source candidate"}]}'
      exit 0
    fi
    echo '{"formulae":[]}'
    exit 0
  fi

  if [ "$3" = "--cask" ]; then
    echo '{"casks":[]}'
    exit 0
  fi
  echo '{"formulae":[]}'
  exit 0
fi

echo "stub brew unsupported: $*" >&2
exit 1
"#,
    )?;
    Ok(())
}

fn write_ruff_stub(stub_bin: &Path) -> Result<(), Box<dyn Error>> {
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
    Ok(())
}

fn write_mypy_stub(stub_bin: &Path) -> Result<(), Box<dyn Error>> {
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
    prepare_home_for_case(home_dir.path(), case.setup)?;

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
    if blocks_external_network(case.setup) {
        for key in [
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "https_proxy",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            command.env(key, "http://127.0.0.1:9");
        }
        command.env("NO_PROXY", "");
        command.env("no_proxy", "");
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
            | CaseSetup::StubInfoSources
            | CaseSetup::StubInfoSourcesBleedingEdge
            | CaseSetup::StubInstallSources
            | CaseSetup::StubInstallSourcesCacheHit
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
        CaseSetup::StubInfoSources => Some("stub_info_sources"),
        CaseSetup::StubInfoSourcesBleedingEdge => Some("stub_info_sources_bleeding_edge"),
        CaseSetup::StubInstallSources => Some("stub_install_sources"),
        CaseSetup::StubInstallSourcesCacheHit => Some("stub_install_sources_cache_hit"),
        CaseSetup::None
        | CaseSetup::UntrackedNix
        | CaseSetup::DefaultLaunchdService
        | CaseSetup::ModifiedTrackedFile => None,
    }
}

fn blocks_external_network(setup: CaseSetup) -> bool {
    matches!(
        setup,
        CaseSetup::StubInfoSourcesBleedingEdge
            | CaseSetup::StubInstallSources
            | CaseSetup::StubInstallSourcesCacheHit
    )
}

fn prepare_home_for_case(home_dir: &Path, setup: CaseSetup) -> Result<(), Box<dyn Error>> {
    if matches!(setup, CaseSetup::StubInstallSourcesCacheHit) {
        seed_cache_hit(home_dir)?;
    }
    Ok(())
}

fn seed_cache_hit(home_dir: &Path) -> Result<(), Box<dyn Error>> {
    let cache_dir = home_dir.join(".cache/nx");
    fs::create_dir_all(&cache_dir)?;
    let cache = json!({
        "schema_version": 1,
        "entries": {
            "nxrscachehit|nxs|unknown": {
                "attr": "nxrscachehit",
                "version": "1.0.0",
                "description": "Stub cache-hit candidate",
                "confidence": 1.0,
                "requires_flake_mod": false,
                "flake_url": null
            }
        }
    });
    let cache_json = serde_json::to_string_pretty(&cache)?;
    fs::write(
        cache_dir.join("packages_v4.json"),
        format!("{cache_json}\n"),
    )?;
    Ok(())
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
    normalized = normalize_unittest_timing(&normalized);
    normalized = normalize_rebuild_ulimit_wrapper(&normalized);
    normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn normalize_unittest_timing(input: &str) -> String {
    unittest_timing_regex()
        .replace_all(input, |caps: &regex::Captures<'_>| {
            format!("{}0.000{}", &caps[1], &caps[2])
        })
        .into_owned()
}

fn normalize_rebuild_ulimit_wrapper(input: &str) -> String {
    rebuild_ulimit_wrapper_regex()
        .replace_all(
            input,
            "stub sudo /run/current-system/sw/bin/darwin-rebuild switch --flake",
        )
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

fn rebuild_ulimit_wrapper_regex() -> &'static Regex {
    static REBUILD_ULIMIT_WRAPPER_REGEX: OnceLock<Regex> = OnceLock::new();
    REBUILD_ULIMIT_WRAPPER_REGEX.get_or_init(|| {
        Regex::new(
            r"stub sudo bash -lc ulimit -n \d+ 2>/dev/null; exec\s*\n\s*/run/current-system/sw/bin/darwin-rebuild switch --flake",
        )
        .expect("invalid rebuild ulimit wrapper regex")
    })
}

fn home_dir() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
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
    let output = normalize_unittest_timing(input);
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
    let output = normalize_unittest_timing(input);
    assert_eq!(output, input);
}

#[test]
fn normalize_rebuild_ulimit_wrapper_collapses_to_direct_stub_command() {
    let input = "stub sudo bash -lc ulimit -n 8192 2>/dev/null; exec\n  /run/current-system/sw/bin/darwin-rebuild switch --flake";
    let output = normalize_rebuild_ulimit_wrapper(input);
    assert_eq!(
        output,
        "stub sudo /run/current-system/sw/bin/darwin-rebuild switch --flake"
    );
}

#[test]
fn normalize_rebuild_ulimit_wrapper_leaves_other_commands_unchanged() {
    let input = "stub sudo /run/current-system/sw/bin/darwin-rebuild switch --flake";
    let output = normalize_rebuild_ulimit_wrapper(input);
    assert_eq!(output, input);
}
