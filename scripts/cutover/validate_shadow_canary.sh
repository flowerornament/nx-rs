#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
NIX_CONFIG_ROOT="${NIX_CONFIG_ROOT:-$HOME/.nix-config}"
RUST_NX="${RUST_NX:-$WORKSPACE_ROOT/target/debug/nx}"
REPORT_PATH="${1:-}"

if [[ ! -x "$RUST_NX" ]]; then
  (cd "$WORKSPACE_ROOT" && cargo build --quiet --bin nx)
fi

if [[ ! -x "$RUST_NX" ]]; then
  echo "rust nx binary not found after build: $RUST_NX" >&2
  exit 2
fi

if ! git -C "$NIX_CONFIG_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "nix-config root is not a git worktree: $NIX_CONFIG_ROOT" >&2
  exit 2
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

RUN_AT="$(date '+%Y-%m-%d %H:%M:%S %Z')"
export NIX_CONFIG_ROOT_NORM="$NIX_CONFIG_ROOT"
export HOME_NORM="$HOME"

declare -a DIRECT_ROWS=()
declare -a CANARY_ROWS=()
declare -a FAILURE_DETAILS=()
DIRECT_ALL_PASS=yes
CANARY_ALL_PASS=yes

GIT_BEFORE="$(git -C "$NIX_CONFIG_ROOT" status --porcelain=v1 --untracked-files=all)"

cmd_display() {
  local rendered
  printf -v rendered '%q ' "$@"
  printf '%s' "${rendered% }"
}

record_direct_case() {
  local id="$1"
  shift

  local case_dir="$TMP_DIR/direct-$id"
  mkdir -p "$case_dir"

  set +e
  env B2NIX_REPO_ROOT="$NIX_CONFIG_ROOT" NO_COLOR=1 TERM=dumb \
    "$RUST_NX" --plain --minimal "$@" >"$case_dir/stdout" 2>"$case_dir/stderr"
  local ec=$?
  set -e

  local pass=no
  if [[ "$ec" -eq 0 ]]; then
    pass=yes
  else
    DIRECT_ALL_PASS=no
    local stdout_preview
    stdout_preview="$(sed -n '1,120p' "$case_dir/stdout" || true)"
    local stderr_preview
    stderr_preview="$(sed -n '1,120p' "$case_dir/stderr" || true)"
    FAILURE_DETAILS+=(
      $'### '\"$id\"$' failed\n\n```text\n'$'stdout:\n'\"$stdout_preview\"$'\n\nstderr:\n'\"$stderr_preview\"$'\n```'
    )
  fi

  DIRECT_ROWS+=("| $id | $(cmd_display "$@") | $ec | $pass |")
}

CANARY_BIN="$TMP_DIR/canary-bin"
mkdir -p "$CANARY_BIN"
ln -sf "$RUST_NX" "$CANARY_BIN/nx"

record_canary_case() {
  local id="$1"
  shift

  local case_dir="$TMP_DIR/canary-$id"
  mkdir -p "$case_dir"

  set +e
  env PATH="$CANARY_BIN:$PATH" B2NIX_REPO_ROOT="$NIX_CONFIG_ROOT" NO_COLOR=1 TERM=dumb \
    nx --plain --minimal "$@" >"$case_dir/stdout" 2>"$case_dir/stderr"
  local ec=$?
  set -e

  local pass=no
  if [[ "$ec" -eq 0 ]]; then
    pass=yes
  else
    CANARY_ALL_PASS=no
    local stdout_preview
    stdout_preview="$(sed -n '1,120p' "$case_dir/stdout" || true)"
    local stderr_preview
    stderr_preview="$(sed -n '1,120p' "$case_dir/stderr" || true)"
    FAILURE_DETAILS+=(
      $'### '\"$id\"$' canary failed\n\n```text\n'$'stdout:\n'\"$stdout_preview\"$'\n\nstderr:\n'\"$stderr_preview\"$'\n```'
    )
  fi

  CANARY_ROWS+=("| $id | nx --plain --minimal $(cmd_display "$@") | $ec | $pass |")
}

PACKAGE_LIST="$(
  env B2NIX_REPO_ROOT="$NIX_CONFIG_ROOT" NO_COLOR=1 TERM=dumb \
    "$RUST_NX" --plain --minimal list --plain \
    | awk 'NF {gsub(/^ +/, "", $0); print}'
)"

PACKAGE_SAMPLE=""
for preferred in ripgrep fd ast-grep; do
  if printf '%s\n' "$PACKAGE_LIST" | grep -Fxq "$preferred"; then
    PACKAGE_SAMPLE="$preferred"
    break
  fi
done

if [[ -z "$PACKAGE_SAMPLE" ]]; then
  PACKAGE_SAMPLE="$(printf '%s\n' "$PACKAGE_LIST" | awk 'NF {print; exit}')"
fi
if [[ -z "$PACKAGE_SAMPLE" ]]; then
  PACKAGE_SAMPLE="ripgrep"
fi

MISSING_PACKAGE="not-a-real-package-nxrs-cutover"

record_direct_case "where_found" where "$PACKAGE_SAMPLE"
record_direct_case "where_not_found" where "$MISSING_PACKAGE"
record_direct_case "list_plain" list --plain
record_direct_case "status" status
record_direct_case "installed_json" installed "$PACKAGE_SAMPLE" --json
record_direct_case "info_json_not_found" info "$MISSING_PACKAGE" --json
record_direct_case "install_dry_run" install --dry-run "$PACKAGE_SAMPLE"
record_direct_case "remove_dry_run" remove --dry-run "$PACKAGE_SAMPLE"

record_canary_case "status" status
record_canary_case "where_found" where "$PACKAGE_SAMPLE"
record_canary_case "installed_json" installed "$PACKAGE_SAMPLE" --json

GIT_AFTER="$(git -C "$NIX_CONFIG_ROOT" status --porcelain=v1 --untracked-files=all)"
MUTATION_SAFE=yes
MUTATION_DIFF=""
if [[ "$GIT_BEFORE" != "$GIT_AFTER" ]]; then
  MUTATION_SAFE=no
  printf '%s\n' "$GIT_BEFORE" >"$TMP_DIR/git-before.txt"
  printf '%s\n' "$GIT_AFTER" >"$TMP_DIR/git-after.txt"
  MUTATION_DIFF="$(diff -u "$TMP_DIR/git-before.txt" "$TMP_DIR/git-after.txt" || true)"
fi

DIRECT_ROWS_TEXT="$(printf '%s\n' "${DIRECT_ROWS[@]}")"
CANARY_ROWS_TEXT="$(printf '%s\n' "${CANARY_ROWS[@]}")"
FAILURE_DETAILS_TEXT="$(printf '%s\n\n' "${FAILURE_DETAILS[@]}")"

OVERALL_DECISION=no
if [[ "$DIRECT_ALL_PASS" == yes && "$CANARY_ALL_PASS" == yes && "$MUTATION_SAFE" == yes ]]; then
  OVERALL_DECISION=yes
fi

REPORT="$(cat <<EOF_REPORT
# nx-rs Cutover Validation Report

- Executed: $RUN_AT
- Workspace: $WORKSPACE_ROOT
- nix-config root: $NIX_CONFIG_ROOT
- Rust nx-rs: $RUST_NX
- Sample installed package used in checks: $PACKAGE_SAMPLE

## Direct Matrix (Rust Binary)

| Case | Command Args | Exit | Pass |
| --- | --- | --- | --- |
$DIRECT_ROWS_TEXT

Direct matrix all pass: $DIRECT_ALL_PASS

## Canary Matrix (PATH-preferred nx-rs)

| Case | Command | Exit | Pass |
| --- | --- | --- | --- |
$CANARY_ROWS_TEXT

Canary matrix all pass: $CANARY_ALL_PASS

## Mutation Safety

Git status unchanged after all checks: $MUTATION_SAFE

## Overall Gate

All gates pass (direct + canary + mutation safety): $OVERALL_DECISION
EOF_REPORT
)"

if [[ "${#FAILURE_DETAILS[@]}" -gt 0 ]]; then
  REPORT+=$'\n\n## Failure Details\n\n'
  REPORT+="$FAILURE_DETAILS_TEXT"
fi

if [[ "$MUTATION_SAFE" == no ]]; then
  REPORT+=$'\n\n### Git Status Diff\n\n```diff\n'
  REPORT+="$MUTATION_DIFF"
  REPORT+=$'\n```\n'
fi

if [[ -n "$REPORT_PATH" ]]; then
  mkdir -p "$(dirname "$REPORT_PATH")"
  printf '%s\n' "$REPORT" >"$REPORT_PATH"
fi

printf '%s\n' "$REPORT"

if [[ "$OVERALL_DECISION" == yes ]]; then
  exit 0
fi

exit 1
