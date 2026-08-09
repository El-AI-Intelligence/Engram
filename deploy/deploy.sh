#!/usr/bin/env bash
# deploy.sh — deploy the Engram Vault UI + mock API to the production host.
#
# Usage:
#   deploy/deploy.sh [user@host]          # default root@204.168.163.161
set -euo pipefail

HOST="${1:-root@204.168.163.161}"
REMOTE_DIR="/root/engram"
REMOTE_UI="${REMOTE_DIR}/ui"
SSH="ssh -o BatchMode=yes ${HOST}"
SCP="scp -o BatchMode=yes"

echo ">> Uploading UI to ${HOST}"
$SSH "mkdir -p ${REMOTE_UI}"

# Sync the UI directory (server.py + static files)
rsync -a --delete \
  ui/ "${HOST}:${REMOTE_UI}/"

echo ">> Installing systemd service"
$SCP deploy/engram-server.service "${HOST}:/etc/systemd/system/"
$SSH "systemctl daemon-reload && \
      systemctl enable engram-server && \
      systemctl restart engram-server && \
      sleep 2 && systemctl is-active engram-server"

echo ">> Updating Caddy config"
# Append the engram block if not already present
$SSH 'grep -q "engram.ellmstack.dev" /etc/caddy/Caddyfile || cat >> /etc/caddy/Caddyfile << CADDYEOF

# ---------------------------------------------------------------------------
# Engram — Memory Vault UI + API (Python mock server)
# ---------------------------------------------------------------------------
engram.ellmstack.dev {
	reverse_proxy localhost:8787

	header {
		X-Frame-Options "DENY"
		X-Content-Type-Options "nosniff"
		Referrer-Policy "strict-origin-when-cross-origin"
		Strict-Transport-Security "max-age=31536000; includeSubDomains"
	}

	header / Content-Security-Policy "default-src '"'"'self'"'"'; script-src '"'"'self'"'"'; style-src '"'"'self'"'"' '"'"'unsafe-inline'"'"' https://fonts.googleapis.com; img-src '"'"'self'"'"' data:; connect-src '"'"'self'"'"'; font-src '"'"'self'"'"' https://fonts.gstatic.com; form-action '"'"'self'"'"'; frame-ancestors '"'"'none'"'"'; base-uri '"'"'self'"'"'"
}
CADDYEOF
' && $SSH "systemctl reload caddy"

echo ">> Health check"
health=$(curl -sf https://engram.ellmstack.dev/health) || { echo "UNHEALTHY"; exit 1; }
echo "   ${health}"
echo ">> Deploy complete."
echo "   https://engram.ellmstack.dev/"
