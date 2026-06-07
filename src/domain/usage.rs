use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::infra::shell_history::ShellHistoryEntry;

const DEFAULT_USAGE_ALIAS_CATALOG: &[(&str, &str)] = &[
    ("awk", "gawk"),
    ("aws", "awscli2"),
    ("aws", "awscli"),
    ("bd", "steveyegge/beads/bd"),
    ("bq", "google-cloud-sdk"),
    ("btm", "bottom"),
    ("claude", "claude-code"),
    ("cmp", "diffutils"),
    ("codex", "codex-cli"),
    ("compare", "imagemagick"),
    ("composite", "imagemagick"),
    ("convert", "imagemagick"),
    ("createdb", "postgresql"),
    ("createuser", "postgresql"),
    ("diff", "diffutils"),
    ("diff3", "diffutils"),
    ("difft", "difftastic"),
    ("display", "imagemagick"),
    ("docker", "docker-client"),
    ("dropdb", "postgresql"),
    ("dropuser", "postgresql"),
    ("egrep", "gnugrep"),
    ("ffplay", "ffmpeg"),
    ("ffprobe", "ffmpeg"),
    ("fgrep", "gnugrep"),
    ("find", "findutils"),
    ("gcloud", "google-cloud-sdk"),
    ("grep", "gnugrep"),
    ("gpg", "gnupg"),
    ("gpg-agent", "gnupg"),
    ("gpg2", "gnupg"),
    ("gs", "ghostscript"),
    ("gsutil", "google-cloud-sdk"),
    ("helm", "kubernetes-helm"),
    ("http", "httpie"),
    ("https", "httpie"),
    ("identify", "imagemagick"),
    ("import", "imagemagick"),
    ("magick", "imagemagick"),
    ("make", "gnumake"),
    ("mogrify", "imagemagick"),
    ("montage", "imagemagick"),
    ("node", "nodejs"),
    ("npm", "nodejs"),
    ("npx", "nodejs"),
    ("nvim", "neovim"),
    ("pdfinfo", "poppler-utils"),
    ("pdfseparate", "poppler-utils"),
    ("pdftocairo", "poppler-utils"),
    ("pdftoppm", "poppler-utils"),
    ("pdftotext", "poppler-utils"),
    ("pdfunite", "poppler-utils"),
    ("pg_dump", "postgresql"),
    ("pg_isready", "postgresql"),
    ("pg_restore", "postgresql"),
    ("ps2pdf", "ghostscript"),
    ("psql", "postgresql"),
    ("python", "python3"),
    ("rga", "ripgrep-all"),
    ("rg", "ripgrep"),
    ("scp", "openssh"),
    ("sdiff", "diffutils"),
    ("sed", "gnused"),
    ("sftp", "openssh"),
    ("sqlite3", "sqlite"),
    ("ssh", "openssh"),
    ("ssh-add", "openssh"),
    ("ssh-agent", "openssh"),
    ("ssh-keygen", "openssh"),
    ("tar", "gnutar"),
    ("tldr", "tealdeer"),
    ("tofu", "opentofu"),
    ("vercel", "vercel-cli"),
    ("wg", "wireguard-tools"),
    ("wg-quick", "wireguard-tools"),
    ("xdg-mime", "xdg-utils"),
    ("xdg-open", "xdg-utils"),
    ("xdg-settings", "xdg-utils"),
    ("xargs", "findutils"),
    ("yq", "yq-go"),
];

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

pub fn default_usage_aliases_for_packages<'a>(
    packages: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, String> {
    let package_match_names = packages
        .into_iter()
        .flat_map(package_match_names)
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();

    let mut aliases = HashMap::new();
    for (alias, package) in DEFAULT_USAGE_ALIAS_CATALOG {
        if package_match_names.contains(*package) {
            aliases
                .entry((*alias).to_string())
                .or_insert_with(|| (*package).to_string());
        }
    }
    aliases
}

pub fn audit_usage_records(
    packages: &[DeclaredPackage],
    shell_history: &[ShellHistoryEntry],
    package_aliases: &HashMap<String, String>,
    now_epoch_secs: i64,
    options: UsageAuditOptions,
) -> Vec<UsageRecord> {
    let since_seconds = i64::try_from(options.since_seconds).unwrap_or(i64::MAX);
    let cutoff = now_epoch_secs.saturating_sub(since_seconds);
    let history_index = ShellHistoryIndex::new(shell_history);
    packages
        .iter()
        .map(|package| audit_package(package, &history_index, package_aliases, cutoff))
        .collect()
}

fn audit_package(
    package: &DeclaredPackage,
    history_index: &ShellHistoryIndex<'_>,
    package_aliases: &HashMap<String, String>,
    cutoff_epoch_secs: i64,
) -> UsageRecord {
    let mut evidence = history_index.evidence_for_package(package, package_aliases);
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

struct ShellHistoryIndex<'a> {
    by_command: HashMap<&'a str, Vec<&'a ShellHistoryEntry>>,
}

impl<'a> ShellHistoryIndex<'a> {
    fn new(shell_history: &'a [ShellHistoryEntry]) -> Self {
        let mut by_command: HashMap<&'a str, Vec<&'a ShellHistoryEntry>> = HashMap::new();
        for entry in shell_history {
            if let Some(command) = command_word(&entry.command) {
                by_command.entry(command).or_default().push(entry);
            }
        }
        Self { by_command }
    }

    fn evidence_for_package(
        &self,
        package: &DeclaredPackage,
        package_aliases: &HashMap<String, String>,
    ) -> Vec<EvidenceItem> {
        command_aliases(&package.name, package_aliases)
            .into_iter()
            .filter_map(|command| {
                self.by_command
                    .get(command.as_str())
                    .map(|entries| (command, entries))
            })
            .flat_map(|(command, entries)| {
                entries.iter().map(move |entry| EvidenceItem {
                    kind: EvidenceKind::ShellHistory,
                    summary: shell_history_summary(&command, entry.started_at_epoch_secs),
                    timestamp: entry.started_at_epoch_secs,
                    confidence: if entry.started_at_epoch_secs.is_some() {
                        EvidenceConfidence::Strong
                    } else {
                        EvidenceConfidence::Medium
                    },
                })
            })
            .collect()
    }
}

fn shell_history_summary(command: &str, timestamp: Option<i64>) -> String {
    if timestamp.is_some() {
        format!("command `{command}` appeared in timestamped shell history")
    } else {
        format!("command `{command}` appeared in untimestamped shell history")
    }
}

fn command_word(command: &str) -> Option<&str> {
    let mut tokens = command.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        if token.contains('=') && !token.starts_with('-') {
            continue;
        }
        if matches!(token, "command" | "noglob") {
            continue;
        }
        if matches!(token, "env" | "sudo" | "time") {
            skip_wrapper_options(token, &mut tokens);
            continue;
        }
        if token.starts_with('-') {
            continue;
        }
        return Some(token.rsplit('/').next().unwrap_or(token));
    }
    None
}

fn skip_wrapper_options<'a>(
    wrapper: &str,
    tokens: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) {
    while let Some(option) = tokens.next_if(|token| token.starts_with('-')) {
        if wrapper_option_takes_value(wrapper, option)
            && !option.contains('=')
            && !short_option_has_inline_value(option)
        {
            tokens.next();
        }
    }
}

fn wrapper_option_takes_value(wrapper: &str, option: &str) -> bool {
    match wrapper {
        "env" => matches!(
            option,
            "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string"
        ),
        "sudo" => matches!(
            option,
            "-A" | "-a"
                | "-b"
                | "-C"
                | "-c"
                | "-D"
                | "-g"
                | "-h"
                | "-p"
                | "-R"
                | "-r"
                | "-T"
                | "-t"
                | "-U"
                | "-u"
                | "--askpass"
                | "--auth-type"
                | "--background"
                | "--close-from"
                | "--chdir"
                | "--group"
                | "--host"
                | "--prompt"
                | "--chroot"
                | "--role"
                | "--command-timeout"
                | "--type"
                | "--other-user"
                | "--user"
        ),
        _ => false,
    }
}

fn short_option_has_inline_value(option: &str) -> bool {
    option.starts_with('-') && !option.starts_with("--") && option.len() > 2
}

fn command_aliases(package: &str, package_aliases: &HashMap<String, String>) -> Vec<String> {
    let slash_bare = package.rsplit('/').next().unwrap_or(package);
    let bare = slash_bare.rsplit('.').next().unwrap_or(slash_bare);
    let mut aliases = Vec::new();
    let package_names = package_match_names(package);
    push_alias(&mut aliases, package);
    push_alias(&mut aliases, slash_bare);
    push_alias(&mut aliases, bare);

    if let Some(cli_name) = bare.strip_suffix("-cli") {
        push_alias(&mut aliases, cli_name);
    }

    for (alias, target) in package_aliases {
        if package_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(target))
        {
            push_alias(&mut aliases, alias);
        }
    }
    aliases
}

fn package_match_names(package: &str) -> Vec<&str> {
    let slash_bare = package.rsplit('/').next().unwrap_or(package);
    let bare = slash_bare.rsplit('.').next().unwrap_or(slash_bare);
    let mut names = Vec::new();
    push_borrowed_unique(&mut names, package);
    push_borrowed_unique(&mut names, slash_bare);
    push_borrowed_unique(&mut names, bare);
    names
}

fn push_borrowed_unique<'a>(values: &mut Vec<&'a str>, value: &'a str) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn push_alias(aliases: &mut Vec<String>, alias: &str) {
    if !aliases.iter().any(|existing| existing == alias) {
        aliases.push(alias.to_string());
    }
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
    use std::collections::HashMap;

    use super::{
        DeclaredPackage, EvidenceConfidence, UsageAuditOptions, UsageSource, UsageStatus,
        audit_usage_records, command_word, default_usage_aliases_for_packages, parse_since_seconds,
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
    fn default_usage_aliases_include_verified_catalog_entries_for_declared_packages() {
        let aliases = default_usage_aliases_for_packages([
            "ripgrep",
            "nodejs",
            "python3",
            "imagemagick",
            "gnugrep",
            "neovim",
            "yq-go",
            "sqlite",
            "postgresql",
            "docker-client",
            "wireguard-tools",
            "gnupg",
            "openssh",
            "gawk",
            "diffutils",
            "ffmpeg",
            "ghostscript",
            "poppler-utils",
        ]);

        assert_eq!(aliases.get("rg").map(String::as_str), Some("ripgrep"));
        assert_eq!(aliases.get("node").map(String::as_str), Some("nodejs"));
        assert_eq!(aliases.get("npm").map(String::as_str), Some("nodejs"));
        assert_eq!(aliases.get("python").map(String::as_str), Some("python3"));
        assert_eq!(
            aliases.get("magick").map(String::as_str),
            Some("imagemagick")
        );
        assert_eq!(aliases.get("grep").map(String::as_str), Some("gnugrep"));
        assert_eq!(aliases.get("nvim").map(String::as_str), Some("neovim"));
        assert_eq!(aliases.get("yq").map(String::as_str), Some("yq-go"));
        assert_eq!(aliases.get("sqlite3").map(String::as_str), Some("sqlite"));
        assert_eq!(aliases.get("psql").map(String::as_str), Some("postgresql"));
        assert_eq!(
            aliases.get("docker").map(String::as_str),
            Some("docker-client")
        );
        assert_eq!(
            aliases.get("wg-quick").map(String::as_str),
            Some("wireguard-tools")
        );
        assert_eq!(aliases.get("gpg").map(String::as_str), Some("gnupg"));
        assert_eq!(aliases.get("ssh").map(String::as_str), Some("openssh"));
        assert_eq!(aliases.get("awk").map(String::as_str), Some("gawk"));
        assert_eq!(aliases.get("diff").map(String::as_str), Some("diffutils"));
        assert_eq!(aliases.get("ffprobe").map(String::as_str), Some("ffmpeg"));
        assert_eq!(
            aliases.get("ps2pdf").map(String::as_str),
            Some("ghostscript")
        );
        assert_eq!(
            aliases.get("pdftotext").map(String::as_str),
            Some("poppler-utils")
        );
        assert!(!aliases.contains_key("vim"));
        assert!(!aliases.contains_key("sg"));
    }

    #[test]
    fn command_word_skips_wrappers_and_env_assignments() {
        assert_eq!(
            command_word("RUST_LOG=debug sudo /opt/bin/rg foo"),
            Some("rg")
        );
        assert_eq!(command_word("sudo -E rg src"), Some("rg"));
        assert_eq!(command_word("sudo -u root rg src"), Some("rg"));
        assert_eq!(command_word("env -u FOO rg src"), Some("rg"));
        assert_eq!(command_word("time -p rg src"), Some("rg"));
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
            &HashMap::from([("rg".to_string(), "ripgrep".to_string())]),
            1_000,
            UsageAuditOptions { since_seconds: 200 },
        );

        assert_eq!(records[0].status, UsageStatus::Recent);
        assert_eq!(records[0].confidence, EvidenceConfidence::Strong);
        assert_eq!(records[1].status, UsageStatus::Old);
        assert_eq!(records[2].status, UsageStatus::Unknown);
    }

    #[test]
    fn untimestamped_shell_history_is_medium_evidence_but_not_recent() {
        let records = audit_usage_records(
            &[package("bat")],
            &[ShellHistoryEntry {
                command: "bat README.md".to_string(),
                started_at_epoch_secs: None,
                duration_secs: None,
            }],
            &package_aliases(),
            1_000,
            UsageAuditOptions { since_seconds: 200 },
        );

        assert_eq!(records[0].status, UsageStatus::Unknown);
        assert_eq!(records[0].confidence, EvidenceConfidence::Medium);
        assert_eq!(
            records[0].evidence[0].summary,
            "command `bat` appeared in untimestamped shell history"
        );
    }

    #[test]
    fn manifest_aliases_match_common_command_names() {
        let records = audit_usage_records(
            &[
                package("steveyegge/beads/bd"),
                package("codex-cli"),
                package("claude-code"),
                package("ast-grep"),
            ],
            &[
                ShellHistoryEntry {
                    command: "bd ready".to_string(),
                    started_at_epoch_secs: None,
                    duration_secs: None,
                },
                ShellHistoryEntry {
                    command: "codex --version".to_string(),
                    started_at_epoch_secs: None,
                    duration_secs: None,
                },
                ShellHistoryEntry {
                    command: "claude -p status".to_string(),
                    started_at_epoch_secs: None,
                    duration_secs: None,
                },
                ShellHistoryEntry {
                    command: "sg run --pattern TODO".to_string(),
                    started_at_epoch_secs: None,
                    duration_secs: None,
                },
            ],
            &HashMap::from([
                ("bd".to_string(), "steveyegge/beads/bd".to_string()),
                ("codex".to_string(), "codex-cli".to_string()),
                ("claude".to_string(), "claude-code".to_string()),
                ("sg".to_string(), "ast-grep".to_string()),
            ]),
            1_000,
            UsageAuditOptions { since_seconds: 200 },
        );

        assert!(records.iter().all(|record| {
            record.status == UsageStatus::Unknown
                && record.confidence == EvidenceConfidence::Medium
                && !record.evidence.is_empty()
        }));
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
            &package_aliases(),
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

    fn package_aliases() -> HashMap<String, String> {
        HashMap::new()
    }
}
