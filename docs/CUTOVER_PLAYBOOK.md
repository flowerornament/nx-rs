# nx-rs Cutover Playbook

Purpose: define and execute a deploy-safe validation workflow before replacing Python `nx` in PATH.

## Safety Boundaries

- Validation target repo: `~/.nix-config`
- Read-only and dry-run commands only for canary validation
- No `update` or `rebuild` in this checklist (mutating/system-level)
- Mutation safety gate: git status in `~/.nix-config` must be unchanged before/after checks

## Execution Command

Run from `nx-rs` repo:

```bash
just cutover-validate
```

Equivalent direct command:

```bash
scripts/cutover/validate_shadow_canary.sh
```

## Checklist

1. Build and resolve binaries
- Python baseline: `~/.nix-config/scripts/nx/nx`
- Rust candidate: `target/debug/nx-rs` (auto-built if missing)

2. Shadow matrix (Python vs Rust)
- Compare exit code, normalized stdout, normalized stderr for:
  - `where <installed package>`
  - `where <missing package>`
  - `list --plain`
  - `status`
  - `installed <installed package> --json`
  - `info <missing package> --json`
  - `install --dry-run <installed package>`
  - `remove --dry-run <installed package>`

3. Canary matrix (PATH-preferred nx-rs)
- Execute `nx` via temporary PATH override pointing to `nx-rs` for:
  - `status`
  - `where <installed package>`
  - `installed <installed package> --json`

4. Mutation safety
- Compare `git -C ~/.nix-config status --porcelain=v1 --untracked-files=all`
  before and after all checks.

## Latest Execution Result

Executed: **2026-02-12 02:30:12 PST**

Shadow matrix:

| Case | Command Args | Python Exit | Rust Exit | Stdout Match | Stderr Match | Pass |
| --- | --- | --- | --- | --- | --- | --- |
| where_found | `where ast-grep` | 0 | 0 | yes | yes | yes |
| where_not_found | `where not-a-real-package-nxrs-cutover` | 0 | 0 | yes | yes | yes |
| list_plain | `list --plain` | 0 | 0 | yes | yes | yes |
| status | `status` | 0 | 0 | yes | yes | yes |
| installed_json | `installed ast-grep --json` | 0 | 0 | yes | yes | yes |
| info_json_not_found | `info not-a-real-package-nxrs-cutover --json` | 0 | 0 | yes | yes | yes |
| install_dry_run | `install --dry-run ast-grep` | 0 | 0 | yes | yes | yes |
| remove_dry_run | `remove --dry-run ast-grep` | 0 | 0 | yes | yes | yes |

Canary matrix:

| Case | Command | Exit | Pass |
| --- | --- | --- | --- |
| status | `nx --plain --minimal status` | 0 | yes |
| where_found | `nx --plain --minimal where ast-grep` | 0 | yes |
| installed_json | `nx --plain --minimal installed ast-grep --json` | 0 | yes |

Mutation safety:

- Git status unchanged: **yes**

## Go/No-Go Criteria

GO for full PATH replacement only if all are true:

1. Shadow matrix: all cases pass (exit + stdout + stderr)
2. Canary matrix: all cases pass
3. Mutation safety: unchanged git status
4. `just compile` passes in `nx-rs`

NO-GO if any gate fails.

## Current Decision

As of **2026-02-12**: **GO** for full production PATH replacement according to this checklist.

Notes:
- The prior `sops-nix` service parity gap was fixed by including `default.nix` in Rust package scanning.
- Rollout can proceed with staged PATH updates and rollback readiness below.

## Rollback Steps

If a canary or cutover attempt causes issues, restore Python `nx` immediately:

1. Remove or disable nx-rs PATH override/symlink.
- If you created `~/.local/bin/nx` symlink to nx-rs:

```bash
rm -f ~/.local/bin/nx
```

2. Ensure Python `nx` resolves first:

```bash
command -v nx
# expected: /Users/morgan/.nix-config/scripts/nx/nx
```

3. Force shell command hash refresh:

```bash
hash -r
```

4. Smoke check with Python baseline:

```bash
/Users/morgan/.nix-config/scripts/nx/nx --plain --minimal status
```

5. If any unexpected file changes occurred in `~/.nix-config`, inspect and revert manually:

```bash
git -C ~/.nix-config status --short
git -C ~/.nix-config diff
```
