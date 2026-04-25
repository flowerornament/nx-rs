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
    let code_dir = home.join("code");
    let mut entries = Vec::new();

    // Static cache directories.
    let candidates: &[(&str, &str, CleanMethod)] = &[
        (
            "cargo-registry",
            ".cargo/registry",
            CleanMethod::RemoveContents,
        ),
        (
            "uv",
            ".cache/uv",
            CleanMethod::Command("uv", &["cache", "clean"]),
        ),
        ("npm", ".cache/npm", CleanMethod::RemoveContents),
        (
            "homebrew",
            "Library/Caches/Homebrew",
            CleanMethod::Command("brew", &["cleanup", "--prune=0"]),
        ),
        (
            "huggingface",
            ".cache/huggingface",
            CleanMethod::RemoveContents,
        ),
        ("puppeteer", ".cache/puppeteer", CleanMethod::RemoveContents),
        (
            "playwright",
            "Library/Caches/ms-playwright",
            CleanMethod::RemoveContents,
        ),
        (
            "xcode-derived",
            "Library/Developer/Xcode/DerivedData",
            CleanMethod::RemoveContents,
        ),
        (
            "core-simulator",
            "Library/Developer/CoreSimulator",
            CleanMethod::RemoveContents,
        ),
        (
            "codex-sessions",
            ".codex/sessions",
            CleanMethod::RemoveContents,
        ),
        ("codex-logs", ".codex/log", CleanMethod::RemoveContents),
        (
            "claude-telemetry",
            ".claude/telemetry",
            CleanMethod::RemoveContents,
        ),
        (
            "claude-file-history",
            ".claude/file-history",
            CleanMethod::RemoveContents,
        ),
        (
            "nix-gc",
            "",
            CleanMethod::Command("nix-collect-garbage", &[]),
        ),
    ];

    for (name, rel_path, method) in candidates {
        let path = if rel_path.is_empty() {
            PathBuf::from("/nix/store")
        } else {
            home.join(rel_path)
        };

        // For nix-gc, estimate dead store paths.
        let size = if *name == "nix-gc" {
            nix_dead_size()
        } else {
            dir_size(&path)
        };

        entries.push(CacheEntry {
            name,
            path,
            size_bytes: size,
            clean: match method {
                CleanMethod::Command(prog, args) => CleanMethod::Command(prog, args),
                CleanMethod::RemoveContents => CleanMethod::RemoveContents,
                CleanMethod::RustTargets => CleanMethod::RustTargets,
                CleanMethod::ElixirBuilds => CleanMethod::ElixirBuilds,
                CleanMethod::NodeModules => CleanMethod::NodeModules,
            },
        });
    }

    // Aggregate build artifact directories under ~/code.
    if code_dir.is_dir() {
        let rust_size = find_dirs_size(&code_dir, "target", 3);
        if rust_size > 0 {
            entries.push(CacheEntry {
                name: "rust-targets",
                path: code_dir.clone(),
                size_bytes: rust_size,
                clean: CleanMethod::RustTargets,
            });
        }

        let elixir_size = find_dirs_size(&code_dir, "_build", 3);
        if elixir_size > 0 {
            entries.push(CacheEntry {
                name: "elixir-builds",
                path: code_dir.clone(),
                size_bytes: elixir_size,
                clean: CleanMethod::ElixirBuilds,
            });
        }

        let node_size = find_dirs_size(&code_dir, "node_modules", 3);
        if node_size > 0 {
            entries.push(CacheEntry {
                name: "node-modules",
                path: code_dir,
                size_bytes: node_size,
                clean: CleanMethod::NodeModules,
            });
        }
    }

    // Sort by size descending.
    entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    entries
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
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let paths: Vec<&str> = stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.trim())
                .collect();
            let mut total = 0u64;
            for p in &paths {
                let sz = Command::new("nix-store").args(["-q", "--size", p]).output();
                if let Ok(sz_out) = sz {
                    if let Ok(n) = String::from_utf8_lossy(&sz_out.stdout)
                        .trim()
                        .parse::<u64>()
                    {
                        total += n;
                    }
                }
            }
            total
        }
        Err(_) => 0,
    }
}

fn find_dirs_size(root: &Path, dirname: &str, max_depth: u32) -> u64 {
    find_named_dirs(root, dirname, max_depth)
        .iter()
        .map(|p| dir_size(p))
        .sum()
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
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
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
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{}M", bytes / MB)
    } else if bytes >= KB {
        format!("{}K", bytes / KB)
    } else {
        format!("{bytes}B")
    }
}
