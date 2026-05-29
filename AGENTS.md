# nx-rs Agent Guide

## Project Goal

Maintain and evolve `nx-rs` with clean, idiomatic, easy-to-read Rust while preserving behavior and safety contracts.

`nx` is a hand-rolled tool for managing Nix configuration repositories.

Primary priorities:

- clarity
- robustness
- low LOC
- functional style

## Guidelines

1. Treat `.agents/SPEC.md` as the behavior contract.
2. Use `.agents/SPEC.md` as the primary maintenance decision reference.
3. Prefer libraries when they simplify code or reduce LOC.
4. Use a functional-first style: pure transforms and explicit side-effect boundaries.
5. Design types up front to encode invariants before implementing command flows.
6. Keep the feedback loop tight: run strict checks frequently via `just`.
7. Actively tend to and enrich the agentic feedback loop. Intelligently make tests and checks happen automatically at the right times in the feedback loop.

## Key Documents

- Behavior contract: `.agents/SPEC.md`
- Managed repo root override: `NX_REPO_ROOT`

## Toolchain And Workflow

Pinned toolchain:

- Rust `1.94.0` via `rust-toolchain.toml`
- Components: `rustfmt`, `clippy`

Use `just` as the primary entrypoint:

```bash
just help           # show workflows and what they enforce
just doctor         # verify local toolchain and paths
just hooks-install  # install/update bd git hooks
just guard          # strict pre-compile checks
just compile        # strict checks + cargo check
just ci             # fmt-check + clippy + test + script tests + check
```

Quality gates:

| What | Command | Details |
|------|---------|---------|
| Format | `just fmt` / `just fmt-check` | `cargo fmt --all`; check-only variant for CI |
| Lint | `just lint` | `cargo clippy` with `-D warnings`, all targets/features |
| Test | `just test` | `cargo test`, all targets/features |
| Script tests | `just test-scripts` | Python helper/release tests |
| Check | `just check` | `cargo check`, all targets/features |
| **Full CI gate** | **`just ci`** | fmt-check + lint + test + test-scripts + check in sequence |
| System tests | `just test-system` | Integration matrix with deterministic stubs |

All flags use `--workspace --all-targets --all-features`. Clippy treats warnings as errors.

Run `just ci` before finishing any code change. For release-adjacent changes, also run `just test-system`.

Agent hook pipeline (`just compile` runs this full sequence):

1. `pre-compile.sh` — fmt-check, clippy, test (same as `just guard`)
2. `compile.sh` — calls pre-compile, then `cargo check`
3. `post-compile.sh` — success confirmation

Quality gate enforcement:
- **Primary**: AGENTS.md instruction to run `just ci` before finishing code changes
- **Safety net**: Claude Code Stop hook runs `just check` before session ends
- **Git hooks**: Owned by bd for beads sync (installed via `just hooks-install`)

## Task Tracking (bd)

```bash
# orient
bd show --current --short
bd query "status=in_progress"
bd ready --explain

# work
bd update <id> --claim
bd note <id> "context"
bd close <id> --suggest-next

# capture
bd todo add "quick thought"
bd create --title="..." --type=task --priority=2

# query
bd query "type=bug AND priority<=1 AND updated>7d"
bd search "keyword"
bd count "status=open"
bd graph --compact <id>

# state
bd kv set/get key [value]
bd find-duplicates
```

Full ref: `bd prime`

## Completion

Before ending a session:
1. Run `just ci` if code changed.
2. Commit with a clear message.
3. `bd dolt push && git push`

Work is not complete until `git push` succeeds.

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
