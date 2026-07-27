# Determinate Substrate Spec

Status: Implemented
Date: 2026-07-26
Issues: `nx-rs-svyk.1`, `nx-rs-svyk.6`, `nx-rs-svyk.7`

## Principle

Determinate owns the Nix runtime, daemon, garbage collection, and private
source caches. nx observes that substrate and orchestrates public commands. It
does not upgrade the daemon in-process or mutate private Nix cache files.
Explicit user-invoked GC commands may call Nix's public GC interface; nx never
runs GC implicitly.

## Contract

### `nx upgrade`

1. Check Determinate before changing `flake.lock`.
2. When `determinate-nixd` is present, run its supported `version` command.
3. A stale version prints installed/latest versions and
   `sudo determinate-nixd upgrade`, then continues.
4. An unavailable or failed check warns and continues.
5. Never run the upgrade command automatically. A daemon restart and a config
   upgrade are separate operations.

`nx upgrade` already requires the network, so this check is not added to a
separate cache.

### `nx rebuild`

1. Keep the normal path local and offline; do not perform a freshness check.
2. On a known Nix failure, classify the observed signature and parse the
   locally installed distribution/version from `nix --version`.
3. If upstream documents the failure as fixed after the installed version,
   report the fixed version and exact upgrade command.
4. Otherwise preserve the original error and recommend the supported
   diagnostic path. Do not clear caches or retry an unchanged command.

The failure path never invokes `determinate-nixd version` or any other network
check.

### `nx doctor`

Report each substrate fact as `healthy`, `warning`, or `unavailable`; these
states are advisory and do not change the doctor exit status:

- Nix distribution and installed version;
- Nix daemon reachability/version and `nix config check`;
- Determinate daemon/client/latest version;
- `lazy-trees`;
- Determinate Nixd garbage-collector strategy;
- real free space on the filesystem backing `/nix`;
- FlakeHub authentication status.

Predicates:

- successful `nix --version` parsing is healthy; failure remains a required-tool
  error;
- successful structured `nix store info --store daemon` output and a successful
  `nix config check` are healthy; either command failing is a required local
  Nix error;
- Determinate current is healthy, stale is warning, and an unavailable or
  unrecognized version check is unavailable;
- effective `nix config show lazy-trees` value `true` is healthy and `false` is
  warning;
- Determinate Nixd GC strategy `automatic`, including its documented default
  when omitted, is healthy; `disabled` is warning;
- at least 30 GiB free on the filesystem backing `/nix` is healthy; less is a
  warning, with an urgent note below 5 percent free;
- logged-in FlakeHub auth is healthy and logged-out is warning because private
  flakes and cache entitlements are unavailable.

Missing required local tools or an unusable Nix installation remain failures.
Staleness, logged-out auth, low disk, and unavailable network checks remain
advisory here; rebuild admission policy is a separate contract.

`determinate-nixd version` has state-dependent text output. Parsing recognizes
the labeled daemon, client, and optional latest-version lines. The sentence
`You are running the latest version of Determinate Nix.` with no latest-version
line means current. An explicit latest version newer than the daemon means
stale. Any other shape is unavailable and must never be reported as stale.
Tests use the exact 3.21.8 output, including its enabled-features list, plus an
observed stale-version fixture.

## Known Issues

Known issues are typed diagnostics, not retry loops. Version-qualified guidance
must carry its distribution so unrelated version spaces cannot be compared.

The initial table contains the tarball pack-indexer `Too many open files`
failure, fixed in Determinate Nix 3.16.0. A fix applies only when the installed
distribution matches; Determinate, upstream Nix, Lix, and unknown distributions
are never compared across version spaces. Every entry must cite an upstream
release and therefore has an explicit deletion condition.

Lazy-tree source-cache signatures remain diagnostic-only until reproduced on
the current Determinate release:

- `failed to insert entry: invalid object specified`
- `object not found - no match for id`

If either signature recurs on Determinate Nix 3.21.8 or later, treat it as an
unknown current upstream defect: preserve the failure, report it with
`determinate-nixd bug`, and keep the manual command as last-resort guidance.
Never mutate a private cache or retry the unchanged command.

`error: adding a file to a tree builder` is context, not a sufficient
classifier: Nix also emits it for deterministic invalid archives. For the two
object-integrity signatures, nx reports the affected effective home and an exact
last-resort command that removes `gitv3`, `tarball-cache-v2`,
`fetcher-cache-v4.sqlite`, `fetcher-cache-v4.sqlite-wal`, and
`fetcher-cache-v4.sqlite-shm` from that home. Root phases use `/var/root` and a
`sudo rm -rf` command; user phases use `$HOME` without sudo. On Determinate,
nx also prints the corresponding `determinate-nixd bug` report topic.

## Retirements

Remove:

- proactive file-descriptor-limit raises;
- tarball-pack and fetcher-cache deletion;
- source-cache deletion and privileged repair;
- source-cache retry loops;
- changed-input prefetching added only to warm lazy-tree caches.

The file-descriptor workaround compensates for a bug fixed in 3.16.0.
Changed-input prefetch defeats lazy evaluation and duplicates work. Private
cache repair has no supported Determinate maintenance contract and has not been
reproduced on 3.21.8.

After prefetch removal, `nix flake check` and the rebuild fetch source inputs on
demand and remain authoritative. Binary cache coverage preflight is unchanged;
it measures build closure substitution, not source-cache warmth. "Offline
rebuild" in this spec forbids a new nx freshness request; it does not add Nix's
`--offline` flag or promise that missing flake inputs require no network.

## Output

- Preserve native Nix output and the two-space nx detail indent.
- Keep substrate success terse.
- Make warnings actionable and include observed and fixed versions.
- Never hide the original Nix diagnostic behind a repair failure.

## Tests

- Parse distribution-qualified Nix versions.
- Parse Determinate current, stale, malformed, and unavailable output.
- `upgrade` checks first, warns without blocking, and never invokes upgrade.
- `rebuild` performs no freshness/network check, including on failure.
- Known failures select guidance from installed distribution/version.
- A fixed-in release is ignored for every other distribution.
- Current or unknown versions never trigger obsolete repair.
- `doctor` represents unavailable advisory checks without failing.
- Doctor predicates cover disabled lazy trees/GC, low disk, and logged-out auth.
- On Determinate Nix 3.21.8, source-cache diagnostics select the exact effective
  user/root manual command, report upstream, and invoke each failed phase
  exactly once. The upgrade test also proves a seeded user cache survives.
- No private cache path, cache deletion, FD raise, source-prefetch, or
  source-cache retry implementation remains.

## Non-Goals

- Managing Determinate releases for the user.
- Replacing Determinate Nixd health or garbage-collection policy.
- Treating FlakeHub login as universally required.
- Adding a general workaround framework before a second documented issue needs
  it.
