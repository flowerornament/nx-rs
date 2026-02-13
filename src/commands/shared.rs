use std::fs;
use std::path::Path;

use crate::domain::location::PackageLocation;

pub fn relative_location(location: &PackageLocation, repo_root: &Path) -> String {
    let path_part = location.path().display().to_string();
    let raw_root = repo_root.display().to_string();
    let canonical_root = fs::canonicalize(repo_root)
        .ok()
        .map(|path| path.display().to_string());

    let mut rel = path_part;
    if let Some(root) = canonical_root {
        let prefix = format!("{root}/");
        rel = rel.strip_prefix(&prefix).unwrap_or(&rel).to_string();
    }
    let raw_prefix = format!("{raw_root}/");
    rel = rel.strip_prefix(&raw_prefix).unwrap_or(&rel).to_string();

    if let Some(line) = location.line() {
        format!("{rel}:{line}")
    } else {
        rel
    }
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

    let lines: Vec<&str> = content.split('\n').collect();
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
        match mode {
            SnippetMode::Add => {
                let marker = if number == line_num { '+' } else { ' ' };
                println!("  │ {marker} {number:4} │ {line}");
            }
            SnippetMode::Remove => {
                if number == line_num {
                    println!("  │ - {number:4} │ {line}");
                } else {
                    println!("  │   {number:4} │ {line}");
                }
            }
        }
    }
    println!("  └{}", "─".repeat(40));
}
