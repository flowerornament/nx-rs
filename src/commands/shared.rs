use std::fs;
use std::path::Path;

use crate::domain::location::PackageLocation;

pub fn relative_location(location: &PackageLocation, repo_root: &Path) -> String {
    let full_path = location.path().display().to_string();
    let rel = strip_repo_prefix(&full_path, repo_root);

    if let Some(line) = location.line() {
        format!("{rel}:{line}")
    } else {
        rel
    }
}

fn strip_repo_prefix(path: &str, repo_root: &Path) -> String {
    let canonical = fs::canonicalize(repo_root).ok();
    let prefixes: Vec<String> = canonical
        .iter()
        .map(|p| format!("{}/", p.display()))
        .chain(std::iter::once(format!("{}/", repo_root.display())))
        .collect();

    for prefix in &prefixes {
        if let Some(stripped) = path.strip_prefix(prefix.as_str()) {
            return stripped.to_string();
        }
    }
    path.to_string()
}

#[derive(Clone, Copy)]
pub enum SnippetMode {
    Add,
    Remove,
}

pub fn show_snippet(
    file_path: &Path,
    line_num: usize,
    context: usize,
    mode: SnippetMode,
    preview: bool,
) {
    if line_num == 0 {
        return;
    }

    let Ok(content) = fs::read_to_string(file_path) else {
        return;
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = line_num.saturating_sub(context + 1);
    let end = usize::min(lines.len(), line_num + context);
    if start >= end {
        return;
    }

    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| file_path.display().to_string());
    let header_suffix = if preview { " (preview)" } else { "" };

    println!();
    println!("  ┌── {file_name}{header_suffix} ───");
    for (offset, line) in lines[start..end].iter().enumerate() {
        let number = start + offset + 1;
        let is_target = number == line_num;
        let marker = match (mode, is_target) {
            (SnippetMode::Add, true) => "+",
            (SnippetMode::Remove, true) => "-",
            _ => " ",
        };
        println!("  │ {marker} {number:4} │ {line}");
    }
    println!("  └{}", "─".repeat(40));
}
