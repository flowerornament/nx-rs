use std::collections::HashSet;

use serde::Serialize;

use crate::infra::shell_history::ShellHistoryEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredPackage {
    pub name: String,
    pub source: UsageSource,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageSource {
    Nix,
    Homebrew,
    Cask,
    Mas,
    Service,
}

impl UsageSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nix => "nix",
            Self::Homebrew => "homebrew",
            Self::Cask => "cask",
            Self::Mas => "mas",
            Self::Service => "service",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageStatus {
    Recent,
    Old,
    Unknown,
    Protected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub enum EvidenceConfidence {
    None,
    Weak,
    Medium,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    ShellHistory,
    Policy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceItem {
    pub kind: EvidenceKind,
    pub summary: String,
    pub timestamp: Option<i64>,
    pub confidence: EvidenceConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageRecord {
    pub name: String,
    pub source: UsageSource,
    pub location: Option<String>,
    pub status: UsageStatus,
    pub last_seen: Option<i64>,
    pub confidence: EvidenceConfidence,
    pub evidence: Vec<EvidenceItem>,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageAuditOptions {
    pub since_seconds: u64,
}

pub fn parse_since_seconds(value: &str) -> Option<u64> {
    let (amount, unit) = split_duration(value)?;
    let multiplier = match unit {
        "d" => 86_400,
        "w" => 7 * 86_400,
        "mo" => 30 * 86_400,
        "y" => 365 * 86_400,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

pub fn audit_usage_records(
    packages: &[DeclaredPackage],
    shell_history: &[ShellHistoryEntry],
    now_epoch_secs: i64,
    options: UsageAuditOptions,
) -> Vec<UsageRecord> {
    let since_seconds = i64::try_from(options.since_seconds).unwrap_or(i64::MAX);
    let cutoff = now_epoch_secs.saturating_sub(since_seconds);
    packages
        .iter()
        .map(|package| audit_package(package, shell_history, cutoff))
        .collect()
}

fn audit_package(
    package: &DeclaredPackage,
    shell_history: &[ShellHistoryEntry],
    cutoff_epoch_secs: i64,
) -> UsageRecord {
    let mut evidence = shell_history_evidence(package, shell_history);
    let protected_reason = protection_reason(package);
    if let Some(reason) = protected_reason {
        evidence.push(EvidenceItem {
            kind: EvidenceKind::Policy,
            summary: reason.to_string(),
            timestamp: None,
            confidence: EvidenceConfidence::Strong,
        });
    }

    let last_seen = evidence.iter().filter_map(|item| item.timestamp).max();
    let confidence = evidence
        .iter()
        .map(|item| item.confidence)
        .max()
        .unwrap_or(EvidenceConfidence::None);
    let status = if protected_reason.is_some() {
        UsageStatus::Protected
    } else if last_seen.is_some_and(|timestamp| timestamp >= cutoff_epoch_secs) {
        UsageStatus::Recent
    } else if last_seen.is_some() {
        UsageStatus::Old
    } else {
        UsageStatus::Unknown
    };

    UsageRecord {
        name: package.name.clone(),
        source: package.source,
        location: package.location.clone(),
        status,
        last_seen,
        confidence,
        evidence,
        suggestions: suggestions(&package.name),
    }
}

fn shell_history_evidence(
    package: &DeclaredPackage,
    shell_history: &[ShellHistoryEntry],
) -> Vec<EvidenceItem> {
    let aliases = command_aliases(&package.name);
    shell_history
        .iter()
        .filter_map(|entry| {
            let command = command_word(&entry.command)?;
            aliases.contains(command).then(|| EvidenceItem {
                kind: EvidenceKind::ShellHistory,
                summary: format!("command `{command}` appeared in shell history"),
                timestamp: entry.started_at_epoch_secs,
                confidence: EvidenceConfidence::Strong,
            })
        })
        .collect()
}

fn command_word(command: &str) -> Option<&str> {
    for token in command.split_whitespace() {
        if token.contains('=') && !token.starts_with('-') {
            continue;
        }
        if matches!(token, "command" | "env" | "noglob" | "sudo" | "time") {
            continue;
        }
        return Some(token.rsplit('/').next().unwrap_or(token));
    }
    None
}

fn command_aliases(package: &str) -> HashSet<&str> {
    let bare = package.rsplit('.').next().unwrap_or(package);
    let mut aliases = HashSet::from([package, bare]);
    match package {
        "ripgrep" => {
            aliases.insert("rg");
        }
        "fd" => {
            aliases.insert("fdfind");
        }
        "neovim" => {
            aliases.insert("nvim");
            aliases.insert("vim");
        }
        "nodejs" => {
            aliases.insert("node");
        }
        "python3" => {
            aliases.insert("python");
            aliases.insert("python3");
        }
        _ => {}
    }
    aliases
}

fn protection_reason(package: &DeclaredPackage) -> Option<&'static str> {
    if package.source == UsageSource::Service {
        return Some("active service declarations are protected from usage pruning");
    }

    let name = package.name.as_str();
    let protected = [
        "bash",
        "darwin-rebuild",
        "fish",
        "git",
        "home-manager",
        "neovim",
        "nix",
        "nx",
        "vim",
        "zsh",
    ];
    protected
        .contains(&name)
        .then_some("core shell, editor, package manager, or nx workflow tool")
}

fn suggestions(name: &str) -> Vec<String> {
    vec![
        format!("nx where {name}"),
        format!("nx remove --dry-run {name}"),
    ]
}

fn split_duration(value: &str) -> Option<(u64, &str)> {
    let digit_len = value
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    let amount = value[..digit_len].parse().ok()?;
    let unit = &value[digit_len..];
    (!unit.is_empty()).then_some((amount, unit))
}

#[cfg(test)]
mod tests {
    use super::{
        DeclaredPackage, EvidenceConfidence, UsageAuditOptions, UsageSource, UsageStatus,
        audit_usage_records, command_word, parse_since_seconds,
    };
    use crate::infra::shell_history::ShellHistoryEntry;

    #[test]
    fn parse_since_accepts_supported_units() {
        assert_eq!(parse_since_seconds("30d"), Some(30 * 86_400));
        assert_eq!(parse_since_seconds("12w"), Some(12 * 7 * 86_400));
        assert_eq!(parse_since_seconds("6mo"), Some(6 * 30 * 86_400));
        assert_eq!(parse_since_seconds("1y"), Some(365 * 86_400));
    }

    #[test]
    fn parse_since_rejects_unknown_shapes() {
        assert_eq!(parse_since_seconds("90"), None);
        assert_eq!(parse_since_seconds("d90"), None);
        assert_eq!(parse_since_seconds("2h"), None);
    }

    #[test]
    fn command_word_skips_wrappers_and_env_assignments() {
        assert_eq!(
            command_word("RUST_LOG=debug sudo /opt/bin/rg foo"),
            Some("rg")
        );
        assert_eq!(
            command_word("env FOO=bar command nvim README.md"),
            Some("nvim")
        );
    }

    #[test]
    fn shell_history_scores_recent_old_and_unknown_records() {
        let packages = vec![package("ripgrep"), package("fd"), package("graphviz")];
        let history = vec![
            ShellHistoryEntry {
                command: "rg src".to_string(),
                started_at_epoch_secs: Some(1_000),
                duration_secs: Some(1),
            },
            ShellHistoryEntry {
                command: "fd Cargo".to_string(),
                started_at_epoch_secs: Some(100),
                duration_secs: None,
            },
        ];

        let records = audit_usage_records(
            &packages,
            &history,
            1_000,
            UsageAuditOptions { since_seconds: 200 },
        );

        assert_eq!(records[0].status, UsageStatus::Recent);
        assert_eq!(records[0].confidence, EvidenceConfidence::Strong);
        assert_eq!(records[1].status, UsageStatus::Old);
        assert_eq!(records[2].status, UsageStatus::Unknown);
    }

    #[test]
    fn services_are_protected() {
        let records = audit_usage_records(
            &[DeclaredPackage {
                name: "sops-nix".to_string(),
                source: UsageSource::Service,
                location: None,
            }],
            &[],
            1_000,
            UsageAuditOptions { since_seconds: 200 },
        );

        assert_eq!(records[0].status, UsageStatus::Protected);
        assert_eq!(records[0].confidence, EvidenceConfidence::Strong);
    }

    fn package(name: &str) -> DeclaredPackage {
        DeclaredPackage {
            name: name.to_string(),
            source: UsageSource::Nix,
            location: None,
        }
    }
}
