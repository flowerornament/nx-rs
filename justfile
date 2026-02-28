set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments := false

STRICT_FLAGS := "--workspace --all-targets --all-features"

default: help

# Show all high-value workflows and what they enforce.
help:
    @echo "nx-rs Workflow"
    @echo
    @echo "Task Tracking (bd)"
    @echo "  just bd-prime        # show AI workflow context for bd"
    @echo "  just bd-status       # database overview"
    @echo "  just bd-ready        # list ready work"
    @echo "  just bd-sync         # sync bd state"
    @echo
    @echo "Bootstrap"
    @echo "  just doctor          # Show local tool versions and paths"
    @echo "  just hooks-install   # Install git hooks (pre-commit strict checks)"
    @echo
    @echo "Daily Loop (strict)"
    @echo "  just compile         # Agent compile hook: fmt + clippy + test + check"
    @echo "  just guard           # Run strict gates without compile wrapper"
    @echo "  just test-system     # Run system command integration matrix with stubs"
    @echo "  just cutover-validate # Run Rust direct/canary validation against ~/.nix-config"
    @echo
    @echo "Raw Commands"
    @echo "  just fmt             # Format source"
    @echo "  just fmt-check       # Verify formatting"
    @echo "  just lint            # Clippy with -D warnings"
    @echo "  just test            # Full test suite"
    @echo "  just check           # cargo check for all targets/features"
    @echo "  just ci              # fmt-check + lint + test + check"
    @echo
    @echo "Notes"
    @echo "  - 'just compile' is the authoritative compile path for agents."
    @echo "  - Hooks intentionally fail fast on style/lint/correctness regressions."

# Print current toolchain and runner versions.
doctor:
    @echo "rustc:   $(rustc --version)"
    @echo "cargo:   $(cargo --version)"
    @echo "rustup:  $(rustup --version | head -n 1)"
    @echo "just:    $(just --version)"
    @echo "cwd:     $(pwd)"

# Install repository-local git hooks and ensure executability.
hooks-install:
    @mkdir -p .githooks
    @chmod +x .githooks/pre-commit scripts/agent-hooks/*.sh
    @git config core.hooksPath .githooks
    @echo "Installed hooks at .githooks (git core.hooksPath configured)."

# Show detailed AI-oriented bd workflow context.
bd-prime:
    @bd prime

# Show bd database status.
bd-status:
    @bd status

# Show ready work from bd.
bd-ready:
    @bd ready

# Sync bd state to storage/jsonl.
bd-sync:
    @bd sync

# Run strict pre-compile checks directly.
guard:
    @scripts/agent-hooks/pre-compile.sh

# Authoritative compile command for agents: strict checks then cargo check.
compile:
    @scripts/agent-hooks/compile.sh

# Format source files.
fmt:
    @cargo fmt --all

# Validate formatting only.
fmt-check:
    @cargo fmt --all --check

# Run strict clippy policy.
lint:
    @cargo clippy {{STRICT_FLAGS}} -- -D warnings

# Run full tests.
test:
    @cargo test {{STRICT_FLAGS}}

# Run system command integration matrix with deterministic stubs.
test-system:
    @cargo build --quiet --bin nx
    @cargo test --test system_command_matrix -- --nocapture

# Run Rust direct/canary validation against ~/.nix-config.
cutover-validate:
    @scripts/cutover/validate_shadow_canary.sh

# Run cargo check across workspace.
check:
    @cargo check {{STRICT_FLAGS}}

# Full strict CI-equivalent local gate.
ci: fmt-check lint test check
