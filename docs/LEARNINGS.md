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
