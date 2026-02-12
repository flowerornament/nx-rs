"""
installed.py - installed package source detection for nx.
"""

from __future__ import annotations

from pathlib import Path

from config import get_config_files
from finder import find_all_packages
from sources import check_overlay_active


def detect_installed_source(
    location: str,
    package_name: str,
    repo_root: Path,
) -> tuple[str, str | None]:
    """Detect source of installed package from its location.

    Uses file-path-first routing with package-list lookup for darwin.nix.

    Args:
        location: File location string (e.g., "packages/nix/cli.nix:42")
        package_name: Name of the package to look up
        repo_root: Path to the nix-config repository root

    Returns:
        Tuple of (source_type, overlay_name or None).
        source_type: "nxs", "homebrew", "cask", "mas", or "flake:<name>"
    """
    file_path = location.rsplit(":", 1)[0]

    # Handle both absolute and relative paths
    if file_path.startswith(str(repo_root)):
        rel_path = file_path.replace(str(repo_root) + "/", "")
    else:
        rel_path = file_path

    # darwin.nix needs secondary lookup for brews/casks/mas
    if rel_path == "system/darwin.nix":
        config_files = get_config_files(repo_root)
        packages = find_all_packages(config_files)
        if package_name in packages.get("casks", []):
            return "cask", None
        if package_name in packages.get("brews", []):
            return "homebrew", None
        if package_name in packages.get("mas", []):
            return "mas", None

    # Non-darwin package locations are nxs (with optional overlay)
    overlay = check_overlay_active(package_name, repo_root)
    if overlay:
        return f"flake:{overlay}", overlay
    return "nxs", None
