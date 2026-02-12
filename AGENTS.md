# nx-rs Agent Guide

## Project Goal

Migrate `nx` from Python to Rust with clean, idiomatic, easy-to-read code while preserving current behavior and safety contracts.

Primary priorities:

- clarity
- robustness
- low LOC
- functional style

## Guidelines

1. Treat `docs/SPEC.md` as the behavior contract.
2. Follow `docs/MIGRATION_PLAN.md` as the authoritative implementation plan.
3. Prefer libraries when they simplify code or reduce LOC.
4. Use a functional-first style: pure transforms and explicit side-effect boundaries.
5. Design types up front to encode invariants before implementing command flows.
6. Preserve semantic parity with Python unless a change is explicitly approved.
7. Keep the feedback loop tight: run strict checks frequently via `just`.
8. Actively tend to and enrich the feedback loop. Intelligently make tests and checks happen automatically at the right times.

## Toolchain And Workflow

Pinned toolchain:

- Rust `1.92.0` via `rust-toolchain.toml`
- Components: `rustfmt`, `clippy`

Use `just` as the primary entrypoint:

```bash
just help           # show workflows and what they enforce
just doctor         # verify local toolchain and paths
just hooks-install  # install git pre-commit hook
just guard          # strict pre-compile checks
just compile        # strict checks + cargo check
just ci             # fmt-check + clippy + test + check
```

Agent hooks:

- `scripts/agent-hooks/pre-compile.sh`
- `scripts/agent-hooks/compile.sh`
- `scripts/agent-hooks/post-compile.sh`
- `.githooks/pre-commit`

## Issue Tracking

This project uses **bd** (beads) for task management. Run `bd help` for commands.

Tracking model:

- `bd` is the only source of truth for executable tasks, dependencies, and status.
- Do not track task checklists or status in markdown docs.
- Migration execution lives under epic `morgan-pnv`.
- Do not track any state in markdown docs
- Avoid duplicating information between AGENTS.md and other markdown docs
- Do not create additional documents unless new categories of information need to be recorded

Quick reference:

```bash
bd prime              # full context about using bd
bd ready              # find available work
bd create --title="..." --type=task --priority=2
bd close <id>         # complete work
bd sync               # sync state (run at session end)
```

## Key Documents

- Behavior contract: `docs/SPEC.md`
- Authoritative migration plan: `docs/MIGRATION_PLAN.md`
- Python reference implementation: `reference/nx-python/README.md`
- Python source baseline: `reference/nx-python/`
- Parent nix-config context: `../../AGENTS.md`
- Parent architecture guide: `../../.agents/ARCHITECTURE.md`
