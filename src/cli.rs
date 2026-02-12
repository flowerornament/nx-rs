use std::ffi::OsString;

use clap::{Parser, Subcommand};

const KNOWN_COMMANDS: &[&str] = &[
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
];

#[derive(Debug, Clone, Parser)]
#[command(
    name = "nx",
    about = "Multi-source package installer for nix-darwin",
    disable_help_subcommand = true,
    arg_required_else_help = true
)]
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
    Install(InstallArgs),
    #[command(alias = "rm")]
    Remove(RemoveArgs),
    Where(WhereArgs),
    List(ListArgs),
    Info(InfoArgs),
    Status,
    Installed(InstalledArgs),
    Undo,
    Update(PassthroughArgs),
    Test,
    Rebuild(PassthroughArgs),
    Upgrade(UpgradeArgs),
}

#[derive(Debug, Clone, Parser)]
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
    let should_insert_install =
        !first.starts_with('-') && !KNOWN_COMMANDS.contains(&first.as_ref());
    if should_insert_install {
        out.insert(1, OsString::from("install"));
    }

    out
}
