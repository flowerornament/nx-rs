use std::env;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub const LOG_FILE_NAME: &str = ".system-command-log.tsv";
pub const STUB_DIR_NAME: &str = ".system-stubs";

pub fn prepend_path(stub_dir: &Path) -> String {
    let mut path_value = stub_dir.to_string_lossy().to_string();
    if let Some(existing) = env::var_os("PATH")
        && !existing.is_empty()
    {
        path_value.push(':');
        path_value.push_str(&existing.to_string_lossy());
    }
    path_value
}

pub fn install_stubs(stub_dir: &Path) -> Result<(), Box<dyn Error>> {
    for program in [
        "git",
        "nix",
        "gh",
        "brew",
        "just",
        "sudo",
        "darwin-rebuild",
        "home-manager",
        "readlink",
        "scutil",
        "hostname",
        "df",
    ] {
        write_executable(&stub_dir.join(program), STUB_SCRIPT)?;
    }
    Ok(())
}

fn write_executable(path: &Path, content: &str) -> Result<(), Box<dyn Error>> {
    fs::write(path, content)?;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

const STUB_SCRIPT: &str = r#"#!/bin/sh
set -eu

program="$(basename "$0")"
log_path="${NX_SYSTEM_IT_LOG:?NX_SYSTEM_IT_LOG must be set}"
mode="${NX_SYSTEM_IT_MODE:-success}"

assert_root_nix_env() {
  if [ "${HOME:-}" != "/var/root" ]; then
    echo "stub sudo HOME was not root: ${HOME:-}" >&2
    exit 1
  fi
  if [ "${NIX_REMOTE:-}" != "daemon" ]; then
    echo "stub sudo NIX_REMOTE was not daemon: ${NIX_REMOTE:-}" >&2
    exit 1
  fi
}

line="${program}	${PWD}"
if [ "${NIX_CONFIG:-}" != "" ]; then
  line="${line}	ENV:NIX_CONFIG=${NIX_CONFIG}"
fi
for arg in "$@"; do
  line="${line}	${arg}"
done
printf "%s\n" "$line" >> "$log_path"

case "$program" in
  git)
    if [ "${1:-}" = "-C" ]; then
      shift
      shift
    fi

    if [ "${1:-}" = "ls-files" ]; then
      if [ "$mode" = "git_preflight_fail" ]; then
        echo "stub git ls-files failed" >&2
        exit 1
      fi
      if [ "${2:-}" = "--others" ] && [ "$mode" = "preflight_untracked" ]; then
        echo "home/untracked-from-stub.nix"
        exit 0
      fi
      if [ "${2:-}" != "--others" ]; then
        find home packages system hosts -type f -name '*.nix' 2>/dev/null | sort
        exit 0
      fi
      exit 0
    fi

    if [ "${1:-}" = "rev-parse" ] && [ "${2:-}" = "--show-toplevel" ]; then
      pwd
      exit 0
    fi

    if [ "${1:-}" = "status" ] && { [ "${2:-}" = "--porcelain" ] || [ "${2:-}" = "--porcelain=v1" ]; }; then
      if [ "$mode" = "undo_dirty" ]; then
        echo " M packages/nix/cli.nix"
      fi
      if [ "$mode" = "upgrade_hash_repair_dirty" ]; then
        echo " M home/agent-sync.nix"
      fi
      exit 0
    fi

    if [ "${1:-}" = "diff" ] && [ "${2:-}" = "--stat" ]; then
      if [ "$mode" = "undo_dirty" ]; then
        echo " 1 file changed, 1 insertion(+)"
      fi
      exit 0
    fi

    if [ "${1:-}" = "checkout" ] && [ "${2:-}" = "--" ]; then
      exit 0
    fi

    exit 0
    ;;
  nix)
    if [ "${1:-}" = "flake" ] && { [ "${2:-}" = "update" ] || [ "${2:-}" = "lock" ]; }; then
      if [ "$mode" = "update_fail" ]; then
        echo "stub nix flake command failed"
        exit 1
      fi
      if [ "$mode" = "upgrade_cache_corruption" ]; then
        marker="${HOME}/.nx-system-it-cache-corruption-once"
        if [ ! -f "$marker" ]; then
          : > "$marker"
          echo "error: failed to insert entry: invalid object specified"
          exit 1
        fi
      fi
      if [ "$mode" = "upgrade_flake_changed" ] || [ "$mode" = "upgrade_prefetch_cache_corruption" ] || [ "$mode" = "upgrade_hash_repair" ]; then
        printf '%s' "${NX_SYSTEM_IT_UPGRADE_NEW_LOCK:?NX_SYSTEM_IT_UPGRADE_NEW_LOCK must be set}" > flake.lock
      fi
      echo "stub nix flake command ok"
      exit 0
    fi

    if [ "${1:-}" = "flake" ] && [ "${2:-}" = "prefetch" ]; then
      if [ "$mode" = "upgrade_prefetch_cache_corruption" ]; then
        marker="${HOME}/.nx-system-it-prefetch-cache-corruption-once"
        if [ ! -f "$marker" ]; then
          : > "$marker"
          echo "error: looking up file '«github:NixOS/nixpkgs/bbbbbbb»/README.md': object not found - no match for id (abc123)" >&2
          exit 1
        fi
      fi
      echo '{"storePath":"/nix/store/source","hash":"sha256-test"}'
      exit 0
    fi

    if [ "${1:-}" = "flake" ] && [ "${2:-}" = "check" ]; then
      if [ "$mode" = "flake_check_fail" ]; then
        echo "stub nix flake check failed" >&2
        exit 1
      fi
      echo "stub nix flake check ok"
      exit 0
    fi

    if [ "${1:-}" = "build" ] && [ "${2:-}" = "--json" ]; then
      if [ "$mode" = "split_build_cache_corruption" ]; then
        marker="${HOME}/.nx-system-it-split-build-cache-corruption-once"
        if [ ! -f "$marker" ]; then
          : > "$marker"
          echo "error: looking up file '«github:flowerornament/nx-rs/b9471c7»/scripts/test-home-manager-module.sh': object not found - no match for id (c2217e)" >&2
          exit 1
        fi
      fi
      if [ "$mode" = "split_build_fail" ]; then
        echo "stub nix build failed" >&2
        exit 1
      fi
      if [ "$mode" = "split_build_invalid_json" ]; then
        echo "not-json"
        exit 0
      fi
      output="${NX_SYSTEM_IT_DARWIN_BUILD_OUTPUT:-/nix/store/new-system}"
      printf '[{"outputs":{"out":"%s"}}]\n' "$output"
      exit 0
    fi

    echo "stub nix unsupported: $*" >&2
    exit 1
    ;;
  gh)
    if [ "${1:-}" = "auth" ] && [ "${2:-}" = "token" ]; then
      if [ "$mode" = "upgrade_with_token" ]; then
        echo "ghp_system_matrix_token"
        exit 0
      fi
      exit 1
    fi
    echo "stub gh unsupported: $*" >&2
    exit 1
    ;;
  brew)
    if [ "${1:-}" = "outdated" ] && [ "${2:-}" = "--json" ]; then
      if [ "$mode" = "upgrade_brew_outdated" ]; then
        echo '{"formulae":[{"name":"ripgrep","installed_versions":["14.1.0"],"current_version":"14.1.1"}],"casks":[]}'
        exit 0
      fi
      echo '{"formulae":[],"casks":[]}'
      exit 0
    fi

    if [ "${1:-}" = "info" ] && [ "${2:-}" = "--json=v2" ]; then
      if [ "${3:-}" = "--cask" ]; then
        echo '{"casks":[]}'
        exit 0
      fi
      if [ "$mode" = "upgrade_brew_outdated" ]; then
        echo '{"formulae":[{"name":"ripgrep","homepage":"https://github.com/BurntSushi/ripgrep","desc":"Search tool"}]}'
        exit 0
      fi
      echo '{"formulae":[]}'
      exit 0
    fi

    if [ "${1:-}" = "upgrade" ]; then
      echo "stub brew upgrade ok"
      exit 0
    fi
    echo "stub brew unsupported: $*" >&2
    exit 1
    ;;
  sudo)
    if [ "$mode" = "sudo_fail" ]; then
      echo "stub sudo failed" >&2
      exit 1
    fi

    if [ "${1:-}" = "-H" ]; then
      shift
      export HOME="/var/root"
    fi

    if [ "${1:-}" = "-v" ]; then
      exit 0
    fi

    if [ "${1:-}" = "-n" ] && [ "${2:-}" = "true" ]; then
      if [ "$mode" = "split_sudo_prompt" ]; then
        echo "sudo: a password is required" >&2
        exit 1
      fi
      exit 0
    fi

    # Handle bash -lc wrapper (ulimit + exec darwin-rebuild)
    if [ "${1:-}" = "bash" ] && [ "${2:-}" = "-lc" ]; then
      cmd="${3:-}"
      cmd="$(printf '%s' "$cmd" | sed "s|/run/current-system/sw/bin/darwin-rebuild|${NX_SYSTEM_IT_DARWIN_REBUILD:?}|g")"
      bash -c "$cmd"
      exit $?
    fi

    if [ "${1:-}" = "/run/current-system/sw/bin/darwin-rebuild" ]; then
      shift
      "${NX_SYSTEM_IT_DARWIN_REBUILD:?NX_SYSTEM_IT_DARWIN_REBUILD must be set}" "$@"
      exit $?
    fi

    if [ "${1:-}" = "/usr/bin/env" ]; then
      shift
      while [ $# -gt 0 ]; do
        case "${1:-}" in
          *=*)
            export "$1"
            shift
            ;;
          *)
            break
            ;;
        esac
      done
    fi

    if [ "${1:-}" = "nix-env" ]; then
      assert_root_nix_env
      if [ "$mode" = "split_profile_set_fail" ]; then
        echo "stub nix-env failed" >&2
        exit 1
      fi
      echo "stub nix-env ok"
      exit 0
    fi

    case "${1:-}" in
      /nix/store/*/activate)
        assert_root_nix_env
        if [ "$mode" = "split_activate_fail" ]; then
          echo "stub activate failed" >&2
          exit 1
        fi
        echo "setting up /etc..." >&2
        echo "Homebrew bundle..." >&2
        echo "Activating home-manager configuration for test" >&2
        echo "Activating linkGeneration" >&2
        echo "stub activate ok"
        exit 0
        ;;
    esac

    echo "stub sudo $*"
    exit 0
    ;;
  just)
    if [ "${1:-}" = "ci" ]; then
      if [ "$mode" = "just_fail" ]; then
        echo "stub just ci failed" >&2
        exit 1
      fi
      echo "stub just ci ok"
      exit 0
    fi
    echo "stub just unsupported: $*" >&2
    exit 1
    ;;
  python3)
    if [ "$mode" = "unittest_fail" ]; then
      echo "stub unittest failed" >&2
      exit 1
    fi
    echo "stub unittest ok"
    exit 0
    ;;
  home-manager)
    if [ "${1:-}" = "generations" ]; then
      printf '%s\n' "${NX_SYSTEM_IT_HOME_MANAGER_GENERATIONS:-2026-04-02 13:00 : id 7 -> /nix/store/example-home-manager-generation (current)}"
      exit 0
    fi

    if [ "${1:-}" = "remove-generations" ]; then
      if [ "$mode" = "hm_remove_fail" ]; then
        echo "stub home-manager remove-generations failed" >&2
        exit 1
      fi
      echo "stub home-manager remove-generations ok"
      exit 0
    fi

    echo "stub home-manager unsupported: $*" >&2
    exit 1
    ;;
  readlink)
    if [ "${1:-}" = "/nix/var/nix/profiles/system" ]; then
      printf '%s\n' "${NX_SYSTEM_IT_CURRENT_SYSTEM:-/nix/store/current-system}"
      exit 0
    fi
    echo "stub readlink unsupported: $*" >&2
    exit 1
    ;;
  scutil)
    if [ "${1:-}" = "--get" ] && [ "${2:-}" = "LocalHostName" ]; then
      printf '%s\n' "${NX_SYSTEM_IT_DARWIN_HOST:-test-host}"
      exit 0
    fi
    echo "stub scutil unsupported: $*" >&2
    exit 1
    ;;
  hostname)
    if [ "${1:-}" = "-s" ]; then
      printf '%s\n' "${NX_SYSTEM_IT_DARWIN_HOST:-test-host}"
      exit 0
    fi
    echo "stub hostname unsupported: $*" >&2
    exit 1
    ;;
  df)
    if [ "${1:-}" = "-h" ] && [ "${2:-}" = "/nix" ]; then
      printf '%s\n' "${NX_SYSTEM_IT_DF_OUTPUT:-Filesystem      Size    Used   Avail Capacity Mounted on
/dev/disk-test  100Gi   40Gi   60Gi   40% /nix}"
      exit 0
    fi

    echo "stub df unsupported: $*" >&2
    exit 1
    ;;
  darwin-rebuild)
    if [ "$mode" = "upgrade_hash_repair" ]; then
      marker="${HOME}/.nx-system-it-hash-repair-once"
      if [ ! -f "$marker" ]; then
        : > "$marker"
        echo "error: hash mismatch in fixed-output derivation '/nix/store/example-npm-deps.drv':" >&2
        echo "         specified: sha256-old" >&2
        echo "            got:    sha256-new" >&2
        exit 1
      fi
    fi
    if [ "$mode" = "darwin_rebuild_fail" ]; then
      echo "stub darwin-rebuild failed" >&2
      exit 1
    fi
    echo "building the system configuration..." >&2
    echo "these 2 derivations will be built:" >&2
    echo "copying path '/nix/store/example-one' from 'https://cache.nixos.org'" >&2
    echo "copying path '/nix/store/example-two' from 'https://cache.nixos.org'" >&2
    echo "building /nix/store/example.drv" >&2
    echo "setting up /etc..." >&2
    echo "Homebrew bundle..." >&2
    echo "Using ripgrep" >&2
    echo "Activating home-manager configuration for test" >&2
    echo "Activating linkGeneration" >&2
    echo "stub darwin-rebuild ok"
    exit 0
    ;;
  *)
    echo "unsupported stub program: $program" >&2
    exit 99
    ;;
esac
"#;
