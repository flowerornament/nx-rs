use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, bail, ensure};

use crate::cli::{SecretAddArgs, SecretArgs, SecretCommand};
use crate::commands::context::AppContext;
use crate::output::printer::Printer;

pub fn cmd_secret(args: &SecretArgs, ctx: &AppContext) -> i32 {
    match &args.command {
        SecretCommand::Add(add_args) => cmd_secret_add(add_args, ctx),
    }
}

fn cmd_secret_add(args: &SecretAddArgs, ctx: &AppContext) -> i32 {
    let key = args.key_name();
    if !is_valid_secret_key(key) {
        ctx.printer.error(
            "Invalid secret key. Use lowercase letters, digits, and underscores; start with a letter.",
        );
        return 1;
    }

    let value = match read_secret_value(args) {
        Ok(value) => value,
        Err(err) => {
            ctx.printer
                .error(&format!("failed to read secret value: {err:#}"));
            return 1;
        }
    };

    let outcome = match add_secret_workflow(&ctx.repo_root, key, &value, run_sops_set) {
        Ok(outcome) => outcome,
        Err(err) => {
            ctx.printer.error(&format!("secret add failed: {err:#}"));
            return 1;
        }
    };

    ctx.printer.success(&format!("Secret key '{key}' added"));
    if outcome.secret_names_changed {
        Printer::detail("Updated home/secrets.nix secretNames.");
    } else {
        Printer::detail("home/secrets.nix already contained this key.");
    }
    Printer::detail(&format!(
        "Run `nx rebuild` so new shells expose ${}.",
        to_env_var_name(key)
    ));

    0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SecretWorkflowOutcome {
    secret_names_changed: bool,
}

fn add_secret_workflow<F>(
    repo_root: &Path,
    key: &str,
    value: &str,
    mut set_secret: F,
) -> anyhow::Result<SecretWorkflowOutcome>
where
    F: FnMut(&Path, &str, &str) -> anyhow::Result<()>,
{
    ensure!(!value.is_empty(), "secret value cannot be empty");

    let (secrets_nix_path, secrets_yaml_path) = secret_paths(repo_root);
    ensure!(
        secrets_nix_path.exists(),
        "required file missing: {}",
        secrets_nix_path.display()
    );
    ensure!(
        secrets_yaml_path.exists(),
        "required file missing: {}",
        secrets_yaml_path.display()
    );

    let original_secrets_nix = fs::read_to_string(&secrets_nix_path)
        .with_context(|| format!("reading {}", secrets_nix_path.display()))?;
    let updated = upsert_secret_name(&original_secrets_nix, key)?;

    if updated.changed {
        fs::write(&secrets_nix_path, &updated.content)
            .with_context(|| format!("writing {}", secrets_nix_path.display()))?;
    }

    if let Err(sops_err) = set_secret(&secrets_yaml_path, key, value) {
        if updated.changed
            && let Err(rollback_err) = fs::write(&secrets_nix_path, &original_secrets_nix)
        {
            return Err(rollback_err).context(format!(
                "sops update failed and rollback of {} failed: {sops_err:#}",
                secrets_nix_path.display()
            ));
        }
        return Err(sops_err);
    }

    Ok(SecretWorkflowOutcome {
        secret_names_changed: updated.changed,
    })
}

fn secret_paths(repo_root: &Path) -> (PathBuf, PathBuf) {
    (
        repo_root.join("home/secrets.nix"),
        repo_root.join("secrets/secrets.yaml"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecretNamesUpdate {
    content: String,
    changed: bool,
}

#[derive(Debug)]
struct SecretNameEntry {
    line_index: usize,
    name: String,
}

fn upsert_secret_name(content: &str, key: &str) -> anyhow::Result<SecretNamesUpdate> {
    let had_trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();

    let list_start = lines
        .iter()
        .position(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("secretNames") && trimmed.contains('[')
        })
        .ok_or_else(|| anyhow::anyhow!("secretNames block not found in home/secrets.nix"))?;

    let list_end = (list_start + 1..lines.len())
        .find(|&idx| lines[idx].trim() == "];")
        .ok_or_else(|| anyhow::anyhow!("secretNames block is malformed (missing closing `];`)"))?;

    let mut entries = Vec::<SecretNameEntry>::new();
    for (idx, line) in lines.iter().enumerate().take(list_end).skip(list_start + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(name) = parse_secret_name_line(trimmed) else {
            bail!(
                "secretNames block has unsupported entry format at line {}",
                idx + 1
            );
        };
        entries.push(SecretNameEntry {
            line_index: idx,
            name: name.to_string(),
        });
    }

    if entries.iter().any(|entry| entry.name == key) {
        return Ok(SecretNamesUpdate {
            content: content.to_string(),
            changed: false,
        });
    }

    let insert_at = entries
        .iter()
        .find(|entry| key < entry.name.as_str())
        .map_or(list_end, |entry| entry.line_index);

    let indent = entries
        .iter()
        .find(|entry| entry.line_index >= insert_at)
        .or_else(|| entries.last())
        .map_or_else(
            || "    ".to_string(),
            |entry| leading_whitespace(&lines[entry.line_index]).to_string(),
        );

    lines.insert(insert_at, format!("{indent}\"{key}\""));

    let mut updated_content = lines.join("\n");
    if had_trailing_newline {
        updated_content.push('\n');
    }

    Ok(SecretNamesUpdate {
        content: updated_content,
        changed: true,
    })
}

fn parse_secret_name_line(line: &str) -> Option<&str> {
    if !line.starts_with('"') || !line.ends_with('"') {
        return None;
    }
    let inner = line.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.is_empty() && !inner.contains('"')).then_some(inner)
}

fn leading_whitespace(line: &str) -> &str {
    &line[..line.len().saturating_sub(line.trim_start().len())]
}

fn is_valid_secret_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn to_env_var_name(key: &str) -> String {
    key.to_ascii_uppercase()
}

fn read_secret_value(args: &SecretAddArgs) -> anyhow::Result<String> {
    if let Some(value) = &args.value {
        ensure!(!value.is_empty(), "secret value cannot be empty");
        return Ok(value.clone());
    }

    ensure!(
        args.value_stdin,
        "secret value required: pass --value or --value-stdin"
    );

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("failed reading from stdin")?;
    let value = trim_single_trailing_newline(input);
    ensure!(!value.is_empty(), "secret value from stdin cannot be empty");
    Ok(value)
}

fn trim_single_trailing_newline(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}

fn run_sops_set(secrets_file: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    run_sops_set_with_program(&sops_program(), secrets_file, key, value)
}

fn sops_program() -> String {
    std::env::var("NX_RS_SOPS_BIN").unwrap_or_else(|_| "sops".to_string())
}

fn run_sops_set_with_program(
    program: &str,
    secrets_file: &Path,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    let index = format!("[\"{key}\"]");
    let mut child = Command::new(program)
        .args(["-i", "set", "--value-stdin", "--idempotent"])
        .arg(secrets_file)
        .arg(index)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {program}"))?;

    let json_value = serde_json::to_string(value).context("failed to encode secret value")?;
    let mut stdin = child
        .stdin
        .take()
        .context("failed to open stdin for sops command")?;
    stdin
        .write_all(json_value.as_bytes())
        .context("failed to send secret value to sops")?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .context("failed waiting for sops set command")?;
    if output.status.success() {
        return Ok(());
    }

    let reason = command_failure_text(&output.stdout, &output.stderr);
    let redacted_reason = redact_secret(&reason, value, &json_value);
    if redacted_reason.is_empty() {
        bail!("sops set command failed");
    }
    bail!("sops set command failed: {redacted_reason}");
}

fn command_failure_text(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr_text = String::from_utf8_lossy(stderr);
    let stdout_text = String::from_utf8_lossy(stdout);
    let preferred = if stderr_text.trim().is_empty() {
        stdout_text.trim()
    } else {
        stderr_text.trim()
    };
    preferred
        .lines()
        .next()
        .map_or_else(String::new, |line| line.trim().to_string())
}

fn redact_secret(text: &str, value: &str, json_value: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut redacted = text.to_string();
    if !json_value.is_empty() {
        redacted = redacted.replace(json_value, "[REDACTED]");
    }
    if !value.is_empty() {
        redacted = redacted.replace(value, "[REDACTED]");
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_repo(root: &Path, secrets_nix_content: &str) {
        let home = root.join("home");
        let secrets = root.join("secrets");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&secrets).unwrap();
        fs::write(home.join("secrets.nix"), secrets_nix_content).unwrap();
        fs::write(secrets.join("secrets.yaml"), "sops:\n  version: 3.11.0\n").unwrap();
    }

    fn canonical_secrets_nix() -> &'static str {
        r#"# nx: secrets management (sops, age)
{ config, lib, inputs, ... }:
let
  secretNames = [
    "alpha_key"
    "gamma_key"
  ];
in
{
}
"#
    }

    #[test]
    fn secret_add_workflow_adds_new_key_and_updates_secret_names() {
        let tmp = TempDir::new().unwrap();
        seed_repo(tmp.path(), canonical_secrets_nix());

        let mut sops_called = false;
        let outcome =
            add_secret_workflow(tmp.path(), "beta_key", "super-secret", |path, key, _| {
                sops_called = true;
                assert!(path.ends_with(Path::new("secrets/secrets.yaml")));
                assert_eq!(key, "beta_key");
                Ok(())
            })
            .unwrap();

        assert!(sops_called);
        assert!(outcome.secret_names_changed);

        let updated = fs::read_to_string(tmp.path().join("home/secrets.nix")).unwrap();
        assert_eq!(updated.matches("\"beta_key\"").count(), 1);
        assert!(updated.find("\"alpha_key\"").unwrap() < updated.find("\"beta_key\"").unwrap());
        assert!(updated.find("\"beta_key\"").unwrap() < updated.find("\"gamma_key\"").unwrap());
    }

    #[test]
    fn secret_add_workflow_existing_key_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        seed_repo(tmp.path(), canonical_secrets_nix());
        let before = fs::read_to_string(tmp.path().join("home/secrets.nix")).unwrap();

        let mut sops_calls = 0;
        let outcome = add_secret_workflow(tmp.path(), "alpha_key", "new-value", |_, _, _| {
            sops_calls += 1;
            Ok(())
        })
        .unwrap();

        let after = fs::read_to_string(tmp.path().join("home/secrets.nix")).unwrap();
        assert!(!outcome.secret_names_changed);
        assert_eq!(sops_calls, 1);
        assert_eq!(before, after);
    }

    #[test]
    fn secret_add_workflow_returns_error_when_sops_command_missing() {
        let tmp = TempDir::new().unwrap();
        seed_repo(tmp.path(), canonical_secrets_nix());
        let before = fs::read_to_string(tmp.path().join("home/secrets.nix")).unwrap();

        let err = add_secret_workflow(tmp.path(), "delta_key", "sensitive", |path, key, value| {
            run_sops_set_with_program("__nx_missing_sops__", path, key, value)
        })
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("failed to spawn __nx_missing_sops__"),
            "unexpected error: {err:#}"
        );
        let after = fs::read_to_string(tmp.path().join("home/secrets.nix")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn secret_add_workflow_fails_when_secrets_nix_update_is_unsafe() {
        let tmp = TempDir::new().unwrap();
        seed_repo(tmp.path(), "{ ... }:\n{\n  # no secretNames block\n}\n");

        let mut sops_called = false;
        let err = add_secret_workflow(tmp.path(), "safe_key", "value", |_, _, _| {
            sops_called = true;
            Ok(())
        })
        .unwrap_err();

        assert!(!sops_called);
        assert!(
            err.to_string().contains("secretNames block not found"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn run_sops_set_failure_redacts_secret_value_from_error() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("fake-sops.sh");
        fs::write(
            &script,
            "#!/bin/sh\npayload=\"$(cat)\"\necho \"bad value: ${payload}\" 1>&2\nexit 1\n",
        )
        .unwrap();
        make_executable(&script);

        let secrets_file = tmp.path().join("secrets.yaml");
        fs::write(&secrets_file, "x\n").unwrap();

        let err = run_sops_set_with_program(
            script.to_str().unwrap(),
            &secrets_file,
            "alpha_key",
            "top-secret-value",
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(!text.contains("top-secret-value"));
        assert!(text.contains("[REDACTED]"));
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}
