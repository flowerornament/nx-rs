#!/usr/bin/env bash
# Install nx — Nix configuration repo helper
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/flowerornament/nx-rs/main/install.sh | bash

set -euo pipefail

REPO="flowerornament/nx-rs"
BINARY_NAME="nx"
INSTALL_DIR="${INSTALL_DIR:-${BIN_DIR:-$HOME/.local/bin}}"
REQUESTED_TAG=""
DRY_RUN=false
CUSTOM_BASE_URL="${NX_RS_INSTALL_BASE_URL:-}"
SUPPORTED_RELEASE_TARGETS=(
    "aarch64-apple-darwin"
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu"
)

info()  { printf '\033[1;34m%s\033[0m\n' "$*"; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

print_help() {
    cat <<'EOF'
Install nx — Nix configuration repo helper

Usage:
  install.sh [OPTIONS]

Options:
  --install-dir PATH  Install to PATH instead of ~/.local/bin
  --tag TAG           Install a specific release tag (for example v1.3.0)
  --print-target      Print the detected release target and exit
  --dry-run           Print the install plan without downloading or writing
  -h, --help          Show this help

Environment:
  INSTALL_DIR         Install directory override
  BIN_DIR             Alias for INSTALL_DIR

Examples:
  curl -fsSL https://raw.githubusercontent.com/flowerornament/nx-rs/main/install.sh | bash
  curl -fsSL https://raw.githubusercontent.com/flowerornament/nx-rs/main/install.sh | INSTALL_DIR="$HOME/bin" bash
  curl -fsSL https://raw.githubusercontent.com/flowerornament/nx-rs/main/install.sh | bash -s -- --install-dir "$HOME/bin"
  curl -fsSL https://raw.githubusercontent.com/flowerornament/nx-rs/main/install.sh | bash -s -- --dry-run
EOF
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || error "Missing required command: $1"
}

source_install_hint() {
    cat >&2 <<'EOF'
Install from source:
  git clone https://github.com/flowerornament/nx-rs.git
  cargo install --path nx-rs --locked --bin nx
EOF
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --install-dir)
            [ "$#" -ge 2 ] || error "--install-dir requires a path"
            INSTALL_DIR="$2"
            shift 2
            ;;
        --tag)
            [ "$#" -ge 2 ] || error "--tag requires a value"
            REQUESTED_TAG="$2"
            shift 2
            ;;
        --print-target)
            PRINT_TARGET=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            print_help
            exit 0
            ;;
        *)
            error "Unknown option: $1 (run with --help)"
            ;;
    esac
done

require_cmd uname
require_cmd curl
require_cmd tar
require_cmd mktemp

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin) os="apple-darwin" ;;
    Linux)  os="unknown-linux-gnu" ;;
    *)      error "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) error "Unsupported architecture: $ARCH" ;;
esac

TARGET="${arch}-${os}"

if [ "${PRINT_TARGET:-false}" = true ]; then
    printf '%s\n' "$TARGET"
    exit 0
fi

supported=false
for supported_target in "${SUPPORTED_RELEASE_TARGETS[@]}"; do
    if [ "$TARGET" = "$supported_target" ]; then
        supported=true
        break
    fi
done

if [ "$supported" != true ]; then
    printf 'error: No prebuilt binary is published for %s.\n' "$TARGET" >&2
    source_install_hint
fi

if [ -n "$CUSTOM_BASE_URL" ]; then
    TAG="${REQUESTED_TAG:-local}"
    DOWNLOAD_BASE="$CUSTOM_BASE_URL"
elif [ -n "$REQUESTED_TAG" ]; then
    TAG="$REQUESTED_TAG"
    DOWNLOAD_BASE="https://github.com/$REPO/releases/download/$TAG"
else
    info "Finding latest release..."
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)
    DOWNLOAD_BASE="https://github.com/$REPO/releases/download/$TAG"
fi

if [ -z "$TAG" ]; then
    printf 'error: No releases found.\n' >&2
    source_install_hint
fi

URL="$DOWNLOAD_BASE/${BINARY_NAME}-${TARGET}.tar.gz"
DEST="$INSTALL_DIR/$BINARY_NAME"

info "Install plan"
printf '  release: %s\n' "$TAG"
printf '  target:  %s\n' "$TARGET"
printf '  url:     %s\n' "$URL"
printf '  dest:    %s\n' "$DEST"

if [ "$DRY_RUN" = true ]; then
    info "Dry run complete. No changes made."
    exit 0
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" -o "$TMPDIR/${BINARY_NAME}.tar.gz" || error "Download failed. Check that a binary exists for $TARGET"
tar xzf "$TMPDIR/${BINARY_NAME}.tar.gz" -C "$TMPDIR"

mkdir -p "$INSTALL_DIR"
mv "$TMPDIR/$BINARY_NAME" "$DEST"
chmod +x "$DEST"

info "Installed to $DEST"

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    echo ""
    echo "Add to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi

echo ""
"$DEST" --help >/dev/null 2>&1 || true
info "Done. Run 'nx --help' to get started."
