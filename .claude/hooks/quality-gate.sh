#!/bin/bash
set -euo pipefail

INPUT=$(cat)
CWD=$(echo "$INPUT" | jq -r '.cwd')
STOP_HOOK_ACTIVE=$(echo "$INPUT" | jq -r '.stop_hook_active // false')

if [ "$STOP_HOOK_ACTIVE" = "true" ]; then
  exit 0
fi

cd "$CWD"

if just check > /tmp/quality-gate.log 2>&1; then
  exit 0
else
  ERRORS=$(tail -30 /tmp/quality-gate.log)
  jq -n --arg errors "$ERRORS" '{"decision":"block","reason":"Quality gate failed. Fix issues:\n\n\($errors)"}'
  exit 0
fi
