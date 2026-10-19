# Upgrade improvement work

Epic: `nx-rs-lhmz`. Evidence and ordering:
[performance-recommendations.md](performance-recommendations.md).

## Shared boundaries

- Use the evaluated candidate system's generated Brewfile when available;
  validate package definitions rather than treating uninstalled packages as
  invalid. Preserve new tap installation and custom platform workflows.
- Keep attempted split-build phases when legacy activation is selected. Any
  execution optimization must retain the already-current shortcut and existing
  sudo authorization scope, with no implicit grant of new privileges.
- Reuse the version-1 trace writer and additive timing fields. Opt-in trace
  directories create unique private files; explicit `NX_TRACE_PATH` keeps its
  existing exclusive-file contract. Retention only removes recognized nx files.
- Use one upgrade scope value for full, targeted-input and Homebrew-only flows.
  A normal full upgrade still refreshes inputs and performs cache admission.
- Bound optional subprocesses with one absolute deadline, including descendants
  and pipe completion. Keep summary failure separate from required upgrade
  failure and preserve each package/input row.
- Observe package state before and after the operation for receipts. Unknown,
  skipped and failed observations stay distinct. Reuse metadata and store
  inspection, avoiding new remote changelog work. Old timing readers remain
  compatible with new records.

## Ordered acceptance

| Recommendation | Bead | Completion evidence |
|---|---|---|
| Early Homebrew validation | `nx-rs-m14j` | Missing/migrated definitions fail before profile changes; valid new installs and new taps remain supported. |
| Accurate fallback phases | `nx-rs-9wfa` | Real orchestration test retains split attempt and separately labels legacy switch, including failure. |
| Routine traces and workload context | `nx-rs-lhmz.1` | Environment and Home Manager controls, bounded file size/count, unique concurrent runs, legacy reader compatibility and test isolation. |
| Evaluation reuse and strategy | `nx-rs-lhmz.2` | Matched controls report source/cache/load and fallback command counts; adopt only justified execution changes and record rejected ones. |
| Homebrew-only scope | `nx-rs-lhmz.3` | Exact call-sequence tests prove no input update/admission/commit; current-pin rebuild, dry run and conflicts covered. |
| Bounded summaries | `nx-rs-lhmz.4` | Hung child/descendant controls terminate within the deadline, optional failures preserve rows, progress uses the shared printer. |
| Observed final receipt | `nx-rs-7q2c` | Installed/upgraded/removed versions, input changes, activation and commit outcomes, partial failure, unknown and PATH shadowing; compatible structured persistence. |

## Research consultation

Decision: keep optional enrichment on the foreground path with bounded child
lifetimes, or detach it. Alternatives: bounded owned subprocesses versus
unbounded/background workers. Missing general evidence: whether cancellation
ownership or tail amplification changes that choice. Stop condition: at most
two directly relevant graph queries. Searches for `tail latency` and
`structured concurrency cancellation` returned no claim specific enough to
change the local decision. Existing source, measured latencies and subprocess
ownership therefore determine the design; no distributed-storage speedup claim
is transferred to this CLI.

## Verification and publication

Use focused tests while implementing, `just ci` for every completed code change,
the deterministic system matrix for command-flow changes, Home Manager smoke
tests for module changes, and the existing performance harness for comparisons.
Do not weaken admission, change flake identity, delete caches, stop shared
workloads or infer runtime savings from loaded-host gate timings. Use scoped jj
commits and publish a review branch. Public release and live adoption remain
explicit final operations.
