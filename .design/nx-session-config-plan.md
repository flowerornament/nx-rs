# Nx Nix-Native Configuration Plan

Date: 2026-04-03
Source spec: `.design/nx-session-config-spec.md`

## Objective

Keep `nx-rs` repo-first and Nix-native by using Home Manager as the declarative
surface for machine/session defaults, instead of introducing a standalone app
config system.

## Workstreams

### 1. Policy And Docs

Document the intended configuration boundary.

Deliverables:

- `.design/nx-session-config-spec.md`
- README clarification about repo-owned vs Home Manager-owned behavior

### 2. Home Manager Module Extension

Extend the module only where it clearly fits the nix-native session-default model.

Deliverables:

- `programs.nx.sops.package`
- `programs.nx.sops.bin`
- validation and descriptions

### 3. Verification

Update smoke coverage and release verification for the supported module surface.

Deliverables:

- Home Manager smoke coverage for `sops` package/env behavior
- release verification remains green

## Dependency Order

Recommended order:

1. policy/doc shape
2. module extension
3. smoke coverage

## Done Criteria

The plan is complete when:

- the configuration boundary is explicit
- Home Manager remains intentionally thin and tool-appropriate
- supported `sops` settings are smoke-tested
- docs reflect the final nix-native model

