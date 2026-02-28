# Flake PATH Cutover Evidence Summary

- Task: `nx-rs-s52.3`
- Executed: 2026-02-28 UTC
- Target config: `~/.nix-config`
- Evidence directory: `.agents/reports/flake-cutover/20260228T075202Z/`

## Rollback Checkpoint (Pre-Mutation)

Captured before any cutover mutation:

- `00-nix-config-git-status-short-branch.txt`
- `01-nix-config-git-status-porcelain.txt`
- `02-nix-config-head.txt`
- `03-shell-path.txt`
- `04-nx-resolution-before.txt`
- Backup snapshots:
  - `backup-flake.nix`
  - `backup-packages-nix-cli.nix`
  - `backup-home-shell.nix`

Notable pre-state:

- `~/.nix-config` already contained flake input/package wiring for `nx-rs`.
- `nx` resolved to cargo first in inherited shell PATH (`~/.local/share/cargo/bin/nx`).

## Mutation Applied

- `~/.nix-config/home/shell.nix`
  - Removed `$HOME/.local/share/cargo/bin` from `home.sessionPath` (prepended segment).
  - Appended cargo bin in `programs.zsh.profileExtra` (`export PATH="$PATH:$HOME/.local/share/cargo/bin"`).
  - Goal: keep cargo tools available while letting flake/Nix `nx` win command resolution.

Diff evidence:

- `32-nix-config-diff-cutover-files.patch`

## Rebuild + Verification

Commands run:

1. `nix flake lock --update-input nx-rs` (`10-nix-flake-lock-update-input-nx-rs.log`)
2. `darwin-rebuild switch --flake .` failed (root required) (`11-darwin-rebuild-switch-flake.log`)
3. `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .` succeeded (`12-sudo-darwin-rebuild-switch-flake.log`)

Post-cutover checks:

- In clean login env, `nx` resolves to `/etc/profiles/per-user/morgan/bin/nx` first (`33-clean-env-nx-resolution-after.txt`).
- Smoke tests from `~/.nix-config` passed:
  - `nx --plain --minimal status` (`34-clean-env-nx-status-after.txt`)
  - `nx where ripgrep` (`35-clean-env-nx-where-ripgrep-after.txt`)
  - `nx list --plain | head -5` snapshot (`36-clean-env-nx-list-plain-head-after.txt`)
  - `nx installed ripgrep --json` (`37-clean-env-nx-installed-ripgrep-json-after.txt`)

## Rollback Ready

Current rollback inputs are captured in backups + patch:

- Restore file(s) from `backup-*.nix` and re-run `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .`.
- Validation and before/after git-status transcripts are retained in this bundle.
