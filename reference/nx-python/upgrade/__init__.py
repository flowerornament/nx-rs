"""
upgrade - Nix flake update with changelog summaries.

Subcommands:
- nx update: Just run nix flake update
- nx switch: Just run darwin-rebuild switch
- nx upgrade: Full workflow with AI changelogs
"""

from .brew_outdated import (
    BrewChangeInfo,
    BrewOutdated,
    enrich_package_info,
    fetch_all_brew_changelogs,
    get_outdated,
)
from .changelog import (
    ChangeInfo,
    InputChange,
    diff_locks,
    fetch_all_changes,
    load_flake_lock,
    short_rev,
    stream_nix_update,
)
from .summarizer import (
    generate_commit_message,
    summarize_brew_change,
    summarize_change,
)

__all__ = [
    "BrewChangeInfo",
    "BrewOutdated",
    "ChangeInfo",
    "InputChange",
    "diff_locks",
    "enrich_package_info",
    "fetch_all_brew_changelogs",
    "fetch_all_changes",
    "generate_commit_message",
    "get_outdated",
    "load_flake_lock",
    "short_rev",
    "stream_nix_update",
    "summarize_brew_change",
    "summarize_change",
]
