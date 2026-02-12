#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

if [[ ! -f "Cargo.toml" ]]; then
  echo "[compile] Cargo.toml not found."
  echo "[compile] Scaffold the Rust workspace before compile."
  exit 2
fi

scripts/agent-hooks/pre-compile.sh

STRICT_FLAGS=(--workspace --all-targets --all-features)
echo "[compile] cargo check ${STRICT_FLAGS[*]}"
cargo check "${STRICT_FLAGS[@]}"

scripts/agent-hooks/post-compile.sh

