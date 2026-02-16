use std::fs;
use std::path::Path;

use anyhow::{Result, bail};

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
    let path = &plan.target_file;
    let content = read_file(path)?;

    let (new_content, line_number) = match plan.insertion_mode {
        InsertionMode::NixManifest => insert_nix_manifest(&content, &plan.package_token),
        InsertionMode::LanguageWithPackages => {
            let lang = plan
                .language_info
                .as_ref()
                .expect("language_info required for LanguageWithPackages");
            insert_language_package(&content, &lang.bare_name, &lang.runtime)
        }
        InsertionMode::HomebrewManifest => insert_homebrew_manifest(&content, &plan.package_token),
        InsertionMode::MasApps => insert_mas_app(&content, &plan.package_token),
    }?;

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

    let mut result: Vec<&str> = Vec::with_capacity(lines.len() + 1);
    for (i, line) in lines.iter().enumerate() {
        if i == insert_at {
            // We'll push owned string below; collect as &str up to here
            break;
        }
        result.push(line);
    }

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
/// Currently bails if no masApps block exists (block creation deferred).
fn insert_mas_app(content: &str, token: &str) -> Result<(String, Option<usize>)> {
    // Check idempotency
    if content.contains(&format!("\"{token}\"")) {
        return Ok((content.to_string(), None));
    }

    // masApps block creation is deferred — bail if missing
    if !content.contains("masApps") {
        bail!("no masApps block found in target file; manual block creation required (deferred)");
    }

    bail!("masApps insertion not yet implemented; add manually for now");
}

// --- Helpers

fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))
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
        let (_, line) = insert_mas_app(content, "Xcode").unwrap();
        assert!(line.is_none());
    }

    #[test]
    fn mas_app_missing_block_errors() {
        let content = "\
{ ... }:
{
  system.defaults = {};
}
";
        let result = insert_mas_app(content, "Xcode");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no masApps block"));
    }

    // --- apply_edit (integration via temp files) ---

    #[test]
    fn apply_edit_writes_file() {
        use crate::domain::plan::{InsertionMode, InstallPlan};
        use crate::domain::source::SourceResult;

        let tmp = tempfile::TempDir::new().unwrap();
        let cli_path = tmp.path().join("cli.nix");
        fs::write(
            &cli_path,
            "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n    ripgrep\n  ];\n}\n",
        )
        .unwrap();

        let plan = InstallPlan {
            source_result: SourceResult::new("fd", "nxs"),
            package_token: "fd".to_string(),
            target_file: cli_path.clone(),
            insertion_mode: InsertionMode::NixManifest,
            is_brew: false,
            is_cask: false,
            is_mas: false,
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
        use crate::domain::source::SourceResult;

        let plan = InstallPlan {
            source_result: SourceResult::new("fd", "nxs"),
            package_token: "fd".to_string(),
            target_file: std::path::PathBuf::from("/nonexistent/cli.nix"),
            insertion_mode: InsertionMode::NixManifest,
            is_brew: false,
            is_cask: false,
            is_mas: false,
            language_info: None,
            routing_warning: None,
        };

        assert!(apply_edit(&plan).is_err());
    }
}
