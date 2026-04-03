# Nx Nix-Native Configuration Specification

Status: Proposed
Date: 2026-04-03
Owner: nx-rs

## Goal

Define a configuration model for `nx-rs` that actually fits a Nix-native tool.

`nx-rs` should not grow a broad application-owned config system just because one
is technically possible. The right target is:

- repo-owned policy stays in the managed Nix repo
- machine/session defaults are configured declaratively through Nix/Home Manager
- CLI flags remain the per-invocation override layer
- environment variables remain a compatibility layer, not the primary product surface

## Design Principle

`nx-rs` has three configuration scopes:

### 1. Repo-Owned Behavior

This remains the primary source of truth.

Examples:

- flake inputs
- package declarations
- manifest/routing annotations
- nix file structure
- rebuild behavior derived from the managed repo

This scope must not move into user config.

### 2. Machine / Session Defaults

This is the right place for declarative Home Manager configuration.

Examples:

- install `nx`
- set `NX_REPO_ROOT`
- disable auto-refresh
- ensure `sops` is available
- select the `sops` binary path
- optionally set safe `generations` planning defaults

### 3. Per-Invocation Overrides

This remains the CLI layer.

Examples:

- `--dry-run`
- `--yes`
- `--no-ai`
- `--engine`
- `--model`
- `--skip-*`

CLI must continue to override lower-precedence defaults where applicable.

## Explicit Non-Goal

Do not introduce a standalone `nx` app config file such as:

- `~/.config/nx/config.toml`
- `nx.toml`

unless we later identify a concrete non-Nix distribution/use case that truly
needs it.

For the actual `nx-rs` audience, Home Manager is the more natural declarative
surface.

## Home Manager As The Primary Declarative Surface

Top-level namespace:

- `programs.nx`

The Home Manager module should be the primary persistent configuration surface
for user/machine defaults.

It should remain intentionally narrow.

### Current supported surface

- `programs.nx.enable`
- `programs.nx.package`
- `programs.nx.repoRoot`
- `programs.nx.autoRefresh`

### Proposed extension

Add a small `sops` sub-surface:

- `programs.nx.sops.package`
- `programs.nx.sops.bin`

Semantics:

- `sops.package`
  - nullable package
  - when non-null, add to `home.packages`
- `sops.bin`
  - nullable string
  - when non-null, export `NX_RS_SOPS_BIN`

### Possible future extension

Only if there is clear value, add a small `generations` default surface for
safe planning defaults:

- `programs.nx.generations.keep`
- `programs.nx.generations.kind`
- `programs.nx.generations.gc`

This should remain optional and should never include destructive confirmation
bypasses.

## Validation Rules

- `repoRoot` must not be empty when set
- `sops.bin` must not be empty when set
- `sops.package` and `sops.bin` may both be set
- future `generations` defaults must validate against the same safe domain the
  CLI accepts

## Precedence

The precedence model should remain simple:

1. CLI flags
2. environment variables
3. Home Manager-rendered session defaults
4. autodetection / built-in defaults

Home Manager participates by writing session variables and installing packages.
It should not create another unrelated precedence layer.

## Explicit Non-Goals For Persistent Config

Do not add persistent defaults for:

- `yes`
- `dry-run`
- `skip-rebuild`
- `skip-commit`
- `skip-brew`
- destructive undo/prune confirmation bypasses

Do not add persistent Home Manager config for:

- routing policy
- manifest policy
- AI/provider defaults unless there is a very strong need
- output style profiles unless the lack of them becomes a real pain point

## Documentation Requirements

README should explain:

- repo-owned behavior vs machine/session defaults
- that `programs.nx` is intentionally thin
- how to configure `repoRoot`, `autoRefresh`, and `sops`
- that the managed repo remains the source of truth for operational behavior

## Verification

Required verification:

- Home Manager smoke tests for supported options
- release verification includes the Home Manager smoke test
- docs stay aligned with the module surface

## Acceptance Criteria

This effort is complete when:

- the nix-native configuration boundary is explicit
- `programs.nx` cleanly models the approved machine/session defaults
- supported `sops` settings are covered by smoke tests
- README reflects the intended model and non-goals

