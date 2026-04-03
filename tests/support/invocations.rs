use std::cmp::Reverse;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const REPO_ROOT_TOKEN: &str = "<REPO_ROOT>";
pub const EXPECTED_CWD_REPO_ROOT: &str = "<REPO_ROOT>";

#[derive(Debug, Clone, Copy)]
pub struct ExpectedCall {
    program: &'static str,
    cwd: &'static str,
    args: &'static [&'static str],
    env: &'static [(&'static str, &'static str)],
}

impl ExpectedCall {
    pub const fn new(
        program: &'static str,
        cwd: &'static str,
        args: &'static [&'static str],
    ) -> Self {
        Self {
            program,
            cwd,
            args,
            env: &[],
        }
    }

    #[allow(dead_code)]
    pub const fn with_env(self, env: &'static [(&'static str, &'static str)]) -> Self {
        Self { env, ..self }
    }
}

#[derive(Debug)]
pub struct Invocation {
    pub program: String,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub fn assert_invocations(
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

        let actual_cwd = normalize_value(actual_call.cwd.to_string_lossy().as_ref(), repo_root);
        assert_eq!(
            actual_cwd, expected_call.cwd,
            "case {case_id}: unexpected cwd at step {index}: {actual_call:?}"
        );

        let actual_args = actual_call
            .args
            .iter()
            .map(|arg| normalize_value(arg, repo_root))
            .collect::<Vec<_>>();
        let expected_args = expected_call
            .args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            actual_args, expected_args,
            "case {case_id}: unexpected args at step {index}: {actual_call:?}"
        );

        let actual_env = actual_call
            .env
            .iter()
            .map(|(key, value)| (key.clone(), normalize_value(value, repo_root)))
            .collect::<Vec<_>>();
        let expected_env = expected_call
            .env
            .iter()
            .map(|(key, value)| (key.to_string(), (*value).to_string()))
            .collect::<Vec<_>>();

        assert_eq!(
            actual_env, expected_env,
            "case {case_id}: unexpected env at step {index}: {actual_call:?}"
        );
    }
}

pub fn read_invocations(path: &Path) -> Result<Vec<Invocation>, Box<dyn Error>> {
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
        let mut env = Vec::new();
        for part in parts {
            if let Some(raw) = part.strip_prefix("ENV:")
                && let Some((key, value)) = raw.split_once('=')
            {
                env.push((key.to_string(), value.to_string()));
                continue;
            }
            args.push(part.to_string());
        }
        out.push(Invocation {
            program: program.to_string(),
            cwd: PathBuf::from(cwd),
            args,
            env,
        });
    }

    Ok(out)
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
