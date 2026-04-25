# Changelog

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
