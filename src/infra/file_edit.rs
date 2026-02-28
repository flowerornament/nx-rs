use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow};

use crate::domain::plan::{InsertionMode, InstallPlan};

// --- Types

/// Outcome of a file edit operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    pub file_changed: bool,
    pub line_number: Option<usize>,
}

// --- Public API

/// Apply the install plan's edit to the target file.
///
/// Dispatches to the per-mode inserter. Idempotent: returns
/// `file_changed: false` if the token is already present.
pub fn apply_edit(plan: &InstallPlan) -> Result<EditOutcome> {
    apply_plan(plan, dispatch_insert)
}

/// Apply a removal to the target file using the plan's insertion mode.
///
/// Dispatches to the per-mode remover. Idempotent: returns
/// `file_changed: false` if the token is not found.
pub fn apply_removal(plan: &InstallPlan) -> Result<EditOutcome> {
    apply_plan(plan, dispatch_remove)
}

/// Shared read-dispatch-write skeleton for both insertion and removal.
fn apply_plan(
    plan: &InstallPlan,
    transform: impl FnOnce(&str, &InstallPlan) -> Result<(String, Option<usize>)>,
) -> Result<EditOutcome> {
    let path = &plan.target_file;
    let content = read_file(path)?;
    let (new_content, line_number) = transform(&content, plan)?;

    if let Some(ln) = line_number {
        fs::write(path, &new_content)?;
        Ok(EditOutcome {
            file_changed: true,
            line_number: Some(ln),
        })
    } else {
        Ok(EditOutcome {
            file_changed: false,
            line_number: None,
        })
    }
}

fn dispatch_insert(content: &str, plan: &InstallPlan) -> Result<(String, Option<usize>)> {
    match plan.insertion_mode {
        InsertionMode::NixManifest => insert_nix_manifest(content, &plan.package_token),
        InsertionMode::LanguageWithPackages => {
            let lang = plan.language_info.as_ref().ok_or_else(|| {
                anyhow!("invalid install plan: language_info required for LanguageWithPackages")
            })?;
            insert_language_package(content, &lang.bare_name, &lang.runtime)
        }
        InsertionMode::HomebrewManifest => insert_homebrew_manifest(content, &plan.package_token),
        InsertionMode::MasApps => Ok(insert_mas_app(content, &plan.package_token)),
    }
}

fn dispatch_remove(content: &str, plan: &InstallPlan) -> Result<(String, Option<usize>)> {
    match plan.insertion_mode {
        InsertionMode::NixManifest => remove_nix_manifest(content, &plan.package_token),
        InsertionMode::LanguageWithPackages => {
            let lang = plan.language_info.as_ref().ok_or_else(|| {
                anyhow!("invalid install plan: language_info required for LanguageWithPackages")
            })?;
            remove_language_package(content, &lang.bare_name, &lang.runtime)
        }
        InsertionMode::HomebrewManifest => remove_homebrew_manifest(content, &plan.package_token),
        InsertionMode::MasApps => remove_mas_app(content, &plan.package_token),
    }
}

// --- Per-mode Inserters
//
// Each returns `(new_content, Some(line_number))` on insertion,
// or `(original_content, None)` when already present (idempotent).

/// Insert a bare identifier alphabetically into `home.packages = with pkgs; [ ... ]`.
///
/// Real format: 4-space indent, bare identifiers, optional `# comment` suffixes,
/// section headers (`# === ... ===`). Skips comment-only and blank lines
/// when finding alphabetical position.
fn insert_nix_manifest(content: &str, token: &str) -> Result<(String, Option<usize>)> {
    // Check idempotency: token already present as a standalone identifier
    if nix_manifest_contains(content, token) {
        return Ok((content.to_string(), None));
    }

    // Find the bracket region of `home.packages = with pkgs; [`
    let (bracket_start, bracket_end) = find_bracket_region(content, "home.packages")
        .or_else(|| find_bracket_region(content, "environment.systemPackages"))
        .ok_or_else(|| {
            anyhow::anyhow!("no home.packages or environment.systemPackages list found")
        })?;

    let lines: Vec<&str> = content.lines().collect();
    let indent = detect_indent_in_region(&lines, bracket_start, bracket_end).unwrap_or("    ");

    // Find alphabetical insertion point among package identifiers
    let insert_at = find_alpha_position(&lines, bracket_start + 1, bracket_end, token);
    let new_line = format!("{indent}{token}");

    // Build the final string with the inserted line
    let mut out = String::with_capacity(content.len() + new_line.len() + 1);
    for line in &lines[..insert_at] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&new_line);
    out.push('\n');
    for line in &lines[insert_at..] {
        out.push_str(line);
        out.push('\n');
    }

    // Preserve original trailing-newline behavior
    if !content.ends_with('\n') {
        out.pop();
    }

    Ok((out, Some(insert_at + 1))) // 1-indexed
}

/// Insert a bare name into the correct `runtime.withPackages (ps: ...)` block.
///
/// Real format: 6-space indent inside withPackages lists. Multiple runtime blocks
/// may exist in one file — must match the correct runtime.
fn insert_language_package(
    content: &str,
    bare_name: &str,
    runtime: &str,
) -> Result<(String, Option<usize>)> {
    // Check idempotency
    if lang_package_contains(content, bare_name, runtime) {
        return Ok((content.to_string(), None));
    }

    // Find the withPackages block for this runtime
    let lines: Vec<&str> = content.lines().collect();
    let (block_start, block_end) = find_with_packages_block(&lines, runtime)
        .ok_or_else(|| anyhow::anyhow!("no {runtime}.withPackages block found"))?;

    let indent = detect_indent_in_region(&lines, block_start, block_end).unwrap_or("      ");
    let insert_at = find_alpha_position(&lines, block_start + 1, block_end, bare_name);
    let new_line = format!("{indent}{bare_name}");

    let mut out = String::with_capacity(content.len() + new_line.len() + 1);
    for line in &lines[..insert_at] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&new_line);
    out.push('\n');
    for line in &lines[insert_at..] {
        out.push_str(line);
        out.push('\n');
    }

    if !content.ends_with('\n') {
        out.pop();
    }

    Ok((out, Some(insert_at + 1)))
}

/// Insert a double-quoted name alphabetically into a homebrew `[ "pkg" ... ]` list.
///
/// Real format: 2-space indent, double-quoted, optional `# comment` suffix.
fn insert_homebrew_manifest(content: &str, token: &str) -> Result<(String, Option<usize>)> {
    let quoted = format!("\"{token}\"");

    // Check idempotency
    if homebrew_manifest_contains(content, token) {
        return Ok((content.to_string(), None));
    }

    // Find the top-level bracket list
    let (bracket_start, bracket_end) = find_top_level_brackets(content)
        .ok_or_else(|| anyhow::anyhow!("no bracket list found in homebrew manifest"))?;

    let lines: Vec<&str> = content.lines().collect();
    let indent = detect_indent_in_region(&lines, bracket_start, bracket_end).unwrap_or("  ");

    // Find alphabetical position among quoted entries
    let insert_at = find_alpha_position_quoted(&lines, bracket_start + 1, bracket_end, token);
    let new_line = format!("{indent}{quoted}");

    let mut out = String::with_capacity(content.len() + new_line.len() + 1);
    for line in &lines[..insert_at] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&new_line);
    out.push('\n');
    for line in &lines[insert_at..] {
        out.push_str(line);
        out.push('\n');
    }

    if !content.ends_with('\n') {
        out.pop();
    }

    Ok((out, Some(insert_at + 1)))
}

/// Insert `"Name" = <id>;` into `masApps = { ... }`.
///
/// Uses a placeholder value (`0`) for new entries because App Store ID lookup
/// is outside deterministic editing scope.
fn insert_mas_app(content: &str, token: &str) -> (String, Option<usize>) {
    let lines: Vec<&str> = content.lines().collect();
    if let Some((block_start, block_end)) = find_mas_apps_block(&lines) {
        // Check idempotency within masApps block
        if find_quoted_line(&lines, block_start + 1, block_end, token).is_some() {
            return (content.to_string(), None);
        }

        let indent = detect_indent_in_region(&lines, block_start, block_end).unwrap_or("    ");
        let insert_at = find_alpha_position_quoted(&lines, block_start + 1, block_end, token);
        let new_line = format!("{indent}\"{token}\" = 0;");

        let mut out = String::with_capacity(content.len() + new_line.len() + 1);
        for line in &lines[..insert_at] {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&new_line);
        out.push('\n');
        for line in &lines[insert_at..] {
            out.push_str(line);
            out.push('\n');
        }

        if !content.ends_with('\n') {
            out.pop();
        }

        return (out, Some(insert_at + 1));
    }

    if let Some((homebrew_start, homebrew_end)) = find_attrset_block(&lines, "homebrew") {
        return insert_mas_block(
            content,
            &lines,
            token,
            homebrew_end,
            homebrew_start,
            homebrew_end,
            false,
        );
    }

    insert_mas_block(
        content,
        &lines,
        token,
        find_top_level_insert(&lines),
        0,
        0,
        true,
    )
}

fn insert_mas_block(
    content: &str,
    lines: &[&str],
    token: &str,
    insert_at: usize,
    indent_start: usize,
    block_end: usize,
    top_level: bool,
) -> (String, Option<usize>) {
    let (key_line, item_indent, line_number) = if top_level {
        let indent = detect_top_level_indent(lines).unwrap_or("  ");
        (
            format!("{indent}homebrew.masApps = {{"),
            format!("{indent}  "),
            insert_at + 2,
        )
    } else {
        let indent = detect_indent_in_region(lines, indent_start, block_end).unwrap_or("    ");
        (
            format!("{indent}masApps = {{"),
            format!("{indent}  "),
            insert_at + 2,
        )
    };

    let entry_line = format!("{item_indent}\"{token}\" = 0;");
    let close_indent = &item_indent[..item_indent.len().saturating_sub(2)];
    let close_line = format!("{close_indent}}};");
    let mut out = String::with_capacity(content.len() + key_line.len() + entry_line.len() + 8);
    for line in &lines[..insert_at] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&key_line);
    out.push('\n');
    out.push_str(&entry_line);
    out.push('\n');
    out.push_str(&close_line);
    out.push('\n');
    for line in &lines[insert_at..] {
        out.push_str(line);
        out.push('\n');
    }

    if !content.ends_with('\n') {
        out.pop();
    }

    (out, Some(line_number))
}

// --- Per-mode Removers
//
// Each returns `(new_content, Some(line_number))` on removal,
// or `(original_content, None)` when the token is absent (idempotent).

/// Remove a bare identifier from `home.packages = with pkgs; [ ... ]`.
fn remove_nix_manifest(content: &str, token: &str) -> Result<(String, Option<usize>)> {
    if !nix_manifest_contains(content, token) {
        return Ok((content.to_string(), None));
    }

    let (bracket_start, bracket_end) = find_bracket_region(content, "home.packages")
        .or_else(|| find_bracket_region(content, "environment.systemPackages"))
        .ok_or_else(|| {
            anyhow::anyhow!("no home.packages or environment.systemPackages list found")
        })?;

    let lines: Vec<&str> = content.lines().collect();
    let remove_idx = find_ident_line(&lines, bracket_start + 1, bracket_end, token);

    remove_idx.map_or_else(
        || Ok((content.to_string(), None)),
        |idx| Ok((splice_out_line(content, &lines, idx), Some(idx + 1))),
    )
}

/// Remove a bare name from the correct `runtime.withPackages (ps: ...)` block.
fn remove_language_package(
    content: &str,
    bare_name: &str,
    runtime: &str,
) -> Result<(String, Option<usize>)> {
    if !lang_package_contains(content, bare_name, runtime) {
        return Ok((content.to_string(), None));
    }

    let lines: Vec<&str> = content.lines().collect();
    let (block_start, block_end) = find_with_packages_block(&lines, runtime)
        .ok_or_else(|| anyhow::anyhow!("no {runtime}.withPackages block found"))?;

    let remove_idx = find_ident_line(&lines, block_start + 1, block_end, bare_name);

    remove_idx.map_or_else(
        || Ok((content.to_string(), None)),
        |idx| Ok((splice_out_line(content, &lines, idx), Some(idx + 1))),
    )
}

/// Remove a double-quoted name from a homebrew `[ "pkg" ... ]` list.
fn remove_homebrew_manifest(content: &str, token: &str) -> Result<(String, Option<usize>)> {
    if !homebrew_manifest_contains(content, token) {
        return Ok((content.to_string(), None));
    }

    let (bracket_start, bracket_end) = find_top_level_brackets(content)
        .ok_or_else(|| anyhow::anyhow!("no bracket list found in homebrew manifest"))?;

    let lines: Vec<&str> = content.lines().collect();
    let remove_idx = find_quoted_line(&lines, bracket_start + 1, bracket_end, token);

    remove_idx.map_or_else(
        || Ok((content.to_string(), None)),
        |idx| Ok((splice_out_line(content, &lines, idx), Some(idx + 1))),
    )
}

/// Remove a `"Name" = <id>;` entry from `masApps = { ... }`.
fn remove_mas_app(content: &str, token: &str) -> Result<(String, Option<usize>)> {
    let lines: Vec<&str> = content.lines().collect();
    let Some((block_start, block_end)) = find_mas_apps_block(&lines) else {
        return Ok((content.to_string(), None));
    };
    let remove_idx = find_quoted_line(&lines, block_start + 1, block_end, token);
    remove_idx.map_or_else(
        || Ok((content.to_string(), None)),
        |idx| Ok((splice_out_line(content, &lines, idx), Some(idx + 1))),
    )
}

// --- Helpers

fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))
}

/// Reconstruct file content with a single line removed.
///
/// Preserves the original trailing-newline behavior.
fn splice_out_line(content: &str, lines: &[&str], remove_idx: usize) -> String {
    let mut out = String::with_capacity(content.len());
    for (i, line) in lines.iter().enumerate() {
        if i == remove_idx {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !content.ends_with('\n') {
        out.pop();
    }
    out
}

/// Find the line index of a bare identifier within a region.
fn find_ident_line(lines: &[&str], start: usize, end: usize, token: &str) -> Option<usize> {
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        let ident = extract_bare_ident(line.trim());
        if ident == token {
            return Some(i);
        }
    }
    None
}

/// Find the line index of a double-quoted value within a region.
fn find_quoted_line(lines: &[&str], start: usize, end: usize, token: &str) -> Option<usize> {
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        if let Some(existing) = extract_quoted_value(line.trim())
            && existing == token
        {
            return Some(i);
        }
    }
    None
}

/// Check if a bare nix identifier already exists as a package entry.
fn nix_manifest_contains(content: &str, token: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        // Extract the identifier (first word before any comment)
        let ident = trimmed.split_whitespace().next().unwrap_or("");
        // Also handle `# comment` suffix: "ripgrep  # fast grep"
        let ident = ident.split('#').next().unwrap_or("").trim();
        if ident == token {
            return true;
        }
    }
    false
}

/// Check if a language package is already in the correct runtime's withPackages block.
fn lang_package_contains(content: &str, bare_name: &str, runtime: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    let Some((block_start, block_end)) = find_with_packages_block(&lines, runtime) else {
        return false;
    };
    for line in &lines[block_start..block_end] {
        let trimmed = line.trim();
        let ident = trimmed.split_whitespace().next().unwrap_or("");
        let ident = ident.split('#').next().unwrap_or("").trim();
        if ident == bare_name {
            return true;
        }
    }
    false
}

/// Check if a homebrew manifest already contains a quoted token.
fn homebrew_manifest_contains(content: &str, token: &str) -> bool {
    let quoted = format!("\"{token}\"");
    content.lines().any(|line| {
        let trimmed = line.trim();
        // Compare the quoted portion before any comment
        let before_comment = trimmed.split('#').next().unwrap_or("");
        before_comment.contains(&quoted)
    })
}

/// Find the line range (`bracket_open`, `bracket_close`) of a `[ ... ]` region after a key.
fn find_bracket_region(content: &str, key: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains(key) && line.contains('[') {
            // Find matching `];`
            for (j, close_line) in lines.iter().enumerate().skip(i) {
                if (close_line.trim_start().starts_with("];") || close_line.contains("];")) && j > i
                {
                    return Some((i, j));
                }
            }
        }
    }
    None
}

/// Find the top-level `[` ... `]` brackets in a homebrew manifest.
///
/// Homebrew manifests (brews.nix, casks.nix) are bare lists starting with `[`.
fn find_top_level_brackets(content: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') || trimmed.ends_with('[') {
            start = Some(i);
            break;
        }
    }
    let start = start?;
    for (j, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed == "]" || trimmed.starts_with(']') {
            return Some((start, j));
        }
    }
    None
}

/// Find the line range (`block_open`, `block_close`) of a `masApps = { ... };` region.
fn find_mas_apps_block(lines: &[&str]) -> Option<(usize, usize)> {
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("masApps") {
            continue;
        }

        let block_start = if line.contains('{') {
            i
        } else {
            lines
                .iter()
                .enumerate()
                .skip(i + 1)
                .find_map(|(j, candidate)| candidate.contains('{').then_some(j))?
        };

        for (j, close_line) in lines.iter().enumerate().skip(block_start + 1) {
            if close_line.trim().starts_with("};") || close_line.contains("};") {
                return Some((block_start, j));
            }
        }
    }
    None
}

fn find_attrset_block(lines: &[&str], key: &str) -> Option<(usize, usize)> {
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(key)
            || trimmed.starts_with(&format!("{key}."))
            || !line.contains('=')
            || !line.contains('{')
        {
            continue;
        }

        let mut depth = brace_delta(line);
        if depth <= 0 {
            continue;
        }

        for (j, candidate) in lines.iter().enumerate().skip(i + 1) {
            depth += brace_delta(candidate);
            if depth == 0 {
                return Some((i, j));
            }
        }
    }
    None
}

fn brace_delta(line: &str) -> i32 {
    line.chars().fold(0_i32, |acc, ch| match ch {
        '{' => acc + 1,
        '}' => acc - 1,
        _ => acc,
    })
}

fn detect_top_level_indent<'a>(lines: &'a [&str]) -> Option<&'a str> {
    lines.iter().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed == "{"
            || trimmed == "}"
            || trimmed.ends_with(':')
        {
            return None;
        }
        let indent_len = line.len() - line.trim_start().len();
        (indent_len > 0).then_some(&line[..indent_len])
    })
}

fn find_top_level_insert(lines: &[&str]) -> usize {
    lines
        .iter()
        .rposition(|line| line.trim() == "}")
        .unwrap_or(lines.len())
}

/// Find the `withPackages` block for a specific runtime.
///
/// Looks for `(runtime.withPackages` and finds the matching `)` or `]))`.
fn find_with_packages_block(lines: &[&str], runtime: &str) -> Option<(usize, usize)> {
    let pattern = format!("{runtime}.withPackages");
    for (i, line) in lines.iter().enumerate() {
        if line.contains(&pattern) {
            // Find the opening `[` on this or next line
            let list_start = if line.contains('[') {
                i
            } else if i + 1 < lines.len() && lines[i + 1].contains('[') {
                i + 1
            } else {
                continue;
            };

            // Find matching `]` — may be `]))` or `];` etc.
            for (j, close_line) in lines.iter().enumerate().skip(list_start + 1) {
                let trimmed = close_line.trim();
                if trimmed.starts_with(']') || trimmed.starts_with(')') || trimmed.contains("]))") {
                    return Some((list_start, j));
                }
            }
        }
    }
    None
}

/// Detect the indent used by entries in a bracket region.
fn detect_indent_in_region<'a>(lines: &'a [&str], start: usize, end: usize) -> Option<&'a str> {
    for line in &lines[start + 1..end] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Return the leading whitespace
        let indent_len = line.len() - line.trim_start().len();
        if indent_len > 0 {
            return Some(&line[..indent_len]);
        }
    }
    None
}

/// Find the alphabetical insertion point among bare identifiers.
///
/// Skips comment-only lines, blank lines, and section headers when comparing.
fn find_alpha_position(lines: &[&str], start: usize, end: usize, token: &str) -> usize {
    let token_lower = token.to_lowercase();
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        let trimmed = line.trim();
        // Skip blanks and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Extract the identifier
        let ident = extract_bare_ident(trimmed);
        if !ident.is_empty() && ident.to_lowercase() > token_lower {
            return i;
        }
    }
    // Insert before the closing bracket
    end
}

/// Find alphabetical insertion point among double-quoted entries.
fn find_alpha_position_quoted(lines: &[&str], start: usize, end: usize, token: &str) -> usize {
    let token_lower = token.to_lowercase();
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(existing) = extract_quoted_value(trimmed)
            && existing.to_lowercase() > token_lower
        {
            return i;
        }
    }
    end
}

/// Extract a bare nix identifier from a line (before any comment).
fn extract_bare_ident(line: &str) -> &str {
    let before_comment = line.split('#').next().unwrap_or("");
    before_comment.split_whitespace().next().unwrap_or("")
}

/// Extract the first double-quoted value from a line.
fn extract_quoted_value(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;

    // --- insert_nix_manifest ---

    #[test]
    fn nix_manifest_alphabetical_insertion() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
    fd
    ripgrep
  ];
}
";
        let (result, line) = insert_nix_manifest(content, "jq").unwrap();
        assert!(line.is_some());
        let lines: Vec<&str> = result.lines().collect();
        // jq should be between fd and ripgrep
        let jq_idx = lines.iter().position(|l| l.trim() == "jq").unwrap();
        let fd_idx = lines.iter().position(|l| l.trim() == "fd").unwrap();
        let rg_idx = lines.iter().position(|l| l.trim() == "ripgrep").unwrap();
        assert!(jq_idx > fd_idx);
        assert!(jq_idx < rg_idx);
    }

    #[test]
    fn nix_manifest_idempotent() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    ripgrep
  ];
}
";
        let (_, line) = insert_nix_manifest(content, "ripgrep").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn nix_manifest_preserves_comments() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat           # cat replacement
    ripgrep       # grep replacement
  ];
}
";
        let (result, line) = insert_nix_manifest(content, "fd").unwrap();
        assert!(line.is_some());
        assert!(result.contains("bat           # cat replacement"));
        assert!(result.contains("ripgrep       # grep replacement"));
    }

    #[test]
    fn nix_manifest_skips_section_headers() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    # === Core tools ===
    bat
    # === Dev tools ===
    ripgrep
  ];
}
";
        let (result, _) = insert_nix_manifest(content, "fd").unwrap();
        let lines: Vec<&str> = result.lines().collect();
        let fd_idx = lines.iter().position(|l| l.trim() == "fd").unwrap();
        let bat_idx = lines.iter().position(|l| l.trim() == "bat").unwrap();
        assert!(fd_idx > bat_idx, "fd should be after bat alphabetically");
    }

    #[test]
    fn nix_manifest_first_in_list() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    ripgrep
  ];
}
";
        let (result, line) = insert_nix_manifest(content, "bat").unwrap();
        assert!(line.is_some());
        let lines: Vec<&str> = result.lines().collect();
        let bat_idx = lines.iter().position(|l| l.trim() == "bat").unwrap();
        let rg_idx = lines.iter().position(|l| l.trim() == "ripgrep").unwrap();
        assert!(bat_idx < rg_idx);
    }

    #[test]
    fn nix_manifest_last_in_list() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
    fd
  ];
}
";
        let (result, line) = insert_nix_manifest(content, "zoxide").unwrap();
        assert!(line.is_some());
        let lines: Vec<&str> = result.lines().collect();
        let zox_idx = lines.iter().position(|l| l.trim() == "zoxide").unwrap();
        let fd_idx = lines.iter().position(|l| l.trim() == "fd").unwrap();
        assert!(zox_idx > fd_idx);
    }

    #[test]
    fn nix_manifest_detects_indent() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
  ];
}
";
        let (result, _) = insert_nix_manifest(content, "fd").unwrap();
        // Should use 4-space indent matching existing entries
        assert!(result.contains("    fd"));
    }

    // --- insert_language_package ---

    #[test]
    fn language_package_into_python_block() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (python3.withPackages (ps: with ps; [
      pyyaml
      rich
    ]))
  ];
}
";
        let (result, line) = insert_language_package(content, "requests", "python3").unwrap();
        assert!(line.is_some());
        let lines: Vec<&str> = result.lines().collect();
        let req_idx = lines.iter().position(|l| l.trim() == "requests").unwrap();
        let rich_idx = lines.iter().position(|l| l.trim() == "rich").unwrap();
        assert!(req_idx < rich_idx, "requests before rich alphabetically");
    }

    #[test]
    fn language_package_idempotent() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (python3.withPackages (ps: with ps; [
      pyyaml
    ]))
  ];
}
";
        let (_, line) = insert_language_package(content, "pyyaml", "python3").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn language_package_correct_runtime_targeting() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (lua5_4.withPackages (ps: [ ps.dkjson ]))
    (python3.withPackages (ps: with ps; [
      pyyaml
    ]))
  ];
}
";
        // Should insert into python3 block, not lua block
        let (result, line) = insert_language_package(content, "rich", "python3").unwrap();
        assert!(line.is_some());
        assert!(result.contains("rich"));
        // Verify rich is near pyyaml, not near dkjson
        let lines: Vec<&str> = result.lines().collect();
        let rich_idx = lines.iter().position(|l| l.trim() == "rich").unwrap();
        let pyyaml_idx = lines.iter().position(|l| l.trim() == "pyyaml").unwrap();
        assert!(rich_idx.abs_diff(pyyaml_idx) <= 2);
    }

    #[test]
    fn language_package_missing_runtime_errors() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (python3.withPackages (ps: with ps; [
      pyyaml
    ]))
  ];
}
";
        let result = insert_language_package(content, "lpeg", "lua5_4");
        assert!(result.is_err());
    }

    // --- insert_homebrew_manifest ---

    #[test]
    fn homebrew_alphabetical_insertion() {
        let content = "\
# nx: homebrew formula manifest
[
  \"deno\"
  \"yt-dlp\"
]
";
        let (result, line) = insert_homebrew_manifest(content, "htop").unwrap();
        assert!(line.is_some());
        let lines: Vec<&str> = result.lines().collect();
        let htop_idx = lines
            .iter()
            .position(|l| l.trim().contains("\"htop\""))
            .unwrap();
        let deno_idx = lines
            .iter()
            .position(|l| l.trim().contains("\"deno\""))
            .unwrap();
        let ytdlp_idx = lines
            .iter()
            .position(|l| l.trim().contains("\"yt-dlp\""))
            .unwrap();
        assert!(htop_idx > deno_idx);
        assert!(htop_idx < ytdlp_idx);
    }

    #[test]
    fn homebrew_idempotent() {
        let content = "\
[
  \"htop\"
]
";
        let (_, line) = insert_homebrew_manifest(content, "htop").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn homebrew_preserves_comments() {
        let content = "\
[
  \"deno\"                          # JS runtime
  \"yt-dlp\"                        # Video downloader
]
";
        let (result, line) = insert_homebrew_manifest(content, "htop").unwrap();
        assert!(line.is_some());
        assert!(result.contains("# JS runtime"));
        assert!(result.contains("# Video downloader"));
    }

    #[test]
    fn homebrew_first_in_list() {
        let content = "\
[
  \"deno\"
]
";
        let (result, line) = insert_homebrew_manifest(content, "bat").unwrap();
        assert!(line.is_some());
        let lines: Vec<&str> = result.lines().collect();
        let bat_idx = lines
            .iter()
            .position(|l| l.trim().contains("\"bat\""))
            .unwrap();
        let deno_idx = lines
            .iter()
            .position(|l| l.trim().contains("\"deno\""))
            .unwrap();
        assert!(bat_idx < deno_idx);
    }

    // --- insert_mas_app ---

    #[test]
    fn mas_app_idempotent() {
        let content = "\
{ ... }:
{
  homebrew.masApps = {
    \"Xcode\" = 497799835;
  };
}
";
        let (_, line) = insert_mas_app(content, "Xcode");
        assert!(line.is_none());
    }

    #[test]
    fn mas_app_alphabetical_insertion() {
        let content = "\
{ ... }:
{
  homebrew.masApps = {
    \"Slack\" = 803453959;
    \"Xcode\" = 497799835;
  };
}
";
        let (result, line) = insert_mas_app(content, "Telegram");
        assert_eq!(line, Some(5));
        assert!(result.contains("\"Telegram\" = 0;"));
        let lines: Vec<&str> = result.lines().collect();
        let slack_idx = lines.iter().position(|l| l.contains("\"Slack\"")).unwrap();
        let telegram_idx = lines
            .iter()
            .position(|l| l.contains("\"Telegram\""))
            .unwrap();
        let xcode_idx = lines.iter().position(|l| l.contains("\"Xcode\"")).unwrap();
        assert!(slack_idx < telegram_idx);
        assert!(telegram_idx < xcode_idx);
    }

    #[test]
    fn mas_app_inserts_into_empty_block() {
        let content = "\
{ ... }:
{
  homebrew.masApps = {
  };
}
";
        let (result, line) = insert_mas_app(content, "Xcode");
        assert_eq!(line, Some(4));
        assert!(result.contains("    \"Xcode\" = 0;"));
    }

    #[test]
    fn mas_app_missing_block_inserts_into_homebrew_attrset() {
        let content = "\
{ pkgs, ... }:
{
  homebrew = {
    enable = true;
    taps = [ ];
    casks = [ ];
  };
}
";
        let (result, line) = insert_mas_app(content, "Xcode");
        assert!(line.is_some());
        assert!(result.contains("    masApps = {"));
        assert!(result.contains("      \"Xcode\" = 0;"));
        assert!(!result.contains("homebrew.masApps = {"));
    }

    #[test]
    fn mas_app_missing_block_falls_back_to_top_level() {
        let content = "\
{ ... }:
{
  system.defaults = {};
}
";
        let (result, line) = insert_mas_app(content, "Xcode");
        assert!(line.is_some());
        assert!(result.contains("  homebrew.masApps = {"));
        assert!(result.contains("    \"Xcode\" = 0;"));
    }

    // --- apply_edit (integration via temp files) ---

    #[test]
    fn apply_edit_writes_file() {
        use crate::domain::plan::{InsertionMode, InstallPlan};
        use crate::domain::source::{PackageSource, SourceResult};

        let tmp = tempfile::TempDir::new().unwrap();
        let cli_path = tmp.path().join("cli.nix");
        fs::write(
            &cli_path,
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n    ripgrep\n  ];\n}\n",
        )
        .unwrap();

        let plan = InstallPlan {
            source_result: SourceResult::new("fd", PackageSource::Nxs),
            package_token: "fd".to_string(),
            target_file: cli_path.clone(),
            insertion_mode: InsertionMode::NixManifest,

            language_info: None,
            routing_warning: None,
        };

        let outcome = apply_edit(&plan).unwrap();
        assert!(outcome.file_changed);
        assert!(outcome.line_number.is_some());

        let written = fs::read_to_string(&cli_path).unwrap();
        assert!(written.contains("fd"));
    }

    #[test]
    fn apply_edit_missing_file_errors() {
        use crate::domain::plan::{InsertionMode, InstallPlan};
        use crate::domain::source::{PackageSource, SourceResult};

        let plan = InstallPlan {
            source_result: SourceResult::new("fd", PackageSource::Nxs),
            package_token: "fd".to_string(),
            target_file: std::path::PathBuf::from("/nonexistent/cli.nix"),
            insertion_mode: InsertionMode::NixManifest,

            language_info: None,
            routing_warning: None,
        };

        assert!(apply_edit(&plan).is_err());
    }

    #[test]
    fn apply_edit_errors_for_language_mode_without_language_info() {
        use crate::domain::plan::{InsertionMode, InstallPlan};
        use crate::domain::source::{PackageSource, SourceResult};

        let tmp = tempfile::TempDir::new().unwrap();
        let languages = tmp.path().join("languages.nix");
        fs::write(
            &languages,
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    (python3.withPackages (ps: with ps; [\n      rich\n    ]))\n  ];\n}\n",
        )
        .unwrap();

        let plan = InstallPlan {
            source_result: SourceResult::new("python3Packages.requests", PackageSource::Nxs),
            package_token: "python3Packages.requests".to_string(),
            target_file: languages,
            insertion_mode: InsertionMode::LanguageWithPackages,
            language_info: None,
            routing_warning: None,
        };

        let err = apply_edit(&plan).expect_err("missing language info should return an error");
        assert!(err.to_string().contains("language_info required"));
    }

    // --- remove_nix_manifest ---

    #[test]
    fn nix_manifest_removes_middle_entry() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
    fd
    ripgrep
  ];
}
";
        let (result, line) = remove_nix_manifest(content, "fd").unwrap();
        assert_eq!(line, Some(5)); // 1-indexed
        assert!(!result.contains("\n    fd\n"));
        assert!(result.contains("    bat\n"));
        assert!(result.contains("    ripgrep\n"));
    }

    #[test]
    fn nix_manifest_removes_first_entry() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
    fd
  ];
}
";
        let (result, line) = remove_nix_manifest(content, "bat").unwrap();
        assert!(line.is_some());
        assert!(!result.contains("bat"));
        assert!(result.contains("    fd\n"));
    }

    #[test]
    fn nix_manifest_removes_last_entry() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
    fd
  ];
}
";
        let (result, line) = remove_nix_manifest(content, "fd").unwrap();
        assert!(line.is_some());
        assert!(!result.contains("fd"));
        assert!(result.contains("    bat\n"));
    }

    #[test]
    fn nix_manifest_remove_not_found() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
  ];
}
";
        let (_, line) = remove_nix_manifest(content, "nonexistent").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn nix_manifest_remove_preserves_comments() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat           # cat replacement
    fd
    ripgrep       # grep replacement
  ];
}
";
        let (result, line) = remove_nix_manifest(content, "fd").unwrap();
        assert!(line.is_some());
        assert!(result.contains("bat           # cat replacement"));
        assert!(result.contains("ripgrep       # grep replacement"));
    }

    #[test]
    fn nix_manifest_remove_preserves_section_headers() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    # === Core tools ===
    bat
    fd
    # === Dev tools ===
    ripgrep
  ];
}
";
        let (result, line) = remove_nix_manifest(content, "fd").unwrap();
        assert!(line.is_some());
        assert!(result.contains("# === Core tools ==="));
        assert!(result.contains("# === Dev tools ==="));
        assert!(result.contains("    bat\n"));
        assert!(result.contains("    ripgrep\n"));
    }

    // --- remove_language_package ---

    #[test]
    fn language_package_removes_from_python_block() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (python3.withPackages (ps: with ps; [
      pyyaml
      requests
      rich
    ]))
  ];
}
";
        let (result, line) = remove_language_package(content, "requests", "python3").unwrap();
        assert!(line.is_some());
        assert!(!result.contains("requests"));
        assert!(result.contains("      pyyaml\n"));
        assert!(result.contains("      rich\n"));
    }

    #[test]
    fn language_package_remove_not_found() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (python3.withPackages (ps: with ps; [
      pyyaml
    ]))
  ];
}
";
        let (_, line) = remove_language_package(content, "nonexistent", "python3").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn language_package_remove_correct_runtime() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (lua5_4.withPackages (ps: [
      dkjson
    ]))
    (python3.withPackages (ps: with ps; [
      pyyaml
      rich
    ]))
  ];
}
";
        let (result, line) = remove_language_package(content, "rich", "python3").unwrap();
        assert!(line.is_some());
        assert!(!result.contains("      rich\n"));
        // lua block untouched
        assert!(result.contains("      dkjson\n"));
        assert!(result.contains("      pyyaml\n"));
    }

    #[test]
    fn language_package_remove_missing_runtime_errors() {
        let content = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    (python3.withPackages (ps: with ps; [
      pyyaml
    ]))
  ];
}
";
        // lua5_4 block doesn't exist but token also doesn't exist → idempotent, not error
        let (_, line) = remove_language_package(content, "lpeg", "lua5_4").unwrap();
        assert!(line.is_none());
    }

    // --- remove_homebrew_manifest ---

    #[test]
    fn homebrew_removes_entry() {
        let content = "\
[
  \"deno\"
  \"htop\"
  \"yt-dlp\"
]
";
        let (result, line) = remove_homebrew_manifest(content, "htop").unwrap();
        assert!(line.is_some());
        assert!(!result.contains("\"htop\""));
        assert!(result.contains("\"deno\""));
        assert!(result.contains("\"yt-dlp\""));
    }

    #[test]
    fn homebrew_remove_not_found() {
        let content = "\
[
  \"deno\"
]
";
        let (_, line) = remove_homebrew_manifest(content, "nonexistent").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn homebrew_remove_preserves_comments() {
        let content = "\
[
  \"deno\"                          # JS runtime
  \"htop\"
  \"yt-dlp\"                        # Video downloader
]
";
        let (result, line) = remove_homebrew_manifest(content, "htop").unwrap();
        assert!(line.is_some());
        assert!(result.contains("# JS runtime"));
        assert!(result.contains("# Video downloader"));
    }

    // --- remove_mas_app ---

    #[test]
    fn mas_app_removes_entry() {
        let content = "\
{ ... }:
{
  homebrew.masApps = {
    \"Xcode\" = 497799835;
    \"Slack\" = 803453959;
  };
}
";
        let (result, line) = remove_mas_app(content, "Xcode").unwrap();
        assert!(line.is_some());
        assert!(!result.contains("Xcode"));
        assert!(result.contains("Slack"));
    }

    #[test]
    fn mas_app_remove_not_found() {
        let content = "\
{ ... }:
{
  homebrew.masApps = {
    \"Xcode\" = 497799835;
  };
}
";
        let (_, line) = remove_mas_app(content, "Nonexistent").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn mas_app_remove_missing_block_is_idempotent() {
        let content = "\
{ ... }:
{
  system.defaults = {};
}
";
        let (_, line) = remove_mas_app(content, "Xcode").unwrap();
        assert!(line.is_none());
    }

    // --- apply_removal (integration via temp files) ---

    #[test]
    fn apply_removal_writes_file() {
        use crate::domain::plan::{InsertionMode, InstallPlan};
        use crate::domain::source::{PackageSource, SourceResult};

        let tmp = tempfile::TempDir::new().unwrap();
        let cli_path = tmp.path().join("cli.nix");
        fs::write(
            &cli_path,
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n    fd\n    ripgrep\n  ];\n}\n",
        )
        .unwrap();

        let plan = InstallPlan {
            source_result: SourceResult::new("fd", PackageSource::Nxs),
            package_token: "fd".to_string(),
            target_file: cli_path.clone(),
            insertion_mode: InsertionMode::NixManifest,
            language_info: None,
            routing_warning: None,
        };

        let outcome = apply_removal(&plan).unwrap();
        assert!(outcome.file_changed);
        assert!(outcome.line_number.is_some());

        let written = fs::read_to_string(&cli_path).unwrap();
        assert!(!written.contains("fd"));
        assert!(written.contains("bat"));
        assert!(written.contains("ripgrep"));
    }

    #[test]
    fn apply_removal_idempotent_no_write() {
        use crate::domain::plan::{InsertionMode, InstallPlan};
        use crate::domain::source::{PackageSource, SourceResult};

        let tmp = tempfile::TempDir::new().unwrap();
        let cli_path = tmp.path().join("cli.nix");
        fs::write(
            &cli_path,
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n",
        )
        .unwrap();

        let plan = InstallPlan {
            source_result: SourceResult::new("nonexistent", PackageSource::Nxs),
            package_token: "nonexistent".to_string(),
            target_file: cli_path,
            insertion_mode: InsertionMode::NixManifest,
            language_info: None,
            routing_warning: None,
        };

        let outcome = apply_removal(&plan).unwrap();
        assert!(!outcome.file_changed);
        assert!(outcome.line_number.is_none());
    }

    #[test]
    fn apply_removal_errors_for_language_mode_without_language_info() {
        use crate::domain::plan::{InsertionMode, InstallPlan};
        use crate::domain::source::{PackageSource, SourceResult};

        let tmp = tempfile::TempDir::new().unwrap();
        let languages = tmp.path().join("languages.nix");
        fs::write(
            &languages,
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    (python3.withPackages (ps: with ps; [\n      rich\n    ]))\n  ];\n}\n",
        )
        .unwrap();

        let plan = InstallPlan {
            source_result: SourceResult::new("python3Packages.rich", PackageSource::Nxs),
            package_token: "python3Packages.rich".to_string(),
            target_file: languages,
            insertion_mode: InsertionMode::LanguageWithPackages,
            language_info: None,
            routing_warning: None,
        };

        let err = apply_removal(&plan).expect_err("missing language info should return an error");
        assert!(err.to_string().contains("language_info required"));
    }

    // --- roundtrip: insert then remove restores original ---

    #[test]
    fn nix_manifest_insert_then_remove_roundtrip() {
        let original = "\
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    bat
    ripgrep
  ];
}
";
        let (with_fd, _) = insert_nix_manifest(original, "fd").unwrap();
        assert!(with_fd.contains("    fd\n"));

        let (restored, _) = remove_nix_manifest(&with_fd, "fd").unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn homebrew_insert_then_remove_roundtrip() {
        let original = "\
[
  \"deno\"
  \"yt-dlp\"
]
";
        let (with_htop, _) = insert_homebrew_manifest(original, "htop").unwrap();
        assert!(with_htop.contains("\"htop\""));

        let (restored, _) = remove_homebrew_manifest(&with_htop, "htop").unwrap();
        assert_eq!(restored, original);
    }
}
