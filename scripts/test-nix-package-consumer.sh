#!/usr/bin/env bash
set -euo pipefail

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'error: missing required command: %s\n' "$1" >&2
        exit 1
    }
}

require_cmd git
require_cmd nix
require_cmd python3

json_quote() {
    python3 - <<'PY' "$1"
import json
import sys

print(json.dumps(sys.argv[1]))
PY
}

ROOT="$(git rev-parse --show-toplevel)"
SYSTEM="$(nix eval --raw --impure --expr 'builtins.currentSystem')"
CONSUMER_NIXPKGS="${NX_RELEASE_CONSUMER_NIXPKGS:-github:nixos/nixpkgs/nixos-unstable}"
TMPDIR="$(python3 - <<'PY'
import os
import tempfile

print(os.path.realpath(tempfile.mkdtemp()))
PY
)"
trap 'rm -rf "$TMPDIR"' EXIT

SOURCE_ROOT="$TMPDIR/source"
CONSUMER_ROOT="$TMPDIR/consumer"
mkdir -p "$SOURCE_ROOT" "$CONSUMER_ROOT"

(
  cd "$ROOT"
  while IFS= read -r -d '' path; do
      if [ -e "$path" ]; then
          printf '%s\0' "$path"
      fi
  done < <(git ls-files -z)
) | tar --null -T - -cf - | tar -xf - -C "$SOURCE_ROOT"

consumer_nixpkgs_json="$(json_quote "$CONSUMER_NIXPKGS")"

cat > "$CONSUMER_ROOT/flake.nix" <<EOF
{
  description = "nx-rs release consumer packaging smoke test";

  inputs = {
    nixpkgs.url = ${consumer_nixpkgs_json};
    nx-rs = {
      url = "path:${SOURCE_ROOT}";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nx-rs, ... }: {
    packages.${SYSTEM}.default = nx-rs.packages.${SYSTEM}.default;
  };
}
EOF

nix build --no-link "$CONSUMER_ROOT#packages.${SYSTEM}.default"
printf 'Nix package consumer smoke test passed with %s.\n' "$CONSUMER_NIXPKGS"
