# Nx Generations Command Implementation Plan

Date: 2026-04-02
Source spec: `.design/generations-command-spec.md`

## Objective

Implement the full `nx generations` command family as a host-scoped maintenance surface with deterministic planning and safe execution.

## Workstreams

### 1. CLI and App Routing

Add the `generations` noun and its subcommands to the clap surface, then split command dispatch so host commands can execute without repo discovery.

Deliverables:

- `cli.rs` command/arg types for `generations`
- `app.rs` host-vs-repo command dispatch split
- lightweight host command context
- command help text and exit-code contract updates

### 2. Domain and Planning Model

Introduce explicit typed models for generation discovery, retention policy, prune planning, and execution outcomes.

Deliverables:

- `domain::generations` module
- pure planning functions
- command-independent JSON-serializable models where appropriate
- planner tests

### 3. Discovery and Execution Infrastructure

Implement shell-backed discovery and execution adapters around the typed model.

Deliverables:

- nix-darwin discovery
- Home Manager discovery
- disk-usage snapshotting
- execution adapter for prune + GC
- parsing and adapter tests

### 4. Command Rendering and UX

Implement `status`, `plan`, and `prune` command handlers with human and JSON renderers.

Deliverables:

- `cmd_generations_status`
- `cmd_generations_plan`
- `cmd_generations_prune`
- dry-run parity between `plan` and `prune --dry-run`
- confirmation UX

### 5. Spec and Verification

Bring the behavior contract and tests into alignment with the shipped surface.

Deliverables:

- `.agents/SPEC.md` updates
- command and integration tests
- final `just ci`

## Dependency Order

Recommended implementation order:

1. CLI/app routing
2. domain/planner
3. discovery adapters
4. command handlers/rendering
5. spec updates and full verification

## Done Criteria

The plan is complete when:

- all three commands exist and match the spec
- host dispatch works from outside a repo
- planning is pure and test-covered
- prune behavior is confirmation-gated and deterministic in dry-run mode
- `.agents/SPEC.md` documents the new contract
- `just ci` passes
