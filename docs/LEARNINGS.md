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

1. `bd` usage in this project is top-level, not local to `scripts/nx-rs`.
- Canonical tracker is `~/.nix-config/.beads`.
- Do not run `bd init` inside `scripts/nx-rs` (it creates a shadow tracker and splits issue state).
- Migration epic is `morgan-pnv` in the top-level tracker.

2. `bd doctor` behavior depends on current directory.
- Running `bd doctor` inside `scripts/nx-rs` reports missing local `.beads` unless one exists.
- For authoritative project health, run from repo root: `cd ~/.nix-config && bd doctor`.

3. Parity baselines are Python-reference snapshots.
- Baselines live in `tests/fixtures/parity/baselines/`.
- Capture command: `just parity-capture`.
- Capture mode is Python-only (`NX_PARITY_CAPTURE=1` with default target behavior).

4. Parity harness is dual-target with case gating.
- Python verification: `just parity-check-python` (or `just parity-check`).
- Rust verification: `just parity-check-rust`.
- Rust parity runs only fixture cases with `"rust_parity": true` in `tests/fixtures/parity/cases.json`.

5. Rust CLI contract includes default install preprocessing.
- Unknown first non-flag token is rewritten to `install` before clap parsing.
- Implemented in `src/cli.rs` (`preprocess_args`), matching Python CLI behavior.

6. Output normalization in parity harness is path-stable.
- Harness normalizes ANSI and absolute paths to stable tokens (for example `<REPO_ROOT>`).
- This is required for reproducible snapshots across temp directories.
