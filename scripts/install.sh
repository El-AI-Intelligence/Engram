#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# install.sh — one-command installer for Engram (engram + engramd + engramd-mcp).
#
#     curl -fsSL https://engram.ellmstack.dev/install.sh | bash
#
# Downloads the latest Linux x86_64 release from GitHub Releases (by default),
# verifies its SHA-256 checksum, and installs the binaries into ~/.local/bin
# (override with INSTALL_DIR). Override the download base with
# ENGRAM_RELEASE_BASE (e.g. the site mirror https://engram.ellmstack.dev/releases).
# Staged on the live site by deploy/engram/deploy.sh.
#
# Other platforms: macOS/Windows assets and installers on GitHub Releases —
# https://github.com/El-AI-Intelligence/engram/releases.
#
# No sudo, no secrets, no telemetry. Everything installed is MIT open source.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

RELEASE_BASE="${ENGRAM_RELEASE_BASE:-https://github.com/El-AI-Intelligence/engram/releases/latest/download}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "Prebuilt binaries currently ship for Linux x86_64 only." >&2
  echo "macOS and Windows assets are on GitHub Releases:" >&2
  echo "  https://github.com/El-AI-Intelligence/engram/releases" >&2
  exit 1
fi

for dep in curl sha256sum tar; do
  command -v "$dep" >/dev/null 2>&1 || { echo "ERROR: $dep is required." >&2; exit 1; }
done

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

mkdir -p "$INSTALL_DIR"

# Linux x86_64 release asset: all three binaries ship in one tarball, named
# exactly as uploaded by .github/workflows/release.yml.
asset="engramd-linux-x86_64.tar.gz"

echo "→ Downloading $asset…"
curl -fsSL "$RELEASE_BASE/$asset" -o "$tmpdir/$asset"
# Downloaded under the FULL asset name so sha256sum -c matches the filename
# recorded in the .sha256 sidecar.
curl -fsSL "$RELEASE_BASE/$asset.sha256" -o "$tmpdir/$asset.sha256"
( cd "$tmpdir" && sha256sum -c "$asset.sha256" ) >/dev/null

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
echo "Engram installed. Next steps:"
echo "  engram onboarding    # encrypted vault + first memory + running daemon (~5 min)"
echo "  engram mcp install   # connect Claude Desktop, Cursor, Windsurf, Claude Code"
