#[path = "support/bin.rs"]
mod support_bin;
#[path = "support/command_io.rs"]
mod support_command_io;
#[path = "support/tree.rs"]
mod support_tree;

use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use support_bin::resolve_nx_bin;
use support_command_io::{ensure_test_layout, run_command_with_optional_stdin};
use support_tree::copy_tree;

#[derive(Debug, Clone, Copy)]
struct MatrixCase {
    id: &'static str,
    cli_args: &'static [&'static str],
    expected_exit: i32,
}

const MATRIX_CASES: &[MatrixCase] = &[
    MatrixCase {
        id: "install_missing_args_parser_error",
        cli_args: &["install"],
        expected_exit: 2,
    },
    MatrixCase {
        id: "remove_missing_args_parser_error",
        cli_args: &["remove"],
        expected_exit: 2,
    },
];

#[test]
fn system_command_matrix() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_base = workspace_root.join("tests/fixtures/system/repo_base");
    let nx_bin = resolve_nx_bin(&workspace_root)?;

    for case in MATRIX_CASES {
        run_case(&nx_bin, &repo_base, case)?;
    }

    Ok(())
}

fn run_case(nx_bin: &Path, repo_base: &Path, case: &MatrixCase) -> Result<(), Box<dyn Error>> {
    let repo_root = TempDir::new()?;
    copy_tree(repo_base, repo_root.path())?;
    ensure_test_layout(repo_root.path())?;

    let home_dir = TempDir::new()?;
    let mut command = Command::new(nx_bin);
    command
        .args(["--plain", "--minimal"])
        .args(case.cli_args)
        .current_dir(repo_root.path())
        .env("NX_REPO_ROOT", repo_root.path())
        .env("HOME", home_dir.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PYTHONDONTWRITEBYTECODE", "1");

    let output = run_command_with_optional_stdin(&mut command, None)?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        exit_code, case.expected_exit,
        "case {}: unexpected exit code\nstdout:\n{}\nstderr:\n{}",
        case.id, stdout, stderr
    );

    Ok(())
}
