# Changelog

## Unreleased

- preserved passwordless Darwin rebuild setups by falling back to the existing `sudo darwin-rebuild` path when split activation would require an interactive sudo prompt

## v1.5.20 - 2026-04-29

- excluded `nix-gc` from default `nx clean-caches` plans so routine cache cleanup cannot force later Nix downloads or local source builds unless store GC is selected explicitly
- added explicit `nix-gc` safety messaging in `clean-caches` help, docs, and prompts while keeping targeted Nix store GC available via `nx clean-caches nix-gc`
- bumped the pinned Rust toolchain documentation to 1.94.0

## v1.5.19 - 2026-04-29

- collapsed carriage-return progress frames in captured command output so Git/Nix fetch progress no longer smears stale percentages into normal lines
- made interactive split `nix build` keep stdout captured for JSON parsing while teeing stderr raw with Nix's `bar-with-logs` format and a bounded diagnostic tail, preserving progress UI without losing retry detection

## v1.5.18 - 2026-04-29

- preserved native terminal output for interactive split-rebuild activation blocks, bounded by separator lines, so Homebrew and Home Manager can render their own colors and progress UI
- kept captured rebuild output for non-interactive runs and `--timing` so tests, timing detail, retry detection, and hash repair stay structured

## v1.5.17 - 2026-04-29

- raised the file descriptor limit for split Darwin `nix build` invocations, preventing large flake graphs from failing under macOS's low default soft limit
- made split rebuild source-cache recovery clear user tarball caches as well as git/fetcher caches, and allow a bounded series of source-cache retries when separate inputs fail in sequence
- made root tarball-cache cleanup non-interactive so retry recovery does not introduce an unexpected sudo password prompt

## v1.5.16 - 2026-04-29

- added shared loading spinners to other long-running lookups: `nx search`, `nx info`, install source resolution, and the Homebrew update check in `nx upgrade`
- removed the old one-off search progress printer so query commands use the same formatter path as cache scanning and cleanup
- updated the tag release workflow to `softprops/action-gh-release@v3`, avoiding GitHub's Node.js 20 deprecation warning

## v1.5.15 - 2026-04-29

- added a shared `Printer::with_loading` helper so commands can wrap slow operations in a standard spinner scope instead of hand-managing spinner lifetimes
- kept `nx clean-caches` visibly busy while confirmed cache deletion runs, including per-cache progress text for large removals such as `target` trees and Nix garbage collection

## v1.5.14 - 2026-04-28

- fixed transient loading spinners so top-level progress starts at column zero, matching the rest of the status glyph layout
- centralized printer layout and ANSI styling rules so status indentation is tested at the formatter boundary instead of being reimplemented by individual commands

## v1.5.13 - 2026-04-27

- clarified the split Darwin sudo prompt so changed-system rebuilds say sudo is for the system profile update and activation, not an unexplained generic authorization step

## v1.5.12 - 2026-04-27

- fixed split Darwin sudo authorization on macOS by using `sudo -v` for the timestamp check while keeping `sudo -H` on the privileged profile and activation commands
- tightened the system-command sudo stub so invalid `sudo -H -v` invocations are caught before release

## v1.5.11 - 2026-04-27

- made `nx upgrade` recover from safe Nix fixed-output hash drift by updating exactly one clean tracked `.nix` hash occurrence, retrying the rebuild, and committing the repaired file with `flake.lock`
- added clear fixed-output hash repair output, manual next-step hints for unsafe cases, a three-repair safety cap, and `NX_NO_AUTO_HASH_FIX=1` as an opt-out
- kept `nx rebuild` non-mutating for fixed-output hash mismatches while surfacing the specified/got hashes and matching file hints

## v1.5.10 - 2026-04-27

- treated likely typos of guarded command names, such as `nx rebulid`, as command errors instead of rewriting them to `nx install`, preserving clap's command suggestions
- kept ordinary bare package inference intact for package names such as `nx docker`

## v1.5.9 - 2026-04-27

- added live per-bucket progress while `nx clean-caches` scans slow cache roots, including the currently scanned code-root cache type and delayed Nix GC sizing
- added targeted cache cleanup with `nx clean-caches <cache-name...>` and `nx clean-caches --only <cache-name,...>`, plus directory counts for grouped code-root caches
- made split Darwin rebuilds retry source-cache object lookup failures surfaced by the build step, matching the existing lazy-source recovery used by `nx upgrade`

## v1.5.8 - 2026-04-25

- removed the extra blank line between the `Rebuilding system` header and the resolved Darwin host line
- changed split Darwin rebuilds to authorize sudo and continue the split activation path instead of falling back to `darwin-rebuild` after a successful build when no sudo timestamp is cached

## v1.5.7 - 2026-04-25

- added `sudo -H` to split Darwin profile updates and activation so sudo sets root's home before `nx` applies its root Nix environment wrapper, suppressing macOS `$HOME` ownership warnings during `nx rebuild` and `nx upgrade`

## v1.5.6 - 2026-04-25

- made `nx upgrade` force-realize changed GitHub flake inputs with `nix flake prefetch --json` before flake check/rebuild so lazy Nix source-cache misses do not surface as rebuild failures
- expanded Nix cache-corruption recovery to catch `object not found - no match for id` source lookup errors and clear both the user git source cache and fetcher cache before retrying

## v1.5.5 - 2026-04-24

- added `nx clean-caches` for scanning and cleaning local development cache directories
- fixed split Darwin rebuild profile updates and activation to run with root `HOME` and `NIX_REMOTE=daemon`, matching `darwin-rebuild` and avoiding root-owned Nix warnings
- made split Darwin rebuilds fall back to legacy `darwin-rebuild switch` when direct split activation would require an interactive sudo prompt

## v1.5.4 - 2026-04-24

- made Darwin rebuilds use the split `nix build` + profile compare + activation path by default, with `platform.split_rebuild = false` as the legacy opt-out
- skipped profile updates and activation when the built system configuration already matches the current system profile, making warm no-op rebuilds substantially faster
- expanded rebuild timing detail to expose split build/profile/activate phases while preserving nested nix-darwin and Home Manager activation markers

## v1.5.3 - 2026-04-24

- added local rebuild timing telemetry with per-phase durations, git HEAD and `flake.lock` fingerprints, `NX_PROFILE_PATH`, `nx rebuild --timing`, and `nx profile`
- carried rebuild timing records through `nx upgrade` and `nx install --rebuild` with the originating command preserved in profile history
- fixed the GitHub Actions-only manifest-drift install test failure by using deterministic system command stubs instead of depending on host `nix`
- made `nx install` show source-resolution progress before slow package source lookups

## v1.5.2 - 2026-04-11

- added first-class `nx --version` / `nx version`, `nx doctor`, and `nx completion <shell>` commands
- made CLI output contracts more truthful by moving `--json` and `--verbose` to the commands that actually support them, tightening required package args, and expanding command-local help/examples
- unified `search`, `info`, and install package lookups behind shared query plumbing, added cache/timing diagnostics, and stabilized result ordering and JSON output for query commands

## v1.5.1 - 2026-04-09

- changed targeted `nx upgrade <input...>` to call the documented `nix flake update <input...>` syntax instead of deprecated `nix flake lock --update-input`
- corrected upgrade help/docs so passthrough args are described as flake-update passthrough rather than rebuild passthrough

## v1.5.0 - 2026-04-08

- added targeted flake input upgrades via `nx upgrade <input...>`, including support for multiple inputs and passthrough rebuild flags after `--`
- made targeted upgrades skip the Homebrew phase by default while preserving the existing repo-wide `nx upgrade` flow for full brew/rebuild/commit runs
- updated the CLI help, README command guide, and release docs to cover targeted upgrades and shared the flake command builder across `update` and `upgrade`

## v1.4.0 - 2026-04-03

- added a first-class Home Manager module for `nx`, including `programs.nx.repoRoot`, `programs.nx.autoRefresh`, `programs.nx.sops.package`, and `programs.nx.sops.bin`
- added nix-native smoke coverage for the Home Manager module and wired it into CI and release verification
- shifted distribution to a fully Nix-first model by removing the curl installer and binary release artifacts
- updated release verification to check `nix build .`, `nix run . -- --help`, and the Home Manager module path directly
- clarified the configuration model as repo-owned behavior plus machine/session defaults from Home Manager

## v1.3.0 - 2026-04-03

- added host-scoped `nx generations status|plan|prune` with deterministic planning, confirmation-gated pruning, JSON output, and dry-run parity
- added `nx lint` and `nx rebuild --preflight` to catch routing annotation gaps and run rebuild safety checks without switching
- improved `nx install`, `nx search`, and `nx info` performance by reusing cached source lookups, batching multi-package install searches, and reducing repeated repo scans
- fixed `nx upgrade` summaries for GitHub-backed `type = git` flake inputs, including SSH GitHub URLs, and hardened secret error redaction for multi-line `sops` failures
- added GitHub Actions CI, a tag-driven release workflow, a curl installer, and a local release verification helper

## v1.2.1 - 2026-03-16

- aligned the public and internal docs with the shipped Rust CLI surface and toolchain
- added CLI/spec anti-drift tests so `.agents/SPEC.md` stays in sync with clap metadata
- clarified internal `.agents` design notes as Rust-first guidance and marked historical material accordingly
- ignored the local `bd` credential artifact so tracker commands stop dirtying the worktree

This is a release-readiness and documentation hardening release. No intended end-user behavior changes.
