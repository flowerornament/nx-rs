use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use walkdir::WalkDir;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PackageBuckets {
    pub nxs: Vec<String>,
    pub brews: Vec<String>,
    pub casks: Vec<String>,
    pub mas: Vec<String>,
    pub services: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackageMatch {
    pub name: String,
    pub location: String,
}

pub fn scan_packages(repo_root: &Path) -> io::Result<PackageBuckets> {
    let mut out = PackageBuckets::default();
    let mut seen = SourceSeen::default();

    for nix_file in collect_nix_files(repo_root) {
        let content = fs::read_to_string(&nix_file)?;
        collect_nixpkgs_packages(&content, &mut out, &mut seen);
        collect_homebrew_brews(&nix_file, &content, &mut out, &mut seen);
        collect_homebrew_casks(&nix_file, &content, &mut out, &mut seen);
        collect_mas_apps(&content, &mut out, &mut seen);
        collect_launchd_services(&content, &mut out, &mut seen);
    }

    Ok(out)
}

pub fn find_package(name: &str, repo_root: &Path) -> io::Result<Option<String>> {
    let mapped = map_name(name);
    let mapped_location = find_package_exact(&mapped, repo_root)?;
    if mapped_location.is_some() {
        return Ok(mapped_location);
    }
    if mapped.eq_ignore_ascii_case(name) {
        return Ok(None);
    }
    find_package_exact(name, repo_root)
}

pub fn find_package_fuzzy(name: &str, repo_root: &Path) -> io::Result<Option<PackageMatch>> {
    if let Some(location) = find_package(name, repo_root)? {
        return Ok(Some(PackageMatch {
            name: map_name(name),
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

fn find_package_exact(name: &str, repo_root: &Path) -> io::Result<Option<String>> {
    let escaped = regex::escape(name);
    let patterns = build_patterns(&escaped)?;

    for file_path in collect_nix_files(repo_root) {
        let content = fs::read_to_string(&file_path)?;
        for (line_index, line) in content.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if is_alias_rhs_for(line, name) {
                continue;
            }
            if patterns.iter().any(|pattern| pattern.is_match(line)) {
                let output_path = fs::canonicalize(&file_path).unwrap_or(file_path.clone());
                let location = format!("{}:{}", output_path.display(), line_index + 1);
                return Ok(Some(location));
            }
        }
    }

    Ok(None)
}

fn build_patterns(escaped_name: &str) -> io::Result<Vec<Regex>> {
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
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
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

fn all_packages(buckets: &PackageBuckets) -> Vec<String> {
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

fn map_name(name: &str) -> String {
    static NAME_MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let map = NAME_MAP.get_or_init(|| {
        HashMap::from([
            ("nvim", "neovim"),
            ("vim", "neovim"),
            ("python", "python3"),
            ("rg", "ripgrep"),
        ])
    });

    let lower = name.to_ascii_lowercase();
    map.get(lower.as_str()).copied().unwrap_or(name).to_string()
}

#[derive(Default)]
struct SourceSeen {
    nxs: HashSet<String>,
    brews: HashSet<String>,
    casks: HashSet<String>,
    mas: HashSet<String>,
    services: HashSet<String>,
}

fn collect_nix_files(repo_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir_name in ["home", "system", "hosts", "packages"] {
        let dir_path = repo_root.join(dir_name);
        if !dir_path.exists() {
            continue;
        }

        for entry in WalkDir::new(&dir_path).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("nix") {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if file_name == "default.nix" || file_name == "common.nix" {
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out
}

fn collect_nixpkgs_packages(content: &str, out: &mut PackageBuckets, seen: &mut SourceSeen) {
    for captures in list_assign_regexes()[0].captures_iter(content) {
        collect_list_items(&captures[1], &mut out.nxs, &mut seen.nxs);
    }
    for captures in list_assign_regexes()[1].captures_iter(content) {
        collect_list_items(&captures[1], &mut out.nxs, &mut seen.nxs);
    }
}

fn collect_homebrew_brews(
    nix_file: &Path,
    content: &str,
    out: &mut PackageBuckets,
    seen: &mut SourceSeen,
) {
    if nix_file.file_name().and_then(|name| name.to_str()) == Some("brews.nix")
        && nix_file
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("homebrew")
    {
        for item in quoted_item_regex().captures_iter(content) {
            push_unique(item[1].to_string(), &mut out.brews, &mut seen.brews);
        }
        return;
    }

    for captures in brews_regex().captures_iter(content) {
        for item in quoted_item_regex().captures_iter(&captures[1]) {
            push_unique(item[1].to_string(), &mut out.brews, &mut seen.brews);
        }
    }
}

fn collect_homebrew_casks(
    nix_file: &Path,
    content: &str,
    out: &mut PackageBuckets,
    seen: &mut SourceSeen,
) {
    if nix_file.file_name().and_then(|name| name.to_str()) == Some("casks.nix")
        && nix_file
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("homebrew")
    {
        for item in quoted_item_regex().captures_iter(content) {
            push_unique(item[1].to_string(), &mut out.casks, &mut seen.casks);
        }
        return;
    }

    for captures in casks_regex().captures_iter(content) {
        for item in quoted_item_regex().captures_iter(&captures[1]) {
            push_unique(item[1].to_string(), &mut out.casks, &mut seen.casks);
        }
    }
}

fn collect_mas_apps(content: &str, out: &mut PackageBuckets, seen: &mut SourceSeen) {
    for captures in mas_regex().captures_iter(content) {
        for item in quoted_item_regex().captures_iter(&captures[1]) {
            push_unique(item[1].to_string(), &mut out.mas, &mut seen.mas);
        }
    }
}

fn collect_launchd_services(content: &str, out: &mut PackageBuckets, seen: &mut SourceSeen) {
    for captures in launchd_regex().captures_iter(content) {
        push_unique(
            captures[1].to_string(),
            &mut out.services,
            &mut seen.services,
        );
    }
}

fn collect_list_items(block: &str, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    for raw_line in block.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line == "[" || line == "]" || line == "{" {
            continue;
        }
        if line.starts_with("inputs.") || line.starts_with("++") {
            continue;
        }
        if let Some(captures) = nix_ident_regex().captures(line) {
            let token = captures[1].to_string();
            if !nix_keywords().contains(token.as_str()) {
                push_unique(token, out, seen);
            }
        }
    }
}

fn push_unique(item: String, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    if seen.insert(item.clone()) {
        out.push(item);
    }
}

fn list_assign_regexes() -> &'static [Regex; 2] {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    RE.get_or_init(|| {
        [
            Regex::new(r"home\.packages\s*=\s*(?:with\s+\w+;\s*)?\[(?s)(.*?)\];")
                .expect("valid regex"),
            Regex::new(r"environment\.systemPackages\s*=\s*(?:with\s+\w+;\s*)?\[(?s)(.*?)\];")
                .expect("valid regex"),
        ]
    })
}

fn brews_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:homebrew\.)?brews\s*=\s*\[(?s)(.*?)\];").expect("valid regex")
    })
}

fn casks_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:homebrew\.)?casks\s*=\s*\[(?s)(.*?)\];").expect("valid regex")
    })
}

fn mas_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:homebrew\.)?masApps\s*=\s*\{(?s)(.*?)\};").expect("valid regex")
    })
}

fn launchd_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"launchd\.(?:user\.)?agents\.([a-zA-Z0-9_-]+)").expect("valid regex")
    })
}

fn quoted_item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""([^"]+)""#).expect("valid regex"))
}

fn nix_ident_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([a-zA-Z][a-zA-Z0-9_.-]*)").expect("valid regex"))
}

fn nix_keywords() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        HashSet::from([
            "with", "pkgs", "lib", "config", "in", "let", "inherit", "rec",
        ])
    })
}
