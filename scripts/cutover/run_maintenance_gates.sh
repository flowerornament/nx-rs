#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$WORKSPACE_ROOT"

CADENCE="${1:-weekly}"
case "$CADENCE" in
  weekly|monthly) ;;
  *)
    echo "usage: scripts/cutover/run_maintenance_gates.sh [weekly|monthly]" >&2
    exit 2
    ;;
esac

UTC_TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_AT_UTC="$(date -u '+%Y-%m-%d %H:%M:%S UTC')"
REPORT_DIR="${REPORT_DIR:-$WORKSPACE_ROOT/.agents/reports/maintenance-gates/$UTC_TIMESTAMP}"
SUMMARY_PATH="$REPORT_DIR/00-summary.md"

mkdir -p "$REPORT_DIR"

DEFAULT_PY_NX="$HOME/code/nx-python/nx"
CUTOVER_PY_NX="${PY_NX:-}"
if [[ -z "$CUTOVER_PY_NX" && -x "$DEFAULT_PY_NX" ]]; then
  CUTOVER_PY_NX="$DEFAULT_PY_NX"
fi

declare -a STEP_ROWS=()
FAIL_COUNT=0

record_step() {
  local step_id="$1"
  local command_display="$2"
  shift 2

  local log_path="$REPORT_DIR/$step_id.log"
  local exit_code=0
  local status="pass"

  set +e
  (
    echo "\$ $command_display"
    "$@"
  ) >"$log_path" 2>&1
  exit_code=$?
  set -e

  if [[ "$exit_code" -ne 0 ]]; then
    status="fail"
    FAIL_COUNT=$((FAIL_COUNT + 1))
  fi

  STEP_ROWS+=("| $step_id | \`$command_display\` | $exit_code | $status | \`$step_id.log\` |")
  echo "[maintenance-gates] $step_id: $status (exit=$exit_code)"
}

record_step "01-just-ci" "just ci" just ci
record_step "02-parity-check-rust" "just parity-check-rust" just parity-check-rust

if [[ -n "$CUTOVER_PY_NX" ]]; then
  record_step \
    "03-cutover-validate" \
    "PY_NX=$CUTOVER_PY_NX just cutover-validate" \
    env PY_NX="$CUTOVER_PY_NX" just cutover-validate
else
  record_step "03-cutover-validate" "just cutover-validate" just cutover-validate
fi

if [[ "$CADENCE" == "monthly" ]]; then
  record_step "04-parity-check-python" "just parity-check" just parity-check
fi

STEP_ROWS_TEXT="$(printf '%s\n' "${STEP_ROWS[@]}")"
OVERALL_STATUS="pass"
if [[ "$FAIL_COUNT" -gt 0 ]]; then
  OVERALL_STATUS="fail"
fi

cat >"$SUMMARY_PATH" <<EOF_SUMMARY
# nx-rs Maintenance Gate Report

- Executed (UTC): $RUN_AT_UTC
- Cadence: $CADENCE
- Workspace root: $WORKSPACE_ROOT
- Report directory: $REPORT_DIR
- Failures: $FAIL_COUNT

## Steps

| Step | Command | Exit | Status | Log |
| --- | --- | --- | --- | --- |
$STEP_ROWS_TEXT

## Overall Gate

- Result: $OVERALL_STATUS
EOF_SUMMARY

echo "[maintenance-gates] summary: $SUMMARY_PATH"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
  exit 1
fi
