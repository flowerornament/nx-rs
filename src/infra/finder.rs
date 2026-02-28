use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::UNIX_EPOCH;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSignature {
    mtime_ns: u128,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    path: PathBuf,
    signature: FileSignature,
}

#[derive(Debug)]
struct IndexedFile {
    path: PathBuf,
    lines: Vec<String>,
}

#[derive(Debug)]
struct FinderIndex {
    snapshots: Vec<FileSnapshot>,
    files: Vec<IndexedFile>,
}

#[derive(Debug)]
struct FinderIndexEntry {
    rebuilds: usize,
    index: Arc<FinderIndex>,
}

#[derive(Debug, Default)]
struct FinderIndexCache {
    by_repo: HashMap<PathBuf, FinderIndexEntry>,
}

static FINDER_INDEX_CACHE: LazyLock<Mutex<FinderIndexCache>> =
    LazyLock::new(|| Mutex::new(FinderIndexCache::default()));

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

#[cfg(test)]
fn finder_index_rebuilds(repo_root: &Path) -> usize {
    let repo_key = canonical_repo_key(repo_root);
    let cache = FINDER_INDEX_CACHE
        .lock()
        .expect("finder index cache lock should not be poisoned");
    cache
        .by_repo
        .get(&repo_key)
        .map_or(0, |entry| entry.rebuilds)
}

fn find_package_exact(name: &str, repo_root: &Path) -> anyhow::Result<Option<PackageLocation>> {
    let escaped = regex::escape(name);
    let patterns = build_patterns(&escaped)?;
    let index = finder_index(repo_root)?;

    for indexed_file in &index.files {
        for (line_index, line) in indexed_file.lines.iter().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if is_alias_rhs_for(line, name) {
                continue;
            }
            if patterns.iter().any(|pattern| pattern.is_match(line)) {
                let output_path = fs::canonicalize(&indexed_file.path)
                    .unwrap_or_else(|_| indexed_file.path.clone());
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

fn finder_index(repo_root: &Path) -> anyhow::Result<Arc<FinderIndex>> {
    let repo_key = canonical_repo_key(repo_root);
    let snapshots = collect_file_snapshots(&repo_key)?;

    {
        let cache = FINDER_INDEX_CACHE
            .lock()
            .expect("finder index cache lock should not be poisoned");
        if let Some(entry) = cache.by_repo.get(&repo_key)
            && entry.index.snapshots == snapshots
        {
            return Ok(Arc::clone(&entry.index));
        }
    }

    // Build outside the cache lock to avoid serializing disk IO across callers.
    let built_index = Arc::new(build_finder_index(&snapshots)?);

    let mut cache = FINDER_INDEX_CACHE
        .lock()
        .expect("finder index cache lock should not be poisoned");
    if let Some(entry) = cache.by_repo.get(&repo_key)
        && entry.index.snapshots == snapshots
    {
        return Ok(Arc::clone(&entry.index));
    }

    let rebuilds = cache
        .by_repo
        .get(&repo_key)
        .map_or(0, |entry| entry.rebuilds)
        + 1;
    cache.by_repo.insert(
        repo_key,
        FinderIndexEntry {
            rebuilds,
            index: Arc::clone(&built_index),
        },
    );
    Ok(built_index)
}

fn canonical_repo_key(repo_root: &Path) -> PathBuf {
    fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf())
}

fn collect_file_snapshots(repo_root: &Path) -> anyhow::Result<Vec<FileSnapshot>> {
    let mut out = Vec::new();
    for path in collect_nix_files(repo_root) {
        let metadata =
            fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?;
        out.push(FileSnapshot {
            path,
            signature: file_signature(&metadata),
        });
    }
    Ok(out)
}

fn file_signature(metadata: &fs::Metadata) -> FileSignature {
    let mtime_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    FileSignature {
        mtime_ns,
        size: metadata.len(),
    }
}

fn build_finder_index(snapshots: &[FileSnapshot]) -> anyhow::Result<FinderIndex> {
    let mut files = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let content = fs::read_to_string(&snapshot.path)
            .with_context(|| format!("reading {}", snapshot.path.display()))?;
        files.push(IndexedFile {
            path: snapshot.path.clone(),
            lines: content.lines().map(str::to_string).collect(),
        });
    }
    Ok(FinderIndex {
        snapshots: snapshots.to_vec(),
        files,
    })
}

fn build_patterns(escaped_name: &str) -> anyhow::Result<Vec<Regex>> {
    let raw_patterns = [
        format!(r"(?i)^\s+{escaped_name}\s*(#.*)?$"),
        format!(r"(?i)^\s+\S+\.{escaped_name}\s*(#.*)?$"),
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
    let _ = parts.next();
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
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    fn write_nix(root: &Path, rel_path: &str, content: &str) {
        let full = root.join(rel_path);
        fs::create_dir_all(full.parent().expect("nix file should have a parent"))
            .expect("parent dirs should be created");
        fs::write(full, content).expect("nix content should be written");
    }

    #[test]
    fn find_package_matches_qualified_suffix() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            r"{ pkgs }:
[
  ocamlPackages.cow
  haskellPackages.pandoc
  ripgrep
]
",
        );

        let found = find_package("cow", root).unwrap();
        assert!(
            found.is_some(),
            "expected 'cow' to match 'ocamlPackages.cow'"
        );

        let found = find_package("pandoc", root).unwrap();
        assert!(
            found.is_some(),
            "expected 'pandoc' to match 'haskellPackages.pandoc'"
        );
    }

    #[test]
    fn find_package_uses_shared_alias_normalization() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            r"{ pkgs }:
[
  neovim
  python3
  ripgrep
  pyyaml
]
",
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

    #[test]
    fn finder_index_rebuilds_only_when_signatures_change() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        let target = root.join("packages/nix/cli.nix");

        write_nix(
            root,
            "packages/nix/cli.nix",
            r"{ pkgs }:
[
  ripgrep
]
",
        );

        assert_eq!(finder_index_rebuilds(root), 0);

        let first = find_package("ripgrep", root).expect("finder lookup should succeed");
        assert!(first.is_some(), "expected initial package to resolve");
        assert_eq!(finder_index_rebuilds(root), 1);

        let second = find_package("ripgrep", root).expect("finder lookup should succeed");
        assert!(second.is_some(), "expected cached lookup to resolve");
        assert_eq!(finder_index_rebuilds(root), 1);

        thread::sleep(Duration::from_millis(2));
        fs::write(
            &target,
            r"{ pkgs }:
[
  ripgrep
  fd
]
",
        )
        .expect("updated nix content should be written");

        let updated = find_package("fd", root).expect("finder lookup should succeed");
        assert!(updated.is_some(), "expected updated package to resolve");
        assert_eq!(finder_index_rebuilds(root), 2);
    }

    #[test]
    fn find_package_fuzzy_prefers_exact_match() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            r"{ pkgs }:
[
  lua
  lua5_4
]
",
        );

        let found = find_package_fuzzy("lua", root)
            .expect("finder should return a successful search result")
            .expect("fuzzy finder should return an exact match");
        assert_eq!(found.name, "lua");
        assert_eq!(found.location.line(), Some(3));
    }

    #[test]
    fn find_package_fuzzy_prefers_prefix_before_substring() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            r"{ pkgs, ... }:
{
  home.packages = with pkgs; [
    stylua
    lua5_4
  ];
}
",
        );

        let found = find_package_fuzzy("lua", root)
            .expect("finder should return a successful search result")
            .expect("fuzzy finder should return a prefix match");
        assert_eq!(found.name, "lua5_4");
        assert_eq!(found.location.line(), Some(5));
    }

    #[test]
    fn find_package_fuzzy_returns_matched_name_and_location() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            r"{ pkgs }:
[
  ripgrep
]
",
        );

        let found = find_package_fuzzy("rg", root)
            .expect("finder should return a successful search result")
            .expect("fuzzy finder should resolve alias to canonical package");
        assert_eq!(found.name, "ripgrep");
        assert_eq!(found.location.line(), Some(3));
    }
}
