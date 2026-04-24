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
      if [ "$mode" = "preflight_untracked" ]; then
        echo "home/untracked-from-stub.nix"
        exit 0
      fi
      exit 0
    fi

    if [ "${1:-}" = "rev-parse" ] && [ "${2:-}" = "--show-toplevel" ]; then
      pwd
      exit 0
    fi

    if [ "${1:-}" = "status" ] && [ "${2:-}" = "--porcelain" ]; then
      if [ "$mode" = "undo_dirty" ]; then
        echo " M packages/nix/cli.nix"
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
      if [ "$mode" = "upgrade_flake_changed" ]; then
        printf '%s' "${NX_SYSTEM_IT_UPGRADE_NEW_LOCK:?NX_SYSTEM_IT_UPGRADE_NEW_LOCK must be set}" > flake.lock
      fi
      echo "stub nix flake command ok"
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
    if [ "$mode" = "darwin_rebuild_fail" ]; then
      echo "stub darwin-rebuild failed" >&2
      exit 1
    fi
    echo "stub darwin-rebuild ok"
    exit 0
    ;;
  *)
    echo "unsupported stub program: $program" >&2
    exit 99
    ;;
esac
"#;
