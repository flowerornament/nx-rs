#!/usr/bin/env bash
set -euo pipefail

if [[ ! -t 0 || ! -t 1 || ! -t 2 ]]; then
  printf 'x demo-output requires terminal stdin, stdout, and stderr\n' >&2
  exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

repo="$scratch/repo"
home="$scratch/home"
stubs="$scratch/stubs"
log="$scratch/invocations.tsv"
profile="$scratch/system-profile"

printf '\n> Preparing output demo\n'
cargo build --quiet --manifest-path "$root/Cargo.toml" --bin nx
target_dir="$(
  cargo metadata --quiet --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
)"
nx_bin="$target_dir/debug/nx"
mkdir -p "$repo" "$home" "$stubs" "$repo/scripts/nx/tests"
cp -R "$root/tests/fixtures/system/repo_base/." "$repo/"
NX_OUTPUT_DEMO_STUB_DIR="$stubs" cargo test \
  --quiet \
  --manifest-path "$root/Cargo.toml" \
  --test system_commands \
  install_output_demo_stubs \
  -- --exact --ignored >/dev/null
ln -s /nix/store/current-system "$profile"
: >"$log"
printf '+ Output demo ready\n'

run_nx() {
  local mode="$1"
  shift
  (
    cd "$repo"
    env \
      HOME="$home" \
      NX_OUTPUT_DEMO=1 \
      NX_REPO_ROOT="$repo" \
      NX_SPLIT_DARWIN=1 \
      NX_SYSTEM_IT_DARWIN_REBUILD="$stubs/darwin-rebuild" \
      NX_SYSTEM_IT_LOG="$log" \
      NX_SYSTEM_IT_MODE="$mode" \
      NX_SYSTEM_PROFILE_PATH="$profile" \
      PATH="$stubs:$PATH" \
      PYTHONDONTWRITEBYTECODE=1 \
      "$nx_bin" "$@"
  )
}

printf '\n  $ nx list'
run_nx success list

printf '\n  $ nx rebuild --preflight'
run_nx success rebuild --preflight

printf '\n  $ nx rebuild --preflight  # expected failure'
if run_nx flake_check_fail rebuild --preflight; then
  printf 'x Expected rebuild preflight to fail\n' >&2
  exit 1
fi

printf '\n  $ nx upgrade'
run_nx success upgrade --no-ai

printf '\n+ Output demo complete\n'
