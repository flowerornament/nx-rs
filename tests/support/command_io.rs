use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn run_command_with_optional_stdin(
    command: &mut Command,
    stdin: Option<&str>,
) -> Result<std::process::Output, Box<dyn Error>> {
    if let Some(input) = stdin {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(input.as_bytes())?;
        }
        return Ok(child.wait_with_output()?);
    }
    Ok(command.output()?)
}

pub fn ensure_test_layout(repo_root: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(repo_root.join("scripts/nx/tests"))?;
    Ok(())
}
