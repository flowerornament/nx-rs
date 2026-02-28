# nx-rs Agent Guide

## Project Goal

Maintain and evolve `nx-rs` as the canonical `nx` implementation with clean, idiomatic, easy-to-read Rust while preserving behavior and safety contracts.

`nx` is a hand-rolled tool for managing the Nix configuration on this--and eventually other--machines. The config is at `~/.nix-config`.

Primary priorities:

- clarity
- robustness
- low LOC
- functional style

## Guidelines

1. Treat `.agents/SPEC.md` as the behavior contract.
2. Treat `.agents/MIGRATION_PLAN.md` as historical migration context; use `.agents/CUTOVER_PLAYBOOK.md` and `.agents/SPEC.md` for current maintenance decisions.
3. Prefer libraries when they simplify code or reduce LOC.
4. Use a functional-first style: pure transforms and explicit side-effect boundaries.
5. Design types up front to encode invariants before implementing command flows.
6. Parity fidelity at the code-structure, syntactic, and semantic level with the Python project is not required.
7. Keep the feedback loop tight: run strict checks frequently via `just`.
8. Actively tend to and enrich the agentic feedback loop. Intelligently make tests and checks happen automatically at the right times in the feedback loop.

## Key Documents

- Behavior contract: `.agents/SPEC.md`
- Migration history/context: `.agents/MIGRATION_PLAN.md`
- Verified operational learnings: `.agents/LEARNINGS.md`
- Cutover runbook and rollback criteria: `.agents/CUTOVER_PLAYBOOK.md`
- Archived Python reference implementation (historical): `~/code/nx-python/`
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

Quality gates:

| What | Command | Details |
|------|---------|---------|
| Format | `just fmt` / `just fmt-check` | `cargo fmt --all`; check-only variant for CI |
| Lint | `just lint` | `cargo clippy` with `-D warnings`, all targets/features |
| Test | `just test` | `cargo test`, all targets/features |
| Check | `just check` | `cargo check`, all targets/features |
| **Full CI gate** | **`just ci`** | fmt-check + lint + test + check in sequence |
| System tests | `just test-system` | Integration matrix with deterministic stubs |
| Cutover validation | `just cutover-validate` | Rust direct/canary validation on `~/.nix-config` |

All flags use `--workspace --all-targets --all-features`. Clippy treats warnings as errors.

Run `just ci` before finishing any code change. For release-adjacent changes, also run `just test-system` and `just cutover-validate`.

Agent hook pipeline (`just compile` runs this full sequence):

1. `pre-compile.sh` — fmt-check, clippy, test (same as `just guard`)
2. `compile.sh` — calls pre-compile, then `cargo check`
3. `post-compile.sh` — success confirmation
4. `.githooks/pre-commit` — runs pre-compile on every commit

## Issue Tracking

This project uses **bd** (beads) for task management. Run `bd help` for commands.

Tracking model:

- `bd` is the only source of truth for executable tasks, dependencies, and status.
- Do not track task checklists or status in markdown docs.
- Use this repo's tracker at `./.beads`.
- Use `bd ready` to identify the active epic/task set for the current session.
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
   # If shipping a release change, bump version first (Cargo.toml + flake.nix).
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

## Rust Guidelines

- Always use /rust-skills and /rust-router skills when writing Rust code, choosing the right sub-skills skills for the current job
- Avoid deep nesting
- Modularize
- Use abstraction
- Plan before acting
- Understand the codebase idioms
- Run `just ci` and fix all issues before finishing (see Quality Gates above)

## Command Module Conventions

- Keep command handlers split by concern under `src/commands/<area>/` when a module starts accumulating unrelated flows.
- Keep one public command entrypoint per concern module (`cmd_*`) and re-export from the parent `mod.rs` to preserve stable call sites.
- Keep orchestration functions as ordered phases (`start`/`prepare`/`apply`) and push side effects (shelling out, file edits, prompts) into leaf helpers.
- Add or update targeted tests when introducing new orchestration boundaries or shared helper contracts.
