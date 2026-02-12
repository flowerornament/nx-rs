"""
changelog.py - Flake.lock parsing, diffing, and GitHub changelog fetching.

Core module for nx upgrade command.
"""

from __future__ import annotations

import json
import re
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from tenacity import stop_after_attempt, wait_exponential

from retry_utils import typed_retry
from shared import run_command, run_json_command, run_streaming_command

# ═══════════════════════════════════════════════════════════════════════════════
# Dataclasses
# ═══════════════════════════════════════════════════════════════════════════════


@dataclass
class FlakeLockInput:
    """Parsed flake.lock input node."""

    name: str
    owner: str | None  # None for non-GitHub sources
    repo: str | None
    rev: str
    last_modified: int
    source_type: str  # "github", "tarball", "file"
    url: str | None = None  # For tarball sources


@dataclass
class InputChange:
    """A changed input between two flake.lock states."""

    name: str
    owner: str
    repo: str
    old_rev: str
    new_rev: str
    old_modified: int = 0
    new_modified: int = 0
    commit_count: int | None = None
    commits: list[str] = field(default_factory=list)
    releases: list[dict[str, Any]] = field(default_factory=list)


@dataclass
class ChangeInfo:
    """Fetched changelog information for a changed input."""

    input_change: InputChange
    total_commits: int = 0
    commit_messages: list[str] = field(default_factory=list)
    releases: list[dict[str, Any]] = field(default_factory=list)
    error: str | None = None


# ═══════════════════════════════════════════════════════════════════════════════
# Flake.lock Parsing
# ═══════════════════════════════════════════════════════════════════════════════


def parse_flake_lock(path: Path) -> dict[str, FlakeLockInput]:
    """Parse flake.lock and extract root input information.

    Handles:
    - GitHub direct sources (type: github)
    - FlakeHub tarball sources (extract owner/repo from URL)
    - Skips file sources (binary artifacts, no changelog)

    Args:
        path: Path to flake.lock file

    Returns:
        Dict mapping input name to FlakeLockInput
    """
    with open(path) as f:
        lock_data = json.load(f)

    nodes = lock_data.get("nodes", {})
    root_inputs = nodes.get("root", {}).get("inputs", {})

    inputs: dict[str, FlakeLockInput] = {}

    for input_name, node_ref in root_inputs.items():
        # Handle indirection (input points to another node)
        if isinstance(node_ref, list):
            # This is a follows reference, skip for now
            continue

        node = nodes.get(node_ref if isinstance(node_ref, str) else input_name, {})
        locked = node.get("locked", {})

        if not locked:
            continue

        source_type = locked.get("type", "")
        rev = locked.get("rev", "")
        last_modified = locked.get("lastModified", 0)

        owner = None
        repo = None
        url = None

        if source_type == "github":
            owner = locked.get("owner")
            repo = locked.get("repo")
        elif source_type == "tarball":
            url = locked.get("url", "")
            # Extract owner/repo from FlakeHub URL pattern
            # Example: https://api.flakehub.com/f/pinned/NixOS/nxs/...
            match = re.search(r"/f/pinned/([^/]+)/([^/]+)/", url)
            if match:
                owner, repo = match.groups()
        elif source_type == "file":
            # Skip binary file sources (no changelog)
            continue
        else:
            # Unknown source type, try to extract what we can
            owner = locked.get("owner")
            repo = locked.get("repo")

        inputs[input_name] = FlakeLockInput(
            name=input_name,
            owner=owner,
            repo=repo,
            rev=rev,
            last_modified=last_modified,
            source_type=source_type,
            url=url,
        )

    return inputs


def diff_locks(
    old_inputs: dict[str, FlakeLockInput],
    new_inputs: dict[str, FlakeLockInput],
) -> tuple[list[InputChange], list[str], list[str]]:
    """Compare two flake.lock states and find changes.

    Args:
        old_inputs: Parsed inputs from old flake.lock
        new_inputs: Parsed inputs from new flake.lock

    Returns:
        Tuple of (changed, added, removed) where:
        - changed: List of InputChange for modified inputs
        - added: List of input names that were added
        - removed: List of input names that were removed
    """
    changed: list[InputChange] = []
    added: list[str] = []
    removed: list[str] = []

    old_names = set(old_inputs.keys())
    new_names = set(new_inputs.keys())

    # Find added and removed
    added = list(new_names - old_names)
    removed = list(old_names - new_names)

    # Find changed (same name but different rev)
    for name in old_names & new_names:
        old_input = old_inputs[name]
        new_input = new_inputs[name]

        if old_input.rev != new_input.rev:
            # Only track changes for inputs with GitHub info
            if new_input.owner and new_input.repo:
                changed.append(
                    InputChange(
                        name=name,
                        owner=new_input.owner,
                        repo=new_input.repo,
                        old_rev=old_input.rev,
                        new_rev=new_input.rev,
                        old_modified=old_input.last_modified,
                        new_modified=new_input.last_modified,
                    )
                )

    return changed, added, removed


# ═══════════════════════════════════════════════════════════════════════════════
# GitHub API Integration
# ═══════════════════════════════════════════════════════════════════════════════


@typed_retry(stop=stop_after_attempt(3), wait=wait_exponential(multiplier=1, min=2, max=10))
def fetch_compare(owner: str, repo: str, base: str, head: str) -> dict | None:
    """Fetch comparison between two commits via GitHub API.

    Args:
        owner: Repository owner
        repo: Repository name
        base: Base commit SHA (old)
        head: Head commit SHA (new)

    Returns:
        Comparison data from GitHub API, or None on failure
    """
    endpoint = f"repos/{owner}/{repo}/compare/{base[:7]}...{head[:7]}"
    success, data = run_json_command(["gh", "api", endpoint], timeout=30)
    return data if success else None


@typed_retry(stop=stop_after_attempt(3), wait=wait_exponential(multiplier=1, min=2, max=10))
def fetch_releases(owner: str, repo: str, per_page: int = 20) -> list[dict]:
    """Fetch recent releases from GitHub API.

    Args:
        owner: Repository owner
        repo: Repository name
        per_page: Number of releases to fetch

    Returns:
        List of release objects from GitHub API
    """
    endpoint = f"repos/{owner}/{repo}/releases?per_page={per_page}"
    success, data = run_json_command(["gh", "api", endpoint], timeout=30)
    return data if success and isinstance(data, list) else []


def fetch_change_info(change: InputChange) -> ChangeInfo:
    """Fetch changelog information for a changed input.

    Uses different strategies based on the input:
    - nxs: Fetch releases only (too many commits)
    - Others: Fetch commit comparison

    Args:
        change: The InputChange to fetch info for

    Returns:
        ChangeInfo with commits and/or releases
    """
    info = ChangeInfo(input_change=change)

    # For nxs, just fetch releases (commits are too numerous)
    if change.name == "nxs" or change.repo == "nxs":
        releases = fetch_releases(change.owner, change.repo)
        info.releases = releases
        info.total_commits = 0  # Don't bother counting
        return info

    # For other repos, fetch comparison
    compare_data = fetch_compare(
        change.owner, change.repo, change.old_rev, change.new_rev
    )

    if compare_data:
        commits = compare_data.get("commits", [])
        info.total_commits = len(commits)
        change.commit_count = len(commits)

        # Extract commit messages (limit to avoid huge lists)
        info.commit_messages = [
            c.get("commit", {}).get("message", "").split("\n")[0]
            for c in commits[:50]
        ]

        # Also fetch releases for context
        releases = fetch_releases(change.owner, change.repo, per_page=10)
        # Filter to releases between the two commits (by date)
        info.releases = releases[:5]  # Just take recent ones for now
    else:
        info.error = "Failed to fetch comparison from GitHub"

    return info


def fetch_all_changes(changes: list[InputChange], max_workers: int = 4) -> list[ChangeInfo]:
    """Fetch changelog info for all changes in parallel.

    Args:
        changes: List of InputChange to fetch
        max_workers: Maximum parallel requests

    Returns:
        List of ChangeInfo for each input
    """
    results: list[ChangeInfo] = []

    with ThreadPoolExecutor(max_workers=max_workers) as executor:
        futures = {executor.submit(fetch_change_info, c): c for c in changes}

        for future in as_completed(futures):
            try:
                info = future.result()
                results.append(info)
            except Exception as e:
                change = futures[future]
                results.append(
                    ChangeInfo(input_change=change, error=str(e))
                )

    # Sort by input name for consistent output
    results.sort(key=lambda x: x.input_change.name)
    return results


# ═══════════════════════════════════════════════════════════════════════════════
# Nix Flake Update
# ═══════════════════════════════════════════════════════════════════════════════


def get_github_token() -> str:
    """Get GitHub token from gh CLI."""
    success, token = run_command(["gh", "auth", "token"], timeout=10)
    return token.strip() if success else ""


def _clear_fetcher_cache() -> bool:
    """Clear Nix fetcher cache to fix corruption issues.

    Returns:
        True if cache was cleared, False if it didn't exist
    """
    cache_path = Path.home() / ".cache" / "nix" / "fetcher-cache-v4.sqlite"
    if cache_path.exists():
        cache_path.unlink()
        return True
    return False


def _is_cache_corruption_error(output: str) -> bool:
    """Check if error output indicates a cache corruption issue."""
    corruption_indicators = [
        "failed to insert entry: invalid object specified",
        "error: adding a file to a tree builder",
    ]
    return any(indicator in output for indicator in corruption_indicators)


def stream_nix_update(
    repo_root: Path,
    printer: Any = None,
    extra_args: list[str] | None = None,
) -> tuple[bool, str]:
    """Run nix flake update with indented streaming output.

    Automatically retries once if cache corruption is detected.

    Args:
        repo_root: Path to the nix-config repository
        printer: Optional Printer instance for output

    Returns:
        Tuple of (success, output)
    """
    # Get GitHub token for authenticated requests
    token = get_github_token()

    cmd = ["nix", "flake", "update"] + (extra_args or [])
    if token:
        cmd.extend(["--option", "access-tokens", f"github.com={token}"])

    max_attempts = 2
    for attempt in range(max_attempts):
        if printer:
            if attempt == 0:
                printer.action("Updating flake inputs")
            else:
                printer.action("Retrying flake update")

        returncode, output = run_streaming_command(
            cmd,
            cwd=repo_root,
            printer=printer,
            indent="  ",
            skip_blank_lines=True,
        )

        if returncode == 0:
            return True, output

        # Check for cache corruption and retry
        if attempt == 0 and _is_cache_corruption_error(output):
            if printer:
                printer.warn("Nix cache corruption detected, clearing cache")
            if _clear_fetcher_cache():
                continue  # Retry

        return False, output

    return False, ""  # Should never reach here


# ═══════════════════════════════════════════════════════════════════════════════
# Utility Functions
# ═══════════════════════════════════════════════════════════════════════════════


def load_flake_lock(repo_root: Path) -> dict[str, FlakeLockInput]:
    """Load and parse flake.lock from repository.

    Args:
        repo_root: Path to the nix-config repository

    Returns:
        Parsed flake.lock inputs
    """
    lock_path = repo_root / "flake.lock"
    if not lock_path.exists():
        return {}
    return parse_flake_lock(lock_path)


def short_rev(rev: str) -> str:
    """Shorten a git revision to 7 characters."""
    return rev[:7] if rev else ""
