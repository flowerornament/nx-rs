"""
claude_ops.py - Claude Code integration for nx.

Functions for AI-powered package installation and removal using Claude Code CLI.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from ai_helpers import run_claude
from config import ConfigFiles
from shared import detect_language_package
from sources import SourceResult

if TYPE_CHECKING:
    from nx_printer import NxPrinter as Printer


def find_inserted_line(file_path: str, search_term: str) -> int | None:
    """Find the line number where a term was inserted."""
    try:
        path = Path(file_path)
        lines = path.read_text().split("\n")
        for i, line in enumerate(lines, 1):
            if search_term in line and not line.strip().startswith("#"):
                return i
    except Exception:
        pass
    return None


@dataclass
class InsertResult:
    """Result of an insert operation."""
    success: bool
    message: str
    line_num: int | None = None
    file_path: str | None = None
    simulated_line: str | None = None  # For dry-run preview


def _build_claude_cmd(prompt: str, model: str | None) -> list[str]:
    cmd = [
        "claude", "-p",
        "--verbose",
        "--output-format", "stream-json",
        "--permission-mode", "acceptEdits",  # Auto-accept file edits
    ]
    if model:
        cmd.extend(["--model", model])
    cmd.append(prompt)
    return cmd


def _maybe_report_activity(tool_name: str, tool_input: dict, printer: Printer | None) -> None:
    if not printer:
        return

    if tool_name == "Edit":
        file_path = tool_input.get("file_path", "")
        printer.activity("editing", Path(file_path).name)
    elif tool_name == "Read":
        file_path = tool_input.get("file_path", "")
        printer.activity("reading", Path(file_path).name)
    elif tool_name == "Bash":
        cmd_str = tool_input.get("command", "")[:50]
        printer.activity("running", cmd_str)
    elif tool_name in ("Glob", "Grep"):
        printer.activity("searching", "files")


def _handle_assistant_event(event: dict, printer: Printer | None) -> None:
    message = event.get("message", {})
    content = message.get("content", [])
    for item in content:
        if item.get("type") == "tool_use":
            tool_name = item.get("name", "")
            tool_input = item.get("input", {})
            _maybe_report_activity(tool_name, tool_input, printer)


def _extract_result_event(event: dict) -> tuple[bool, str] | None:
    if event.get("type") != "result":
        return None
    success = not event.get("is_error", False)
    final_result = event.get("result", "")
    return success, final_result


def _maybe_retry_with_fallback(
    success: bool,
    final_result: str,
    prompt: str,
    repo_root: Path,
    printer: Printer | None,
    model: str | None,
    _fallback_attempted: bool,
) -> tuple[bool, str] | None:
    if success or _fallback_attempted:
        return None
    if "not_found_error" not in final_result or "model:" not in final_result:
        return None

    if model:
        if printer:
            printer.warn(f"Model '{model}' not available, falling back to default...")
        return run_claude_streaming(prompt, repo_root, printer, model=None, _fallback_attempted=True)

    if printer:
        printer.warn("Default model not available, falling back to sonnet...")
    return run_claude_streaming(prompt, repo_root, printer, model="sonnet", _fallback_attempted=True)


def run_claude_streaming(
    prompt: str,
    repo_root: Path,
    printer: Printer | None = None,
    model: str | None = None,
    _fallback_attempted: bool = False,
) -> tuple[bool, str]:
    """Run Claude with streaming output, showing tool use in real-time."""

    cmd = _build_claude_cmd(prompt, model)

    try:
        process = subprocess.Popen(
            cmd,
            cwd=repo_root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,  # Line buffered
        )
        assert process.stdout is not None

        final_result = ""
        success = False

        for raw_line in process.stdout:
            line = raw_line.strip()
            if not line:
                continue

            try:
                event = json.loads(line)
                if event.get("type") == "assistant":
                    _handle_assistant_event(event, printer)

                result = _extract_result_event(event)
                if result:
                    success, final_result = result

            except json.JSONDecodeError:
                continue

        process.wait()

        fallback = _maybe_retry_with_fallback(
            success,
            final_result,
            prompt,
            repo_root,
            printer,
            model,
            _fallback_attempted,
        )
        if fallback:
            return fallback

        return success, final_result

    except Exception as e:
        return False, f"Error running Claude: {e}"


def predict_install_location(
    source_result: SourceResult,
    config_files: ConfigFiles,
) -> tuple[str | None, int | None, str | None]:
    """Predict where a package would be installed for dry-run preview.

    Returns: (file_path, insert_line, simulated_line)
    """
    name = source_result.name
    source = source_result.source

    # Map source to target file
    if source == "nxs":
        target_file = str(config_files.packages)
    elif source == "cask":
        target_file = str(config_files.homebrew_casks)
    elif source in ("homebrew", "brew"):
        target_file = str(config_files.homebrew_brews)
    elif source == "mas":
        target_file = str(config_files.darwin)
    else:
        return None, None, name

    return target_file, None, None  # Claude will predict the actual location


def _build_install_prompt(source_result: SourceResult) -> tuple[str, str]:
    if source_result.source in ("cask",):
        search_term = f'"{source_result.name}"'
        prompt = f"""Add the GUI application "{source_result.name}" to this nix-darwin configuration.

## Package Info
- Name: {source_result.name}
- Source: homebrew cask (GUI application)
- Description: {source_result.description}

## Instructions
1. Read CLAUDE.md to find the ARCHITECTURE.md path (near the top), then read that architecture doc
2. Find the homebrew cask manifest (packages/homebrew/casks.nix)
3. Add "{source_result.name}" to the casks list alphabetically
4. Use the Edit tool to make the change

IMPORTANT: Only add the package. Do not run any commands, commit changes, or perform other actions."""
        return search_term, prompt

    if source_result.source in ("mas",):
        search_term = f'"{source_result.name}"'
        prompt = f"""Add the Mac App Store app "{source_result.name}" to this nix-darwin configuration.

## Package Info
- Name: {source_result.name}
- Source: Mac App Store

## Instructions
1. Read CLAUDE.md to find the ARCHITECTURE.md path (near the top), then read that architecture doc
2. Find homebrew.masApps (check system/darwin.nix or hosts/*.nix)
3. Add the app (you may need to look up the App Store ID)
4. Use the Edit tool to make the change

IMPORTANT: Only add the package. Do not run any commands, commit changes, or perform other actions."""
        return search_term, prompt

    if source_result.source in ("homebrew", "brew"):
        search_term = f'"{source_result.name}"'
        prompt = f"""Add "{source_result.name}" as a homebrew formula to this nix-darwin configuration.

## Package Info
- Name: {source_result.name}
- Source: homebrew brew
- Description: {source_result.description}

## Instructions
1. Read CLAUDE.md to find the ARCHITECTURE.md path (near the top), then read that architecture doc
2. Find the homebrew brew manifest (packages/homebrew/brews.nix)
3. Add "{source_result.name}" to the brews list with a brief comment
4. Use the Edit tool to make the change

IMPORTANT: Only add the package. Do not run any commands, commit changes, or perform other actions."""
        return search_term, prompt

    search_term = source_result.attr or source_result.name
    prompt = f"""Add the package "{source_result.attr}" to this nix-darwin configuration.

## Package Info
- Name: {source_result.name}
- Attribute: pkgs.{source_result.attr}
- Source: {source_result.source}
- Description: {source_result.description}

## Instructions
1. Read CLAUDE.md to find the ARCHITECTURE.md path (near the top), then read that architecture doc
2. Decide whether it belongs in packages/nix/cli.nix or packages/nix/languages.nix
3. Add the package in the right location with a brief inline comment
5. Use the Edit tool to make the change

IMPORTANT: Only add the package. Do not run any commands, commit changes, or perform other actions."""
    return search_term, prompt


def _build_targeted_install_prompt(
    source_result: SourceResult,
    package_token: str,
    target_file: str,
    insertion_mode: str,
) -> tuple[str, str]:
    search_term = package_token
    insertion_instructions = "Add the package in the appropriate section in that file."

    if insertion_mode == "language_with_packages":
        lang_info = detect_language_package(package_token)
        if lang_info:
            bare_name, runtime, method = lang_info
            search_term = bare_name
            insertion_instructions = (
                f"Add '{bare_name}' inside the existing {runtime}.{method} list."
                " Do not add it as a top-level package."
            )
    elif insertion_mode == "homebrew_manifest":
        quoted_name = f'"{source_result.name}"'
        search_term = quoted_name
        if source_result.source == "cask":
            insertion_instructions = f"Add {quoted_name} to the casks list alphabetically."
        else:
            insertion_instructions = f"Add {quoted_name} to the brews list alphabetically."
    elif insertion_mode == "mas_apps":
        quoted_name = f'"{source_result.name}"'
        search_term = quoted_name
        insertion_instructions = (
            f"Add {quoted_name} to homebrew.masApps with its App Store ID and keep keys alphabetized."
        )

    prompt = f"""Add one package to this nix-darwin configuration.

## Planned Install Contract
- Package name: {source_result.name}
- Package token: {package_token}
- Target file: {target_file}
- Insertion mode: {insertion_mode}
- Source: {source_result.source}
- Description: {source_result.description or "n/a"}

## Instructions
1. Read CLAUDE.md to find the ARCHITECTURE.md path (near the top), then read that architecture doc
2. Edit ONLY {target_file}
3. {insertion_instructions}
4. Preserve existing formatting and alphabetical ordering
5. Use the Edit tool to make the change

IMPORTANT: Only add this package. Do not run commands, commit changes, or edit other files."""
    return search_term, prompt


def _claude_missing() -> bool:
    return shutil.which("claude") is None


def _dry_run_prompt(source_result: SourceResult) -> str:
    return f"""Analyze where to add "{source_result.name}" ({source_result.source}) to this nix config.

Read CLAUDE.md to find ARCHITECTURE.md, then determine the exact file and line number where you would add this package.

IMPORTANT: Do NOT make any edits. Just analyze and report.

Output ONLY in this exact format:
FILE: <absolute path>
LINE: <line number to insert after>
CODE: <the exact line you would add>

Nothing else - just those three lines."""


def _parse_dry_run_response(source_result: SourceResult, response: str) -> InsertResult:
    file_path = None
    line_num = None
    simulated = None
    for line in response.strip().split("\n"):
        if line.startswith("FILE:"):
            file_path = line.split(":", 1)[1].strip()
        elif line.startswith("LINE:"):
            try:
                line_num = int(line.split(":", 1)[1].strip())
            except ValueError:
                pass
        elif line.startswith("CODE:"):
            simulated = line.split(":", 1)[1].strip()

    return InsertResult(
        success=True,
        message=f"Would add {source_result.name}",
        line_num=line_num,
        file_path=file_path,
        simulated_line=simulated,
    )


def _dry_run_fallback_result(
    source_result: SourceResult,
    target_file: str | None,
) -> InsertResult:
    return InsertResult(
        success=True,
        message=f"Would add {source_result.name} to {Path(target_file).name if target_file else 'config'}",
        file_path=target_file,
    )


def _dry_run_planned_result(
    source_result: SourceResult,
    package_token: str,
    target_file: str,
    insertion_mode: str,
) -> InsertResult:
    simulated_line = package_token
    if insertion_mode == "language_with_packages":
        lang_info = detect_language_package(package_token)
        if lang_info:
            simulated_line = lang_info[0]

    return InsertResult(
        success=True,
        message=f"Would add {source_result.name} to {Path(target_file).name}",
        file_path=target_file,
        simulated_line=simulated_line,
    )


def _scan_inserted_line(
    search_term: str,
    config_files: ConfigFiles,
    preferred_file: str | None = None,
) -> tuple[int | None, str | None]:
    if preferred_file:
        preferred_path = Path(preferred_file)
        if preferred_path.exists():
            found_line = find_inserted_line(str(preferred_path), search_term)
            if found_line:
                return found_line, str(preferred_path)

    for check_file in config_files.all_files:
        if check_file.exists():
            found_line = find_inserted_line(str(check_file), search_term)
            if found_line:
                return found_line, str(check_file)
    return None, None


def insert_via_claude(
    source_result: SourceResult,
    repo_root: Path,
    config_files: ConfigFiles,
    printer: Printer | None = None,
    dry_run: bool = False,
    model: str | None = None,
    package_token: str | None = None,
    target_file: str | None = None,
    insertion_mode: str | None = None,
) -> InsertResult:
    """Use Claude to analyze where to place the package and make the edit.

    Single Claude call that:
    1. Reads CLAUDE.md and ARCHITECTURE.md to understand file organization
    2. Decides which file and location is appropriate
    3. Makes the edit with proper formatting
    """
    effective_token = package_token or source_result.attr or source_result.name
    planned_mode = insertion_mode or "nix_manifest"

    if target_file:
        search_term, prompt = _build_targeted_install_prompt(
            source_result,
            effective_token,
            target_file,
            planned_mode,
        )
    else:
        search_term, prompt = _build_install_prompt(source_result)

    if _claude_missing():
        return InsertResult(False, "Claude CLI not found. Install with: brew install claude-code")

    if dry_run:
        if target_file:
            return _dry_run_planned_result(source_result, effective_token, target_file, planned_mode)

        target_file, _, _ = predict_install_location(source_result, config_files)
        success, response = run_claude(_dry_run_prompt(source_result), cwd=repo_root, timeout=30)
        if success:
            return _parse_dry_run_response(source_result, response)
        return _dry_run_fallback_result(source_result, target_file)

    # Real install path
    if printer:
        printer.activity("analyzing", source_result.name)

    success, result_text = run_claude_streaming(prompt, repo_root, printer, model=model)

    if not success:
        return InsertResult(False, f"Claude error: {result_text}")

    preferred_file = None
    if target_file:
        preferred_path = Path(target_file)
        if not preferred_path.is_absolute():
            preferred_path = repo_root / preferred_path
        preferred_file = str(preferred_path)

    line_num, inserted_file = _scan_inserted_line(search_term, config_files, preferred_file)
    return InsertResult(True, result_text, line_num, inserted_file)


def insert_service_via_claude(
    name: str,
    config_files: ConfigFiles,
    repo_root: Path,
    dry_run: bool = False,
) -> tuple[bool, str]:
    """Add a launchd service configuration."""
    services_file = config_files.services
    prompt = f"""Add a launchd agent for {name} to {services_file}.

Read the existing file to understand the pattern, then create a service configuration.
The binary is likely at /opt/homebrew/opt/{name}/bin/{name} or in the nix store.

Use the Edit tool to add the configuration."""

    if dry_run:
        return True, f"[DRY RUN] Would add service for: {name}"

    success, response = run_claude(prompt, cwd=repo_root, timeout=60)
    if success:
        return True, response
    else:
        return False, f"Claude error: {response}"


def remove_line_directly(
    file_path: str,
    line_num: int,
) -> tuple[bool, str]:
    """Remove a line (or block) from a file directly (no Claude needed).

    If the line starts a Nix block (ends with '= {' or '= ['), removes
    the entire block including all lines until the matching closing brace.
    Also removes comment lines immediately preceding the block.

    Args:
        file_path: Path to the file
        line_num: 1-based line number to remove

    Returns:
        Tuple of (success, message)
    """
    try:
        path = Path(file_path)
        lines = path.read_text().splitlines(keepends=True)

        if line_num < 1 or line_num > len(lines):
            return False, f"Line {line_num} out of range (file has {len(lines)} lines)"

        idx = line_num - 1  # Convert to 0-based
        target_line = lines[idx].rstrip()
        removed_desc = target_line.strip()

        # Check if this line starts a block (e.g., "launchd.agents.foo = {")
        if target_line.endswith("= {") or target_line.endswith("= ["):
            # Find matching closing brace/bracket
            opener = "{" if target_line.rstrip().endswith("{") else "["
            closer = "}" if opener == "{" else "]"
            depth = 1
            end_idx = idx + 1

            while end_idx < len(lines) and depth > 0:
                line = lines[end_idx]
                # Count braces/brackets (simple counting, not parsing strings)
                depth += line.count(opener) - line.count(closer)
                end_idx += 1

            # Find preceding comment lines (consecutive # lines right before the block)
            start_idx = idx
            while start_idx > 0:
                prev_line = lines[start_idx - 1].strip()
                if prev_line.startswith("#") and prev_line != "#":
                    start_idx -= 1
                elif prev_line == "":
                    # Include one blank line before comments
                    if start_idx > 1 and lines[start_idx - 2].strip().startswith("#"):
                        start_idx -= 1
                    break
                else:
                    break

            # Remove the block (start_idx to end_idx)
            del lines[start_idx:end_idx]
            removed_desc = f"block ({end_idx - start_idx} lines)"
        else:
            # Single line removal
            del lines[idx]

        # Write back
        path.write_text("".join(lines))

        return True, f"Removed: {removed_desc}"
    except Exception as e:
        return False, str(e)


def remove_via_claude(
    name: str,
    location: str,
    repo_root: Path,
    printer: Printer | None = None,
    dry_run: bool = False,
    model: str | None = None,
) -> tuple[bool, str]:
    """Remove a package using Claude Code (fallback, prefer remove_line_directly)."""
    prompt = f"""Remove the package "{name}" from {location}.

Remove the entire line including any inline comment.
If it was the only item in a section, you can remove the section header comment too.

Only make the edit, no explanation. Use the Edit tool."""

    if dry_run:
        return True, f"[DRY RUN] Would run Claude with prompt:\n{prompt}"

    success, response = run_claude_streaming(prompt, repo_root, printer, model=model)
    if success:
        return True, response
    else:
        return False, f"Claude error: {response}"
