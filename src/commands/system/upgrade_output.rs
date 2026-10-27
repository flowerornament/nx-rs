use std::collections::BTreeMap;

use crate::cli::UpgradeArgs;
use crate::domain::upgrade_receipt::{
    ObservationStatus, PackageState, UpgradeReceipt, UpgradeStep,
};
use crate::output::printer::Printer;

pub(super) fn print(
    receipt: &UpgradeReceipt,
    code: i32,
    args: &UpgradeArgs,
    elapsed_ms: u128,
    printer: &Printer,
) {
    println!();
    let title = format!(
        "Upgrade {} · {}.{}s",
        if code == 0 { "complete" } else { "incomplete" },
        elapsed_ms / 1000,
        elapsed_ms % 1000 / 100
    );
    if code == 0 {
        printer.success(&title);
    } else {
        printer.error(&title);
    }
    print_observations(receipt, args.flow.verbose);
    for line in outcome_lines(receipt) {
        Printer::paragraph(&line);
    }
    if args.flow.verbose {
        Printer::sub_paragraph(&format!(
            "Inputs: {}; Homebrew: {}; rebuild: {}; activation: {}; commit: {}",
            receipt.inputs, receipt.homebrew, receipt.rebuild, receipt.activation, receipt.commit
        ));
        Printer::sub_paragraph(&format!(
            "System profile: {} → {}",
            receipt.system_before.as_deref().unwrap_or("unknown"),
            receipt.system_after.as_deref().unwrap_or("unknown")
        ));
        for shadowed in &receipt.shadowed_commands {
            Printer::sub_paragraph(shadowed);
        }
    } else {
        for line in path_notes(&receipt.shadowed_commands) {
            Printer::paragraph(&line);
        }
    }
}

fn outcome_lines(receipt: &UpgradeReceipt) -> Vec<String> {
    let mut lines = vec![format!(
        "Nix system: {}",
        match receipt.activation {
            UpgradeStep::AlreadyCurrent => "already current; activation skipped".to_owned(),
            UpgradeStep::Completed => "activated".to_owned(),
            UpgradeStep::Skipped => "rebuild skipped".to_owned(),
            _ => format!(
                "rebuild {}; activation {}",
                receipt.rebuild, receipt.activation
            ),
        }
    )];
    let inputs = if receipt.changed_inputs.is_empty() {
        receipt.inputs.to_string()
    } else {
        format!("updated {}", receipt.changed_inputs.join(", "))
    };
    let commit = match receipt.commit {
        UpgradeStep::Committed => "committed locally".to_owned(),
        UpgradeStep::NotNeeded | UpgradeStep::NoOwnedChanges => "no commit needed".to_owned(),
        _ => format!("commit {}", receipt.commit),
    };
    lines.push(format!("Config: {inputs}; {commit}"));
    if receipt.homebrew != UpgradeStep::Completed {
        lines.push(format!("Homebrew upgrade: {}", receipt.homebrew));
    }
    lines
}

pub(super) fn package_label(name: &str, verbose: bool) -> String {
    if verbose {
        return name.to_owned();
    }
    name.strip_prefix("formula:").map_or_else(
        || {
            name.strip_prefix("cask:")
                .map_or_else(|| name.to_owned(), |name| format!("{name} (cask)"))
        },
        str::to_owned,
    )
}

pub(super) fn version_change(old: &str, new: &str, cask: bool, verbose: bool) -> String {
    let primary = |version: &str| {
        version
            .split_once(',')
            .filter(|(_, build)| {
                !build.is_empty() && build.bytes().all(|byte| byte.is_ascii_digit())
            })
            .map(|(version, _)| version.to_owned())
    };
    if cask
        && !verbose
        && let (Some(old), Some(new)) = (primary(old), primary(new))
        && old != new
    {
        return format!("{old} → {new}");
    }
    format!("{old} → {new}")
}

fn versions(packages: &[PackageState]) -> String {
    let mut versions: Vec<_> = packages
        .iter()
        .map(|package| package.version.as_str())
        .collect();
    versions.sort_unstable();
    versions.dedup();
    versions.join(", ")
}

fn print_observations(receipt: &UpgradeReceipt, verbose: bool) {
    for observed in &receipt.packages {
        let scope = if observed.scope == "Homebrew" {
            "Homebrew"
        } else {
            "Nix profile references"
        };
        match observed.status {
            ObservationStatus::Unknown => Printer::paragraph(&format!(
                "{scope}: changes unknown ({})",
                observed
                    .detail
                    .as_deref()
                    .unwrap_or("observation unavailable")
            )),
            ObservationStatus::Skipped => {
                if verbose {
                    Printer::detail(&format!("{scope}: observation skipped"));
                }
            }
            ObservationStatus::Observed if observed.changes.is_empty() => {
                if scope == "Homebrew" || verbose {
                    Printer::detail(&format!("{scope}: no observed package changes"));
                }
            }
            ObservationStatus::Observed => {
                Printer::detail(&format!("{scope} · {} changed", observed.changes.len()));
                let rows = observed
                    .changes
                    .iter()
                    .map(|change| {
                        let old = versions(&change.before);
                        let new = versions(&change.after);
                        let value = if change.before.is_empty() {
                            format!(
                                "{} {new}",
                                if scope == "Homebrew" {
                                    "installed"
                                } else {
                                    "added reference"
                                }
                            )
                        } else if change.after.is_empty() {
                            format!(
                                "{} {old}",
                                if scope == "Homebrew" {
                                    "removed"
                                } else {
                                    "removed reference"
                                }
                            )
                        } else if old == new {
                            format!("{new} (output changed)")
                        } else {
                            version_change(&old, &new, change.name.starts_with("cask:"), verbose)
                        };
                        (package_label(&change.name, verbose), value)
                    })
                    .collect::<Vec<_>>();
                Printer::rows(&rows);
            }
        }
    }
}

// Receipts persist these strings; unfamiliar diagnostic formats remain visible.
fn path_notes(shadowed_commands: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for shadowed in shadowed_commands {
        if let Some((name, selected)) = shadowed.split_once(": PATH selects ")
            && let Some(path) = selected.strip_suffix(" instead of Homebrew's upgraded command")
        {
            groups
                .entry(
                    path.rsplit_once('/')
                        .map_or(path, |(directory, _)| directory),
                )
                .or_default()
                .push(name);
        } else {
            notes.push(shadowed.clone());
        }
    }
    for (directory, names) in &groups {
        notes.push(format!(
            "Note: {} use {directory}, so Homebrew's updates do not change those commands.",
            names.join(", ")
        ));
    }
    notes
}

pub(super) fn homebrew_line_visible(line: &str) -> bool {
    let line = line.trim();
    if line.contains("Warning:") || line.contains("Error:") || line.contains("error:") {
        return true;
    }
    !line.is_empty()
        && ![
            "==> Downloading",
            "==> Fetching",
            "==> Pouring",
            "==> Linking",
            "==> Unlinking",
            "==> Purging",
            "==> Cleanup",
            "Removing:",
            "✔︎ Bottle",
            "✔︎ Cask",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn homebrew_filter_preserves_warnings_caveats_prompts_and_unknown_output() {
        for line in [
            "Warning: caveat",
            "Error: failed",
            "  details",
            "==> Caveats",
            "Proceed? [y/N]",
            "unfamiliar output",
            "🍺 unfamiliar informational output",
            "==> Linking Error: failure",
        ] {
            assert!(homebrew_line_visible(line), "{line}");
        }
        for line in [
            "==> Linking Binary tool",
            "==> Purging files for version 1",
            "Removing: old package",
            "✔︎ Bottle tool (2)",
            "",
        ] {
            assert!(!homebrew_line_visible(line), "{line}");
        }
    }

    #[test]
    fn cask_display_keeps_build_only_updates_and_verbose_identity() {
        assert_eq!(
            version_change("1.4.4,123", "1.4.5,456", true, false),
            "1.4.4 → 1.4.5"
        );
        assert_eq!(
            version_change("1.4.4,123", "1.4.4,456", true, false),
            "1.4.4,123 → 1.4.4,456"
        );
        assert_eq!(
            version_change("1.4.4,123", "1.4.5,456", true, true),
            "1.4.4,123 → 1.4.5,456"
        );
        assert_eq!(
            version_change("1,abc", "2,def", true, false),
            "1,abc → 2,def"
        );
    }

    #[test]
    fn path_notes_group_commands_without_changing_recorded_diagnostics() {
        let raw = ["node", "npm", "npx"].map(|name| format!("{name}: PATH selects /etc/profiles/per-user/morgan/bin/{name} instead of Homebrew's upgraded command"));
        assert_eq!(
            path_notes(&raw),
            [
                "Note: node, npm, npx use /etc/profiles/per-user/morgan/bin, so Homebrew's updates do not change those commands."
            ]
        );
        assert!(raw[0].contains("/bin/node"));
        for unknown in [
            "unrecognized diagnostic",
            "tool: PATH selects unfamiliar diagnostic",
        ] {
            assert_eq!(path_notes(&[unknown.into()]), [unknown]);
        }
    }

    #[test]
    fn outcomes_distinguish_unchanged_activation_from_partial_failure() {
        let mut receipt = super::super::receipt::start(&UpgradeArgs::parse_from(["upgrade"]));
        receipt.activation = UpgradeStep::AlreadyCurrent;
        receipt.rebuild = UpgradeStep::Completed;
        receipt.homebrew = UpgradeStep::Completed;
        receipt.inputs = UpgradeStep::LockChanged;
        receipt.changed_inputs = vec!["sops-nix".into()];
        receipt.commit = UpgradeStep::Committed;
        assert_eq!(
            outcome_lines(&receipt),
            [
                "Nix system: already current; activation skipped",
                "Config: updated sops-nix; committed locally"
            ]
        );
        receipt.rebuild = UpgradeStep::Failed;
        receipt.activation = UpgradeStep::Failed;
        receipt.commit = UpgradeStep::NotRun;
        assert_eq!(
            outcome_lines(&receipt),
            [
                "Nix system: rebuild failed; activation failed",
                "Config: updated sops-nix; commit not run"
            ]
        );
    }
}
