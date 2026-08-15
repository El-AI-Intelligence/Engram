#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# install.sh — one-command installer for Engram (engramd + engramd-mcp).
#
#     curl -fsSL https://engram.ellmstack.dev/install.sh | bash
#
# Downloads the latest prebuilt Linux x86_64 release binaries, verifies their
# SHA-256 checksums, and installs them into ~/.local/bin (override with
# INSTALL_DIR). Staged on the live site by deploy/engram/deploy.sh.
#
# Other platforms: build from source once the repo is public —
# https://github.com/El-AI-Intelligence/engram.
#
# No sudo, no secrets, no telemetry. Everything installed is MIT open source.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

RELEASE_BASE="${ENGRAM_RELEASE_BASE:-https://engram.ellmstack.dev/releases}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "Prebuilt binaries currently ship for Linux x86_64 only." >&2
  echo "For other platforms, build from source once the repo is public:" >&2
  echo "  https://github.com/El-AI-Intelligence/engram" >&2
  exit 1
fi

for dep in curl sha256sum; do
  command -v "$dep" >/dev/null 2>&1 || { echo "ERROR: $dep is required." >&2; exit 1; }
done

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

mkdir -p "$INSTALL_DIR"

for bin in engramd engramd-mcp; do
  echo "→ Downloading $bin…"
  curl -fsSL "$RELEASE_BASE/$bin-linux-x86_64" -o "$tmpdir/$bin"
  curl -fsSL "$RELEASE_BASE/$bin-linux-x86_64.sha256" -o "$tmpdir/$bin.sha256"
  ( cd "$tmpdir" && sha256sum -c "$bin.sha256" ) >/dev/null
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
