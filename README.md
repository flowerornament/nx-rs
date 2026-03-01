# nx-rs

`nx-rs` provides `nx`, a CLI for managing packages and related Nix configuration repositories.

## About

`nx` is a workflow-oriented tool for maintaining a Nix config repo from the command line.
It helps you find package definitions, inspect what is installed, and make deterministic
manifest edits for package add/remove flows.

The tool is intended for repositories that manage system and home configuration in Nix,
and it uses `NX_REPO_ROOT` to target the repository it should operate on.

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

`nx` requires `NX_REPO_ROOT`:

```bash
export NX_REPO_ROOT=/path/to/your/config-repo
```

### Examples

```bash
# Bare package names are treated as "install"
nx ripgrep

nx install --cask firefox
nx remove ripgrep
nx where ripgrep
nx list --plain
nx status
nx upgrade
```

### Commands

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
nx <command> --help
```

### Command Behavior

- `where`, `list`, `info`, `status`, `installed`, and `search` are read-only.
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
bd sync
```

## License

MIT.
