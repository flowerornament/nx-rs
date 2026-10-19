use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn run_command_with_optional_stdin(
    command: &mut Command,
    stdin: Option<&str>,
) -> Result<std::process::Output, Box<dyn Error>> {
    // Test commands must opt in explicitly to external timing/trace locations.
    // Preserve overrides supplied by a test; remove inherited user configuration.
    for key in [
        "NX_PROFILE_PATH",
        "NX_TRACE_PATH",
        "NX_TRACE_DIR",
        "NX_TRACE_KEEP",
        "NX_TRACE_MAX_BYTES",
        "NX_SUMMARY_TIMEOUT_SECONDS",
        "XDG_STATE_HOME",
    ] {
        if !command.get_envs().any(|(name, _)| name == key) {
            command.env_remove(key);
        }
    }
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

#[allow(dead_code)]
pub fn run_measured_command(
    command: &mut Command,
    stdin: Option<&str>,
) -> Result<(std::process::Output, std::time::Duration), Box<dyn Error>> {
    let started = std::time::Instant::now();
    let output = run_command_with_optional_stdin(command, stdin)?;
    Ok((output, started.elapsed()))
}
