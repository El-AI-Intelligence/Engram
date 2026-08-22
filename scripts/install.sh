#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# install.sh — one-command installer for Engram by El AI Intelligence (engram + engramd + engramd-mcp).
#
#     curl -fsSL https://engram.ellmstack.dev/install.sh | bash
#
# Downloads the latest release for your platform from GitHub Releases (by
# default), verifies its SHA-256 checksum, and installs the binaries into
# ~/.local/bin (override with INSTALL_DIR). Override the download base with
# ENGRAM_RELEASE_BASE (e.g. the site mirror https://engram.ellmstack.dev/releases).
# Staged on the live site by deploy/engram/deploy.sh.
#
# Supported: Linux (x86_64, arm64) and macOS (x86_64, arm64). Windows uses
# install.ps1 (https://engram.ellmstack.dev/install.ps1). macOS verifies
# checksums with `shasum -a 256` (no sha256sum by default); macOS binaries are
# signed + notarized + stapled by CI, so Gatekeeper accepts them.
#
# ENGRAM_FORCE_PLATFORM=<os>-<arch> is TEST-ONLY: simulate another platform
# from any dev box to exercise the download/verify/extract paths.
#
# No sudo, no secrets, no telemetry. Everything installed is MIT open source.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

RELEASE_BASE="${ENGRAM_RELEASE_BASE:-https://github.com/El-AI-Intelligence/engram/releases/latest/download}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# ── Platform detection ──────────────────────────────────────────────────────
if [[ -n "${ENGRAM_FORCE_PLATFORM:-}" ]]; then
  os="${ENGRAM_FORCE_PLATFORM%%-*}"
  arch="${ENGRAM_FORCE_PLATFORM##*-}"
  echo "NOTE: ENGRAM_FORCE_PLATFORM=${ENGRAM_FORCE_PLATFORM} — simulating ${os}/${arch} (test-only)." >&2
else
  os="$(uname -s)"
  arch="$(uname -m)"
fi
# Normalize case (uname -s is "Darwin" on macOS; FORCE values may vary too).
os="$(printf '%s' "$os" | tr '[:upper:]' '[:lower:]')"
arch="$(printf '%s' "$arch" | tr '[:upper:]' '[:lower:]')"

case "$os-$arch" in
  linux-x86_64|linux-amd64)
    asset="engramd-linux-x86_64.tar.gz"
    hasher=(sha256sum) ;;
  linux-arm64|linux-aarch64)
    asset="engramd-linux-arm64.tar.gz"
    hasher=(sha256sum) ;;
  darwin-x86_64)
    asset="engramd-darwin-x86_64.tar.gz"
    hasher=(shasum -a 256) ;;
  darwin-arm64|darwin-aarch64)
    asset="engramd-darwin-arm64.tar.gz"
    hasher=(shasum -a 256) ;;
  *)
    echo "Prebuilt binaries ship for: Linux (x86_64, arm64) and macOS (x86_64, arm64)." >&2
    echo "Your platform: ${os}/${arch}. Windows uses install.ps1 — all assets:" >&2
    echo "  https://github.com/El-AI-Intelligence/engram/releases" >&2
    exit 1
    ;;
esac

for dep in curl tar awk "${hasher[0]}"; do
  command -v "$dep" >/dev/null 2>&1 || { echo "ERROR: $dep is required." >&2; exit 1; }
done

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

mkdir -p "$INSTALL_DIR"

# Release asset: all three binaries ship in one tarball, named exactly as
# uploaded by .github/workflows/release.yml.
echo "→ Downloading $asset…"
curl -fsSL "$RELEASE_BASE/$asset" -o "$tmpdir/$asset"
curl -fsSL "$RELEASE_BASE/$asset.sha256" -o "$tmpdir/$asset.sha256"

# Explicit compare (not `sha256sum -c`) — works identically with macOS shasum.
expected="$(awk '{print $1}' "$tmpdir/$asset.sha256")"
actual="$("${hasher[@]}" "$tmpdir/$asset" | awk '{print $1}')"
if [[ "$actual" != "$expected" ]]; then
  echo "ERROR: checksum mismatch for $asset — refusing to install." >&2
  echo "  expected: $expected" >&2
  echo "  actual:   $actual" >&2
  exit 1
fi
echo "✓ Checksum verified."

echo "→ Extracting…"
tar xzf "$tmpdir/$asset" -C "$tmpdir"

for bin in engram engramd engramd-mcp; do
  install -m 0755 "$tmpdir/$bin" "$INSTALL_DIR/$bin"
  echo "✓ Installed $bin ($INSTALL_DIR/$bin)"
done

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "NOTE: add $INSTALL_DIR to your PATH:" >&2
     echo "  export PATH=\"$INSTALL_DIR:\$PATH\"" >&2 ;;
esac

echo
echo "Engram by El AI Intelligence installed. Next steps:"
echo "  engram onboarding    # encrypted vault + first memory + running daemon (~5 min)"
echo "  engram mcp install   # connect Claude Desktop, Cursor, Windsurf, Claude Code"
