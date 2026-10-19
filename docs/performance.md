# Local Nix performance evidence

Current ranked recommendations and new measurements:
[Upgrade performance recommendations, 2026-10-19](performance-recommendations.md).

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

## Evaluation-cache investigation, 2026-10-14 UTC

Ordinary successful upgrades recorded 18.1s, 40.9s and 107.4s total. The 18.1s
run had an unchanged lock hash and spent 8.6s updating inputs and 8.0s in cache
preflight. Its rebuild took 0.4s and recognized an already-current system. These
observations do not alone locate the work inside preflight.

`just perf-nix --host Ishikawa --compare-eval-cache --repeat 3` produced:

| Component | Median | Range |
|---|---:|---:|
| Explicit derivation evaluation | 15.356s | 11.955–52.248s |
| Warm planning | 0.137s | 0.131–0.339s |
| Planning with evaluation cache disabled for that command | 13.501s | 12.488–33.063s |
| Flake check, no builds | 0.135s | 0.129–0.138s |

All 12 commands succeeded. No download activities were observed. Evaluation and
the cache-disabled plans each recorded 16 local source-copy activities. Raw logs,
input identity and per-sample load are under `.nx/perf/real-ishikawa-eval-cache/`.
Our quality gate ran during these measurements: the wide ranges are not isolated
regression estimates. Explicit evaluation before a plan warms the cache;
rotating order is not a cold-cache experiment.

A disposable Git copy of the configuration isolated commit identity. The same
file tree (`941453375f5755fd2f8a6f7e529d744802bc2bc5`) produced:

| Command | Elapsed |
|---|---:|
| Initial plan | 20.492s |
| Repeat at identical HEAD | 0.191s |
| Plan after an empty commit, identical tree | 26.221s |
| Repeat at the new HEAD | 0.188s |

All four plans succeeded without observed downloads. No original configuration,
lock, profile or cache was modified by this experiment. Evidence and the one-off
reproduction script remain under `.nx/perf/eval-git-identity/`. This establishes
that Git metadata changes can invalidate evaluation reuse even without file
changes; it does not establish the exact cause of every recorded preflight.
Flakes can legitimately depend on revision metadata, so automatically changing
flake identity to hide commits would change semantics.

### Concurrency and actionable opportunities

Current top-level upgrade stages run sequentially. Optional input-summary workers
already overlap inside their stage; nested rebuild timings include their children.
Use `python3 scripts/perf_system.py --timeline TRACE` to see phase/command start and
end offsets and peak concurrent commands. The command interval union counts
parallel work once; summed command work is not CPU time.

- Preserve evaluation-cache reuse and investigate its invalidation before removing
  admission or adding parallel Nix evaluations. The warm plan is already cheap.
- Optional summary fetches could overlap Homebrew inspection after admission, but
  need buffered results and one owner of terminal output. Input update -> admission
  -> activation remains an ordering constraint. Trace a real upgrade before
  implementing this scheduling change; the existing no-change summary is free.
- A system test leaked tiny failed upgrades into the real timing ledger. Tests now
  isolate their paths and ignore inherited timing/trace state; future records also
  identify their repository. Existing historical evidence is preserved.
- Full normal upgrade traces remain the next measurement, including update network
  activity, actual Homebrew work and activation, under the config's coordination
  rules. Component experiments are not whole-upgrade speedup claims.
