#!/usr/bin/env bash
set -euo pipefail

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'error: missing required command: %s\n' "$1" >&2
        exit 1
    }
}

require_cmd nix
require_cmd python3
require_cmd git

json_quote() {
    python3 - <<'PY' "$1"
import json
import sys

print(json.dumps(sys.argv[1]))
PY
}

ROOT="$(git rev-parse --show-toplevel)"
TMPDIR="$(python3 - <<'PY'
import os
import tempfile

print(os.path.realpath(tempfile.mkdtemp()))
PY
)"
trap 'rm -rf "$TMPDIR"' EXIT

SOURCE_ROOT="$TMPDIR/source"
mkdir -p "$SOURCE_ROOT"
(
  cd "$ROOT"
  while IFS= read -r -d '' path; do
      if [ -e "$path" ]; then
          printf '%s\0' "$path"
      fi
  done < <(git ls-files -z)
) | tar --null -T - -cf - | tar -xf - -C "$SOURCE_ROOT"

configured_json="$TMPDIR/configured.json"
bare_json="$TMPDIR/bare.json"
eval_module="$TMPDIR/eval-home-manager-module.nix"
root_json="$(json_quote "$SOURCE_ROOT")"

cat > "$eval_module" <<'EOF'
{ root, mode }:
let
  flake = builtins.getFlake "path:${root}";
  pkgs = import flake.inputs.nixpkgs { system = builtins.currentSystem; };
  lib = flake.inputs.nixpkgs.lib;
  module = flake.outputs.homeManagerModules.default;
  stub = { lib, ... }: {
    options.assertions = lib.mkOption {
      type = lib.types.listOf lib.types.attrs;
      default = [ ];
    };
    options.home.packages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
    };
    options.home.sessionVariables = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
    };
  };
  caseModule =
    if mode == "configured" then
      {
        programs.nx.enable = true;
        programs.nx.repoRoot = "/tmp/nix-config";
        programs.nx.autoRefresh = false;
        programs.nx.sops.package = pkgs.sops;
        programs.nx.sops.bin = "/run/current-system/sw/bin/sops";
      }
    else if mode == "bare" then
      {
        programs.nx.enable = true;
      }
    else if mode == "invalid-sops-bin" then
      {
        programs.nx.enable = true;
        programs.nx.sops.bin = "";
      }
    else
      {
        programs.nx.enable = true;
        programs.nx.repoRoot = "";
      };
  evaluated = lib.evalModules {
    modules = [ module stub caseModule ];
    specialArgs = { inherit pkgs; };
  };
in
{
  packageCount = builtins.length evaluated.config.home.packages;
  hasRepoRoot = evaluated.config.home.sessionVariables ? NX_REPO_ROOT;
  repoRoot = evaluated.config.home.sessionVariables.NX_REPO_ROOT or null;
  hasAutoRefresh = evaluated.config.home.sessionVariables ? NX_RS_AUTO_REFRESH;
  autoRefresh = evaluated.config.home.sessionVariables.NX_RS_AUTO_REFRESH or null;
  hasSopsBin = evaluated.config.home.sessionVariables ? NX_RS_SOPS_BIN;
  sopsBin = evaluated.config.home.sessionVariables.NX_RS_SOPS_BIN or null;
}
EOF

eval_module_json="$(json_quote "$eval_module")"
nix eval --impure --json --expr "import ${eval_module_json} { root = ${root_json}; mode = \"configured\"; }" > "$configured_json"
nix eval --impure --json --expr "import ${eval_module_json} { root = ${root_json}; mode = \"bare\"; }" > "$bare_json"

if nix eval --impure --json --expr "import ${eval_module_json} { root = ${root_json}; mode = \"invalid\"; }" >/dev/null 2>&1; then
    printf 'invalid repoRoot case unexpectedly succeeded\n' >&2
    exit 1
fi

if nix eval --impure --json --expr "import ${eval_module_json} { root = ${root_json}; mode = \"invalid-sops-bin\"; }" >/dev/null 2>&1; then
    printf 'invalid sops.bin case unexpectedly succeeded\n' >&2
    exit 1
fi

python3 - <<'PY' "$configured_json" "$bare_json"
import json
import pathlib
import sys

configured = json.loads(pathlib.Path(sys.argv[1]).read_text())
bare = json.loads(pathlib.Path(sys.argv[2]).read_text())

if configured["packageCount"] < 1:
    raise SystemExit("configured case did not add nx to home.packages")

if configured["packageCount"] < 2:
    raise SystemExit("configured case did not add the optional sops package")

if bare["packageCount"] < 1:
    raise SystemExit("bare case did not add nx to home.packages")

if not configured["hasRepoRoot"] or configured["repoRoot"] != "/tmp/nix-config":
    raise SystemExit("configured case did not export NX_REPO_ROOT correctly")

if not configured["hasAutoRefresh"] or configured["autoRefresh"] != "0":
    raise SystemExit("configured case did not export NX_RS_AUTO_REFRESH=0")

if not configured["hasSopsBin"] or configured["sopsBin"] != "/run/current-system/sw/bin/sops":
    raise SystemExit("configured case did not export NX_RS_SOPS_BIN correctly")

if bare["hasRepoRoot"]:
    raise SystemExit("bare case unexpectedly exported NX_REPO_ROOT")

if bare["hasAutoRefresh"]:
    raise SystemExit("bare case unexpectedly exported NX_RS_AUTO_REFRESH")

if bare["hasSopsBin"]:
    raise SystemExit("bare case unexpectedly exported NX_RS_SOPS_BIN")

print("configured_package_count=true")
print("configured_repo_root=true")
print("configured_auto_refresh_disabled=true")
print("configured_sops_package=true")
print("configured_sops_bin=true")
print("bare_package_count=true")
print("bare_session_variables_absent=true")
PY

printf 'Home Manager module smoke test passed.\n'
