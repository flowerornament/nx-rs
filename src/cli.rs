use std::ffi::OsString;

use clap::{Args, Parser, Subcommand, ValueEnum};

const KNOWN_COMMANDS: &[&str] = &[
    "version",
    "help",
    "completion",
    "doctor",
    "init",
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
    "profile",
    "lint",
    "undo",
    "update",
    "test",
    "rebuild",
    "upgrade",
    "generations",
];

const ROOT_HELP: &str = "Run `nx help <topic>` for hierarchical help, or `nx <command> --help` for full command docs.\n\nExamples:\n  nx version\n  nx doctor\n  nx help install\n  nx upgrade --dry-run\n  nx completion zsh";
const VERSION_HELP: &str = "Examples:\n  nx --version\n  nx -V\n  nx version\n  nx version --json";
const COMPLETION_HELP: &str = "Examples:\n  nx completion zsh > ~/.zsh/completions/_nx\n  nx completion bash > /usr/local/etc/bash_completion.d/nx\n\nNotes:\n  - Completion scripts are written to stdout.\n  - Re-run this after upgrading nx if completions drift.";
const DOCTOR_HELP: &str = "Examples:\n  nx doctor\n  nx doctor --verbose\n  nx doctor --json\n\nNotes:\n  - `doctor` is repo-scoped and expects to run inside a managed nix config repo.\n  - Use it when repo discovery, manifest health, routing, cache, or tool availability seem suspicious.";
const INIT_HELP: &str = "Examples:\n  nx init\n  nx init --refresh\n\nNotes:\n  - `init` scans the repo and writes `.nx/manifest.toml`.\n  - `--refresh` re-scans and merges with the existing manifest when possible.";
const INSTALL_HELP: &str = "Examples:\n  nx ripgrep\n  nx install ripgrep fd\n  nx install firefox --cask\n  nx install pyyaml --verbose\n\nNotes:\n  - Bare tokens like `nx ripgrep` are treated as `nx install ripgrep`.\n  - `--verbose` surfaces cache and query timing details during package resolution.";
const REMOVE_HELP: &str = "Examples:\n  nx remove ripgrep\n  nx rm firefox --dry-run\n  nx uninstall ripgrep --yes\n\nNotes:\n  - `rm` and `uninstall` are aliases of `remove`.\n  - `--dry-run` previews file edits without writing them.";
const SEARCH_HELP: &str = "Examples:\n  nx search ripgrep\n  nx search ripgrep --nur\n  nx search ripgrep --source homebrew --json\n  nx search ripgrep --verbose\n\nNotes:\n  - `search` never edits the repo.\n  - `--verbose` surfaces cache state, backend availability, and timing details.";
const WHERE_HELP: &str = "Examples:\n  nx where ripgrep\n\nNotes:\n  - `where` shows the owning file and snippet for an installed declaration.";
const LIST_HELP: &str = "Examples:\n  nx list\n  nx list nix --verbose\n  nx list homebrew --json\n\nNotes:\n  - Source filters are `nix`, `homebrew`, and `mas`.";
const INFO_HELP: &str = "Examples:\n  nx info ripgrep\n  nx info ripgrep --nur\n  nx info ripgrep --source homebrew\n  nx info ripgrep --verbose\n\nNotes:\n  - `info` shares the package-query pipeline with `search` and install resolution.\n  - `--verbose` includes query diagnostics in addition to package metadata.";
const STATUS_HELP: &str = "Examples:\n  nx status\n  nx status --json\n\nNotes:\n  - `status` is a read-only package distribution summary for the managed repo.";
const INSTALLED_HELP: &str = "Examples:\n  nx installed ripgrep fd\n  nx installed ripgrep --show-location\n  nx installed ripgrep fd --json\n\nNotes:\n  - Exit status is success only when every requested package is installed.";
const PROFILE_HELP: &str = "Examples:\n  nx profile\n  nx profile --limit 20\n  nx profile --json\n\nNotes:\n  - `profile` reads local rebuild timing records from ~/.local/state/nx/timings.jsonl.\n  - Set NX_PROFILE_PATH to override the timing file location.";
const LINT_HELP: &str = "Examples:\n  nx lint\n  nx lint --json\n\nNotes:\n  - `lint` checks first-line `# nx:` routing metadata and routing keyword overlap.";
const UNDO_HELP: &str = "Examples:\n  nx undo\n  nx undo --yes\n\nNotes:\n  - `undo` reverts modified tracked files via git checkout and prompts by default.";
const UPDATE_HELP: &str = "Examples:\n  nx update\n  nx update -- --commit-lock-file\n  nx update -- --flake ./hosts/macbook\n\nNotes:\n  - Additional args after `--` are passed directly to `nix flake update`.";
const TEST_HELP: &str =
    "Examples:\n  nx test\n\nNotes:\n  - `test` runs the managed repo quality gate (`just ci`).";
const REBUILD_HELP: &str = "Examples:\n  nx rebuild\n  nx rebuild --preflight\n  nx rebuild --timing\n  nx rebuild -- --show-trace\n\nNotes:\n  - `--preflight` stops after lint, git, and flake checks.\n  - Rebuild timings are recorded locally and can be reviewed with `nx profile`.\n  - Darwin repos can opt into split rebuilds with `platform.split_rebuild = true` or NX_SPLIT_DARWIN=1.\n  - Additional args after `--` are passed directly to `darwin-rebuild switch`.";
const GENERATIONS_HELP: &str = "Examples:\n  nx generations status\n  nx generations plan\n  nx generations prune --dry-run\n  nx generations prune --keep 5 --kind darwin\n\nNotes:\n  - `nx generations` is host-scoped and works from any directory.\n  - Use `plan` or `prune --dry-run` to preview exact prune/GC commands.";
const GENERATIONS_PRUNE_HELP: &str = "Examples:\n  nx generations prune --dry-run\n  nx generations prune --yes\n  nx generations prune --keep 5 --kind darwin\n  nx generations prune --kind home-manager --no-gc\n\nNotes:\n  - `--dry-run` renders the same plan as `nx generations plan`.\n  - By default, `prune` asks for confirmation before mutating the host.";
const UPGRADE_HELP: &str = "Examples:\n  nx upgrade\n  nx upgrade --dry-run\n  nx upgrade nx-rs\n  nx upgrade nx-rs anneal -- --show-trace\n\nNotes:\n  - Without positional inputs, `upgrade` runs the full repo-wide flow: flake update, brew, rebuild, and commit.\n  - With positional inputs, `upgrade` updates only those flake inputs and skips the brew phase by default.";
const SECRET_HELP: &str = "Examples:\n  nx secret add example_secret_key --value '<token>'\n  printf '%s' '<token>' | nx secret add example_secret_key --value-stdin";
const SECRET_ADD_HELP: &str = "Examples:
  nx secret add example_secret_key --value '<token>'
  nx secret add --name example_secret_key --value '<token>'
  printf '%s' '<token>' | nx secret add example_secret_key --value-stdin

Notes:
  - `--` stops option parsing; do not put it before `--name` or `--value`.
  - Prefer `--value-stdin` for sensitive values to avoid shell history leaks.";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "nx",
    about = "CLI for managing Nix config repos and host generations",
    version,
    disable_help_subcommand = true,
    arg_required_else_help = true,
    after_long_help = ROOT_HELP
)]
pub struct Cli {
    #[command(flatten)]
    pub style: GlobalStyleArgs,
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

#[derive(Debug, Clone, Subcommand)]
pub enum CommandKind {
    #[command(about = "Show nx version information")]
    Version(VersionArgs),
    #[command(about = "Show hierarchical help for commands and flags")]
    Help(HelpArgs),
    #[command(about = "Generate shell completion scripts")]
    Completion(CompletionArgs),
    #[command(about = "Diagnose repo and host prerequisites")]
    Doctor(DoctorArgs),
    #[command(about = "Scan repo and generate .nx/manifest.toml")]
    Init(InitArgs),
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
    Status(StatusArgs),
    #[command(about = "Check whether package(s) are installed")]
    Installed(InstalledArgs),
    #[command(about = "Show recent local rebuild timings")]
    Profile(ProfileArgs),
    #[command(about = "Check nx routing annotations and keyword conflicts")]
    Lint(LintArgs),
    #[command(about = "Revert modified tracked files via git checkout")]
    Undo(UndoArgs),
    #[command(about = "Run nix flake update")]
    Update(UpdateArgs),
    #[command(about = "Run repo quality checks")]
    Test(TestArgs),
    #[command(about = "Run darwin-rebuild switch with preflight checks")]
    Rebuild(RebuildArgs),
    #[command(about = "Run repo-wide or targeted flake upgrade flows")]
    Upgrade(UpgradeArgs),
    #[command(about = "Inspect and manage host Nix generations")]
    Generations(GenerationsArgs),
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = VERSION_HELP)]
pub struct VersionArgs {
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompletionShellArg {
    Bash,
    Elvish,
    Fish,
    #[value(name = "powershell")]
    PowerShell,
    Zsh,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = COMPLETION_HELP)]
pub struct CompletionArgs {
    #[arg(
        value_enum,
        value_name = "SHELL",
        help = "Shell to generate completions for"
    )]
    pub shell: CompletionShellArg,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = DOCTOR_HELP)]
pub struct DoctorArgs {
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
    #[arg(long, short = 'v', help = "Show additional diagnostic detail")]
    pub verbose: bool,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = GENERATIONS_HELP)]
pub struct GenerationsArgs {
    #[arg(long, global = true, help = "Emit machine-readable JSON output")]
    pub json: bool,
    #[command(subcommand)]
    pub command: GenerationsCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum GenerationsCommand {
    #[command(about = "Show generation families and the active retention policy")]
    Status(GenerationsStatusArgs),
    #[command(about = "Render the retention and execution plan without mutating the host")]
    Plan(GenerationsPlanArgs),
    #[command(about = "Prune generations and optionally run garbage collection")]
    Prune(GenerationsPruneArgs),
}

#[derive(Debug, Clone, Args)]
pub struct GenerationsStatusArgs {
    #[command(flatten)]
    pub policy: GenerationsPolicyArgs,
}

#[derive(Debug, Clone, Args)]
pub struct GenerationsPlanArgs {
    #[command(flatten)]
    pub policy: GenerationsPolicyArgs,
    #[arg(long, help = "Skip garbage collection in the rendered plan")]
    pub no_gc: bool,
}

#[derive(Debug, Clone, Args)]
#[command(after_long_help = GENERATIONS_PRUNE_HELP)]
pub struct GenerationsPruneArgs {
    #[command(flatten)]
    pub policy: GenerationsPolicyArgs,
    #[arg(long, help = "Skip garbage collection after pruning")]
    pub no_gc: bool,
    #[arg(long, short = 'y', help = "Skip confirmation prompts")]
    pub yes: bool,
    #[arg(
        long,
        short = 'n',
        help = "Preview the prune plan without mutating anything"
    )]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args)]
pub struct GenerationsPolicyArgs {
    #[arg(
        long,
        value_name = "N",
        default_value_t = 10,
        help = "Keep the newest N generations"
    )]
    pub keep: usize,
    #[arg(long, value_enum, default_value_t = GenerationKindArg::All, help = "Generation families to include")]
    pub kind: GenerationKindArg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GenerationKindArg {
    All,
    Darwin,
    HomeManager,
}

#[derive(Debug, Clone, Parser, Default)]
#[command(after_long_help = INSTALL_HELP)]
pub struct InstallArgs {
    #[arg(
        value_name = "PACKAGES",
        required = true,
        num_args = 1..,
        help = "Package names/attributes to install"
    )]
    pub packages: Vec<String>,
    #[command(flatten)]
    pub flow: InstallFlowArgs,
    #[command(flatten)]
    pub target: InstallTargetArgs,
    #[command(flatten)]
    pub source: PackageSourceArgs,
    #[arg(long, help = "Offer to scaffold a service definition after install")]
    pub service: bool,
    #[command(flatten)]
    pub ai: InstallAiArgs,
    #[arg(long, short = 'v', help = "Show cache and query timing diagnostics")]
    pub verbose: bool,
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

    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.verbose
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct InstallFlowArgs {
    #[arg(long, short = 'y', help = "Skip prompts and accept defaults")]
    pub yes: bool,
    #[arg(long, short = 'n', help = "Preview changes without writing files")]
    pub dry_run: bool,
    #[arg(long, help = "Run rebuild after successful installs")]
    pub rebuild: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct InstallTargetArgs {
    #[arg(long, help = "Force Homebrew cask resolution")]
    pub cask: bool,
    #[arg(long, help = "Force Mac App Store resolution")]
    pub mas: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PackageSourceArgs {
    #[arg(long, help = "Prefer unstable or latest package variants")]
    pub bleeding_edge: bool,
    #[arg(long, help = "Include NUR in source selection")]
    pub nur: bool,
    #[arg(long, help = "Pin source backend (for example: nxs, nur, homebrew)")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct InstallAiArgs {
    #[arg(long, help = "Show routing rationale for AI-assisted decisions")]
    pub explain: bool,
    #[arg(
        long,
        help = "AI engine for routing/edit fallbacks (claude-code|codex|claude)"
    )]
    pub engine: Option<String>,
    #[arg(long, help = "Model identifier passed to the selected AI engine")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SearchOutputArgs {
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
    #[arg(long, short = 'v', help = "Show cache and query timing diagnostics")]
    pub verbose: bool,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = SEARCH_HELP)]
pub struct SearchArgs {
    #[arg(value_name = "PACKAGE", help = "Package name to search")]
    pub package: String,
    #[command(flatten)]
    pub source: PackageSourceArgs,
    #[command(flatten)]
    pub output: SearchOutputArgs,
}

impl SearchArgs {
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
    pub const fn json(&self) -> bool {
        self.output.json
    }

    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.output.verbose
    }
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = REMOVE_HELP)]
pub struct RemoveArgs {
    #[arg(
        value_name = "PACKAGES",
        required = true,
        num_args = 1..,
        help = "Installed package names/attributes to remove"
    )]
    pub packages: Vec<String>,
    #[arg(long, short = 'y', help = "Skip confirmation prompts")]
    pub yes: bool,
    #[arg(long, short = 'n', help = "Preview removals without writing files")]
    pub dry_run: bool,
    #[arg(long, help = "Model identifier for AI fallback removal path")]
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
#[command(after_long_help = WHERE_HELP)]
pub struct WhereArgs {
    #[arg(
        value_name = "PACKAGE",
        required = true,
        help = "Package name to locate in configuration"
    )]
    pub package: String,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = LIST_HELP)]
pub struct ListArgs {
    #[arg(
        value_name = "SOURCE",
        help = "Optional source filter (nix|homebrew|mas)"
    )]
    pub source: Option<String>,
    #[arg(long, help = "Show richer per-package details")]
    pub verbose: bool,
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
    #[arg(long, help = "Use plain output formatting for this command")]
    pub plain: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct InfoOutputArgs {
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
    #[arg(long, help = "Show additional source candidate details")]
    pub verbose: bool,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = INFO_HELP)]
pub struct InfoArgs {
    #[arg(
        value_name = "PACKAGE",
        required = true,
        help = "Package name to inspect"
    )]
    pub package: String,
    #[command(flatten)]
    pub source: PackageSourceArgs,
    #[command(flatten)]
    pub output: InfoOutputArgs,
}

impl InfoArgs {
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
    pub const fn json(&self) -> bool {
        self.output.json
    }

    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.output.verbose
    }
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = STATUS_HELP)]
pub struct StatusArgs {
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = INSTALLED_HELP)]
pub struct InstalledArgs {
    #[arg(
        value_name = "PACKAGES",
        required = true,
        num_args = 1..,
        help = "Package names to verify"
    )]
    pub packages: Vec<String>,
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
    #[arg(long, help = "Include file path and line location for matches")]
    pub show_location: bool,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = PROFILE_HELP)]
pub struct ProfileArgs {
    #[arg(
        long,
        default_value_t = 10,
        help = "Number of recent timing records to show"
    )]
    pub limit: usize,
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
}

#[derive(Debug, Clone, Parser, Default)]
#[command(after_long_help = LINT_HELP)]
pub struct LintArgs {
    #[arg(long, help = "Emit machine-readable JSON output")]
    pub json: bool,
}

#[derive(Debug, Clone, Parser, Default)]
#[command(after_long_help = UNDO_HELP)]
pub struct UndoArgs {
    #[arg(short, long, help = "Skip confirmation prompt")]
    pub yes: bool,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = UPDATE_HELP)]
pub struct UpdateArgs {
    #[arg(
        last = true,
        help = "Arguments passed through to the underlying nix flake update command"
    )]
    pub passthrough: Vec<String>,
}

#[derive(Debug, Clone, Parser, Default)]
#[command(after_long_help = REBUILD_HELP)]
pub struct RebuildArgs {
    #[arg(
        long,
        help = "Run lint, git, and flake preflight checks without rebuilding"
    )]
    pub preflight: bool,
    #[arg(long, help = "Print rebuild phase timings after recording them")]
    pub timing: bool,
    #[arg(
        last = true,
        help = "Arguments passed through to the underlying darwin-rebuild command"
    )]
    pub passthrough: Vec<String>,
}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = TEST_HELP)]
pub struct TestArgs {}

#[derive(Debug, Clone, Parser)]
#[command(after_long_help = UPGRADE_HELP)]
pub struct UpgradeArgs {
    #[command(flatten)]
    pub flow: UpgradeFlowArgs,
    #[command(flatten)]
    pub skip: UpgradeSkipArgs,
    #[arg(
        value_name = "INPUTS",
        help = "Optional flake input names to upgrade instead of updating the entire lockfile"
    )]
    pub targets: Vec<String>,
    #[arg(
        last = true,
        help = "Arguments passed through to the underlying nix flake update invocation"
    )]
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

    #[must_use]
    pub fn has_targets(&self) -> bool {
        !self.targets.is_empty()
    }

    #[must_use]
    pub fn should_run_brew_phase(&self) -> bool {
        !self.skip_brew() && !self.has_targets()
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct UpgradeFlowArgs {
    #[arg(
        long,
        short = 'n',
        help = "Preview upgrade actions without mutating files"
    )]
    pub dry_run: bool,
    #[arg(long, short = 'v', help = "Enable verbose upgrade output")]
    pub verbose: bool,
    #[arg(long, help = "Disable AI-assisted recovery prompts")]
    pub no_ai: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct UpgradeSkipArgs {
    #[arg(long, help = "Skip rebuild step")]
    pub skip_rebuild: bool,
    #[arg(long, help = "Skip git commit step")]
    pub skip_commit: bool,
    #[arg(long, help = "Skip brew update/upgrade step")]
    pub skip_brew: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct HelpArgs {
    #[arg(
        value_name = "TOPIC",
        num_args = 0..,
        allow_hyphen_values = true,
        help = "Help topic path (command path and/or flag query)"
    )]
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Parser, Default)]
#[command(after_long_help = INIT_HELP)]
pub struct InitArgs {
    #[arg(long, help = "Re-scan and merge with existing manifest")]
    pub refresh: bool,
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
    if first == "help" && out.len() >= 3 {
        let topic = out[2].to_string_lossy();
        if topic.starts_with('-') && topic != "--" && topic != "-h" && topic != "--help" {
            out.insert(2, OsString::from("--"));
        }
        return out;
    }

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
    use clap::error::ErrorKind;
    use std::collections::{BTreeMap, BTreeSet};

    const SPEC_DOC: &str = include_str!("../.agents/SPEC.md");

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
    fn preprocess_args_help_command_passes_through() {
        let result = preprocess_args(["nx", "help", "install"]);
        assert_eq!(result[1], OsString::from("help"));
        assert_eq!(result[2], OsString::from("install"));
    }

    #[test]
    fn preprocess_args_help_long_flag_query_inserts_double_dash() {
        let result = preprocess_args(["nx", "help", "--verbose"]);
        assert_eq!(result[1], OsString::from("help"));
        assert_eq!(result[2], OsString::from("--"));
        assert_eq!(result[3], OsString::from("--verbose"));
    }

    #[test]
    fn preprocess_args_help_short_flag_query_inserts_double_dash() {
        let result = preprocess_args(["nx", "help", "-v"]);
        assert_eq!(result[1], OsString::from("help"));
        assert_eq!(result[2], OsString::from("--"));
        assert_eq!(result[3], OsString::from("-v"));
    }

    #[test]
    fn preprocess_args_help_help_flag_does_not_insert_double_dash() {
        let result = preprocess_args(["nx", "help", "--help"]);
        assert_eq!(result[1], OsString::from("help"));
        assert_eq!(result[2], OsString::from("--help"));
        assert_eq!(result.len(), 3);
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

    fn markdown_code_spans(line: &str) -> Vec<&str> {
        let mut spans = Vec::new();
        let mut parts = line.split('`');
        let _ = parts.next();
        while let Some(span) = parts.next() {
            spans.push(span);
            let _ = parts.next();
        }
        spans
    }

    fn spec_section(start: &str, end: &str) -> &'static str {
        let (_, rest) = SPEC_DOC
            .split_once(start)
            .expect("spec section start should exist");
        let (section, _) = rest.split_once(end).expect("spec section end should exist");
        section
    }

    fn add_spec_flag_token(
        token: &str,
        long_flags: &mut BTreeSet<String>,
        short_flags: &mut BTreeSet<char>,
    ) {
        for part in token.split('/') {
            if let Some(long) = part.strip_prefix("--") {
                long_flags.insert(long.to_string());
            } else if let Some(short) = part.strip_prefix('-')
                && short.len() == 1
            {
                short_flags.insert(short.chars().next().expect("single short flag"));
            }
        }
    }

    fn spec_known_commands() -> BTreeSet<String> {
        spec_section("Known commands:\n", "## 2.2 Global Options")
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("- `"))
            .filter_map(|line| line.split('`').next())
            .map(str::to_owned)
            .collect()
    }

    fn spec_root_flags() -> (BTreeSet<String>, BTreeSet<char>) {
        let mut long_flags = BTreeSet::new();
        let mut short_flags = BTreeSet::new();

        for line in spec_section("## 2.2 Global Options", "## 2.3 Command Options").lines() {
            for token in markdown_code_spans(line) {
                add_spec_flag_token(token, &mut long_flags, &mut short_flags);
            }
        }

        (long_flags, short_flags)
    }

    fn spec_subcommand_flags() -> BTreeMap<String, (BTreeSet<String>, BTreeSet<char>)> {
        let mut by_command = BTreeMap::new();
        let mut current_command = None::<String>;

        for line in spec_section("## 2.3 Command Options", "## 2.4 Exit Code Contract").lines() {
            if let Some(command_line) = line.strip_prefix("- `") {
                let command = command_line
                    .split('`')
                    .next()
                    .expect("command bullet should include code span")
                    .to_string();
                by_command
                    .entry(command.clone())
                    .or_insert_with(|| (BTreeSet::new(), BTreeSet::new()));
                current_command = Some(command);
                continue;
            }

            if let Some(options_line) = line.trim_start().strip_prefix("- options: ") {
                let Some(command) = current_command.as_ref() else {
                    panic!("options line should follow a command bullet");
                };
                let (long_flags, short_flags) = by_command
                    .get_mut(command)
                    .expect("current command should be initialized");
                for token in markdown_code_spans(options_line) {
                    add_spec_flag_token(token, long_flags, short_flags);
                }
            }
        }

        by_command
    }

    fn clap_command_names_and_aliases() -> BTreeSet<String> {
        Cli::command()
            .get_subcommands()
            .flat_map(|subcommand| {
                std::iter::once(subcommand.get_name())
                    .chain(subcommand.get_all_aliases())
                    .map(str::to_owned)
            })
            .collect()
    }

    fn command_for_path<'a>(path: impl IntoIterator<Item = &'a str>) -> clap::Command {
        let mut command = Cli::command();
        for segment in path {
            command = command
                .find_subcommand_mut(segment)
                .unwrap_or_else(|| panic!("subcommand path segment `{segment}` should exist"))
                .clone();
        }
        command
    }

    fn local_long_flags_for_subcommand_path(path: &[&str]) -> BTreeSet<String> {
        let subcommand = command_for_path(path.iter().copied());
        let mut flags: BTreeSet<_> = subcommand
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(str::to_owned))
            .collect();

        for inherited in ["help", "plain", "unicode", "minimal"] {
            flags.remove(inherited);
        }

        flags
    }

    fn declared_long_flags_for_subcommand_path(path: &[&str]) -> BTreeSet<String> {
        let subcommand = command_for_path(path.iter().copied());
        let mut flags = BTreeSet::new();
        for arg in subcommand.get_arguments() {
            if let Some(longs) = arg.get_long_and_visible_aliases() {
                flags.extend(longs.into_iter().map(str::to_owned));
            }
        }
        flags.remove("help");
        flags.remove("version");
        flags
    }

    fn declared_short_flags_for_subcommand_path(path: &[&str]) -> BTreeSet<char> {
        let subcommand = command_for_path(path.iter().copied());
        let mut flags = BTreeSet::new();
        for arg in subcommand.get_arguments() {
            if let Some(shorts) = arg.get_short_and_visible_aliases() {
                flags.extend(shorts);
            }
        }
        flags.remove(&'h');
        flags.remove(&'V');
        flags
    }

    fn local_long_flags_for_subcommand(command: &str) -> BTreeSet<String> {
        local_long_flags_for_subcommand_path(&[command])
    }

    fn declared_long_flags_for_subcommand(command: &str) -> BTreeSet<String> {
        declared_long_flags_for_subcommand_path(&[command])
    }

    fn declared_short_flags_for_subcommand(command: &str) -> BTreeSet<char> {
        declared_short_flags_for_subcommand_path(&[command])
    }

    fn root_long_flags() -> BTreeSet<String> {
        let mut flags: BTreeSet<_> = Cli::command()
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(str::to_owned))
            .collect();
        flags.remove("help");
        flags.remove("version");
        flags
    }

    fn root_short_flags() -> BTreeSet<char> {
        let mut flags: BTreeSet<_> = Cli::command()
            .get_arguments()
            .filter_map(clap::Arg::get_short)
            .collect();
        flags.remove(&'h');
        flags.remove(&'V');
        flags
    }

    fn assert_subcommand_local_long_flags(command: &str, expected: &[&str]) {
        let expected: BTreeSet<_> = expected.iter().map(|flag| (*flag).to_owned()).collect();
        let actual = local_long_flags_for_subcommand(command);
        assert_eq!(
            actual, expected,
            "unexpected local long-flag set for `{command}`"
        );
    }

    #[test]
    fn known_commands_stay_in_sync_with_clap_subcommand_surface() {
        let known_commands: BTreeSet<_> = KNOWN_COMMANDS
            .iter()
            .map(|command| (*command).to_owned())
            .collect();
        let clap_commands = clap_command_names_and_aliases();
        assert_eq!(known_commands, clap_commands);
    }

    #[test]
    fn spec_known_commands_match_clap_subcommand_surface() {
        assert_eq!(spec_known_commands(), clap_command_names_and_aliases());
    }

    #[test]
    fn preprocess_args_flag_passes_through() {
        let result = preprocess_args(["nx", "--help"]);
        assert_eq!(result[1], OsString::from("--help"));
    }

    #[test]
    fn preprocess_args_no_subcommand_keeps_argv_shape() {
        let result = preprocess_args(["nx"]);
        assert_eq!(result, vec![OsString::from("nx")]);
    }

    fn render_root_help() -> String {
        let mut cmd = Cli::command();
        let mut help = Vec::<u8>::new();
        cmd.write_long_help(&mut help)
            .expect("root help should render");
        String::from_utf8(help).expect("help should be utf8")
    }

    fn render_invocation_help<const N: usize>(args: [&str; N]) -> String {
        let err = Cli::try_parse_from(args).expect_err("help invocation should not parse");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        err.to_string()
    }

    #[test]
    fn no_args_requests_help_instead_of_install_inference() {
        let err = Cli::try_parse_from(["nx"]).expect_err("no args should trigger help");
        assert_eq!(
            err.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn root_help_lists_spec_global_style_flags() {
        let help = render_invocation_help(["nx", "--help"]);
        assert!(help.contains("Run `nx help <topic>` for hierarchical help"));

        let expected_longs: BTreeSet<_> = ["plain", "unicode", "minimal"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(root_long_flags(), expected_longs);

        let expected_shorts: BTreeSet<char> = BTreeSet::new();
        assert_eq!(root_short_flags(), expected_shorts);
    }

    #[test]
    fn spec_global_flags_match_root_flag_metadata() {
        let (expected_longs, expected_shorts) = spec_root_flags();
        assert_eq!(root_long_flags(), expected_longs);
        assert_eq!(root_short_flags(), expected_shorts);
    }

    #[test]
    fn secret_add_parses_positional_key() {
        let cli =
            Cli::try_parse_from(["nx", "secret", "add", "example_secret_key", "--value", "v"])
                .expect("secret add should parse with positional key");
        let CommandKind::Secret(SecretArgs {
            command: SecretCommand::Add(add_args),
        }) = cli.command
        else {
            panic!("expected secret command");
        };
        assert_eq!(add_args.key_name(), "example_secret_key");
    }

    #[test]
    fn secret_add_parses_name_flag_key() {
        let cli = Cli::try_parse_from([
            "nx",
            "secret",
            "add",
            "--name",
            "example_secret_key",
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
        assert_eq!(add_args.key_name(), "example_secret_key");
    }

    #[test]
    fn secret_add_parses_key_alias_flag() {
        let cli = Cli::try_parse_from([
            "nx",
            "secret",
            "add",
            "--key",
            "example_secret_key",
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
        assert_eq!(add_args.key_name(), "example_secret_key");
    }

    #[test]
    fn secret_add_help_includes_examples_and_double_dash_note() {
        let help = render_root_help();
        assert!(help.contains("Run `nx help <topic>` for hierarchical help"));

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
        assert!(add_help.contains("nx secret add --name example_secret_key"));
        assert!(add_help.contains("`--` stops option parsing"));
    }

    #[test]
    fn global_style_flags_parse_at_root() {
        let cli = Cli::try_parse_from(["nx", "--unicode", "info", "ripgrep"]).expect("parse");
        assert!(cli.unicode());
    }

    #[test]
    fn version_flag_parses_at_root() {
        let err =
            Cli::try_parse_from(["nx", "--version"]).expect_err("version should short-circuit");
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn help_command_parses_topic_path() {
        let cli = Cli::try_parse_from(["nx", "help", "secret", "add"]).expect("parse help");
        let CommandKind::Help(args) = cli.command else {
            panic!("expected help command");
        };
        assert_eq!(args.topics, vec!["secret".to_string(), "add".to_string()]);
    }

    #[test]
    fn version_subcommand_parses_json_option() {
        let cli = Cli::try_parse_from(["nx", "version", "--json"]).expect("parse version");
        let CommandKind::Version(args) = cli.command else {
            panic!("expected version command");
        };
        assert!(args.json);
    }

    #[test]
    fn completion_subcommand_parses_shell() {
        let cli = Cli::try_parse_from(["nx", "completion", "zsh"]).expect("parse completion");
        let CommandKind::Completion(args) = cli.command else {
            panic!("expected completion command");
        };
        assert_eq!(args.shell, CompletionShellArg::Zsh);
    }

    #[test]
    fn generations_prune_parses_policy_and_dry_run_flags() {
        let cli = Cli::try_parse_from([
            "nx",
            "generations",
            "prune",
            "--keep",
            "25",
            "--kind",
            "home-manager",
            "--no-gc",
            "--dry-run",
        ])
        .expect("parse generations prune");

        let CommandKind::Generations(GenerationsArgs {
            command: GenerationsCommand::Prune(args),
            ..
        }) = cli.command
        else {
            panic!("expected generations prune command");
        };
        assert_eq!(args.policy.keep, 25);
        assert_eq!(args.policy.kind, GenerationKindArg::HomeManager);
        assert!(args.no_gc);
        assert!(args.dry_run);
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
        assert!(args.verbose());
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
    fn upgrade_parses_target_inputs_and_passthrough() {
        let cli = Cli::try_parse_from(["nx", "upgrade", "nx-rs", "anneal", "--", "--show-trace"])
            .expect("parse targeted upgrade");
        let CommandKind::Upgrade(args) = cli.command else {
            panic!("expected upgrade command");
        };
        assert_eq!(
            args.targets,
            vec!["nx-rs".to_string(), "anneal".to_string()]
        );
        assert_eq!(args.passthrough, vec!["--show-trace".to_string()]);
    }

    #[test]
    fn list_exposes_spec_options_and_globals_via_metadata() {
        let expected_longs: BTreeSet<_> = ["verbose", "json", "plain"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(declared_long_flags_for_subcommand("list"), expected_longs);

        let expected_shorts: BTreeSet<char> = BTreeSet::new();
        assert_eq!(declared_short_flags_for_subcommand("list"), expected_shorts);
    }

    #[test]
    fn info_exposes_spec_options_and_globals_via_metadata() {
        let expected_longs: BTreeSet<_> = ["verbose", "json", "bleeding-edge", "nur", "source"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(declared_long_flags_for_subcommand("info"), expected_longs);

        let expected_shorts: BTreeSet<char> = BTreeSet::new();
        assert_eq!(declared_short_flags_for_subcommand("info"), expected_shorts);
    }

    #[test]
    fn install_exposes_spec_options_via_metadata() {
        let expected_longs: BTreeSet<_> = [
            "yes",
            "dry-run",
            "cask",
            "mas",
            "service",
            "rebuild",
            "bleeding-edge",
            "nur",
            "source",
            "explain",
            "engine",
            "model",
            "verbose",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            declared_long_flags_for_subcommand("install"),
            expected_longs
        );

        let expected_shorts: BTreeSet<_> = ['y', 'n', 'v'].into_iter().collect();
        assert_eq!(
            declared_short_flags_for_subcommand("install"),
            expected_shorts
        );
    }

    #[test]
    fn upgrade_exposes_spec_options_via_metadata() {
        let expected_longs: BTreeSet<_> = [
            "dry-run",
            "verbose",
            "skip-rebuild",
            "skip-commit",
            "skip-brew",
            "no-ai",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            declared_long_flags_for_subcommand("upgrade"),
            expected_longs
        );

        let expected_shorts: BTreeSet<_> = ['n', 'v'].into_iter().collect();
        assert_eq!(
            declared_short_flags_for_subcommand("upgrade"),
            expected_shorts
        );
    }

    #[test]
    fn remaining_spec_subcommands_expose_expected_local_long_flags() {
        assert_subcommand_local_long_flags("remove", &["yes", "dry-run", "model"]);
        assert_subcommand_local_long_flags("where", &[]);
        assert_subcommand_local_long_flags("installed", &["json", "show-location"]);
        assert_subcommand_local_long_flags("lint", &["json"]);
        assert_subcommand_local_long_flags("status", &["json"]);
        assert_subcommand_local_long_flags("undo", &["yes"]);
        assert_subcommand_local_long_flags("update", &[]);
        assert_subcommand_local_long_flags("test", &[]);
        assert_subcommand_local_long_flags("rebuild", &["preflight", "timing"]);
        assert_subcommand_local_long_flags("profile", &["limit", "json"]);
        assert_subcommand_local_long_flags("version", &["json"]);
        assert_subcommand_local_long_flags("doctor", &["json", "verbose"]);
    }

    #[test]
    fn spec_command_option_blocks_match_clap_metadata() {
        for (command, (expected_longs, expected_shorts)) in spec_subcommand_flags() {
            let path: Vec<_> = command.split(' ').collect();
            assert_eq!(
                declared_long_flags_for_subcommand_path(&path),
                expected_longs,
                "unexpected long-flag set for spec block `{command}`"
            );
            assert_eq!(
                declared_short_flags_for_subcommand_path(&path),
                expected_shorts,
                "unexpected short-flag set for spec block `{command}`"
            );
        }
    }
}
