# nx

Multi-source package installer for nix-darwin.

`nx` manages packages across nixpkgs, Homebrew, casks, NUR, Mac App Store, and flake inputs from a single CLI. It edits your nix-darwin config files directly, so `darwin-rebuild switch` picks up changes.

## Quick Start

```bash
nx ripgrep          # search all sources, install best match
nx install --cask firefox   # explicit cask install (skips search)
nx remove ripgrep   # remove from config
nx upgrade          # update flake inputs + brew + rebuild
nx rebuild          # darwin-rebuild switch
nx where ripgrep    # show which config file contains it
nx list             # list all installed packages by source
```

Bare package names are treated as `install`:

```bash
nx ripgrep          # equivalent to: nx install ripgrep
```

Typos are caught before they become broken installs:

```bash
nx rebuiild         # error: unknown command 'rebuiild'. Did you mean 'rebuild'?
```

## Install

Requires Rust 1.92+ (pinned in `rust-toolchain.toml`).

```bash
cargo install --path .
```

Or use `just` for development:

```bash
just doctor         # verify toolchain
just ci             # fmt + clippy + test + check
```

## How It Works

1. **Search** - queries nixpkgs, Homebrew, NUR, flake inputs in parallel
2. **Route** - picks the best source by confidence score and preference
3. **Edit** - inserts the package into the correct `.nix` config file
4. **Rebuild** - `nx rebuild` applies changes via `darwin-rebuild switch`

Config repo location: `~/.nix-config`

## Commands

| Command | Description |
|---------|-------------|
| `install <pkg>` | Search and install a package |
| `remove <pkg>` | Remove a package from config |
| `where <pkg>` | Show config file location |
| `list [source]` | List installed packages |
| `info <pkg>` | Show package details |
| `installed <pkg>` | Check if installed |
| `status` | Show system status |
| `update` | Update flake inputs only |
| `upgrade` | Full upgrade (flake + brew + rebuild) |
| `rebuild` | darwin-rebuild switch |
| `test` | Run config tests |
| `undo` | Revert uncommitted config changes |

## Development

```bash
just help           # show all workflows
just guard          # strict pre-compile checks
just compile        # guard + cargo check
just ci             # fmt + clippy + test + check
```

## License

Private.
