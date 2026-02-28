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
- Rust candidate: `target/debug/nx` (auto-built if missing)

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

Executed: **2026-02-27 23:45:38 PST** (2026-02-28 UTC)

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
- Archived pre-cutover gate bundle transcripts/reports: `.agents/reports/cutover-gates/20260228T074322Z/`

Historical note:
- Legacy in-tree copy was previously moved from `~/.nix-config/scripts/nx-rs` to `/tmp/nx-rs-legacy-20260212-032055`.

## Go/No-Go Criteria

GO for full PATH replacement only if all are true:

1. Shadow matrix: all cases pass (exit + stdout + stderr)
2. Canary matrix: all cases pass
3. Mutation safety: unchanged git status
4. `just compile` passes in `nx-rs`

NO-GO if any gate fails.

## Current Parity Evidence (2026-02-27 PST / 2026-02-28 UTC)

Validated parity harness totals:
- Python target (`just parity-check`): **60/60 parity-enabled cases passing**.
- Rust target (`just parity-check-rust`): **68/68 parity-enabled cases passing**.
- Fixture inventory (`tests/fixtures/parity/cases.json`): **68 total cases** (8 Rust-only cases).

Coverage spans all command families: query (`where`, `list`, `info`, `status`, `installed`), mutation (`install`, `remove`), system (`update`, `test`, `rebuild`, `upgrade`), and undo flows with setup variants.

SPEC v1.0 remains reconciled to Python source audit, with clause-level closure tracked in `.agents/spec_traceability_matrix_v1.tsv`.

## Current Decision

As of **2026-02-27 PST** (2026-02-28 UTC): **GO** for full production PATH replacement.

Evidence trail:
- Shadow/canary cutover validation passed (2026-02-27 23:45:38 PST), archived at `.agents/reports/cutover-gates/20260228T074322Z/`.
- Dual-target parity harness verified (`60/60` Python target; `68/68` Rust target) on 2026-02-27 PST / 2026-02-28 UTC.
- SPEC reconciled to v1.0 against Python source audit (2026-02-16).
- `just ci` green (fmt + clippy + test + check) on 2026-02-27 PST / 2026-02-28 UTC (transcript archived in the same gate bundle directory).
- Legacy in-tree copy decommissioned and quarantined.

## Flake Cutover Procedure

The production cutover uses nix flakes. nx-rs exposes a `flake.nix` that builds the `nx` binary.

### Prerequisites

1. `nix build` succeeds in `~/code/nx-rs`.
2. `just ci` passes.
3. `just cutover-validate` passes (shadow + canary + mutation safety).

### Step 1: Extract Python nx to standalone repo

```bash
# Clean copy (no git history needed — ~/code/nx-python/ has the frozen copy)
mkdir -p ~/code/nx-python
cp -R ~/.nix-config/scripts/nx/* ~/code/nx-python/
cd ~/code/nx-python
git init && git add . && git commit -m "Initial commit: extract from nix-config"
# Create repo on GitHub, then:
# git remote add origin git@github.com:flowerornament/nx-python.git
# git push -u origin main
```

### Step 2: Add nx-rs as flake input in nix-config

In `~/.nix-config/flake.nix`, add to `inputs`:

```nix
nx-rs = {
  url = "github:flowerornament/nx-rs";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

### Step 3: Add nx package to system packages

Where system packages are declared (e.g. in a host or home module), add:

```nix
inputs.nx-rs.packages.${pkgs.system}.default
```

### Step 4: Remove Python nx from nix-config

```bash
rm -rf ~/.nix-config/scripts/nx
```

Remove any PATH entries or shell aliases that pointed to `scripts/nx/nx`.

### Step 5: Rebuild and verify

```bash
cd ~/.nix-config
nix flake lock --update-input nx-rs
darwin-rebuild switch --flake .
hash -r
command -v nx    # should resolve to /run/current-system/sw/bin/nx or similar
nx --plain --minimal status
```

### Step 6: Smoke test

```bash
nx where ripgrep
nx list --plain | head -5
nx installed ripgrep --json
```

## Rollback (Flake-Based)

If the Rust `nx` has issues after cutover:

1. Remove the `nx-rs` input from `~/.nix-config/flake.nix` and its package reference.

2. Restore Python `nx`:

```bash
# If nx-python is on GitHub:
git clone git@github.com:flowerornament/nx-python.git ~/.nix-config/scripts/nx
# Or from local copy:
cp -R ~/code/nx-python ~/.nix-config/scripts/nx
```

3. Rebuild:

```bash
cd ~/.nix-config && darwin-rebuild switch --flake .
hash -r
command -v nx
```

4. Smoke check:

```bash
nx --plain --minimal status
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
