# nx-rs

`nx-rs` provides `nx`, a CLI for managing packages and related Nix configuration repositories.

## About

`nx` is a workflow-oriented tool for maintaining a Nix config repo from the command line.
It helps you find package definitions, inspect what is installed, and make deterministic
manifest edits for package add/remove flows.

The tool is intended for repositories that manage system and home configuration in Nix,
and it uses `NX_REPO_ROOT` as an override when you want to target a repo other than the
current `flake.nix` tree.

## Usage

### Install

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
nix flake lock --update-input nx-rs
sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .
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
| `NX_RS_SOPS_BIN` | No | `sops` | Override the `sops` executable used by `nx secret add`. |
| `NX_RS_AUTO_REFRESH` | No | enabled | Controls auto-refresh of a local cargo-installed `nx` binary before `rebuild`/`upgrade`. Set to `0`, `false`, or `no` to disable. |
| `NO_COLOR` | No | unset | Disables colored output when set. |
| `TERM` | No | shell/default | If set to `dumb`, color output is disabled. |

### Examples

```bash
# Scan the current repo and write .nx/manifest.toml
nx init

# Bare package names are treated as "install"
nx ripgrep

nx install --cask firefox
nx search ripgrep
nx remove ripgrep
nx where ripgrep
nx list --plain
nx status
nx upgrade
```

### Commands

- `help` (hierarchical command/flag help topics)
- `init`
- `install`
- `remove` (aliases: `rm`, `uninstall`)
- `search`
- `where`
- `list`
- `info`
- `status`
- `installed`
- `secret add` (top-level command `secret`, alias `secrets`)
- `undo`
- `update`
- `test`
- `rebuild`
- `upgrade`

Use command help for full options:

```bash
nx --help
nx help <topic>
nx <command> --help
```

### Command Behavior

- `help`, `where`, `list`, `info`, `status`, `installed`, and `search` are read-only.
- `init` scans the repo and writes `.nx/manifest.toml` after confirmation.
- `install` and `remove` support `--dry-run` to preview changes.

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
