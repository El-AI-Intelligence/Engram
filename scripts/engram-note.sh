#!/usr/bin/env bash
# engram-note.sh — capture a quick note into a running engramd daemon.
#
# Usage: engram-note.sh "note text" [tag1 tag2 ...]
#
# Posts to the daemon as a semantic-layer note (source "interaction", scope
# "note", strict_local privacy). The daemon's noise/dedupe/embedding pipeline
# applies as normal — a rejected note is reported with its skip_reason.
#
#   ENGRAMD_URL  override the daemon base URL (default http://127.0.0.1:8799)

set -euo pipefail

URL="${ENGRAMD_URL:-http://127.0.0.1:8799}"
CONTENT="${1:-}"
shift || true

if [ -z "$CONTENT" ]; then
    echo "usage: engram-note.sh \"note text\" [tags...]" >&2
    exit 2
fi

TAGS=("note" "$@")

python3 - "$URL" "$CONTENT" "${TAGS[@]}" <<'PY'
import json, sys, urllib.request

url, content = sys.argv[1], sys.argv[2]
tags = sys.argv[3:]

payload = {
    "content": content,
    "source": "interaction",
    "layer": "semantic",
    "tags": tags,
    "scope": "note",
    "privacy_level": "strict_local",
}
req = urllib.request.Request(
    f"{url}/memories",
    data=json.dumps(payload).encode(),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=10) as resp:
    out = json.load(resp)

if out.get("skipped"):
    print(f"skipped: {out.get('skip_reason')}")
    sys.exit(1)
print(out.get("id", "?"))
PY
