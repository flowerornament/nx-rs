#![allow(dead_code)]

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellHistoryEntry {
    pub command: String,
    pub started_at_epoch_secs: Option<i64>,
    pub duration_secs: Option<u64>,
}

impl ShellHistoryEntry {
    fn timestamped(command: &str, started_at_epoch_secs: i64, duration_secs: Option<u64>) -> Self {
        Self {
            command: command.to_string(),
            started_at_epoch_secs: Some(started_at_epoch_secs),
            duration_secs,
        }
    }
}

pub fn parse_zsh_extended_history(text: &str) -> Vec<ShellHistoryEntry> {
    text.lines().filter_map(parse_zsh_extended_line).collect()
}

pub fn parse_bash_timestamped_history(text: &str) -> Vec<ShellHistoryEntry> {
    let mut pending_timestamp = None;
    let mut entries = Vec::new();

    for line in text.lines() {
        if let Some(comment) = line.strip_prefix('#') {
            pending_timestamp = parse_bash_timestamp_comment(comment);
            continue;
        }

        if line.trim().is_empty() {
            continue;
        }

        if let Some(timestamp) = pending_timestamp.take() {
            entries.push(ShellHistoryEntry::timestamped(line, timestamp, None));
        }
    }

    entries
}

fn parse_zsh_extended_line(line: &str) -> Option<ShellHistoryEntry> {
    let rest = line.strip_prefix(": ")?;
    let (epoch, rest) = rest.split_once(':')?;
    let (duration, command) = rest.split_once(';')?;
    let started_at_epoch_secs = epoch.trim().parse().ok()?;
    let duration_secs = duration.trim().parse().ok()?;

    Some(ShellHistoryEntry::timestamped(
        command,
        started_at_epoch_secs,
        Some(duration_secs),
    ))
}

fn parse_bash_timestamp_comment(line: &str) -> Option<i64> {
    if line.is_empty() || line.contains(char::is_whitespace) {
        return None;
    }
    line.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{ShellHistoryEntry, parse_bash_timestamped_history, parse_zsh_extended_history};

    #[test]
    fn parses_zsh_extended_history_records() {
        let entries = parse_zsh_extended_history(
            ": 1760000000:3;rg package\nignored\n: 1760000010:0;nx where ripgrep\n",
        );

        assert_eq!(
            entries,
            vec![
                ShellHistoryEntry {
                    command: "rg package".to_string(),
                    started_at_epoch_secs: Some(1_760_000_000),
                    duration_secs: Some(3),
                },
                ShellHistoryEntry {
                    command: "nx where ripgrep".to_string(),
                    started_at_epoch_secs: Some(1_760_000_010),
                    duration_secs: Some(0),
                },
            ]
        );
    }

    #[test]
    fn skips_malformed_zsh_extended_history_records() {
        let entries = parse_zsh_extended_history(
            ": not-an-epoch:3;rg package\n: 1760000000:nope;fd src\n: 1760000010:1;bat README.md",
        );

        assert_eq!(
            entries,
            vec![ShellHistoryEntry {
                command: "bat README.md".to_string(),
                started_at_epoch_secs: Some(1_760_000_010),
                duration_secs: Some(1),
            }]
        );
    }

    #[test]
    fn parses_bash_history_timestamp_comments() {
        let entries = parse_bash_timestamped_history(
            "#1760000000\nrg package\n#1760000010\nnx where ripgrep\nuntimestamped\n",
        );

        assert_eq!(
            entries,
            vec![
                ShellHistoryEntry {
                    command: "rg package".to_string(),
                    started_at_epoch_secs: Some(1_760_000_000),
                    duration_secs: None,
                },
                ShellHistoryEntry {
                    command: "nx where ripgrep".to_string(),
                    started_at_epoch_secs: Some(1_760_000_010),
                    duration_secs: None,
                },
            ]
        );
    }

    #[test]
    fn ignores_bash_timestamp_comments_without_commands() {
        let entries =
            parse_bash_timestamped_history("#1760000000\n\n#not-a-timestamp\n#1760000010\nfd src");

        assert_eq!(
            entries,
            vec![ShellHistoryEntry {
                command: "fd src".to_string(),
                started_at_epoch_secs: Some(1_760_000_010),
                duration_secs: None,
            }]
        );
    }
}
