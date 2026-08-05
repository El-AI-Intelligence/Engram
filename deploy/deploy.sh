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

echo ">> Health check"
health=$(curl -sf https://engram.ellmstack.dev/health) || { echo "UNHEALTHY"; exit 1; }
echo "   ${health}"
echo ">> Deploy complete."
echo "   https://engram.ellmstack.dev/"
