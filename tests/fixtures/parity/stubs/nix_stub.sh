#!/bin/sh
mode="${NX_PARITY_MODE:-stub_system_success}"

if [ "$1" = "flake" ] && [ "$2" = "update" ]; then
  if [ "$mode" = "stub_update_fail" ]; then
    echo "stub nix flake update failed"
    exit 1
  fi
  if [ "$mode" = "stub_upgrade_flake_changed" ]; then
    cat <<'JSON' > flake.lock
{
  "nodes": {
    "root": {
      "inputs": {
        "nixpkgs": "nixpkgs"
      }
    },
    "nixpkgs": {
      "locked": {
        "lastModified": 1700000001,
        "owner": "NixOS",
        "repo": "nixpkgs",
        "rev": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "type": "github"
      }
    }
  }
}
JSON
  fi
  echo "stub nix flake update ok"
  exit 0
fi

if [ "$1" = "flake" ] && [ "$2" = "check" ]; then
  if [ "$mode" = "stub_rebuild_flake_check_fail" ]; then
    echo "stub nix flake check failed" >&2
    exit 1
  fi
  echo "stub nix flake check ok"
  exit 0
fi

if [ "$mode" = "stub_info_sources" ] || [ "$mode" = "stub_info_sources_bleeding_edge" ]; then
  if [ "$1" = "search" ] && [ "$2" = "--json" ]; then
    query="$4"
    if [ "$query" = "python3Packages.requests" ] || [ "$query" = "requests" ]; then
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.python3Packages.requests":{"pname":"requests","version":"1.0.0","description":"Stub Python requests package"}}
JSON
      exit 0
    fi
    if [ "$query" = "ripgrep" ]; then
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.ripgrep":{"pname":"ripgrep","version":"14.1.1","description":"Stub ripgrep package"}}
JSON
      exit 0
    fi
    echo "{}"
    exit 0
  fi

  if [ "$1" = "eval" ] && [ "$2" = "--json" ]; then
    attr="$3"
    case "$attr" in
      *python3Packages.requests.name)
        echo "\"python3Packages.requests\""
        ;;
      *python3Packages.requests.version)
        echo "\"1.0.0\""
        ;;
      *python3Packages.requests.meta)
        cat <<'JSON'
{"description":"Stub Python requests package","homepage":"https://example.test/requests","license":{"spdxId":"MIT"},"broken":false,"insecure":false}
JSON
        ;;
      *ripgrep.name)
        echo "\"ripgrep\""
        ;;
      *ripgrep.version)
        echo "\"14.1.1\""
        ;;
      *ripgrep.meta)
        cat <<'JSON'
{"description":"Stub ripgrep package","homepage":"https://example.test/ripgrep","license":{"spdxId":"MIT"},"broken":false,"insecure":false}
JSON
        ;;
      *)
        echo "null"
        ;;
    esac
    exit 0
  fi
fi

if [ "$mode" = "stub_install_sources" ] || [ "$mode" = "stub_install_sources_cache_hit" ]; then
  if [ "$1" = "search" ] && [ "$2" = "--json" ]; then
    target="$3"
    query="$4"
    if [ "$query" = "nxrsmulti" ]; then
      if [ "$target" = "github:nix-community/NUR" ]; then
        cat <<'JSON'
{"legacyPackages.aarch64-darwin.nxrsmultinur":{"pname":"nxrsmultinur","version":"1.0.0","description":"Stub NUR multi-source candidate"}}
JSON
        exit 0
      fi
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.nxrsmultinxs":{"pname":"nxrsmultinxs","version":"1.0.0","description":"Stub nixpkgs multi-source candidate"}}
JSON
      exit 0
    fi
    if [ "$query" = "nxrsdryrun" ]; then
      cat <<'JSON'
{"legacyPackages.aarch64-darwin.nxrsdryrun":{"pname":"nxrsdryrun","version":"1.0.0","description":"Stub dry-run install candidate"}}
JSON
      exit 0
    fi
    if [ "$query" = "nxrsplatform" ]; then
      cat <<'JSON'
{
  "legacyPackages.aarch64-darwin.nxrsplatformincompatible": {
    "pname": "nxrsplatformincompatible",
    "version": "1.0.0",
    "description": "Stub platform-incompatible candidate"
  },
  "legacyPackages.aarch64-darwin.nxrsplatformfallback": {
    "pname": "nxrsplatformfallback",
    "version": "1.0.0",
    "description": "Stub platform-compatible fallback"
  }
}
JSON
      exit 0
    fi
    echo "{}"
    exit 0
  fi

  if [ "$1" = "eval" ] && [ "$2" = "--json" ]; then
    attr="$3"
    case "$attr" in
      *nxrsplatformincompatible.meta.platforms)
        echo '["x86_64-windows","aarch64-windows"]'
        ;;
      *.meta.platforms)
        echo '["aarch64-darwin","x86_64-darwin","x86_64-linux","aarch64-linux"]'
        ;;
      *)
        echo "null"
        ;;
    esac
    exit 0
  fi
fi

echo "stub nix unsupported: $*" >&2
exit 0

