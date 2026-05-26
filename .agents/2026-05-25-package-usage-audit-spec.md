# Package Usage Audit Spec

Status: Proposed
Date: 2026-05-25
Scope: Design for an advisory `nx` feature that helps identify declared packages with little local evidence of use.

This is a proposal document. The implemented behavior contract remains
[`SPEC.md`](./SPEC.md) until this feature lands and tests make it normative.

## Research Notes

Prior art suggests three useful constraints:

- Debian `popularity-contest` maps package usage by pairing installed packages with the most recently used executable file and its access/change times. The useful idea is package-level evidence from executable use; the risky idea is background reporting.
- Homebrew analytics separates explicitly requested installs from dependency installs, records command/formula events, keeps retention bounded, and supports opt-out. For `nx`, the right analogue is local-only, explainable evidence with no network reporting.
- zsh `EXTENDED_HISTORY` stores command start time and duration in `~/.zsh_history`; `INC_APPEND_HISTORY_TIME` improves duration fidelity after commands finish. Existing shell history is a good retrospective source, but its absence is not proof of non-use.
- macOS Spotlight exposes `kMDItemLastUsedDate`, updated by LaunchServices for opened files/apps. It is useful for casks and GUI apps, but should be labeled as Spotlight-derived and treated as best-effort.
- Nix garbage collection answers "is this store path still rooted?", not "does the user still use this top-level package". Old generations can keep unused packages alive, so store reachability is the wrong primary signal for package pruning.

## Problem

`nx list` and `nx status` show what the managed repo declares, and `nx remove`
can remove packages safely. There is no command that helps answer:

> Which declared packages look unused enough that I should review them?

The feature should reduce clutter without pretending to know intent.

## Goals

- Provide a read-only audit of declared packages with transparent evidence.
- Rank review candidates by "no evidence of recent use", not by certainty.
- Preserve privacy: all data stays local; no background collection; no network calls.
- Keep the default output short enough to act on.
- Make every candidate actionable with `nx where` and `nx remove --dry-run` guidance.
- Support scripts with stable JSON output.

## Non-Goals

- Do not auto-remove packages.
- Do not create a daemon or install shell hooks in the MVP.
- Do not report package usage to a remote service.
- Do not claim absence of history means absence of use.
- Do not audit transitive dependencies; only packages declared in managed config are candidates.

## Proposed Command

Primary command:

```text
nx unused [OPTIONS]
```

Alias:

```text
nx audit unused
```

The top-level `unused` command is short for daily use. The `audit unused` alias
leaves room for future audits without forcing a nested command on the common path.

Options:

- `--since <DURATION>`: usage window, default `90d`. Accepts `30d`, `12w`, `6mo`, `1y`.
- `--source <SOURCE>`: filter to `nix`, `homebrew`, `cask`, `service`, or `all`.
- `--json`: emit machine-readable results.
- `--verbose/-v`: include all evidence, including weak and protected evidence.
- `--limit <N>`: cap default output, default `25`.
- `--include-protected`: include packages that policy marks as core tooling.
- `--history <PATH>`: read an extra shell history file.
- `--no-history`: skip shell history scanning.
- `--no-spotlight`: skip macOS Spotlight metadata for app bundles.

Exit codes:

- `0`: audit completed, even when candidates were found.
- `1`: audit could not read required repo/package state.
- `2`: clap usage error.

## Evidence Model

Each declared package becomes a `UsageRecord`.

Fields:

- `name`: declared package token, e.g. `ripgrep` or `firefox`.
- `source`: `nix`, `homebrew`, `cask`, `service`, `mas`, or `unknown`.
- `location`: owning config file and line when available.
- `status`: `recent`, `old`, `unknown`, or `protected`.
- `last_seen`: optional timestamp.
- `confidence`: `strong`, `medium`, `weak`, or `none`.
- `evidence`: list of evidence items.
- `suggestions`: commands such as `nx where <name>` and `nx remove --dry-run <name>`.

Evidence item fields:

- `kind`: `shell-history`, `spotlight`, `launchd`, `repo-reference`, `profile`, `policy`.
- `summary`: short human text.
- `timestamp`: optional timestamp.
- `confidence`: item confidence.

Confidence rules:

- Strong: direct command execution in timestamped shell history; cask app bundle has recent `kMDItemLastUsedDate`; service is loaded/running.
- Medium: command appears in untimestamped shell history; package command appears in repo scripts/config; cask app exists but Spotlight has no recent date.
- Weak: package is declared in a language/runtime bundle or known to provide libraries/tools with names that do not match the package token.
- None: no evidence found.

Status rules:

- `recent`: any strong evidence within `--since`, or medium evidence with a timestamp inside the window.
- `old`: evidence exists, but most recent timestamp is older than `--since`.
- `unknown`: no evidence or only weak evidence.
- `protected`: package matches a configurable policy bucket and is hidden unless `--include-protected`.

## Data Sources

### Declared Packages

Use existing finder/list plumbing:

- `find_all_packages()` for buckets.
- `find_package()` for locations.
- manifest health warnings should be surfaced, because a drifted manifest weakens audit coverage.

### Command Evidence

Scan shell history files:

- default: `$HISTFILE` when set, then `~/.zsh_history`, `~/.bash_history`, `~/.config/fish/fish_history` if present.
- parse zsh extended history records of the form `: <epoch>:<duration>;<command>`.
- parse fish YAML-ish history entries with timestamps when feasible.
- parse bash history as untimestamped unless timestamp comments are present.

Command matching:

- Build a command alias set for each package.
- Use explicit package aliases first, then discovered binaries from current profile paths where cheap.
- Match only command words, not arbitrary substrings.
- Treat aliases and shell functions as medium confidence unless the resolved executable is known.

### macOS GUI Evidence

For casks and GUI app packages:

- use known Homebrew cask artifact metadata when available.
- otherwise inspect common app bundle locations.
- query `mdls -name kMDItemLastUsedDate <app>`.
- label all results as Spotlight evidence.

### Service Evidence

For declared services:

- launchd: `launchctl print` or `launchctl list` for current loaded/running state.
- Home Manager/nix-darwin services: config declaration is not usage; running state is usage evidence.
- Running service evidence defaults to protected/recent.

### Repo Reference Evidence

Search managed repo and common user script roots for command tokens:

- repo root only by default.
- optional future setting for extra roots.
- ignore generated files, lockfiles, and target directories.

Repo references are medium confidence when they are executable context
(scripts, shell aliases, launch agents), weak elsewhere.

### Nix Store Evidence

Use store/profile data only as supporting context:

- installed/rooted package path is not usage evidence.
- closure size can be included later to prioritize high-impact candidates.
- old generations should not keep a package out of the candidate list.

## Default Human Output

Default output should lead with review candidates, not telemetry internals.

Example:

```text
  Unused Package Audit (90d)

  Review candidates (8)
  Package          Source    Last evidence       Why
  graphviz         brew      none                no command/app evidence found
  viu              nix       2026-01-08          last shell use 138d ago
  gitx             cask      none                app installed, no Spotlight last-used date

  Protected (hidden): ripgrep, jq, zsh, home-manager

  Try:
    nx where graphviz
    nx remove --dry-run graphviz
```

Output rules:

- Never say "unused" for a package row without a qualifier such as "candidate" or "no evidence".
- Show the window in the title.
- Show count of hidden protected packages when applicable.
- Show manifest drift warnings before candidate rows.
- In `--verbose`, render evidence bullets under each package.
- In `--json`, emit all records, including protected records, with stable field names.

## Protection Policy

The audit should hide noisy false positives by default.

Built-in protected categories:

- core package manager tooling: `nix`, `home-manager`, `darwin-rebuild` providers.
- shell/editor basics: configured shell, `vim`, `neovim`/current editor, `git`.
- `nx` itself and packages required by `nx` workflows.
- active services.
- packages with repo-local comments containing `nx: keep`, `nx: required`, or `nx: protected`.

Future config:

```toml
[unused]
protected = ["ripgrep", "jq", "ffmpeg"]
history_days = 90
extra_history = ["~/.local/share/atuin/history.db"]
```

## Privacy And Safety

- Default mode reads local history but does not store raw commands.
- If caching is added later, store only package-level summaries in
  `~/.local/state/nx/usage-audit.json`, never full command lines.
- No network calls.
- No background telemetry.
- No removal action from `nx unused`; removal remains an explicit `nx remove` flow.
- Redact paths outside the repo/home in normal output unless `--verbose` or `--json`.

## Implementation Shape

Suggested modules:

- `src/commands/unused.rs`: command orchestration and rendering.
- `src/domain/usage.rs`: `UsageRecord`, `EvidenceItem`, scoring rules, policy.
- `src/infra/shell_history.rs`: parsers for zsh/bash/fish history.
- `src/infra/macos_usage.rs`: Spotlight/cask app evidence.
- `src/infra/service_usage.rs`: launchd/service evidence.

Keep the flow functional:

1. collect declared packages.
2. collect evidence sources.
3. normalize evidence into package records.
4. score records.
5. render human or JSON output.

All IO should sit at the infra boundary. Scoring should be pure and heavily unit-tested.

## MVP Acceptance Criteria

- `nx unused` audits Nix, Homebrew, cask, and service buckets from existing package discovery.
- zsh extended history parsing is covered by unit tests.
- Untimestamped history produces medium-confidence evidence but does not mark a package recent.
- Cask Spotlight lookup is optional and skipped gracefully off macOS.
- Output never auto-removes and always suggests `nx remove --dry-run`.
- JSON output includes `name`, `source`, `location`, `status`, `last_seen`, `confidence`, and `evidence`.
- Manifest drift is reported when present.
- Anti-drift tests update `.agents/SPEC.md` only when implementation lands.

## Open Questions

- Should `atuin` or other shell-history databases be supported in MVP, or left for a second pass?
- Should closure size be included in default output, or only behind `--verbose`?
- Should `nx remove` learn to consume a package list from `nx unused --json`, or is manual review enough?
- Should package comments such as `# used by scripts/foo` become structured keep reasons?
