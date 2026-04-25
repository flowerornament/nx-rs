use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[derive(Clone, Copy)]
enum CleanMethod {
    /// Remove entire directory contents.
    RemoveContents,
    /// Run external tool command.
    Command(&'static str, &'static [&'static str]),
    /// Remove only Rust `target/` directories under a code root.
    RustTargets,
    /// Remove only Elixir `_build/` directories under a code root.
    ElixirBuilds,
    /// Remove only `node_modules/` directories under a code root.
    NodeModules,
}

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

pub fn cmd_clean_caches(args: &CleanCachesArgs, ctx: &HostContext<'_>) -> i32 {
    if args.dry_run {
        ctx.printer.dry_run_banner();
    }

    ctx.printer.action("Scanning cache directories");
    let home = dirs_home();
    let entries = scan_caches(&home);

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

fn scan_caches(home: &Path) -> Vec<CacheEntry> {
    let mut entries = static_cache_entries(home);
    entries.extend(code_cache_entries(&home.join("code")));

    entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    entries
}

fn static_cache_entries(home: &Path) -> Vec<CacheEntry> {
    CACHE_CANDIDATES
        .iter()
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
                clean: candidate.clean,
            }
        })
        .collect()
}

fn code_cache_entries(code_dir: &Path) -> Vec<CacheEntry> {
    if !code_dir.is_dir() {
        return Vec::new();
    }

    let mut sizes = CodeCacheSizes::default();
    walk_code_cache_dirs(code_dir, 3, 0, &mut sizes);
    sizes.entries(code_dir)
}

#[derive(Default)]
struct CodeCacheSizes {
    rust_targets: u64,
    elixir_builds: u64,
    node_modules: u64,
}

impl CodeCacheSizes {
    fn add(&mut self, dirname: &str, path: &Path) -> bool {
        match dirname {
            "target" => self.rust_targets += dir_size(path),
            "_build" => self.elixir_builds += dir_size(path),
            "node_modules" => self.node_modules += dir_size(path),
            _ => return false,
        }
        true
    }

    fn entries(&self, code_dir: &Path) -> Vec<CacheEntry> {
        [
            ("rust-targets", self.rust_targets, CleanMethod::RustTargets),
            (
                "elixir-builds",
                self.elixir_builds,
                CleanMethod::ElixirBuilds,
            ),
            ("node-modules", self.node_modules, CleanMethod::NodeModules),
        ]
        .into_iter()
        .filter(|&(_, size_bytes, _)| size_bytes > 0)
        .map(|(name, size_bytes, clean)| CacheEntry {
            name,
            path: code_dir.to_path_buf(),
            size_bytes,
            clean,
        })
        .collect()
    }
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
        CleanMethod::RustTargets => remove_named_subdirs(&entry.path, "target", 3),
        CleanMethod::ElixirBuilds => remove_named_subdirs(&entry.path, "_build", 3),
        CleanMethod::NodeModules => remove_named_subdirs(&entry.path, "node_modules", 3),
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

fn walk_code_cache_dirs(dir: &Path, max_depth: u32, depth: u32, sizes: &mut CodeCacheSizes) {
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
        if sizes.add(&name, &path) {
            continue;
        }

        if !name.starts_with('.') {
            walk_code_cache_dirs(&path, max_depth, depth + 1, sizes);
        }
    }
}

fn find_named_dirs(root: &Path, dirname: &str, max_depth: u32) -> Vec<PathBuf> {
    let mut results = Vec::new();
    walk_for_dirs(root, dirname, max_depth, 0, &mut results);
    results
}

fn walk_for_dirs(dir: &Path, target: &str, max_depth: u32, depth: u32, out: &mut Vec<PathBuf>) {
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
        if entry.file_name() == target {
            out.push(path);
            // Don't recurse into target dirs.
        } else if !entry.file_name().to_string_lossy().starts_with('.') {
            walk_for_dirs(&path, target, max_depth, depth + 1, out);
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

fn remove_named_subdirs(root: &Path, dirname: &str, max_depth: u32) -> Result<(), String> {
    let dirs = find_named_dirs(root, dirname, max_depth);
    for d in &dirs {
        fs::remove_dir_all(d).map_err(|e| format!("rm {}: {e}", d.display()))?;
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
