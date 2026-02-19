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

#[derive(Debug, Clone, Parser)]
#[command(
    name = "nx",
    about = "Multi-source package installer for nix-darwin",
    disable_help_subcommand = true,
    arg_required_else_help = true
)]
#[allow(clippy::struct_excessive_bools)] // CLI flag surface intentionally mirrors SPEC switches.
pub struct Cli {
    #[arg(long, global = true)]
    pub plain: bool,
    #[arg(long, global = true)]
    pub unicode: bool,
    #[arg(long, global = true)]
    pub minimal: bool,
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
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
#[command(about = "Manage encrypted secrets via sops")]
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
pub struct SecretAddArgs {
    #[arg(
        value_name = "KEY",
        help = "Secret key name (lowercase letters, digits, underscores)"
    )]
    pub key: String,
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

/// Levenshtein edit distance between two strings (two-row DP).
fn levenshtein(a: &str, b: &str) -> usize {
    let b_len = b.chars().count();
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

/// Find the closest known command within edit distance <= 2.
fn closest_command(input: &str) -> Option<&'static str> {
    KNOWN_COMMANDS
        .iter()
        .filter_map(|&cmd| {
            let d = levenshtein(input, cmd);
            (d > 0 && d <= 2).then_some((d, cmd))
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, cmd)| cmd)
}

pub fn preprocess_args<I, T>(args: I) -> Result<Vec<OsString>, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut out: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if out.len() < 2 {
        return Ok(out);
    }

    let first = out[1].to_string_lossy();
    if first.starts_with('-') || KNOWN_COMMANDS.contains(&first.as_ref()) {
        return Ok(out);
    }

    // Not a known command — check for typo before treating as package name
    if let Some(suggestion) = closest_command(&first) {
        return Err(format!(
            "unknown command '{first}'. Did you mean '{suggestion}'?"
        ));
    }

    out.insert(1, OsString::from("install"));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- levenshtein ---

    #[test]
    fn levenshtein_identical() {
        assert_eq!(levenshtein("rebuild", "rebuild"), 0);
    }

    #[test]
    fn levenshtein_single_insert() {
        assert_eq!(levenshtein("rebild", "rebuild"), 1);
    }

    #[test]
    fn levenshtein_double_insert() {
        assert_eq!(levenshtein("rebuiild", "rebuild"), 1);
    }

    #[test]
    fn levenshtein_swap() {
        assert_eq!(levenshtein("upgade", "upgrade"), 1);
    }

    #[test]
    fn levenshtein_distant() {
        assert!(levenshtein("xyz", "rebuild") > 2);
    }

    // --- closest_command ---

    #[test]
    fn closest_command_finds_rebuild() {
        assert_eq!(closest_command("rebuiild"), Some("rebuild"));
    }

    #[test]
    fn closest_command_finds_upgrade() {
        assert_eq!(closest_command("upgade"), Some("upgrade"));
    }

    #[test]
    fn closest_command_rejects_distant() {
        assert_eq!(closest_command("ripgrep"), None);
    }

    #[test]
    fn closest_command_rejects_exact() {
        // Exact match should not fire (distance == 0)
        assert_eq!(closest_command("rebuild"), None);
    }

    // --- preprocess_args ---

    #[test]
    fn preprocess_args_typo_errors() {
        let result = preprocess_args(["nx", "rebuiild"]);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("rebuiild"), "error should contain typo");
        assert!(msg.contains("rebuild"), "error should suggest correction");
    }

    #[test]
    fn preprocess_args_package_name_inserts_install() {
        let result = preprocess_args(["nx", "ripgrep"]).unwrap();
        assert_eq!(result[1], OsString::from("install"));
        assert_eq!(result[2], OsString::from("ripgrep"));
    }

    #[test]
    fn preprocess_args_known_command_passes_through() {
        let result = preprocess_args(["nx", "rebuild"]).unwrap();
        assert_eq!(result[1], OsString::from("rebuild"));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn preprocess_args_secret_alias_passes_through() {
        let result = preprocess_args(["nx", "secrets"]).unwrap();
        assert_eq!(result[1], OsString::from("secrets"));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn preprocess_args_flag_passes_through() {
        let result = preprocess_args(["nx", "--help"]).unwrap();
        assert_eq!(result[1], OsString::from("--help"));
    }
}
