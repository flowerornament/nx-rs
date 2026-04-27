# nx-rs

`nx-rs` provides `nx`, a CLI for managing Nix configuration repositories and host maintenance tasks.

## About

`nx` is a workflow-oriented tool for maintaining a Nix config repo from the command line.
It helps you find package definitions, inspect what is installed, make deterministic
manifest edits for package add/remove flows, and manage host-level Nix generations.

The tool is intended for repositories that manage system and home configuration in Nix,
and it uses `NX_REPO_ROOT` as an override when you want to target a repo other than the
current `flake.nix` tree.

Host maintenance commands under `nx generations` are intentionally host-scoped and do not
require a repository root.

## Usage

### Install

#### Nix

For a one-off install, use `nix run` or `nix profile install`.
If you want persistent session defaults like `NX_REPO_ROOT`, prefer the Home Manager path below.

Run without installing:

```bash
nix run github:flowerornament/nx-rs -- --help
```

Install into your profile:

```bash
nix profile install github:flowerornament/nx-rs
```

Add `nx-rs` to your Nix configuration repository:

```nix
# flake.nix inputs
nx-rs = {
  url = "github:flowerornament/nx-rs";
  inputs.nixpkgs.follows = "nixpkgs";
};

# package list/module
inputs.nx-rs.packages.${pkgs.system}.default
```

Then rebuild:

```bash
nix flake update nx-rs
sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .
```

#### Nix + Home Manager

This is the recommended install path for declarative Nix users.

For a declarative user-level install, `nx-rs` now exports a Home Manager
module. It installs `nx` into `home.packages` and can optionally export the
same environment variables that drive `nx` at runtime.

Add the flake input:

```nix
nx-rs = {
  url = "github:flowerornament/nx-rs";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Then include the module in your Home Manager configuration:

```nix
{
  imports = [
    inputs.nx-rs.homeManagerModules.default
  ];

  programs.nx = {
    enable = true;
    repoRoot = "/Users/alice/code/nix-config";
    autoRefresh = false;
    cleanCaches.skip = [ "huggingface" ];
  };
}
```

Available module options:

- `programs.nx.enable`
- `programs.nx.package`
- `programs.nx.repoRoot`
- `programs.nx.autoRefresh`
- `programs.nx.sops.package`
- `programs.nx.sops.bin`
- `programs.nx.cleanCaches.codeRoots`
- `programs.nx.cleanCaches.scanDepth`
- `programs.nx.cleanCaches.skip`

The module only manages binary installation and environment variables. Repo
structure, manifests, and command behavior still live in the target Nix config
repository itself.

That boundary is intentional:

- the managed repo owns `nx` behavior over your Nix configuration
- `programs.nx` owns machine/session defaults only
- CLI flags still own one-off overrides

If you use `nx secret add`, you can wire `sops` declaratively too:

```nix
{
  imports = [
    inputs.nx-rs.homeManagerModules.default
  ];

  programs.nx = {
    enable = true;
    sops.package = pkgs.sops;
    sops.bin = "${pkgs.sops}/bin/sops";
  };
}
```

### Configure Repository Root

`nx` auto-detects the managed repo by walking up from the current working directory until
it finds `flake.nix`. Set `NX_REPO_ROOT` only when you want to override that detection:

```bash
export NX_REPO_ROOT=/path/to/your/config-repo
```

### LLM-Assisted Functions

Some flows can call local AI CLIs when helpful:

- `install`:
  - `--engine codex|claude` selects the engine (`codex` default).
  - `--model <name>` selects the model for the chosen engine.
  - `--explain` prints extra decision details during install resolution.
- `remove`:
  - `--model <name>` sets the Claude model used for AI fallback edits.
- `upgrade`:
  - Prints optional AI-generated change summaries for flake/homebrew updates.
  - `--no-ai` disables AI summaries entirely.

Requirements:

- `codex` and/or `claude` must be installed and available on `PATH` for the corresponding features.
- Authentication and provider-specific environment variables are managed by those CLIs.

### Environment Variables

`nx` reads the following environment variables:

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `NX_REPO_ROOT` | No | auto-detect from `cwd` / `flake.nix` | Absolute/relative path override for the target Nix configuration repository. |
| `NX_PROFILE_PATH` | No | `~/.local/state/nx/timings.jsonl` | Override where rebuild/upgrade timing records are written and read. |
| `NX_RS_SOPS_BIN` | No | `sops` | Override the `sops` executable used by `nx secret add`. |
| `NX_RS_AUTO_REFRESH` | No | enabled | Controls auto-refresh of a local cargo-installed `nx` binary before `rebuild`/`upgrade`. Set to `0`, `false`, or `no` to disable. |
| `NX_NO_AUTO_HASH_FIX` | No | unset | Disable automatic fixed-output hash repair during `nx upgrade` when set to `1`, `true`, `yes`, or `on`. |
| `NX_CODE_ROOTS` | No | `~/code` | Colon-separated roots scanned by `nx clean-caches`; set to an empty string to disable code-root scans. |
| `NX_CLEAN_SCAN_DEPTH` | No | `3` | Maximum directory depth for `nx clean-caches` code-root scans; values above `8` are clamped. |
| `NX_CLEAN_SKIP` | No | unset | Comma-separated cache names for `nx clean-caches` to skip. |
| `NO_COLOR` | No | unset | Disables colored output when set. |
| `TERM` | No | shell/default | If set to `dumb`, color output is disabled. |

### Examples

Repository setup and discovery:

```bash
# Scan the current repo and write .nx/manifest.toml
nx init

# Show the config location for a package
nx where ripgrep

# Show package distribution across nix/homebrew/mas sources
nx status
```

Package changes:

```bash
# Bare package names are treated as "install"
nx ripgrep
# Likely command typos are left as command errors with suggestions
nx rebulid

# Explicit install flows
nx install ripgrep fd
nx install --cask firefox
nx install --mas Tailscale
nx install --dry-run --bleeding-edge neovim

# Remove installed packages
nx remove ripgrep
nx remove --dry-run firefox
```

Read-only inspection:

```bash
nx search ripgrep
nx info ripgrep
nx list --plain
nx installed ripgrep fd
```

System and host maintenance:

```bash
nx lint
nx rebuild --preflight
nx rebuild --timing
nx rebuild
nx profile --limit 20
nx upgrade
nx upgrade nx-rs
nx generations status
nx generations plan
nx generations prune --dry-run
nx clean-caches --dry-run
nx clean-caches rust-targets node-modules
nx clean-caches --only nix-gc,codex-sessions
```

Secrets:

```bash
nx secret add github_token --value-stdin
printf '%s' "$TOKEN" | nx secret add github_token --value-stdin
```

### Command Guide

Use command help for the full flag surface:

```bash
nx --version
nx --help
nx help <topic>
nx <command> --help
```

---

#### `version`

Shows the current `nx` version.

- Supports `nx --version`, `nx -V`, and `nx version`.
- Supports `nx version --json`.

---

#### `help`

Shows hierarchical help for commands and flags. Use `nx help install`, `nx help secret add`, or `nx help -- --dry-run` when you want the command tree rather than raw clap help.

---

#### `completion`

Generates shell completion scripts to stdout.

- Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.
- Example: `nx completion zsh > ~/.zsh/completions/_nx`.

---

#### `doctor`

Diagnoses repo and host prerequisites that affect normal `nx` usage.

- Reports repo root resolution, manifest health, routing status, flake lock state, cache availability, and tool presence.
- Supports `--verbose` for extra detail.
- Supports `--json` for automation.

---

#### `init`

Scans the current repo and writes `.nx/manifest.toml`. Use `--refresh` to rescan and merge with an existing manifest.

---

#### `install`

Installs one or more packages into the managed repo. If you run `nx ripgrep`, `nx` treats the bare token as `nx install ripgrep`. Likely command typos such as `nx rebulid` are left as command errors so `nx` can suggest the intended command.

- Use `--dry-run` to preview edits without writing files.
- Use `--yes` to skip interactive prompts and accept defaults.
- Use `--cask` or `--mas` to force macOS package targets.
- Use `--bleeding-edge`, `--nur`, or `--source` to bias source selection.
- Use `--service` to offer service scaffolding after install.
- Use `--rebuild` to rebuild after successful changes.
- Use `--engine`, `--model`, and `--explain` for AI-assisted routing/edit flows.
- Use `--verbose` to surface cache and query timing diagnostics.

---

#### `remove` / `rm` / `uninstall`

Removes installed packages from the managed repo.

- Supports `--dry-run` and `--yes`.
- Supports `--model` for the AI fallback removal path.
- `rm` and `uninstall` are aliases of `remove`.

---

#### `secret add`

Adds or updates an encrypted secret via `sops`.

- Accepts a positional key or `--name`.
- Accepts `--value` or `--value-stdin`.
- Prefer `--value-stdin` to avoid leaving secrets in shell history.
- Top-level command aliases: `secret`, `secrets`.

---

#### `search`

Searches package sources without editing the repo.

- Good first step when you are not sure which backend will provide a package.
- Supports `--bleeding-edge`, `--nur`, `--source`, `--json`, and `--verbose`.

---

#### `where`

Shows where a package is declared in the managed repo. This is the fastest way to jump to the owning file before a manual edit or review.

---

#### `list`

Lists installed packages by source bucket.

- Optional source filter: `nix`, `homebrew`, or `mas`.
- Supports `--verbose`, `--json`, and `--plain`.

---

#### `info`

Shows package metadata plus candidate source information.

- Useful when deciding between nixpkgs, NUR, flake input, Homebrew, or MAS routes.
- Supports `--json`, `--bleeding-edge`, `--nur`, `--source`, and `--verbose`.

---

#### `status`

Shows a package distribution summary for the managed repo. This is a read-only overview of how packages are split across supported source families.

- Supports `--json`.

---

#### `installed`

Checks whether one or more packages are currently installed.

- Supports `--json`.
- Supports `--show-location` to include file and line matches.
- Returns success only when all requested packages are installed.

---

#### `lint`

Checks `# nx:` routing annotations and keyword overlap across routable `.nix` files. Use this before reorganizing manifests or adding new routable files.

- Supports `--json`.

---

#### `undo`

Reverts modified tracked files via git checkout. Use `--yes` to skip the confirmation prompt.

---

#### `update`

Runs `nix flake update`. Additional args after `--` are passed through to the underlying flake update invocation.

---

#### `test`

Runs repo quality checks. This is the CLI entry point for validating the managed Nix config repository itself.

---

#### `rebuild`

Runs `darwin-rebuild switch` for the managed repo.

- Use `--preflight` to stop after lint, git, and flake checks without switching.
- Use `--timing` to print phase timings after recording them locally.
- Additional args after `--` pass through to the underlying `darwin-rebuild` command.

---

#### `profile`

Shows recent local rebuild and upgrade timing records.

- Defaults to `~/.local/state/nx/timings.jsonl`.
- Set `NX_PROFILE_PATH` to read/write a different timing file.
- Use `--json` for machine-readable records.

---

#### `upgrade`

Runs the upgrade flow for either the whole repo or named flake inputs.

- Use `--dry-run` to preview without mutating files.
- `nx upgrade` with no positional inputs runs the full repo-wide flow: flake update, Homebrew update/upgrade, rebuild, and git commit.
- `nx upgrade <input...>` updates only the named flake inputs via `nix flake update <input...>`.
- After changed GitHub-backed inputs are written to `flake.lock`, `nx upgrade` prefetches those exact revisions so Nix lazy source caches are warm before flake check and rebuild.
- If rebuild reports a Nix fixed-output hash mismatch, `nx upgrade` updates the unique clean matching hash in a tracked `.nix` file, retries, and includes that file in the upgrade commit. It prints the file, line, old hash, and new hash when it does this. If it cannot do that safely, it prints the exact next action.
- Targeted input upgrades skip the Homebrew phase by default.
- Use `--skip-brew`, `--skip-rebuild`, or `--skip-commit` to trim the flow further.
- Use `--no-ai` to disable AI-generated summaries and recovery prompts.
- Additional args after `--` pass through to the underlying `nix flake update` command.

Examples:

```bash
nx upgrade
nx upgrade --dry-run
nx upgrade nx-rs
nx upgrade nx-rs anneal -- --show-trace
```

---

#### `generations`

Host-scoped retention and garbage-collection commands. These work from any directory and do not require a managed repo.

- `nx generations status` shows discovered nix-darwin and Home Manager generations plus the active retention policy.
- `nx generations plan` renders the exact prune and garbage-collection plan without mutating the host.
- `nx generations prune` executes that plan, prompting by default.
- Important flags: `--keep`, `--kind`, `--no-gc`, `--yes`, `--dry-run`.
- `nx generations prune --dry-run` renders the same plan as `nx generations plan`.

---

#### `clean-caches`

Scans host cache directories and common build artifacts, reports sizes, and cleans them after confirmation.

- Supports `--dry-run` and `--yes`.
- Code-root scans default to `~/code` and depth `3`; scan depth is capped at `8`.
- Large Nix GC and code-root scans show live per-bucket loading feedback while sizes are computed.
- Positional cache names or `--only` limit the scan and clean plan to selected caches.
- Code-root build artifacts report how many directories were discovered.
- Nix GC sizing runs last because dead-store estimation can be slower than normal cache directory sizing.
- Set `NX_CODE_ROOTS`, `NX_CLEAN_SCAN_DEPTH`, and `NX_CLEAN_SKIP` directly, or configure them declaratively with `programs.nx.cleanCaches`.
- Valid skip names: `cargo-registry`, `uv`, `npm`, `homebrew`, `huggingface`, `puppeteer`, `playwright`, `xcode-derived`, `core-simulator`, `codex-sessions`, `codex-logs`, `claude-telemetry`, `claude-file-history`, `nix-gc`, `rust-targets`, `elixir-builds`, `node-modules`.

---

### Behavior Notes

- `help`, `where`, `list`, `info`, `status`, `installed`, `search`, `generations status`, and `generations plan` are read-only.
- `version`, `completion`, and `doctor` are also read-only.
- `install` and `remove` support `--dry-run` to preview changes before writing files.
- `generations` commands are host-scoped and work from any directory.
- Most commands operate on the current repo root; set `NX_REPO_ROOT` only when you want to override auto-discovery.

## Development

### Toolchain

Rust is pinned in `rust-toolchain.toml` (`1.93.1`).

### Workflow

```bash
just help
just doctor
just hooks-install
just compile
just ci
```

Local install for ad hoc testing:

```bash
cargo install --path .
```

### Quality Gates

Standard gate:

```bash
just ci
```

Additional strict checks:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic
just test-system
```

### Task Tracking

Work is tracked in `bd` (`./.beads`):

```bash
bd ready
bd create --title="<task>" --type=task --priority=2
bd close <id>
bd dolt push
```

## License

MIT.
