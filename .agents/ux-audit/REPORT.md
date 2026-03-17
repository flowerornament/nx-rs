# nx UX Audit Report

Date: 2026-03-01
Binary: local release build (includes `installed` single-package fix + cwd auto-detect)
Test repo: `~/.nix-config` (108 packages across nxs/brew/cask/services)

Historical snapshot only. Some findings below were fixed after this capture; treat
`.agents/SPEC.md`, the current CLI help, and the test suite as authoritative.
Known resolved since capture:
- `list invalid-source` now exits `1`
- `undo` now supports `-y, --yes`

## Test Matrix

43 raw output captures in this directory. Every command tested with normal, edge, and error cases.

| Command | Variants Tested | Status |
|---------|----------------|--------|
| list | default, --json, --plain, --verbose, source filter, invalid source, subdir | Pass |
| status | default | Pass |
| info | installed pkg, not installed, --json, no args | Pass |
| where | found, not found, no args | Pass |
| installed | single, single --show-location, not found, multi, --json, no args | Pass (fix verified) |
| search | results, --json, no results, no args | Pass |
| install | --dry-run, --dry-run --yes, already installed, no args, real install | Pass |
| remove | --dry-run, --dry-run --yes, not found, no args, real remove | Pass |
| undo | after remove (nothing), after install (reverts) | Pass |
| upgrade | --dry-run | Pass |
| init | existing manifest, --refresh | Pass |
| secret | no subcommand, add no args | Pass |
| help | --help, install --help, no args | Pass |
| subdir | list from packages/nix/, status from home/ | Pass |

## Spacing & Alignment Issues

**S1. info title format doesn't match Pattern 16** (nx-rs-nwh)
- Shows `ripgrep (installed (nxs))` — nested parens, no bold
- Design system: `**ripgrep**  installed (nxs)` — bold name, space, badge

**S2. Inconsistent blank line placement** (nx-rs-fun)
- `search-default.txt` L2-L3: double blank between searching and results
- `upgrade-dry-run.txt`: no blank before `+` result lines after headers
- `remove-not-found.txt`: no blank between dry-run header and error
- `info-not-installed.txt`: no blank before suggestion block

**S3. Prompt and result merge on same line** (nx-rs-4yh)
- `Revert all changes? [y/N]: + Reverted 1 files` — should be separate lines
- Same in init: `Write .nx/manifest.toml? [Y/n]:   Cancelled.`

**S4. search uses `>` action glyph for static results title** (nx-rs-2en)
- `> Results for 'firefox'` — search is done, not in progress
- Should use title pattern at column 2

**S5. installed: single vs multi structural mismatch** (nx-rs-spa)
- Multi has leading blank + title header; single has neither

## Issues Found

### High Priority

**H1. Dry-run preview shows wrong context for install**
- `install --dry-run cowsay` shows end-of-file context (lines 105-107)
- Real `install cowsay` correctly shows insertion at line 12
- Dry-run doesn't compute actual insertion point
- Files: `install-dry-run.txt` vs `install-remove-undo-cycle.txt`

**H2. Exit code 0 for some error paths**
- `remove --dry-run nonexistent-xyz` exits 0 (should be 1)
- `info nonexistent-pkg` exits 0 (debatable — SPEC says 0)
- `list invalid-source` exits 0 (SPEC says 1 for invalid filter — bug)

**H3. `list --verbose` has no visible effect**
- Output identical to `list` default
- Design system expects `list -v` to show locations per package
- File: `list-verbose.txt` vs `list-default.txt`

### Medium Priority

**M1. `list` lacks title line**
- `status` has `Package Status (108 packages installed)` — good
- `list` starts directly with package names — no header, no count
- Inconsistent with design system and `status`

**M2. Filtered list gives no context**
- `list nix` outputs 82 names with no indication it's filtered
- Should show header like `nxs (82 packages)` or similar

**M3. Remove success uses `+` sigil**
- `+ cowsay removed from cli.nix` — the `+` implies addition
- Dry-run correctly uses `- Would remove`
- Real remove should use `-` or neutral sigil

**M4. `undo` lacks `--yes` flag**
- `install` and `remove` both support `--yes`
- `undo` requires interactive confirmation — asymmetric for scripting

**M5. Stray `* cli.nix` line in remove output**
- Appears between diff panel and success message
- Looks like intermediate git status leaking into output
- File: `install-remove-undo-cycle.txt` line 39

### Low Priority

**L1. `--plain` identical to default**
- No visible difference in `list` output
- Flag is effectively a no-op for `list`

**L2. `info` no-args and `where` no-args show clap error**
- Exit code 2 (correct)
- Message is functional but uses clap default format, not nx design system
- Minor: could use `x Missing argument` with suggestion

**L3. `secret add` no-args shows clap help**
- Same as L2 — clap default, not nx-styled error

## What Works Well

- **Code panels**: Consistent `┌──/└──` box drawing with line numbers and `+`/`-` markers
- **Glyph system**: `+` success, `x` error, `>` action, `~` dry-run, `!` warning — used consistently
- **Progressive disclosure**: `info` shows appropriate detail levels (default vs `--json`)
- **Dry-run banner**: `~ Dry Run (no changes will be made)` is clear and prominent
- **Search output**: Clean result format with version, description, confidence
- **Status command**: Best-formatted command — title, table, counts, examples
- **Auto-detect**: Works from all subdirectories (walk-up to flake.nix)
- **Install/remove/undo cycle**: Full round-trip works correctly, repo left clean
- **Error recovery suggestions**: `Check installed: nx list | grep -i ...` — helpful

## Color Note

Current "entire line is one color" behavior is intentional and accepted.
The Python version had more granular color application but the current approach is cleaner.
