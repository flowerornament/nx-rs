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

/// Render Python/Typer-compatible missing positional argument output.
pub fn missing_argument_error(command: &str, argument: &str) -> i32 {
    eprintln!("Usage: nx {command} [OPTIONS] {argument}");
    eprintln!("Try 'nx {command} --help' for help.");
    eprintln!();
    eprintln!("Error: Missing argument '{argument}'.");
    2
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

    let Ok(file_content) = fs::read_to_string(file_path) else {
        return;
    };

    let lines: Vec<&str> = file_content.lines().collect();
    let start = line_num.saturating_sub(context + 1);
    let end = usize::min(lines.len(), line_num + context);
    if start >= end {
        return;
    }

    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| file_path.display().to_string(), str::to_string);
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

pub fn show_dry_run_preview(
    file_path: &Path,
    insert_after_line: usize,
    simulated_line: &str,
    context: usize,
) {
    if insert_after_line == 0 {
        return;
    }

    let Ok(file_content) = fs::read_to_string(file_path) else {
        return;
    };
    let lines: Vec<&str> = file_content.lines().collect();
    if lines.is_empty() {
        return;
    }

    let start = insert_after_line.saturating_sub(context + 1);
    let end = usize::min(lines.len(), insert_after_line + context);
    if start >= end {
        return;
    }

    let inferred_indent = lines[start..end]
        .iter()
        .find_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            Some(
                line.chars()
                    .take_while(|c| c.is_whitespace())
                    .collect::<String>(),
            )
        })
        .unwrap_or_default();
    let simulated = format!("{}{}", inferred_indent, simulated_line.trim_start());

    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| file_path.display().to_string(), str::to_string);

    println!();
    println!("  ┌── {file_name} (preview) ───");
    for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
        println!("  │   {:4} │ {line}", idx + 1);
        if idx + 1 == insert_after_line {
            println!("  │ +      │ {simulated}");
        }
    }
    println!("  └{}", "─".repeat(40));
}

#[cfg(test)]
mod tests {
    use super::relative_location;
    use crate::domain::location::PackageLocation;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn relative_location_strips_repo_prefix_and_keeps_line_suffix() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let repo_root = tmp.path();
        let location = PackageLocation::new(repo_root.join("home/services.nix"), Some(42));

        let rendered = relative_location(&location, repo_root);
        assert_eq!(rendered, "home/services.nix:42");
    }

    #[test]
    fn relative_location_keeps_absolute_path_when_outside_repo() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let repo_root = tmp.path();
        let outside = PathBuf::from("/tmp/nx-rs-shared-test-outside.nix");
        let location = PackageLocation::new(outside.clone(), Some(7));

        let rendered = relative_location(&location, repo_root);
        assert_eq!(rendered, format!("{}:7", outside.display()));
    }
}
