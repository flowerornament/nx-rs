use std::collections::BTreeSet;
use std::fmt::Write as _;

use clap::{Arg, Command, CommandFactory};

use crate::cli::{Cli, HelpArgs};

#[derive(Debug, Clone)]
struct ResolvedCommand {
    command: Command,
    path: Vec<String>,
    consumed: usize,
}

#[derive(Debug, Clone)]
struct FlagMatch {
    command_path: String,
    long: Option<String>,
    short: Option<char>,
    aliases: Vec<String>,
    help: String,
}

#[derive(Debug, Clone)]
struct CommandMatch {
    path: String,
    about: String,
    aliases: Vec<String>,
}

pub fn cmd_help(args: &HelpArgs) -> i32 {
    match render_help(args) {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(message) => {
            eprintln!("{message}");
            2
        }
    }
}

fn render_help(args: &HelpArgs) -> Result<String, String> {
    let root = Cli::command();
    if args.topics.is_empty() {
        return render_command_help(root);
    }

    let resolved = resolve_command_path(&root, &args.topics);
    if resolved.consumed == args.topics.len() {
        return render_command_help(resolved.command);
    }

    let remaining = &args.topics[resolved.consumed..];
    if remaining.len() != 1 {
        return Err(format!(
            "Unable to resolve help path '{}'. Try `nx help` for available topics.",
            args.topics.join(" ")
        ));
    }
    let query = &remaining[0];

    let command_matches = find_command_matches(&resolved.command, &resolved.path, query);
    let flag_matches = find_flag_matches(&resolved.command, &resolved.path, query);

    if !command_matches.is_empty() && !flag_matches.is_empty() {
        return Ok(render_combined_matches(
            query,
            &resolved.path,
            &command_matches,
            &flag_matches,
        ));
    }
    if !command_matches.is_empty() {
        return Ok(render_command_matches(
            query,
            &resolved.path,
            &command_matches,
        ));
    }
    if !flag_matches.is_empty() {
        return Ok(render_flag_matches(query, &resolved.path, &flag_matches));
    }

    Err(format!(
        "No help topic matched '{query}'. Try `nx help` for available commands."
    ))
}

fn render_command_help(mut command: Command) -> Result<String, String> {
    let mut out = Vec::<u8>::new();
    command
        .write_long_help(&mut out)
        .map_err(|err| format!("failed to render help: {err}"))?;
    let mut text = String::from_utf8(out).map_err(|err| format!("invalid help text: {err}"))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn resolve_command_path(root: &Command, topics: &[String]) -> ResolvedCommand {
    let mut command = root.clone();
    let mut path = vec![command.get_name().to_string()];
    let mut consumed = 0;

    for topic in topics {
        if topic.starts_with('-') {
            break;
        }
        let Some(next) = find_child_command(&command, topic) else {
            break;
        };
        path.push(next.get_name().to_string());
        command = next.clone();
        consumed += 1;
    }

    ResolvedCommand {
        command,
        path,
        consumed,
    }
}

fn find_child_command<'a>(command: &'a Command, topic: &str) -> Option<&'a Command> {
    command.get_subcommands().find(|child| {
        child.get_name() == topic || child.get_all_aliases().any(|alias| alias == topic)
    })
}

fn find_flag_matches(command: &Command, base_path: &[String], query: &str) -> Vec<FlagMatch> {
    let matcher = FlagMatcher::new(query);
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    collect_flag_matches(command, base_path, &matcher, &mut seen, &mut results);
    results
}

fn collect_flag_matches(
    command: &Command,
    path: &[String],
    matcher: &FlagMatcher,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<FlagMatch>,
) {
    let command_path = path.join(" ");
    for arg in command.get_arguments() {
        if !matcher.matches(arg) {
            continue;
        }
        let long = arg.get_long().map(str::to_owned);
        let short = arg.get_short();
        let key = format!("{command_path}|{}|{:?}", arg.get_id(), short);
        if !seen.insert(key) {
            continue;
        }
        out.push(FlagMatch {
            command_path: command_path.clone(),
            long,
            short,
            aliases: collect_flag_aliases(arg),
            help: arg
                .get_help()
                .map(std::string::ToString::to_string)
                .unwrap_or_default(),
        });
    }

    for child in command.get_subcommands() {
        let mut next_path = path.to_vec();
        next_path.push(child.get_name().to_string());
        collect_flag_matches(child, &next_path, matcher, seen, out);
    }
}

fn render_flag_matches(query: &str, scope: &[String], matches: &[FlagMatch]) -> String {
    let mut out = render_flag_matches_section(query, scope, matches);
    out.push_str("\nUse `nx help <command>` for full command usage.\n");
    out
}

fn render_flag_matches_section(query: &str, scope: &[String], matches: &[FlagMatch]) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "Flag matches for '{query}' within '{}':\n",
        scope.join(" ")
    )
    .expect("writing to String should not fail");

    for item in matches {
        let mut names = Vec::new();
        if let Some(short) = item.short {
            names.push(format!("-{short}"));
        }
        if let Some(long) = &item.long {
            names.push(format!("--{long}"));
        }
        if names.is_empty() {
            continue;
        }
        writeln!(out, "- {}: {}", item.command_path, names.join(", "))
            .expect("writing to String should not fail");
        if !item.aliases.is_empty() {
            writeln!(out, "  aliases: {}", item.aliases.join(", "))
                .expect("writing to String should not fail");
        }
        if !item.help.is_empty() {
            writeln!(out, "  {}", item.help).expect("writing to String should not fail");
        }
    }

    out
}

fn find_command_matches(command: &Command, base_path: &[String], query: &str) -> Vec<CommandMatch> {
    let query_lower = query.to_ascii_lowercase();
    let mut out = Vec::new();
    collect_command_matches(command, base_path, &query_lower, &mut out);
    out
}

fn collect_command_matches(
    command: &Command,
    path: &[String],
    query_lower: &str,
    out: &mut Vec<CommandMatch>,
) {
    for child in command.get_subcommands() {
        let mut child_path = path.to_vec();
        child_path.push(child.get_name().to_string());
        let path_display = child_path.join(" ");
        let aliases: Vec<String> = child.get_all_aliases().map(str::to_owned).collect();
        let about = child
            .get_about()
            .map(std::string::ToString::to_string)
            .unwrap_or_default();

        let name_match = child.get_name().contains(query_lower);
        let alias_match = aliases.iter().any(|alias| alias.contains(query_lower));
        let about_match = about.to_ascii_lowercase().contains(query_lower);
        let path_match = path_display.to_ascii_lowercase().contains(query_lower);
        if name_match || alias_match || about_match || path_match {
            out.push(CommandMatch {
                path: path_display.clone(),
                about: about.clone(),
                aliases: aliases.clone(),
            });
        }

        collect_command_matches(child, &child_path, query_lower, out);
    }
}

fn render_command_matches(query: &str, scope: &[String], matches: &[CommandMatch]) -> String {
    let mut out = render_command_matches_section(query, scope, matches);
    out.push_str("\nUse `nx help <command>` to open a command topic.\n");
    out
}

fn render_command_matches_section(
    query: &str,
    scope: &[String],
    matches: &[CommandMatch],
) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "Command topics matching '{query}' within '{}':\n",
        scope.join(" ")
    )
    .expect("writing to String should not fail");

    for item in matches {
        writeln!(out, "- {}", item.path).expect("writing to String should not fail");
        if !item.about.is_empty() {
            writeln!(out, "  {}", item.about).expect("writing to String should not fail");
        }
        if !item.aliases.is_empty() {
            writeln!(out, "  aliases: {}", item.aliases.join(", "))
                .expect("writing to String should not fail");
        }
    }

    out
}

fn render_combined_matches(
    query: &str,
    scope: &[String],
    command_matches: &[CommandMatch],
    flag_matches: &[FlagMatch],
) -> String {
    let mut out = render_command_matches_section(query, scope, command_matches);
    out.push('\n');
    out.push_str(&render_flag_matches_section(query, scope, flag_matches));
    out.push_str("\nUse `nx help <command>` to open command usage, or `nx help -- <flag>` for exact flag lookup.\n");
    out
}

#[derive(Debug, Clone)]
enum FlagQuery {
    Long(String),
    Short(char),
    Text(String),
}

#[derive(Debug, Clone)]
struct FlagMatcher {
    query: FlagQuery,
}

impl FlagMatcher {
    fn new(raw: &str) -> Self {
        if let Some(long) = raw.strip_prefix("--") {
            return Self {
                query: FlagQuery::Long(long.to_ascii_lowercase()),
            };
        }
        if let Some(short) = raw.strip_prefix('-')
            && short.chars().count() == 1
        {
            return Self {
                query: FlagQuery::Short(short.chars().next().unwrap_or_default()),
            };
        }
        Self {
            query: FlagQuery::Text(raw.to_ascii_lowercase()),
        }
    }

    fn matches(&self, arg: &Arg) -> bool {
        let long = arg.get_long().map(str::to_ascii_lowercase);
        let short = arg.get_short();
        let long_aliases = collect_flag_aliases_lower(arg);
        let short_aliases = arg.get_all_short_aliases().unwrap_or_default();
        if long.is_none() && short.is_none() {
            return false;
        }
        let help = arg
            .get_help()
            .map(|value| value.to_string().to_ascii_lowercase())
            .unwrap_or_default();

        match &self.query {
            FlagQuery::Long(target) => {
                long.as_ref().is_some_and(|value| value == target)
                    || long_aliases.iter().any(|alias| alias == target)
            }
            FlagQuery::Short(target) => {
                short == Some(*target) || short_aliases.iter().any(|alias| alias == target)
            }
            FlagQuery::Text(target) => {
                long.as_ref().is_some_and(|value| value.contains(target))
                    || long_aliases.iter().any(|alias| alias.contains(target))
                    || short
                        .map(|value| value.to_string())
                        .is_some_and(|value| value == *target)
                    || short_aliases
                        .iter()
                        .map(std::string::ToString::to_string)
                        .any(|value| value == *target)
                    || help.contains(target)
            }
        }
    }
}

fn collect_flag_aliases(arg: &Arg) -> Vec<String> {
    let long = arg.get_long();
    let mut aliases: BTreeSet<_> = arg
        .get_all_aliases()
        .unwrap_or_default()
        .into_iter()
        .filter(|alias| Some(*alias) != long)
        .map(str::to_owned)
        .collect();
    aliases.retain(|alias| !alias.is_empty());
    aliases.into_iter().collect()
}

fn collect_flag_aliases_lower(arg: &Arg) -> Vec<String> {
    collect_flag_aliases(arg)
        .into_iter()
        .map(|alias| alias.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_without_topics_renders_root_usage() {
        let text = render_help(&HelpArgs::default()).expect("root help should render");
        assert!(text.contains("Usage: nx [OPTIONS] <COMMAND>"));
    }

    #[test]
    fn help_command_topic_renders_install_help() {
        let text = render_help(&HelpArgs {
            topics: vec!["install".to_string()],
        })
        .expect("install help should render");
        assert!(text.contains("Usage: install"));
        assert!(text.contains("--dry-run"));
    }

    #[test]
    fn help_flag_query_finds_dry_run_matches() {
        let text = render_help(&HelpArgs {
            topics: vec!["dry-run".to_string()],
        })
        .expect("flag query should render");
        assert!(text.contains("--dry-run"));
        assert!(text.contains("nx install"));
        assert!(text.contains("nx remove"));
    }

    #[test]
    fn help_scoped_flag_query_limits_to_subtree() {
        let text = render_help(&HelpArgs {
            topics: vec!["install".to_string(), "--dry-run".to_string()],
        })
        .expect("scoped flag query should render");
        assert!(text.contains("nx install"));
        assert!(!text.contains("nx remove"));
    }

    #[test]
    fn help_unknown_topic_reports_error() {
        let err = render_help(&HelpArgs {
            topics: vec!["definitely-unknown-topic".to_string()],
        })
        .expect_err("unknown topic should error");
        assert!(err.contains("No help topic matched"));
    }

    #[test]
    fn help_flag_query_matches_visible_aliases() {
        let text = render_help(&HelpArgs {
            topics: vec!["--key".to_string()],
        })
        .expect("alias flag query should render");
        assert!(text.contains("nx secret add"));
        assert!(text.contains("--name"));
        assert!(text.contains("aliases: key"));
    }

    #[test]
    fn help_ambiguous_query_prioritizes_commands_before_flags() {
        let text = render_help(&HelpArgs {
            topics: vec!["sec".to_string()],
        })
        .expect("ambiguous query should render");
        let command_index = text
            .find("Command topics matching 'sec'")
            .expect("command section should render");
        let flag_index = text
            .find("Flag matches for 'sec'")
            .expect("flag section should render");
        assert!(command_index < flag_index);
        assert!(text.contains("nx secret"));
    }
}
