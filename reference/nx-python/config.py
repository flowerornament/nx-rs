"""
Repository and config file detection for nx.

Handles finding the nix-config repository root and locating
all configuration files used for package management.

Config files are discovered dynamically by reading # nx: comments
at the top of .nix files.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path

from shared import _read_nx_comment, run_command


@dataclass
class ConfigFiles:
    """Dynamically discovered config file paths.

    Files are discovered by scanning for # nx: comments.
    The 'by_purpose' dict maps keywords to file paths.
    """
    repo_root: Path
    by_purpose: dict[str, Path] = field(default_factory=dict)
    all_files: list[Path] = field(default_factory=list)

    # Convenience accessors with fallbacks
    @property
    def packages(self) -> Path:
        return self._find_by_keywords(["cli tools", "utilities"]) or self.repo_root / "packages" / "nix" / "cli.nix"

    @property
    def services(self) -> Path:
        return self._find_by_keywords(["services", "daemons"]) or self.repo_root / "home" / "services.nix"

    @property
    def darwin(self) -> Path:
        return self._find_by_keywords(["macos system"]) or self.repo_root / "system" / "darwin.nix"

    @property
    def homebrew_brews(self) -> Path:
        return self._find_by_keywords(["formula manifest", "brews"]) or self.repo_root / "packages" / "homebrew" / "brews.nix"

    @property
    def homebrew_casks(self) -> Path:
        return self._find_by_keywords(["cask manifest", "gui apps"]) or self.repo_root / "packages" / "homebrew" / "casks.nix"

    @property
    def homebrew_taps(self) -> Path:
        return self._find_by_keywords(["taps manifest"]) or self.repo_root / "packages" / "homebrew" / "taps.nix"

    @property
    def languages(self) -> Path:
        return self._find_by_keywords(["language", "runtimes", "toolchains"]) or self.repo_root / "packages" / "nix" / "languages.nix"

    @property
    def shell(self) -> Path:
        return self._find_by_keywords(["shell"]) or self.repo_root / "home" / "shell.nix"

    @property
    def editors(self) -> Path:
        return self._find_by_keywords(["editor"]) or self.repo_root / "home" / "editors.nix"

    @property
    def git(self) -> Path:
        return self._find_by_keywords(["git", "version control"]) or self.repo_root / "home" / "git.nix"

    @property
    def terminal(self) -> Path:
        return self._find_by_keywords(["terminal", "multiplexer"]) or self.repo_root / "home" / "terminal.nix"

    def _find_by_keywords(self, keywords: list[str]) -> Path | None:
        """Find a file whose # nx: comment contains any of the keywords."""
        for keyword in keywords:
            keyword_lower = keyword.lower()
            for purpose, path in self.by_purpose.items():
                if keyword_lower in purpose.lower():
                    return path
        return None


def find_repo_root() -> Path:
    """Find the nix-config repository root."""
    # Check environment variable first
    env_root = os.environ.get("B2NIX_REPO_ROOT")
    if env_root:
        return Path(env_root).expanduser().resolve()

    # Check if we're in a git repo
    success, output = run_command(["git", "rev-parse", "--show-toplevel"])
    if success and output:
        git_root = Path(output)
        if (git_root / "flake.nix").exists():
            return git_root

    # Fall back to default location
    default = Path.home() / ".nix-config"
    if default.exists():
        return default

    raise RuntimeError("Could not find nix-config repository")


def get_config_files(repo_root: Path) -> ConfigFiles:
    """Discover config files by scanning for # nx: comments.

    Scans home/**/*.nix, system/**/*.nix, hosts/**/*.nix, and packages/**/*.nix for files
    with # nx: comments describing their purpose.
    """
    by_purpose: dict[str, Path] = {}
    all_files: list[Path] = []

    # Directories to scan
    scan_dirs = [
        repo_root / "home",
        repo_root / "system",
        repo_root / "hosts",
        repo_root / "packages",
    ]

    for dir_path in scan_dirs:
        if not dir_path.exists():
            continue

        for nix_file in dir_path.rglob("*.nix"):
            # Skip default.nix and common.nix
            if nix_file.name in ("default.nix", "common.nix"):
                continue

            all_files.append(nix_file)

            # Read # nx: comment
            purpose = _read_nx_comment(nix_file)
            if purpose:
                by_purpose[purpose] = nix_file

    return ConfigFiles(
        repo_root=repo_root,
        by_purpose=by_purpose,
        all_files=all_files,
    )
