# nx-rs

Rust implementation of `nx` for nix-darwin package management.

## Status

- Migration and cutover are complete.
- `nx-rs` is the canonical implementation.
- Python `nx` remains only as a parity reference at `~/code/nx-python`.

## Quick Start

```bash
nx ripgrep
nx install --cask firefox
nx remove ripgrep
nx where ripgrep
nx list --plain
nx status
nx upgrade
```

Bare package names are interpreted as `install`:

```bash
nx ripgrep    # equivalent to: nx install ripgrep
```

## Install

Production (via flake) should be managed from `~/.nix-config`:

```nix
# flake.nix inputs
nx-rs = {
  url = "github:flowerornament/nx-rs";
  inputs.nixpkgs.follows = "nixpkgs";
};

# package list/module
inputs.nx-rs.packages.${pkgs.system}.default
```

Development/local install:

```bash
cargo install --path .
```

## Behavior Contract

- Contract source: `.agents/SPEC.md`
- Operational playbook: `.agents/CUTOVER_PLAYBOOK.md`
- Ongoing learnings: `.agents/LEARNINGS.md`

## Maintenance Gates

Run these checks on the documented cadence (or before release-sensitive changes):

```bash
just ci
just parity-check-rust
PY_NX="$HOME/code/nx-python/nx" just cutover-validate
```

Monthly cross-implementation parity:

```bash
just parity-check
```

## Development

```bash
just help
just doctor
just guard
just compile
just ci
```

## License

Private.
