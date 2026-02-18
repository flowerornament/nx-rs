use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::domain::source::normalize_name;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigOptionInfo {
    pub path: String,
    pub example: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlakeHubInfo {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HintSeed {
    path: &'static str,
    example: &'static str,
}

static HM_MODULES: LazyLock<HashMap<&'static str, HintSeed>> = LazyLock::new(|| {
    HashMap::from([
        (
            "neovim",
            hint("programs.neovim", "programs.neovim.enable = true;"),
        ),
        (
            "emacs",
            hint("programs.emacs", "programs.emacs.enable = true;"),
        ),
        (
            "helix",
            hint("programs.helix", "programs.helix.enable = true;"),
        ),
        (
            "vscode",
            hint("programs.vscode", "programs.vscode.enable = true;"),
        ),
        (
            "kakoune",
            hint("programs.kakoune", "programs.kakoune.enable = true;"),
        ),
        ("zsh", hint("programs.zsh", "programs.zsh.enable = true;")),
        (
            "bash",
            hint("programs.bash", "programs.bash.enable = true;"),
        ),
        (
            "fish",
            hint("programs.fish", "programs.fish.enable = true;"),
        ),
        (
            "nushell",
            hint("programs.nushell", "programs.nushell.enable = true;"),
        ),
        (
            "git",
            hint(
                "programs.git",
                "programs.git.enable = true; programs.git.userName = \"...\";",
            ),
        ),
        (
            "lazygit",
            hint("programs.lazygit", "programs.lazygit.enable = true;"),
        ),
        ("gh", hint("programs.gh", "programs.gh.enable = true;")),
        (
            "jujutsu",
            hint("programs.jujutsu", "programs.jujutsu.enable = true;"),
        ),
        (
            "yazi",
            hint("programs.yazi", "programs.yazi.enable = true;"),
        ),
        ("lf", hint("programs.lf", "programs.lf.enable = true;")),
        ("nnn", hint("programs.nnn", "programs.nnn.enable = true;")),
        (
            "ranger",
            hint("programs.ranger", "programs.ranger.enable = true;"),
        ),
        (
            "tmux",
            hint("programs.tmux", "programs.tmux.enable = true;"),
        ),
        (
            "zellij",
            hint("programs.zellij", "programs.zellij.enable = true;"),
        ),
        (
            "starship",
            hint("programs.starship", "programs.starship.enable = true;"),
        ),
        (
            "direnv",
            hint("programs.direnv", "programs.direnv.enable = true;"),
        ),
        ("fzf", hint("programs.fzf", "programs.fzf.enable = true;")),
        (
            "zoxide",
            hint("programs.zoxide", "programs.zoxide.enable = true;"),
        ),
        (
            "atuin",
            hint("programs.atuin", "programs.atuin.enable = true;"),
        ),
        ("bat", hint("programs.bat", "programs.bat.enable = true;")),
        ("eza", hint("programs.eza", "programs.eza.enable = true;")),
        (
            "btop",
            hint("programs.btop", "programs.btop.enable = true;"),
        ),
        (
            "htop",
            hint("programs.htop", "programs.htop.enable = true;"),
        ),
        (
            "firefox",
            hint("programs.firefox", "programs.firefox.enable = true;"),
        ),
        (
            "chromium",
            hint("programs.chromium", "programs.chromium.enable = true;"),
        ),
        (
            "qutebrowser",
            hint(
                "programs.qutebrowser",
                "programs.qutebrowser.enable = true;",
            ),
        ),
        ("mpv", hint("programs.mpv", "programs.mpv.enable = true;")),
        (
            "password-store",
            hint(
                "programs.password-store",
                "programs.password-store.enable = true;",
            ),
        ),
        (
            "pass",
            hint(
                "programs.password-store",
                "programs.password-store.enable = true;",
            ),
        ),
        ("gpg", hint("programs.gpg", "programs.gpg.enable = true;")),
        ("ssh", hint("programs.ssh", "programs.ssh.enable = true;")),
        (
            "alacritty",
            hint("programs.alacritty", "programs.alacritty.enable = true;"),
        ),
        (
            "kitty",
            hint("programs.kitty", "programs.kitty.enable = true;"),
        ),
        (
            "wezterm",
            hint("programs.wezterm", "programs.wezterm.enable = true;"),
        ),
        (
            "ghostty",
            hint("programs.ghostty", "programs.ghostty.enable = true;"),
        ),
        ("rio", hint("programs.rio", "programs.rio.enable = true;")),
        (
            "rofi",
            hint("programs.rofi", "programs.rofi.enable = true;"),
        ),
        (
            "i3status",
            hint("programs.i3status", "programs.i3status.enable = true;"),
        ),
        (
            "waybar",
            hint("programs.waybar", "programs.waybar.enable = true;"),
        ),
    ])
});

static DARWIN_SERVICES: LazyLock<HashMap<&'static str, HintSeed>> = LazyLock::new(|| {
    HashMap::from([
        (
            "yabai",
            hint("services.yabai", "services.yabai.enable = true;"),
        ),
        (
            "skhd",
            hint("services.skhd", "services.skhd.enable = true;"),
        ),
        (
            "aerospace",
            hint("services.aerospace", "services.aerospace.enable = true;"),
        ),
        (
            "spacebar",
            hint("services.spacebar", "services.spacebar.enable = true;"),
        ),
        (
            "karabiner-elements",
            hint(
                "services.karabiner-elements",
                "services.karabiner-elements.enable = true;",
            ),
        ),
        (
            "sketchybar",
            hint("services.sketchybar", "services.sketchybar.enable = true;"),
        ),
        (
            "syncthing",
            hint("services.syncthing", "services.syncthing.enable = true;"),
        ),
        (
            "lorri",
            hint("services.lorri", "services.lorri.enable = true;"),
        ),
    ])
});

const FLAKEHUB_API_BASE: &str = "https://api.flakehub.com/flakes?q=";

pub fn hm_module_info(name: &str, repo_root: &Path) -> Option<ConfigOptionInfo> {
    lookup_config_option(name, repo_root, &HM_MODULES)
}

pub fn darwin_service_info(name: &str, repo_root: &Path) -> Option<ConfigOptionInfo> {
    lookup_config_option(name, repo_root, &DARWIN_SERVICES)
}

pub fn search_flakehub(name: &str) -> Vec<FlakeHubInfo> {
    search_flakehub_with(name, fetch_flakehub_json)
}

fn lookup_config_option(
    name: &str,
    repo_root: &Path,
    options: &HashMap<&'static str, HintSeed>,
) -> Option<ConfigOptionInfo> {
    let key = normalize_name(name);
    let seed = options.get(key.as_str())?;
    Some(ConfigOptionInfo {
        path: seed.path.to_string(),
        example: seed.example.to_string(),
        enabled: option_enabled(seed.path, repo_root),
    })
}

fn option_enabled(path: &str, repo_root: &Path) -> bool {
    let escaped = regex::escape(path);
    let pattern = format!(r"(?m)\b{escaped}\.enable\s*=\s*true\b");
    let Ok(enabled_re) = Regex::new(&pattern) else {
        return false;
    };

    collect_nix_files(repo_root).into_iter().any(|nix_file| {
        fs::read_to_string(&nix_file)
            .ok()
            .is_some_and(|content| enabled_re.is_match(&content))
    })
}

fn collect_nix_files(repo_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir_name in ["home", "system", "hosts", "packages"] {
        let dir = repo_root.join(dir_name);
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("nix") {
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out
}

fn search_flakehub_with<F>(name: &str, mut fetch: F) -> Vec<FlakeHubInfo>
where
    F: FnMut(&str) -> Option<Value>,
{
    let encoded = url_encode_component(name);
    let url = format!("{FLAKEHUB_API_BASE}{encoded}");
    let Some(payload) = fetch(&url) else {
        return Vec::new();
    };

    let flake_list = payload
        .as_array()
        .cloned()
        .or_else(|| payload.get("flakes").and_then(Value::as_array).cloned())
        .unwrap_or_default();

    let needle = name.to_ascii_lowercase();
    let mut out = Vec::new();
    for entry in flake_list {
        let Some(flake) = entry.as_object() else {
            continue;
        };

        let project = flake
            .get("project")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let description = flake
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let relevant = project.to_ascii_lowercase().contains(&needle)
            || description.to_ascii_lowercase().contains(&needle);
        if !relevant || project.is_empty() {
            continue;
        }

        let org = flake.get("org").and_then(Value::as_str).unwrap_or_default();
        let version = flake
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string);

        out.push(FlakeHubInfo {
            name: if org.is_empty() {
                project.to_string()
            } else {
                format!("{org}/{project}")
            },
            description: description.to_string(),
            version,
        });

        if out.len() == 5 {
            break;
        }
    }

    out
}

fn fetch_flakehub_json(url: &str) -> Option<Value> {
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            "10",
            "--header",
            "Accept: application/json",
            url,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn url_encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

const fn hint(path: &'static str, example: &'static str) -> HintSeed {
    HintSeed { path, example }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn hm_module_info_reports_enabled() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home directory should be created");
        fs::write(home.join("git.nix"), "programs.git.enable = true;\n")
            .expect("fixture should be written");

        let info = hm_module_info("git", tmp.path()).expect("git should have hm module");
        assert_eq!(info.path, "programs.git");
        assert!(info.enabled);
    }

    #[test]
    fn darwin_service_info_reports_enabled() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let system = tmp.path().join("system");
        fs::create_dir_all(&system).expect("system directory should be created");
        fs::write(system.join("darwin.nix"), "services.yabai.enable = true;\n")
            .expect("fixture should be written");

        let info = darwin_service_info("yabai", tmp.path()).expect("yabai should be supported");
        assert_eq!(info.path, "services.yabai");
        assert!(info.enabled);
    }

    #[test]
    fn config_option_returns_none_for_unknown_package() {
        let tmp = TempDir::new().expect("temp dir should be created");
        assert!(hm_module_info("not-a-real-package", tmp.path()).is_none());
        assert!(darwin_service_info("not-a-real-package", tmp.path()).is_none());
    }

    #[test]
    fn search_flakehub_with_filters_and_limits() {
        let payload = serde_json::json!([
            {"org":"Org","project":"ripgrep-tools","description":"ripgrep helper"},
            {"org":"Org","project":"not-relevant","description":"no match"},
            {"org":"Org","project":"ripgrep-kit-1","description":"match"},
            {"org":"Org","project":"ripgrep-kit-2","description":"match"},
            {"org":"Org","project":"ripgrep-kit-3","description":"match"},
            {"org":"Org","project":"ripgrep-kit-4","description":"match"},
            {"org":"Org","project":"ripgrep-kit-5","description":"match"}
        ]);

        let results = search_flakehub_with("ripgrep", |_| Some(payload.clone()));
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].name, "Org/ripgrep-tools");
        assert_eq!(results[0].description, "ripgrep helper");
    }

    #[test]
    fn search_flakehub_with_accepts_object_payload() {
        let payload = serde_json::json!({
            "flakes": [
                {"org":"Acme","project":"tool","description":"tool for rust", "version":"1.2.3"}
            ]
        });
        let results = search_flakehub_with("tool", |_| Some(payload.clone()));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Acme/tool");
        assert_eq!(results[0].version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn search_flakehub_with_returns_empty_when_fetch_fails() {
        let results = search_flakehub_with("tool", |_| None);
        assert!(results.is_empty());
    }

    #[test]
    fn url_encode_component_encodes_reserved_characters() {
        assert_eq!(
            url_encode_component("python3Packages.requests"),
            "python3Packages.requests"
        );
        assert_eq!(url_encode_component("foo/bar baz"), "foo%2Fbar%20baz");
    }
}
