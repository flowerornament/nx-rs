use std::ffi::OsString;

use clap::{Parser, Subcommand};

const KNOWN_COMMANDS: &[&str] = &[
    "install",
    "remove",
    "rm",
    "uninstall",
    "secret",
    "secrets",
    "search",
    "where",
    "list",
    "info",
    "status",
    "installed",
    "undo",
    "update",
    "test",
    "rebuild",
    "upgrade",
];

const ROOT_HELP: &str = "Run `nx <command> --help` for command-specific usage and examples.";
const SECRET_HELP: &str = "Examples:\n  nx secret add redacted_api_key --value '<token>'\n  printf '%s' '<token>' | nx secret add redacted_api_key --value-stdin";
const SECRET_ADD_HELP: &str = "Examples:
  nx secret add redacted_api_key --value '<token>'
  nx secret add --name redacted_api_key --value '<token>'
  printf '%s' '<token>' | nx secret add redacted_api_key --value-stdin

Notes:
  - `--` stops option parsing; do not put it before `--name` or `--value`.
  - Prefer `--value-stdin` for sensitive values to avoid shell history leaks.";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "nx",
    about = "Multi-source package installer for nix-darwin",
    disable_help_subcommand = true,
    arg_required_else_help = true,
    after_long_help = ROOT_HELP
)]
#[allow(clippy::struct_excessive_bools)] // CLI flag surface intentionally mirrors SPEC switches.
pub struct Cli {
    #[arg(long, global = true, help = "Use plain output formatting")]
    pub plain: bool,
    #[arg(long, global = true, help = "Force Unicode/emoji output")]
    pub unicode: bool,
    #[arg(long, global = true, help = "Minimal output (less context)")]
    pub minimal: bool,
    #[arg(long, short = 'v', global = true, help = "Verbose output")]
    pub verbose: bool,
    #[arg(long, global = true, help = "JSON output when supported")]
    pub json: bool,
    #[command(subcommand)]
    pub command: CommandKind,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CommandKind {
    #[command(about = "Install package(s) into nix config")]
    Install(InstallArgs),
    #[command(alias = "rm", alias = "uninstall")]
    #[command(about = "Remove package(s) from nix config")]
    Remove(RemoveArgs),
    #[command(alias = "secrets")]
    #[command(about = "Manage encrypted secrets via sops")]
    Secret(SecretArgs),
    #[command(about = "Search package sources without installing")]
    Search(SearchArgs),
    #[command(about = "Show where a package is declared")]
    Where(WhereArgs),
    #[command(about = "List installed packages by source")]
    List(ListArgs),
    #[command(about = "Show package metadata and source candidates")]
    Info(InfoArgs),
    #[command(about = "Show package distribution summary")]
    Status,
    #[command(about = "Check whether package(s) are installed")]
    Installed(InstalledArgs),
    #[command(about = "Revert modified tracked files via git checkout")]
    Undo,
    #[command(about = "Run nix flake update")]
    Update(PassthroughArgs),
    #[command(about = "Run repo quality checks")]
    Test,
    #[command(about = "Run darwin-rebuild switch with preflight checks")]
    Rebuild(PassthroughArgs),
    #[command(about = "Run full upgrade flow (flake, brew, rebuild, commit)")]
    Upgrade(UpgradeArgs),
}

#[derive(Debug, Clone, Parser)]
#[allow(clippy::struct_excessive_bools)] // Install contract is flag-rich by design.
pub struct InstallArgs {
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,
    #[arg(long, short = 'y')]
    pub yes: bool,
    #[arg(long, short = 'n')]
    pub dry_run: bool,
    #[arg(long)]
    pub cask: bool,
    #[arg(long)]
    pub mas: bool,
    #[arg(long)]
    pub service: bool,
    #[arg(long)]
    pub rebuild: bool,
    #[arg(long)]
    pub bleeding_edge: bool,
    #[arg(long)]
    pub nur: bool,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub explain: bool,
    #[arg(long)]
    pub engine: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Parser)]
pub struct SearchArgs {
    #[arg(value_name = "PACKAGE")]
    pub package: String,
    #[arg(long)]
    pub bleeding_edge: bool,
    #[arg(long)]
    pub nur: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct RemoveArgs {
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,
    #[arg(long, short = 'y')]
    pub yes: bool,
    #[arg(long, short = 'n')]
    pub dry_run: bool,
    #[arg(long)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Parser)]
#[command(
    about = "Manage encrypted secrets via sops",
    after_long_help = SECRET_HELP
)]
pub struct SecretArgs {
    #[command(subcommand)]
    pub command: SecretCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SecretCommand {
    #[command(about = "Add or update a secret key/value")]
    Add(SecretAddArgs),
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = SECRET_ADD_HELP)]
pub struct SecretAddArgs {
    #[arg(
        value_name = "KEY",
        help = "Secret key name (lowercase letters, digits, underscores)",
        required_unless_present = "name",
        conflicts_with = "name"
    )]
    pub key: Option<String>,
    #[arg(
        long,
        visible_alias = "key",
        value_name = "KEY",
        help = "Secret key name (alternative to positional KEY)",
        required_unless_present = "key",
        conflicts_with = "key"
    )]
    pub name: Option<String>,
    #[arg(
        long,
        value_name = "VALUE",
        help = "Secret value passed directly as an argument (prefer --value-stdin)",
        required_unless_present = "value_stdin",
        conflicts_with = "value_stdin"
    )]
    pub value: Option<String>,
    #[arg(
        long,
        help = "Read secret value from stdin",
        required_unless_present = "value",
        conflicts_with = "value"
    )]
    pub value_stdin: bool,
}

impl SecretAddArgs {
    #[must_use]
    pub fn key_name(&self) -> &str {
        self.key
            .as_deref()
            .or(self.name.as_deref())
            .expect("clap enforces required secret key")
    }
}

#[derive(Debug, Clone, Parser)]
pub struct WhereArgs {
    #[arg(value_name = "PACKAGE")]
    pub package: Option<String>,
}

#[derive(Debug, Clone, Parser)]
pub struct ListArgs {
    #[arg(value_name = "SOURCE")]
    pub source: Option<String>,
    #[arg(long)]
    pub verbose: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub plain: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct InfoArgs {
    #[arg(value_name = "PACKAGE")]
    pub package: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub bleeding_edge: bool,
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct InstalledArgs {
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub show_location: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct PassthroughArgs {
    #[arg(last = true)]
    pub passthrough: Vec<String>,
}

#[derive(Debug, Clone, Parser)]
#[allow(clippy::struct_excessive_bools)] // Upgrade command intentionally exposes multiple independent toggles.
pub struct UpgradeArgs {
    #[arg(long, short = 'n')]
    pub dry_run: bool,
    #[arg(long, short = 'v')]
    pub verbose: bool,
    #[arg(long)]
    pub skip_rebuild: bool,
    #[arg(long)]
    pub skip_commit: bool,
    #[arg(long)]
    pub skip_brew: bool,
    #[arg(long)]
    pub no_ai: bool,
    #[arg(last = true)]
    pub passthrough: Vec<String>,
}

pub fn preprocess_args<I, T>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut out: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if out.len() < 2 {
        return out;
    }

    let first = out[1].to_string_lossy();
    if first.starts_with('-') || KNOWN_COMMANDS.contains(&first.as_ref()) {
        return out;
    }

    out.insert(1, OsString::from("install"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    // --- preprocess_args ---

    #[test]
    fn preprocess_args_typo_like_token_inserts_install() {
        let result = preprocess_args(["nx", "upgade", "--dry-run"]);
        assert_eq!(result[1], OsString::from("install"));
        assert_eq!(result[2], OsString::from("upgade"));
        assert_eq!(result[3], OsString::from("--dry-run"));
    }

    #[test]
    fn preprocess_args_package_name_inserts_install() {
        let result = preprocess_args(["nx", "ripgrep"]);
        assert_eq!(result[1], OsString::from("install"));
        assert_eq!(result[2], OsString::from("ripgrep"));
    }

    #[test]
    fn preprocess_args_known_command_passes_through() {
        let result = preprocess_args(["nx", "rebuild"]);
        assert_eq!(result[1], OsString::from("rebuild"));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn preprocess_args_secret_alias_passes_through() {
        let result = preprocess_args(["nx", "secrets"]);
        assert_eq!(result[1], OsString::from("secrets"));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn preprocess_args_flag_passes_through() {
        let result = preprocess_args(["nx", "--help"]);
        assert_eq!(result[1], OsString::from("--help"));
    }

    #[test]
    fn secret_add_parses_positional_key() {
        let cli = Cli::try_parse_from(["nx", "secret", "add", "redacted_api_key", "--value", "v"])
            .expect("secret add should parse with positional key");
        let CommandKind::Secret(SecretArgs {
            command: SecretCommand::Add(add_args),
        }) = cli.command
        else {
            panic!("expected secret command");
        };
        assert_eq!(add_args.key_name(), "redacted_api_key");
    }

    #[test]
    fn secret_add_parses_name_flag_key() {
        let cli = Cli::try_parse_from([
            "nx",
            "secret",
            "add",
            "--name",
            "redacted_api_key",
            "--value",
            "v",
        ])
        .expect("secret add should parse with --name");
        let CommandKind::Secret(SecretArgs {
            command: SecretCommand::Add(add_args),
        }) = cli.command
        else {
            panic!("expected secret command");
        };
        assert_eq!(add_args.key_name(), "redacted_api_key");
    }

    #[test]
    fn secret_add_parses_key_alias_flag() {
        let cli = Cli::try_parse_from([
            "nx",
            "secret",
            "add",
            "--key",
            "redacted_api_key",
            "--value",
            "v",
        ])
        .expect("secret add should parse with --key alias");
        let CommandKind::Secret(SecretArgs {
            command: SecretCommand::Add(add_args),
        }) = cli.command
        else {
            panic!("expected secret command");
        };
        assert_eq!(add_args.key_name(), "redacted_api_key");
    }

    #[test]
    fn secret_add_help_includes_examples_and_double_dash_note() {
        let mut cmd = Cli::command();
        let mut help = Vec::<u8>::new();
        cmd.write_long_help(&mut help)
            .expect("root help should render");
        let help = String::from_utf8(help).expect("help should be utf8");
        assert!(help.contains("Run `nx <command> --help`"));

        let mut secret_add_cmd = Cli::command();
        let secret_add = secret_add_cmd
            .find_subcommand_mut("secret")
            .expect("secret command should exist")
            .find_subcommand_mut("add")
            .expect("secret add command should exist");
        let mut add_help = Vec::<u8>::new();
        secret_add
            .write_long_help(&mut add_help)
            .expect("secret add help should render");
        let add_help = String::from_utf8(add_help).expect("help should be utf8");
        assert!(add_help.contains("nx secret add --name redacted_api_key"));
        assert!(add_help.contains("`--` stops option parsing"));
    }
}
