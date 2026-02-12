"""
ai_helpers.py - AI/LLM helper functions for nx.
"""

from __future__ import annotations

import re
from pathlib import Path

from shared import _read_nx_comment, detect_language_package, run_command


def run_claude(
    prompt: str,
    cwd: Path | None = None,
    timeout: int = 60,
) -> tuple[bool, str]:
    """Run Claude CLI with --print flag."""
    cmd = ["claude", "--print", "-p", prompt]
    return run_command(cmd, cwd=cwd, timeout=timeout)


def run_codex(
    prompt: str,
    cwd: Path | None = None,
    timeout: int = 60,
    reasoning: str = "low",
) -> tuple[bool, str]:
    """Run Codex CLI in non-interactive mode."""
    cmd = [
        "codex", "exec",
        "-m", "gpt-5.2-codex",
        "-c", f'model_reasoning_effort="{reasoning}"',
        "--full-auto",
        prompt,
    ]
    return run_command(cmd, cwd=cwd, timeout=timeout)


def build_routing_context(repo_root: Path) -> str:
    """Build routing context by scanning config file structure."""
    lines = ["Nix config file structure:"]

    # Scan all directories that might contain .nix files
    scan_dirs = [
        repo_root / "home",
        repo_root / "system",
        repo_root / "hosts",
        repo_root / "packages",
    ]

    for dir_path in scan_dirs:
        if not dir_path.exists():
            continue

        for nix_file in sorted(dir_path.rglob("*.nix")):
            # Skip common import hubs
            if nix_file.name in ("default.nix", "common.nix"):
                continue

            rel_path = nix_file.relative_to(repo_root).as_posix()
            nx_comment = _read_nx_comment(nix_file)

            if nx_comment:
                lines.append(f"- {rel_path} → {nx_comment}")
            else:
                lines.append(f"- {rel_path}")

    # Add general routing guidance
    lines.append("")
    lines.append("Routing rules:")
    lines.append("- CLI tools go in packages/nix/cli.nix")
    lines.append("- Language runtimes/toolchains go in packages/nix/languages.nix")
    lines.append("- MCP tools (*-mcp, mcp-*) always go in packages/nix/cli.nix")
    lines.append("- Homebrew formulas go in packages/homebrew/brews.nix")
    lines.append("- GUI apps (casks) go in packages/homebrew/casks.nix")
    lines.append("- Homebrew taps go in packages/homebrew/taps.nix")
    lines.append("")
    lines.append("Language packages (add to withPackages, not as standalone packages):")
    lines.append("- python3Packages.X → add to python3.withPackages in the languages file")
    lines.append("- luaPackages.X → add to lua.withPackages in the languages file")
    lines.append("- nodePackages.X → add to nodejs in the languages file")

    return "\n".join(lines)


def detect_mcp_tool(package_name: str) -> bool:
    """Detect if a package is an MCP (Model Context Protocol) tool."""
    name_lower = package_name.lower()
    return name_lower.endswith("-mcp") or name_lower.startswith("mcp-")


def _normalize_path_token(token: str) -> str:
    return token.strip().strip("`\"'[](){}<>.,:;")


def _extract_path_tokens(text: str) -> list[str]:
    return [_normalize_path_token(tok) for tok in re.findall(r"[A-Za-z0-9_./-]+\.nix", text)]


def _match_candidate(token: str, candidates: list[str]) -> str | None:
    for candidate in candidates:
        if token == candidate or token.endswith(f"/{candidate}"):
            return candidate

    basename_matches = [c for c in candidates if Path(c).name == Path(token).name]
    if len(basename_matches) == 1:
        return basename_matches[0]
    return None


def _select_candidate_from_output(output: str, candidates: list[str]) -> list[str]:
    matches: list[str] = []
    for token in _extract_path_tokens(output):
        matched = _match_candidate(token, candidates)
        if matched and matched not in matches:
            matches.append(matched)

    # Handle plain candidate mentions even when regex tokenization misses punctuation.
    for candidate in candidates:
        if candidate in output and candidate not in matches:
            matches.append(candidate)

    return matches


def _resolve_fixed_target(
    package_name: str,
    *,
    fallback: str,
) -> str | None:
    if detect_mcp_tool(package_name):
        return fallback
    return None


def _resolve_candidate_routing(
    package_name: str,
    output: str,
    candidates: list[str],
    fallback: str,
) -> tuple[str, str | None]:
    matches = _select_candidate_from_output(output, candidates)
    if len(matches) == 1:
        return matches[0], None
    if len(matches) > 1:
        choices = ", ".join(matches)
        return (
            fallback,
            f"Ambiguous routing for {package_name} ({choices}); using fallback {fallback}",
        )
    return fallback, f"Unrecognized routing output for {package_name}; using fallback {fallback}"


def route_package_codex_decision(
    package_name: str,
    context: str,
    cwd: Path | None = None,
    *,
    candidate_files: list[str] | None = None,
    default_target: str | None = None,
) -> tuple[str, str | None]:
    """Route a package to a target file, with optional constrained candidates.

    Returns:
        (target_file, warning)
    """
    fallback = default_target or "packages/nix/cli.nix"

    fixed_target = _resolve_fixed_target(
        package_name,
        fallback=fallback,
    )
    if fixed_target:
        return fixed_target, None

    prompt: str
    if candidate_files:
        candidates = "\n".join(f"- {path}" for path in candidate_files)
        prompt = f"""{context}

Choose exactly one file for '{package_name}' from this allowed list:
{candidates}

Reply with only one exact path from the list."""
    else:
        prompt = f"""{context}

Which packages/nix/*.nix file for '{package_name}'? Just the path (e.g., packages/nix/cli.nix)."""

    success, output = run_codex(prompt, cwd=cwd, timeout=30, reasoning="low")
    if not success:
        return fallback, f"Routing model unavailable for {package_name}; using fallback {fallback}"

    if candidate_files:
        return _resolve_candidate_routing(package_name, output, candidate_files, fallback)

    for token in _extract_path_tokens(output):
        return token, None

    return fallback, f"Routing output missing target for {package_name}; using fallback {fallback}"


def route_package_codex(
    package_name: str,
    context: str,
    cwd: Path | None = None,
    is_cask: bool = False,
    is_brew: bool = False,
    is_mas: bool = False,
) -> str | None:
    """Route a package to the appropriate config file using Codex.

    Compatibility wrapper for older call sites.
    """
    _ = (is_cask, is_brew, is_mas)
    target_file, _warning = route_package_codex_decision(
        package_name,
        context,
        cwd=cwd,
    )
    return target_file


def _build_language_prompt(
    bare_name: str,
    runtime: str,
    method: str,
    target_file: str,
    comment: str,
) -> str:
    return f"""Add '{bare_name}' to the {runtime}.{method} list in {target_file}.

Find the existing {runtime}.withPackages block and add '{bare_name}' alphabetically inside the list.
If there's a comment, add it inline: {bare_name}  {comment}

Example of what to look for:
  ({runtime}.withPackages (ps: with ps; [
    existing-package
    {bare_name}  # <-- add here, alphabetically
  ]))

Just make the edit, no explanation."""


def _build_homebrew_manifest_prompt(package_name: str, target_file: str) -> str | None:
    for suffix in (
        "packages/homebrew/brews.nix",
        "packages/homebrew/casks.nix",
        "packages/homebrew/taps.nix",
    ):
        if target_file.endswith(suffix):
            return f"""Add "{package_name}" to the list in {target_file}.
Keep the list alphabetized. Just make the edit, no explanation."""
    return None


def _build_edit_prompt(
    package_name: str,
    target_file: str,
    comment: str,
    lang_info: tuple[str, str, str] | None,
    is_brew: bool,
    is_mas: bool,
) -> str:
    if lang_info:
        bare_name, runtime, method = lang_info
        return _build_language_prompt(bare_name, runtime, method, target_file, comment)

    manifest_prompt = _build_homebrew_manifest_prompt(package_name, target_file)
    if manifest_prompt:
        return manifest_prompt

    if "darwin.nix" in target_file:
        if is_mas:
            return f"""Add "{package_name}" to the homebrew.masApps set in {target_file}.
Look up the App Store ID if needed and add it as "{package_name}" = <id>;.
Keep keys alphabetized. Just make the edit, no explanation."""
        list_name = "brews" if is_brew else "casks"
        return f"""Add "{package_name}" to the homebrew.{list_name} list in {target_file}.
Add it alphabetically within the {list_name} list. Just make the edit, no explanation."""

    return f"""Add '{package_name}' {comment} to {target_file} in the appropriate section.
Add it alphabetically within its section. Just make the edit, no explanation."""


def edit_via_codex(
    package_name: str,
    target_file: str,
    description: str,
    cwd: Path,
    dry_run: bool = False,
    *,
    is_brew: bool = False,
    is_cask: bool = False,
    is_mas: bool = False,
) -> tuple[bool, str]:
    """Add a package to a config file using Codex."""
    # Check if this is a language package needing withPackages treatment
    lang_info = detect_language_package(package_name)

    if dry_run:
        if lang_info:
            bare_name, runtime, _ = lang_info
            return True, f"[DRY RUN] Would add '{bare_name}' to {runtime}.withPackages in {target_file}"
        return True, f"[DRY RUN] Would add {package_name} to {target_file}"

    # Build edit prompt
    comment = f"# {description[:40]}" if description else ""
    prompt = _build_edit_prompt(package_name, target_file, comment, lang_info, is_brew, is_mas)

    success, output = run_codex(prompt, cwd=cwd, timeout=60, reasoning="low")

    if success:
        if lang_info:
            bare_name, runtime, _ = lang_info
            return True, f"Added {bare_name} to {runtime}.withPackages in {target_file}"
        return True, f"Added {package_name} to {target_file}"
    else:
        return False, f"Codex error: {output}"
