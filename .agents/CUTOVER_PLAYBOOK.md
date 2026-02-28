# nx-rs Cutover Playbook

Purpose: define and execute a deploy-safe validation workflow for production `nx-rs` behavior on `~/.nix-config`.

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

1. Build and resolve binary
- Rust candidate: `target/debug/nx` (auto-built if missing)

2. Direct matrix (Rust binary execution)
- Execute and require exit `0` for:
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

Executed: **2026-02-28 08:04:08 PST** (2026-02-28 UTC)

Direct matrix:

| Case | Command Args | Exit | Pass |
| --- | --- | --- | --- |
| where_found | `where ripgrep` | 0 | yes |
| where_not_found | `where not-a-real-package-nxrs-cutover` | 0 | yes |
| list_plain | `list --plain` | 0 | yes |
| status | `status` | 0 | yes |
| installed_json | `installed ripgrep --json` | 0 | yes |
| info_json_not_found | `info not-a-real-package-nxrs-cutover --json` | 0 | yes |
| install_dry_run | `install --dry-run ripgrep` | 0 | yes |
| remove_dry_run | `remove --dry-run ripgrep` | 0 | yes |

Canary matrix:

| Case | Command | Exit | Pass |
| --- | --- | --- | --- |
| status | `nx --plain --minimal status` | 0 | yes |
| where_found | `nx --plain --minimal where ripgrep` | 0 | yes |
| installed_json | `nx --plain --minimal installed ripgrep --json` | 0 | yes |

Mutation safety:

- Git status unchanged: **yes**
- Archived pre-cutover gate bundle transcripts/reports: `.agents/reports/cutover-gates/20260228T074322Z/`

Historical note:
- Legacy in-tree copy was previously moved from `~/.nix-config/scripts/nx-rs` to `/tmp/nx-rs-legacy-20260212-032055`.

## Go/No-Go Criteria

GO for full PATH replacement only if all are true:

1. Direct matrix: all cases pass
2. Canary matrix: all cases pass
3. Mutation safety: unchanged git status
4. `just compile` passes in `nx-rs`

NO-GO if any gate fails.

## Current Decision

As of **2026-02-27 PST** (2026-02-28 UTC): **GO** for full production PATH replacement.

Evidence trail:
- Direct/canary cutover validation passed (2026-02-28 08:04:08 PST).
- SPEC reconciled to v1.0 against Python source audit (2026-02-16).
- `just ci` green (fmt + clippy + test + check) on 2026-02-27 PST / 2026-02-28 UTC (transcript archived in the same gate bundle directory).
- Legacy in-tree copy decommissioned and quarantined.

## Post-Cutover Validation Policy

Parity with Python was a cutover acceptance criterion and is treated as complete.

Operating policy:

1. Normal development and release work uses `just ci` as the standing quality gate.
2. `just test-system` and `just cutover-validate` remain available as ad hoc forensic tools when debugging suspected behavior drift or doing exceptional migration/recovery work.
3. There is no recurring weekly/monthly parity validation schedule.

## Flake Cutover Procedure

The production cutover uses nix flakes. nx-rs exposes a `flake.nix` that builds the `nx` binary.

### Prerequisites

1. `nix build` succeeds in `~/code/nx-rs`.
2. `just ci` passes.
3. `just cutover-validate` passes (direct + canary + mutation safety).

### Step 1: Add nx-rs as flake input in nix-config

In `~/.nix-config/flake.nix`, add to `inputs`:

```nix
nx-rs = {
  url = "github:flowerornament/nx-rs";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

### Step 2: Add nx package to system packages

Where system packages are declared (e.g. in a host or home module), add:

```nix
inputs.nx-rs.packages.${pkgs.system}.default
```

### Step 3: Remove Python nx from nix-config

```bash
rm -rf ~/.nix-config/scripts/nx
```

Remove any PATH entries or shell aliases that pointed to `scripts/nx/nx`.

### Step 4: Rebuild and verify

```bash
cd ~/.nix-config
nix flake lock --update-input nx-rs
sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .
hash -r
command -v nx    # should resolve to /run/current-system/sw/bin/nx or similar
nx --plain --minimal status
```

### Step 5: Smoke test

```bash
nx where ripgrep
nx list --plain | head -5
nx installed ripgrep --json
```

### Latest Flake Cutover Execution Result

Executed: **2026-02-28 UTC**

- Rollback checkpoint captured before mutation (git status snapshots + backups for `flake.nix`, `packages/nix/cli.nix`, and `home/shell.nix`).
- `nix flake lock --update-input nx-rs` completed (with deprecation warning for alias).
- `darwin-rebuild switch --flake .` required root on this host; `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .` succeeded.
- PATH shadowing fix applied in `~/.nix-config/home/shell.nix`:
  - removed `$HOME/.local/share/cargo/bin` from prepended `home.sessionPath`
  - appended cargo path in `programs.zsh.profileExtra`
- Clean login env verification now resolves `nx` to flake-managed path first:
  - `/etc/profiles/per-user/morgan/bin/nx`
  - then `~/.local/share/cargo/bin/nx`
- Smoke checks passed from `~/.nix-config`:
  - `nx --plain --minimal status`
  - `nx where ripgrep`
  - `nx list --plain | head -5`
  - `nx installed ripgrep --json`
- Archived transcripts/reports: `.agents/reports/flake-cutover/20260228T075202Z/`

### Latest Post-Cutover Smoke + Rollback Drill Result

Executed: **2026-02-28 UTC**

- Post-cutover clean-env smoke matrix passed against `B2NIX_REPO_ROOT=~/.nix-config`:
  - `nx --plain --minimal status`
  - `nx --plain --minimal where ripgrep`
  - `nx --plain --minimal list --plain | head -5`
  - `nx --plain --minimal installed ripgrep --json`
- Direct/canary gate re-run passed:
  - `scripts/cutover/validate_shadow_canary.sh`
- Rollback rehearsal applied pre-cutover backups from `.agents/reports/flake-cutover/20260228T075202Z/` and rebuilt successfully with root.
- During rollback apply, only `~/.nix-config/home/shell.nix` changed relative to current post-cutover state.
- Clean-env verification after rollback resolved `nx` to cargo-first path:
  - `/Users/morgan/.local/share/cargo/bin/nx`
  - then `/etc/profiles/per-user/morgan/bin/nx`
- Restore rehearsal reapplied post-cutover snapshots, rebuilt successfully, and clean-env verification returned to flake-first resolution:
  - `/etc/profiles/per-user/morgan/bin/nx`
  - then `/Users/morgan/.local/share/cargo/bin/nx`
- `~/.nix-config` git status returned to the exact pre-drill state (empty before-vs-final porcelain diff).
- Archived transcripts/reports: `.agents/reports/flake-rollback-drill/20260228T081016Z/`

## Rollback (Flake-Based)

If the Rust `nx` has issues after cutover:

1. Pin `nx-rs` input in `~/.nix-config/flake.lock` back to the last known-good revision.

2. Rebuild:

```bash
cd ~/.nix-config && sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .
hash -r
command -v nx
```

3. Smoke check:

```bash
nx --plain --minimal status
```

## Decommission Task: In-Tree `nx-rs` Copy

Target to decommission after standalone repo cutover is canonical:
`~/.nix-config/scripts/nx-rs`.

### Preconditions

1. Standalone repo (`/Users/morgan/code/nx-rs`) is canonical for development and issue tracking.
2. `just compile` passes in standalone repo.
3. `just cutover-validate` passes (direct + canary + mutation safety).
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
