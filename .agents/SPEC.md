# nx Behavior Specification

Status: Final v1.2
Date: 2026-03-16
Scope: Defines the observable behavior contract for `nx-rs`.
Historical parity notes from earlier `nx` implementations are informative only when this
document explicitly preserves them; deprecated implementations are not a maintenance dependency.
Reconciled against: repository test coverage, source audit, and production validation.

## 1. Source Of Truth

This spec is maintained for `nx-rs`.

Source inputs used to shape it include:

- behavior asserted in this repository's tests
- current `nx-rs` source behavior where it is intentionally documented here
- audited historical `nx` behavior only where parity is still an explicit goal

When implementation and this document disagree, this repository's tests are considered normative.

## 2. CLI Surface Contract

## 2.1 Invocation

- Binary name: `nx`
- Framework behavior: clap-based Rust CLI with root help when no command is provided and a
  hierarchical `help` subcommand for topic lookup
- Command preprocessing:
  - If first CLI arg is not a known command and does not start with `-`, prepend `install`.
  - Example: `nx ripgrep` behaves as `nx install ripgrep`.

Known commands:

- `version`
- `help`
- `completion`
- `doctor`
- `init`
- `install`
- `remove`
- `rm` (alias of `remove`)
- `uninstall` (alias of `remove`)
- `secret`
- `secrets` (alias of `secret`)
- `search`
- `where`
- `list`
- `info`
- `status`
- `installed`
- `profile`
- `lint`
- `undo`
- `update`
- `test`
- `rebuild`
- `upgrade`
- `generations`
- `clean-caches`

## 2.2 Global Options

Defined at root callback:

- `--plain`
- `--unicode`
- `--minimal`

## 2.3 Command Options

- `version`
  - options: `--json`
- `help`
  - args: `[topics...]`
- `completion`
  - args: `<shell>`
- `doctor`
  - options: `--json`, `--verbose/-v`
- `init`
  - options: `--refresh`
- `install`
  - args: `<packages...>`
  - options: `--yes/-y`, `--dry-run/-n`, `--verbose/-v`, `--cask`, `--mas`, `--service`, `--rebuild`, `--bleeding-edge`, `--nur`, `--source`, `--explain`, `--engine`, `--model`
- `remove` / `rm` / `uninstall`
  - args: `<packages...>`
  - options: `--yes/-y`, `--dry-run/-n`, `--model`
- `secret` / `secrets`
  - subcommands: `add`
- `secret add`
  - args: `[key]`
  - options: `--name/--key`, `--value`, `--value-stdin`
- `search`
  - args: `<package>`
  - options: `--bleeding-edge`, `--nur`, `--source`, `--json`, `--verbose/-v`
- `where`
  - args: `<package>`
- `list`
  - args: `[source]`
  - options: `--verbose`, `--json`, `--plain`
- `info`
  - args: `<package>`
  - options: `--json`, `--bleeding-edge`, `--nur`, `--source`, `--verbose`
- `status`
  - options: `--json`
- `installed`
  - args: `<packages...>`
  - options: `--json`, `--show-location`
- `profile`
  - options: `--limit`, `--json`
- `lint`
  - options: `--json`
- `undo`
  - options: `--yes/-y`
- `update`
  - passthrough args accepted
- `test`
  - no args
- `rebuild`
  - options: `--preflight`, `--timing`
  - passthrough args accepted
- `upgrade`
  - args: `[inputs...]`
  - options: `--dry-run/-n`, `--verbose/-v`, `--skip-rebuild`, `--skip-commit`, `--skip-brew`, `--no-ai`
  - passthrough args accepted
- `generations`
  - subcommands: `status`, `plan`, `prune`
  - options: `--json`
- `generations status`
  - options: `--keep`, `--kind`
- `generations plan`
  - options: `--keep`, `--kind`, `--no-gc`
- `generations prune`
  - options: `--keep`, `--kind`, `--no-gc`, `--yes/-y`, `--dry-run/-n`
- `clean-caches`
  - args: `[caches...]`
  - options: `--only`, `--dry-run/-n`, `--yes/-y`

## 2.4 Exit Code Contract

- `version`: `0`.
- `help`: `0` when help renders successfully; `2` when the requested topic path cannot be resolved.
- `completion`: `0`.
- `doctor`: `0` when all checks pass; `1` when any check fails.
- `generations status`: `0`.
- `generations plan`: `0`.
- `generations prune`: `0` on success, no-op, user cancellation, or `--dry-run`; `1` on discovery, command execution, or post-prune refresh failure.
- `init`: `0` on success or user cancellation; `1` on manifest load/save failure.
- `install`: `0` if all requested install actions succeeded or nothing selected; `1` on partial failure; clap usage errors exit `2`.
- `remove`/`rm`/`uninstall`: `0`; clap usage errors exit `2`.
- `secret add`: `0` on successful update; `1` on input validation, file, or `sops` failure.
- `search`: `0` when at least one result is rendered; `1` on not found or rendering failure.
- `where`: `0` (including not-found); clap usage errors exit `2`.
- `list`: `1` for invalid source filter; otherwise `0`.
- `info`: `0` (including not-found); clap usage errors exit `2`.
- `status`: `0`.
- `installed`: `0` only if all requested packages are installed; clap usage errors exit `2`.
- `profile`: `0` when timing records render successfully; `1` on timing file read/render failure.
- `lint`: `0` when routing metadata passes; otherwise `1`.
- `undo`: `0`.
- `update`: `0` on flake update success, else `1`.
- `test`: `0` if all steps pass, else `1`.
- `rebuild`: `0` on successful rebuild, else `1`.
- `upgrade`: `0` on successful flow, else `1`.

## 3. Repository Discovery And Config Routing

## 3.1 Repo Root Resolution

Resolution order:

1. `NX_REPO_ROOT` env var
2. Walk up from `cwd` looking for `flake.nix`
3. Error if neither found

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
- Location matching may resolve module-style entries such as `programs.<name>`,
  `services.<name>`, and `launchd.(user.)agents.<name>` when those are present in managed config.

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

Note: this bucket scan intentionally stays narrower than `find_package(name)`.
Generic `services.<name>` and `systemd.services.<name>` attrpaths are not counted in the
`services` source bucket.

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

- Turbo/Codex mode (no prompt) refuses and warns: use claude-code or claude engine.
- Claude path may prompt to add flake input unless `--yes`.
- `--dry-run` reports intended flake input action without mutation.

## 7.6 Platform-Incompatible Primary Result Fallback

For nix-based sources:

- Check availability on current platform.
- If unavailable, try next candidate from same source with available attr.
- If no same-source fallback, skip with error.

## 7.7 Engine Execution Semantics

- Default (`--engine=claude-code`): streaming engine via `claude-codes` crate with activity display. Uses Max subscription auth by default (unsets `ANTHROPIC_API_KEY`); set `NX_AI_BILLING=api` for API key billing.
- `--engine=codex`: turbo path via `codex exec`.
- `--engine=claude`: raw Claude CLI path via `claude --print`.
- All engines must consume same `InstallPlan` contract (`package_token`, `target_file`, `insertion_mode`).

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
- Single package: always prints result (success or "not installed" warning). `--show-location` adds file location.
- Multi package: prints summary header with count, each result with location.

## 10. System Command Contracts

## 10.1 `undo`

- Lists modified files from `git status --porcelain`.
- If none: prints `Nothing to undo.` and exits `0`.
- Prompts `Revert all changes?` (default no) unless `--yes` is set, then reverts each modified file using `git checkout -- <file>`.

## 10.2 `update`

- Runs `nix flake update` via shared streaming function.
- Accepts passthrough args.
- Success message instructs `nx rebuild` or `nx upgrade`.

## 10.3 `test`

Runs `just ci` from the repository root through the shared streaming command path.

- Returns `0` when `just ci` succeeds.
- Returns `1` when `just ci` fails or cannot be started.

## 10.4 `rebuild`

Preflight requirements:

0. When `--preflight` is passed, lint routing metadata first and exit after successful checks.
1. Git preflight must succeed.
2. No untracked `.nix` files under `home/`, `packages/`, `system/`, `hosts/`.
3. `nix flake check <repo_root>` must pass.

Then run the default rebuild path:

- `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake <repo_root> [passthrough...]`

Experimental split Darwin rebuild:

- Enabled by default for Darwin manifests. Set `platform.split_rebuild = false` to opt out.
- `NX_SPLIT_DARWIN=1` enables the split path for Darwin repos without a manifest.
- Applies only to Darwin manifests using the default `darwin-rebuild` command and no passthrough args.
- Falls back to the default rebuild path when the split path cannot confidently preserve behavior.
- Runs `nix build --json --no-link <repo_root>#darwinConfigurations.<host>.system`.
- Resolves `<host>` from `NX_DARWIN_HOST`, `scutil --get LocalHostName`, then `hostname -s`.
- If the built system path equals `/nix/var/nix/profiles/system`'s symlink target, exits `0` without profile update or activation. `NX_SYSTEM_PROFILE_PATH` may override the compare target for sandboxed tests.
- Otherwise runs `nix-env -p /nix/var/nix/profiles/system --set <systemConfig>` and `<systemConfig>/activate`, sudo-wrapped when platform sudo is enabled.
- Retries once after clearing Nix git/fetcher caches when flake check or rebuild output reports lazy source object lookup failures.

Routing lint rules:

- Every routable managed `.nix` file under `home/`, `packages/`, `system/`, `hosts/` must start with a non-empty `# nx:` comment.
- Built-in routing keywords must not match more than one routable file.

## 10.4.1 `lint`

Runs the routing lint rules without invoking git, nix, or rebuild commands.

- Returns `0` when all routing checks pass.
- Returns `1` when any routing issue is found.

## 10.5 `upgrade`

High-level phases:

1. Flake phase:
  - load old lock
  - dry-run: skip update
  - non-dry-run with no positional inputs: stream `nix flake update`
  - non-dry-run with positional inputs: stream `nix flake update <input...>`, preserving CLI order
  - load new lock and diff
  - for changed GitHub-backed inputs, run `nix flake prefetch --json github:<owner>/<repo>/<rev>` to force-realize lazy flake sources before check/rebuild
  - fetch change info and summaries
2. Brew phase:
  - repo-wide upgrades run brew unless `--skip-brew`
  - targeted input upgrades skip brew by default
  - `brew outdated --json`
  - enrich and changelog fetch
  - non-dry-run `brew upgrade <pkgs...>`
3. Rebuild unless `--skip-rebuild`
4. Commit `flake.lock` unless `--skip-commit` (and if flake changes exist)

Dry-run behavior:

- Prints dry-run banner.
- No file/system mutation.

## 10.6 `clean-caches`

- Host-scoped and does not require repository discovery.
- Scans known cache directories plus code-root build artifacts and reports sizes before cleaning.
- Optional positional cache names or `--only <names>` limit the scan and clean plan to selected cache names.
- Shows live per-bucket loading feedback during cache sizing; Nix GC and large code roots may take minutes.
- Scans Nix GC last because dead-store estimation may be slower than normal cache directory sizing.
- Reports code-root cache directory counts for grouped build artifact entries.
- Prompts before mutation unless `--yes` is set.
- `--dry-run` reports caches without mutation.
- `NX_CODE_ROOTS` overrides code roots as a colon-separated list. Missing defaults to `~/code`; an empty value disables code-root scans.
- `NX_CLEAN_SCAN_DEPTH` overrides code-root scan and removal depth. Missing or invalid values default to `3`; values above `8` clamp to `8`.
- `NX_CLEAN_SKIP` skips comma-separated cache names and warns for unknown names.

## 11. Upgrade/Changelog Contracts

- `stream_nix_update`:
  - fetches `gh auth token` and passes it via `NIX_CONFIG=access-tokens = github.com=...` when available.
  - runs either `nix flake update` or `nix flake update <input...>` depending on whether targeted inputs were requested.
  - retries once if output indicates known fetcher-cache corruption.
  - corruption retry clears `~/.cache/nix/gitv3` and `~/.cache/nix/fetcher-cache-v4.sqlite`.
- Changed-input prefetch:
  - runs only for changed inputs with a non-empty new revision.
  - uses the same GitHub access token bridge as flake update.
  - retries once after clearing the user nix git/fetcher cache if prefetch output reports lazy source object lookup failures.
  - failure is warning-only; the later flake check/rebuild remains authoritative.
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

## 14. Legacy Compatibility Notes

- Preserve permissive behavior where current CLI does not fail hard:
  - `where` not-found returns `0`
  - `remove` per-item failures do not change command exit
  - `info` not-found returns `0`
- Preserve safety behavior where current CLI fails hard:
  - rebuild preflight failures
  - missing install attr for nix-based sources
  - invalid `list` source filter
- Historical parity notes from earlier implementations are informative only when preserved by
  the current sections above.
