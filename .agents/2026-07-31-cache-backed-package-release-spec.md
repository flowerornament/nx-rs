# Cache-Backed Package Release Channel Specification

Status: Proposed
Date: 2026-07-31
Owner: flowerornament projects
Epic: `nx-rs-xet9`
Pilot: `nx-rs-8ah3`
Foundation: `nx-rs-1uxl`

## Purpose

Published flowerornament command-line packages should behave like a small,
reliable package channel:

- `nx upgrade` resolves the latest stable release of every configured package;
- Ishikawa downloads prebuilt Nix outputs instead of compiling those packages;
- independent users can run or install the same packages directly from GitHub;
- source builds remain a correct fallback when a binary is unavailable;
- local worktrees under `~/code` affect only explicitly invoked development
  commands, never stable system rebuilds.

The first implementation is an Anneal pilot. No other project migrates until
that pilot proves the complete producer-to-consumer path.

## Problem

The current configuration has CI but not a shared artifact identity.

1. `.nix-config` makes project inputs follow its moving `nixpkgs`.
2. Nx and Anneal Home Manager modules reconstruct their package with the
   consumer's `pkgs`.
3. Project CI either runs Cargo directly or performs a Linux-only Nix smoke
   build without publishing a Nix runtime closure.
4. The resulting CI and consumer derivations differ, so no binary cache could
   substitute one for the other.
5. A repository hosted on GitHub is not automatically resolved from an
   equivalent checkout under `~/code`; the declared flake URL remains the
   source authority.

Observed on 2026-07-30:

| Package | `.nix-config` output | Standalone project output |
|---|---|---|
| Anneal 0.23.0 | `yj6vvr610lsnzblh2h79fam8nn6nlhrd-anneal-0.23.0` | `f3sqgbhmgzclclv55hnriij84ihp82mx-anneal-0.23.0` |
| nx 1.5.33 | `wq5fjzwm8kgfsj1hb8rlp0aklsds4ib1-nx-1.5.33` | `557qjhq8lcj7lkypyj48sqmgiskin3ss-nx-1.5.33` |

Within `.nix-config`, each module package currently equals its corresponding
input package. The mismatch is between the root-overridden package and the
standalone producer package.

## Terms

- **Producer**: the repository that owns a package, flake, lock file, and
  release.
- **Consumer**: `.nix-config` or another flake that installs the producer's
  package.
- **Release channel**: the mutable `release` branch pointing at the latest
  stable, cache-ready release.
- **Release identity**: source revision, committed producer lock graph, target
  system, package expression, and output name.
- **Runtime closure**: the package output and store paths needed to execute it,
  excluding build-only inputs.
- **Cache-ready**: every required release output is present and queryable in
  the public binary cache.

## Governing Invariants

### 1. One release identity

For a given source revision and system, CI, standalone users, and
`.nix-config` must request the same package output path.

The producer's committed `flake.lock` is authoritative for the default
package. Consumers must not replace the producer's `nixpkgs` input when they
want the cached release package.

Consumer-native rebuilding remains an explicit opt-in through a package
override or overlay. It is not the default installation path.

### 2. One package definition

Each producer exposes:

- `packages.<system>.default`;
- `apps.<system>.default` when the package is runnable;
- a Home Manager module when project-specific configuration is useful.

A Home Manager module's `package` option defaults to the producer output for
the active system:

```nix
self.packages.${pkgs.stdenv.hostPlatform.system}.default
```

The module must not re-import the package expression with the consumer's
`pkgs` as its default. The option remains overrideable with
`lib.types.package`.

### 3. Cache before channel visibility

The `release` branch must never point at a revision whose required outputs are
not cache-ready.

The release order is:

1. commit the release source, version, and producer lock;
2. push the commit to the development branch;
3. build and publish every required Nix package output;
4. verify each output through the public cache endpoint from a clean context;
5. create the immutable version tag;
6. advance `release` to that tagged commit;
7. publish ordinary GitHub release assets, when the project has them.

Tagging and channel publication may be one guarded release operation, but
channel visibility remains after cache verification.

### 4. Failure preserves correctness

The cache is an acceleration layer, not the source of truth.

- A missing cache path may fall back to a normal source build.
- A cache outage must not make the flake unevaluable or remove source builds.
- A release operation fails before moving `release` when publication or
  verification fails.
- `nx upgrade` retains its existing source-build admission warning and
  rollback behavior.

### 5. Stable and development sources are separate

Stable packages use remote, lockable GitHub release-channel inputs.

Development commands may use local `~/code` worktrees, but only lazily when the
user invokes a `*-dev` command. Dirty or broken local work must never enter the
stable system closure.

## Binary Cache

### Public cache

Use one public Cachix cache for public flowerornament projects, preferably:

```text
https://flowerornament.cachix.org
```

If that name is unavailable, select one stable organization-level name and use
it everywhere. The cache name, endpoint, and public key are durable public
interface.

One cache is appropriate because all current public projects share one owner
and write-trust boundary. Split caches only if contributor trust or retention
policy later diverges.

### Publication scope

Publish only the package runtime closure to the public cache:

```bash
out="$(nix build --no-link --print-out-paths .#packages.<system>.default)"
cachix push <cache> "$out"
```

Do not use a whole-store scan for release publication. Do not intentionally
publish project source trees, Cargo vendor derivations, test fixtures, dev
shells, or build-only intermediates.

Cachix may omit closure members already available from configured upstream
caches. The release verification checks the final runtime output, not the
number of uploaded paths.

### CI-local acceleration

GitHub Actions may use Magic Nix Cache for build-only intermediates after the
pilot measures a material CI cost. It is optional and never satisfies the
public substitution contract because it is available only inside Actions.

Keep the two roles distinct:

- Magic Nix Cache: optional CI-local build acceleration;
- Cachix: public release runtime distribution.

### Retention

The free Cachix tier is finite and least-recently-used paths may be collected.
After publication, pin each package and system under a stable, system-specific
name with bounded history:

```bash
cachix pin <cache> anneal-aarch64-darwin "$out" --keep-revisions 3
```

Retain the latest three release outputs per package and system unless measured
usage justifies another bound. Every pin name includes the system so publishing
one matrix entry cannot replace another system's retained revisions.

The pilot records compressed cache growth before the wider rollout.

## Supported Systems

CI builds every system the producer advertises. A flake must not claim a
package system that its release pipeline never verifies.

Default runner mapping:

| Nix system | GitHub runner |
|---|---|
| `aarch64-darwin` | `macos-15` |
| `x86_64-darwin` | `macos-15-intel` |
| `x86_64-linux` | `ubuntu-24.04` |
| `aarch64-linux` | `ubuntu-24.04-arm` |

Projects may expose fewer systems when their implementation is genuinely
platform-specific. Transcribe, for example, may remain Darwin-only when its
runtime requires Apple Silicon.

The matrix builds native packages. Cross-compilation is not a substitute for
the native release proof unless a project separately demonstrates identical
Nix outputs and runtime behavior.

## GitHub Actions Trust Contract

1. Pin every third-party action to a reviewed full commit SHA.
2. Give each job explicit minimum `permissions`.
3. Use a per-cache Cachix write token, not a personal account token.
4. Store the token as `CACHIX_AUTH_TOKEN`.
5. Pull requests from forks are read-only and never receive the token.
6. Only trusted development-branch and release workflows publish.
7. Never run unreviewed pull-request code in a job holding the write token.
8. Managed Cachix signing is the default; do not add a long-lived signing key
   unless a concrete requirement outweighs the additional secret.
9. Document token rotation and revoke the token after suspected workflow or
   maintainer compromise.

Because a writer can publish binaries trusted by consumers, cache write access
is equivalent to release authority.

## Producer Flake Contract

Each public producer:

1. commits `flake.nix` and `flake.lock`;
2. owns its default package's `nixpkgs` revision;
3. exposes the package for every supported system;
4. uses a Git-tracked source tree and deterministic language lock files;
5. keeps source-build instructions functional;
6. advertises the public cache for standalone use:

```nix
nixConfig = {
  extra-substituters = [ "https://<cache>.cachix.org" ];
  extra-trusted-public-keys = [ "<cache public key>" ];
};
```

Nix may require users to approve flake-provided configuration. Documentation
must explain the cache trust boundary and provide both:

```bash
nix run github:flowerornament/<project>/release
nix profile add github:flowerornament/<project>/release
```

Users may instead configure the cache persistently with `cachix use <cache>`.

Do not rely on an input flake's `nixConfig` to establish host-wide policy.
System consumers configure the cache explicitly.

## Consumer Contract

### `.nix-config`

After a producer is cache-ready:

1. use `github:` for GitHub-hosted public flakes;
2. track `refs/heads/release` for stable packages;
3. remove `inputs.nixpkgs.follows = "nixpkgs"` for independent cached package
   inputs;
4. configure the Cachix endpoint and public key through Determinate's supported
   custom Nix settings;
5. consume the producer's package output or module default;
6. retain package overrides only where intentionally rebuilding against the
   host package set;
7. remove duplicate `*-dev` remote inputs when a lazy local wrapper replaces
   them.

A normal `nx upgrade` continues to run `nix flake update`, so each mutable
release input resolves to the newest cache-ready revision. No Cachix-specific
update logic belongs in nx.

### Local development

Development wrappers resolve local checkouts only at invocation:

```bash
nix run "path:$HOME/code/nx-rs" -- "$@"
nix run "path:$HOME/code/anneal" -- "$@"
```

The implementation must handle shell quoting and missing directories with a
clear error. The local paths are not flake inputs of the stable system.

## Source Transport

Public GitHub repositories use the `github:` flake fetcher. It retrieves
archive snapshots rather than Git history and keeps lock identity in
`flake.lock`.

Private repositories require an explicit policy:

- public Cachix publication, accepting binary disclosure;
- authenticated private cache;
- or local/source building.

Do not publish a private package output to the public cache by default.
`torrent-getter` remains outside the public migration until `nx-rs-xr3c` is
resolved.

## Anneal Pilot

The pilot is deliberately end to end. A successful CI upload alone is not
enough.

### Phase A: foundation

1. Create the public Cachix cache.
2. Record its endpoint and managed public key.
3. Generate a cache-scoped write token.
4. Store the token only in the Anneal repository for the pilot.
5. Verify unauthenticated read access.
6. Record initial cache usage.

The Cachix CLI was installed locally on 2026-07-31 as
`cachix 1.11.1` through the user's versioned Nix profile. `cachix doctor`
currently reports no authentication and no configured caches, as expected.

This profile entry is bootstrap tooling, not permanent unmanaged machine state.
During `nx-rs-ibnj`, add Cachix to the declarative system package set and remove
the profile entry after the declarative executable is active. Do not leave two
independently updated Cachix installations on `PATH`.

### First implementation slice: one identity test

Before creating credentials or changing CI, extend Anneal's existing
`scripts/test-home-manager-module.sh` contract with one failing assertion:

```text
the package installed by the unoverridden Home Manager module has the same
drvPath as packages.<currentSystem>.default
```

The test evaluates the module through the producer flake, reads the installed
package from `home.packages`, and compares exact derivation paths. A matching
name or version is not sufficient.

The first implementation change then makes only the smallest producer-side
flake/module change needed to pass that assertion while preserving the existing
package override tests. Run the complete Anneal gate after it passes. Stop and
review the resulting derivation identity before creating the cache, adding
secrets, or writing a publication workflow.

### Phase B: producer identity

1. Preserve Anneal's standalone package as the canonical derivation.
2. Change its Home Manager module default to that package.
3. Retain and test `programs.anneal.package`.
4. Ensure the producer lock is committed and release-controlled.
5. Add assertions that all advertised package systems exist.

### Phase C: native CI and publication

1. Build the supported matrix from the producer lock.
2. Run existing Rust and installer gates unchanged.
3. Publish only each runtime output to Cachix.
4. Pin each output with bounded release retention.
5. Query Cachix for each expected output path.
6. Record build duration, uploaded size, and cache size.

### Phase D: release gate

1. Make cache verification a prerequisite of the Anneal release operation.
2. Prove a failed or absent cache output leaves the existing `release` branch
   unchanged.
3. Publish a pilot release through the real path.

### Phase E: external consumer proof

From a clean GitHub-hosted runner or disposable store:

1. configure only `cache.nixos.org` and the public Cachix cache;
2. evaluate the stable Anneal release;
3. assert the requested output path equals the CI output path;
4. run `nix build` and `nix run`;
5. prove the Anneal output is substituted rather than built;
6. verify `anneal --version`.

### Phase F: Ishikawa proof

1. update only the Anneal input in a disposable `.nix-config` transaction;
2. assert its output path equals the cached producer path;
3. run nx binary-cache preflight;
4. prove Anneal is not listed under source builds;
5. perform the normal rebuild;
6. verify the installed executable and Home Manager configuration;
7. verify a dirty `~/code/anneal` checkout cannot affect the stable rebuild.

Only after all six phases pass may `nx-rs-8ah3` close and the remaining
migration beads become actionable.

## Verification Commands

Representative checks, adjusted for the active system and output:

```bash
# Producer identity
nix eval --raw .#packages.aarch64-darwin.default.drvPath
nix build --no-link --print-out-paths .#packages.aarch64-darwin.default

# Cache presence
nix path-info --store https://<cache>.cachix.org <output-path>

# Standalone behavior
nix run github:flowerornament/anneal/release -- --version

# Consumer identity
nix eval --impure --raw --expr '
  let f = builtins.getFlake "/Users/morgan/.nix-config";
  in f.inputs.anneal.packages.aarch64-darwin.default.drvPath
'

# Host admission
nx rebuild --preflight
```

Tests must compare exact derivation and output paths. Matching package names or
versions is insufficient.

## Rollout After The Pilot

The migration order is:

1. extract only the workflow structure proven common by Anneal
   (`nx-rs-h72d`);
2. migrate nx-rs and its release process (`nx-rs-f5ti`);
3. migrate public storage-planner and transcribe packages, then other public
   CLIs (`nx-rs-3wr3`);
4. update `.nix-config` consumption only after each producer is independently
   cache-ready (`nx-rs-ibnj`);
5. resolve private package policy separately (`nx-rs-xr3c`).

Do not create a generalized workflow before the pilot. Duplication in one
pilot is cheaper than preserving an unproven abstraction across every
repository.

## Observability

Each producer release reports:

- source revision and semantic version;
- target system;
- derivation path;
- output path;
- whether the build substituted or built;
- cache publication result;
- cache presence verification;
- pin name and retention;
- elapsed build and upload time.

`nx upgrade` remains responsible for consumer-side closure coverage. It should
not gain Cachix APIs, cache credentials, release discovery, or cache mutation.

## Recovery

- **CI build fails**: do not publish the release channel.
- **Cachix push fails**: preserve the built output in CI logs where practical,
  but do not publish the channel.
- **Cache verification fails**: retry only the idempotent upload or query;
  never advance the channel on an unverified path.
- **Cache unavailable to a consumer**: Nix may build from source.
- **Free tier pressure**: inspect measured cache usage, remove obsolete
  unpinned paths through Cachix policy, or purchase capacity; do not silently
  stop pinning current releases.
- **Compromised write token**: revoke it, stop releases, rotate the token,
  inspect published paths, and republish trusted current outputs before
  resuming.
- **Bad release**: move the release channel only through the documented release
  process to a previously tagged, cache-ready revision.

## Non-Goals

- Turning nx into a package registry or Cachix client.
- Automatically upgrading local `~/code` repositories.
- Replacing semantic version tags with the mutable release branch.
- Requiring Cachix for source builds.
- Publishing dev shells or every CI store path.
- Solving private package distribution before its disclosure decision.
- Building a central `.nix-config` CI system that owns producer packages.
- Replacing GitHub release archives used by non-Nix installers.
- Introducing a reusable workflow before the Anneal pilot proves the contract.

## Alternatives Considered

### GitHub Actions artifacts

Useful for downloads within workflows and GitHub releases, but not a Nix
substituter available to `nx upgrade`.

### Magic Nix Cache alone

Useful inside GitHub Actions but explicitly unavailable as a public cache.

### FlakeHub Cache

Strong repository-scoped authentication and trusted-CI publication, but
currently paid-only and requires workstation authentication. Revisit if
private distribution or Cachix operations become a material burden.

### GitHub release binaries as Nix package inputs

Good parallel distribution for non-Nix users, but fixed-output hash and release
ordering add another manifest contract. Keep existing release archives; do not
make them the primary Nix substitution mechanism.

### Consumer-owned nixpkgs for every package

Idiomatic for overlays and tightly integrated modules, but deliberately
produces consumer-specific package identities. Preserve it as an explicit
override, not the default release path.

### Stable packages from `~/code`

Fast locally but allows dirty or broken worktrees to alter system evaluation.
Use local paths only for lazily invoked development commands.

### Self-hosted binary cache

Avoids a hosted service but adds storage, signing, retention, availability, and
operational ownership. It is not justified before the hosted pilot measures a
real constraint.

## Acceptance

This specification is accepted when:

- the bead dependency graph encodes foundation, pilot, and blocked rollout;
- Cachix CLI installation is verified without silently configuring trust or
  credentials;
- the Anneal pilot has explicit identity, publication, release, external, and
  Ishikawa proofs;
- all project migrations are blocked on that pilot;
- private package publication is fail-closed;
- cache trust and CI secret authority are explicit;
- source builds remain a supported fallback;
- the user approves execution after reviewing this contract.

## References

- Nix flakes and `follows`:
  <https://nix.dev/manual/nix/2.24/command-ref/new-cli/nix3-flake.html>
- Nix GitHub archive fetcher:
  <https://nix.dev/manual/nix/2.22/command-ref/new-cli/nix3-flake>
- Nix custom binary caches:
  <https://nix.dev/guides/recipes/add-binary-cache.html>
- Nix CI with Cachix:
  <https://nix.dev/guides/recipes/continuous-integration-github-actions.html>
- Cachix getting started:
  <https://docs.cachix.org/getting-started>
- Cachix GitHub Action:
  <https://github.com/cachix/cachix-action>
- Cachix runtime closure publication:
  <https://docs.cachix.org/pushing>
- Cachix security:
  <https://docs.cachix.org/security>
- Cachix retention and pins:
  <https://docs.cachix.org/garbage-collection>
  and <https://docs.cachix.org/pins>
- Cachix pricing:
  <https://www.cachix.org/pricing>
- GitHub-hosted runner systems:
  <https://docs.github.com/en/actions/reference/runners/github-hosted-runners>
- GitHub Actions billing:
  <https://docs.github.com/en/billing/concepts/product-billing/github-actions>
- GitHub Actions security:
  <https://docs.github.com/en/actions/reference/security/secure-use>
- Magic Nix Cache scope:
  <https://github.com/marketplace/actions/magic-nix-cache>
- FlakeHub Cache:
  <https://docs.determinate.systems/flakehub/cache/>
