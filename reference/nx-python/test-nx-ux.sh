#!/usr/bin/env bash
# Quick UX demo of nx commands
# Run: ./scripts/nx/test-nx-ux.sh

set -e
cd "$(dirname "$0")/../.."

echo "========================================"
echo "  nx UX Demo"
echo "========================================"
echo ""

echo "--- status ---"
./scripts/nx/nx status
echo ""

echo "--- list (first 20 lines) ---"
python - <<'PY'
import subprocess

result = subprocess.run(["./scripts/nx/nx", "list"], capture_output=True, text=True)
lines = result.stdout.splitlines()
for line in lines[:20]:
    print(line)
print("  ...")
PY
echo ""

echo "--- where ripgrep ---"
./scripts/nx/nx where ripgrep
echo ""

echo "--- where nonexistent ---"
./scripts/nx/nx where nonexistent123 || true
echo ""

echo "--- installed (silent, exit codes only) ---"
echo -n "  ripgrep: "
./scripts/nx/nx installed ripgrep && echo "exit 0" || echo "exit 1"
echo -n "  nonexistent: "
./scripts/nx/nx installed nonexistent123 && echo "exit 0" || echo "exit 1"
echo ""

echo "--- info ripgrep (first 20 lines) ---"
./scripts/nx/nx info ripgrep | head -20
echo "  ..."
echo ""

echo "--- ripgrep --dry-run (already installed) ---"
./scripts/nx/nx ripgrep --dry-run
echo ""

echo "--- rm ripgrep --dry-run ---"
./scripts/nx/nx rm ripgrep --dry-run
echo ""

echo "========================================"
echo "  Done!"
echo "========================================"
