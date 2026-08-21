#!/usr/bin/env bash
# ── Engram by El AI Intelligence — Install Script ─────────────────────────────────────
#
# One-command install:
#   curl -fsSL https://engram.ellmstack.dev/install.sh | bash
#
# Or with options:
#   curl -fsSL https://engram.ellmstack.dev/install.sh | bash -s -- --version 0.1.0 --dir ~/bin
#
# Installs the `engram` and `engramd` binaries. Tries:
#   1. cargo install (if Rust toolchain is available)
#   2. Pre-built binary download from GitHub Releases (fallback)
#
# Environment variables:
#   ENGRAM_INSTALL_DIR  — where to install (default: ~/.local/bin)

set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────────────
REPO="El-AI-Intelligence/engram"
DEFAULT_VERSION="latest"
DEFAULT_INSTALL_DIR="$HOME/.local/bin"
BOLD="\033[1m"
GREEN="\033[32m"
YELLOW="\033[33m"
RED="\033[31m"
RESET="\033[0m"

# ── Argument parsing ─────────────────────────────────────────────────────────
VERSION="$DEFAULT_VERSION"
INSTALL_DIR="${ENGRAM_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --dir) INSTALL_DIR="$2"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        --help|-h)
            echo "Usage: $0 [options]"
            echo "  --version VERSION   Install a specific version (default: latest)"
            echo "  --dir DIR           Install directory (default: ~/.local/bin)"
            echo "  --dry-run           Show what would be installed, don't do it"
            echo "  --help              Show this help"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo ""
echo -e "  ${BOLD}🧠 Engram by El AI Intelligence — Install${RESET}"
echo "  Your AI deserves a memory."
echo ""

# ── Method 1: cargo install (preferred) ──────────────────────────────────────
if command -v cargo &>/dev/null && cargo --version &>/dev/null; then
    echo -e "  ${GREEN}✓${RESET} Rust toolchain detected. Installing via cargo..."
    echo ""
    if [ "$DRY_RUN" = true ]; then
        echo "  [dry-run] cargo install engramd"
    else
        cargo install engramd
    fi
    echo ""
    echo -e "  ${GREEN}✅ Engram by El AI Intelligence installed!${RESET}"
    echo ""
    echo "  Quick start:"
    echo "    engram init         Set up your vault"
    echo "    engram daemon       Start the memory server"
    echo "    engram demo         See the demo"
    echo ""
    echo "  Or open the UI: engram daemon → http://localhost:8787"
    echo ""
    exit 0
fi

echo -e "  ${YELLOW}!${RESET} Rust not detected. Installing pre-built binary..."
echo ""

# ── Method 2: Pre-built binary ───────────────────────────────────────────────
# Detect platform
case "$(uname -s)" in
    Linux)  OS="linux" ;;
    Darwin) OS="darwin" ;;
    *)
        echo -e "  ${RED}✗${RESET} Unsupported OS: $(uname -s)"
        echo "  Engram by El AI Intelligence supports Linux and macOS."
        echo "  On Windows, open PowerShell and run:"
        echo "    iex (irm https://engram.ellmstack.dev/install.ps1)"
        echo "  (or install from source: cargo install engramd)"
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)
        echo -e "  ${RED}✗${RESET} Unsupported architecture: $(uname -m)"
        echo "  Engram by El AI Intelligence supports x86_64 and arm64."
        exit 1
        ;;
esac

# Build platform tag matching release artifact naming convention
PLATFORM_TAG="${OS}-${ARCH}"

# Resolve latest version from GitHub API
if [ "$VERSION" = "latest" ]; then
    echo "  Resolving latest release..."
    LATEST_URL="https://api.github.com/repos/${REPO}/releases/latest"
    if command -v curl &>/dev/null; then
        VERSION=$(curl -fsSL "$LATEST_URL" 2>/dev/null | grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"/\1/')
    elif command -v wget &>/dev/null; then
        VERSION=$(wget -qO- "$LATEST_URL" 2>/dev/null | grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"/\1/')
    fi
    if [ -z "$VERSION" ]; then
        echo -e "  ${RED}✗${RESET} Could not determine latest version."
        echo "  Install Rust and use: cargo install engramd"
        echo "  Or specify a version: curl ... | bash -s -- --version v0.1.0"
        exit 1
    fi
    echo "  → Latest: $VERSION"
fi

# Strip 'v' prefix if present
VERSION_CLEAN="${VERSION#v}"

TARBALL="engramd-${PLATFORM_TAG}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${TARBALL}"

echo "  Platform: ${PLATFORM_TAG}"
echo "  Download: ${DOWNLOAD_URL}"
echo "  Install:  ${INSTALL_DIR}"
echo ""

if [ "$DRY_RUN" = true ]; then
    echo "  [dry-run] Would download and extract to ${INSTALL_DIR}"
    exit 0
fi

# Download
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

echo -n "  Downloading..."
if command -v curl &>/dev/null; then
    curl -fsSL "$DOWNLOAD_URL" -o "$TMPDIR/$TARBALL" || {
        echo ""
        echo -e "  ${RED}✗${RESET} Download failed. The binary may not exist for your platform."
        echo "  Install Rust and use: cargo install engramd"
        exit 1
    }
else
    wget --fail -q "$DOWNLOAD_URL" -O "$TMPDIR/$TARBALL" || {
        echo ""
        echo -e "  ${RED}✗${RESET} Download failed."
        echo "  Install Rust and use: cargo install engramd"
        exit 1
    }
fi
echo -e " ${GREEN}OK${RESET}"

# Extract
echo -n "  Extracting..."
mkdir -p "$TMPDIR/extract"
tar xzf "$TMPDIR/$TARBALL" -C "$TMPDIR/extract"
echo -e " ${GREEN}OK${RESET}"

# Install
echo -n "  Installing..."
mkdir -p "$INSTALL_DIR"
cp "$TMPDIR/extract/engram" "$INSTALL_DIR/engram"
cp "$TMPDIR/extract/engramd" "$INSTALL_DIR/engramd"
chmod +x "$INSTALL_DIR/engram" "$INSTALL_DIR/engramd"
echo -e " ${GREEN}OK${RESET}"

# Check PATH
if ! echo "$PATH" | tr ':' '\n' | grep -qxF "$INSTALL_DIR"; then
    echo ""
    echo -e "  ${YELLOW}⚠${RESET}  ${INSTALL_DIR} is not in your PATH."
    echo ""
    echo "  Add this to your shell config:"
    echo ""
    echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo ""
    echo "  Then restart your terminal, or run:"
    echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo ""
fi

# Verify
"$INSTALL_DIR/engram" --version 2>/dev/null || true

echo ""
echo -e "  ${GREEN}✅ Engram by El AI Intelligence installed!${RESET}"
echo ""
echo "  Quick start:"
echo "    engram init         Set up your vault"
echo "    engram daemon       Start the memory server"
echo "    engram demo         See the demo"
echo ""
echo "  Or open the UI: engram daemon → http://localhost:8787"
echo ""
