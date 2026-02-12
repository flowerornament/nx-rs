"""
summarizer.py - AI-powered changelog summarization.

Uses smart routing between Claude (detailed) and Codex (fast) based on
the input type and number of changes.
"""

from __future__ import annotations

from ai_helpers import run_claude, run_codex

from .brew_outdated import BrewChangeInfo, BrewOutdated
from .changelog import ChangeInfo, InputChange

# ═══════════════════════════════════════════════════════════════════════════════
# Smart Routing
# ═══════════════════════════════════════════════════════════════════════════════


# Key inputs that get detailed Claude summaries
KEY_INPUTS = {"nxs", "home-manager", "nix-darwin"}


def should_use_detailed_summary(input_name: str, commit_count: int) -> bool:
    """Determine if an input should use Claude for detailed summary.

    Uses Claude for:
    - Key inputs (nxs, home-manager, nix-darwin)
    - Large changes (>50 commits)

    Uses Codex for:
    - Smaller repos with few commits
    - Quick categorization

    Args:
        input_name: Name of the flake input
        commit_count: Number of commits in the change

    Returns:
        True if Claude should be used, False for Codex
    """
    return input_name in KEY_INPUTS or commit_count > 50


# ═══════════════════════════════════════════════════════════════════════════════
# AI Summarization
# ═══════════════════════════════════════════════════════════════════════════════


def summarize_with_codex(
    commits: list[str],
    input_name: str,
) -> str | None:
    """Quick commit categorization using Codex.

    Args:
        commits: List of commit message first lines
        input_name: Name of the input for context

    Returns:
        Summary string or None on failure
    """
    if not commits:
        return None

    # Limit commits to avoid huge prompts
    commit_text = "\n".join(commits[:30])

    prompt = f"""Summarize these commits for {input_name} in 1-2 sentences.
Focus on: new features, breaking changes, bug fixes, security patches.
Be concise and specific.

Commits:
{commit_text}

Summary:"""

    success, response = run_codex(prompt, timeout=30, reasoning="low")
    if success and response:
        # Clean up response
        return response.strip().split("\n")[0][:200]
    return None


def summarize_with_claude(
    input_name: str,
    commits: list[str],
    releases: list[dict],
) -> str | None:
    """Detailed changelog summary using Claude.

    Args:
        input_name: Name of the input
        commits: List of commit messages
        releases: List of release objects from GitHub

    Returns:
        Summary string or None on failure
    """
    # Build context from commits and releases
    context_parts = []

    if releases:
        release_text = []
        for rel in releases[:5]:
            tag = rel.get("tag_name", "")
            name = rel.get("name", "")
            body = rel.get("body", "")[:300]
            if tag:
                release_text.append(f"Release {tag}: {name}\n{body}")
        if release_text:
            context_parts.append("Recent releases:\n" + "\n\n".join(release_text))

    if commits:
        context_parts.append("Recent commits:\n" + "\n".join(commits[:20]))

    if not context_parts:
        return None

    context = "\n\n".join(context_parts)

    prompt = f"""Summarize the key changes in {input_name} in 2-3 sentences.

Focus on:
- New features or capabilities users will notice
- Breaking changes or things to watch out for
- Security patches or important bug fixes
- Notable version updates (if applicable)

Skip minor refactors, dependency bumps, and internal changes.

{context}

Summary:"""

    success, response = run_claude(prompt, timeout=45)
    if success and response:
        # Clean up response
        lines = response.strip().split("\n")
        # Take first 2-3 non-empty lines
        summary_lines = [line for line in lines if line.strip()][:3]
        return " ".join(summary_lines)[:400]
    return None


def summarize_change(change_info: ChangeInfo) -> str | None:
    """Summarize a single flake input change.

    Uses smart routing to choose between Claude and Codex.

    Args:
        change_info: ChangeInfo with commits and releases

    Returns:
        Summary string or None
    """
    change = change_info.input_change
    commits = change_info.commit_messages
    releases = change_info.releases

    # Check if we have anything to summarize
    if not commits and not releases:
        return None

    # Choose summarization strategy
    commit_count = len(commits) if commits else 0

    if should_use_detailed_summary(change.name, commit_count):
        return summarize_with_claude(change.name, commits, releases)
    else:
        return summarize_with_codex(commits, change.name)


def summarize_brew_change(change_info: BrewChangeInfo) -> str | None:
    """Summarize a Homebrew package change.

    Args:
        change_info: BrewChangeInfo with releases

    Returns:
        Summary string or None
    """
    if not change_info.releases:
        return None

    pkg = change_info.package

    # Build release context
    release_text = []
    for rel in change_info.releases[:3]:
        tag = rel.get("tag_name", "")
        body = rel.get("body", "")[:200]
        if tag:
            release_text.append(f"{tag}: {body}")

    if not release_text:
        return None

    prompt = f"""Summarize these release notes for {pkg.name} ({pkg.installed_version} → {pkg.current_version}) in 1 sentence.
Focus on key changes users should know about.

{chr(10).join(release_text)}

Summary:"""

    success, response = run_codex(prompt, timeout=30, reasoning="low")
    if success and response:
        return response.strip().split("\n")[0][:150]
    return None


# ═══════════════════════════════════════════════════════════════════════════════
# Commit Message Generation
# ═══════════════════════════════════════════════════════════════════════════════


def generate_commit_message(
    flake_changes: list[InputChange],
    brew_updates: list[BrewOutdated],
) -> str:
    """Generate a git commit message summarizing all changes.

    Args:
        flake_changes: List of changed flake inputs
        brew_updates: List of updated Homebrew packages

    Returns:
        Commit message string
    """
    parts = []

    # Flake input changes
    if flake_changes:
        flake_names = [c.name for c in flake_changes[:5]]
        if len(flake_changes) > 5:
            flake_names.append(f"+{len(flake_changes) - 5} more")
        parts.append(f"flake ({', '.join(flake_names)})")

    # Brew updates
    if brew_updates:
        brew_names = [p.name for p in brew_updates[:3]]
        if len(brew_updates) > 3:
            brew_names.append(f"+{len(brew_updates) - 3} more")
        parts.append(f"brew ({', '.join(brew_names)})")

    if parts:
        return f"Update {' + '.join(parts)}"
    else:
        return "Update flake inputs"
