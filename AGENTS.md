# nx-rs Agent Guide

## Project Goal

Migrate `nx` from Python to Rust with clean, idiomatic, easy-to-read code while preserving current behavior and safety contracts.

`nx` is a hand-rolled tool for managing the Nix configuration on this--and eventually other--machines. The config is at `~/.nix-config`.

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

## Key Documents

- Behavior contract: `docs/SPEC.md`
- Authoritative migration plan: `docs/MIGRATION_PLAN.md`
- Verified operational learnings: `docs/LEARNINGS.md`
- Python reference implementation: `reference/nx-python/README.md`
- Python source baseline: `reference/nx-python/`
- Legacy context repo: `~/.nix-config`

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
- Use this repo's tracker at `./.beads`.
- Current active migration continuation epic is `nx-rs-0a1`.
- Historical migration execution in `~/.nix-config` was tracked under `morgan-pnv`.
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

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**

- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
