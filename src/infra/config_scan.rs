use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Context;
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

pub fn scan_packages(repo_root: &Path) -> anyhow::Result<PackageBuckets> {
    let mut out = PackageBuckets::default();
    let mut seen = SourceSeen::default();

    for nix_file in collect_nix_files(repo_root) {
        let content = fs::read_to_string(&nix_file)
            .with_context(|| format!("reading {}", nix_file.display()))?;
        collect_nixpkgs_packages(&content, &mut out, &mut seen);
        collect_homebrew_items(
            &nix_file,
            &content,
            "brews.nix",
            brews_regex(),
            &mut out.brews,
            &mut seen.brews,
        );
        collect_homebrew_items(
            &nix_file,
            &content,
            "casks.nix",
            casks_regex(),
            &mut out.casks,
            &mut seen.casks,
        );
        collect_mas_apps(&content, &mut out, &mut seen);
        collect_launchd_services(&content, &mut out, &mut seen);
    }

    Ok(out)
}

/// Collect `.nix` files for package/service scanning.
///
/// Skips only `common.nix`. Unlike `ConfigFiles::discover`, this intentionally
/// includes `default.nix` because it may contain launchd service definitions.
pub fn collect_nix_files(repo_root: &Path) -> Vec<PathBuf> {
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
            if file_name == "common.nix" {
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out
}

#[derive(Default)]
struct SourceSeen {
    nxs: HashSet<String>,
    brews: HashSet<String>,
    casks: HashSet<String>,
    mas: HashSet<String>,
    services: HashSet<String>,
}

fn collect_nixpkgs_packages(content: &str, out: &mut PackageBuckets, seen: &mut SourceSeen) {
    for captures in list_assign_regexes()[0].captures_iter(content) {
        collect_list_items(&captures[1], &mut out.nxs, &mut seen.nxs);
    }
    for captures in list_assign_regexes()[1].captures_iter(content) {
        collect_list_items(&captures[1], &mut out.nxs, &mut seen.nxs);
    }
}

fn collect_homebrew_items(
    nix_file: &Path,
    content: &str,
    file_name: &str,
    regex: &Regex,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    if nix_file.file_name().and_then(|name| name.to_str()) == Some(file_name)
        && nix_file
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("homebrew")
    {
        for item in quoted_item_regex().captures_iter(content) {
            push_unique(item[1].to_string(), out, seen);
        }
        return;
    }

    for captures in regex.captures_iter(content) {
        for item in quoted_item_regex().captures_iter(&captures[1]) {
            push_unique(item[1].to_string(), out, seen);
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
    for token in block.lines().filter_map(extract_package_name) {
        push_unique(token, out, seen);
    }
}

fn extract_package_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || matches!(trimmed, "[" | "]" | "{")
        || trimmed.starts_with("inputs.")
        || trimmed.starts_with("++")
    {
        return None;
    }
    let captures = nix_ident_regex().captures(trimmed)?;
    let token = captures[1].to_string();
    if nix_keywords().contains(token.as_str()) {
        return None;
    }
    Some(token)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scan_packages_includes_launchd_service_from_default_nix() {
        let temp = TempDir::new().expect("temp dir should be created");
        let home_darwin = temp.path().join("home/darwin");
        fs::create_dir_all(&home_darwin).expect("home/darwin should exist");
        fs::write(
            home_darwin.join("default.nix"),
            r#"{ lib, ... }:
{
  launchd.agents.sops-nix.config.EnvironmentVariables.PATH =
    lib.mkForce "/usr/bin:/bin:/usr/sbin:/sbin";
}
"#,
        )
        .expect("default.nix should be written");

        let buckets = scan_packages(temp.path()).expect("scan should succeed");
        assert!(buckets.services.contains(&"sops-nix".to_string()));
    }

    #[test]
    fn scan_packages_excludes_launchd_service_from_common_nix() {
        let temp = TempDir::new().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        fs::create_dir_all(&home_dir).expect("home should exist");
        fs::write(
            home_dir.join("common.nix"),
            r#"{ ... }:
{
  launchd.agents.ignored-common.config.EnvironmentVariables.PATH = "/usr/bin";
}
"#,
        )
        .expect("common.nix should be written");

        let buckets = scan_packages(temp.path()).expect("scan should succeed");
        assert!(!buckets.services.contains(&"ignored-common".to_string()));
    }
}
