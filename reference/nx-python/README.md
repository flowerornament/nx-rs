# nx

`nx` is a Python CLI for managing this nix-darwin + home-manager config. It
routes packages to the right `.nix` file, can query multiple sources, and
includes upgrade helpers.

## Usage

Run the reference copy from this repo root:

```bash
./reference/nx-python/nx <command> [args...]
```

Common commands:

```
nx <pkg>            # install package
nx rm <pkg>         # remove package
nx where <pkg>      # find where a package is configured
nx list             # list installed packages
nx info <pkg>       # detailed package info
nx status           # show package distribution
nx rebuild          # darwin-rebuild switch
nx upgrade          # full upgrade (update + changelog + rebuild)
nx undo             # undo last change
nx test             # ruff + mypy + unit tests
```

## Behavior

- **Routing**: each `.nix` file has a `# nx:` comment describing its purpose.
  `nx` reads discovered manifests at runtime (no fixed file-path map).
- **Install planning**: `search.py` builds a shared `InstallPlan` (token,
  target file, insertion mode, source flags) used by both Codex and Claude
  execution paths.
- **Routing safety**: fuzzy/LLM routing is constrained to discovered candidate
  manifests. Ambiguous or unrecognized model output emits a warning and falls
  back to a deterministic safe target.
- **Sources**: searches flake-input overlays, nixpkgs, NUR, and Homebrew.
  Default ranking is `flake-input` -> `nxs` -> `nur` -> `homebrew`/`cask`
  (with `--bleeding-edge`, `nur` is ranked above `nxs`).
- **Search cache**: multi-source search results are schema-versioned and query
  keys are normalized via alias mappings for stable cache hits.
- **Finder index**: package discovery uses an mtime/size-validated in-memory
  index so repeated `where/list/installed` operations avoid full rescans unless
  files changed.
- **Network behavior**: `nx info` only queries FlakeHub with
  `--bleeding-edge`.
- **AI engines** (optional): Codex is the default for edits; Claude can be
  used for more complex routing/edits.
- **Rebuild safety**: `nx rebuild` checks for untracked `.nix` files in
  `home/`, `packages/`, `system/`, and `hosts/` before flake evaluation.

## Current Install Routing Targets

- `packages/nix/cli.nix`: default fallback target for Nix CLI packages
- `packages/nix/languages.nix`: language runtimes/toolchains and `withPackages`
- `packages/homebrew/brews.nix`: Homebrew formulae
- `packages/homebrew/casks.nix`: Homebrew casks (GUI apps)
- `system/darwin.nix`: fallback for `masApps` / system-level entries

## Internals

- `config.py` scans `home/`, `system/`, `hosts/`, and `packages/` for `.nix`
  files and reads line-1 `# nx:` comments.
- `ConfigFiles` provides keyword-based accessors with stable fallbacks to the
  current layout (`packages/nix/*`, `packages/homebrew/*`, `system/darwin.nix`).
- `search.py` orchestrates source search + install, creates shared
  `InstallPlan` objects, and delegates only execution to
  `ai_helpers.py`/`claude_ops.py`.
- `ai_helpers.py` provides constrained routing decisions with deterministic
  fallback warnings when model output is ambiguous.
- `cache.py` stores search results with schema versioning and normalized query
  keys.
- `finder.py` parses package declarations across modules and dedicated
  manifests and maintains an mtime-aware in-memory index for repeated lookups.

## Code Layout

```
nx
├── cli.py            # Typer CLI wiring
├── commands.py       # command implementations
├── search.py         # search + install orchestration
├── sources.py        # source search + package info
├── finder.py         # find existing package declarations
├── cache.py          # schema-aware multi-source cache
├── config.py         # repo/config discovery
├── ai_helpers.py     # Codex/Claude helpers
├── claude_ops.py     # Claude streaming + edit helpers
├── shared.py         # shared utilities
└── tests/            # unit tests
```

## Testing

Run the full suite:

```bash
./reference/nx-python/nx test
```

This runs ruff, mypy, and unit tests.
