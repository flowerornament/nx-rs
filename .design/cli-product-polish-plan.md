# Nx CLI Product Polish Implementation Plan

Date: 2026-04-10
Source spec: `.design/cli-product-polish-spec.md`

## Objective

Implement the full CLI polish pass for `nx`: baseline command affordances, truthful output flags, shared package-query behavior, first-class diagnostics, and documentation parity.

## Workstreams

### 1. CLI Contract and Routing

Deliverables:

- `version`, `doctor`, and `completion` command surface
- root `--version` / `-V`
- required-argument help cleanup
- command metadata and exit-code updates

### 2. Output Contract Cleanup

Deliverables:

- style flags remain global
- `--json` / `--verbose` become command-local and truthful
- command contexts updated to reflect the new contract
- structured output for doctor/lint/version/completion and other supported commands

### 3. Shared Query Pipeline

Deliverables:

- shared package query helper/model reused by `search`, `info`, and install resolution
- consistent source preferences
- cache-hit metadata
- unavailable-backend reporting

### 4. Performance and Determinism

Deliverables:

- deterministic result sorting for equal-priority candidates
- per-phase timing capture for verbose package-query commands
- multi-package read-phase batching retained or improved without changing prompt sequencing

### 5. Help and Documentation Parity

Deliverables:

- curated long-help text for top-level commands
- README command guide updates
- `.agents/SPEC.md` CLI contract updates
- command tests and snapshot-equivalent assertions where useful

## Dependency Order

Recommended execution order:

1. design/spec and bd task setup
2. CLI contract additions (`version`, `doctor`, `completion`)
3. output flag contract cleanup
4. shared query pipeline refactor
5. timing/determinism improvements
6. docs/spec parity and final verification

## Done Criteria

The plan is complete when:

- the CLI exposes the new baseline commands and version flags
- help output is first-class across the command surface
- output flags are truthful
- package-query flows share one deterministic lookup path
- README and `.agents/SPEC.md` match implementation
- `just ci` passes
