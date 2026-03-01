# nx-rs

`nx-rs` provides `nx`, a CLI for managing packages and related Nix configuration repositories.

## What `nx` Does

- Finds where packages are declared (`where`, `installed`, `status`, `list`)
- Searches candidate sources (`search`, `info`)
- Applies deterministic edits to Nix manifests (`install`, `remove`)
- Runs operational flows (`update`, `rebuild`, `upgrade`, `undo`)
- Manages encrypted secrets via `sops` (`secret add`)

## Quick Start

```bash
# Bare package names are treated as "install"
nx ripgrep

# Common workflows
nx install --cask firefox
nx remove ripgrep
nx where ripgrep
nx list --plain
nx status
nx upgrade
```

Output defaults to colored/interactive when supported. Use `--plain` (or `NO_COLOR=1`) for script-friendly output.

## Install (Production via Flake)

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

## Repository Root Configuration

`nx` requires the repository root via `NX_REPO_ROOT`.

Set it in your environment (shell config, direnv, or a `sops-nix` rendered env template):

```bash
export NX_REPO_ROOT=/path/to/your/config-repo
```

## Local Development

Toolchain is pinned in `rust-toolchain.toml` (Rust `1.92.0`).

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

## Command Surface

Primary commands:

- `install`, `remove` (`rm`, `uninstall`)
- `where`, `list`, `info`, `status`, `installed`, `search`
- `undo`, `update`, `test`, `rebuild`, `upgrade`
- `secret` (`secrets`) with `secret add`

Use command help for full options:

```bash
nx --help
nx <command> --help
```

## Safety Model

- Prefer `--dry-run` for mutating commands (`install`, `remove`).
- Commands like `where`, `list`, `info`, `status`, `installed`, and `search` are read-only.
- Repository root is configured via `NX_REPO_ROOT`.

## Behavior Contract

- `.agents/SPEC.md`

## Quality Gates

Standard gate:

```bash
just ci
```

Release-adjacent/operational validation:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic
just test-system
```

## Task Tracking

Work is tracked in `bd` (`./.beads`):

```bash
bd ready
bd create --title="<task>" --type=task --priority=2
bd close <id>
bd sync
```

## License

Private.
