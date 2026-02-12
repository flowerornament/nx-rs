use std::fs;
use std::path::Path;

pub fn relative_location(location: &str, repo_root: &Path) -> String {
    let (path_part, suffix) = split_location(location);
    let raw_root = repo_root.display().to_string();
    let canonical_root = fs::canonicalize(repo_root)
        .ok()
        .map(|path| path.display().to_string());

    let mut rel = path_part.to_string();
    if let Some(root) = canonical_root {
        let prefix = format!("{root}/");
        rel = rel.strip_prefix(&prefix).unwrap_or(&rel).to_string();
    }
    let raw_prefix = format!("{raw_root}/");
    rel = rel.strip_prefix(&raw_prefix).unwrap_or(&rel).to_string();

    format!("{rel}{suffix}")
}

pub fn location_path_and_line(location: &str) -> (&str, Option<usize>) {
    match location.rsplit_once(':') {
        Some((path, line)) if line.chars().all(|ch| ch.is_ascii_digit()) => {
            (path, line.parse::<usize>().ok())
        }
        _ => (location, None),
    }
}

fn split_location(location: &str) -> (&str, &str) {
    match location.rsplit_once(':') {
        Some((path, line)) if line.chars().all(|ch| ch.is_ascii_digit()) => {
            (path, &location[path.len()..])
        }
        _ => (location, ""),
    }
}

#[derive(Clone, Copy)]
pub enum SnippetMode {
    Add,
    Remove,
}

pub fn show_snippet(
    file_path: &str,
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

    let path = Path::new(file_path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_path);
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
