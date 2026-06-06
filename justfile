set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments := false

STRICT_FLAGS := "--workspace --all-targets --all-features"

[private]
default:
    @just --list

# Print current toolchain and runner versions.
[group('bootstrap')]
doctor:
    @echo "rustc:   $(rustc --version)"
    @echo "cargo:   $(cargo --version)"
    @echo "rustup:  $(rustup --version | head -n 1)"
    @echo "just:    $(just --version)"
    @echo "cwd:     $(pwd)"

# Install/update bd hooks and ensure agent scripts are executable.
[group('bootstrap')]
hooks-install:
    @bd hooks install --force
    @chmod +x scripts/agent-hooks/*.sh
    @echo "Hooks installed."

# Show detailed AI-oriented bd workflow context.
[group('bd')]
bd-prime:
    @bd prime

# Show bd database status.
[group('bd')]
bd-status:
    @bd status

# Show ready work from bd.
[group('bd')]
bd-ready:
    @bd ready

# Show bd issue database stats.
[group('bd')]
bd-stats:
    @bd stats

# Run strict pre-compile checks directly.
[group('check')]
guard:
    @scripts/agent-hooks/pre-compile.sh

# Authoritative compile command for agents: strict checks then cargo check.
[group('check')]
compile:
    @scripts/agent-hooks/compile.sh

# Format source files.
[group('check')]
fmt:
    @cargo fmt --all

# Validate formatting only.
[group('check')]
fmt-check:
    @cargo fmt --all --check

# Run strict clippy policy.
[group('check')]
lint:
    @cargo clippy {{STRICT_FLAGS}} -- -D warnings

# Run full tests.
[group('check')]
test:
    @cargo test {{STRICT_FLAGS}}

# Run script/helper tests.
[group('check')]
test-scripts:
    @python3 -m unittest discover -s scripts -p 'test_*.py'

# Run system command integration matrix with deterministic stubs.
[group('check')]
test-system:
    @cargo build --quiet --bin nx
    @cargo test --test system_command_matrix --test system_init --test system_manifest_drift --test system_query --test system_commands --test system_upgrade -- --nocapture

# Run cargo check across workspace.
[group('check')]
check:
    @cargo check {{STRICT_FLAGS}}

# Build a release binary.
[group('build')]
build:
    @cargo build --release

# Full strict CI-equivalent local gate.
[group('check')]
ci: fmt-check lint test test-scripts check

# Update release versions and scaffold changelog.
[group('release')]
[arg('version', pattern='[0-9]+\.[0-9]+\.[0-9]+', help='Semver release, for example 1.5.25')]
release-bump version:
    @python3 scripts/release.py bump {{quote(version)}}

# Release-readiness checks and validation.
[group('release')]
release-verify:
    @python3 scripts/release.py verify

# Create and push an annotated release tag, then publish origin/release.
[group('release')]
[arg('version', pattern='[0-9]+\.[0-9]+\.[0-9]+', help='Semver release, for example 1.5.25')]
[confirm("This will tag, force-update origin/release, and trigger the public GitHub release workflow. Continue?")]
release-tag version:
    @python3 scripts/release.py tag {{quote(version)}}
