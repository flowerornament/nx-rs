# Changelog

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
