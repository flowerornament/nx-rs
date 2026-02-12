#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

if [[ ! -f "Cargo.toml" ]]; then
  echo "[pre-compile] Cargo.toml not found."
  echo "[pre-compile] Run toolchain/bootstrap steps before invoking compile hooks."
  exit 2
fi

STRICT_FLAGS=(--workspace --all-targets --all-features)

echo "[pre-compile] cargo fmt --all --check"
cargo fmt --all --check

echo "[pre-compile] cargo clippy ${STRICT_FLAGS[*]} -- -D warnings"
cargo clippy "${STRICT_FLAGS[@]}" -- -D warnings

echo "[pre-compile] cargo test ${STRICT_FLAGS[*]}"
cargo test "${STRICT_FLAGS[@]}"

