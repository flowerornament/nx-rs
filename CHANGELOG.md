# Changelog

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
