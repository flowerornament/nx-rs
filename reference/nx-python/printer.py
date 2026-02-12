"""
Terminal UI printer with Rich formatting support.

Provides consistent output formatting for CLI applications with:
- Semantic colors via Rich theme
- Glyph-based status indicators
- Code snippet previews
- Plain-text fallback when Rich unavailable
"""

from __future__ import annotations

import shutil
import sys
import textwrap
from contextlib import AbstractContextManager
from pathlib import Path
from types import TracebackType
from typing import Any, ClassVar, Literal, cast

try:
    from rich import box
    from rich.console import Console
    from rich.live import Live
    from rich.padding import Padding
    from rich.panel import Panel
    from rich.prompt import Confirm
    from rich.spinner import Spinner
    from rich.syntax import Syntax
    from rich.table import Table
    from rich.text import Text
    from rich.theme import Theme

    _RICH_AVAILABLE = True
except ImportError:
    box = Console = Live = Padding = Panel = Confirm = Spinner = Syntax = Table = Text = Theme = None
    _RICH_AVAILABLE = False

try:
    from wcwidth import wcswidth
except ImportError:
    # Fallback if wcwidth not available
    def wcswidth(s: str) -> int:
        return len(s)


class _PlainStatus:
    """Plain text status context manager (no spinner).

    Silent in plain mode - spinners are a rich-mode feature.
    """
    def __init__(self, message: str):
        self.message = message

    def __enter__(self) -> _PlainStatus:
        return self  # Silent - don't print status text

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> Literal[False]:
        return False

    def update(self, message: str) -> None:
        pass  # No-op for plain mode


class _RichStatus:
    """Rich status context manager with explicit indent control.

    Uses a transient Live spinner so progress output is ephemeral and clears
    before final output, while keeping spinner glyphs aligned to column 2.
    """

    def __init__(
        self,
        console: Any,
        message: str,
        indent: str,
        spinner_name: str = "line",
        spinner_style: str = "activity",
        leading_blank: bool = True,
    ):
        self.console = console
        self.indent = indent
        self.leading_blank = leading_blank
        self.spinner = Spinner(
            spinner_name,
            text=Text.from_ansi(message),
            style=spinner_style,
        )
        self.live = Live(
            Padding(self.spinner, (0, 0, 0, len(indent)), expand=False),
            console=console,
            refresh_per_second=12.5,
            transient=True,
        )

    def __enter__(self) -> _RichStatus:
        if self.leading_blank:
            print()
        self.live.start()
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> Literal[False]:
        self.live.stop()
        return False

    def update(self, message: str) -> None:
        self.spinner.update(text=Text.from_ansi(message))
        self.live.update(
            Padding(self.spinner, (0, 0, 0, len(self.indent)), expand=False),
            refresh=True,
        )


class Printer:
    """Terminal output with Rich formatting.

    Layout grid:
    - Columns 0-2: Gutter (glyphs only)
    - Column 3+: Content starts here
    - Column 6+: Sub-indent for nested details

    Uses Rich Theme for semantic colors. Falls back to plain text if Rich unavailable.
    """

    # Layout constants (per design system)
    GUTTER = "   "        # 3 chars: glyph + 2 spaces (e.g., "✓  ")
    INDENT = "  "         # 2 spaces - content indent (column 2)
    INDENT2 = "    "      # 4 spaces - sub-indent (column 4)

    # === Three-Tier Glyph System ===
    # Tier 1: Material Design (nf-md-*) - default when Nerd Font detected
    # Tier 2: Unicode - fallback when Nerd Font not available, or --unicode flag
    # Tier 3: ASCII - maximum compatibility, --minimal flag

    # Status glyphs - Material Design (Nerd Font)
    GLYPHS_NERD: ClassVar[dict[str, str]] = {
        "success": "󰄬",      # nf-md-check
        "error": "󰅖",        # nf-md-close
        "warning": "󰀦",      # nf-md-alert
        "action": "󰁔",       # nf-md-arrow_decision
        "dry_run": "󰈈",      # nf-md-eye
        "bullet": "󰧟",       # nf-md-circle-medium
        "result_add": "+",   # plus for additions (green)
        "result_remove": "-", # minus for removals (red)
        "snippet_marker": "󰐕", # nf-md-plus (line marker in code panel)
    }

    # Status glyphs - Unicode (no Nerd Font needed)
    GLYPHS_UNICODE: ClassVar[dict[str, str]] = {
        "success": "✔",      # U+2714 HEAVY CHECK MARK
        "error": "✘",        # U+2718 HEAVY BALLOT X
        "warning": "!",      # plain exclamation
        "action": "➜",       # U+279C HEAVY ROUND-TIPPED RIGHTWARDS ARROW
        "dry_run": "~",      # tilde (preview/approximate)
        "bullet": "•",       # U+2022 BULLET
        "result_add": "+",   # plus for additions (green)
        "result_remove": "-", # minus for removals (red)
        "snippet_marker": "+", # plus (line marker in code panel)
    }

    # Status glyphs - ASCII (maximum compatibility)
    GLYPHS_MINIMAL: ClassVar[dict[str, str]] = {
        "success": "+",      # plus (success/done)
        "error": "x",        # lowercase x
        "warning": "!",      # plain exclamation
        "action": ">",       # plain greater-than
        "dry_run": "~",      # tilde
        "bullet": "-",       # plain hyphen
        "result_add": "+",   # plus for additions
        "result_remove": "-", # minus for removals
        "snippet_marker": "+", # plus (line marker in code panel)
    }

    # Activity glyphs - Generic defaults (just "working" for base class)
    # Subclasses can provide domain-specific activity glyphs
    ACTIVITY_GLYPHS_NERD: ClassVar[dict[str, str]] = {
        "working": "󰁔",      # nf-md-arrow_decision (generic)
    }

    ACTIVITY_GLYPHS_UNICODE: ClassVar[dict[str, str]] = {
        "working": "➜",      # heavy arrow (generic)
    }

    ACTIVITY_GLYPHS_MINIMAL: ClassVar[dict[str, str]] = {
        "working": ">",
    }

    def __init__(
        self,
        use_plain: bool = False,
        use_minimal: bool = False,
        use_unicode: bool = False,
        activity_glyphs: dict[str, str] | None = None,
    ):
        self.use_plain = use_plain
        self.use_minimal = use_minimal
        self.use_unicode = use_unicode
        self.console = None
        self.has_rich = False

        # Determine glyph tier: minimal > unicode > nerd (auto-detect)
        if use_minimal:
            self.glyphs = self.GLYPHS_MINIMAL
            base_activity = self.ACTIVITY_GLYPHS_MINIMAL
        elif use_unicode:
            self.glyphs = self.GLYPHS_UNICODE
            base_activity = self.ACTIVITY_GLYPHS_UNICODE
        elif self._detect_nerd_font():
            self.glyphs = self.GLYPHS_NERD
            base_activity = self.ACTIVITY_GLYPHS_NERD
        else:
            # Auto-fallback to Unicode if Nerd Font not detected
            self.glyphs = self.GLYPHS_UNICODE
            base_activity = self.ACTIVITY_GLYPHS_UNICODE

        # Merge custom activity glyphs with defaults
        self.activity_glyphs = {**base_activity, **(activity_glyphs or {})}

        if not use_plain and _RICH_AVAILABLE:
            # Semantic color theme (per design system)
            THEME = Theme({
                "success": "green",
                "error": "bold red",
                "warning": "yellow",
                "heading": "bold",
                "path": "cyan",
                "number": "cyan",
                "callout": "cyan",
                "dim": "dim",
                "activity": "magenta",   # LLM/agent activity
            })

            self.console = Console(theme=THEME)
            self.Table = Table
            self.Panel = Panel
            self.Confirm = Confirm
            self.Syntax = Syntax
            self.Live = Live
            self.Spinner = Spinner
            self.Padding = Padding
            self.Text = Text
            self.box = box
            self.has_rich = True

    @staticmethod
    def _detect_nerd_font() -> bool:
        """Check if terminal can render Nerd Font (Material Design) icons.

        Tests a known Material Design glyph. If wcwidth returns 0 or negative,
        the font likely doesn't support these glyphs.
        """
        test_glyph = "󰁔"  # nf-md-arrow_decision
        width = int(wcswidth(test_glyph))
        # Nerd Font glyphs typically have width 1 or 2
        # If 0 or -1, the font doesn't have this glyph
        return width > 0

    def _pad_glyph(self, glyph: str, target_width: int = 2) -> str:
        """Pad glyph to target terminal width for consistent alignment.

        Different glyphs have different terminal widths. This ensures
        text after glyphs always starts at the same column.
        """
        width = int(wcswidth(glyph))
        if width <= 0:
            # Unknown or zero-width character, assume width 1
            width = 1
        padding = " " * max(0, target_width - width)
        return glyph + padding

    @staticmethod
    def _wrap_plain_line(text: str, indent: str) -> str:
        """Wrap a plain-text line while preserving indent on continuation lines."""
        term_width = shutil.get_terminal_size(fallback=(80, 24)).columns
        wrapper = textwrap.TextWrapper(
            width=max(len(indent) + 20, term_width),
            initial_indent=indent,
            subsequent_indent=indent,
            replace_whitespace=False,
            drop_whitespace=False,
            expand_tabs=False,
        )
        return wrapper.fill(text)

    def _print_indented_text(self, text: str, indent: str, style: str | None = None) -> None:
        """Print text at a fixed indent with wrapped continuation alignment."""
        lines = text.splitlines() or [text]
        for line in lines:
            if not line:
                print()
                continue
            if self.has_rich and self.console:
                rendered = self.Text.from_ansi(line)
                if style:
                    rendered.stylize(style)
                self.console.print(
                    self.Padding(rendered, (0, 0, 0, len(indent)), expand=False),
                    overflow="fold",
                )
            else:
                print(self._wrap_plain_line(line, indent))

    def _print_indented_renderable(self, renderable: Any, indent: str) -> None:
        """Print a rich renderable at a fixed indent with wrapped alignment."""
        if self.has_rich and self.console:
            self.console.print(
                self.Padding(renderable, (0, 0, 0, len(indent)), expand=False),
                overflow="fold",
            )
        else:
            self._print_indented_text(str(renderable), indent)

    def stream_line(self, text: str, indent: str = "  ") -> None:
        """Print one streamed command output line with consistent indentation."""
        self._print_indented_text(text, indent)

    def action(self, text: str) -> None:
        """Print action header with arrow at column 0."""
        print()
        g = self._pad_glyph(self.glyphs["action"])
        if self.has_rich and self.console:
            self.console.print(f"[callout]{g}[/callout][heading]{text}[/heading]")
        else:
            print(f"{g}{text}")

    def command_header(self, title: str, subtitle: str = "", leading_blank: bool = True) -> None:
        """Print a bold header for command output.

        Pattern: **Title** (subtitle) at column 3

        Args:
            title: Bold header text
            subtitle: Optional text in parentheses
            leading_blank: Whether to print a blank line before (default True)
        """
        if leading_blank:
            print()
        text = f"{title} ({subtitle})" if subtitle else title
        if self.has_rich and self.console:
            rendered = self.Text(title, style="heading")
            if subtitle:
                rendered.append(f" ({subtitle})")
            self._print_indented_renderable(rendered, self.INDENT)
        else:
            self._print_indented_text(text, self.INDENT)

    def section(self, title: str, count: int = 0, tag: str = "") -> None:
        """Print a bold section header at column 3.

        Pattern: **Title** (count) or **Title** (tag)
        Examples:
            **nxs** (49)
            **ripgrep** installed (nxs)
        """
        print()
        suffix = ""
        if count > 0:
            suffix += f" ({count})"
        if tag:
            suffix += f" ({tag})"

        if self.has_rich and self.console:
            rendered = self.Text(title, style="heading")
            if suffix:
                rendered.append(suffix)
            self._print_indented_renderable(rendered, self.INDENT)
        else:
            self._print_indented_text(f"{title}{suffix}", self.INDENT)

    def bullet(self, text: str) -> None:
        """Print a bulleted list item with bullet in gutter.

        Pattern: • item (bullet dim, content default)
        """
        g = self._pad_glyph(self.glyphs["bullet"])
        if self.has_rich and self.console:
            rendered = self.Text()
            rendered.append(g, style="dim")
            rendered.append(text)
            self._print_indented_renderable(rendered, "")
        else:
            self._print_indented_text(f"{g}{text}", "")

    def line(self, text: str) -> None:
        """Print text at column 3."""
        self._print_indented_text(text, self.INDENT)

    def detail(self, text: str) -> None:
        """Print dim text at column 6 (sub-indent for nested details)."""
        self._print_indented_text(text, self.INDENT2, style="dim")

    def heading(self, text: str) -> None:
        """Print bold heading text at column 2."""
        self._print_indented_text(text, self.INDENT, style="heading")

    def numbered_option(self, num: int, text: str) -> None:
        """Print a numbered option at column 4 (sub-indent)."""
        if self.has_rich and self.console:
            rendered = self.Text()
            rendered.append(f"{num}.", style="callout")
            rendered.append(f" {text}")
            self._print_indented_renderable(rendered, self.INDENT2)
        else:
            self._print_indented_text(f"{num}. {text}", self.INDENT2)

    def kv_line(self, key: str, value: str, indent: int = 0) -> None:
        """Print a key-value pair with fixed-width key."""
        spaces = " " * indent
        if self.has_rich and self.console:
            rendered = self.Text(f"{spaces}{key + ':':<13}")
            rendered.append(value, style="dim")
            self._print_indented_renderable(rendered, self.INDENT2)
        else:
            self._print_indented_text(f"{spaces}{key + ':':<13}{value}", self.INDENT2)

    def dim(self, text: str) -> None:
        """Print dim/muted text at column 2."""
        self._print_indented_text(text, self.INDENT, style="dim")

    def suggestion(self, text: str) -> None:
        """Print suggestion/callout text at column 2.

        NOT dim - suggestions are calls to action and should stand out.
        """
        self._print_indented_text(text, self.INDENT, style="callout" if self.has_rich else None)

    def location(self, path: str) -> None:
        """Print a file location in path color at column 3."""
        if self.has_rich and self.console:
            rendered = self.Text("Location: ")
            rendered.append(path, style="path")
            self._print_indented_renderable(rendered, self.INDENT)
        else:
            self._print_indented_text(f"Location: {path}", self.INDENT)

    def activity(self, activity_type: str, text: str) -> None:
        """Print LLM/agent activity in magenta (glyph and text).

        Activity types: reading, editing, searching, routing, adding, running, analyzing, working
        Icon is placed in gutter (column 0) with text aligned after it.
        Uses _pad_glyph to ensure consistent alignment across different glyphs.
        """
        glyph = self.activity_glyphs.get(activity_type, self.activity_glyphs["working"])
        padded = self._pad_glyph(glyph)
        if self.has_rich and self.console:
            rendered = self.Text(f"{padded}{text}", style="activity")
            self._print_indented_renderable(rendered, "")
        else:
            self._print_indented_text(f"{padded}{text}", "")

    def result_add(self, text: str) -> None:
        """Print addition result (green +) with spacing before."""
        print()  # Blank line before results
        g = self._pad_glyph(self.glyphs["result_add"])
        if self.has_rich and self.console:
            self.console.print(f"[success]{g}[/success]{text}")
        else:
            print(f"{g}{text}")

    def result_remove(self, text: str) -> None:
        """Print removal result (red -) with spacing before."""
        print()  # Blank line before results
        g = self._pad_glyph(self.glyphs["result_remove"])
        if self.has_rich and self.console:
            self.console.print(f"[error]{g}[/error]{text}")
        else:
            print(f"{g}{text}")


    def status(self, message: str, leading_blank: bool = True) -> AbstractContextManager[Any]:
        """Return a context manager for showing a spinner.

        Args:
            message: Status text.
            leading_blank: Insert a blank line before spinner output.
        """
        if self.has_rich and self.console:
            return cast(
                AbstractContextManager[Any],
                _RichStatus(
                    self.console,
                    message=message,
                    indent="",
                    spinner_name="line",
                    spinner_style="activity",
                    leading_blank=leading_blank,
                ),
            )
        else:
            return _PlainStatus(message)

    def show_snippet(self, file_path: str, line_num: int, context: int = 2, mode: str = "add") -> None:
        """Show a code snippet around a specific line.

        Args:
            mode: "add" uses snippet_marker (+), "remove" uses result_remove (-)
        """
        try:
            path = Path(file_path)
            lines = path.read_text().split("\n")

            start = max(0, line_num - context - 1)
            end = min(len(lines), line_num + context)

            snippet_lines = lines[start:end]
            marker_glyph = self.glyphs["result_remove"] if mode == "remove" else self.glyphs["snippet_marker"]

            # Minimal box-drawing style per design system
            # Title is just filename (line marker shows which line)
            print()
            if self.has_rich and self.console:
                self.console.print(f"{self.INDENT}┌── {path.name} ───")
                for i, line in enumerate(snippet_lines, start + 1):
                    marker = marker_glyph if i == line_num else " "
                    self.console.print(f"{self.INDENT}│ {marker} [number]{i:4d}[/number] │ {line}")
                self.console.print(f"{self.INDENT}└{'─' * 40}")
            else:
                print(f"{self.INDENT}┌── {path.name} ───")
                for i, line in enumerate(snippet_lines, start + 1):
                    marker = marker_glyph if i == line_num else " "
                    print(f"{self.INDENT}│ {marker} {i:4d} │ {line}")
                print(f"{self.INDENT}└{'─' * 40}")
        except Exception:
            pass  # Silently fail if we can't show the snippet

    def show_dry_run_preview(self, file_path: str, insert_after_line: int, simulated_line: str, context: int = 1) -> None:
        """Show a dry-run preview with the simulated line inserted.

        Uses minimal box-drawing with + marker for additions.
        """
        try:
            path = Path(file_path)
            lines = path.read_text().split("\n")

            start = max(0, insert_after_line - context - 1)
            end = min(len(lines), insert_after_line + context)

            # Infer indentation from surrounding lines
            indent = ""
            for i in range(start, end):
                line = lines[i]
                if line.strip() and not line.strip().startswith("#"):
                    indent = line[:len(line) - len(line.lstrip())]
                    break

            # Ensure simulated line has proper indentation
            simulated_stripped = simulated_line.lstrip()
            simulated_formatted = indent + simulated_stripped

            # Minimal box-drawing style per design system
            print()
            if self.has_rich and self.console:
                self.console.print(f"{self.INDENT}┌── {path.name} (preview) ───")
                for i in range(start, end):
                    self.console.print(f"{self.INDENT}│   [number]{i+1:4d}[/number] │ {lines[i]}")
                    if i + 1 == insert_after_line:
                        # New line: + marker in place of number
                        self.console.print(f"{self.INDENT}│ [success]+     [/success] │ [success]{simulated_formatted}[/success]")
                self.console.print(f"{self.INDENT}└{'─' * 40}")
            else:
                print(f"{self.INDENT}┌── {path.name} (preview) ───")
                for i in range(start, end):
                    print(f"{self.INDENT}│   {i+1:4d} │ {lines[i]}")
                    if i + 1 == insert_after_line:
                        print(f"{self.INDENT}│ +      │ {simulated_formatted}")
                print(f"{self.INDENT}└{'─' * 40}")
        except Exception:
            pass

    def show_removal_preview(self, file_path: str, line_num: int, context: int = 1) -> None:
        """Show a dry-run preview for removal.

        Uses minimal box-drawing with - marker for removals.
        """
        try:
            path = Path(file_path)
            lines = path.read_text().split("\n")

            start = max(0, line_num - context - 1)
            end = min(len(lines), line_num + context)

            snippet_lines = lines[start:end]

            # Minimal box-drawing style per design system
            print()
            if self.has_rich and self.console:
                self.console.print(f"{self.INDENT}┌── {path.name} (preview) ───")
                for i, line in enumerate(snippet_lines, start + 1):
                    if i == line_num:
                        self.console.print(f"{self.INDENT}│ [error]-[/error] [number]{i:4d}[/number] │ [error]{line}[/error]")
                    else:
                        self.console.print(f"{self.INDENT}│   [number]{i:4d}[/number] │ {line}")
                self.console.print(f"{self.INDENT}└{'─' * 40}")
            else:
                print(f"{self.INDENT}┌── {path.name} (preview) ───")
                for i, line in enumerate(snippet_lines, start + 1):
                    if i == line_num:
                        print(f"{self.INDENT}│ - {i:4d} │ {line}")
                    else:
                        print(f"{self.INDENT}│   {i:4d} │ {line}")
                print(f"{self.INDENT}└{'─' * 40}")
        except Exception:
            pass

    def info(self, text: str) -> None:
        """Print dim informational text at column 3."""
        self._print_indented_text(text, self.INDENT, style="dim")

    def success(self, text: str) -> None:
        """Print success message with green prefix."""
        g = self._pad_glyph(self.glyphs["success"])
        if self.has_rich and self.console:
            self.console.print(f"[success]{g}[/success]{text}")
        else:
            print(f"{g}{text}")

    def complete(self, text: str) -> None:
        """Print completion message with green prefix."""
        print()  # Blank line before
        g = self._pad_glyph(self.glyphs["success"])
        if self.has_rich and self.console:
            self.console.print(f"[success]{g}[/success]{text}")
        else:
            print(f"{g}{text}")

    def warn(self, text: str) -> None:
        """Print warning message with yellow prefix."""
        g = self._pad_glyph(self.glyphs["warning"])
        if self.has_rich and self.console:
            self.console.print(f"[warning]{g}[/warning]{text}")
        else:
            print(f"{g}{text}")

    def error(self, text: str) -> None:
        """Print error message with red prefix."""
        g = self._pad_glyph(self.glyphs["error"])
        if self.has_rich and self.console:
            self.console.print(f"[error]{g}[/error]{text}")
        else:
            print(f"{g}{text}", file=sys.stderr)

    def dry_run_banner(self) -> None:
        """Print dry-run mode banner with warning styling."""
        print()
        g = self._pad_glyph(self.glyphs["dry_run"])
        if self.has_rich and self.console:
            self.console.print(f"[warning]{g}[/warning][heading]Dry Run[/heading] [dim](no changes will be made)[/dim]")
        else:
            print(f"{g}Dry Run (no changes will be made)")

    def multi_column_list(self, items: list[str], col_width: int = 16, max_cols: int = 5) -> None:
        """Print items in multiple columns (like brew list).

        Args:
            items: List of strings to display
            col_width: Width of each column
            max_cols: Maximum number of columns
        """
        if not items:
            return

        # Calculate optimal column count based on longest item
        max_item_len = max(len(item) for item in items) + 2  # padding
        actual_col_width = max(col_width, max_item_len)

        # Determine number of columns (aim for ~80 char terminal width minus indent)
        available_width = 78 - len(self.INDENT)
        num_cols = min(max_cols, max(1, available_width // actual_col_width))

        # Sort items and arrange in columns
        sorted_items = sorted(items)
        num_rows = (len(sorted_items) + num_cols - 1) // num_cols

        for row in range(num_rows):
            row_items = []
            for col in range(num_cols):
                idx = row + col * num_rows
                if idx < len(sorted_items):
                    row_items.append(sorted_items[idx].ljust(actual_col_width))
            print(f"{self.INDENT}{''.join(row_items).rstrip()}")

    def confirm(self, prompt: str, default: bool = True) -> bool:
        """Ask for confirmation with proper indentation."""
        if self.has_rich and self.console:
            try:
                return bool(self.Confirm.ask(prompt, default=default, console=self.console))
            except EOFError:
                return default
        suffix = " [Y/n]: " if default else " [y/N]: "
        full_prompt = f"{self.INDENT}{prompt}{suffix}"
        try:
            response = input(full_prompt).strip().lower()
        except EOFError:
            return default  # Non-interactive: use default
        if not response:
            return default
        return response in ("y", "yes")
