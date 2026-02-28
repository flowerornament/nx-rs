# Learnings Log

Purpose:
- Capture only high-confidence operational learnings that future agents must know to work safely and efficiently in this repo.
- Keep this file factual and compact.
- Do not track plans, status, or TODOs here (use `bd` for execution tracking).

Update rules for future agents:
- Add an entry only when behavior has been observed and verified in this repo.
- Prefer concrete commands/paths over abstract advice.
- If a learning becomes invalid, update or remove it promptly.

## Confirmed Learnings

1. This standalone repo uses local `bd` state.
- Canonical tracker is `./.beads` in this repo.
- Active migration continuation epic is `nx-rs-0a1`.
- Historical migration in `~/.nix-config/scripts/nx-rs` was tracked under `morgan-pnv`.

2. `bd doctor` behavior depends on current directory.
- Running `bd doctor` must be done from this repo root for authoritative status.
- `bd doctor` in unrelated parent directories checks different rigs and gives unrelated warnings.

3. Parity baselines support target-specific capture.
- Baselines live in `tests/fixtures/parity/baselines/`.
- Python capture command: `just parity-capture`.
- Rust capture is supported with `NX_PARITY_CAPTURE=1 NX_PARITY_TARGET=rust`.

4. Parity harness is dual-target with case gating.
- Python verification: `just parity-check-python` (or `just parity-check`).
- Rust verification: `just parity-check-rust`.
- Python target runs fixture cases with `python_parity=true` (default when omitted).
- Rust target runs fixture cases with `rust_parity=true`.

5. Rust CLI contract includes default install preprocessing.
- Unknown first non-flag token is rewritten to `install` before clap parsing.
- Implemented in `src/cli.rs` (`preprocess_args`), matching Python CLI behavior.

6. Output normalization in parity harness is path-stable.
- Harness normalizes ANSI and absolute paths to stable tokens (for example `<REPO_ROOT>`).
- This is required for reproducible snapshots across temp directories.

7. Cutover validation is scripted and now passes full shadow/canary gates.
- Run manual shadow/canary validation with `just cutover-validate` (script: `scripts/cutover/validate_shadow_canary.sh`).
- Verified on 2026-02-12 against `~/.nix-config`: shadow matrix, canary matrix, and mutation safety all passed.
- The `sops-nix` parity gap was fixed by including `default.nix` files in Rust nix scan collection (matching Python finder behavior).

8. Legacy in-tree `nx-rs` decommission audit is clean outside legacy directory.
- Repo audit command: `rg -n --hidden --glob '!.git' '~/.nix-config/scripts/nx-rs|\.nix-config/scripts/nx-rs|scripts/nx-rs'`.
- `~/.nix-config` audit command: `rg -n --hidden --glob '!.git' --glob '!scripts/nx-rs/**' 'scripts/nx-rs|\.nix-config/scripts/nx-rs'`.
- Verified on 2026-02-12: no matches outside `~/.nix-config/scripts/nx-rs`.

9. Cutover validation still passes after quarantining legacy in-tree copy.
- Legacy directory was moved to `/tmp/nx-rs-legacy-20260212-032055`.
- Re-verified on 2026-02-12 with `just cutover-validate`: shadow matrix, canary matrix, and mutation safety all passed.

10. Parity harness coverage includes Rust-only search/install deterministic flows, stubbed upgrade brew path, expanded Rust info JSON parity, missing-arg parser failures, and interactive undo-confirm flow.
- `tests/fixtures/parity/cases.json` currently has 68 cases.
- 60 cases run in Python parity target; 68 cases run in Rust parity target.
- Eight cases are Rust-only (`python_parity=false`): `search_found_stubbed`, `search_not_found_stubbed`, `search_json_found_stubbed`, `search_bleeding_edge_stubbed`, `implicit_install_unqualified_token_stubbed`, `install_yes_mutates_stubbed`, `install_yes_rebuild_stubbed`, and `install_flake_input_claude_yes_stubbed`.
- `info_json_found` and `info_json_sources_not_installed` are now enabled for Rust parity with Python-shaped source metadata output.
- `upgrade_brew_stubbed_no_updates` verifies brew-phase parity with deterministic `brew outdated --json` stubs.
- Missing-arg coverage now includes `install`, `remove`, `where`, `info`, and `installed`, each returning parser-style exit code `2` with Python-matching stderr.
- Undo coverage now includes `undo_dirty_confirmed` (stdin `y`) in parity fixtures and `undo_dirty_confirmed_reverts` in system command matrix with deterministic git call assertions.
- Verified on 2026-02-28 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

11. SPEC reconciliation found three drift items fixed in v1.0.
- `info` exit code: clarified returns 0 on not-found (matching `where` behavior).
- `installed` JSON format: corrected to show query strings as top-level keys, not nested.
- Config scan vs finder scan: clarified `default.nix` excluded from purpose routing but included in package discovery via finder's independent glob.
- Section 15 (pre-Rust toolchain setup) removed as completed planning artifact.

12. Local cargo-installed nx can auto-refresh on system commands.
- When `nx` resolves to `~/.local/share/cargo/bin/nx`, `rebuild` and `upgrade` now preflight-check whether local `nx-rs` sources are newer than the binary.
- If stale, nx runs `cargo install --path <nx-rs-root> --force`, prints a re-run hint, and exits without executing the system command payload.
- Auto-refresh is opt-out via `NX_RS_AUTO_REFRESH=0` (also accepts `false`/`no`).
- Verified on 2026-02-19 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

13. CLI default-install preprocessing should not special-case typo-like subcommands.
- Rust CLI now mirrors Python `run_cli` behavior: first non-flag token that is not a known command is always treated as a package name by injecting `install`.
- The prior typo-suggestion rejection path (for near-miss command names like `upgade`) was removed to avoid semantic drift from SPEC/Python.
- Parity harness prepends global flags (`--plain --minimal`) before case args, so this preprocessing path is best locked by `src/cli.rs` unit tests unless the harness adds per-case flag control.
- Verified on 2026-02-20 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

14. Rebuild command shape must stay direct under sudo to preserve sudoers `NOPASSWD` compatibility.
- SPEC/Python contract for rebuild is `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake <repo_root> [passthrough...]`.
- Wrapping rebuild as `sudo bash -lc ...` can bypass host rules scoped to `/run/current-system/sw/bin/darwin-rebuild` and reintroduce password prompts.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

15. Upgrade failure-path parity now explicitly covers flake-phase short-circuit behavior.
- Added `upgrade_flake_failure` parity fixture (`upgrade --no-ai` with `stub_update_fail`) to lock Python/Rust alignment on exit code `1` and no mutation when `nix flake update` fails.
- Added `upgrade_flake_failure_short_circuit` in `tests/system_command_matrix.rs` to assert `upgrade` stops after the failed flake update command and does not continue to downstream phases.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

16. Upgrade flake-change commit path now has Python/Rust parity coverage and aligned output/commit messaging.
- Added parity setup mode `stub_upgrade_flake_changed` plus fixture cases `upgrade_flake_changed_commits_lockfile` and `upgrade_flake_changed_skip_commit`.
- Rust upgrade output for changed flake inputs now matches Python shape in plain/minimal mode, including the GitHub compare failure warning when comparison fetch fails.
- Rust commit step now uses Python-style message generation for flake changes (`Update flake (<inputs...>)`) and success text (`Committed: ...`), with system matrix assertions updated to lock the command arguments and marker output.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

17. Upgrade passthrough-args contract now has explicit parity and invocation coverage.
- Added parity fixture case `upgrade_flake_passthrough_stubbed` to lock Python/Rust behavior for `upgrade -- ...` forwarding to `nix flake update`.
- Added `upgrade_passthrough_flake_update_args` to `tests/system_command_matrix.rs` to assert exact flake-update argv (`flake update --commit-lock-file foo`) under deterministic stubs.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

18. Upgrade command contract coverage now locks GitHub-token injection and cache-corruption retry semantics.
- `tests/system_command_matrix.rs` now stubs `gh` and asserts `upgrade` always probes `gh auth token` before `nix flake update`.
- Added `upgrade_flake_update_injects_access_token_option` to assert token-bearing updates include `--option access-tokens github.com=<token>`.
- Added `upgrade_flake_update_cache_corruption_retries_once` to assert one retry after the known cache-corruption signature (`failed to insert entry: invalid object specified`).
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

19. Upgrade brew-phase contract coverage now locks no-update, update, and dry-run invocation semantics.
- `tests/system_command_matrix.rs` now includes:
  - `upgrade_brew_no_updates_short_circuit` to assert brew phase runs `brew outdated --json` and exits without `brew upgrade` when nothing is outdated.
  - `upgrade_brew_with_updates_runs_upgrade` to assert brew phase calls `brew outdated --json`, metadata fetch (`brew info --json=v2`), then `brew upgrade <pkg>`.
  - `upgrade_brew_with_updates_dry_run_skips_upgrade` to assert dry-run still inspects outdated/metadata but does not execute `brew upgrade`.
- Deterministic stubs now cover `brew outdated --json`, `brew info --json=v2`, and `brew upgrade` in the system matrix harness.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

20. Upgrade rebuild-phase contract coverage now locks run and failure semantics when rebuild is not skipped.
- `tests/system_command_matrix.rs` now includes:
  - `upgrade_runs_rebuild_when_not_skipped` to assert `upgrade` runs rebuild preflight (`git ls-files`), flake check (`nix flake check`), and `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake <repo_root>` when `--skip-rebuild` is not set.
  - `upgrade_rebuild_failure_exits_nonzero` to assert a rebuild failure in upgrade flow returns exit code `1`.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

21. Upgrade commit-phase contract now explicitly gates commits on lockfile diffs.
- Added `upgrade_no_flake_changes_skips_commit` to `tests/system_command_matrix.rs` to assert `upgrade --skip-brew --skip-rebuild --no-ai` runs flake update but does not call `git add/commit` when lock inputs are unchanged.
- Added parity fixture case `upgrade_flake_unchanged_no_skip_commit` with baseline output `All flake inputs up to date`, locking Python/Rust alignment for no-change/no-`--skip-commit` behavior.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

22. Source-search timeout/failure contract is now deterministic and non-blocking.
- `src/infra/sources.rs` parallel search now uses detached worker threads plus `recv_timeout`, so timeout returns partial results immediately instead of waiting for slow workers.
- Added deterministic unit coverage for SPEC §6.3 semantics:
  - timeout returns partial results and emits warning when warnings are enabled
  - quiet path suppresses timeout warning
  - individual source failure preserves other source results and emits warning
  - quiet path suppresses source-failure warning
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

23. SPEC clause-to-evidence traceability now has a machine-readable baseline.
- Added `.agents/spec_traceability_matrix_v1.tsv` mapping SPEC v1 sections 2-14 to Rust implementation paths and explicit unit/system/parity evidence references.
- Matrix rows classify each clause as `covered`, `partial`, `missing`, or `covered_by_subclauses`, so follow-up work can target uncovered contracts directly.
- Tracker alignment: closed `nx-rs-29h.1` and created `nx-rs-29h.5` for uncovered SPEC §4/§12 gaps identified by the matrix.
- Verified on 2026-02-28 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

24. SPEC §2/§7 contract closure now includes root `--json` propagation and non-dry-run install parity fixtures.
- Root-level `--json` is now carried via `AppContext` global flags and honored by JSON-capable handlers (`list`, `info`, `installed`, `search`) in addition to per-command `--json`.
- General-nix routing candidates now enforce SPEC §7.4 invariants by constraining to fallback-manifest siblings and excluding `packages/nix/languages.nix`.
- Added Rust parity fixtures: `install_yes_mutates_stubbed`, `install_yes_rebuild_stubbed`, and `install_flake_input_claude_yes_stubbed`; plus system-matrix cases for root `--json` handler propagation.
- Verified on 2026-02-27 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

25. SPEC §11 cache-corruption retry contract now has explicit deletion evidence.
- `tests/system_command_matrix.rs` now seeds `$HOME/.cache/nix/fetcher-cache-v4.sqlite` for `upgrade_flake_update_cache_corruption_retries_once` and asserts the file is deleted after the retry path runs.
- This closes the remaining SPEC §11 traceability gap for `stream_nix_update` cache-corruption handling in `.agents/spec_traceability_matrix_v1.tsv`.
- Verified on 2026-02-28 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

26. Flake PATH cutover on this host requires explicit cargo-path de-prioritization and root rebuild invocation.
- During cutover (`nx-rs-s52.3`), `~/.nix-config` already had `nx-rs` flake input/package wiring, but inherited shell PATH still resolved `nx` to `~/.local/share/cargo/bin/nx` before Nix profile paths.
- Reliable fix in `~/.nix-config/home/shell.nix`: remove `$HOME/.local/share/cargo/bin` from prepended `home.sessionPath` and append it in `programs.zsh.profileExtra` (`export PATH="$PATH:$HOME/.local/share/cargo/bin"`), preserving cargo tools while letting flake-managed `nx` win resolution.
- `darwin-rebuild switch --flake .` now fails without root on this host; use `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .` for cutover and rollback rebuild steps.
- Verification should use a clean login shell environment to avoid inherited PATH contamination from long-lived sessions.
- Verified on 2026-02-28 with evidence bundle `.agents/reports/flake-cutover/20260228T075202Z/` against `~/.nix-config`.

27. Post-cutover rollback rehearsal is two-way reversible on this host with explicit snapshot restore.
- In `nx-rs-s52.4`, applying pre-cutover backups from `.agents/reports/flake-cutover/20260228T075202Z/` changed only `~/.nix-config/home/shell.nix` relative to the current post-cutover state; `flake.nix` and `packages/nix/cli.nix` remained identical.
- After rollback apply + `sudo /run/current-system/sw/bin/darwin-rebuild switch --flake .`, clean login env resolution flipped to cargo-first (`~/.local/share/cargo/bin/nx` before `/etc/profiles/per-user/morgan/bin/nx`) and smoke checks still passed against `B2NIX_REPO_ROOT=~/.nix-config`.
- Restoring pre-drill snapshots + rebuild returned resolution to flake-first (`/etc/profiles/per-user/morgan/bin/nx` before cargo path), with an empty before-vs-final `git status --porcelain` diff in `~/.nix-config`.
- Verified on 2026-02-28 with evidence bundle `.agents/reports/flake-rollback-drill/20260228T081016Z/`.

28. Non-SPEC command surface is now explicitly constrained and documented as intentional additive extensions.
- Locked CLI parser coverage in `src/cli.rs` for `search`, `uninstall`, and `secret`/`secrets` passthrough behavior plus a command-set boundary test (`known_commands_match_spec_plus_intentional_extensions`).
- Documented extension policy in SPEC §14 and updated `.agents/spec_traceability_matrix_v1.tsv` §2.1 notes to treat these as deliberate compatibility extensions rather than unresolved drift.
- Verified on 2026-02-28 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

29. Post-cutover policy treats Python parity as a completed one-time acceptance goal.
- Recurring weekly/monthly maintenance cadence was removed from `.agents/CUTOVER_PLAYBOOK.md`; standard ongoing gate is `just ci`.
- `just parity-check-rust`, `just parity-check`, and `just cutover-validate` remain available for ad hoc forensic/recovery validation only.
- Historical maintenance-gate artifacts remain preserved under `.agents/reports/maintenance-gates/`.
- Verified on 2026-02-28 with `just ci`, `cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.
