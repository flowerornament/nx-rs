use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, bail};
use regex::Regex;
use serde_json::Value;

use crate::domain::source::is_tap_qualified;
use crate::infra::shell::{CapturedCommand, first_nonempty_output, run_captured_command_with_env};
use crate::output::printer::Printer;

#[derive(Debug, Default, PartialEq, Eq)]
struct Declarations {
    formulae: Vec<String>,
    casks: Vec<String>,
}

/// Read the evaluated Brewfile, without evaluating Nix again or executing Ruby.
pub(super) fn validate_candidate(system: &Path, printer: &Printer) -> anyhow::Result<()> {
    static BREWFILE: OnceLock<Regex> = OnceLock::new();
    let activation = match fs::read_to_string(system.join("activate")) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("reading candidate activation"),
    };
    let pattern = BREWFILE.get_or_init(|| {
        Regex::new(r#"brew bundle[^\n]*--file=['"]([^'"]+)['"]"#).expect("Brewfile pattern")
    });
    let Some(path) = pattern.captures(&activation).and_then(|m| m.get(1)) else {
        return Ok(());
    };
    let declarations =
        parse_brewfile(&fs::read_to_string(path.as_str()).context("reading candidate Brewfile")?)?;
    if declarations.formulae.is_empty() && declarations.casks.is_empty() {
        return Ok(());
    }
    printer.action("Validating Homebrew declarations");
    printer.with_loading("Checking formula and cask definitions", |_| {
        validate(&declarations, |names, cask| {
            let mut args = vec![
                "info",
                "--json=v2",
                if cask { "--cask" } else { "--formula" },
            ];
            args.extend(names.iter().map(String::as_str));
            run_captured_command_with_env(
                "brew",
                &args,
                None,
                Some(&[("HOMEBREW_NO_AUTO_UPDATE", "1")]),
            )
        })
    })?;
    let deferred = declarations
        .formulae
        .iter()
        .chain(&declarations.casks)
        .filter(|name| is_tap_qualified(name))
        .count();
    if deferred > 0 {
        Printer::detail(&format!(
            "{deferred} tapped declarations are resolved by Homebrew during activation"
        ));
    }
    printer.success("Homebrew core declarations validated");
    Ok(())
}

fn parse_brewfile(content: &str) -> anyhow::Result<Declarations> {
    static ENTRY: OnceLock<Regex> = OnceLock::new();
    let pattern = ENTRY.get_or_init(|| {
        Regex::new(r#"^(brew|cask)\s+"([a-zA-Z0-9@._+/-]+)"(?:\s*,.*)?$"#)
            .expect("Brewfile entry pattern")
    });
    let mut result = Declarations::default();
    for line in content.lines().map(str::trim) {
        if !line.starts_with("brew ") && !line.starts_with("cask ") {
            continue;
        }
        let entry = pattern
            .captures(line)
            .context("unsupported generated Brewfile entry")?;
        let target = if &entry[1] == "cask" {
            &mut result.casks
        } else {
            &mut result.formulae
        };
        target.push(entry[2].to_string());
    }
    Ok(result)
}

fn validate(
    declarations: &Declarations,
    mut query: impl FnMut(&[String], bool) -> anyhow::Result<CapturedCommand>,
) -> anyhow::Result<()> {
    for (names, cask) in [(&declarations.formulae, false), (&declarations.casks, true)] {
        // A newly declared tap need not be installed yet; brew installs it later.
        let core = names
            .iter()
            .filter(|name| !is_tap_qualified(name))
            .cloned()
            .collect::<Vec<_>>();
        if core.is_empty() {
            continue;
        }
        let output = query(&core, cask)?;
        let kind = if cask { "cask" } else { "formula" };
        if output.code != 0 {
            bail!(
                "Homebrew {kind} declaration check failed: {}. Check for renamed packages or formula/cask migrations before rebuilding",
                first_nonempty_output(&output)
            );
        }
        let data: Value =
            serde_json::from_str(&output.stdout).context("invalid Homebrew definition metadata")?;
        let entries = data
            .get(if cask { "casks" } else { "formulae" })
            .and_then(Value::as_array)
            .context("missing Homebrew definition list")?;
        let mut available = HashSet::new();
        for entry in entries {
            for key in ["name", "full_name", "token", "full_token"] {
                if let Some(name) = entry.get(key).and_then(Value::as_str) {
                    available.insert(name);
                }
            }
            for key in ["aliases", "oldnames", "old_tokens"] {
                if let Some(aliases) = entry.get(key).and_then(Value::as_array) {
                    available.extend(aliases.iter().filter_map(Value::as_str));
                }
            }
        }
        for name in &core {
            if !available.contains(name.as_str()) {
                bail!(
                    "Homebrew {kind} definition not found: {name}; check for renamed packages or formula/cask migrations"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_definitions_without_requiring_installations() {
        let declarations = parse_brewfile("tap \"example/tools\"\nbrew \"git\", trusted: true\ncask \"vercel\"\ncask \"example/tools/new-app\"").unwrap();
        let mut calls = Vec::new();
        validate(&declarations, |names, cask| {
            calls.push((names.to_vec(), cask));
            let json = if cask {
                r#"{"casks":[{"token":"vercel","installed":null}]}"#
            } else {
                r#"{"formulae":[{"name":"git","installed":[]}]}"#
            };
            Ok(CapturedCommand::captured(0, json.into(), String::new()))
        })
        .unwrap();
        assert_eq!(
            calls,
            vec![(vec!["git".into()], false), (vec!["vercel".into()], true)]
        );
    }

    #[test]
    fn missing_and_migrated_definitions_fail() {
        let declarations = parse_brewfile("brew \"vercel-cli\"").unwrap();
        for (code, json) in [
            (1, ""),
            (0, r#"{"formulae":[]}"#),
            (0, "{}"),
            (0, "invalid"),
        ] {
            assert!(
                validate(&declarations, |_, _| Ok(CapturedCommand::captured(
                    code,
                    json.into(),
                    "not found".into()
                )))
                .is_err()
            );
        }
    }

    #[test]
    fn aliases_are_valid_but_dynamic_ruby_is_not_interpreted() {
        let declarations = parse_brewfile("brew \"old-name\"").unwrap();
        validate(&declarations, |_, _| {
            Ok(CapturedCommand::captured(
                0,
                r#"{"formulae":[{"name":"new-name","oldnames":["old-name"]}]}"#.into(),
                String::new(),
            ))
        })
        .unwrap();
        assert!(parse_brewfile("brew ENV.fetch(\"PACKAGE\")").is_err());
    }
}
