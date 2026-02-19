# nx Behavior Specification (Python Reference)

Status: Final v1.0
Date: 2026-02-16
Scope: Defines observable behavior of `~/code/nx-python` as migration target for Rust.
Reconciled against: 37 verified parity cases, Python source audit, and live cutover validation.

## 1. Source Of Truth

This spec is derived from:

- CLI wiring and command implementations in:
  - `~/code/nx-python/cli.py`
  - `~/code/nx-python/commands.py`
  - `~/code/nx-python/search.py`
  - `~/code/nx-python/sources.py`
  - `~/code/nx-python/config.py`
  - `~/code/nx-python/finder.py`
  - `~/code/nx-python/cache.py`
  - `~/code/nx-python/upgrade/*.py`
- Behavior asserted in `~/code/nx-python/tests/`.

When implementation and this document disagree, tests are considered normative.

## 2. CLI Surface Contract

## 2.1 Invocation

- Binary name: `nx`
- Framework behavior in Python: Typer app with `no_args_is_help=True`
- Command preprocessing:
  - If first CLI arg is not a known command and does not start with `-`, prepend `install`.
  - Example: `nx ripgrep` behaves as `nx install ripgrep`.

Known commands:

- `install`
- `remove`
- `rm` (alias of `remove`)
- `where`
- `list`
- `info`
- `status`
- `installed`
- `undo`
- `update`
- `test`
- `rebuild`
- `upgrade`

## 2.2 Global Options

Defined at root callback and persisted in app state:

- `--plain`
- `--unicode`
- `--minimal`
- `--verbose` / `-v`
- `--json`

## 2.3 Command Options

- `install`
  - args: `<packages...>`
  - options: `--yes/-y`, `--dry-run/-n`, `--cask`, `--mas`, `--service`, `--rebuild`, `--bleeding-edge`, `--nur`, `--source`, `--explain`, `--engine`, `--model`
- `remove` / `rm`
  - args: `<packages...>`
  - options: `--yes/-y`, `--dry-run/-n`, `--model`
- `where`
  - args: `<package>`
- `list`
  - args: `[source]`
  - options: `--verbose`, `--json`, `--plain`
- `info`
  - args: `<package>`
  - options: `--json`, `--bleeding-edge`, `--verbose`
- `status`
  - no args
- `installed`
  - args: `<packages...>`
  - options: `--json`, `--show-location`
- `undo`
  - no args
- `update`
  - passthrough args accepted
- `test`
  - no args
- `rebuild`
  - passthrough args accepted
- `upgrade`
  - options: `--dry-run/-n`, `--verbose/-v`, `--skip-rebuild`, `--skip-commit`, `--skip-brew`, `--no-ai`
  - passthrough args accepted

## 2.4 Exit Code Contract

- `install`: `2` when no package args; otherwise `0` if all requested install actions succeeded or nothing selected; `1` on partial failure.
- `remove`/`rm`: `2` when no package args; otherwise `0`.
- `where`: `2` when no package arg; otherwise `0` (including not-found).
- `list`: `1` for invalid source filter; otherwise `0`.
- `info`: `2` when no package arg; otherwise `0` (including not-found).
- `status`: `0`.
- `installed`: `2` when no package args; otherwise `0` only if all requested packages are installed.
- `undo`: `0`.
- `update`: `0` on flake update success, else `1`.
- `test`: `0` if all steps pass, else `1`.
- `rebuild`: `0` on successful rebuild, else `1`.
- `upgrade`: `0` on successful flow, else `1`.

## 3. Repository Discovery And Config Routing

## 3.1 Repo Root Resolution

Resolution order:

1. `B2NIX_REPO_ROOT` env var
2. `git rev-parse --show-toplevel` if contains `flake.nix`
3. `~/.nix-config`
4. error

## 3.2 Config File Discovery

- Scan `.nix` files under:
  - `home/`
  - `system/`
  - `hosts/`
  - `packages/`
- Skip `default.nix` and `common.nix` from the purpose-routed set and `all_files`.
- Read line 1 `# nx:` comment for routing purpose map.

Note: the finder (Section 4) independently collects all `.nix` files in the same directories via glob, including `default.nix`. This means `default.nix` files are excluded from purpose routing but included in package discovery.

`ConfigFiles` must provide purpose-based accessors with stable fallback paths for:

- packages (`packages/nix/cli.nix`)
- languages (`packages/nix/languages.nix`)
- services (`home/services.nix`)
- darwin (`system/darwin.nix`)
- homebrew manifests (`packages/homebrew/{brews,casks,taps}.nix`)

## 4. Finder Contract

## 4.1 `find_package(name)`

- Alias-aware lookup via `NAME_MAPPINGS` (case-insensitive).
- Searches parsed index hints first, then regex scan.
- Returns `file_path:line` or `None`.
- Must avoid false positives from alias assignments like `vim = "nvim";`.

## 4.2 `find_all_packages()`

Returns source buckets:

- `nxs`
- `brews`
- `casks`
- `mas`
- `services`

Parsing targets include:

- `home.packages`, `environment.systemPackages`
- `homebrew.brews`, dedicated `brews.nix`
- `homebrew.casks`, dedicated `casks.nix`
- `homebrew.masApps`
- `launchd.agents.*` and `launchd.user.agents.*`

## 4.3 Finder Index Performance Contract

- Index cached in-memory per `repo_root`.
- Cache key validity based on file signature `(mtime_ns, size)`.
- Rebuild only when signature set changes.
- Test-visible metric increments exactly on rebuild.

## 4.4 Fuzzy Lookup (`find_package_fuzzy`)

Resolution order:

1. exact
2. prefix match (`lua` -> `lua5_4`)
3. substring match (`rg` -> `ripgrep`)

Returns `(matched_name, location)` or `(None, None)`.

## 5. Cache Contract (`MultiSourceCache`)

- Path: `~/.cache/nx/packages_v4.json`
- Envelope:
  - `schema_version` (current: `1`)
  - `entries` map
- Key format: `<normalized_name>|<source>|<revision>`
- Name normalization uses alias map (case-insensitive).
- Revisions loaded from `flake.lock` (truncated to 12 chars).
- `get_all(name)` source order: `nxs`, `nur`, `homebrew`, `cask`
- Guardrail: if cached results are homebrew-only (no `nxs`/`nur`), return empty to force fresh search.
- Schema mismatch invalidates cache.

## 6. Source Search Contract

## 6.1 Search Inputs

`SourcePreferences` fields:

- `bleeding_edge`
- `nur`
- `force_source`
- `is_cask`
- `is_mas`

## 6.2 Search Shortcuts

`search_all_sources(name, prefs, flake_lock_path)` order:

1. forced source (`--source`: `nxs|unstable|nur|homebrew`)
2. explicit source shortcut:
  - `--cask` returns synthetic cask result
  - `--mas` returns synthetic mas result
3. language override for `python3Packages.*`, etc. (must validate attr/platform)
4. parallel primary search (`nxs`, optional `flake-input`, optional `nur`)
5. always append homebrew formula + cask alternatives
6. sort by source priority and confidence
7. deduplicate by `(source, attr)`

## 6.3 Parallel Search Failure/Timeout

- Uses `as_completed(..., timeout=45)`.
- On timeout: warn, keep partial completed results, cancel pending, do not block.
- Individual source failure logs warning but does not fail whole search.

## 6.4 Platform Availability Check

`check_nix_available(attr)`:

- If `nix` missing: permissive `(True, None)`.
- Evaluates `meta.platforms`.
- Reject only when explicit string list excludes current platform.
- Structured/non-string platform specs are treated permissively.

## 7. Install Flow Contract

## 7.1 Search Resolution

Per package:

1. If already installed via finder: return synthetic `source="installed"` result.
2. Else check cache.
3. Else query sources and cache best-per-source.
4. If any candidate (including alternates) is already installed, mark as installed and skip installation.

## 7.2 User Confirmation

- Show results grouped as:
  - installable
  - already installed
  - unknown/not found
- When a single package has multiple source alternatives:
  - interactive numbered prompt `Install? [1/2/.../n]:`
  - `2` selects alternative source
  - empty input defaults to option 1
- `--yes` or `--dry-run` bypass confirmation.

## 7.3 InstallPlan Contract (Shared Across Engines)

`InstallPlan` fields:

- `source_result`
- `package_token`
- `target_file`
- `insertion_mode`:
  - `nix_manifest`
  - `language_with_packages`
  - `homebrew_manifest`
  - `mas_apps`
- `is_brew`, `is_cask`, `is_mas`
- `language_info`
- `routing_warning`

Required safety:

- For `nxs|nur|flake-input`, missing `attr` is hard error: refuse install plan.

Routing behavior:

- cask -> `homebrew/casks.nix`
- homebrew formula -> `homebrew/brews.nix`
- mas -> `system/darwin.nix`
- language package -> `packages/nix/languages.nix` with `withPackages` insertion mode
- general nix package -> `route_package_codex_decision(...)` over constrained candidate manifest files

## 7.4 Routing Safety Invariants

- Candidate list constrained to discovered `.nix` manifests in same parent as default target.
- Language manifest excluded from general-nix candidate set.
- Ambiguous/unrecognized routing output must:
  - fallback to deterministic target
  - emit warning surfaced to user.
- MCP tools (`*-mcp`, `mcp-*`) force fallback target (`packages/nix/cli.nix`).

## 7.5 Flake Input Modification Gate

If source requires flake mod:

- Turbo/Codex mode (no prompt) refuses and warns: use Claude engine.
- Claude path may prompt to add flake input unless `--yes`.
- `--dry-run` reports intended flake input action without mutation.

## 7.6 Platform-Incompatible Primary Result Fallback

For nix-based sources:

- Check availability on current platform.
- If unavailable, try next candidate from same source with available attr.
- If no same-source fallback, skip with error.

## 7.7 Engine Execution Semantics

- `--engine=codex` uses turbo path.
- `--engine=claude` uses Claude edit path.
- Both engines must consume same `InstallPlan` contract (`package_token`, `target_file`, `insertion_mode`).

## 7.8 Post-Install

- On successful (non-dry-run) installs, print `Run: nx rebuild`.
- If `--rebuild`, run darwin-rebuild switch directly.

## 8. Remove Flow Contract

- `rm` is exact alias of `remove`.
- Per package:
  - locate with finder
  - dry-run prints preview and `Would remove ...`
  - non-dry-run confirms unless `--yes`
- Removal strategy:
  - if concrete line known: direct file edit removal
  - else fallback to Claude-based edit
- Command returns `0` even when individual packages are not found.

## 9. Query Commands Contract

## 9.1 `where`

- Prints success + snippet when found.
- Not found prints suggestion (`Try: nx info <name>`).
- Exit code remains `0` for not-found.

## 9.2 `list`

- `--plain`: one package per line with two-space indent.
- `--json`: raw source->package-list JSON.
- optional source filter via alias normalization:
  - valid aliases include `nix`, `nxs`, `brew`, `brews`, `homebrew`, `cask`, `casks`, `mas`, `service`, `services`.
- invalid filter -> error + valid source list + exit `1`.

## 9.3 `info`

JSON mode returns:

- `name`
- `installed` boolean
- `location`
- `sources[]` with metadata fields
- `hm_module` optional
- `darwin_service` optional
- `flakehub[]` optional

Network behavior:

- FlakeHub lookup in `info` is only performed when `--bleeding-edge` is set.

## 9.4 `status`

- Produces total count + per-source distribution table.

## 9.5 `installed`

- Supports fuzzy package match.
- JSON output format (query strings as top-level keys):
  - `{ "<query>": { "match": <name-or-null>, "location": <loc-or-null> } }`
- Exit `0` only if all requested packages resolved to installed locations.
- Single package non-json mode with `--show-location` includes normalized location.

## 10. System Command Contracts

## 10.1 `undo`

- Lists modified files from `git status --porcelain`.
- If none: prints `Nothing to undo.` and exits `0`.
- Prompts `Revert all changes?` (default no), then reverts each modified file using `git checkout -- <file>`.

## 10.2 `update`

- Runs `nix flake update` via shared streaming function.
- Accepts passthrough args.
- Success message instructs `nx rebuild` or `nx upgrade`.

## 10.3 `test`

Runs in order:

1. `ruff check .` (cwd: `scripts/nx`)
2. `mypy .` (cwd: `scripts/nx`)
3. `python3 -m unittest discover -s scripts/nx/tests` (cwd: repo root)

Any failure stops pipeline and returns `1`.

## 10.4 `rebuild`

Preflight requirements:

1. Git preflight must succeed.
2. No untracked `.nix` files under `home/`, `packages/`, `system/`, `hosts/`.
3. `nix flake check <repo_root>` must pass.

Then run:

- `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake <repo_root> [passthrough...]`

## 10.5 `upgrade`

High-level phases:

1. Flake phase:
  - load old lock
  - dry-run: skip update
  - non-dry-run: stream `nix flake update`
  - load new lock and diff
  - fetch change info and summaries
2. Brew phase (unless `--skip-brew`):
  - `brew outdated --json`
  - enrich and changelog fetch
  - non-dry-run `brew upgrade <pkgs...>`
3. Rebuild unless `--skip-rebuild`
4. Commit `flake.lock` unless `--skip-commit` (and if flake changes exist)

Dry-run behavior:

- Prints dry-run banner.
- No file/system mutation.

## 11. Upgrade/Changelog Contracts

- `stream_nix_update`:
  - fetches `gh auth token` and passes as `--option access-tokens github.com=...` when available.
  - retries once if output indicates known fetcher-cache corruption.
  - corruption retry clears `~/.cache/nix/fetcher-cache-v4.sqlite`.
- `parse_flake_lock`:
  - supports `github` and `tarball` inputs.
  - skips `file` type.
  - extracts owner/repo from FlakeHub tarball URLs.
- `diff_locks`:
  - returns `(changed, added, removed)` at input level.

## 12. Utility Contracts

- `split_location("a:12:34")` -> path `a:12`, line `34`.
- `relative_path` strips repo root prefix and keeps `:line` suffix.
- `detect_language_package` recognizes versioned Python package sets (e.g. `python313Packages.*` -> `python3.withPackages` handling).
- `add_flake_input` is idempotent for existing input.
- `run_streaming_command`:
  - returns `(returncode, joined_output)`.
  - supports printer stream callback.
  - plain output must preserve indent on wrapped lines.

## 13. Output/UX Contracts Backed By Tests

- Dry-run install output includes `Dry Run`.
- Dry-run remove output includes `Would remove`.
- Rebuild flow invokes streaming command path when preflight and flake-check pass.
- `list --plain` includes discovered package names.
- `info --json` includes package name and source metadata.
- `installed --json` includes queried package key.

## 14. Known Compatibility Notes For Rust Port

- Preserve permissive behavior where current CLI does not fail hard:
  - `where` not-found returns `0`
  - `remove` per-item failures do not change command exit
  - `info` not-found returns `0`
- Preserve safety behavior where current CLI fails hard:
  - rebuild preflight failures
  - missing install attr for nix-based sources
  - invalid `list` source filter

