#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# update-formula.sh — refresh Homebrew formula sha256s + version after a release tag.
#
#   VERSION=v0.2.0 ./scripts/update-formula.sh
#
# Fetches the four release .sha256 sidecars from GitHub Releases and rewrites
# the four sha256 lines in Formula/engramd.rb (in the order the formula lists
# them: darwin-arm64, darwin-x86_64, linux-arm64, linux-x86_64), plus the
# `version "…"` line. Run after the release CI finishes building; commit + push
# the formula change. Override the target with FORMULA_PATH (e.g. a tap clone).
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="El-AI-Intelligence/engram"
FORMULA="${FORMULA_PATH:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/Formula/engramd.rb}"
VERSION="${VERSION:?Set VERSION, e.g. VERSION=v0.2.0 ./scripts/update-formula.sh}"
VER="${VERSION#v}"  # formula stores plain versions ("0.2.0"), tags carry "v"

if ! command -v curl >/dev/null 2>&1; then
  echo "ERROR: curl is required." >&2
  exit 1
fi
[[ -f "$FORMULA" ]] || { echo "ERROR: formula not found at $FORMULA." >&2; exit 1; }

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

# The formula must list exactly four sha256 lines; replace them in file order
# (the formula lists platforms in the same order as the fetch loop above).
count="$(grep -cE 'sha256 "[0-9a-f]{64}"' "$FORMULA" || true)"
if [[ "$count" -ne 4 ]]; then
  echo "ERROR: expected 4 sha256 lines in ${FORMULA}, found ${count}." >&2
  exit 1
fi

export FORMULA_SHA1="${SHA[darwin-arm64]}" FORMULA_SHA2="${SHA[darwin-x86_64]}" \
       FORMULA_SHA3="${SHA[linux-arm64]}"   FORMULA_SHA4="${SHA[linux-x86_64]}" \
       FORMULA_VERSION="$VER"
perl -0pi -e 'my @sha = @ENV{qw(FORMULA_SHA1 FORMULA_SHA2 FORMULA_SHA3 FORMULA_SHA4)};
  s/sha256 "[0-9a-f]{64}"/"sha256 \"" . shift(@sha) . "\""/ge;
  s/version "[^"]*"/version "$ENV{FORMULA_VERSION}"/' "$FORMULA"

if ! grep -qE "^  version \"${VER}\"" "$FORMULA"; then
  echo "ERROR: no version line to update in ${FORMULA} — add: version \"${VER}\"" >&2
  exit 1
fi

echo "✅ ${FORMULA} updated for ${VERSION} (version ${VER}). Commit and push, e.g.:"
echo "   git add Formula/engramd.rb && git commit -m 'Formula: sha256s for ${VERSION}'"
