#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
NIX_CONFIG_ROOT="${NIX_CONFIG_ROOT:-$HOME/.nix-config}"
DEFAULT_PY_NX="$HOME/code/nx-python/nx"
LEGACY_PY_NX="$NIX_CONFIG_ROOT/scripts/nx/nx"
PY_NX="${PY_NX:-}"

if [[ -z "$PY_NX" ]]; then
  if [[ -x "$DEFAULT_PY_NX" ]]; then
    PY_NX="$DEFAULT_PY_NX"
  else
    PY_NX="$LEGACY_PY_NX"
  fi
fi

RUST_NX="${RUST_NX:-$WORKSPACE_ROOT/target/debug/nx}"
REPORT_PATH="${1:-}"

if [[ ! -x "$PY_NX" ]]; then
  echo "python nx entrypoint not found or not executable: $PY_NX" >&2
  exit 2
fi

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

declare -a SHADOW_ROWS=()
declare -a CANARY_ROWS=()
declare -a SHADOW_FAIL_DETAILS=()
SHADOW_ALL_PASS=yes
CANARY_ALL_PASS=yes

GIT_BEFORE="$(git -C "$NIX_CONFIG_ROOT" status --porcelain=v1 --untracked-files=all)"

normalize_file() {
  local input="$1"
  local output="$2"

  perl -CSDA -pe '
    s/\r\n/\n/g;
    s/\Q$ENV{NIX_CONFIG_ROOT_NORM}\E/<REPO_ROOT>/g if defined $ENV{NIX_CONFIG_ROOT_NORM};
    s/\Q$ENV{HOME_NORM}\E/<HOME>/g if defined $ENV{HOME_NORM};
    s/[ \t]+$//;
  ' "$input" >"$output"
}

cmd_display() {
  local rendered
  printf -v rendered '%q ' "$@"
  printf '%s' "${rendered% }"
}

record_shadow_case() {
  local id="$1"
  shift

  local case_dir="$TMP_DIR/shadow-$id"
  mkdir -p "$case_dir"

  set +e
  env B2NIX_REPO_ROOT="$NIX_CONFIG_ROOT" NO_COLOR=1 TERM=dumb \
    "$PY_NX" --plain --minimal "$@" >"$case_dir/py.stdout" 2>"$case_dir/py.stderr"
  local py_ec=$?
  env B2NIX_REPO_ROOT="$NIX_CONFIG_ROOT" NO_COLOR=1 TERM=dumb \
    "$RUST_NX" --plain --minimal "$@" >"$case_dir/rs.stdout" 2>"$case_dir/rs.stderr"
  local rs_ec=$?
  set -e

  normalize_file "$case_dir/py.stdout" "$case_dir/py.stdout.norm"
  normalize_file "$case_dir/py.stderr" "$case_dir/py.stderr.norm"
  normalize_file "$case_dir/rs.stdout" "$case_dir/rs.stdout.norm"
  normalize_file "$case_dir/rs.stderr" "$case_dir/rs.stderr.norm"

  local exit_match=no
  local stdout_match=no
  local stderr_match=no
  local pass=no

  if [[ "$py_ec" -eq "$rs_ec" ]]; then
    exit_match=yes
  fi
  if cmp -s "$case_dir/py.stdout.norm" "$case_dir/rs.stdout.norm"; then
    stdout_match=yes
  fi
  if cmp -s "$case_dir/py.stderr.norm" "$case_dir/rs.stderr.norm"; then
    stderr_match=yes
  fi

  if [[ "$exit_match" == yes && "$stdout_match" == yes && "$stderr_match" == yes ]]; then
    pass=yes
  else
    SHADOW_ALL_PASS=no

    if [[ "$stdout_match" == no ]]; then
      local stdout_diff
      stdout_diff="$(
        diff -u "$case_dir/py.stdout.norm" "$case_dir/rs.stdout.norm" \
          | sed -n '1,120p' \
          || true
      )"
      SHADOW_FAIL_DETAILS+=(
        $'### '"$id"$' stdout mismatch\n\n```diff\n'"$stdout_diff"$'\n```'
      )
    fi

    if [[ "$stderr_match" == no ]]; then
      local stderr_diff
      stderr_diff="$(
        diff -u "$case_dir/py.stderr.norm" "$case_dir/rs.stderr.norm" \
          | sed -n '1,120p' \
          || true
      )"
      SHADOW_FAIL_DETAILS+=(
        $'### '"$id"$' stderr mismatch\n\n```diff\n'"$stderr_diff"$'\n```'
      )
    fi
  fi

  SHADOW_ROWS+=("| $id | $(cmd_display "$@") | $py_ec | $rs_ec | $stdout_match | $stderr_match | $pass |")
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
    nx --plain --minimal "$@" >"$case_dir/canary.stdout" 2>"$case_dir/canary.stderr"
  local ec=$?
  set -e

  local pass=no
  if [[ "$ec" -eq 0 ]]; then
    pass=yes
  else
    CANARY_ALL_PASS=no
  fi

  CANARY_ROWS+=("| $id | nx --plain --minimal $(cmd_display "$@") | $ec | $pass |")
}

PACKAGE_LIST="$(
  env B2NIX_REPO_ROOT="$NIX_CONFIG_ROOT" NO_COLOR=1 TERM=dumb \
    "$PY_NX" --plain --minimal list --plain \
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

record_shadow_case "where_found" where "$PACKAGE_SAMPLE"
record_shadow_case "where_not_found" where "$MISSING_PACKAGE"
record_shadow_case "list_plain" list --plain
record_shadow_case "status" status
record_shadow_case "installed_json" installed "$PACKAGE_SAMPLE" --json
record_shadow_case "info_json_not_found" info "$MISSING_PACKAGE" --json
record_shadow_case "install_dry_run" install --dry-run "$PACKAGE_SAMPLE"
record_shadow_case "remove_dry_run" remove --dry-run "$PACKAGE_SAMPLE"

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

SHADOW_ROWS_TEXT="$(printf '%s\n' "${SHADOW_ROWS[@]}")"
CANARY_ROWS_TEXT="$(printf '%s\n' "${CANARY_ROWS[@]}")"
SHADOW_FAIL_DETAILS_TEXT="$(printf '%s\n\n' "${SHADOW_FAIL_DETAILS[@]}")"

OVERALL_DECISION=no
if [[ "$SHADOW_ALL_PASS" == yes && "$CANARY_ALL_PASS" == yes && "$MUTATION_SAFE" == yes ]]; then
  OVERALL_DECISION=yes
fi

REPORT="$(cat <<EOF_REPORT
# nx-rs Cutover Validation Report

- Executed: $RUN_AT
- Workspace: $WORKSPACE_ROOT
- nix-config root: $NIX_CONFIG_ROOT
- Python nx: $PY_NX
- Rust nx-rs: $RUST_NX
- Sample installed package used in checks: $PACKAGE_SAMPLE

## Shadow Matrix (Python vs Rust)

| Case | Command Args | Python Exit | Rust Exit | Stdout Match | Stderr Match | Pass |
| --- | --- | --- | --- | --- | --- | --- |
$SHADOW_ROWS_TEXT

Shadow matrix all pass: $SHADOW_ALL_PASS

## Canary Matrix (PATH-preferred nx-rs)

| Case | Command | Exit | Pass |
| --- | --- | --- | --- |
$CANARY_ROWS_TEXT

Canary matrix all pass: $CANARY_ALL_PASS

## Mutation Safety

Git status unchanged after all checks: $MUTATION_SAFE

## Overall Gate

All gates pass (shadow + canary + mutation safety): $OVERALL_DECISION
EOF_REPORT
)"

if [[ "${#SHADOW_FAIL_DETAILS[@]}" -gt 0 ]]; then
  REPORT+=$'\n\n## Shadow Failure Details\n\n'
  REPORT+="$SHADOW_FAIL_DETAILS_TEXT"
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
