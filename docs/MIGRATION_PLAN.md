# nx-rs Authoritative Migration Plan

## Baseline

- Legacy Python implementation is preserved at `reference/nx-python/`.
- Original source at `../nx` remains untouched.
- Architecture context reviewed from:
  - `../../AGENTS.md`
  - `../../.agents/ARCHITECTURE.md`

## Goal And Constraints

- Goal: rewrite `nx` as clean, idiomatic, easy-to-read Rust.
- Priorities: clarity, robustness, low LOC.
- Non-priority: micro-optimizations/perf tuning.
- Functional target: preserve current CLI behavior and safety contracts.

## Locked Strategy

Authoritative migration strategy: **Hybrid contract-first port**.

Execution model:

1. Treat `docs/SPEC.md` as migration contract.
2. Implement fresh Rust design against that contract.
3. Reference Python code only when behavior is ambiguous.
4. Lock behavior with parity fixtures before broad Rust feature work.

## Locked Decisions

These decisions are no longer optional.

1. Scope: full command parity remains the target, delivered in phased checkpoints.
2. Parity target: semantic parity (behavior, exit codes, file edits), not byte-for-byte terminal text parity.
3. AI paths: in scope for v1, but only after deterministic edit engine is complete.
4. Platform: darwin-first contract (matching current tool intent and tested behavior).
5. Toolchain policy: pin exact stable Rust toolchain in `rust-toolchain.toml` before writing Rust source.
6. Dependency policy: library-first where it clearly simplifies implementation or reduces LOC.
7. Programming style: functional-first style (immutable data flow, pure transforms, side effects isolated at boundaries).
8. Design policy: type design happens up front; encode invariants in types before implementing command logic.
9. Feedback loop policy: strict style/lint/correctness checks are run automatically in the compile workflow via agent hooks.
10. Task runner policy: `just` is mandatory as the primary workflow entrypoint and should remain self-documenting.
11. Agent management policy: prefer proven libraries over custom ad-hoc wrappers when integrating agent/workflow behavior.
12. Compatibility policy: no intentional behavior changes during migration unless explicitly approved.
13. Migration done criteria: `fmt`, `clippy -D warnings`, full tests, and parity fixtures for critical flows.

## Why This Is Best For Agentic Coding

- Agentic loops perform best with explicit acceptance criteria.
- Parity fixtures reduce hallucinated behavior changes.
- Small vertical slices keep changes reviewable and recoverable.
- Typed Rust domain models reduce ambiguous state and edge-case bugs.

## Conversion Principles

- `main.rs` thin; logic in `lib.rs`.
- Strong typed domain structs for plans/results/options.
- One subprocess boundary layer for all shell calls.
- Deterministic file editing core (line/block/list operations).
- AI engine integrations isolated as adapters, not in core command logic.
- Keep modules focused and small; split when files exceed readability.

## Target Rust Architecture

```text
nx-rs/
├── Cargo.toml
├── src/
│   ├── main.rs                 # CLI entrypoint only
│   ├── lib.rs                  # crate wiring
│   ├── cli.rs                  # clap parser + default install injection
│   ├── app.rs                  # app state/bootstrap
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── install.rs
│   │   ├── remove.rs
│   │   ├── query.rs            # where/list/info/status/installed
│   │   └── system.rs           # undo/update/test/rebuild/upgrade
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── config.rs           # ConfigFiles contract
│   │   ├── source.rs           # SourceResult/SourcePreferences/PackageInfo
│   │   ├── plan.rs             # InstallPlan contract
│   │   └── upgrade.rs          # lock/brew change models
│   ├── infra/
│   │   ├── mod.rs
│   │   ├── shell.rs            # run_command/run_streaming command wrapper
│   │   ├── config_scan.rs
│   │   ├── finder.rs
│   │   ├── cache.rs
│   │   ├── sources.rs
│   │   ├── file_edit.rs        # deterministic text edits
│   │   └── ai/
│   │       ├── mod.rs
│   │       ├── codex.rs
│   │       └── claude.rs
│   └── output/
│       ├── mod.rs
│       └── printer.rs
├── tests/
│   ├── cli_parity.rs
│   ├── finder_parity.rs
│   ├── install_plan_parity.rs
│   └── fixtures/
└── reference/
    └── nx-python/              # frozen legacy reference
```

## Dependency Plan (Keep Minimal)

- `clap` (derive) for CLI.
- `anyhow` for binary-level errors + context.
- `serde`, `serde_json` for lock/cache/json output.
- `regex` for package parsing/editing patterns.
- `thiserror` only if we expose reusable library errors; otherwise skip.
- `which` for command availability checks.
- `tempfile` for integration tests.
- `assert_cmd`, `predicates` for CLI tests.
- `tracing`, `tracing-subscriber` for structured diagnostics during iterative migration.
- `duct` (or equivalent) for concise process orchestration where it reduces subprocess boilerplate.

## Execution Phases (Tracked In bd)

Phase structure is strategic only here; all deliverables, acceptance criteria,
and status are tracked in `bd` under epic `morgan-pnv` (tracking conventions live in `AGENTS.md`).

1. `phase:-1`: toolchain and guardrails (`just`, hooks, strict checks).
2. `phase:0`: contract freeze and parity fixture foundation.
3. `phase:1`: rust skeleton and CLI contract.
4. `phase:2`: read-only core (config/finder/query commands).
5. `phase:3`: sources, cache, and install planning.
6. `phase:4`: deterministic mutations, then AI adapters.
7. `phase:5`: system command parity.
8. `phase:6`: upgrade workflow parity.
9. `phase:7`: hardening, simplification, final parity gates.

## Parity Harness Direction

- Use fixture-driven black-box parity checks.
- Compare semantic behavior: exit codes, key output semantics, and file diffs.
- Prioritize high-risk flows first (routing, fallback, dry-run, rebuild preflight).

## Risks And Mitigations

- Risk: accidental behavior drift.
  - Mitigation: fixture parity and explicit spec before implementation.
- Risk: Rust code grows larger than needed.
  - Mitigation: keep core deterministic, isolate optional AI adapters.
- Risk: brittle text parsing/editing.
  - Mitigation: table-driven tests with real `.nix` snippets and idempotency checks.
- Risk: command execution side effects in tests.
  - Mitigation: shell abstraction + mocked command runner.

## Implementation Order

Toolchain -> contracts/fixtures -> read-only core -> install planning ->
deterministic edits -> AI adapters -> upgrade workflow -> hardening.
