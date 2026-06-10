#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

mode="${1:-default}"

case "${mode}" in
  default|record)
    ;;
  *)
    echo "usage: $0 [default|record]" >&2
    exit 2
    ;;
esac

ms() {
  python3 -c 'import time; print(int(time.time() * 1000))'
}

fmt_elapsed() {
  local millis="$1"
  printf "%d.%02ds" "$((millis / 1000))" "$(((millis % 1000) / 10))"
}

log_dir="$(mktemp -d "${TMPDIR:-/tmp}/nx-quality-gate.XXXXXX")"
preserve_logs=0

cleanup() {
  if [[ "${preserve_logs}" -eq 0 ]]; then
    rm -rf "${log_dir}"
  fi
}
trap cleanup EXIT

run_step() {
  local label="$1"
  shift
  local log_file="${log_dir}/${label}.log"
  local start end elapsed

  start="$(ms)"
  if "$@" >"${log_file}" 2>&1; then
    end="$(ms)"
    elapsed=$((end - start))
    printf "  %-13s %s\n" "${label}:" "$(fmt_elapsed "${elapsed}")" >&2
    printf "%s %s\n" "${label}" "${elapsed}" >>"${log_dir}/times"
    return 0
  fi

  preserve_logs=1
  echo "  ${label}: failed" >&2
  echo "---- ${label} log (last 200 lines) ----" >&2
  tail -n 200 "${log_file}" >&2 || true
  echo "---- full log: ${log_file} ----" >&2
  return 1
}

step_millis() {
  local label="$1"
  awk -v label="${label}" '$1 == label { print $2 }' "${log_dir}/times"
}

append_ledger() {
  local ledger="${NX_GATE_LEDGER:-}"

  if [[ "${mode}" = "record" && -z "${ledger}" ]]; then
    ledger=".nx/gate-times.csv"
  fi
  if [[ -z "${ledger}" ]]; then
    return 0
  fi

  mkdir -p "$(dirname "${ledger}")"
  if [[ ! -s "${ledger}" ]]; then
    printf 'timestamp,change,fmt_ms,clippy_ms,test_ms,scripts_ms,check_ms,total_ms\n' >>"${ledger}"
  fi

  local timestamp change
  timestamp="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  change="$(jj log -r @ --no-graph -T 'change_id.short()' 2>/dev/null || git rev-parse --short HEAD 2>/dev/null || printf 'unknown')"

  printf '%s,%s,%s,%s,%s,%s,%s,%s\n' \
    "${timestamp}" \
    "${change}" \
    "$(step_millis fmt)" \
    "$(step_millis clippy)" \
    "$(step_millis test)" \
    "$(step_millis scripts)" \
    "$(step_millis check)" \
    "${total}" >>"${ledger}"
  printf "  %-13s %s\n" "ledger:" "${ledger}" >&2
}

echo "--- quality gate ---" >&2
run_step fmt just fmt-check
run_step clippy just lint
run_step test just test
run_step scripts just test-scripts
run_step check just check
echo "--------------------" >&2

total=0
while read -r _ millis; do
  total=$((total + millis))
done <"${log_dir}/times"
printf "  %-13s %s\n" "total:" "$(fmt_elapsed "${total}")" >&2
append_ledger
