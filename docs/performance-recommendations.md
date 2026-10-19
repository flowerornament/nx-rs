# Upgrade performance recommendations

Evidence collected on Ishikawa, 2026-10-19 UTC. Nx v1.5.43 is published at
`c785f0fe0e9669f841059162ac9f44451ecbac0f`. This investigation changes no
upgrade scheduling, cache policy, privileges, or input identities.

## Recommendation order

The order weighs reliable completion, recurring wall time, confidence, and
implementation cost. Potential savings below are not measured implementation
speedups. The two user upgrades had different inputs, package work and host
conditions; their totals are not an A/B comparison.

| Rank | Action | Evidence and expected value | Cost / confidence |
|---|---|---|---|
| 1 | Validate Homebrew declarations before changing the system profile; report and repair formula/cask migrations explicitly. | The user's 191.5s run failed during activation because `vercel-cli` was still declared as a formula. `vercel` was already installed as a cask. Correcting the two declarations let the subsequent upgrade complete. A read-only minimal Brewfile check rejected the obsolete declaration in 1.59s and accepted the cask in 0.88s. Catch invalid declarations before activation, while allowing legitimate new installs. | Medium; failure and repair verified. |
| 2 | Preserve timing phases across Darwin fallback, then remove avoidable duplicate build/evaluation work. | Both observed upgrade paths built as the user, then invoked passwordless `darwin-rebuild switch`, which invokes `nix build` again. The fallback drops the split-build child phases from the timing report. Rebuild took 58.2s in the failed run and 17.5s in the successful retry, but those totals also include necessary activation. | Small for accurate phase reporting; medium for execution changes. Duplicate commands verified; savings need a full trace. |
| 3 | Make routine slow/failing runs retain usable traces and workload context. | Both actual upgrades have stage timings but no complete command trace. The 94.1s cache-preflight outlier occurred during observed load near 125 on 18 CPUs, with many unrelated Rust compiler processes. Current JSON history does not record this context. Opt-in unique trace files, bounded retention and start/end load would remove the need to arrange a special upgrade just to diagnose one. | Small/medium; high diagnostic value, no claimed runtime saving. |
| 4 | Reduce expensive evaluation repeats while preserving flake identity and cache admission; qualify performance under a controlled build workload. | Warm planning is 0.29s median; cache-disabled planning is 11.28s median at the current pin. Historical identical-tree Git-commit controls also invalidate reuse. Config edits legitimately change the evaluation key. Inspect user/root evaluation and fallback first; compare one change at a time under matched load. | Medium; cache effect verified. The cause of all 94.1s is not established. |
| 5 | Offer explicit update scopes so Homebrew-only work can avoid refreshing every flake input. | An unchanged full input refresh takes 9.46–13.09s in a disposable config copy. Seventeen observed URL activities are serial and occupy 4.90–7.78s. A scoped operation could avoid that work when the user does not want fresh Nix inputs. Existing targeted-input upgrades already skip Homebrew. Preserve the meaning of a normal full upgrade. | Small/medium; recurring input-refresh cost verified. |
| 6 | Give optional GitHub/AI summaries a deadline and visible progress; then consider overlapping them with Homebrew inspection. | Summaries took 8.08s in the failed run and zero in the unchanged-input retry. Workers already overlap across inputs, but all are joined before continuing. The GitHub and AI subprocesses use blocking capture without a timeout. In the failed run, hiding the whole summary behind later work could save at most 8.08s, about 4.2% of total. | Small/medium for bounds and progress; medium for buffered overlap. Savings are an upper bound. |
| 7 | Produce the final observed-change receipt (`nx-rs-7q2c`) and distinguish partial activation from complete success. | The failed run installed the new nx profile before Brewfile activation failed. The retry really upgraded four casks and completed activation, although the lockfile did not change. A Git commit summary alone cannot describe either outcome. Reuse observed versions/profile deltas; avoid more remote enrichment. | Medium; user-visible correctness need verified, not a speed optimization. |

The Vercel declaration repair is complete in the active working tree and was
adopted by the user's successful retry. An isolated two-file commit,
`de8d624`, was published in [nix-config PR #1](https://github.com/flowerornament/nix-config/pull/1),
excluding the configuration's preexisting unpublished commits and unrelated
working changes. During this investigation the config's other active writer
independently committed the same repair as `7220cd7` and published it on `main`.
The duplicate PR was then closed. Both local and remote config `main` were
verified at `dd6cdbc637541401afccf6a5f12bbbb1cf69a577`, containing that repair.

## What to leave alone for now

- Nx's own orchestration is not the leading measured cost. Representative
  controlled flows spend approximately 3–6ms outside subprocess intervals.
  That residual includes trace/runtime overhead and is not a pure CPU measure.
- Keep binary-cache admission. Warm admission is cheap; removing it would allow
  unexpected source builds and does not address the measured invalidation cost.
- Keep evaluation caching, lazy trees, and the current flake identity semantics.
  Current settings already enable evaluation caching, lazy trees and parallel
  evaluation (`eval-cores = 0`). Do not automatically hide Git revision changes.
- Do not add parallel Nix evaluations as the first optimization. Independent
  input URL resolution is a different opportunity from evaluation; the observed
  URL activities were serial inside one Nix invocation. Concurrent lock writers
  are not a safe way to parallelize those checks.
- Do not combine every producer's nixpkgs through `follows` to simplify the
  graph: the config intentionally retains producer pins to preserve public-cache
  derivation identity.
- Do not switch the torrent-getter input to an unauthenticated public URL. The
  repository is private. The SSH fetch is visible, but these traces do not
  isolate its entire cost.
- Do not tune Homebrew around a supposed 14.8s no-op query: the successful
  retry's Homebrew phase actually upgraded four applications. Inspection alone
  with automatic update disabled measured about one second after warming.

## Actual user runs

These are saved timing records and observed terminal output, not invocations
started by the investigator. The failed run used nx 1.5.42 and installed the new
profile; the retry used nx 1.5.43. Both refer to the user's actual config.

| Top-level stage | Failed upgrade | Successful retry |
|---|---:|---:|
| Determinate check | 5.968s | 0.651s |
| Flake update | 15.548s | 9.996s |
| Cache preflight | 94.113s | 20.523s |
| Flake summaries | 8.082s | 0s |
| Homebrew | 9.540s | 14.754s |
| Rebuild, including its children | 58.194s | 17.484s |
| Complete command | **191.463s, failed** | **63.422s, succeeded** |

The retry upgraded antigravity-cli, flashspace, ghostty, and jayjay, then printed
`System rebuilt`. Its lock hash was unchanged. The preceding failed upgrade had
changed the lock. Their elapsed-time difference is not evidence that v1.5.43
itself made the upgrade 128 seconds faster.

Eight earlier successful records with complete upgrade-stage data ranged from
13.239s to 107.434s. Their flake-update median was 10.383s; cache-preflight ranged
from 0.113s to 18.101s. Legacy rebuild-only records and known test pollution were
excluded rather than mixed into those stage comparisons.

## Controlled measurements

### Existing system scenarios

Command: `just perf-system --repeat 3 --output .nx/perf/ranked-20261019/controlled`.
All 31 scenarios completed three matched traced/untraced pairs: 186 Nx scenario
executions. The suite asserts behavior and command sequences. Network and
package work are simulated. Sampled one-minute load was 55.1 at start and 43.7
at end. Trace deltas are noisy and sometimes negative; do not infer a precise
tracing overhead from these loaded-host samples.

| Scenario | Process-wall median | Range | Median outside subprocess intervals |
|---|---:|---:|---:|
| No flake changes | 153.3ms | 130.2–170.9ms | 3.3ms |
| Flake changes and commit | 258.0ms | 170.2–315.3ms | 4.5ms |
| Rebuild path | 390.8ms | 302.0–540.1ms | 6.4ms |
| Modern tapped casks | 213.4ms | 191.7–250.8ms | 4.0ms |

### Current pinned configuration

Command: `just perf-nix --host Ishikawa --compare-eval-cache --repeat 3`.
All 12 invocations succeeded. Lock updates were forbidden; no system activation,
profile changes, builds or cache deletion were requested. Tracked configuration
content remained stable during the run:

`376d44292dc8eddf3fc5f84fa2fcc0a8a9a50201be66a235dc38b2c697ade397`

| Operation | Median | Range |
|---|---:|---:|
| Explicit derivation evaluation | 13.756s | 12.768–17.550s |
| Warm build planning | 0.290s | 0.141–0.343s |
| Planning with eval-cache disabled for that command | 11.282s | 11.142–13.808s |
| Flake check, no builds | 0.129s | 0.128–0.132s |

No download activities were observed in these component commands. Expensive
evaluation samples each recorded 17 local source-copy activities. Missing
download activities do not prove an absence of every network request. Existing
caches were retained, and explicit evaluation before planning can warm reuse;
this is not a cold-cache benchmark. Sampled one-minute load ranged 31.0–39.2.

### Real input refresh and Homebrew inspection

The existing release binary ran three upgrades against a disposable copy of
tracked config files, with `--skip-rebuild --skip-brew --skip-commit --no-ai`.
Each had its own trace and timing file. All three succeeded with identical
before/after lock hashes (`f4a5a24ea3a24634`). The real managed config, installed
apps and system profile were not mutated by this control.

Process-wall times: **10.018s, 9.455s, 13.088s**. In the middle sample, 8.465s
was the Nix flake-update subprocess and 0.636s the Determinate version query.
Seventeen completed Nix type-101 URL activities were recorded in each run;
their intervals did not overlap. Summed URL-activity durations were 5.049s,
4.905s and 7.775s. One endpoint taking 2.988s contributed to the slowest run.
These are observed Nix activity intervals, not an exhaustive HTTP-request count.

Three `HOMEBREW_NO_AUTO_UPDATE=1 brew outdated --json --greedy-auto-updates`
reads then took **1.825s, 1.038s, 0.993s**, reporting no eligible updates after
the user's four-cask upgrade. Disabling auto-update is a measurement condition,
not a proposed default policy change.

Two minimal Brewfiles then tested the proposed early check, using
`brew bundle check --no-upgrade --file FILE` with auto-update disabled:
`brew "vercel-cli"` failed in 1.588s; `cask "vercel"` passed in 0.881s.
A nonzero bundle check alone is not an invalid-declaration verdict: valid new
packages can simply be uninstalled. Implementation must distinguish missing
installations from missing/migrated package definitions, preferably reusing
batched Homebrew metadata rather than adding one subprocess per declaration.

## Evidence, limits and next implementation slice

Raw traces, output, settings, host observations, fixture measurements and the
read-only collection script remain under `.nx/perf/ranked-20261019/` locally.
The collection preserves failed runs. A compact measurement receipt is in
`docs/performance/2026-10-19.json`. Earlier Git-identity controls and their
limitations remain in [performance.md](performance.md).

The user completed a second real upgrade while read-only work was underway.
Its stage timings are useful operational evidence, not an isolated benchmark.
About 1 TiB of filesystem space was available. A later memory-pressure query
reported 61% free, with about 9.7 GiB of swap occupied; no paging rates were
measured. These snapshots do not establish memory or disk as the latency cause
and do not justify deleting warm caches or buying hardware.
No complete trace was attached to either user-started run, so exact user/root
evaluation costs, prompt latency, and all network traffic remain unresolved.
The investigator did not run host activation, start a managed-config upgrade,
send Hail messages, or stop other workloads. A full investigator-run upgrade
would require the config's nix-ops coordination and explicit message permission.

Start implementation with pre-activation Homebrew validation and preserving
fallback phase accounting. Then add routine trace retention and use the next
normal upgrade to quantify duplicate evaluation before changing execution.
Keep existing correctness scenarios as gates; add cases for migrated
declarations, legitimate missing installations and fallback timing retention. Hardware/load observations are
conditions, not CI performance thresholds. Trace artifacts remain suitable for
later git-evidence import; no integration or imported receipt is claimed here.

For Nix setting semantics, consult the [Determinate performance documentation](https://docs.determinate.systems/determinate-nix/best-practices/).
The measurements and recommendation order above are local evidence, rather than
vendor speedup estimates.

## Implementation follow-through

The seven recommendations are tracked under `nx-rs-lhmz`; see
[upgrade-improvement-plan.md](upgrade-improvement-plan.md) for acceptance and
qualification decisions. The harness supports plan-only runs, HEAD drift checks
and optional load refusal. The first qualified retry refused at 56.493 load on
18 CPUs, before issuing a Nix command; this is not a speed measurement.
Warm-cache identity and the narrow sudo policy remain intact. Further duplicate
evaluation optimization needs the trace from a normal upgrade.
