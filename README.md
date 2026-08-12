# nx-rs

`nx-rs` builds `nx`, a Rust CLI for working with a personal Nix configuration
repo.

## What Is `nx`?

`nx` sits between raw `nix`, `darwin-rebuild`, Homebrew, and your config repo.
It knows where packages, services, secrets, Homebrew manifests, and host
settings live, then turns common maintenance workflows into short, repeatable
commands.

Use it when your machine is managed as code and you want one CLI for daily work:

- inspect what your Nix repo declares and where each package lives
- add or remove packages across nixpkgs, Homebrew, casks, MAS apps, and service
  manifests
- run repo preflight checks and rebuild the host configuration
- upgrade flake inputs, Homebrew packages, and the system in one flow
- diagnose common Nix failures, such as lazy source cache misses and
  file-descriptor exhaustion, and safely repair fixed-output hash drift
- inspect rebuild timings, prune old generations, and clean local development
  caches

`nx` does not replace Nix. It is a repo-aware command layer for people who
already keep their system and home configuration in a flake-backed Nix repo.

## Repository Model

Most commands operate on a managed repo. `nx` finds it by walking up from the
current directory until it sees `flake.nix`. Set `NX_REPO_ROOT` only when you
want to target a different repo.

Run `nx init` once to scan the repo and write `.nx/manifest.toml`. The manifest
records the files `nx` should use for package lists, services, Homebrew
manifests, rebuild behavior, and local package alias hints. Run
`nx init --refresh` after moving files; refresh preserves hand-edited alias
entries while filling in known defaults for newly discovered packages.

Host maintenance commands under `nx generations` and `nx clean-caches` are
intentionally host-scoped and can run from any directory.

## Usage

### First Commands

After installing `nx`, start with:

```bash
nx doctor         # check repo health and the local Nix substrate
nx init           # create .nx/manifest.toml for the current config repo
nx status         # summarize declared packages by source
nx where ripgrep  # jump to the file that declares a package
nx rebuild        # run preflight checks and switch the system
nx upgrade        # update inputs/Homebrew, rebuild, and commit changes
```

### Install

#### Nix

For a one-off install, use `nix run` or `nix profile install`. If you want
persistent session defaults like `NX_REPO_ROOT`, prefer the Home Manager module.

Run without installing:

```bash
nix run github:flowerornament/nx-rs/release -- --help
```

Install into your profile:

```bash
nix profile install github:flowerornament/nx-rs?ref=refs/heads/release
```

Add `nx-rs` to your Nix configuration repository:

```nix
# flake.nix inputs
nx-rs.url = "github:flowerornament/nx-rs?ref=refs/heads/release";

# package list/module
inputs.nx-rs.packages.${pkgs.system}.default
```

Then rebuild:

```bash
nix flake update nx-rs
sudo /nix/var/nix/profiles/system/sw/bin/darwin-rebuild switch --flake .
```

The `release` branch is a moving branch maintained by the release process. It
points at the latest tagged release; the default branch is for development.
The producer's committed `flake.lock` defines the cached package identity, so
consumers should not make `nx-rs.inputs.nixpkgs` follow another input.

The flake advertises the public `flowerornament.cachix.org` cache and its
trusted key. Nix may ask you to approve that configuration on first use. The
cache is an acceleration layer: when an output is unavailable, the same flake
still builds correctly from source. To configure the cache persistently, use
`cachix use flowerornament` or your system's supported Nix settings.

#### Nix + Home Manager

This is the recommended install path for declarative Nix users. The module
installs `nx` into `home.packages` and can export the same environment variables
that drive `nx` at runtime.

Add the flake input:

```nix
nx-rs.url = "github:flowerornament/nx-rs?ref=refs/heads/release";
```

Then include the module in your Home Manager configuration:

```nix
{
  imports = [
    inputs.nx-rs.homeManagerModules.default
  ];

  programs.nx = {
    enable = true;
    repoRoot = "/Users/alice/code/nix-config";
    autoRefresh = false;
    cleanCaches.skip = [ "huggingface" ];
  };
}
```

Available module options:

- `programs.nx.enable`
- `programs.nx.package`
- `programs.nx.repoRoot`
- `programs.nx.autoRefresh`
- `programs.nx.sops.package`
- `programs.nx.sops.bin`
- `programs.nx.cleanCaches.codeRoots`
- `programs.nx.cleanCaches.scanDepth`
- `programs.nx.cleanCaches.skip`

The module only manages binary installation and environment variables. Repo
structure, manifests, and command behavior stay in the target Nix config repo.

That boundary is intentional:

- the managed repo owns `nx` behavior over your Nix configuration
- `programs.nx` owns machine/session defaults only
- CLI flags still own one-off overrides

If you use `nx secret add`, you can wire `sops` declaratively too:

```nix
{
  imports = [
    inputs.nx-rs.homeManagerModules.default
  ];

  programs.nx = {
    enable = true;
    sops.package = pkgs.sops;
    sops.bin = "${pkgs.sops}/bin/sops";
  };
}
```

### Configure Repository Root

`nx` auto-detects the managed repo by walking up from the current directory
until it finds `flake.nix`. Set `NX_REPO_ROOT` only when you want to override
that detection:

```bash
export NX_REPO_ROOT=/path/to/your/config-repo
```

### LLM-Assisted Functions

Some flows can call local AI CLIs when helpful:

- `install`:
  - `--engine codex|claude` selects the engine (`codex` default).
  - `--model <name>` selects the model for the chosen engine.
  - `--explain` prints extra decision details during install resolution.
- `remove`:
  - `--model <name>` sets the Claude model used for AI fallback edits.
- `upgrade`:
  - Prints optional AI-generated change summaries for flake/homebrew updates.
  - `--no-ai` disables AI summaries entirely.

Requirements:

- `codex` and/or `claude` must be installed and available on `PATH` for the
  corresponding features.
- Authentication and provider-specific environment variables are managed by
  those CLIs.

### Environment Variables

`nx` reads these optional environment variables:

- `NX_REPO_ROOT`: target repo override; defaults to walking up from `cwd` until
  `flake.nix`.
- `NX_PROFILE_PATH`: rebuild/upgrade timing file; defaults to
  `~/.local/state/nx/timings.jsonl`.
- `NX_RS_SOPS_BIN`: `sops` executable for `nx secret add`; defaults to `sops`.
- `NX_RS_AUTO_REFRESH`: auto-refresh local cargo-installed `nx` binaries before
  `rebuild` and `upgrade`; set to `0`, `false`, or `no` to disable.
- `NX_NO_AUTO_HASH_FIX`: disable fixed-output hash repair during `nx rebuild`
  and `nx upgrade`; set to `1`, `true`, `yes`, or `on`.
- `NX_CODE_ROOTS`: colon-separated roots scanned when project build-artifact
  caches are selected explicitly; defaults to `~/code`. Use an empty string to
  disable code-root scans.
- `NX_CLEAN_SCAN_DEPTH`: maximum `nx clean-caches` code-root scan depth;
  defaults to `3`; values above `8` are clamped.
- `NX_CLEAN_SKIP`: comma-separated cache names for `nx clean-caches` to skip.
- `NO_COLOR`: disable colored output when set.
- `TERM`: if set to `dumb`, color output is disabled.

### Examples

Repository setup and discovery:

```bash
# Scan the current repo and write .nx/manifest.toml
nx init

# Show the config location for a package
nx where ripgrep

# Show package distribution across nix/homebrew/mas sources
nx status

# Review packages with little local evidence of recent use
nx unused
```

Package changes:

```bash
# Bare package names are treated as "install"
nx ripgrep
# Likely command typos are left as command errors with suggestions
nx rebulid

# Explicit install flows
nx install ripgrep fd
nx install --cask firefox
nx install --mas Tailscale
nx install --dry-run --bleeding-edge neovim

# Remove installed packages
nx remove ripgrep
nx remove --dry-run firefox
```

Read-only inspection:

```bash
nx search ripgrep
nx info ripgrep
nx list --plain
nx installed ripgrep fd
nx unused --since 30d
```

System and host maintenance:

```bash
nx lint
nx rebuild --preflight
nx rebuild --timing
nx rebuild
nx profile --limit 20
nx upgrade
nx upgrade nx-rs
nx generations status
nx generations plan
nx generations prune --dry-run
nx clean-caches --dry-run
nx clean-caches rust-targets node-modules
nx clean-caches nix-gc --dry-run
```

Secrets:

```bash
nx secret add github_token --value-stdin
printf '%s' "$TOKEN" | nx secret add github_token --value-stdin
```

### Command Guide

Use command help for the full flag surface:

```bash
nx --version
nx --help
nx help <topic>
nx <command> --help
```

---

#### `version`

Shows the current `nx` version.

- Supports `nx --version`, `nx -V`, and `nx version`.
- Supports `nx version --json`.

---

#### `help`

Shows hierarchical help for commands and flags. Use `nx help install`,
`nx help secret add`, or `nx help -- --dry-run` when you want the command tree.

---

#### `completion`

Generates shell completion scripts to stdout.

- Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.
- Example: `nx completion zsh > ~/.zsh/completions/_nx`.

---

#### `doctor`

Diagnoses repo and host prerequisites that affect normal `nx` usage.

- Reports repo root resolution, manifest health, routing status, flake lock
  state, cache availability, and tool presence.
- Reports public Nix substrate health: installed distribution/version, daemon
  and configuration checks, Determinate freshness, effective `lazy-trees`,
  garbage-collector strategy, and observed `/nix` filesystem availability.
- Reserves substrate warnings for actionable conditions; optional settings and
  ordinary disk availability remain informational.
- Remains read-only: it never upgrades Nix, clears private caches, or runs
  garbage collection.
- Supports `--verbose` for extra detail.
- Supports `--json` for automation.

---

#### `init`

Scans the current repo and writes `.nx/manifest.toml`. Use `--refresh` to
rescan and merge with an existing manifest. Init also seeds known package alias
hints, such as `rg = "ripgrep"`, when matching packages are declared.

---

#### `install`

Installs one or more packages into the managed repo. Bare tokens become package
installs, so `nx ripgrep` means `nx install ripgrep`. Likely command typos such
as `nx rebulid` stay command errors so `nx` can suggest the intended command.

- Use `--dry-run` to preview edits without writing files.
- Use `--yes` to skip interactive prompts and accept defaults.
- Use `--cask` or `--mas` to force macOS package targets.
- Use `--bleeding-edge`, `--nur`, or `--source` to bias source selection.
- Use `--service` to offer service scaffolding after install.
- Use `--rebuild` to rebuild after successful changes.
- Use `--engine`, `--model`, and `--explain` for AI-assisted routing/edit flows.
- Use `--verbose` to surface cache and query timing diagnostics.
- Source resolution shows live loading feedback while package queries run.

---

#### `remove` / `rm` / `uninstall`

Removes installed packages from the managed repo.

- Supports `--dry-run` and `--yes`.
- Supports `--model` for AI-assisted fallback removal.
- `rm` and `uninstall` are aliases of `remove`.
- Exits `1` on the first package that cannot be found, looked up, or removed; successful removals and no-change outcomes such as dry-runs, cancellations, and successful AI no-ops exit `0`.

---

#### `secret add`

Adds or updates an encrypted secret via `sops`.

- Accepts a positional key or `--name`.
- Accepts `--value` or `--value-stdin`.
- Prefer `--value-stdin` to avoid leaving secrets in shell history.
- Top-level command aliases: `secret`, `secrets`.

---

#### `search`

Searches package sources without editing the repo.

- Good first step when you are not sure which backend will provide a package.
- Supports `--bleeding-edge`, `--nur`, `--source`, `--json`, and `--verbose`.
- Shows live loading feedback while package sources are queried.

---

#### `where`

Shows where a package is declared in the managed repo. Use it to jump to the
owning file before a manual edit or review.

---

#### `list`

Lists installed packages by source bucket.

- Optional source filter: `nix`, `homebrew`, or `mas`.
- Supports `--verbose`, `--json`, and `--plain`.

---

#### `info`

Shows package metadata plus candidate source information.

- Useful when deciding between nixpkgs, NUR, flake input, Homebrew, or MAS
  routes.
- Supports `--json`, `--bleeding-edge`, `--nur`, `--source`, and `--verbose`.
- Shows live loading feedback while package metadata and source candidates are
  collected.

---

#### `status`

Shows a read-only package distribution summary for the managed repo.

- Supports `--json`.

---

#### `unused`

Runs a read-only advisory audit for declared packages with little local evidence
of recent use.

- Supports `--since`, `--source`, `--limit`, `--include-protected`, `--json`,
  `--verbose`, `--history`, `--no-history`, and `--no-spotlight`.
- Scans shell history locally; timestamped entries are stronger evidence, while
  untimestamped entries are shown as evidence without claiming recency. It does
  not store raw commands, send telemetry, or auto-remove packages.
- Uses `.nx/manifest.toml` `[aliases]` as local command evidence hints. `nx init`
  seeds known aliases, and hand-written entries are preserved on refresh. For
  example, `rg = "ripgrep"` makes `rg ...` count as evidence for `ripgrep`.
- Rows are review candidates, not proof. Use `nx where <name>` and
  `nx remove --dry-run <name>` before removing anything.

---

#### `installed`

Checks whether one or more packages are currently installed.

- Supports `--json`.
- Supports `--show-location` to include file and line matches.
- Returns success only when all requested packages are installed.

---

#### `lint`

Checks `# nx:` routing annotations and keyword overlap across routable `.nix`
files. Use this before reorganizing manifests or adding new routable files.

- Supports `--json`.

---

#### `undo`

Reverts modified tracked files via git checkout. Use `--yes` to skip the
confirmation prompt.

---

#### `update`

Runs `nix flake update`. In a terminal, Nix owns stderr and renders its native
colored `bar` progress unchanged. Non-interactive execution uses structured
diagnostics. Additional args after `--` pass through to the underlying flake
update invocation except `--log-format`, which nx owns.

---

#### `test`

Runs repo quality checks. This validates the managed Nix config repo itself.

---

#### `rebuild`

Runs `darwin-rebuild switch` for the managed repo.

- Use `--preflight` to stop after lint, git, and flake checks without switching.
- Use `--timing` to print phase timings after recording them locally.
- Use `--verbose` to ask Nix for full build logs during rebuild phases.
- Interactive checks, builds, profile updates, and `darwin-rebuild` use Nix's
  native colored `bar` renderer unchanged. `--verbose` selects
  `bar-with-logs`. Nx gives stderr a pseudoterminal and relays its bytes
  unchanged while retaining a bounded diagnostic tail; split-build stdout is
  captured only to read the resulting store path.
- Direct Nix commands in non-interactive runs and `--timing` use Nix's
  `internal-json` protocol for diagnostics, timing, and safe hash repair.
- If split activation would need an interactive sudo prompt and passwordless
  `sudo darwin-rebuild` is available, `nx` falls back to that path.
- Interactive activation inherits stdin and stdout and receives a
  pseudoterminal-backed stderr, so Nix, Homebrew, and Home Manager keep their
  terminal-aware progress behavior.
- Recognized lazy-tree source-cache failures preserve the original command
  failure and print the exact manual repair for the cache owner. Nx never
  clears Determinate's private caches or retries these failures automatically:

  ```bash
  # User-owned Nix invocation
  rm -rf "$HOME/.cache/nix/gitv3" "$HOME/.cache/nix/tarball-cache-v2" "$HOME/.cache/nix/fetcher-cache-v4.sqlite" "$HOME/.cache/nix/fetcher-cache-v4.sqlite-wal" "$HOME/.cache/nix/fetcher-cache-v4.sqlite-shm"

  # Root-owned Nix invocation
  sudo rm -rf /var/root/.cache/nix/gitv3 /var/root/.cache/nix/tarball-cache-v2 /var/root/.cache/nix/fetcher-cache-v4.sqlite /var/root/.cache/nix/fetcher-cache-v4.sqlite-wal /var/root/.cache/nix/fetcher-cache-v4.sqlite-shm
  ```

- Recognized file-descriptor exhaustion is qualified by Nix distribution and
  version. Determinate Nix older than `3.16.0` receives
  `sudo determinate-nixd upgrade`; other runtimes are not compared against the
  Determinate version space.
- If a structured rebuild reports a fixed-output hash mismatch, `nx rebuild`
  updates the unique clean matching hash in a tracked `.nix` file and retries.
  It prints the file, line, old hash, and new hash. Set
  `NX_NO_AUTO_HASH_FIX=1` to require a manual edit.
- Additional args after `--` pass through to the underlying `darwin-rebuild`
  command except `--log-format`, which nx owns for managed Nix output.

---

#### `profile`

Shows recent local rebuild and upgrade timing records.

- Defaults to `~/.local/state/nx/timings.jsonl`.
- Set `NX_PROFILE_PATH` to read/write a different timing file.
- Use `--json` for machine-readable records.

---

#### `upgrade`

Runs the upgrade flow for either the whole repo or named flake inputs.

- Use `--dry-run` to preview without mutating files.
- When Determinate Nix is installed, `nx upgrade` checks it before flake
  evaluation. An available update prints installed/latest versions and advises
  `sudo determinate-nixd upgrade`; nx never upgrades or restarts the daemon
  inside the pipeline.
- `nx upgrade` with no positional inputs runs the full repo-wide flow: flake
  update, binary cache admission, Homebrew update/upgrade, rebuild, and git
  commit.
- `nx upgrade <input...>` updates only the named flake inputs via
  `nix flake update <input...>`.
- Nx commits `flake.lock` and any repaired hash files once, at the end of a
  successful upgrade, using the reported root input names when available and
  leaving unrelated staged or unstaged work untouched. Byte-level lock changes
  are still committed when only metadata or transitive nodes changed. Use
  `nx update -- --commit-lock-file` when Nix should own a lock-only commit instead.
- Flake check and build realize changed sources on demand.
- Before Homebrew or rebuild, nx dry-runs the candidate system closure. Excessive
  source builds require interactive approval or `--yes`; otherwise automation
  fails closed. Rejection or an unavailable coverage check atomically restores
  the original `flake.lock`.
- Use `--yes` to preapprove an excessive source-build plan and let a long cache
  check continue unattended. It still fails closed when coverage cannot be
  established.
- Nix input-realization diagnostics and unrelated progress around recognized
  planning sections do not invalidate cache coverage. Output with no recognized
  plan remains unavailable, and concurrent upgrades are refused during admission.
- Retry unavailable coverage once after Nix realizes its inputs. Use
  `--allow-source-builds` only after independently verifying cache coverage.
- If a structured rebuild reports a fixed-output hash mismatch, `nx` updates
  the unique clean matching hash in a tracked `.nix` file, retries, and includes
  that file in the upgrade commit. It prints the file, line, old hash, and new
  hash. If repair is unsafe, it prints the exact next action.
- Homebrew update checks show live loading feedback before upgrade details are
  rendered.
- Interactive flake updates, checks, and builds preserve Nix's native colored
  `bar` progress exactly; native Nix rows are intentionally not indented by nx.
- Use `--verbose` to select Nix's native `bar-with-logs` output for direct Nix
  phases.
- Targeted input upgrades skip the Homebrew phase by default.
- Use `--skip-brew`, `--skip-rebuild`, or `--skip-commit` to trim the flow.
- Use `--no-ai` to disable AI-generated summaries and recovery prompts.
- Additional args after `--` pass through to the underlying `nix flake update`
  command except `--log-format` and `--commit-lock-file`, which nx owns.

Examples:

```bash
nx upgrade
nx upgrade --dry-run
nx upgrade nx-rs
nx upgrade nx-rs anneal -- --show-trace
```

---

#### `generations`

Host-scoped retention and garbage-collection commands. They work from any
directory and do not require a managed repo.

- `nx generations status` shows discovered nix-darwin and Home Manager
  generations plus the active retention policy.
- `nx generations plan` renders the exact prune and garbage-collection plan
  without mutating the host.
- `nx generations prune` executes that plan, prompting by default.
- Important flags: `--keep`, `--kind`, `--no-gc`, `--yes`, `--dry-run`.
- `nx generations prune --dry-run` renders the same plan as
  `nx generations plan`.

---

#### `clean-caches`

Scans host cache directories, reports sizes, and cleans them after confirmation.

- Supports `--dry-run` and `--yes`.
- Project build artifacts are excluded by default. Select `rust-targets`,
  `elixir-builds`, or `node-modules` explicitly to scan configured code roots.
- Explicit code-root scans default to `~/code` and depth `3`; scan depth is
  capped at `8`.
- Nix store GC is excluded by default because it can force future downloads or
  local source builds; select `nix-gc` explicitly when that is intended.
- Nx never runs Nix store GC implicitly. Explicit `nx generations prune` and
  `nx clean-caches nix-gc` operations remain available.
- Agent session history is excluded by default so routine cleanup does not
  remove resume/history data. Select `codex-sessions`, `codex-logs`, or
  `claude-file-history` explicitly when you want to clean those directories.
- Explicitly selected large code-root scans show live per-bucket loading
  feedback while sizes are computed.
- If selected, Nix GC sizing runs last because dead-store estimation can be
  slower than normal cache directory sizing.
- Confirmed cleaning shows live per-cache loading feedback before final success
  or warning lines are printed.
- Positional cache names or `--only` limit the scan and clean plan to selected
  caches.
- Code-root build artifacts report how many directories were discovered.
- Set `NX_CODE_ROOTS`, `NX_CLEAN_SCAN_DEPTH`, and `NX_CLEAN_SKIP` directly, or
  configure them declaratively with `programs.nx.cleanCaches`.
- Valid skip names: `cargo-registry`, `uv`, `npm`, `homebrew`, `huggingface`,
  `puppeteer`, `playwright`, `xcode-derived`, `core-simulator`,
  `codex-sessions`, `codex-logs`, `claude-telemetry`, `claude-file-history`,
  `nix-gc`, `rust-targets`, `elixir-builds`, `node-modules`.

---

### Behavior Notes

- `help`, `where`, `list`, `info`, `status`, `installed`, `search`,
  `generations status`, and `generations plan` are read-only.
- `version`, `completion`, and `doctor` are also read-only.
- `install` and `remove` support `--dry-run` to preview changes before writing
  files.
- `generations` commands are host-scoped and work from any directory.
- Most commands operate on the current repo root; set `NX_REPO_ROOT` only when
  you want to override auto-discovery.

## Development

### Toolchain

Rust is pinned in `rust-toolchain.toml` (`1.94.0`).

### Workflow

This repo is jj-first for local development. Git remains the GitHub, CI, and
release-tag transport layer, but day-to-day work uses Jujutsu:

```bash
jj git fetch
jj new main -m "task: short description"
jj status
jj diff
just ci
jj commit -m "area: describe the change"
jj git push --change @-
```

For direct publication to `main`, move the bookmark explicitly after committing:

```bash
jj bookmark move main --to @-
jj git push --bookmark main
```

See `AGENTS.md` for the full jj workflow, agent workspace guidance, and
recovery commands.

```bash
just --list
just doctor
just hooks-install
just compile
just ci
```

Local install for ad hoc testing:

```bash
cargo install --path .
```

### Quality Gates

Standard gate:

```bash
just ci
```

Additional strict checks:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic
just test-system
```

### Release Process

Releases are local-first and tag-driven. The release helper updates all version
files, verifies the committed release state, pushes the annotated tag, and moves
`origin/release` to the tagged commit for downstream flake consumers.

```bash
jj git fetch
jj new main -m "release: prepare vX.Y.Z"
just release-bump X.Y.Z
# Fill CHANGELOG.md and update user-facing docs for shipped behavior.
jj status
jj diff
jj commit -m "Release vX.Y.Z"
just release-verify
jj bookmark move main --to @-
jj git push --bookmark main
just cache-verify
just release-tag X.Y.Z
git ls-remote origin refs/heads/release 'refs/tags/vX.Y.Z^{}'
```

`just release-verify` must run from a clean jj working copy, then runs the full
CI gate, system matrix, release build, Home Manager and package consumer smoke
tests, and Nix build/run checks. After pushing the release commit, wait for its
four-platform Nix Cache workflow to publish and prove tokenless substitution;
`just cache-verify` checks the same outputs locally. `just release-tag` repeats
that cache gate, then uses Git to create the annotated tag and update
`origin/release`. It prompts before running; use `just --yes release-tag X.Y.Z`
only for explicit automation.

Public flowerornament producers reuse nx-rs's cache workflow at an immutable
commit SHA. Each caller owns only its supported system/runner matrix and
executable name; the shared workflow owns runtime-closure publication and the
fresh-runner, local-build-disabled substitution proof.

### Task Tracking

Work is tracked in `bd` (`./.beads`):

```bash
bd ready
bd create --title="<task>" --type=task --priority=2
bd close <id>
bd dolt push
```

## License

MIT.
