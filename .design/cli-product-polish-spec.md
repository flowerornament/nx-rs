# Nx CLI Product Polish Specification

Status: Proposed
Date: 2026-04-10
Owner: nx-rs

## Goal

Raise `nx` from a capable internal tool to a consistently polished CLI:

- expected baseline affordances must exist
- help output must be trustworthy
- advertised flags must be truthful
- package query flows must share one coherent model
- performance-sensitive commands must expose enough signal to explain latency

This spec defines the intended full surface for the current polish pass. It is not a staged v1/v2 document.

## Product Surface

New top-level commands:

- `nx version`
- `nx doctor`
- `nx completion <shell>`

Existing command families remain in place:

- `help`
- `init`
- `install`
- `remove`
- `secret`
- `search`
- `where`
- `list`
- `info`
- `status`
- `installed`
- `lint`
- `undo`
- `update`
- `test`
- `rebuild`
- `upgrade`
- `generations`

## CLI Contract

### Baseline affordances

The root command must support:

- `nx --version`
- `nx -V`
- `nx version`

The above forms must render the same version string and exit `0`.

### Output flags

Global root flags must be limited to formatting/style concerns:

- `--plain`
- `--unicode`
- `--minimal`

Machine-readable and detail-expanding flags must be command-local, not root-global:

- `--json`
- `--verbose`

If a command advertises `--json`, it must emit valid JSON on stdout with no mixed human chatter.
If a command advertises `--verbose`, it must provide materially more diagnostic detail than the default output.

### Required-argument help

Commands that require an operand as part of normal use must present it as required in clap help rather than accepting an optional-looking arg and failing later with a custom usage error.

### Command help quality

Every top-level command must provide first-class long help with:

- a concise purpose statement
- at least one realistic example
- notes for behavior that users are likely to miss

Nested command families (`secret`, `generations`) must extend that standard to their subcommands.

## Doctor Command

`nx doctor` is repo-scoped and diagnoses both repo and host prerequisites that affect normal `nx` usage.

Doctor output must include:

- resolved repo root
- whether repo discovery used `NX_REPO_ROOT`
- manifest health
- routing audit status
- presence/absence of required tools on PATH
- flake lock availability
- package cache availability/path

Doctor should surface actionable remediation text for each failed check.

`nx doctor --json` must emit a structured report suitable for automation.

## Completion Command

`nx completion <shell>` must emit shell completion scripts for supported clap-complete shells:

- `bash`
- `zsh`
- `fish`
- `elvish`
- `powershell`

The command is read-only and writes the generated script to stdout.

## Shared Package Query Model

`search`, `info`, and install resolution must consume a shared query pipeline with:

- one source preference model
- one cache policy
- one source ordering model
- one unavailable-source reporting model
- one deterministic result sort

Shared query state should distinguish:

- whether the lookup was satisfied from cache
- whether the lookup required live source probing
- per-phase timing measurements
- unavailable backends and reasons

### Source preferences

The shared model must support:

- unstable / bleeding-edge preference
- NUR inclusion
- forced source selection
- explicit cask / MAS targeting

Where a command exposes a subset of preferences, that subset must still map cleanly onto the shared model.

## Search and Info Semantics

### `nx search`

`search` must remain a lightweight source search without repo edits.

`search --verbose` must surface diagnostic detail including, when relevant:

- cache hit/miss
- total lookup duration
- unavailable backends

`search --json` must include enough structure to preserve source, attr, version, confidence, description, cache status, and timing metadata.

### `nx info`

`info` must share the same source preference model as `search` for overlapping concerns.

In particular:

- `--bleeding-edge` must influence source ordering/search behavior, not just ancillary enrichment
- `--nur` must be supported

`info --verbose` must include query diagnostics in addition to richer package metadata.

`info --json` must expose query diagnostics and unavailable backends in structured form.

## Install Resolution Semantics

Install candidate resolution must reuse the shared package query model.

For multi-package installs:

- read-only lookup work should be prefetched in parallel
- interactive prompts and file edits must remain serialized and deterministic

`install --verbose` must surface cache/timing details for package resolution without altering prompt order.

## Determinism and Timing

Package query results must sort deterministically for equal-priority candidates.

Timing support is part of the product contract:

- verbose package-query commands must report enough timing information to explain latency
- cache hits and misses must be visible when verbose mode is enabled

Warm-cache and cold-cache runs may differ in latency, but they should not produce unexplained ordering drift.

## Exit Codes

Additional command exit-code expectations:

- `version`: `0`
- `doctor`: `0` when all checks pass; `1` when any check fails
- `completion`: `0` on success; `2` on invalid shell selection

## Verification

Done criteria for this feature set:

- new commands are implemented and documented
- root baseline affordances behave as specified
- no advertised `--json` or `--verbose` flag is inert
- package query flows share one deterministic lookup pipeline
- verbose output can explain package-query latency
- README and `.agents/SPEC.md` match the implemented CLI contract
- `just ci` passes
