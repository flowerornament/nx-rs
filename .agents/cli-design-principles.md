# CLI Design Principles

High-level philosophy for CLI interfaces. These principles guide design decisions but don't specify implementation details.

**For specific patterns:** See `NX_DESIGN_SYSTEM.md`

---

## Core Principles

### 1. Clarity Over Cleverness
- Every output should answer: "What happened? What do I do next?"
- Avoid jargon; prefer plain language
- Show the most important information first

### 2. Progressive Disclosure
- Default output: essential info only
- `--verbose`: detailed information
- `--debug`: everything (for troubleshooting)

### 3. Consistency
- Same patterns across all commands
- Same colors mean the same things
- Same glyphs for the same states

### 4. Graceful Degradation
- Rich output when terminal supports it
- Tiered fallbacks: fancy glyphs → Unicode → ASCII
- Auto-detect capabilities, let users override
- Machine-readable output for scripts (`--json`)

### 5. Respect User Time
- Fast operations need minimal output
- Long operations need progress feedback
- Errors should be actionable

### 6. Bold + Color First
Use bold titles and semantic colors before adding complexity like table borders. Strategic emphasis solves most visual hierarchy problems without visual clutter.

### 7. DRY Styling
Centralize all output formatting. No inline styling scattered through the codebase. Define semantic methods (`success()`, `error()`, `heading()`) and use them everywhere.

---

## Color Philosophy

### Rules
1. **Never rely on color alone** - always pair with glyphs or text
2. **Use dim for noise reduction** - secondary info shouldn't compete
3. **Reserve red for actual errors** - don't cry wolf
4. **Reserve yellow for genuine caution** - not for informational messages
5. **Define colors semantically** - "success" not "green", so you can tune later

### State Semantics
- **Success** (green): completed, exists, installed
- **Error** (red): failed, not found, broken
- **Warning** (yellow): caution, deprecation - use sparingly!
- **Already installed** = Success, not warning

---

## Error Philosophy

### Always Help Recovery
When something fails, don't just report the error. Help the user fix it:
1. **What failed** - clear status
2. **Why it failed** - brief explanation
3. **How to fix it** - actionable suggestions

### Be Specific
Bad: "Operation failed"
Good: "Package 'rg' not found. Did you mean 'ripgrep'?"

---

## Progress Philosophy

### Show Work
- Operations over 1 second need feedback (spinner)
- Multi-step operations should show progress
- Always clear transient output (spinners) before final results

### Streaming Steps
Show each step as it completes. Replace spinners with result glyphs. Give a final summary.

---

## AI/Agent Transparency

When AI is making decisions:

### Show the Process
Users should see that work is happening:
- What sources are being searched
- What decisions are being made
- Where changes will go
- Use activity-specific glyphs (reading, searching, routing, etc.)

### Explain Routing
When AI chooses between options, explain briefly:
- Source chosen and why
- Target file and location
- Confidence level (optional)

### Never Leave Users Wondering
The user should never think "what is it doing?" Show activity, show decisions, show results.

---

## Output Modes

### Rich Mode (default)
Full formatting with colors, glyphs, panels. Auto-detects glyph support.

### Unicode Mode (`--unicode`)
Standard Unicode glyphs only (no Nerd Font required). For terminals without patched fonts.

### Minimal Mode (`--minimal`)
ASCII-only glyphs for maximum compatibility. Works everywhere.

### Plain Mode (`--plain`)
No colors, simple output. For piping, logging, accessibility.

### JSON Mode (`--json`)
Structured output for scripting. Include all relevant data.

### Silent Mode
For scripting commands that should only return exit codes. The Unix way.

---

## Box Drawing Reference

For code panels and trees:

| Character | Unicode | Use |
|-----------|---------|-----|
| `─` | U+2500 | Horizontal line |
| `│` | U+2502 | Vertical line |
| `┌` | U+250C | Top-left corner |
| `└` | U+2514 | Bottom-left corner |
| `├` | U+251C | T-junction |

### Dependency Trees
```
ripgrep
└── pcre2
    ├── bzip2
    └── zlib
```

---

## Code Implementation

### Use Rich with Semantic Themes

Don't scatter inline style strings. Define semantic colors centrally:

```python
from rich.console import Console
from rich.theme import Theme

# Define once, use everywhere
THEME = Theme({
    "success": "green",
    "error": "bold red",
    "warning": "yellow",
    "info": "dim",
    "path": "cyan",
    "heading": "bold",
    "callout": "cyan",
})

console = Console(theme=THEME)

# Then use semantic names
console.print("[success]✓[/success] Package installed")
console.print("[error]✗[/error] Not found")
console.print("Location: [path]packages/nix/cli.nix:12[/path]")
```

### Drop ANSI Fallbacks

Rich is the standard for Python CLIs now. Maintaining dual code paths (Rich vs ANSI) adds complexity for marginal benefit. If Rich isn't available, plain uncolored output is acceptable.

```python
# Avoid this pattern:
if self.has_rich:
    self.console.print(f"[green]✓[/] {text}")
else:
    print(f"{self._green('✓')} {text}")  # ANSI codes

# Prefer this:
self.console.print(f"[success]✓[/success] {text}")
# Falls back to plain text if Rich unavailable
```

### Centralize Output Methods

All output should go through a single class/module. No `print()` scattered through command handlers.

```python
class Output:
    """All CLI output goes through here."""

    def __init__(self):
        self.console = Console(theme=THEME)

    def success(self, text: str) -> None:
        self.console.print(f"[success]✓[/success] {text}")

    def error(self, text: str) -> None:
        self.console.print(f"[error]✗[/error] {text}", stderr=True)

    def heading(self, title: str, count: int = None) -> None:
        if count is not None:
            self.console.print(f"[heading]{title}[/heading] ({count})")
        else:
            self.console.print(f"[heading]{title}[/heading]")
```

### Consider Typer for New CLIs

For new projects or major refactors, [Typer](https://github.com/fastapi/typer) provides:
- Type-hint based argument parsing
- Automatic help generation
- Rich integration out of the box
- Cleaner command structure

```python
import typer
from rich.console import Console

app = typer.Typer()
console = Console()

@app.command()
def install(package: str, dry_run: bool = False):
    """Install a package."""
    if dry_run:
        console.print("[yellow]DRY RUN[/yellow]")
    console.print(f"[success]✓[/success] Installed {package}")

if __name__ == "__main__":
    app()
```

### Test Your Output

Rich supports capturing output for testing:

```python
from rich.console import Console

def test_success_message():
    console = Console(force_terminal=True, record=True)
    output = Output(console=console)

    output.success("Package installed")

    result = console.export_text()
    assert "✓" in result
    assert "Package installed" in result
```

### Comparison: Library Approaches

| Library | Style Syntax | Semantic Colors | Notes |
|---------|--------------|-----------------|-------|
| **Rich** | `[bold red]text[/]` | Via Theme | Most flexible, pure output |
| **Typer** | Uses Rich | Via Rich Theme | Full CLI framework |
| **Cleo** (Poetry) | `<info>text</>` | Built-in tags | Own ecosystem |
| **Click** | `click.style()` | Limited | Older, less pretty |

Rich is the right choice for Python CLIs. The Theme system gives you semantic colors without sacrificing flexibility.

---

## References

- **Homebrew** - Gold standard for package manager UX
- **Cargo** - Clean, informative build output
- **GitHub CLI** - Modern, consistent design
- **pnpm** - Fast, minimal output
- [Rich documentation](https://rich.readthedocs.io/)
- [Typer](https://github.com/fastapi/typer) - Modern CLI framework
- [Cleo](https://github.com/python-poetry/cleo) - Poetry's CLI library
