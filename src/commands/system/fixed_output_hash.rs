use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};

use crate::infra::shell::run_captured_command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FixedOutputHashMismatch {
    pub(super) specified: String,
    pub(super) got: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FixedOutputHashTarget {
    pub(super) path: PathBuf,
    pub(super) line_number: usize,
    pub(super) column_number: usize,
}

pub(super) fn parse_fixed_output_hash_mismatch(output: &str) -> Option<FixedOutputHashMismatch> {
    if !output.contains("hash mismatch in fixed-output derivation") {
        return None;
    }

    let mut specified = None;
    let mut got = None;

    for line in output.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("specified:") {
            specified = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("got:") {
            got = Some(value.trim().to_string());
        }
    }

    match (specified, got) {
        (Some(specified), Some(got)) if !specified.is_empty() && !got.is_empty() => {
            Some(FixedOutputHashMismatch { specified, got })
        }
        _ => None,
    }
}

pub(super) fn find_fixed_output_hash_targets(
    repo_root: &Path,
    hash: &str,
) -> anyhow::Result<Vec<FixedOutputHashTarget>> {
    let mut targets = Vec::new();
    for rel_path in tracked_nix_files(repo_root)? {
        let full_path = repo_root.join(&rel_path);
        let content = fs::read_to_string(&full_path)
            .with_context(|| format!("reading {}", full_path.display()))?;

        for (index, line) in content.lines().enumerate() {
            for (column, _) in line.match_indices(hash) {
                targets.push(FixedOutputHashTarget {
                    path: rel_path.clone(),
                    line_number: index + 1,
                    column_number: column + 1,
                });
            }
        }
    }
    Ok(targets)
}

pub(super) fn path_is_clean(repo_root: &Path, rel_path: &Path) -> bool {
    let Some(path) = rel_path.to_str() else {
        return false;
    };
    run_captured_command(
        "git",
        &["status", "--porcelain=v1", "--", path],
        Some(repo_root),
    )
    .is_ok_and(|output| output.code == 0 && output.stdout.trim().is_empty())
}

pub(super) fn apply_fixed_output_hash_repair(
    repo_root: &Path,
    target: &FixedOutputHashTarget,
    mismatch: &FixedOutputHashMismatch,
) -> anyhow::Result<()> {
    let full_path = repo_root.join(&target.path);
    let content = fs::read_to_string(&full_path)
        .with_context(|| format!("reading {}", full_path.display()))?;

    let mut updated = String::with_capacity(content.len());
    let mut replaced = false;
    for (index, segment) in content.split_inclusive('\n').enumerate() {
        if index + 1 != target.line_number {
            updated.push_str(segment);
            continue;
        }

        let start = target.column_number.saturating_sub(1);
        let Some(rest) = segment.get(start..) else {
            bail!(
                "hash location {}:{} is no longer valid",
                target.path.display(),
                target.line_number
            );
        };
        if !rest.starts_with(&mismatch.specified) {
            bail!(
                "hash {} no longer appears at {}:{}",
                mismatch.specified,
                target.path.display(),
                target.line_number
            );
        }

        let end = start + mismatch.specified.len();
        updated.push_str(&segment[..start]);
        updated.push_str(&mismatch.got);
        updated.push_str(&segment[end..]);
        replaced = true;
    }

    if !replaced {
        bail!(
            "line {} no longer exists in {}",
            target.line_number,
            target.path.display()
        );
    }

    fs::write(&full_path, updated).with_context(|| format!("writing {}", full_path.display()))
}

fn tracked_nix_files(repo_root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let output = run_captured_command("git", &["ls-files", "--", "*.nix"], Some(repo_root))
        .context("listing tracked nix files")?;

    if output.code != 0 {
        bail!("git ls-files failed");
    }

    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| is_safe_relative_path(path))
        .collect())
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}
