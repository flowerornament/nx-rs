# Nx Nix Packaging And Module Plan

Date: 2026-04-03
Source spec: `.design/nix-packaging-spec.md`

## Objective

Port the worthwhile nix-side infrastructure from `anneal` into `nx-rs`:

- split nix packaging
- add a fitted Home Manager module
- add nix smoke coverage in CI and release verification

## Workstreams

### 1. Flake And Package Split

Move the package derivation into `nix/package.nix` and keep `flake.nix` focused on exports and version wiring.

Deliverables:

- `nix/package.nix`
- `flake.nix` package/app export cleanup
- version stays centralized and synced

### 2. Home Manager Module

Add `nix/home-manager.nix` and export `homeManagerModules.default`.

Deliverables:

- `programs.nx.enable`
- `programs.nx.package`
- `programs.nx.repoRoot`
- `programs.nx.autoRefresh`
- module evaluation tests via a smoke script

### 3. Nix Verification And Docs

Add nix-focused smoke coverage and document the new install surfaces.

Deliverables:

- `scripts/test-home-manager-module.sh`
- CI job for nix smoke
- release verifier integration
- README Nix/Home Manager docs

## Dependency Order

Recommended order:

1. package split
2. Home Manager module
3. smoke coverage + docs

## Done Criteria

The plan is complete when:

- `packages`, `apps`, and `homeManagerModules` are exported cleanly
- the Home Manager module evaluates and installs `nx` declaratively
- CI and release verification cover the nix surface
- README explains the new Nix install path

