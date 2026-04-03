# Nx Nix Packaging And Module Specification

Status: Proposed
Date: 2026-04-03
Owner: nx-rs

## Goal

Bring `nx-rs`'s Nix integration, packaging layout, and release validation closer to the stronger setup used in `anneal`, while keeping the design fitted to `nx-rs` rather than copying irrelevant surface area.

This work should improve four things:

- clearer flake structure
- declarative Nix-native installation paths
- nix-focused CI coverage
- release confidence for published binaries and Nix consumers

## Product Surface

`nx-rs` should expose:

- `packages.<system>.default`
- `apps.<system>.default`
- `homeManagerModules.default`

The Home Manager module should be intentionally minimal: it exists to install `nx` and manage the environment variables that already define `nx`'s runtime behavior. It should not invent a new config file format or duplicate existing CLI behavior.

## Flake Layout

The flake should stop embedding the full package derivation inline.

Required structure:

- `nix/package.nix`
  - canonical package derivation for `nx`
- `nix/home-manager.nix`
  - exported Home Manager module
- `flake.nix`
  - version constant
  - package/app exports
  - Home Manager module export

The package derivation should remain the single source of truth for `pname`, version wiring, cargo lock usage, and metadata.

## Home Manager Module

### Intent

The module should provide a declarative way to:

- install `nx`
- optionally set `NX_REPO_ROOT`
- optionally disable auto-refresh through `NX_RS_AUTO_REFRESH`

It should not attempt to model every CLI flag or add shell aliases.

### Module Surface

Top-level option namespace:

- `programs.nx`

Required options:

- `programs.nx.enable`
- `programs.nx.package`
- `programs.nx.repoRoot`
- `programs.nx.autoRefresh`

Option semantics:

- `enable`
  - installs `nx` into `home.packages`
- `package`
  - defaults to the flake's packaged `nx`
- `repoRoot`
  - nullable string
  - when non-null, exports `NX_REPO_ROOT`
- `autoRefresh`
  - boolean
  - defaults to `true`
  - when `false`, exports `NX_RS_AUTO_REFRESH=0`

### Non-Goals

The module should not:

- manage shell completions
- manage `bd`
- manage git hooks
- add bespoke config files for `nx`
- export a nix-darwin or NixOS module unless a concrete need appears later

## CI Coverage

GitHub CI should include a Nix-focused smoke test in addition to the Rust/test/install jobs already present.

Required smoke coverage:

- evaluate the exported Home Manager module from the flake
- verify the configured case installs `nx` into `home.packages`
- verify `repoRoot` maps to `NX_REPO_ROOT`
- verify `autoRefresh = false` maps to `NX_RS_AUTO_REFRESH=0`
- verify the bare case installs the package without forcing session variables

This should be implemented as a standalone script under `scripts/` and called from CI and local release verification.

## Release Verification

The release helper should treat nix integration as part of release readiness.

Required release checks:

- package/app/module exports evaluate from the flake
- Home Manager smoke script passes
- release targets stay aligned across:
  - GitHub release workflow
  - installer target list
  - README binary availability list

## Documentation

README should document:

- `nix run github:flowerornament/nx-rs`
- `nix profile install github:flowerornament/nx-rs`
- Home Manager module usage
- available module options

The docs should be clear that:

- the Home Manager module installs the binary and session variables only
- repository-owned behavior still lives in the target config repo, not in the module

## Acceptance Criteria

This effort is complete when:

- package derivation is split into `nix/package.nix`
- `homeManagerModules.default` is exported
- the Home Manager module installs `nx` and manages the supported env vars
- CI runs a nix-focused smoke test
- release verification runs the same nix smoke test
- README documents the Nix and Home Manager install paths
- `just ci` and `just test-system` pass

