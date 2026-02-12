"""
finder.py - Package finding and duplicate detection for nx.

Functions for locating where packages are configured in the nix-darwin setup.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from config import ConfigFiles
from shared import NAME_MAPPINGS


@dataclass
class FinderIndex:
    """Indexed snapshot of Nix files and parsed package metadata."""

    signatures: dict[Path, tuple[int, int]]
    file_lines: dict[Path, list[str]]
    packages_by_source: dict[str, tuple[str, ...]]
    location_hints: dict[str, str]


_FINDER_INDEX_CACHE: dict[Path, FinderIndex] = {}
_FINDER_INDEX_METRICS: dict[str, int] = {"build_count": 0}


def _build_package_patterns(pkg_name: str) -> list[str]:
    """Build regex patterns to match package declarations for a given name."""
    escaped = re.escape(pkg_name)
    return [
        # In package lists: just the name on its own line or with pkgs. prefix
        rf"^\s+{escaped}\s*(#.*)?$",                    # Simple: ripgrep or ripgrep # comment
        rf"^\s+pkgs\.{escaped}\s",                      # pkgs.ripgrep
        rf'^\s+"{escaped}"',                            # "ripgrep" (in homebrew lists)
        # Home-manager modules (programs.X and services.X)
        rf"^\s*programs\.{escaped}\.enable",            # programs.git.enable = true
        rf"^\s*programs\.{escaped}\s*=",                # programs.git = {
        rf"^\s*services\.{escaped}\.enable",            # services.mpd.enable = true
        rf"^\s*services\.{escaped}\s*=",                # services.mpd = {
        # Launchd agents (nix-darwin)
        rf"^\s*launchd\.(?:user\.)?agents\.{escaped}\s*=",  # launchd.agents.X = {
    ]


def _scan_files_for_patterns(
    file_lines: dict[Path, list[str]],
    patterns: list[str],
    skip_alias_name: str,
) -> str | None:
    """Scan nix files for matching patterns, returning 'file:line' or None."""
    for file_path, lines in file_lines.items():
        for line_num, line in enumerate(lines, 1):
            # Skip comments
            if line.strip().startswith("#"):
                continue

            # Skip shell aliases (e.g., vim = "nvim")
            if "=" in line:
                parts = line.split("=", 1)
                if len(parts) > 1 and f'"{skip_alias_name}"' in parts[1]:
                    continue

            # Check for package declaration patterns (case-insensitive)
            for pattern in patterns:
                if re.search(pattern, line, re.IGNORECASE):
                    return f"{file_path}:{line_num}"

    return None


def find_package(name: str, config_files: ConfigFiles) -> str | None:
    """Find where a package is already configured by scanning all .nix files.

    Looks for actual package declarations, not aliases or config references.
    Performs case-insensitive matching to catch variants like RIPGREP vs ripgrep.

    Args:
        name: Package name to search for
        config_files: ConfigFiles with paths to nix configuration

    Returns:
        Location string as "file_path:line_num" or None if not found
    """
    # Apply name mapping (e.g., nvim -> neovim)
    mapped_name = NAME_MAPPINGS.get(name.lower(), NAME_MAPPINGS.get(name, name))

    index = _get_finder_index(config_files)

    # Search with mapped name first
    hint = _hint_location(mapped_name, index)
    if hint:
        return hint

    patterns = _build_package_patterns(mapped_name)
    result = _scan_files_for_patterns(index.file_lines, patterns, name)
    if result:
        return result

    # Also search with original name if different from mapped
    if mapped_name.lower() != name.lower():
        hint = _hint_location(name, index)
        if hint:
            return hint
        patterns = _build_package_patterns(name)
        result = _scan_files_for_patterns(index.file_lines, patterns, name)
        if result:
            return result

    return None


def find_all_packages(config_files: ConfigFiles) -> dict[str, list[str]]:
    """Find all packages by scanning all .nix files for known patterns.

    Searches for:
    - home.packages / environment.systemPackages -> nxs
    - homebrew.brews -> brews
    - homebrew.casks -> casks
    - homebrew.masApps -> mas
    - launchd.agents.* / launchd.user.agents.* -> services

    Args:
        config_files: ConfigFiles with paths to nix configuration

    Returns:
        Dict mapping source names to lists of package names
    """
    index = _get_finder_index(config_files)
    return {key: list(values) for key, values in index.packages_by_source.items()}


def _collect_nix_files(config_files: ConfigFiles) -> list[Path]:
    repo_root = config_files.repo_root
    nix_files: set[Path] = set(config_files.all_files)
    for dir_path in (repo_root / "home", repo_root / "system", repo_root / "hosts", repo_root / "packages"):
        if not dir_path.exists():
            continue
        nix_files.update(dir_path.rglob("*.nix"))
    return sorted(nix_files)


def _scan_nix_content(
    nix_file: Path,
    content: str,
    result: dict[str, list[str]],
) -> None:
    _collect_nixpkgs_packages(content, result)
    _collect_homebrew_brews(nix_file, content, result)
    _collect_homebrew_casks(nix_file, content, result)
    _collect_mas_apps(content, result)
    _collect_launchd_services(content, result)


def _collect_nixpkgs_packages(content: str, result: dict[str, list[str]]) -> None:
    patterns = [
        r"home\.packages\s*=\s*(?:with\s+\w+;\s*)?\[(.*?)\];",
        r"environment\.systemPackages\s*=\s*(?:with\s+\w+;\s*)?\[(.*?)\];",
    ]
    for pattern in patterns:
        for match in re.finditer(pattern, content, re.DOTALL):
            packages = _extract_nix_list_items(match.group(1))
            result["nxs"].extend(packages)


def _collect_homebrew_brews(nix_file: Path, content: str, result: dict[str, list[str]]) -> None:
    # Dedicated manifest file: packages/homebrew/brews.nix
    if nix_file.name == "brews.nix" and nix_file.parent.name == "homebrew":
        for item in re.findall(r'"([^"]+)"', content):
            if item not in result["brews"]:
                result["brews"].append(item)
        return

    for match in re.finditer(r"(?:homebrew\.)?brews\s*=\s*\[(.*?)\];", content, re.DOTALL):
        for item in re.findall(r'"([^"]+)"', match.group(1)):
            if item not in result["brews"]:
                result["brews"].append(item)


def _collect_homebrew_casks(nix_file: Path, content: str, result: dict[str, list[str]]) -> None:
    # Dedicated manifest file: packages/homebrew/casks.nix
    if nix_file.name == "casks.nix" and nix_file.parent.name == "homebrew":
        for item in re.findall(r'"([^"]+)"', content):
            if item not in result["casks"]:
                result["casks"].append(item)
        return

    for match in re.finditer(r"(?:homebrew\.)?casks\s*=\s*\[(.*?)\];", content, re.DOTALL):
        for item in re.findall(r'"([^"]+)"', match.group(1)):
            if item not in result["casks"]:
                result["casks"].append(item)


def _collect_mas_apps(content: str, result: dict[str, list[str]]) -> None:
    for match in re.finditer(r"(?:homebrew\.)?masApps\s*=\s*\{(.*?)\};", content, re.DOTALL):
        for item in re.findall(r'"([^"]+)"', match.group(1)):
            if item not in result["mas"]:
                result["mas"].append(item)


def _collect_launchd_services(content: str, result: dict[str, list[str]]) -> None:
    for match in re.findall(r"launchd\.(?:user\.)?agents\.([a-zA-Z0-9_-]+)", content):
        if match not in result["services"]:
            result["services"].append(match)


def _extract_nix_list_items(list_content: str) -> list[str]:
    """Extract package names from a Nix list expression.

    Handles:
    - Simple names: ripgrep, fd, bat
    - Dotted names: nerd-fonts.hack, python3Packages.weasyprint
    - Skips: inputs.*, comments, Nix keywords

    Args:
        list_content: String content between [ and ] in a Nix list

    Returns:
        List of package name strings
    """
    packages = []
    nix_keywords = {"with", "pkgs", "lib", "config", "in", "let", "inherit", "rec"}

    for raw_line in list_content.split("\n"):
        line = raw_line.strip()

        # Skip empty, comments, brackets
        if not line or line.startswith("#") or line in ["[", "]", "{"]:
            continue

        # Skip flake inputs (inputs.foo.bar)
        if line.startswith("inputs."):
            continue

        # Skip ++ concatenation lines
        if line.startswith("++"):
            continue

        # Extract the package name (handles dotted paths)
        match = re.match(r"^([a-zA-Z][a-zA-Z0-9_.-]*)", line)
        if match:
            pkg = match.group(1)
            # Skip Nix keywords
            if pkg.lower() not in nix_keywords and pkg not in packages:
                packages.append(pkg)

    return packages


def _file_signatures(files: list[Path]) -> dict[Path, tuple[int, int]]:
    signatures: dict[Path, tuple[int, int]] = {}
    for path in files:
        try:
            stat = path.stat()
        except OSError:
            continue
        signatures[path] = (stat.st_mtime_ns, stat.st_size)
    return signatures


def _dedupe_lists(result: dict[str, list[str]]) -> dict[str, tuple[str, ...]]:
    deduped: dict[str, tuple[str, ...]] = {}
    for key, values in result.items():
        seen: set[str] = set()
        out: list[str] = []
        for value in values:
            if value in seen:
                continue
            seen.add(value)
            out.append(value)
        deduped[key] = tuple(out)
    return deduped


def _index_location_hints(file_lines: dict[Path, list[str]]) -> dict[str, str]:
    hints: dict[str, str] = {}
    simple_pkg = re.compile(r"^\s+([A-Za-z][A-Za-z0-9_.-]*)\s*(#.*)?$")
    pkgs_attr = re.compile(r"^\s+pkgs\.([A-Za-z][A-Za-z0-9_.-]*)\b")
    quoted_item = re.compile(r'^\s+"([^"]+)"')
    programs = re.compile(r"^\s*programs\.([A-Za-z][A-Za-z0-9_.-]*)(?:\.enable|\s*=)")
    services = re.compile(r"^\s*services\.([A-Za-z][A-Za-z0-9_.-]*)(?:\.enable|\s*=)")
    launchd = re.compile(r"^\s*launchd\.(?:user\.)?agents\.([A-Za-z][A-Za-z0-9_.-]*)\s*=")

    for file_path, lines in file_lines.items():
        for line_num, line in enumerate(lines, 1):
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue

            if "=" in line:
                parts = line.split("=", 1)
                if len(parts) > 1 and '"' in parts[1]:
                    # Likely alias assignment like vim = "nvim"; not a package declaration.
                    continue

            location = f"{file_path}:{line_num}"
            for pattern in (simple_pkg, pkgs_attr, quoted_item, programs, services, launchd):
                match = pattern.search(line)
                if not match:
                    continue
                token = match.group(1).lower()
                hints.setdefault(token, location)
                break

    return hints


def _build_finder_index(config_files: ConfigFiles, files: list[Path]) -> FinderIndex:
    _FINDER_INDEX_METRICS["build_count"] += 1

    signatures = _file_signatures(files)
    file_lines: dict[Path, list[str]] = {}
    parsed: dict[str, list[str]] = {
        "nxs": [],
        "brews": [],
        "casks": [],
        "mas": [],
        "services": [],
    }

    for nix_file in files:
        try:
            content = nix_file.read_text()
        except Exception:
            continue
        file_lines[nix_file] = content.split("\n")
        _scan_nix_content(nix_file, content, parsed)

    return FinderIndex(
        signatures=signatures,
        file_lines=file_lines,
        packages_by_source=_dedupe_lists(parsed),
        location_hints=_index_location_hints(file_lines),
    )


def _is_index_current(index: FinderIndex, files: list[Path]) -> bool:
    current_signatures = _file_signatures(files)
    if set(index.signatures) != set(current_signatures):
        return False

    for path, signature in current_signatures.items():
        if index.signatures.get(path) != signature:
            return False

    return True


def _get_finder_index(config_files: ConfigFiles) -> FinderIndex:
    repo_root = config_files.repo_root.resolve()
    files = _collect_nix_files(config_files)
    cached = _FINDER_INDEX_CACHE.get(repo_root)

    if cached and _is_index_current(cached, files):
        return cached

    index = _build_finder_index(config_files, files)
    _FINDER_INDEX_CACHE[repo_root] = index
    return index


def _hint_location(name: str, index: FinderIndex) -> str | None:
    mapped = NAME_MAPPINGS.get(name.lower(), NAME_MAPPINGS.get(name, name))
    for candidate in (name, mapped):
        key = candidate.lower()
        if key in index.location_hints:
            return index.location_hints[key]
    return None


def _reset_finder_index_cache() -> None:
    """Testing helper: clear in-memory finder index cache."""
    _FINDER_INDEX_CACHE.clear()


def _finder_index_build_count() -> int:
    """Testing helper: return how many times the index was rebuilt."""
    return _FINDER_INDEX_METRICS["build_count"]


def find_package_fuzzy(name: str, config_files: ConfigFiles) -> tuple[str | None, str | None]:
    """Find a package with fuzzy matching.

    Tries in order:
    1. Exact match
    2. Case-insensitive prefix match (lua -> lua5_4)
    3. Case-insensitive substring match (rg -> ripgrep)

    Args:
        name: Package name or partial name to search for
        config_files: ConfigFiles with paths to nix configuration

    Returns:
        Tuple of (matched_name, location) or (None, None) if not found
    """
    # Try exact match first
    location = find_package(name, config_files)
    if location:
        return name, location

    # Get all installed packages for fuzzy matching
    all_packages = find_all_packages(config_files)
    all_names = []
    for source_pkgs in all_packages.values():
        all_names.extend(source_pkgs)

    name_lower = name.lower()

    # Try prefix match (lua -> lua5_4, python -> python3)
    for pkg in all_names:
        if pkg.lower().startswith(name_lower):
            location = find_package(pkg, config_files)
            if location:
                return pkg, location

    # Try substring match (rg -> ripgrep)
    for pkg in all_names:
        if name_lower in pkg.lower():
            location = find_package(pkg, config_files)
            if location:
                return pkg, location

    return None, None
