# nx Design System

Specific patterns and specs for nx CLI output.

**Created:** 2026-01-11
**Updated:** 2026-01-14
**Status:** Implemented
**See also:** `CLI_DESIGN_PRINCIPLES.md` for philosophy

---

## Core Philosophy

Design systems work because you make decisions upfront about **primitives that compose well**, then combine them. You don't make separate decisions for every element - you define components that work together by design.

This doc defines those primitives. All nx output should be expressible as combinations of these elements.

---

## Layout Grid

```
Column:  0  1  2  3  4  5  6  ...
         ├────┼──────────────────
         │gut-│ content
         │ter │
         →    │ Installing dog
         ✓    │ Added to packages.nix
              │ Version: 1.2.3
              │    └─ sub-detail
```

| Zone | Columns | Purpose |
|------|---------|---------|
| Gutter | 0-1 | Icons/glyphs only. Never text. |
| Content | 2+ | All text starts here |
| Sub-indent | 4+ | Details nested under a parent (+2 from content) |

**Rules:**
- Icons sit in the gutter, LEFT of content margin
- Text never starts before column 2
- Icons don't push text right - they occupy their own space

---

## Typography

### Title Line
```
**Title** (count)
```

- Title in **bold**
- Count in parentheses, normal weight
- One space between title and count
- Examples:
  - `**Package Status** (62 packages)`
  - `**Installed** (62 packages)`
  - `**nxs** (49)`

### Body Text
Normal weight, starts at column 2.

### Secondary Text
Dim/gray. Used sparingly for truly background info.

### Paths
Distinct color (see Color System). Not dim - paths are useful info.

### Numbers/Counts
Can have distinct color for scannability.

### Suggestions/Callouts
**Not gray.** Suggestions are calls to action - they should stand out.
Use callout color or normal weight.

```
Try: nx info foo
```

---

## Color System

Define colors **semantically**. Map to actual colors in one place so they can be tuned later.

| Semantic Name | Meaning | Suggested Color |
|---------------|---------|-----------------|
| `success` | Completed, positive | Green |
| `error` | Failed, negative | Red |
| `warning` | Caution (use sparingly) | Yellow |
| `heading` | Titles, emphasis | Bold (not a color) |
| `path` | File paths, locations | Cyan or Blue |
| `number` | Counts, line numbers | Cyan or Magenta |
| `callout` | Suggestions, CTAs | Cyan or Blue |
| `dim` | Truly secondary info | Gray |
| `default` | Normal body text | White/default |
| `activity` | LLM/agent operations | Magenta |

**Rules:**
- Never rely on color alone - pair with glyphs or text
- `dim` is for noise reduction, not for actionable content
- `error` (red) only for actual failures
- `warning` (yellow) only for genuine caution

---

## Glyphs

Three-tier system with auto-detection:
1. **Material Design** (default) - Nerd Font nf-md-* icons
2. **Unicode** (`--unicode`) - Standard Unicode, no Nerd Font needed
3. **ASCII** (`--minimal`) - Maximum compatibility

### Status Indicators

| Semantic | Material Design | Unicode | ASCII | Color |
|----------|-----------------|---------|-------|-------|
| Action | `󰁔` nf-md-arrow_decision | `➜` | `>` | `callout` |
| Success | `󰄬` nf-md-check | `✔` | `+` | `success` |
| Error | `󰅖` nf-md-close | `✘` | `x` | `error` |
| Warning | `󱈸` nf-md-alert-outline | `!` | `!` | `warning` |
| Dry run | `󰈈` nf-md-eye | `~` | `~` | `warning` |
| Bullet | `󰧟` nf-md-circle-medium | `•` | `-` | `dim` |
| Snippet marker | `󰐕` nf-md-plus | `+` | `+` | `default` |

### Activity Glyphs (LLM/Agent Operations)

Shown in magenta (`activity` color) at column 2:

| Activity | Material Design | Unicode | ASCII |
|----------|-----------------|---------|-------|
| reading | `󰈙` nf-md-file_document | `➜` | `>` |
| editing | `󰏫` nf-md-pencil | `✎` | `*` |
| searching | `󰍉` nf-md-magnify | `◉` | `?` |
| routing | `󰁔` nf-md-arrow_decision | `➜` | `>` |
| adding | `󰐕` nf-md-plus | `⊕` | `+` |
| running | `󰑮` nf-md-play | `▶` | `>` |
| analyzing | `󰁔` nf-md-arrow_decision | `➜` | `>` |

### Diff Markers (in code panels)

| Glyph | Meaning | Color |
|-------|---------|-------|
| `+` | Addition / highlighted line | `success` |
| `-` | Removal | `error` |

### Deprecated

| Glyph | Replacement |
|-------|-------------|
| `==>` | `➜` in gutter + bold title |
| `ok` | `✔` (heavy check mark) |
| `ERR` | `✘` (heavy ballot X) |
| `!!` | `!` (single exclamation) |
| `▶` | `~` (tilde for dry run) |

---

## Structural Patterns

### 1. Title Line

```
  **Package Status** (62 packages)
```
- Column 2 start
- Bold title
- Parenthetical count

### 2. Action Header

```
➜ Installing dog
```
- Arrow glyph at column 0
- Text at column 2
- Used for operations in progress

### 3. Result Line

```
✔ dog
    packages/nix/cli.nix:12
```
- Glyph at column 0
- Name at column 2
- Details at column 4 (sub-indent)

Or single-line variant:
```
✔ dog at packages/nix/cli.nix:12
```

### 4. Key-Value Pairs

```
  Version:      1.2.3
  Description:  A package that does things
  Homepage:     https://example.com
```
- Labels at column 2, fixed width (12-14 chars)
- Values immediately after
- Paths in `path` color
- Numbers in `number` color

### 5. Code Panel

```
  ┌─ packages.nix
  │     11 │ fd
  │ +   12 │ ripgrep
  │     13 │ fzf
  └────────────────────
```

**Structure:**
- Panel starts at column 2
- Filename in header (with `:line` or `(preview)` as needed)
- Line numbers right-aligned
- 1-2 lines of context above/below

**Line markers (in gutter before line number):**
- `+` - highlight/current line, or addition (green for previews)
- `-` - removal (red, for previews)

**Border color by context:**
- White - default (per minimal design)

### 6. Search Result List

```
  **Found packages** (1)
  dog
    pkgs.dog (confidence: 100%)
```
- Title line with count
- Package name at column 2
- Details sub-indented (column 4) in `dim`
- Shows source, confidence, description
- Used by: `install` when finding packages

### 7. Suggestion

```
  Try: nx info foo
```
- Normal weight or `callout` color
- NOT dim/gray
- Actionable - user should notice it

### 8. Error with Recovery

```
✘ foo not found

  Try: nx search foo
```
- Error glyph + message
- Blank line
- Suggestion for recovery

### 8b. Result Line (Diff Semantics)

```
+ Would add mutagen to packages.nix
- Would remove ripgrep from packages.nix
```
- Blank line before (to separate from activity)
- Uses diff-marker semantics:
  - `+` (green/`success`) for additions: "Would add...", "Added..."
  - `-` (red/`error`) for removals: "Would remove...", "Removed..."
- Shows the final result of an operation

### 9. Tables

```
  Source       Count  Examples
  nxs         49  ripgrep, fd, bat, ...
  homebrew         5  mpd, rmpc, ...
  casks            6  ghostty, aerospace, ...
```
- Header row in **bold**
- Starts at column 2
- Numbers right-aligned
- Examples in `dim`
- No heavy borders (minimal/rounded if any)

### 10. Multi-Column Lists

```
  ast-grep        fd              jq              pandoc          uv
  bat             fzf             lazygit         pipx            vim
  bun             gcalcli         llm             python3         viu
```
- Items sorted alphabetically, reading left-to-right
- Column width based on longest item
- Starts at column 2
- Used for compact package listings

### 11. Confirmation Prompts

```
  Undo changes to packages/nix/cli.nix? [Y/n]:
```
- Question at column 2
- Default option capitalized: `[Y/n]` or `[y/N]`
- Single line, waits for input

### 12. Progress/Spinners

```
  ⠋ Analyzing package...
  ✔ Found in nxs
  ⠋ Adding to config...
```
- Spinner glyph at column 2 (not gutter - it's inline status)
- Replaced with `✔` or `✘` when step completes
- Shows what's happening during AI/long operations
- **Must clear before final output** - no spinner residue

### 13. Agent Activity Stream

```
  󰈙 Reading packages.nix
  󰏫 Editing packages.nix
  󰁔 Routing mutagen
  󰍉 Searching files
```
- Shows real-time AI/agent activity during operations
- Material Design (nf-md-*) glyphs per activity type (see Activity Glyphs table)
- All in `activity` color (magenta) - both glyph and text
- **No trailing ellipsis** - the glyph indicates ongoing activity
- Column 2 start (same as other content)
- Transient - this is process feedback, not final output
- Uses wcwidth for glyph padding to maintain alignment

### 14. Bullet List

```
  • packages/nix/cli.nix
  • packages/nix/languages.nix
```
- Bullet `•` at column 2
- Items at column 4
- Used for simple lists (files to undo, options, etc.)
- Bullet in `dim`, content in default weight

### 15. Name + Location Line

```
  ripgrep                      packages/nix/cli.nix:16
  bat                          packages/nix/cli.nix:9
```
- Package name at column 2
- Location right-aligned (or fixed column)
- Location in `path` color
- Used by: `list -v` for package listings with locations
- Single line per item (not sub-indented like Result Line)

### 16. Title with Status Badge

```
  **ripgrep**  installed (nxs)
  **tree**  not installed
```
- Bold package/item name
- Two spaces
- Status badge: `installed`, `not installed`, etc.
- Optional source in parentheses: `(nxs)`, `(homebrew)`
- Used by: `info` command header
- Status could be colored: green for installed, dim for not installed

---

## Spacing Rules

### Blank Lines

**Add blank line before:**
- Title lines (except at very start of output)
- Code panels
- New sections
- Completion messages (✔ Done)
- Result lines (+ Would add..., - Would remove...)

**Add blank line after:**
- Completion messages (before hints)
- Code panels

**No blank line:**
- Between tightly related items (glyph + its detail)
- At the very start of output
- Between activity lines during LLM processing

### Command Output Pattern

```
➜ Action header

  Section title
  content...

  Code panel...

✔ Done

  Hint text
```

Each major section should be separated by a single blank line.

### Consistency Requirement

Commands showing similar data must use identical patterns:

| Command | Title Pattern |
|---------|---------------|
| `status` | `**Package Status** (62 packages)` |
| `list` | `**Installed** (62 packages)` |
| `list -v` | `**Installed** (62 packages)` |

The title text can differ, but the **structure** (bold + parenthetical count) must match.

---

## Command Patterns

### Informational Commands
`status`, `list`, `list -v`, `info`, `where`

- Start with title line
- No action arrow (nothing is happening)
- Use result lines for items

### Action Commands
`install`, `rm`, `undo`

- Start with action header (`→ Installing...`)
- Show progress/steps
- End with result glyph (`✓` or `✗`)

### Preview Commands
`--dry-run` variants

- Title: `DRY RUN` (bold, maybe warning color)
- Show what *would* happen
- Use `+`/`-` markers in code panels
- End with: `Run without --dry-run to apply.`

---

## Implementation Notes

See `CLI_DESIGN_PRINCIPLES.md` for general code recommendations.

### Rich Theme (Python)

Use Rich's Theme system for semantic colors:

```python
from rich.console import Console
from rich.theme import Theme

THEME = Theme({
    "success": "green",
    "error": "bold red",
    "warning": "yellow",
    "heading": "bold",
    "path": "cyan",
    "number": "cyan",
    "callout": "cyan",
    "dim": "dim",
})

console = Console(theme=THEME)
```

Then use semantic names in output:
```python
console.print("[success]✓[/success] {name} installed")
console.print("[heading]Package Status[/heading] ({count})")
console.print("Location: [path]{file}:{line}[/path]")
```

### Layout Constants

```python
GUTTER_WIDTH = 2      # Columns 0-1
CONTENT_COL = 2       # Content starts here
SUB_INDENT = 4        # Nested details
INDENT = "  "         # 2 spaces
INDENT2 = "    "      # 4 spaces
```

### Glyph + Text Helper

```python
def glyph_line(self, glyph: str, text: str, color: str = None):
    """Print glyph at column 0, text at column 2."""
    # Glyph takes 1 char, then 1 space to reach column 2
    styled_glyph = self.colorize(glyph, color) if color else glyph
    print(f"{styled_glyph} {text}")
```

---

## Implementation Status

Most patterns are implemented. Remaining work:

### Known Limitations
- Nerd Font glyphs have variable terminal widths - slight alignment variations between activity types
- Auto-detection uses wcwidth which may not match all terminal/font combinations

### Completed (2026-01-14)
- Three-tier glyph system (Material Design → Unicode → ASCII)
- Auto-detection with `--unicode` and `--minimal` flags
- Activity glyphs for LLM operations (all Material Design nf-md-*)
- Code preview boxes with `+`/`-` markers
- Semantic color theming via Rich
- Printer class centralizing all output

---

## Audit Checklist

When reviewing a command's output, check:

**Layout**
- [ ] Text starts at column 2?
- [ ] Glyphs in gutter (column 0), not pushing text?
- [ ] Blank lines in right places?

**Typography**
- [ ] Title uses `**Bold** (count)` pattern?
- [ ] Suggestions are NOT gray?
- [ ] Paths have `path` color?

**Consistency**
- [ ] Similar commands have identical structure?
- [ ] No deprecated `==>` arrows?
- [ ] Tables use same styling across commands?

**Elements**
- [ ] Tables: header bold, numbers right-aligned?
- [ ] Multi-column lists: sorted, proper width?
- [ ] Spinners: cleared before final output?
- [ ] Confirmations: default capitalized in `[Y/n]`?
- [ ] Agent activity: all magenta (glyph + text), no trailing `...`?
- [ ] Result lines: success glyph (green), blank line before?
- [ ] Bullet lists: bullet dim, content default?
- [ ] Name + location lines: location in `path` color?
- [ ] Title with status badge: bold name, status colored?

---

## Files

| File | Purpose |
|------|---------|
| `scripts/nx/nx` | Entry point |
| `scripts/nx/printer.py` | Output formatting, glyph system |
| `scripts/nx/commands.py` | Command implementations |
| `.agents/NX_DESIGN_SYSTEM.md` | Patterns & specs (this doc) |
