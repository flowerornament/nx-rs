"""
sources.py - Multi-source package search for nx v3

Searches across multiple Nix package sources:
- nxs (nixpkgs, as pinned)
- NUR (Nix User Repository)
- flake inputs (existing overlays)
- homebrew (fallback)
"""

from __future__ import annotations

import json
import logging
import platform as _platform
import re
import shutil
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import Any, cast

from tenacity import stop_after_attempt, wait_exponential

from retry_utils import typed_retry
from shared import (
    NAME_MAPPINGS,
    clean_attr_path,
    detect_language_package,
    parse_nix_search_results,
    run_json_command,
    score_match,
)

logger = logging.getLogger(__name__)

# ═══════════════════════════════════════════════════════════════════════════════
# Types
# ═══════════════════════════════════════════════════════════════════════════════

@dataclass
class SourcePreferences:
    """User preferences for source selection."""
    bleeding_edge: bool = False      # Prefer overlays/NUR when possible
    nur: bool = False                # Enable NUR search
    force_source: str | None = None  # Force specific source
    is_cask: bool = False            # GUI application
    is_mas: bool = False             # Mac App Store


@dataclass
class SourceResult:
    """Result from searching a package source."""
    name: str                        # Original search name
    source: str                      # "nxs" | "nur" | "flake-input" | "homebrew" | "cask" | "mas"
    attr: str | None = None       # Attribute path (e.g., "ripgrep" for pkgs.ripgrep)
    version: str | None = None    # Version if available
    confidence: float = 0.0          # Match confidence (0.0-1.0)
    description: str = ""            # Package description
    requires_flake_mod: bool = False # Needs flake.nix modification?
    flake_url: str | None = None  # URL for new flake input


# ═══════════════════════════════════════════════════════════════════════════════
# Individual Source Searches
# ═══════════════════════════════════════════════════════════════════════════════


def _mapped_name(name: str) -> str:
    """Resolve common aliases case-insensitively."""
    return NAME_MAPPINGS.get(name.lower(), NAME_MAPPINGS.get(name, name))


def _search_name_variants(name: str) -> list[str]:
    """Generate stable search variants for punctuation-heavy package names."""
    variants: list[str] = []
    for candidate in (_mapped_name(name), name):
        if candidate and candidate not in variants:
            variants.append(candidate)
        compact = re.sub(r"[-_.]+", "", candidate)
        if compact and compact not in variants:
            variants.append(compact)
    return variants[:3]


def _search_nix_source(
    name: str,
    targets: list[str],
    source: str,
    requires_flake_mod: bool = False,
    flake_url: str | None = None,
    timeout: int = 30,
) -> list[SourceResult]:
    """Shared nix search helper for nxs/NUR."""
    if shutil.which("nix") is None:
        return []

    all_entries: list[dict[str, Any]] = []
    seen_attrs: set[str] = set()
    for search_name in _search_name_variants(name):
        success = False
        data = None
        for target in targets:
            success, data = run_json_command(
                ["nix", "search", "--json", target, search_name],
                timeout=timeout,
            )
            if success and data:
                break
        if not success or not data:
            continue

        for entry in parse_nix_search_results(data):
            attr = entry.get("attrPath")
            if not isinstance(attr, str) or not attr or attr in seen_attrs:
                continue
            seen_attrs.add(attr)
            all_entries.append(entry)

    if not all_entries:
        return []

    results = []
    for entry in all_entries:
        attr = entry.get("attrPath", "")
        pname = entry.get("pname", "")
        score = score_match(_mapped_name(name), attr, pname)

        if score >= 0.3:
            attr_clean = clean_attr_path(attr)
            description = entry.get("description", "")
            if isinstance(description, str) and len(description) > 100:
                description = description[:97] + "..."

            results.append(SourceResult(
                name=name,
                source=source,
                attr=attr_clean,
                version=entry.get("version"),
                confidence=score,
                description=description,
                requires_flake_mod=requires_flake_mod,
                flake_url=flake_url,
            ))

    results.sort(key=lambda r: r.confidence, reverse=True)
    return results[:5]


def _eval_nix_attr(
    targets: list[str],
    attr_path: str,
    timeout: int = 15,
) -> tuple[bool, Any | None]:
    for target in targets:
        success, data = run_json_command(
            ["nix", "eval", "--json", f"{target}#{attr_path}"],
            timeout=timeout,
        )
        if success and data is not None:
            return True, data
    return False, None


def get_current_system() -> str:
    """Return the current Nix system identifier (e.g., 'aarch64-darwin')."""
    machine = _platform.machine()
    arch_map = {"arm64": "aarch64", "x86_64": "x86_64", "aarch64": "aarch64"}
    arch = arch_map.get(machine, machine)
    os_name = "darwin" if _platform.system() == "Darwin" else "linux"
    return f"{arch}-{os_name}"


def check_nix_available(attr: str) -> tuple[bool, str | None]:
    """Check if a nix package is available on the current platform.

    Evaluates meta.platforms and rejects packages that explicitly
    exclude the current system.

    Returns:
        (True, None) if available or can't determine.
        (False, reason) if definitely not available.
    """
    if shutil.which("nix") is None:
        return True, None

    targets = ["nixpkgs"]
    success, platforms = _eval_nix_attr(targets, f"{attr}.meta.platforms", timeout=15)

    if not success or platforms is None:
        return True, None  # Can't determine; allow

    if not isinstance(platforms, list) or not platforms:
        return True, None  # Empty or unexpected; allow

    current = get_current_system()
    platform_strings = [p for p in platforms if isinstance(p, str)]

    if platform_strings and current not in platform_strings:
        return False, f"not available on {current} (only: {', '.join(platform_strings)})"

    return True, None


def search_nxs(name: str, prefer_unstable: bool = False) -> list[SourceResult]:
    """Search nixpkgs for a package."""
    # Search nixpkgs directly (no "nxs" flake exists in registry)
    if prefer_unstable:
        search_targets = ["github:nixos/nixpkgs/nixos-unstable", "nixpkgs"]
    else:
        search_targets = ["nixpkgs", "github:nixos/nixpkgs/nixos-unstable"]

    return _search_nix_source(name, search_targets, "nxs")


def search_nur(name: str) -> list[SourceResult]:
    """Search NUR (Nix User Repository) for a package."""
    return _search_nix_source(
        name,
        ["github:nix-community/NUR"],
        "nur",
        requires_flake_mod=True,
        flake_url="github:nix-community/NUR",
        timeout=60,
    )


def search_flake_inputs(name: str, flake_lock_path: Path) -> list[SourceResult]:
    """Check existing flake inputs for package overlays."""
    if not flake_lock_path.exists():
        return []

    try:
        with open(flake_lock_path) as f:
            lock = json.load(f)
    except Exception:
        return []

    results = []
    nodes = lock.get("nodes", {})

    # Known overlays that provide packages
    overlay_packages = {
        "neovim-nightly-overlay": ["neovim", "neovim-nightly"],
        "rust-overlay": ["rust", "cargo", "rustc", "rust-analyzer"],
        "fenix": ["rust", "cargo", "rustc", "rust-analyzer", "rust-src"],
        "nxs-mozilla": ["firefox", "firefox-nightly"],
    }

    search_name = _mapped_name(name).lower()

    for input_name, _input_data in nodes.items():
        if input_name == "root":
            continue

        # Check if this input provides the package
        if input_name in overlay_packages:
            provided = overlay_packages[input_name]
            for pkg in provided:
                if search_name in pkg.lower() or pkg.lower() in search_name:
                    results.append(SourceResult(
                        name=name,
                        source="flake-input",
                        attr=pkg,
                        confidence=0.9 if pkg.lower() == search_name else 0.7,
                        description=f"From {input_name} overlay",
                    ))

    return results


def search_homebrew(
    name: str,
    is_cask: bool = False,
    allow_fallback: bool = True,
) -> list[SourceResult]:
    """Search Homebrew for a package."""
    entry = _get_homebrew_info_entry(name, is_cask=is_cask)
    if not entry:
        # Try the opposite (cask vs formula)
        if allow_fallback and not is_cask:
            return search_homebrew(name, is_cask=True, allow_fallback=False)
        return []

    if is_cask:
        return [SourceResult(
            name=name,
            source="cask",
            attr=entry.get("token", name),
            version=entry.get("version"),
            confidence=1.0,
            description=entry.get("desc", "GUI application"),
        )]

    return [SourceResult(
        name=name,
        source="homebrew",
        attr=entry.get("name", name),
        version=entry.get("versions", {}).get("stable"),
        confidence=0.8,  # Lower than nxs by default
        description=entry.get("desc", ""),
    )]


# ═══════════════════════════════════════════════════════════════════════════════
# Orchestration Helpers
# ═══════════════════════════════════════════════════════════════════════════════


def _submit_parallel_searches(
    executor: ThreadPoolExecutor,
    name: str,
    prefs: SourcePreferences,
    flake_lock_path: Path | None,
) -> dict[Any, str]:
    futures: dict[Any, str] = {}
    futures[executor.submit(search_nxs, name)] = "nxs"
    if flake_lock_path:
        futures[executor.submit(search_flake_inputs, name, flake_lock_path)] = "flake"
    if prefs.nur or prefs.bleeding_edge:
        futures[executor.submit(search_nur, name)] = "nur"
    return futures


def _collect_future_results(
    future: Any,
    source_name: str,
    name: str,
) -> list[SourceResult]:
    try:
        return cast(list[SourceResult], future.result())
    except Exception as exc:
        logger.warning(
            "Search source '%s' failed for '%s': %s",
            source_name,
            name,
            exc,
        )
        return []


def _cancel_pending_futures(futures: dict[Any, str]) -> None:
    for future in futures:
        if not future.done():
            future.cancel()


def _parallel_search(
    name: str,
    prefs: SourcePreferences,
    flake_lock_path: Path | None = None,
) -> list[SourceResult]:
    """Execute parallel searches across enabled sources.

    Args:
        name: Package name to search
        prefs: Source preferences
        flake_lock_path: Path to flake.lock (optional)

    Returns:
        Combined list of results from all sources
    """
    results: list[SourceResult] = []
    executor = ThreadPoolExecutor(max_workers=4)
    futures = _submit_parallel_searches(executor, name, prefs, flake_lock_path)
    timed_out = False

    try:
        # Collect results. If one source stalls, keep completed sources and move on.
        processed: set[Any] = set()
        try:
            for future in as_completed(futures, timeout=45):
                processed.add(future)
                source_name = futures.get(future, "unknown")
                results.extend(_collect_future_results(future, source_name, name))
        except TimeoutError:
            timed_out = True
            logger.warning(
                "Timed out waiting for one or more search sources for '%s'; using partial results",
                name,
            )

        # Collect any futures that completed before/after timeout but were not yielded.
        for future, source_name in futures.items():
            if future in processed or not future.done():
                continue
            results.extend(_collect_future_results(future, source_name, name))

        return results
    finally:
        if timed_out:
            _cancel_pending_futures(futures)
            executor.shutdown(wait=False, cancel_futures=True)
        else:
            executor.shutdown(wait=True)


def _sort_results(results: list[SourceResult], prefs: SourcePreferences) -> None:
    """Sort results in-place by source priority and confidence.

    Args:
        results: List of results to sort (modified in-place)
        prefs: Source preferences (affects priority order)
    """
    # Priority: flake-input (existing) > nxs > nur > homebrew
    source_priority = {
        "flake-input": 0,
        "nxs": 1,
        "nur": 2,
        "homebrew": 3,
        "cask": 4,
    }

    # If bleeding edge requested, prefer unstable/overlays
    if prefs.bleeding_edge:
        source_priority = {
            "flake-input": 0,
            "nur": 1,
            "nxs": 2,
            "homebrew": 3,
            "cask": 4,
        }

    def sort_key(r: SourceResult) -> tuple:
        return (source_priority.get(r.source, 99), -r.confidence)

    results.sort(key=sort_key)


def _deduplicate_results(results: list[SourceResult]) -> list[SourceResult]:
    """Remove duplicate results by (source, attr) key.

    Args:
        results: List of results (should be pre-sorted)

    Returns:
        Deduplicated list preserving order
    """
    seen: set = set()
    unique_results: list[SourceResult] = []
    for r in results:
        key = (r.source, r.attr)
        if key not in seen:
            seen.add(key)
            unique_results.append(r)
    return unique_results


# ═══════════════════════════════════════════════════════════════════════════════
# Main Orchestration
# ═══════════════════════════════════════════════════════════════════════════════

def search_all_sources(
    name: str,
    prefs: SourcePreferences,
    flake_lock_path: Path | None = None,
) -> list[SourceResult]:
    """
    Search all enabled sources for a package.

    Returns results sorted by preference and confidence.
    """
    forced = _search_forced_source(name, prefs)
    if forced is not None:
        return forced

    explicit = _search_explicit_source(name, prefs)
    if explicit is not None:
        return explicit

    lang_result = _search_language_override(name)
    if lang_result is not None:
        return lang_result

    # Parallel search of primary sources
    results = _parallel_search(name, prefs, flake_lock_path)

    # Always search homebrew (both formulas and casks) to provide alternatives
    formula_results = search_homebrew(name, is_cask=False, allow_fallback=False)
    cask_results = search_homebrew(name, is_cask=True, allow_fallback=False)
    results.extend(formula_results)
    results.extend(cask_results)

    # Sort and deduplicate
    _sort_results(results, prefs)
    return _deduplicate_results(results)


def _search_forced_source(name: str, prefs: SourcePreferences) -> list[SourceResult] | None:
    if not prefs.force_source:
        return None
    if prefs.force_source == "nxs":
        return search_nxs(name)
    if prefs.force_source == "unstable":
        return search_nxs(name, prefer_unstable=True)
    if prefs.force_source == "nur":
        return search_nur(name)
    if prefs.force_source == "homebrew":
        return search_homebrew(name, prefs.is_cask)
    return None


def _search_explicit_source(name: str, prefs: SourcePreferences) -> list[SourceResult] | None:
    if prefs.is_cask:
        return [SourceResult(
            name=name,
            source="cask",
            attr=name,
            confidence=1.0,
            description="GUI application (cask)",
        )]
    if prefs.is_mas:
        return [SourceResult(
            name=name,
            source="mas",
            attr=name,
            confidence=1.0,
            description="Mac App Store app",
        )]
    return None


def _validate_language_override(attr: str) -> tuple[bool, str | None]:
    """Validate that a language package attr exists and matches this platform."""
    if shutil.which("nix") is None:
        return False, "nix command unavailable"

    targets = ["nixpkgs", "github:nixos/nixpkgs/nixos-unstable"]
    exists, _ = _eval_nix_attr(targets, f"{attr}.name", timeout=15)
    if not exists:
        return False, "attribute not found in nixpkgs"

    available, reason = check_nix_available(attr)
    if not available:
        return False, reason

    return True, None


def _search_language_override(name: str) -> list[SourceResult] | None:
    lang_info = detect_language_package(name)
    if not lang_info:
        return None

    valid, reason = _validate_language_override(name)
    if not valid:
        if reason and reason != "nix command unavailable":
            logger.warning("Skipping language override '%s': %s", name, reason)
        return None

    _bare_name, runtime, _ = lang_info
    return [SourceResult(
        name=name,
        source="nxs",
        attr=name,
        confidence=1.0,
        description=f"{runtime} package",
    )]


# ═══════════════════════════════════════════════════════════════════════════════
# Package Metadata Queries
# ═══════════════════════════════════════════════════════════════════════════════

@dataclass
class PackageInfo:
    """Detailed package information from any source."""
    name: str
    source: str                           # nxs, homebrew, cask, nur, flake:*
    version: str | None = None
    description: str | None = None
    homepage: str | None = None
    license: str | None = None
    changelog: str | None = None
    dependencies: list[str] | None = None
    build_dependencies: list[str] | None = None
    caveats: str | None = None
    artifacts: list[str] | None = None  # For casks: what gets installed
    broken: bool = False
    insecure: bool = False
    head_available: bool = False          # Homebrew HEAD build available


def get_nix_package_info(attr: str) -> PackageInfo | None:
    """Get detailed info about a nxs package.

    Args:
        attr: Package attribute (e.g., "ripgrep", "python3Packages.requests")

    Returns:
        PackageInfo or None if not found
    """
    if shutil.which("nix") is None:
        return None

    # Apply name mapping (e.g., nvim -> neovim)
    attr = NAME_MAPPINGS.get(attr, attr)

    targets = ["nxs", "nixpkgs", "github:nixos/nixpkgs/nixos-unstable"]

    # Get version
    success, version_data = _eval_nix_attr(targets, f"{attr}.version", timeout=15)
    version = version_data if success and isinstance(version_data, str) else None

    # Get meta
    success, meta = _eval_nix_attr(targets, f"{attr}.meta", timeout=15)
    if not success or not meta:
        return None

    # Parse license
    license_info = meta.get("license")
    license_str = None
    if isinstance(license_info, dict):
        license_str = license_info.get("spdxId") or license_info.get("fullName")
    elif isinstance(license_info, list) and license_info:
        first = license_info[0]
        if isinstance(first, dict):
            license_str = first.get("spdxId") or first.get("fullName")

    return PackageInfo(
        name=attr,
        source="nxs",
        version=version,
        description=meta.get("description"),
        homepage=meta.get("homepage"),
        license=license_str,
        changelog=meta.get("changelog"),
        broken=meta.get("broken", False),
        insecure=meta.get("insecure", False),
    )


def get_homebrew_formula_info(name: str) -> PackageInfo | None:
    """Get detailed info about a Homebrew formula.

    Args:
        name: Formula name (e.g., "mpd", "wget")

    Returns:
        PackageInfo or None if not found
    """
    f = _get_homebrew_info_entry(name, is_cask=False)
    if not f:
        return None
    versions = f.get("versions", {})

    # Check if HEAD build is available
    head_available = versions.get("head") is not None

    return PackageInfo(
        name=f.get("name", name),
        source="homebrew",
        version=versions.get("stable"),
        description=f.get("desc"),
        homepage=f.get("homepage"),
        license=f.get("license"),
        dependencies=f.get("dependencies", []),
        build_dependencies=f.get("build_dependencies", []),
        caveats=f.get("caveats"),
        head_available=head_available,
    )


def get_homebrew_cask_info(name: str) -> PackageInfo | None:
    """Get detailed info about a Homebrew cask.

    Args:
        name: Cask token (e.g., "ghostty", "firefox")

    Returns:
        PackageInfo or None if not found
    """
    c = _get_homebrew_info_entry(name, is_cask=True)
    if not c:
        return None

    # Extract artifacts (apps, binaries, etc.)
    artifacts = []
    for artifact in c.get("artifacts", []):
        if isinstance(artifact, dict):
            for key, value in artifact.items():
                if key in ("app", "binary", "pkg"):
                    if isinstance(value, list):
                        artifacts.extend(value)
                    else:
                        artifacts.append(str(value))

    return PackageInfo(
        name=c.get("token", name),
        source="cask",
        version=c.get("version"),
        description=c.get("desc"),
        homepage=c.get("homepage"),
        artifacts=artifacts if artifacts else None,
    )


def _get_homebrew_info_entry(name: str, is_cask: bool) -> dict[str, Any] | None:
    if shutil.which("brew") is None:
        return None

    cmd = ["brew", "info", "--json=v2"]
    if is_cask:
        cmd.append("--cask")
    cmd.append(name)

    success, data = run_json_command(cmd, timeout=15)
    if not success or not data:
        return None

    key = "casks" if is_cask else "formulae"
    entries = data.get(key, [])
    if not entries:
        return None

    entry = entries[0]
    if not isinstance(entry, dict):
        return None
    return entry


# Known overlays and the packages they replace/provide
OVERLAY_PACKAGES: dict[str, tuple[str, str, str]] = {
    # package_name: (overlay_name, attr_in_overlay, description)
    "neovim": ("neovim-nightly-overlay", "default", "Neovim nightly build"),
    "nvim": ("neovim-nightly-overlay", "default", "Neovim nightly build"),
    "rust": ("fenix", "default.toolchain", "Rust nightly toolchain"),
    "cargo": ("fenix", "default.toolchain", "Rust nightly toolchain"),
    "rustc": ("fenix", "default.toolchain", "Rust nightly toolchain"),
    "rust-analyzer": ("fenix", "rust-analyzer", "Rust analyzer nightly"),
    "emacs": ("emacs-overlay", "emacs-git", "Emacs from git master"),
    "zig": ("zig-overlay", "master", "Zig nightly build"),
    # nxs-mozilla
    "firefox": ("nxs-mozilla", "firefox-nightly-bin", "Firefox Nightly"),
    "firefox-nightly": ("nxs-mozilla", "firefox-nightly-bin", "Firefox Nightly"),
    # rust-overlay (alternative to fenix)
    "rust-bin": ("rust-overlay", "rust", "Rust from rust-overlay"),
}

# Home-manager modules - maps package names to their programs.X module
# Format: package_name -> (hm_module_path, example_config)
HM_MODULES: dict[str, tuple[str, str]] = {
    # Editors
    "neovim": ("programs.neovim", "programs.neovim.enable = true;"),
    "nvim": ("programs.neovim", "programs.neovim.enable = true;"),
    "vim": ("programs.vim", "programs.vim.enable = true;"),
    "emacs": ("programs.emacs", "programs.emacs.enable = true;"),
    "helix": ("programs.helix", "programs.helix.enable = true;"),
    "vscode": ("programs.vscode", "programs.vscode.enable = true;"),
    "kakoune": ("programs.kakoune", "programs.kakoune.enable = true;"),
    # Shells
    "zsh": ("programs.zsh", "programs.zsh.enable = true;"),
    "bash": ("programs.bash", "programs.bash.enable = true;"),
    "fish": ("programs.fish", "programs.fish.enable = true;"),
    "nushell": ("programs.nushell", "programs.nushell.enable = true;"),
    # Git & version control
    "git": ("programs.git", "programs.git.enable = true; programs.git.userName = \"...\";"),
    "lazygit": ("programs.lazygit", "programs.lazygit.enable = true;"),
    "gh": ("programs.gh", "programs.gh.enable = true;"),
    "jujutsu": ("programs.jujutsu", "programs.jujutsu.enable = true;"),
    # File managers
    "yazi": ("programs.yazi", "programs.yazi.enable = true;"),
    "lf": ("programs.lf", "programs.lf.enable = true;"),
    "nnn": ("programs.nnn", "programs.nnn.enable = true;"),
    "ranger": ("programs.ranger", "programs.ranger.enable = true;"),
    # Terminal tools
    "tmux": ("programs.tmux", "programs.tmux.enable = true;"),
    "zellij": ("programs.zellij", "programs.zellij.enable = true;"),
    "starship": ("programs.starship", "programs.starship.enable = true;"),
    "direnv": ("programs.direnv", "programs.direnv.enable = true;"),
    "fzf": ("programs.fzf", "programs.fzf.enable = true;"),
    "zoxide": ("programs.zoxide", "programs.zoxide.enable = true;"),
    "atuin": ("programs.atuin", "programs.atuin.enable = true;"),
    "bat": ("programs.bat", "programs.bat.enable = true;"),
    "eza": ("programs.eza", "programs.eza.enable = true;"),
    "btop": ("programs.btop", "programs.btop.enable = true;"),
    "htop": ("programs.htop", "programs.htop.enable = true;"),
    # Browsers
    "firefox": ("programs.firefox", "programs.firefox.enable = true;"),
    "chromium": ("programs.chromium", "programs.chromium.enable = true;"),
    "qutebrowser": ("programs.qutebrowser", "programs.qutebrowser.enable = true;"),
    # Media
    "mpv": ("programs.mpv", "programs.mpv.enable = true;"),
    # Password managers
    "password-store": ("programs.password-store", "programs.password-store.enable = true;"),
    "pass": ("programs.password-store", "programs.password-store.enable = true;"),
    # Misc
    "gpg": ("programs.gpg", "programs.gpg.enable = true;"),
    "ssh": ("programs.ssh", "programs.ssh.enable = true;"),
    "alacritty": ("programs.alacritty", "programs.alacritty.enable = true;"),
    "kitty": ("programs.kitty", "programs.kitty.enable = true;"),
    "wezterm": ("programs.wezterm", "programs.wezterm.enable = true;"),
    "ghostty": ("programs.ghostty", "programs.ghostty.enable = true;"),
    "rio": ("programs.rio", "programs.rio.enable = true;"),
    "rofi": ("programs.rofi", "programs.rofi.enable = true;"),
    "i3status": ("programs.i3status", "programs.i3status.enable = true;"),
    "waybar": ("programs.waybar", "programs.waybar.enable = true;"),
}

# Darwin-specific services and options
# Format: package_name -> (darwin_option_path, example_config)
DARWIN_SERVICES: dict[str, tuple[str, str]] = {
    # Window managers
    "yabai": ("services.yabai", "services.yabai.enable = true;"),
    "skhd": ("services.skhd", "services.skhd.enable = true;"),
    "aerospace": ("services.aerospace", "services.aerospace.enable = true;"),
    "spacebar": ("services.spacebar", "services.spacebar.enable = true;"),
    # Utilities
    "karabiner-elements": ("services.karabiner-elements", "services.karabiner-elements.enable = true;"),
    "sketchybar": ("services.sketchybar", "services.sketchybar.enable = true;"),
    # Services that work via launchd
    "syncthing": ("services.syncthing", "services.syncthing.enable = true;"),
    "lorri": ("services.lorri", "services.lorri.enable = true;"),
}


def check_overlay_active(name: str, repo_root: Path) -> str | None:
    """Check if a package has an active overlay applied.

    Args:
        name: Package name (e.g., "neovim", "nvim")
        repo_root: Path to the nix-config repo

    Returns:
        Overlay name if active (e.g., "neovim-nightly-overlay"), None otherwise
    """
    name_lower = NAME_MAPPINGS.get(name.lower(), name.lower())
    if name_lower not in OVERLAY_PACKAGES:
        return None

    overlay_name, _, _ = OVERLAY_PACKAGES[name_lower]

    # Check if overlay is in flake.lock
    flake_lock = repo_root / "flake.lock"
    if not flake_lock.exists():
        return None

    try:
        with open(flake_lock) as f:
            lock = json.load(f)
        nodes = lock.get("nodes", {})
        if overlay_name not in nodes:
            return None
    except Exception:
        return None

    # Check if overlay is applied in any .nix file
    # Look for patterns like: nxs.overlays = [ inputs.neovim-nightly-overlay... ]
    for nix_file in repo_root.glob("**/*.nix"):
        try:
            content = nix_file.read_text()
            if f"inputs.{overlay_name}" in content and "overlays" in content:
                return overlay_name
        except Exception:
            continue

    return None


def get_flake_overlay_info(name: str, flake_lock_path: Path | None = None) -> PackageInfo | None:
    """Get info about a package from known flake overlays.

    Checks overlays like neovim-nightly-overlay, rust-overlay, etc.

    Args:
        name: Package name
        flake_lock_path: Path to flake.lock to check which overlays are available

    Returns:
        PackageInfo or None if not found in any overlay
    """
    if shutil.which("nix") is None:
        return None

    # Use the global overlay map
    overlay_map = OVERLAY_PACKAGES

    name_lower = name.lower()
    if name_lower not in overlay_map:
        return None

    overlay_name, pkg_attr, desc = overlay_map[name_lower]

    # Check if overlay is in flake.lock
    if flake_lock_path and flake_lock_path.exists():
        try:
            with open(flake_lock_path) as f:
                lock = json.load(f)
            nodes = lock.get("nodes", {})
            if overlay_name not in nodes:
                return None  # Overlay not configured
        except Exception:
            pass

    # Try to get version from the overlay
    # Use the local flake reference if available

    success, version = run_json_command(
        ["nix", "eval", "--json", f"path:.#packages.aarch64-darwin.{pkg_attr}.version"],
        timeout=15,
    )

    version_str = version if success and isinstance(version, str) else "nightly"

    return PackageInfo(
        name=name,
        source=f"flake:{overlay_name}",
        version=version_str,
        description=desc,
        homepage=f"https://github.com/nix-community/{overlay_name}",
    )


def get_nur_package_info(name: str) -> PackageInfo | None:
    """Get info about a package from NUR (Nix User Repository).

    NUR is organized by maintainer (repos.<maintainer>.<package>), so we check
    known popular packages directly rather than searching.

    Args:
        name: Package name

    Returns:
        PackageInfo or None if not found
    """
    if shutil.which("nix") is None:
        return None

    # Known NUR packages and their locations
    # Format: package_name -> (maintainer, attr, description)
    nur_packages = {
        "firefox-addons": ("rycee", "firefox-addons", "Firefox browser extensions"),
        "ublock-origin": ("rycee", "firefox-addons.ublock-origin", "Ad blocker for Firefox"),
        "vimix-gtk-themes": ("mweinelt", "vimix-gtk-themes", "Flat Material Design theme"),
    }

    name_lower = name.lower().replace("-", "").replace("_", "")

    for pkg_name, (maintainer, attr, desc) in nur_packages.items():
        if name_lower in pkg_name.replace("-", "").replace("_", ""):
            # Try to get version
            success, version = _eval_nix_attr(
                ["github:nix-community/NUR"],
                f"repos.{maintainer}.{attr}.version",
                timeout=15,
            )
            return PackageInfo(
                name=f"nur.repos.{maintainer}.{attr}",
                source="nur",
                version=version if success and isinstance(version, str) else None,
                description=desc,
                homepage="https://github.com/nix-community/NUR",
            )

    return None


def get_package_set_info(name: str) -> PackageInfo | None:
    """Check if a name refers to a package set (like nerd-fonts) and provide guidance.

    Args:
        name: Package name that might be a set

    Returns:
        PackageInfo with guidance, or None
    """
    if shutil.which("nix") is None:
        return None

    # Known package sets and example subpackages
    package_sets = {
        "nerd-fonts": ["hack", "fira-code", "jetbrains-mono", "meslo-lg", "iosevka"],
        "python3Packages": ["requests", "numpy", "pandas"],
        "nodePackages": ["typescript", "prettier", "eslint"],
        "rubyPackages": ["rails", "bundler"],
        "perlPackages": ["DBI", "Mojolicious"],
    }

    name_lower = name.lower().replace("-", "").replace("_", "")

    for set_name, examples in package_sets.items():
        set_normalized = set_name.lower().replace("-", "").replace("_", "")
        if name_lower == set_normalized:
            # It's a package set, not a package
            examples_str = ", ".join(f"{set_name}.{e}" for e in examples[:4])
            return PackageInfo(
                name=set_name,
                source="nxs-set",
                description=f"Package set. Use specific packages like: {examples_str}",
                homepage="https://search.nixos.org",
            )

    return None


@dataclass
class HMModuleInfo:
    """Home-manager module information."""
    module_path: str           # e.g., "programs.neovim"
    example_config: str        # e.g., "programs.neovim.enable = true;"
    is_enabled: bool = False   # Whether it's already enabled in config


@dataclass
class DarwinServiceInfo:
    """Darwin service information."""
    service_path: str          # e.g., "services.yabai"
    example_config: str        # e.g., "services.yabai.enable = true;"
    is_enabled: bool = False   # Whether it's already enabled in config


def get_hm_module_info(name: str, repo_root: Path | None = None) -> HMModuleInfo | None:
    """Check if a package has a home-manager module available.

    Args:
        name: Package name (e.g., "neovim", "git")
        repo_root: Path to nix config repo to check if already enabled

    Returns:
        HMModuleInfo or None if no module available
    """
    name_lower = NAME_MAPPINGS.get(name.lower(), name.lower())

    if name_lower not in HM_MODULES:
        return None

    module_path, example = HM_MODULES[name_lower]

    # Check if already enabled
    is_enabled = False
    if repo_root:
        for nix_file in repo_root.glob("**/*.nix"):
            try:
                content = nix_file.read_text()
                # Look for pattern like: programs.neovim.enable = true
                if f"{module_path}.enable" in content and "true" in content:
                    is_enabled = True
                    break
            except Exception:
                continue

    return HMModuleInfo(
        module_path=module_path,
        example_config=example,
        is_enabled=is_enabled,
    )


def get_darwin_service_info(name: str, repo_root: Path | None = None) -> DarwinServiceInfo | None:
    """Check if a package has a nix-darwin service option.

    Args:
        name: Package name (e.g., "yabai", "skhd")
        repo_root: Path to nix config repo to check if already enabled

    Returns:
        DarwinServiceInfo or None if no service available
    """
    name_lower = NAME_MAPPINGS.get(name.lower(), name.lower())

    if name_lower not in DARWIN_SERVICES:
        return None

    service_path, example = DARWIN_SERVICES[name_lower]

    # Check if already enabled
    is_enabled = False
    if repo_root:
        for nix_file in repo_root.glob("**/*.nix"):
            try:
                content = nix_file.read_text()
                # Look for pattern like: services.yabai.enable = true
                if f"{service_path}.enable" in content and "true" in content:
                    is_enabled = True
                    break
            except Exception:
                continue

    return DarwinServiceInfo(
        service_path=service_path,
        example_config=example,
        is_enabled=is_enabled,
    )


@dataclass
class FlakeHubResult:
    """Result from FlakeHub search."""
    flake_name: str         # e.g., "DeterminateSystems/nuenv"
    description: str
    visibility: str         # "public" or "unlisted"
    version: str | None = None


@typed_retry(stop=stop_after_attempt(3), wait=wait_exponential(multiplier=1, min=2, max=10))
def search_flakehub(name: str) -> list[FlakeHubResult]:
    """Search FlakeHub for packages.

    FlakeHub is a registry of Nix flakes. This searches for flakes
    that might provide the requested package.

    Args:
        name: Package name to search for

    Returns:
        List of matching FlakeHub flakes
    """
    # FlakeHub search API
    encoded_name = urllib.parse.quote(name)
    url = f"https://api.flakehub.com/flakes?q={encoded_name}"

    try:
        req = urllib.request.Request(url, headers={"Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as response:
            data = json.loads(response.read().decode())
    except (urllib.error.URLError, urllib.error.HTTPError, json.JSONDecodeError):
        return []

    results = []
    # The API returns a list directly
    flakes = data if isinstance(data, list) else data.get("flakes", [])

    # Filter to those with the search term in project name or description
    name_lower = name.lower()
    relevant = [
        f for f in flakes
        if name_lower in f.get("project", "").lower()
        or name_lower in (f.get("description") or "").lower()
    ]

    for flake in relevant[:5]:  # Top 5 results
        results.append(FlakeHubResult(
            flake_name=f"{flake.get('org', '')}/{flake.get('project', '')}",
            description=flake.get("description") or "",
            visibility="public",
            version=None,  # Version requires additional API call
        ))

    return results


def get_package_info(
    name: str,
    source_hint: str | None = None,
    flake_lock_path: Path | None = None,
    include_bleeding_edge: bool = True,
) -> list[PackageInfo]:
    """Get info about a package from all relevant sources.

    Args:
        name: Package name
        source_hint: Optional hint for which source to check first
        flake_lock_path: Path to flake.lock for overlay detection
        include_bleeding_edge: Whether to check NUR and flake overlays

    Returns:
        List of PackageInfo from all sources where package exists
    """
    results = []

    # Check nxs
    if source_hint in (None, "nxs", "nix"):
        nix_info = get_nix_package_info(name)
        if nix_info:
            results.append(nix_info)

    # Check if it's a package set (like nerd-fonts)
    if not results:
        set_info = get_package_set_info(name)
        if set_info:
            results.append(set_info)

    # Check flake overlays (neovim-nightly, fenix, etc.)
    if include_bleeding_edge and source_hint in (None, "flake", "overlay", "nightly"):
        overlay_info = get_flake_overlay_info(name, flake_lock_path)
        if overlay_info:
            results.append(overlay_info)

    # Check NUR
    if include_bleeding_edge and source_hint in (None, "nur"):
        nur_info = get_nur_package_info(name)
        if nur_info:
            results.append(nur_info)

    # Check homebrew formula
    if source_hint in (None, "homebrew", "brew", "brews"):
        brew_info = get_homebrew_formula_info(name)
        if brew_info:
            results.append(brew_info)

    # Check homebrew cask
    if source_hint in (None, "cask", "casks"):
        cask_info = get_homebrew_cask_info(name)
        if cask_info:
            results.append(cask_info)

    return results
