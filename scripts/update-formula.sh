#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# update-formula.sh — refresh Homebrew formula sha256s after a release tag.
#
#   VERSION=v0.2.0 ./scripts/update-formula.sh
#
# Fetches the four release .sha256 sidecars from GitHub Releases and rewrites
# the PLACEHOLDER_UPDATE_ON_RELEASE lines in Formula/engramd.rb. Run after the
# release CI finishes building; commit + push the formula change.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="El-AI-Intelligence/engram"
FORMULA="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/Formula/engramd.rb"
VERSION="${VERSION:?Set VERSION, e.g. VERSION=v0.2.0 ./scripts/update-formula.sh}"

if ! command -v curl >/dev/null 2>&1; then
  echo "ERROR: curl is required." >&2
  exit 1
fi

fetch() { # fetch <platform> — prints the sha256 hex
  local url="https://github.com/${REPO}/releases/download/${VERSION}/engramd-$1.tar.gz.sha256"
  curl -fsSL "$url" | awk '{print $1}'
}

declare -A SHA
for plat in darwin-arm64 darwin-x86_64 linux-arm64 linux-x86_64; do
  echo -n "Fetching ${plat}… "
  if h="$(fetch "$plat")" && [[ "$h" =~ ^[0-9a-f]{64}$ ]]; then
    SHA[$plat]="$h"
    echo "OK"
  else
    echo "FAILED (release artifacts missing?)"
    echo "Run this only after the release CI has finished building ${VERSION}."
    exit 1
  fi
done

# Rewrite placeholders in the formula's four sha256 lines, in platform order.
perl -0pi \
  -e 's/(engramd-darwin-arm64\.tar\.gz"\n\s*)sha256 "[^"]*"/$1sha256 "'"${SHA[darwin-arm64]}"'"/' \
  -e 's/(engramd-darwin-x86_64\.tar\.gz"\n\s*)sha256 "[^"]*"/$1sha256 "'"${SHA[darwin-x86_64]}"'"/' \
  -e 's/(engramd-linux-arm64\.tar\.gz"\n\s*)sha256 "[^"]*"/$1sha256 "'"${SHA[linux-arm64]}"'"/' \
  -e 's/(engramd-linux-x86_64\.tar\.gz"\n\s*)sha256 "[^"]*"/$1sha256 "'"${SHA[linux-x86_64]}"'"/' \
  "$FORMULA"

if grep -q "PLACEHOLDER_UPDATE_ON_RELEASE" "$FORMULA"; then
  echo "ERROR: placeholders remain in ${FORMULA} — rewrite did not match." >&2
  exit 1
fi

echo "✅ ${FORMULA} updated for ${VERSION}. Commit and push, e.g.:"
echo "   git add Formula/engramd.rb && git commit -m 'Formula: sha256s for ${VERSION}'"
