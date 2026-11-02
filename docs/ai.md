# AI features

Nx uses installed, logged-in Claude Code and Codex CLIs. It does not call a model
API directly or fall back to API-key billing. Package metadata requests to
Homebrew and GitHub are separate from model inference.

## Where a model helps

| Feature | Model role | Execution |
|---|---|---|
| Package installation | Advise on ambiguous routing | Read-only; five-second budget; validated candidate paths and deterministic fallback |
| Install/remove editing | Handle a typed unsupported edit shape | Deterministic edits first; bounded file-edit fallback |
| Requested service setup | Prepare configuration following existing files | Uses the selected install engine; bounded file-edit fallback |
| Upgrade notes | Explain trustworthy candidate change data | One combined request, overlapping essential work; cancelled if not ready at completion |

Ordinary deterministic edits, rebuilds, searches and package-state observations
need no login probe or model request. API-only or missing credentials cannot
block those paths. Required AI edits fail explicitly if subscription login is
unavailable; routing falls back to its deterministic target.

## Authentication and model choice

The boundary clears API-key, auth-token and alternate-provider environment
variables in its children. Claude additionally receives session-only settings
that require `claudeai` login and neutralize those settings-file environment
values. Codex selects its built-in OpenAI provider and requires ChatGPT login.
Codex endpoint overrides are constrained to its official ChatGPT service.
A bounded status check verifies the login before inference and is reused within
the invocation. Nx does not read credential files, log keys, modify global
configuration, or invoke login/logout commands.

The model comes from the installed CLI's configuration and defaults. Explicit
install/remove `--model` selections still apply. Small read-only tasks request
low effort, disable unnecessary tools/hooks/MCP setup, and retain the CLI's
model choice. File-edit fallbacks keep required editing tools; Claude receives
file-edit permissions rather than unrestricted Bash/blanket permission bypass.
`NX_AI_BILLING=api` is rejected rather than enabling another billing route.

Claude's [`--bare` mode](https://code.claude.com/docs/en/env-vars) skips OAuth
credentials, so it is unsuitable here. The normal CLI can select an inherited
API key even when the user is logged in; subscription login must be checked in
the same constrained environment used for the request.
[Claude authentication](https://code.claude.com/docs/en/authentication) and
[Codex authentication](https://learn.chatgpt.com/docs/auth) describe the supported
login methods and precedence. Account entitlements and subscription limits
still apply; Nx never retries a failed subscription request with an API key.

## Upgrade notes and latency

Input revisions and Homebrew versions render immediately. An owned background
worker obtains evidence, then makes at most one combined model request, preferring
a logged-in Claude subscription and otherwise a logged-in Codex ChatGPT account.
It does not launch an agent per package. Missing evidence is not sent to a model.

Essential package upgrade, build, activation and commit work proceeds independently.
When that work finishes, Nx uses completed notes and cached evidence; unfinished
optional work is cancelled and its processes reaped. It does not wait out the
summary budget or leave inference running after exit. Very short operations may
finish before fresh notes are ready. Notes describe candidate updates; the
observed receipt remains authoritative about what changed and activated.

`NX_SUMMARY_TIMEOUT_SECONDS` bounds the complete background batch (default 10s).
`--no-ai` omits inference and AI prose, while still allowing source information.
Dry runs do not launch enrichment. Verbose output reports source coverage,
subscription availability, provider errors and cancellation without
repeating a generic warning under every package.

Source discovery uses Homebrew's homepage and uniquely matching GitHub links,
not a hand-maintained project map or an invented repository. Homebrew notes try
published releases matching the exact version (including `v`/`V` tags), then version comparisons. Flake
comparisons use complete locked revisions. Release-note excerpts and sampled
commit titles do not establish every change or risk in an update.

Notes and repository discoveries are cached privately for seven days at
`$XDG_CACHE_HOME/nx/update-notes-v1.json` (default `~/.cache/nx/`). The new file
is separate from existing package caches and receipt formats. Stored keys bind
notes to exact old/new identities; cached AI text remains labeled as AI.

## Audit evidence, 2026-10-28

The prior implementation had four CLI execution paths with different billing
behavior. Only streaming Claude attempted to clear one API-key variable, and
an environment override could re-enable that key. Read-only status checks in
the maintainer environment showed raw Claude selecting an inherited API key;
with the child policy it reported a Claude Max subscription. Codex reported
ChatGPT login. Those probes took about 0.14s and 0.16s respectively. This shows
an authentication-selection risk; it is not evidence of past charges.

Other repairs address write-capable routing, streaming failures reported as
success, a service setup path ignoring the engine choice, and an obsolete
Codex restriction on deterministic flake-input edits. Dated model names in
protocol test fixtures are not runtime model pins. The unused Claude SDK client
feature is disabled; only its protocol types are needed.

Explicit real subscription smokes completed in 2.15s (Claude) and 4.90s
(Codex), without model overrides. Live homepage checks resolved Ghostty to
`ghostty-org/ghostty` and Mole to `tw93/Mole`; Ghostty's page was about 1.9MB,
so discovery accepts compressed HTTPS pages within the shared output limit.
In the controlled upgrade harness, a 60-second stalled inference was
cancelled and the upgrade finished in 2.62s; a successful-note run took 3.18s.
The live two-package Ghostty/Mole evidence-and-summary batch completed in 6.03s
with one Claude subscription request and useful notes for both packages.
These harness observations establish bounded optional work, not production
upgrade latency. Normal tests use stub CLIs and isolated cache directories;
real subscription probes require the explicitly ignored smoke test.

The research graph supplied no directly applicable task-lifetime evidence.
The worker therefore follows the repository's existing owned process-group,
deadline and output-bound contracts, with explicit cancellation and joining.
