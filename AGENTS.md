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
- On the maintainer's machine, `main` runs as `nx-dev` beside release `nx`, sharing `NX_*` env + `~/.local/state/nx/` (timings, undo). Keep on-disk state formats backward-compatible or migrate, else the release binary breaks.

## Version Control With jj

This repo is jj-first. Use Jujutsu for local work; treat Git as the
GitHub/CI/release transport layer. The repo is intentionally colocated, so
`.jj/` and `.git/` live side by side and GitHub still sees ordinary Git commits
and tags.

Do not use Git for day-to-day local work. Use Git commands only when a script
or documented release step needs Git-specific behavior, such as annotated tags
or `ls-remote` verification.

Core model:

- Every task starts as a jj change.
- `main`, `release`, and `beads-sync` are tracked jj bookmarks for the matching
  remote Git branches.
- There is no current branch in jj. Git may report detached HEAD in a colocated
  workspace; that is normal.
- Publishing is explicit. Move or create a bookmark, then `jj git push`.
- Recovery is first-class. Use `jj op log` and `jj undo` before reaching for
  destructive file or history operations.

Daily workflow:

```bash
jj git fetch
jj new main -m "task: short description"
jj status
jj diff
just ci
jj commit -m "area: describe the change"
jj git push --change @-
```

`jj git push --change @-` follows the GitHub-oriented jj workflow: it publishes
the completed change under a generated remote bookmark for review or handoff
without moving `main`.

Direct `main` publication:

```bash
jj git fetch
jj new main -m "task: short description"
# edit, test, and review
jj commit -m "area: describe the change"
jj bookmark move main --to @-
jj git push --bookmark main
```

Use direct `main` publication only when the task is complete, gates have passed,
and pushing `main` is explicitly intended.

Agent/concurrent workspaces:

```bash
jj workspace add ../nx-rs-agent-name --name agent-name -r main -m "agent: task"
```

Prefer one jj workspace per active agent/session when work might overlap. This
keeps each agent's work in a separate recoverable working-copy commit while
sharing the same repository store.

Useful recovery and cleanup commands:

```bash
jj log
jj log -r 'remote_bookmarks()..@'
jj op log
jj undo
jj diff --summary
jj abandon @      # only for an unwanted empty/current change
```

## Toolchain And Workflow

Pinned toolchain:

- Rust `1.94.0` via `rust-toolchain.toml`
- Components: `rustfmt`, `clippy`

Use `just` as the primary entrypoint:

```bash
just --list         # show grouped workflows and what they enforce
just doctor         # verify local toolchain and paths
just hooks-install  # install/update bd hooks
just guard          # strict pre-compile checks
just compile        # strict checks + cargo check
just ci             # timed fmt-check + clippy + test + script tests + check
just ci-record      # same gate, appending timings to .nx/gate-times.csv
```

Quality gates:

| What | Command | Details |
|------|---------|---------|
| Format | `just fmt` / `just fmt-check` | `cargo fmt --all`; check-only variant for CI |
| Lint | `just lint` | `cargo clippy` with `-D warnings`, all targets/features |
| Test | `just test` | `cargo test`, all targets/features |
| Script tests | `just test-scripts` | Python helper/release tests |
| Check | `just check` | `cargo check`, all targets/features |
| **Full CI gate** | **`just ci`** | Timed fmt-check + lint + test + test-scripts + check in sequence |
| Gate timing ledger | `just ci-record` | Same gate, appending a local ignored CSV row to `.nx/gate-times.csv` |
| System tests | `just test-system` | Integration matrix with deterministic stubs |

All flags use `--workspace --all-targets --all-features`. Clippy treats warnings as errors. The lint policy enables `clippy::all`, `clippy::pedantic`, and a curated low-noise subset of `clippy::nursery`; do not enable the whole nursery group without first proving the new warnings improve the loop more than they add churn.

Run `just ci` before finishing any code change. Use `just ci-record` when tuning the development loop or comparing gate cost over time. For release-adjacent changes, also run `just test-system`.

Agent hook pipeline (`just compile` runs this full sequence):

1. `pre-compile.sh` — fmt-check, clippy, test (same as `just guard`)
2. `compile.sh` — calls pre-compile, then `cargo check`
3. `post-compile.sh` — success confirmation

Quality gate enforcement:
- **Primary**: AGENTS.md instruction to run `just ci` before finishing code changes
- **Safety net**: Claude Code Stop hook runs `just check` before session ends
- **bd hooks**: Owned by bd for beads sync (installed via `just hooks-install`)

Justfile conventions:
- `just` and `just --list` are the discoverable command surface. Keep recipe
  comments and `[group(...)]` attributes current instead of maintaining a
  separate hand-written help recipe.
- `just compile` is the authoritative compile path for agents.
- Hooks intentionally fail fast on style, lint, and correctness regressions.
- Release recipes use `just` attributes for safety: `[arg(...)]` validates
  semver arguments, `quote()` escapes shell interpolation, and `[confirm(...)]`
  guards the public tagging step.

## Release Flow

Release automation is local-first and tag-driven. Day-to-day work lands on
`main`; release commits are ordinary commits on `main`; downstream flake
consumers that want the latest published release should track
`refs/heads/release`.

The `release` branch is generated state. `just release-tag` moves it to the
new annotated version tag with `--force-with-lease` after the tag push.

Before bumping, verify shipped behavior is reflected in the docs agents and
users read. CLI help strings are authoritative, but these must match:

- `CHANGELOG.md` — entry for the target version, scaffolded by `release-bump`
- `README.md` — command behavior, install examples, and user-facing workflows
- `.agents/SPEC.md` — behavior-contract changes, when command semantics changed
- `AGENTS.md` — release/process changes, when agent workflow changed

Write docs as if they were always correct, without "added" or "updated"
language.

Canonical sequence:

```bash
jj git fetch
jj new main -m "release: prepare v1.5.25"
just release-bump 1.5.25
# Fill CHANGELOG.md and update docs for shipped user-facing behavior.
jj status
jj diff
jj commit -m "Release v1.5.25"
just release-verify
jj bookmark move main --to @-
jj git push --bookmark main
just release-tag 1.5.25
git ls-remote origin refs/heads/release 'refs/tags/v1.5.25^{}'
```

`just release-verify` intentionally requires a clean working copy. Commit the
release-prep change with `jj commit` before running it so the consumer-flake
smoke tests see the same Git-compatible source that will be tagged.

`just release-verify` checks version alignment across `Cargo.toml`,
`Cargo.lock`, and `flake.nix`; CHANGELOG readiness with no `TODO`/`TBD`
placeholders; then runs `just ci`, `just test-system`, `just build`, Home
Manager module smoke tests, Nix package consumer smoke tests, `nix build .`,
`nix run . -- --help`, and `./target/release/nx --help`.

`just release-tag` creates and pushes `vX.Y.Z`, then publishes
`origin/release` at the same commit. It prompts before running because this is
the public release step; use `just --yes release-tag X.Y.Z` only for explicit
automation. The final `git ls-remote` check should show matching object IDs for
`refs/heads/release` and the peeled tag. Pushing the version tag triggers
`.github/workflows/release.yml`.

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
2. Commit with a clear jj message.
3. Publish intentionally with `jj git push --change @-` or by moving `main`
   and running `jj git push --bookmark main`.
4. `bd dolt push`

Work is not complete until the relevant `jj git push` succeeds.

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
