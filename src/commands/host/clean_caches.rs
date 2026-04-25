use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use crate::app::dirs_home;
use crate::cli::CleanCachesArgs;
use crate::commands::context::HostContext;
use crate::output::printer::Printer;

// ─── clean-caches ───────────────────────────────────────────────────────────

struct CacheEntry {
    name: &'static str,
    path: PathBuf,
    size_bytes: u64,
    clean: CleanMethod,
}

struct CacheCandidate {
    name: &'static str,
    location: CacheLocation,
    clean: CleanMethod,
}

#[derive(Clone, Copy)]
enum CacheLocation {
    HomeRelative(&'static str),
    NixStoreGc,
}

#[derive(Clone)]
enum CleanMethod {
    /// Remove entire directory contents.
    RemoveContents,
    /// Run external tool command.
    Command(&'static str, &'static [&'static str]),
    /// Remove already discovered code cache directories.
    RemovePaths(Vec<PathBuf>),
}

struct CleanCachesConfig {
    code_roots: Vec<PathBuf>,
    scan_depth: u32,
    skip: Vec<String>,
}

struct CodeCacheTarget {
    name: &'static str,
    dirname: &'static str,
}

enum CodeCacheMatch {
    NotMatched,
    Skipped,
    Recorded,
}

const CODE_ROOTS_ENV: &str = "NX_CODE_ROOTS";
const CLEAN_SCAN_DEPTH_ENV: &str = "NX_CLEAN_SCAN_DEPTH";
const CLEAN_SKIP_ENV: &str = "NX_CLEAN_SKIP";
const DEFAULT_SCAN_DEPTH: u32 = 3;
const MAX_SCAN_DEPTH: u32 = 8;

const CACHE_CANDIDATES: &[CacheCandidate] = &[
    cache_dir("cargo-registry", ".cargo/registry"),
    cache_command("uv", ".cache/uv", "uv", &["cache", "clean"]),
    cache_dir("npm", ".cache/npm"),
    cache_command(
        "homebrew",
        "Library/Caches/Homebrew",
        "brew",
        &["cleanup", "--prune=0"],
    ),
    cache_dir("huggingface", ".cache/huggingface"),
    cache_dir("puppeteer", ".cache/puppeteer"),
    cache_dir("playwright", "Library/Caches/ms-playwright"),
    cache_dir("xcode-derived", "Library/Developer/Xcode/DerivedData"),
    cache_dir("core-simulator", "Library/Developer/CoreSimulator"),
    cache_dir("codex-sessions", ".codex/sessions"),
    cache_dir("codex-logs", ".codex/log"),
    cache_dir("claude-telemetry", ".claude/telemetry"),
    cache_dir("claude-file-history", ".claude/file-history"),
    CacheCandidate {
        name: "nix-gc",
        location: CacheLocation::NixStoreGc,
        clean: CleanMethod::Command("nix-collect-garbage", &[]),
    },
];

const CODE_CACHE_TARGETS: &[CodeCacheTarget] = &[
    CodeCacheTarget {
        name: "rust-targets",
        dirname: "target",
    },
    CodeCacheTarget {
        name: "elixir-builds",
        dirname: "_build",
    },
    CodeCacheTarget {
        name: "node-modules",
        dirname: "node_modules",
    },
];

const fn cache_dir(name: &'static str, rel_path: &'static str) -> CacheCandidate {
    CacheCandidate {
        name,
        location: CacheLocation::HomeRelative(rel_path),
        clean: CleanMethod::RemoveContents,
    }
}

const fn cache_command(
    name: &'static str,
    rel_path: &'static str,
    program: &'static str,
    args: &'static [&'static str],
) -> CacheCandidate {
    CacheCandidate {
        name,
        location: CacheLocation::HomeRelative(rel_path),
        clean: CleanMethod::Command(program, args),
    }
}

impl CleanCachesConfig {
    fn from_env(home: &Path) -> Self {
        Self {
            code_roots: parse_code_roots(home, env::var(CODE_ROOTS_ENV).ok().as_deref()),
            scan_depth: parse_scan_depth(env::var(CLEAN_SCAN_DEPTH_ENV).ok().as_deref()),
            skip: parse_skip_names(env::var(CLEAN_SKIP_ENV).ok().as_deref()),
        }
    }

    fn skips(&self, name: &str) -> bool {
        self.skip.iter().any(|skip| skip == name)
    }

    fn unknown_skip_names(&self) -> Vec<&str> {
        self.skip
            .iter()
            .map(String::as_str)
            .filter(|name| !clean_cache_skip_names().any(|valid| valid == *name))
            .collect()
    }
}

pub fn cmd_clean_caches(args: &CleanCachesArgs, ctx: &HostContext<'_>) -> i32 {
    if args.dry_run {
        ctx.printer.dry_run_banner();
    }

    ctx.printer.action("Scanning cache directories");
    let home = dirs_home();
    let config = CleanCachesConfig::from_env(&home);
    for name in config.unknown_skip_names() {
        ctx.printer
            .warn(&format!("unknown {CLEAN_SKIP_ENV} entry: {name}"));
    }
    let entries = scan_caches(&home, &config);

    if entries.is_empty() {
        Printer::body("No caches found.");
        return 0;
    }

    let total_bytes: u64 = entries.iter().map(|e| e.size_bytes).sum();

    println!();
    Printer::heading(&format!(
        "Cache Directories  ({})",
        format_size(total_bytes)
    ));
    println!();

    for entry in &entries {
        if entry.size_bytes == 0 {
            continue;
        }
        ctx.printer.action(&format!(
            "{:<24} {:>8}",
            entry.name,
            format_size(entry.size_bytes)
        ));
        Printer::sub_detail(&entry.path.display().to_string());
    }

    let nonzero: Vec<&CacheEntry> = entries.iter().filter(|e| e.size_bytes > 0).collect();

    if nonzero.is_empty() {
        println!();
        Printer::body("All caches are empty.");
        return 0;
    }

    if args.dry_run {
        println!();
        Printer::body("Dry run — no changes made.");
        return 0;
    }

    if !args.yes {
        println!();
        if !Printer::confirm("Clean all listed caches?", false) {
            Printer::body("Cancelled.");
            return 0;
        }
    }

    println!();
    let mut cleaned_bytes: u64 = 0;
    let mut cleaned_count = 0;

    for entry in &nonzero {
        match clean_entry(entry) {
            Ok(()) => {
                ctx.printer.success(&format!(
                    "{} ({})",
                    entry.name,
                    format_size(entry.size_bytes)
                ));
                cleaned_bytes += entry.size_bytes;
                cleaned_count += 1;
            }
            Err(err) => {
                ctx.printer.warn(&format!("{}: {err}", entry.name));
            }
        }
    }

    println!();
    ctx.printer.success(&format!(
        "Cleaned {cleaned_count} caches, freed {}",
        format_size(cleaned_bytes)
    ));
    0
}

fn scan_caches(home: &Path, config: &CleanCachesConfig) -> Vec<CacheEntry> {
    let mut entries = static_cache_entries(home, config);
    for code_root in &config.code_roots {
        entries.extend(code_cache_entries(code_root, config));
    }

    entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    entries
}

fn static_cache_entries(home: &Path, config: &CleanCachesConfig) -> Vec<CacheEntry> {
    CACHE_CANDIDATES
        .iter()
        .filter(|candidate| !config.skips(candidate.name))
        .map(|candidate| {
            let path = match candidate.location {
                CacheLocation::HomeRelative(rel_path) => home.join(rel_path),
                CacheLocation::NixStoreGc => PathBuf::from("/nix/store"),
            };
            let size_bytes = match candidate.location {
                CacheLocation::HomeRelative(_) => dir_size(&path),
                CacheLocation::NixStoreGc => nix_dead_size(),
            };

            CacheEntry {
                name: candidate.name,
                path,
                size_bytes,
                clean: candidate.clean.clone(),
            }
        })
        .collect()
}

fn code_cache_entries(code_dir: &Path, config: &CleanCachesConfig) -> Vec<CacheEntry> {
    if !code_dir.is_dir()
        || CODE_CACHE_TARGETS
            .iter()
            .all(|target| config.skips(target.name))
    {
        return Vec::new();
    }

    let mut sizes = CodeCacheSizes::default();
    walk_code_cache_dirs(code_dir, config.scan_depth, 0, config, &mut sizes);
    sizes.entries(code_dir)
}

#[derive(Default)]
struct CodeCacheSizes {
    entries: Vec<CodeCacheSize>,
}

struct CodeCacheSize {
    target: &'static CodeCacheTarget,
    size_bytes: u64,
    paths: Vec<PathBuf>,
}

impl CodeCacheSizes {
    fn record_target(
        &mut self,
        dirname: &str,
        path: &Path,
        config: &CleanCachesConfig,
    ) -> CodeCacheMatch {
        let Some(target) = CODE_CACHE_TARGETS
            .iter()
            .find(|target| target.dirname == dirname)
        else {
            return CodeCacheMatch::NotMatched;
        };

        if config.skips(target.name) {
            return CodeCacheMatch::Skipped;
        }

        let size_bytes = dir_size(path);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.target.name == target.name)
        {
            entry.size_bytes += size_bytes;
            entry.paths.push(path.to_path_buf());
        } else {
            self.entries.push(CodeCacheSize {
                target,
                size_bytes,
                paths: vec![path.to_path_buf()],
            });
        }

        CodeCacheMatch::Recorded
    }

    fn entries(&self, code_dir: &Path) -> Vec<CacheEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.size_bytes > 0)
            .map(|entry| CacheEntry {
                name: entry.target.name,
                path: code_dir.to_path_buf(),
                size_bytes: entry.size_bytes,
                clean: CleanMethod::RemovePaths(entry.paths.clone()),
            })
            .collect()
    }
}

fn parse_code_roots(home: &Path, value: Option<&str>) -> Vec<PathBuf> {
    let roots = match value {
        None => vec![home.join("code")],
        Some(raw) => raw
            .split(':')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect(),
    };
    prune_code_roots(roots)
}

fn parse_scan_depth(value: Option<&str>) -> u32 {
    value
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .map_or(DEFAULT_SCAN_DEPTH, |depth| depth.min(MAX_SCAN_DEPTH))
}

fn parse_skip_names(value: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    for name in value
        .into_iter()
        .flat_map(|raw| raw.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_string());
        }
    }
    names
}

pub(crate) fn clean_cache_skip_names() -> impl Iterator<Item = &'static str> {
    CACHE_CANDIDATES
        .iter()
        .map(|candidate| candidate.name)
        .chain(CODE_CACHE_TARGETS.iter().map(|target| target.name))
}

fn prune_code_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.into_iter().map(|root| normalize_path(&root)).fold(
        Vec::<PathBuf>::new(),
        |mut kept, root| {
            if kept
                .iter()
                .any(|existing| root == *existing || root.starts_with(existing))
            {
                return kept;
            }

            kept.retain(|existing| !existing.starts_with(&root));
            kept.push(root);
            kept
        },
    )
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn clean_entry(entry: &CacheEntry) -> Result<(), String> {
    match &entry.clean {
        CleanMethod::RemoveContents => remove_dir_contents(&entry.path),
        CleanMethod::Command(prog, args) => {
            let status = Command::new(prog)
                .args(args.iter())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|e| format!("{prog}: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("{prog} exited {}", status.code().unwrap_or(-1)))
            }
        }
        CleanMethod::RemovePaths(paths) => remove_paths(paths),
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let output = Command::new("du")
        .args(["-sk", &path.to_string_lossy()])
        .output();
    match output {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            s.split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0)
                * 1024
        }
        Err(_) => 0,
    }
}

fn nix_dead_size() -> u64 {
    let output = Command::new("nix-store")
        .args(["--gc", "--print-dead"])
        .output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut total = 0u64;
            for p in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let sz = Command::new("nix-store").args(["-q", "--size", p]).output();
                if let Ok(sz_out) = sz
                    && let Ok(n) = String::from_utf8_lossy(&sz_out.stdout)
                        .trim()
                        .parse::<u64>()
                {
                    total += n;
                }
            }
            total
        }
        Err(_) => 0,
    }
}

fn walk_code_cache_dirs(
    dir: &Path,
    max_depth: u32,
    depth: u32,
    config: &CleanCachesConfig,
    sizes: &mut CodeCacheSizes,
) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name = name.to_string_lossy();
        match sizes.record_target(&name, &path, config) {
            CodeCacheMatch::NotMatched if !name.starts_with('.') => {
                walk_code_cache_dirs(&path, max_depth, depth + 1, config, sizes);
            }
            CodeCacheMatch::Recorded | CodeCacheMatch::Skipped | CodeCacheMatch::NotMatched => {}
        }
    }
}

fn remove_dir_contents(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            fs::remove_dir_all(&p).map_err(|e| format!("rm {}: {e}", p.display()))?;
        } else {
            fs::remove_file(&p).map_err(|e| format!("rm {}: {e}", p.display()))?;
        }
    }
    Ok(())
}

fn remove_paths(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        if !path.exists() {
            continue;
        }
        fs::remove_dir_all(path).map_err(|e| format!("rm {}: {e}", path.display()))?;
    }
    Ok(())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        let tenths = (u128::from(bytes) * 10) / u128::from(GB);
        format!("{}.{}G", tenths / 10, tenths % 10)
    } else if bytes >= MB {
        format!("{}M", bytes / MB)
    } else if bytes >= KB {
        format!("{}K", bytes / KB)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_roots_missing_uses_home_code() {
        let home = Path::new("/Users/tester");

        let roots = parse_code_roots(home, None);

        assert_eq!(roots, vec![PathBuf::from("/Users/tester/code")]);
    }

    #[test]
    fn code_roots_empty_disables_code_scan() {
        let roots = parse_code_roots(Path::new("/Users/tester"), Some(""));

        assert!(roots.is_empty());
    }

    #[test]
    fn code_roots_parse_colon_list() {
        let roots = parse_code_roots(Path::new("/Users/tester"), Some("/a/code:/b/code:: "));

        assert_eq!(
            roots,
            vec![PathBuf::from("/a/code"), PathBuf::from("/b/code")]
        );
    }

    #[test]
    fn code_roots_drop_duplicates_and_children() {
        let roots = parse_code_roots(
            Path::new("/Users/tester"),
            Some("/a/code/project:/a/code:/a/code:/b/./code"),
        );

        assert_eq!(
            roots,
            vec![PathBuf::from("/a/code"), PathBuf::from("/b/code")]
        );
    }

    #[test]
    fn scan_depth_defaults_on_missing_or_invalid() {
        assert_eq!(parse_scan_depth(None), DEFAULT_SCAN_DEPTH);
        assert_eq!(parse_scan_depth(Some("not-a-number")), DEFAULT_SCAN_DEPTH);
        assert_eq!(parse_scan_depth(Some("5")), 5);
        assert_eq!(parse_scan_depth(Some("999")), MAX_SCAN_DEPTH);
    }

    #[test]
    fn skip_names_trim_and_deduplicate() {
        assert_eq!(
            parse_skip_names(Some("huggingface, nix-gc, huggingface,,")),
            vec!["huggingface".to_string(), "nix-gc".to_string()]
        );
    }

    #[test]
    fn skip_filter_recognizes_static_and_code_cache_names() {
        let config = CleanCachesConfig {
            code_roots: Vec::new(),
            scan_depth: DEFAULT_SCAN_DEPTH,
            skip: vec!["huggingface".to_string(), "rust-targets".to_string()],
        };

        assert!(config.skips("huggingface"));
        assert!(config.skips("rust-targets"));
        assert!(!config.skips("node-modules"));
        assert!(config.unknown_skip_names().is_empty());
    }

    #[test]
    fn unknown_skip_names_are_reportable() {
        let config = CleanCachesConfig {
            code_roots: Vec::new(),
            scan_depth: DEFAULT_SCAN_DEPTH,
            skip: vec!["huggingface".to_string(), "mystery-cache".to_string()],
        };

        assert_eq!(config.unknown_skip_names(), vec!["mystery-cache"]);
    }

    #[test]
    fn docs_and_help_list_all_skip_names() {
        let readme = include_str!("../../../README.md");
        let cli = include_str!("../../cli.rs");

        for name in clean_cache_skip_names() {
            assert!(readme.contains(name), "README is missing {name}");
            assert!(cli.contains(name), "CLI help is missing {name}");
        }
    }
}
