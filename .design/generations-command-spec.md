# Nx Generations Command Specification

Status: Proposed
Date: 2026-04-02
Owner: nx-rs

## Goal

Add a first-class host maintenance surface to `nx` for inspecting, retaining, pruning, and garbage-collecting Nix generations.

This feature is explicitly **host-scoped**, not repo-scoped. It must work from any directory and must not require `NX_REPO_ROOT` or discovery of `flake.nix`.

The command family should replace the need for ad hoc local scripts such as `~/.nix-config/scripts/prune-nix-generations` while preserving the same operational safety and improving composability, determinism, and testability.

## Product Surface

Top-level noun:

- `nx generations`

Subcommands:

- `nx generations status`
- `nx generations plan`
- `nx generations prune`

Optional future expansion is allowed under this noun, but the above three commands define the intended full surface for this feature.

## Command Semantics

### `nx generations status`

Show the current state of known generation families and the host maintenance posture without mutating anything.

Status output must include:

- nix-darwin generation count
- Home Manager generation count
- current generation identifiers for each family when discoverable
- configured/default retention policy
- `/nix` disk usage snapshot
- whether any generations are currently pruneable under the active policy

### `nx generations plan`

Render the exact retention and execution plan for the active policy without mutating the host.

This is the canonical dry-run command. It must be deterministic for a fixed discovered host state.

Plan output must include:

- discovered generations grouped by family
- keep/prune decision for every discovered generation
- rationale for each keep decision
- exact command sequence that `prune` would execute
- before-state disk usage
- whether garbage collection would run

### `nx generations prune`

Execute the active prune plan.

Behavior:

- computes the same plan as `plan`
- prompts for confirmation by default
- executes generation removals
- optionally runs garbage collection
- reports after-state disk usage
- renders a final summary with exact outcomes

If no generations are eligible for pruning and GC is disabled, the command should be a no-op success.

If no generations are eligible for pruning and GC is enabled, the command may still run GC if the user requested it or if the default policy includes it.

## Flags

Shared flags:

- `--json`
- `--plain`
- `--unicode`
- `--minimal`

Retention and execution flags:

- `--keep <N>`
- `--kind <all|darwin|home-manager>`
- `--no-gc`
- `--yes`

Dry-run compatibility:

- `nx generations prune --dry-run` must behave identically to `nx generations plan`

Defaults:

- `keep = 10`
- `kind = all`
- `gc = enabled`
- confirmation required for `prune`

## Domain Model

The implementation should encode this feature around explicit typed planning rather than shell-script orchestration.

### Core types

- `GenerationKind`
  - `DarwinSystem`
  - `HomeManager`
- `GenerationId`
- `GenerationRecord`
- `RetentionPolicy`
- `RetentionDecision`
- `PrunePlan`
- `ExecutionPlan`
- `ExecutionOutcome`
- `DiskUsageSnapshot`

### `GenerationRecord`

Represents a single discovered generation with normalized fields:

- family/kind
- identifier
- creation timestamp if discoverable
- current/active marker
- raw display label
- raw source metadata needed for rendering or execution

### `RetentionPolicy`

Represents host cleanup policy independently of rendering or command execution.

Required fields:

- `keep_newest: usize`
- `kinds: set of GenerationKind`
- `run_gc: bool`

The policy model must be extensible for future rules, but this feature is defined around the complete current rule set above.

### `RetentionDecision`

Each discovered generation must be classified into exactly one outcome:

- `KeepCurrent`
- `KeepByRetentionWindow`
- `Prune`

The planner must preserve the current generation for a family even if it would otherwise fall outside the newest `N`.

## Discovery Model

Discovery must be separated from planning and execution.

### nix-darwin discovery

Discover generations from `/nix/var/nix/profiles/system-*-link`.

The implementation must normalize:

- numeric generation id
- symlink target if needed
- active/current profile

### Home Manager discovery

Discover generations from `home-manager generations`.

The implementation must parse CLI output into structured records and normalize ids so that planner logic is independent of shell output formatting.

### Disk usage discovery

Discover `/nix` disk usage before execution and after execution.

The human renderer may show `df -h` style output. The internal model should not depend on locale-sensitive terminal formatting.

## Planning Rules

Planning must be pure once discovery data is available.

Rules:

1. Partition generations by family.
2. Sort each family newest-first using normalized identity/time ordering.
3. Mark the active/current generation as kept.
4. Keep the newest `N` generations in each selected family.
5. Mark all remaining selected generations for prune.
6. Excluded families are reported but not scheduled for deletion.
7. Build an execution plan from the prune decisions plus optional GC.

Planning must never shell out.

## Execution Model

Execution must consume a previously built plan.

Execution phases:

1. optional confirmation
2. prune Home Manager generations
3. prune nix-darwin generations
4. run `nix-collect-garbage -d` unless disabled
5. re-snapshot disk usage
6. render outcome

Execution should preserve phase boundaries so partial failures can be reported precisely.

## Shell Commands

The intended underlying commands are:

- Home Manager prune:
  - `home-manager remove-generations <ids...>`
- nix-darwin/system prune:
  - `sudo nix-env --delete-generations --profile /nix/var/nix/profiles/system <ids...>`
- garbage collection:
  - `sudo nix-collect-garbage -d`

The implementation may use alternative commands only if behavior is equivalent and the spec/tests are updated accordingly.

## Output Contract

### Human output

Human output should be concise but explicit. It must:

- show discovered counts
- show keep/prune preview
- show exact generation ids to prune
- show exact commands during `plan`
- show before/after disk usage around `prune`
- clearly indicate when nothing will be removed

### JSON output

JSON output should expose:

- discovery results
- normalized retention policy
- decisions
- execution plan
- execution result when applicable

JSON output must preserve enough structure for external tooling to diff plans and reason about safety without scraping human text.

## Error Handling

Failure modes must remain specific.

Categories:

- discovery failure
- parse failure
- confirmation cancellation
- prune command failure
- garbage collection failure
- post-execution status refresh failure

Required behavior:

- user cancellation exits successfully with no changes
- discovery or parse failure exits non-zero before any mutation
- prune failure exits non-zero and reports exactly which phase failed
- GC failure after successful pruning exits non-zero and reports partial success

## App Integration

`nx generations` must be implemented as a **host command**.

This implies an app-architecture change:

- host commands must bypass repo-root discovery
- repo-bound commands must retain existing behavior

Introduce a lightweight host context rather than forcing this command through `AppContext`.

## Testing Requirements

The feature is not complete without tests covering:

- nix-darwin generation discovery parsing
- Home Manager generation parsing
- policy planning for mixed generation sets
- current-generation preservation
- `--kind` filtering
- `--no-gc`
- `prune --dry-run` parity with `plan`
- no-op plan rendering
- partial execution failure reporting
- host-command dispatch without a repo root

Favor unit tests for planning and parsing, and command-level tests for dispatch/UX boundaries.

## Non-Goals

These are explicitly out of scope for this feature even in its full vision:

- semantic analysis of why store paths are retained
- editing nix config files
- pinning generations from inside `nx`
- cross-host retention policies
- automatic scheduled cleanup

## Success Criteria

The feature is successful when:

- `nx generations` works from any directory
- dry-run plans are deterministic for a fixed host state
- the planner is testable without shelling out
- the mutating command is safer and clearer than the current standalone script
- users no longer need a separate prune helper script for standard host generation cleanup
