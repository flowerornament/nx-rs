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
output_demo="${NX_OUTPUT_DEMO:-0}"

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
    if [ "${1:-}" = "--log-format" ]; then
      shift
      shift
    fi

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
      if [ "$mode" = "upgrade_lock_unreadable_post" ]; then
        printf '\377' > flake.lock
      elif [ "$mode" = "upgrade_flake_changed" ] || [ "$mode" = "upgrade_prefetch_cache_corruption" ] || [ "$mode" = "upgrade_hash_repair" ]; then
        printf '%s' "${NX_SYSTEM_IT_UPGRADE_NEW_LOCK:?NX_SYSTEM_IT_UPGRADE_NEW_LOCK must be set}" > flake.lock
      fi
      if [ "$output_demo" = "1" ]; then
        echo '@nix {"action":"start","id":70,"level":0,"parent":0,"text":"fetching updated flake inputs","type":112}' >&2
        sleep 0.18
        echo '@nix {"action":"stop","id":70}' >&2
      else
        echo "stub nix flake command ok"
      fi
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
        if [ "$output_demo" = "1" ]; then
          echo 'error: flake evaluation failed' >&2
        else
          echo "stub nix flake check failed" >&2
        fi
        exit 1
      fi
      if [ "$output_demo" = "1" ]; then
        echo '@nix {"action":"start","id":71,"level":0,"parent":0,"text":"evaluating flake checks","type":104}' >&2
        sleep 0.18
        echo '@nix {"action":"stop","id":71}' >&2
      else
        echo "stub nix flake check ok"
      fi
      exit 0
    fi

    if [ "${1:-}" = "build" ]; then
      is_dry_run=0
      for arg in "$@"; do
        if [ "$arg" = "--dry-run" ]; then
          is_dry_run=1
        fi
      done
      if [ "$is_dry_run" = "1" ]; then
        if [ "$mode" = "cache_preflight_misses" ]; then
          echo "these 6 derivations will be built:" >&2
          for name in starship-1.23.0 terminal-notifier-2.0.0 python3.12-httpx-0.28.1 darwin-system-26.05pre home-manager-generation nix-2.24.9; do
            echo "  /nix/store/00000000000000000000000000000000-${name}.drv" >&2
          done
        else
          echo "these 2 derivations will be built:" >&2
          echo "  /nix/store/00000000000000000000000000000000-starship-1.23.0.drv" >&2
          echo "  /nix/store/11111111111111111111111111111111-terminal-notifier-2.0.0.drv" >&2
        fi
        echo "these 1 paths will be fetched (1.00 MiB download, 2.00 MiB unpacked):" >&2
        echo "  /nix/store/22222222222222222222222222222222-bash-5.2p37" >&2
        exit 0
      fi
      if [ "$mode" = "split_build_cache_corruption" ]; then
        marker="${HOME}/.nx-system-it-split-build-cache-corruption-once"
        if [ ! -f "$marker" ]; then
          : > "$marker"
          echo "error: looking up file '«github:flowerornament/nx-rs/b9471c7»/scripts/test-home-manager-module.sh': object not found - no match for id (c2217e)" >&2
          exit 1
        fi
      fi
      if [ "$mode" = "split_build_fail" ]; then
        echo "error: builder for '/nix/store/sijh5v1ag1q0ad4bngvjxycf5716qfqx-anneal-0.13.1.drv' failed with exit code 101" >&2
        echo "       > failures:" >&2
        echo "       > ---- app::tests::eval_git_mtime_uses_git_history stdout ----" >&2
        echo "       > thread 'app::tests::eval_git_mtime_uses_git_history' panicked at crates/anneal-cli/src/app.rs:2491:35:" >&2
        echo "       > git [\"init\"] failed to run: No such file or directory (os error 2)" >&2
        echo "       > error: test failed, to rerun pass -p anneal-cli --lib" >&2
        echo "error: Build failed due to failed dependency" >&2
        exit 1
      fi
      if [ "$mode" = "split_build_invalid_json" ]; then
        echo "not-json"
        exit 0
      fi
      if [ "$output_demo" = "1" ]; then
        emit() {
          printf '%s\n' "$1" >&2
          sleep 0.12
        }
        emit '@nix {"action":"start","id":1,"level":0,"parent":0,"text":"","type":104}'
        emit '@nix {"action":"result","fields":[12,51,3,0],"id":1,"type":105}'
        emit '@nix {"action":"start","fields":["https://cache.nixos.org/nar/long-demo-path"],"id":2,"level":0,"parent":0,"text":"","type":101}'
        emit '@nix {"action":"result","fields":[67108864,536870912,0,0],"id":2,"type":105}'
        emit '@nix {"action":"msg","level":1,"msg":"warning: substituter response was slow\ncontinuing with cached metadata"}'
        emit '@nix {"action":"result","fields":[36,51,2,0],"id":1,"type":105}'
        emit '@nix {"action":"result","fields":[402653184,536870912,0,0],"id":2,"type":105}'
        emit '@nix {"action":"result","fields":[51,51,0,0],"id":1,"type":105}'
      else
        echo '@nix {"action":"start","id":1,"level":0,"parent":0,"text":"copying paths","type":103}' >&2
        echo '@nix {"action":"start","id":2,"level":0,"parent":0,"text":"building derivations","type":104}' >&2
      fi
      output="${NX_SYSTEM_IT_DARWIN_BUILD_OUTPUT:-/nix/store/new-system}"
      printf '%s\n' "$output"
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

    if [ "${1:-}" = "-H" ] && [ "${2:-}" = "-v" ]; then
      echo "usage: sudo -v [-ABkNnS] [-g group] [-h host] [-p prompt] [-u user]" >&2
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
      if [ "$mode" = "split_sudo_prompt_legacy_available" ]; then
        echo "sudo: a password is required" >&2
        exit 1
      fi
      if [ "$mode" = "split_sudo_prompt_run_current_legacy_available" ]; then
        echo "sudo: a password is required" >&2
        exit 1
      fi
      exit 0
    fi

    if [ "${1:-}" = "-n" ] && [ "${2:-}" = "-l" ] && [ "${3:-}" = "/nix/var/nix/profiles/system/sw/bin/darwin-rebuild" ]; then
      if [ "$mode" = "split_sudo_prompt_legacy_available" ]; then
        shift
        shift
        printf '%s' "$1"
        shift
        for arg in "$@"; do
          printf ' %s' "$arg"
        done
        printf '\n'
        exit 0
      fi
      echo "sudo: a password is required" >&2
      exit 1
    fi

    if [ "${1:-}" = "-n" ] && [ "${2:-}" = "-l" ] && [ "${3:-}" = "/run/current-system/sw/bin/darwin-rebuild" ]; then
      if [ "$mode" = "split_sudo_prompt_run_current_legacy_available" ]; then
        shift
        shift
        printf '%s' "$1"
        shift
        for arg in "$@"; do
          printf ' %s' "$arg"
        done
        printf '\n'
        exit 0
      fi
      echo "sudo: a password is required" >&2
      exit 1
    fi

    if [ "${1:-}" = "-n" ] && [ "${2:-}" = "/nix/var/nix/profiles/system/sw/bin/darwin-rebuild" ]; then
      if [ "$mode" = "split_sudo_prompt_legacy_available" ]; then
        echo "darwin-rebuild [--help] {edit | switch | activate | build | check}"
        exit 0
      fi
      echo "sudo: a password is required" >&2
      exit 1
    fi

    # Handle bash -lc wrapper (ulimit + exec darwin-rebuild)
    if [ "${1:-}" = "bash" ] && [ "${2:-}" = "-lc" ]; then
      cmd="${3:-}"
      cmd="$(printf '%s' "$cmd" | sed "s|/nix/var/nix/profiles/system/sw/bin/darwin-rebuild|${NX_SYSTEM_IT_DARWIN_REBUILD:?}|g")"
      bash -c "$cmd"
      exit $?
    fi

    if [ "${1:-}" = "/nix/var/nix/profiles/system/sw/bin/darwin-rebuild" ]; then
      shift
      "${NX_SYSTEM_IT_DARWIN_REBUILD:?NX_SYSTEM_IT_DARWIN_REBUILD must be set}" "$@"
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
        if [ "$output_demo" = "1" ]; then
          printf '  copying paths...\r  building derivations...\r  activation dependencies ready\n' >&2
        else
          echo '@nix {"action":"start","id":1,"level":0,"parent":0,"text":"copying paths","type":103}' >&2
          echo '@nix {"action":"start","id":2,"level":0,"parent":0,"text":"building derivations","type":104}' >&2
        fi
        if [ "$output_demo" = "1" ]; then
          echo "  setting up /etc..." >&2
          echo "  applying Homebrew bundle..." >&2
          echo "  activating Home Manager..." >&2
          echo "  linking generation..." >&2
        else
          echo "setting up /etc..." >&2
          echo "Homebrew bundle..." >&2
          echo "Activating home-manager configuration for test" >&2
          echo "Activating linkGeneration" >&2
          echo "stub activate ok"
        fi
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
    echo '@nix {"action":"start","id":1,"level":0,"parent":0,"text":"copying paths","type":103}' >&2
    echo '@nix {"action":"start","id":2,"level":0,"parent":0,"text":"building derivations","type":104}' >&2
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
