# Local Nix performance evidence

## Ishikawa, 2026-10-09

Command: `just perf-nix --host Ishikawa --repeat 3`, followed by the same
measurement with `--build`. No activation or lock updates. Existing caches
retained; first sample was not deliberately cold. All 21 component invocations
succeeded. Determinate Nix 3.23.1 (Nix 2.35.2).

| Operation | First run median (range), seconds | Subsequent run median (range), seconds |
|---|---:|---:|
| Explicit `nix eval …system.drvPath` | 16.057 (12.071–16.108) | 14.198 (13.412–14.867) |
| Build dry-run | 0.136 (0.127–1.307) | 0.177 (0.132–0.183) |
| Flake check, no builds | 0.234 (0.136–0.888) | 0.130 (0.125–0.235) |
| System build, no link | — | 0.125 (0.124–0.131) |

Tracked configuration content SHA256:
`5b2a1ffe48bd42337c9afdc5d93d628effea092cfb82d33dc850e259b6cc47b9`.
Realized system:
`/nix/store/3bknxv3abd04gx16papkfym8x41il1bb-darwin-system-26.11.4cff07d`.

Raw evidence remains locally under `.nx/perf/real-ishikawa-first/` and
`.nx/perf/real-ishikawa-build/`. Read with `just perf-nix --report PATH`.
One-minute load ranged 33.1–44.5 during the first run, 25.8–30.5 during the
second; our quality gate also ran during these observations. These are loaded-host
observations, not regression thresholds or isolated CPU measurements.

The first explicit evaluation recorded four download activities: three cache
narinfo queries and one pinned Determinate source download. Later evaluations
recorded zero downloads and 16 local source-copy activities each. All planning,
checking and build samples recorded zero download activities. This does not
prove absence of other network traffic.

Cached system realization and planning are fast at this pin. Explicit `nix eval`
remains much slower; it is a different evaluator/cache path and is not an estimate
of the evaluation portion of `nx upgrade`. The measurements do not justify
removing cache admission. Preserve the normal build path and investigate slow
explicit evaluation only where Nx actually uses it.

## Remaining questions

- Full normal upgrade: input updates, optional enrichment, Homebrew and activation.
- Fresh-input evaluation versus repeated runs, with matched cache conditions.
- Missing binaries: substitution coverage, download bytes and source-build cost.
- Responsiveness: first visible progress and longest unexplained silence.
- Nx overhead: controlled process-wall measurements at lower machine load.

Use `just perf-system` for Nx orchestration controls and `NX_TRACE_PATH` during
normal commands for phase attribution. Do not compare stub subprocess costs to
real network or package-build costs.
