"""
NxPrinter - nx-specific terminal output extensions.

Extends the generic Printer class with package management methods.
"""

from __future__ import annotations

from printer import Printer
from shared import format_source_display

# Activity glyphs for nx workflows - Material Design (Nerd Font)
NX_ACTIVITY_GLYPHS_NERD = {
    "reading": "󰈙",      # nf-md-file_document
    "editing": "󰏫",      # nf-md-pencil
    "searching": "󰍉",    # nf-md-magnify
    "routing": "󰁔",      # nf-md-arrow_decision
    "adding": "󰐕",       # nf-md-plus
    "running": "󰑮",      # nf-md-play
    "analyzing": "󰁔",    # nf-md-arrow_decision (same as action)
    "working": "󰁔",      # nf-md-arrow_decision (generic)
}

# Activity glyphs - Unicode
NX_ACTIVITY_GLYPHS_UNICODE = {
    "reading": "➜",      # heavy arrow (consistent)
    "editing": "✎",      # pencil
    "searching": "◉",    # fisheye/target
    "routing": "➜",      # heavy arrow (same as action)
    "adding": "⊕",       # circled plus
    "running": "▶",      # play triangle
    "analyzing": "➜",    # heavy arrow (same as action)
    "working": "➜",      # heavy arrow (generic)
}

# Activity glyphs - ASCII (maximum compatibility)
NX_ACTIVITY_GLYPHS_MINIMAL = {
    "reading": ">",
    "editing": "*",
    "searching": "?",
    "routing": ">",
    "adding": "+",
    "running": ">",
    "analyzing": ">",
    "working": ">",
}


class NxPrinter(Printer):
    """Printer subclass with nx package management extensions."""

    def __init__(
        self,
        use_plain: bool = False,
        use_minimal: bool = False,
        use_unicode: bool = False,
    ):
        # Select activity glyphs based on tier
        if use_minimal:
            activity_glyphs = NX_ACTIVITY_GLYPHS_MINIMAL
        elif use_unicode:
            activity_glyphs = NX_ACTIVITY_GLYPHS_UNICODE
        elif self._detect_nerd_font():
            activity_glyphs = NX_ACTIVITY_GLYPHS_NERD
        else:
            activity_glyphs = NX_ACTIVITY_GLYPHS_UNICODE

        super().__init__(
            use_plain=use_plain,
            use_minimal=use_minimal,
            use_unicode=use_unicode,
            activity_glyphs=activity_glyphs,
        )

    # === Package Output Methods (nx-specific) ===

    def package_line(self, name: str, source: str, desc: str = "") -> None:
        """Print a package search result: name via source - description."""
        if self.has_rich and self.console:
            self.console.print(f"{self.INDENT}{name} [dim]via {source}[/dim]{desc}")
        else:
            print(f"{self.INDENT}{name} via {source}{desc}")

    def not_found(self, name: str, suggestions: list[str] | None = None) -> None:
        """Package not found - error with recovery suggestions."""
        self.error(f"{name} not found")
        if suggestions:
            print()  # Blank line before suggestions
            for s in suggestions:
                self.suggestion(s)

    def not_installed(self, name: str, install_hint: bool = True) -> None:
        """Package not installed - with optional install suggestion."""
        self.warn(f"{name} is not installed")
        if install_hint:
            self.suggestion(f"Install with: nx {name}")

    def status_table(self, packages: dict[str, list[str]]) -> None:
        """Display package distribution status at column 3."""
        if self.has_rich and self.console:
            table = self.Table(box=self.box.ROUNDED, show_header=True, header_style="heading")
            table.add_column("Source", style="callout")
            table.add_column("Count", justify="right", style="number")
            table.add_column("Examples", style="dim")

            for source, pkgs in packages.items():
                if pkgs:
                    display_name = format_source_display(source)
                    examples = ", ".join(sorted(pkgs)[:4])
                    if len(pkgs) > 4:
                        examples += ", ..."
                    table.add_row(display_name, str(len(pkgs)), examples)

            print()
            self.console.print(table)
        else:
            # Plain text table at column 3
            print()
            header = f"{'Source':<12} {'Count':>5}  Examples"
            print(f"{self.INDENT}{header}")
            for source, pkgs in packages.items():
                if pkgs:
                    display_name = format_source_display(source)
                    examples = ", ".join(sorted(pkgs)[:4])
                    if len(pkgs) > 4:
                        examples += ", ..."
                    print(f"{self.INDENT}{display_name:<12} {len(pkgs):>5}  {examples}")
