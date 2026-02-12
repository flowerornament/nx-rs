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

Executed: **2026-02-12 03:26:11 PST**

Shadow matrix:

| Case | Command Args | Python Exit | Rust Exit | Stdout Match | Stderr Match | Pass |
| --- | --- | --- | --- | --- | --- | --- |
| where_found | `where ripgrep` | 0 | 0 | yes | yes | yes |
| where_not_found | `where not-a-real-package-nxrs-cutover` | 0 | 0 | yes | yes | yes |
| list_plain | `list --plain` | 0 | 0 | yes | yes | yes |
| status | `status` | 0 | 0 | yes | yes | yes |
| installed_json | `installed ripgrep --json` | 0 | 0 | yes | yes | yes |
| info_json_not_found | `info not-a-real-package-nxrs-cutover --json` | 0 | 0 | yes | yes | yes |
| install_dry_run | `install --dry-run ripgrep` | 0 | 0 | yes | yes | yes |
| remove_dry_run | `remove --dry-run ripgrep` | 0 | 0 | yes | yes | yes |

Canary matrix:

| Case | Command | Exit | Pass |
| --- | --- | --- | --- |
| status | `nx --plain --minimal status` | 0 | yes |
| where_found | `nx --plain --minimal where ripgrep` | 0 | yes |
| installed_json | `nx --plain --minimal installed ripgrep --json` | 0 | yes |

Mutation safety:

- Git status unchanged: **yes**

Post-run note:
- Verified after moving legacy in-tree copy from `~/.nix-config/scripts/nx-rs` to `/tmp/nx-rs-legacy-20260212-032055`.

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

## Decommission Task: In-Tree `nx-rs` Copy

Target to decommission after standalone repo cutover is canonical:
`~/.nix-config/scripts/nx-rs`.

### Preconditions

1. Standalone repo (`/Users/morgan/code/nx-rs`) is canonical for development and issue tracking.
2. `just compile` passes in standalone repo.
3. `just cutover-validate` passes (shadow + canary + mutation safety).
4. Current `~/.nix-config` git status is known before decommission starts.

### Reference Audit (hooks/path/tooling)

Run before moving legacy directory:

```bash
# from /Users/morgan/code/nx-rs
rg -n --hidden --glob '!.git' '~/.nix-config/scripts/nx-rs|\.nix-config/scripts/nx-rs|scripts/nx-rs'

# from ~/.nix-config
cd ~/.nix-config
rg -n --hidden --glob '!.git' --glob '!scripts/nx-rs/**' 'scripts/nx-rs|\.nix-config/scripts/nx-rs'
```

Expected result: no matches outside `~/.nix-config/scripts/nx-rs`.
Audit status on **2026-02-12**: passed.

### Decommission Sequence

1. Capture pre-state and backup target path.

```bash
STAMP="$(date +%Y%m%d-%H%M%S)"
LEGACY_DIR="$HOME/.nix-config/scripts/nx-rs"
LEGACY_ARCHIVE="$HOME/.nix-config/scripts/nx-rs.decommission-$STAMP"
git -C ~/.nix-config status --porcelain=v1 --untracked-files=all > /tmp/nxrs-decom-git-before.txt
command -v nx
```

Verification:

```bash
test -d "$LEGACY_DIR"
test ! -e "$LEGACY_ARCHIVE"
```

Rollback:

```bash
# No filesystem changes yet. Stop if any precondition check fails.
```

2. Quarantine legacy in-tree copy (non-destructive move).

```bash
mv "$LEGACY_DIR" "$LEGACY_ARCHIVE"
```

Verification:

```bash
test ! -e "$LEGACY_DIR"
test -d "$LEGACY_ARCHIVE"
```

Rollback:

```bash
mv "$LEGACY_ARCHIVE" "$LEGACY_DIR"
```

3. Verify path/hook resolution after legacy-copy removal.

```bash
hash -r
command -v nx
type -a nx
rg -n 'scripts/nx-rs|\.nix-config/scripts/nx-rs' \
  ~/.zshrc ~/.zprofile ~/.bashrc ~/.bash_profile ~/.config/fish/config.fish 2>/dev/null || true
```

Verification:
- `command -v nx` resolves to intended canonical command (typically `~/.nix-config/scripts/nx/nx` until full replacement).
- Shell config search shows no required `scripts/nx-rs` entries.

Rollback:

```bash
mv "$LEGACY_ARCHIVE" "$LEGACY_DIR"
hash -r
```

4. Re-run cutover gates after legacy-copy removal.

```bash
cd /Users/morgan/code/nx-rs
just cutover-validate
git -C ~/.nix-config status --porcelain=v1 --untracked-files=all > /tmp/nxrs-decom-git-after.txt
diff -u /tmp/nxrs-decom-git-before.txt /tmp/nxrs-decom-git-after.txt
```

Verification:
- `just cutover-validate` exits 0.
- git status diff is empty.

Rollback:

```bash
mv "$LEGACY_ARCHIVE" "$LEGACY_DIR"
hash -r
```

5. Finalize only after soak window.

```bash
# Optional: keep archive for rollback until confidence window passes.
rm -rf "$LEGACY_ARCHIVE"
```

Verification:

```bash
test ! -e "$LEGACY_ARCHIVE"
```

Rollback:

```bash
# After permanent deletion, restore from external backup or git history.
```
