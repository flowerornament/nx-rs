use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlakeInputEdit {
    Added { input_name: String },
    AlreadyExists { input_name: String },
}

pub fn add_flake_input(
    flake_path: &Path,
    flake_url: &str,
    input_name: Option<&str>,
) -> anyhow::Result<FlakeInputEdit> {
    if !flake_path.exists() {
        bail!("flake.nix not found");
    }

    let content = fs::read_to_string(flake_path)
        .with_context(|| format!("reading {}", flake_path.display()))?;
    let resolved_name =
        input_name.map_or_else(|| derive_flake_input_name(flake_url), str::to_string);

    if input_exists(&content, &resolved_name) {
        return Ok(FlakeInputEdit::AlreadyExists {
            input_name: resolved_name,
        });
    }

    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let trailing_newline = content.ends_with('\n');

    let start_idx = lines
        .iter()
        .position(|line| inputs_opening_regex().is_match(line))
        .ok_or_else(|| anyhow::anyhow!("inputs block not found"))?;
    let end_idx = find_block_end(&lines, start_idx)
        .ok_or_else(|| anyhow::anyhow!("inputs block end not found"))?;

    let base_indent = lines[start_idx]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    let indent = format!("{base_indent}  ");
    let attr = format_flake_input_attr(&resolved_name);
    let new_line = format!("{indent}{attr}.url = \"{flake_url}\";");
    lines.insert(end_idx, new_line);

    let mut updated = lines.join("\n");
    if trailing_newline {
        updated.push('\n');
    }

    fs::write(flake_path, updated).with_context(|| format!("writing {}", flake_path.display()))?;

    Ok(FlakeInputEdit::Added {
        input_name: resolved_name,
    })
}

fn input_exists(content: &str, input_name: &str) -> bool {
    let escaped = regex::escape(input_name);
    let pattern = format!(r#"(?m)^\s*("{escaped}"|{escaped})\.url\s*="#);
    Regex::new(&pattern)
        .expect("input-exists regex should compile")
        .is_match(content)
}

fn inputs_opening_regex() -> &'static Regex {
    static ONCE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| Regex::new(r"\binputs\s*=\s*\{").expect("inputs regex should compile"))
}

fn find_block_end(lines: &[String], start_idx: usize) -> Option<usize> {
    let mut depth = 0isize;
    for (idx, line) in lines.iter().enumerate().skip(start_idx) {
        let opens = isize::try_from(line.matches('{').count()).expect("brace count should fit");
        let closes = isize::try_from(line.matches('}').count()).expect("brace count should fit");
        depth += opens;
        depth -= closes;
        if depth == 0 && idx > start_idx {
            return Some(idx);
        }
    }
    None
}

fn derive_flake_input_name(flake_url: &str) -> String {
    let url = flake_url.trim().trim_end_matches('/');
    let mut name = String::new();

    if url.contains("flakehub.com") {
        let parts: Vec<&str> = url.split('/').collect();
        if let Some(idx) = parts.iter().position(|part| *part == "f")
            && idx + 2 < parts.len()
        {
            name = parts[idx + 2].to_string();
        }
    }

    if name.is_empty()
        && url.contains(':')
        && url.contains('/')
        && let Some((_, suffix)) = url.split_once(':')
    {
        name = suffix.rsplit('/').next().unwrap_or_default().to_string();
    }

    if name.is_empty() {
        name = url.rsplit('/').next().unwrap_or_default().to_string();
    }

    let normalized = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    if normalized.is_empty() {
        "input".to_string()
    } else {
        normalized
    }
}

fn format_flake_input_attr(name: &str) -> String {
    let valid = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$")
        .expect("flake attr regex should compile")
        .is_match(name);
    if valid {
        name.to_string()
    } else {
        format!("\"{name}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_flake(tmp: &TempDir, content: &str) -> std::path::PathBuf {
        let path = tmp.path().join("flake.nix");
        fs::write(&path, content).expect("flake file should be written");
        path
    }

    #[test]
    fn derive_name_from_flakehub_url() {
        let name = derive_flake_input_name("https://flakehub.com/f/DeterminateSystems/fh");
        assert_eq!(name, "fh");
    }

    #[test]
    fn derive_name_from_github_style_url() {
        let name = derive_flake_input_name("github:nix-community/NUR");
        assert_eq!(name, "nur");
    }

    #[test]
    fn format_attr_quotes_invalid_identifier() {
        assert_eq!(
            format_flake_input_attr("nix-community"),
            "\"nix-community\""
        );
        assert_eq!(format_flake_input_attr("nur"), "nur");
    }

    #[test]
    fn add_flake_input_inserts_line_into_inputs_block() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let flake = write_flake(
            &tmp,
            r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
  };
}
"#,
        );

        let outcome = add_flake_input(&flake, "github:nix-community/NUR", None)
            .expect("flake input should be added");
        assert_eq!(
            outcome,
            FlakeInputEdit::Added {
                input_name: "nur".to_string()
            }
        );

        let updated = fs::read_to_string(&flake).expect("updated flake should be readable");
        assert!(updated.contains("nur.url = \"github:nix-community/NUR\";"));
    }

    #[test]
    fn add_flake_input_is_idempotent_when_present() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let flake = write_flake(
            &tmp,
            r#"{
  inputs = {
    nur.url = "github:nix-community/NUR";
  };
}
"#,
        );

        let before = fs::read_to_string(&flake).expect("flake should be readable");
        let outcome = add_flake_input(&flake, "github:nix-community/NUR", Some("nur"))
            .expect("existing input should not error");
        let after = fs::read_to_string(&flake).expect("flake should be readable after");

        assert_eq!(
            outcome,
            FlakeInputEdit::AlreadyExists {
                input_name: "nur".to_string()
            }
        );
        assert_eq!(before, after);
    }

    #[test]
    fn add_flake_input_errors_without_inputs_block() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let flake = write_flake(
            &tmp,
            r"{
  outputs = { self, nixpkgs }: {};
}
",
        );

        let err = add_flake_input(&flake, "github:nix-community/NUR", None)
            .expect_err("missing inputs block should error");
        assert!(err.to_string().contains("inputs block not found"));
    }

    #[test]
    fn add_flake_input_errors_when_file_missing() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let missing = tmp.path().join("flake.nix");

        let err = add_flake_input(&missing, "github:nix-community/NUR", None)
            .expect_err("missing flake file should error");
        assert!(err.to_string().contains("flake.nix not found"));
    }
}
