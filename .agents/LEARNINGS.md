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

10. Parity harness coverage includes Rust-only search, stubbed upgrade brew path, and expanded Rust info JSON parity.
- `tests/fixtures/parity/cases.json` currently has 50 cases.
- 46 cases run in Python parity target; 48 cases run in Rust parity target.
- Four `search_*` cases are Rust-only (`python_parity=false`) with stubbed baselines.
- `info_json_found` and `info_json_sources_not_installed` are now enabled for Rust parity with Python-shaped source metadata output.
- `upgrade_brew_stubbed_no_updates` verifies brew-phase parity with deterministic `brew outdated --json` stubs.
- Verified on 2026-02-19 with `just ci`, `just parity-check-rust`, and `PY_NX="$HOME/code/nx-python/nx" just cutover-validate`.

11. SPEC reconciliation found three drift items fixed in v1.0.
- `info` exit code: clarified returns 0 on not-found (matching `where` behavior).
- `installed` JSON format: corrected to show query strings as top-level keys, not nested.
- Config scan vs finder scan: clarified `default.nix` excluded from purpose routing but included in package discovery via finder's independent glob.
- Section 15 (pre-Rust toolchain setup) removed as completed planning artifact.
