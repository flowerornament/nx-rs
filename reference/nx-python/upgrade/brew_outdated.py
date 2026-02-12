"""
brew_outdated.py - Homebrew outdated detection and changelog fetching.

Handles checking for outdated Homebrew packages and fetching their changelogs.
Used by nx upgrade command.
"""

from __future__ import annotations

import json
import re
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from typing import Any

from shared import run_command, run_json_command

# ═══════════════════════════════════════════════════════════════════════════════
# Dataclasses
# ═══════════════════════════════════════════════════════════════════════════════


@dataclass
class BrewOutdated:
    """An outdated Homebrew package."""

    name: str
    installed_version: str
    current_version: str
    is_cask: bool
    homepage: str | None = None
    description: str | None = None


@dataclass
class BrewChangeInfo:
    """Changelog information for an outdated package."""

    package: BrewOutdated
    releases: list[dict[str, Any]] = field(default_factory=list)
    release_notes: str | None = None
    error: str | None = None


# ═══════════════════════════════════════════════════════════════════════════════
# Homebrew Outdated Detection
# ═══════════════════════════════════════════════════════════════════════════════


def get_outdated() -> list[BrewOutdated]:
    """Get list of outdated Homebrew packages.

    Returns:
        List of BrewOutdated packages
    """
    success, output = run_command(["brew", "outdated", "--json"], timeout=60)

    if not success or not output:
        return []

    try:
        data = json.loads(output)
    except json.JSONDecodeError:
        return []

    outdated: list[BrewOutdated] = []

    # Parse formulae
    for formula in data.get("formulae", []):
        name = formula.get("name", "")
        installed = formula.get("installed_versions", [""])[0]
        current = formula.get("current_version", "")

        if name and installed and current:
            outdated.append(
                BrewOutdated(
                    name=name,
                    installed_version=installed,
                    current_version=current,
                    is_cask=False,
                )
            )

    # Parse casks
    for cask in data.get("casks", []):
        name = cask.get("name", "")
        installed = cask.get("installed_versions", "")
        current = cask.get("current_version", "")

        if name and installed and current:
            outdated.append(
                BrewOutdated(
                    name=name,
                    installed_version=installed,
                    current_version=current,
                    is_cask=True,
                )
            )

    return outdated


def enrich_package_info(packages: list[BrewOutdated]) -> list[BrewOutdated]:
    """Enrich packages with homepage and description from brew info.

    Args:
        packages: List of outdated packages

    Returns:
        Same packages with homepage/description filled in
    """
    if not packages:
        return packages

    # Batch fetch info for all packages
    formulae = [p.name for p in packages if not p.is_cask]
    casks = [p.name for p in packages if p.is_cask]

    formula_info = {}
    cask_info = {}

    # Fetch formula info
    if formulae:
        success, output = run_command(
            ["brew", "info", "--json=v2", *formulae], timeout=60
        )
        if success and output:
            try:
                data = json.loads(output)
                for f in data.get("formulae", []):
                    name = f.get("name", "")
                    formula_info[name] = {
                        "homepage": f.get("homepage"),
                        "desc": f.get("desc"),
                    }
            except json.JSONDecodeError:
                pass

    # Fetch cask info
    if casks:
        success, output = run_command(
            ["brew", "info", "--json=v2", "--cask", *casks], timeout=60
        )
        if success and output:
            try:
                data = json.loads(output)
                for c in data.get("casks", []):
                    name = c.get("token", "")
                    cask_info[name] = {
                        "homepage": c.get("homepage"),
                        "desc": c.get("desc"),
                    }
            except json.JSONDecodeError:
                pass

    # Enrich packages
    for pkg in packages:
        if pkg.is_cask:
            info = cask_info.get(pkg.name, {})
        else:
            info = formula_info.get(pkg.name, {})

        pkg.homepage = info.get("homepage")
        pkg.description = info.get("desc")

    return packages


# ═══════════════════════════════════════════════════════════════════════════════
# Changelog Fetching
# ═══════════════════════════════════════════════════════════════════════════════


def extract_github_info(homepage: str) -> tuple[str, str] | None:
    """Extract owner/repo from a GitHub URL.

    Args:
        homepage: Package homepage URL

    Returns:
        Tuple of (owner, repo) or None if not a GitHub URL
    """
    if not homepage:
        return None

    # Match github.com URLs
    match = re.match(r"https?://github\.com/([^/]+)/([^/]+)/?", homepage)
    if match:
        owner, repo = match.groups()
        # Clean up repo name (remove .git suffix)
        repo = repo.rstrip(".git")
        return owner, repo

    return None


def fetch_github_releases(
    owner: str, repo: str, per_page: int = 10
) -> list[dict[str, Any]]:
    """Fetch releases from GitHub API.

    Args:
        owner: Repository owner
        repo: Repository name
        per_page: Number of releases to fetch

    Returns:
        List of release objects
    """
    endpoint = f"repos/{owner}/{repo}/releases?per_page={per_page}"
    success, data = run_json_command(["gh", "api", endpoint], timeout=30)
    return data if success and isinstance(data, list) else []


def filter_releases_by_version(
    releases: list[dict[str, Any]],
    installed_version: str,
    current_version: str,
) -> list[dict[str, Any]]:
    """Filter releases to those between installed and current version.

    Args:
        releases: List of GitHub releases
        installed_version: Currently installed version
        current_version: Latest available version

    Returns:
        Filtered list of releases
    """
    # Normalize version strings for comparison
    def normalize(v: str) -> str:
        return re.sub(r"^v", "", v.strip())

    installed_norm = normalize(installed_version)
    current_norm = normalize(current_version)

    filtered = []
    found_current = False
    found_installed = False

    for release in releases:
        tag = normalize(release.get("tag_name", ""))

        # Start collecting after finding current version
        if tag == current_norm:
            found_current = True

        if found_current and not found_installed:
            filtered.append(release)

        # Stop after finding installed version
        if tag == installed_norm:
            found_installed = True
            break

    return filtered


def fetch_brew_changelog(pkg: BrewOutdated) -> BrewChangeInfo:
    """Fetch changelog for an outdated Homebrew package.

    Args:
        pkg: The outdated package

    Returns:
        BrewChangeInfo with releases or error
    """
    info = BrewChangeInfo(package=pkg)

    # Check if homepage is GitHub
    if not pkg.homepage:
        return info
    github_info = extract_github_info(pkg.homepage)
    if not github_info:
        # Not a GitHub project, just return homepage for manual review
        return info

    owner, repo = github_info

    # Fetch releases
    releases = fetch_github_releases(owner, repo)
    if not releases:
        info.error = "No releases found"
        return info

    # Filter to relevant releases
    relevant = filter_releases_by_version(
        releases, pkg.installed_version, pkg.current_version
    )

    info.releases = relevant if relevant else releases[:3]

    # Extract combined release notes
    notes = []
    for rel in info.releases[:5]:
        tag = rel.get("tag_name", "")
        body = rel.get("body", "")
        if tag:
            notes.append(f"## {tag}")
        if body:
            # Truncate long release notes
            notes.append(body[:500] if len(body) > 500 else body)

    info.release_notes = "\n\n".join(notes) if notes else None

    return info


def fetch_all_brew_changelogs(
    packages: list[BrewOutdated], max_workers: int = 4
) -> list[BrewChangeInfo]:
    """Fetch changelogs for all outdated packages in parallel.

    Args:
        packages: List of outdated packages
        max_workers: Maximum parallel requests

    Returns:
        List of BrewChangeInfo for each package
    """
    results: list[BrewChangeInfo] = []

    with ThreadPoolExecutor(max_workers=max_workers) as executor:
        futures = {executor.submit(fetch_brew_changelog, p): p for p in packages}

        for future in as_completed(futures):
            try:
                info = future.result()
                results.append(info)
            except Exception as e:
                pkg = futures[future]
                results.append(BrewChangeInfo(package=pkg, error=str(e)))

    # Sort by package name
    results.sort(key=lambda x: x.package.name)
    return results
