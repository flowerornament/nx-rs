use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::Context;
use regex::Regex;

use crate::domain::location::PackageLocation;
use crate::domain::source::normalize_name;
use crate::infra::config_scan::{collect_nix_files, scan_packages};

#[derive(Debug, Clone)]
pub struct PackageMatch {
    pub name: String,
    pub location: PackageLocation,
}

pub fn find_package(name: &str, repo_root: &Path) -> anyhow::Result<Option<PackageLocation>> {
    let mapped = normalize_name(name);
    let mapped_location = find_package_exact(&mapped, repo_root)?;
    if mapped_location.is_some() {
        return Ok(mapped_location);
    }
    if mapped.eq_ignore_ascii_case(name) {
        return Ok(None);
    }
    find_package_exact(name, repo_root)
}

pub fn find_package_fuzzy(name: &str, repo_root: &Path) -> anyhow::Result<Option<PackageMatch>> {
    if let Some(location) = find_package(name, repo_root)? {
        return Ok(Some(PackageMatch {
            name: normalize_name(name),
            location,
        }));
    }

    let buckets = scan_packages(repo_root)?;
    let all_candidates = all_packages(&buckets);
    if let Some(candidate) = find_fuzzy_match(name, &all_candidates)
        && let Some(location) = find_package_exact(&candidate, repo_root)?
    {
        return Ok(Some(PackageMatch {
            name: candidate,
            location,
        }));
    }

    Ok(None)
}

fn find_package_exact(name: &str, repo_root: &Path) -> anyhow::Result<Option<PackageLocation>> {
    let escaped = regex::escape(name);
    let patterns = build_patterns(&escaped)?;

    for file_path in collect_nix_files(repo_root) {
        let content = fs::read_to_string(&file_path)
            .with_context(|| format!("reading {}", file_path.display()))?;
        for (line_index, line) in content.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if is_alias_rhs_for(line, name) {
                continue;
            }
            if patterns.iter().any(|pattern| pattern.is_match(line)) {
                let output_path = fs::canonicalize(&file_path).unwrap_or(file_path.clone());
                let location = PackageLocation::parse(&format!(
                    "{}:{}",
                    output_path.display(),
                    line_index + 1
                ));
                return Ok(Some(location));
            }
        }
    }

    Ok(None)
}

fn build_patterns(escaped_name: &str) -> anyhow::Result<Vec<Regex>> {
    let raw_patterns = [
        format!(r"(?i)^\s+{escaped_name}\s*(#.*)?$"),
        format!(r"(?i)^\s+pkgs\.{escaped_name}\b"),
        format!(r#"(?i)^\s*"{escaped_name}""#),
        format!(r"(?i)^\s*programs\.{escaped_name}(?:\.enable|\s*=)"),
        format!(r"(?i)^\s*services\.{escaped_name}(?:\.enable|\s*=)"),
        format!(r"(?i)^\s*launchd\.(?:user\.)?agents\.{escaped_name}\s*="),
    ];

    raw_patterns
        .into_iter()
        .map(|pattern| Regex::new(&pattern))
        .collect::<Result<Vec<_>, _>>()
        .context("invalid search pattern")
}

fn is_alias_rhs_for(line: &str, name: &str) -> bool {
    if !line.contains('=') {
        return false;
    }
    let mut parts = line.splitn(2, '=');
    let _lhs = parts.next();
    let rhs = parts.next().unwrap_or_default();
    let quoted = format!("\"{name}\"");
    rhs.contains(&quoted)
}

fn find_fuzzy_match(query: &str, candidates: &[String]) -> Option<String> {
    let query_lower = query.to_ascii_lowercase();

    if let Some(exact) = candidates
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(query))
    {
        return Some(exact.clone());
    }

    if let Some(prefix) = candidates
        .iter()
        .find(|candidate| candidate.to_ascii_lowercase().starts_with(&query_lower))
    {
        return Some(prefix.clone());
    }

    candidates
        .iter()
        .find(|candidate| candidate.to_ascii_lowercase().contains(&query_lower))
        .cloned()
}

fn all_packages(buckets: &crate::infra::config_scan::PackageBuckets) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for package in buckets
        .nxs
        .iter()
        .chain(&buckets.brews)
        .chain(&buckets.casks)
        .chain(&buckets.mas)
        .chain(&buckets.services)
    {
        if seen.insert(package.clone()) {
            out.push(package.clone());
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_nix(root: &Path, rel_path: &str, content: &str) {
        let full = root.join(rel_path);
        fs::create_dir_all(full.parent().expect("nix file should have a parent"))
            .expect("parent dirs should be created");
        fs::write(full, content).expect("nix content should be written");
    }

    #[test]
    fn find_package_uses_shared_alias_normalization() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            r#"{ pkgs }:
[
  neovim
  python3
  ripgrep
  pyyaml
]
"#,
        );

        for alias in ["nvim", "python", "rg", "py-yaml"] {
            let found =
                find_package(alias, root).expect("finder should return a successful search result");
            assert!(
                found.is_some(),
                "expected alias {alias} to resolve to a canonical package"
            );
        }
    }
}
