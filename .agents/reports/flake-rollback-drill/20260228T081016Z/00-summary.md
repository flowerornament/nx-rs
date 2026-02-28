# Post-Cutover Smoke + Rollback Drill Evidence Summary

- Task: `nx-rs-s52.4`
- Executed: 2026-02-28 UTC
- Target config: `~/.nix-config`
- Evidence directory: `.agents/reports/flake-rollback-drill/20260228T081016Z`

## Pre-Drill Checkpoint

Captured before rollback rehearsal:

- `00-nix-config-git-status-short-branch-before.txt`
- `01-nix-config-git-status-porcelain-before.txt`
- `02-nix-config-head-before.txt`
- `03-clean-env-path-and-resolution-before.txt`
- Pre-drill restore snapshots:
  - `pre-drill-flake.nix`
  - `pre-drill-packages-nix-cli.nix`
  - `pre-drill-home-shell.nix`

## Post-Cutover Smoke Matrix

Clean-env smoke checks against `B2NIX_REPO_ROOT=~/.nix-config` passed:

- `10-clean-env-nx-resolution-post-cutover.txt`
- `11-clean-env-nx-status-post-cutover.txt`
- `12-clean-env-nx-where-ripgrep-post-cutover.txt`
- `13-clean-env-nx-list-plain-head-post-cutover.txt`
- `14-clean-env-nx-installed-ripgrep-json-post-cutover.txt`

Parity shadow/canary gate also passed with explicit Python entrypoint override:

- Report: `15-cutover-validate-report-post-cutover.md`
- Transcript: `16-cutover-validate-stdout-post-cutover.log`

## Rollback Rehearsal (Applied + Verified)

Rollback source snapshots from cutover task `nx-rs-s52.3`:

- `.agents/reports/flake-cutover/20260228T075202Z/backup-flake.nix`
- `.agents/reports/flake-cutover/20260228T075202Z/backup-packages-nix-cli.nix`
- `.agents/reports/flake-cutover/20260228T075202Z/backup-home-shell.nix`

Observed mutation after rollback apply:

- Git status showed only `home/shell.nix` modified (`21-nix-config-git-status-porcelain-after-rollback-apply.txt`).
- Patch captured at `22-nix-config-diff-after-rollback-apply.patch`.

Rollback rebuild and verification:

- Rebuild: `23-sudo-darwin-rebuild-switch-flake-rollback.log`
- Clean-env PATH placed cargo bin before Nix profile and resolved `nx` to cargo (`24-clean-env-path-and-resolution-after-rollback.txt`).
- Smoke checks passed (`25`-`27`).

## Restore Rehearsal (Return To Cutover State)

Restored pre-drill snapshots and rebuilt:

- Apply-state status/diff: `30`-`32`
- Rebuild log: `33-sudo-darwin-rebuild-switch-flake-restore.log`

Post-restore verification:

- Clean-env PATH restored to post-cutover order and resolved `nx` to `/etc/profiles/per-user/morgan/bin/nx` first (`34-clean-env-path-and-resolution-after-restore.txt`).
- Smoke checks passed (`35`-`37`).

## Reversibility Proof

- Final git status in `~/.nix-config` returned to clean (`40`/`41`).
- Before-vs-final porcelain diff is empty (`42-nix-config-git-status-before-vs-final.diff`).
- Drill demonstrates two-way operational reversibility: post-cutover -> rollback PATH state -> post-cutover state.
