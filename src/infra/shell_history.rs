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

    fn untimestamped(command: &str) -> Self {
        Self {
            command: command.to_string(),
            started_at_epoch_secs: None,
            duration_secs: None,
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

pub fn parse_timestamped_shell_history(text: &str) -> Vec<ShellHistoryEntry> {
    let mut entries = parse_zsh_extended_history(text);
    entries.extend(parse_bash_timestamped_history(text));
    entries
}

pub fn parse_shell_history(text: &str) -> Vec<ShellHistoryEntry> {
    let mut entries = parse_timestamped_shell_history(text);
    entries.extend(parse_fish_history(text));
    entries.extend(parse_untimestamped_shell_history(text));
    entries
}

fn parse_untimestamped_shell_history(text: &str) -> Vec<ShellHistoryEntry> {
    let mut entries = Vec::new();
    let mut skip_next_command = false;

    for line in text.lines() {
        let command = line.trim();
        let is_bash_timestamp = command
            .strip_prefix('#')
            .is_some_and(|comment| parse_bash_timestamp_comment(comment).is_some());
        if is_bash_timestamp {
            skip_next_command = true;
            continue;
        }
        if command.is_empty() || command.starts_with(": ") {
            continue;
        }
        if is_fish_history_metadata(command) {
            continue;
        }
        if skip_next_command {
            skip_next_command = false;
            continue;
        }
        entries.push(ShellHistoryEntry::untimestamped(command));
    }

    entries
}

pub fn parse_fish_history(text: &str) -> Vec<ShellHistoryEntry> {
    let mut entries = Vec::new();
    let mut pending_command: Option<String> = None;
    let mut pending_timestamp: Option<i64> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(command) = trimmed.strip_prefix("- cmd:") {
            push_fish_entry(
                &mut entries,
                pending_command.take(),
                pending_timestamp.take(),
            );
            pending_command = Some(command.trim().to_string());
            continue;
        }
        if let Some(timestamp) = trimmed.strip_prefix("when:") {
            pending_timestamp = timestamp.trim().parse().ok();
        }
    }

    push_fish_entry(&mut entries, pending_command, pending_timestamp);
    entries
}

fn push_fish_entry(
    entries: &mut Vec<ShellHistoryEntry>,
    command: Option<String>,
    timestamp: Option<i64>,
) {
    let Some(command) = command.filter(|command| !command.trim().is_empty()) else {
        return;
    };
    entries.push(ShellHistoryEntry {
        command,
        started_at_epoch_secs: timestamp,
        duration_secs: None,
    });
}

fn is_fish_history_metadata(command: &str) -> bool {
    command.starts_with("- cmd:") || command.starts_with("when:") || command.starts_with("paths:")
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
    use super::{
        ShellHistoryEntry, parse_bash_timestamped_history, parse_fish_history, parse_shell_history,
        parse_timestamped_shell_history, parse_zsh_extended_history,
    };

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

    #[test]
    fn parses_mixed_timestamped_shell_history() {
        let entries =
            parse_timestamped_shell_history(": 1760000000:3;rg package\n#1760000010\nfd src");

        assert_eq!(
            entries,
            vec![
                ShellHistoryEntry {
                    command: "rg package".to_string(),
                    started_at_epoch_secs: Some(1_760_000_000),
                    duration_secs: Some(3),
                },
                ShellHistoryEntry {
                    command: "fd src".to_string(),
                    started_at_epoch_secs: Some(1_760_000_010),
                    duration_secs: None,
                },
            ]
        );
    }

    #[test]
    fn parses_fish_history_records() {
        let entries = parse_fish_history(
            "- cmd: rg package\n  when: 1760000000\n- cmd: bat README.md\n- cmd: fd src\n  when: nope\n",
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
                    command: "bat README.md".to_string(),
                    started_at_epoch_secs: None,
                    duration_secs: None,
                },
                ShellHistoryEntry {
                    command: "fd src".to_string(),
                    started_at_epoch_secs: None,
                    duration_secs: None,
                },
            ]
        );
    }

    #[test]
    fn parse_shell_history_includes_untimestamped_commands_without_duplicates() {
        let entries = parse_shell_history(
            ": 1760000000:3;rg package\n#1760000010\nfd src\nbat README.md\n- cmd: nx unused\n  when: 1760000020\n",
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
                    command: "fd src".to_string(),
                    started_at_epoch_secs: Some(1_760_000_010),
                    duration_secs: None,
                },
                ShellHistoryEntry {
                    command: "nx unused".to_string(),
                    started_at_epoch_secs: Some(1_760_000_020),
                    duration_secs: None,
                },
                ShellHistoryEntry {
                    command: "bat README.md".to_string(),
                    started_at_epoch_secs: None,
                    duration_secs: None,
                },
            ]
        );
    }
}
