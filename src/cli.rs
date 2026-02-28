use std::ffi::OsString;

use clap::{Args, Parser, Subcommand};

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
pub struct Cli {
    #[command(flatten)]
    pub style: GlobalStyleArgs,
    #[command(flatten)]
    pub output: GlobalOutputArgs,
    #[command(subcommand)]
    pub command: CommandKind,
}

impl Cli {
    #[must_use]
    pub const fn plain(&self) -> bool {
        self.style.plain
    }

    #[must_use]
    pub const fn unicode(&self) -> bool {
        self.style.unicode
    }

    #[must_use]
    pub const fn minimal(&self) -> bool {
        self.style.minimal
    }

    #[must_use]
    pub const fn json(&self) -> bool {
        self.output.json
    }

    #[must_use]
    pub const fn verbose_requested(&self) -> bool {
        self.output.verbose
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct GlobalStyleArgs {
    #[arg(long, global = true, help = "Use plain output formatting")]
    pub plain: bool,
    #[arg(long, global = true, help = "Force Unicode/emoji output")]
    pub unicode: bool,
    #[arg(long, global = true, help = "Minimal output (less context)")]
    pub minimal: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct GlobalOutputArgs {
    #[arg(long, short = 'v', global = true, help = "Verbose output")]
    pub verbose: bool,
    #[arg(long, global = true, help = "JSON output when supported")]
    pub json: bool,
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

#[derive(Debug, Clone, Parser, Default)]
pub struct InstallArgs {
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,
    #[command(flatten)]
    pub flow: InstallFlowArgs,
    #[command(flatten)]
    pub target: InstallTargetArgs,
    #[command(flatten)]
    pub source: InstallSourceArgs,
    #[arg(long)]
    pub service: bool,
    #[command(flatten)]
    pub ai: InstallAiArgs,
}

impl InstallArgs {
    #[must_use]
    pub const fn yes(&self) -> bool {
        self.flow.yes
    }

    #[must_use]
    pub const fn dry_run(&self) -> bool {
        self.flow.dry_run
    }

    #[must_use]
    pub const fn rebuild(&self) -> bool {
        self.flow.rebuild
    }

    #[must_use]
    pub const fn cask(&self) -> bool {
        self.target.cask
    }

    #[must_use]
    pub const fn mas(&self) -> bool {
        self.target.mas
    }

    #[must_use]
    pub const fn bleeding_edge(&self) -> bool {
        self.source.bleeding_edge
    }

    #[must_use]
    pub const fn nur(&self) -> bool {
        self.source.nur
    }

    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.source.source.as_deref()
    }

    #[must_use]
    pub const fn service(&self) -> bool {
        self.service
    }

    #[must_use]
    pub const fn explain(&self) -> bool {
        self.ai.explain
    }

    #[must_use]
    pub fn engine(&self) -> Option<&str> {
        self.ai.engine.as_deref()
    }

    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.ai.model.as_deref()
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct InstallFlowArgs {
    #[arg(long, short = 'y')]
    pub yes: bool,
    #[arg(long, short = 'n')]
    pub dry_run: bool,
    #[arg(long)]
    pub rebuild: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct InstallTargetArgs {
    #[arg(long)]
    pub cask: bool,
    #[arg(long)]
    pub mas: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct InstallSourceArgs {
    #[arg(long)]
    pub bleeding_edge: bool,
    #[arg(long)]
    pub nur: bool,
    #[arg(long)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct InstallAiArgs {
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
pub struct UpgradeArgs {
    #[command(flatten)]
    pub flow: UpgradeFlowArgs,
    #[command(flatten)]
    pub skip: UpgradeSkipArgs,
    #[arg(last = true)]
    pub passthrough: Vec<String>,
}

impl UpgradeArgs {
    #[must_use]
    pub const fn dry_run(&self) -> bool {
        self.flow.dry_run
    }

    #[must_use]
    pub const fn no_ai(&self) -> bool {
        self.flow.no_ai
    }

    #[must_use]
    pub const fn skip_rebuild(&self) -> bool {
        self.skip.skip_rebuild
    }

    #[must_use]
    pub const fn skip_commit(&self) -> bool {
        self.skip.skip_commit
    }

    #[must_use]
    pub const fn skip_brew(&self) -> bool {
        self.skip.skip_brew
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct UpgradeFlowArgs {
    #[arg(long, short = 'n')]
    pub dry_run: bool,
    #[arg(long, short = 'v')]
    pub verbose: bool,
    #[arg(long)]
    pub no_ai: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct UpgradeSkipArgs {
    #[arg(long)]
    pub skip_rebuild: bool,
    #[arg(long)]
    pub skip_commit: bool,
    #[arg(long)]
    pub skip_brew: bool,
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
    use std::collections::BTreeSet;

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
    fn preprocess_args_search_command_passes_through() {
        let result = preprocess_args(["nx", "search", "ripgrep"]);
        assert_eq!(result[1], OsString::from("search"));
        assert_eq!(result[2], OsString::from("ripgrep"));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn preprocess_args_uninstall_alias_passes_through() {
        let result = preprocess_args(["nx", "uninstall", "ripgrep"]);
        assert_eq!(result[1], OsString::from("uninstall"));
        assert_eq!(result[2], OsString::from("ripgrep"));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn preprocess_args_secret_alias_passes_through() {
        let result = preprocess_args(["nx", "secrets"]);
        assert_eq!(result[1], OsString::from("secrets"));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn known_commands_match_spec_plus_intentional_extensions() {
        let spec_commands: BTreeSet<_> = [
            "install",
            "remove",
            "rm",
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
        ]
        .into_iter()
        .collect();
        let known_commands: BTreeSet<_> = KNOWN_COMMANDS.iter().copied().collect();

        assert!(spec_commands.is_subset(&known_commands));

        let extensions: BTreeSet<_> = known_commands.difference(&spec_commands).copied().collect();
        let expected_extensions: BTreeSet<_> = ["search", "secret", "secrets", "uninstall"]
            .into_iter()
            .collect();
        assert_eq!(extensions, expected_extensions);
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

    #[test]
    fn global_json_and_unicode_flags_parse_at_root() {
        let cli =
            Cli::try_parse_from(["nx", "--json", "--unicode", "info", "ripgrep"]).expect("parse");
        assert!(cli.json());
        assert!(cli.unicode());
    }

    #[test]
    fn global_verbose_flag_parses_at_root() {
        let cli = Cli::try_parse_from(["nx", "--verbose", "status"]).expect("parse");
        assert!(cli.verbose_requested());
    }

    #[test]
    fn install_parses_explain_engine_and_model_options() {
        let cli = Cli::try_parse_from([
            "nx",
            "install",
            "ripgrep",
            "--explain",
            "--engine",
            "claude",
            "--model",
            "sonnet",
        ])
        .expect("parse install flags");

        let CommandKind::Install(args) = cli.command else {
            panic!("expected install command");
        };
        assert!(args.explain());
        assert_eq!(args.engine(), Some("claude"));
        assert_eq!(args.model(), Some("sonnet"));
    }

    #[test]
    fn remove_parses_model_option() {
        let cli = Cli::try_parse_from(["nx", "remove", "ripgrep", "--model", "sonnet"])
            .expect("parse remove model");
        let CommandKind::Remove(args) = cli.command else {
            panic!("expected remove command");
        };
        assert_eq!(args.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn list_parses_verbose_option() {
        let cli = Cli::try_parse_from(["nx", "list", "--verbose"]).expect("parse list verbose");
        let CommandKind::List(args) = cli.command else {
            panic!("expected list command");
        };
        assert!(args.verbose);
    }

    #[test]
    fn info_parses_verbose_option() {
        let cli = Cli::try_parse_from(["nx", "info", "ripgrep", "--verbose"])
            .expect("parse info verbose");
        let CommandKind::Info(args) = cli.command else {
            panic!("expected info command");
        };
        assert!(args.verbose);
    }

    #[test]
    fn upgrade_parses_verbose_option() {
        let cli =
            Cli::try_parse_from(["nx", "upgrade", "--verbose"]).expect("parse upgrade verbose");
        let CommandKind::Upgrade(args) = cli.command else {
            panic!("expected upgrade command");
        };
        assert!(args.flow.verbose);
    }

    #[test]
    fn install_help_lists_spec_options_after_flatten_refactor() {
        let mut root = Cli::command();
        let install = root
            .find_subcommand_mut("install")
            .expect("install command should exist");
        let mut help = Vec::<u8>::new();
        install
            .write_long_help(&mut help)
            .expect("install help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        for flag in [
            "--yes",
            "--dry-run",
            "--cask",
            "--mas",
            "--service",
            "--rebuild",
            "--bleeding-edge",
            "--nur",
            "--source",
            "--explain",
            "--engine",
            "--model",
        ] {
            assert!(
                help.contains(flag),
                "install help should contain flag {flag}"
            );
        }
    }

    #[test]
    fn upgrade_help_lists_spec_options_after_flatten_refactor() {
        let mut root = Cli::command();
        let upgrade = root
            .find_subcommand_mut("upgrade")
            .expect("upgrade command should exist");
        let mut help = Vec::<u8>::new();
        upgrade
            .write_long_help(&mut help)
            .expect("upgrade help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        for flag in [
            "--dry-run",
            "--verbose",
            "--skip-rebuild",
            "--skip-commit",
            "--skip-brew",
            "--no-ai",
        ] {
            assert!(
                help.contains(flag),
                "upgrade help should contain flag {flag}"
            );
        }
    }
}
