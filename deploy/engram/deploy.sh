#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# deploy.sh — install Engram Memory Vault artifacts on this host.
#
#   - Syncs the static landing page and vault UI into /srv/engram/
#   - Installs the Caddy site config (imported from the main Caddyfile)
#   - Installs and starts the engramd systemd unit
#   - Validates and reloads Caddy (if installed)
#
# Usage: sudo ./deploy.sh [-y]
#   -y   skip the confirmation prompt
#
# This script only touches this machine — it does not deploy anywhere remote.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

ASSUME_YES=0
for arg in "$@"; do
  case "$arg" in
    -y|--yes) ASSUME_YES=1 ;;
    *) echo "Unknown argument: $arg" >&2; exit 2 ;;
  esac
done

# ── Guards ───────────────────────────────────────────────────────────────────

if [[ $EUID -ne 0 ]]; then
  echo "ERROR: this script must be run as root (use sudo)." >&2
  exit 1
fi

if ! command -v rsync >/dev/null 2>&1; then
  echo "ERROR: rsync is required but not installed." >&2
  exit 1
fi

if [[ ! -f /etc/engram/engramd.env ]]; then
  echo "WARNING: /etc/engram/engramd.env does not exist."
  echo "         engramd will start without auth on 127.0.0.1, but Caddy will"
  echo "         inject an empty bearer token. Create it with at least:"
  echo "           ENGRAMD_API_KEY=<random-token>"
elif ! grep -qE '^ENGRAMD_API_KEY=.+' /etc/engram/engramd.env; then
  echo "WARNING: ENGRAMD_API_KEY is not set in /etc/engram/engramd.env."
fi

if [[ $ASSUME_YES -ne 1 ]]; then
  read -r -p "Deploy Engram (landing, vault UI, Caddyfile, engramd unit) to this host? [y/N] " reply
  if [[ ! "$reply" =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 0
  fi
fi

# ── Static sites ─────────────────────────────────────────────────────────────

install -d -m 0755 /srv/engram /srv/engram/landing /srv/engram/vault
rsync -a --delete "$REPO_ROOT/ui/landing/" /srv/engram/landing/
rsync -a --delete "$REPO_ROOT/ui/engram-vault/" /srv/engram/vault/
echo "Synced landing page and vault UI to /srv/engram/"

# ── Caddy site config ────────────────────────────────────────────────────────

install -d -m 0755 /etc/caddy
install -m 0644 "$REPO_ROOT/deploy/caddy/engram.Caddyfile" /etc/caddy/engram.Caddyfile
echo "Installed /etc/caddy/engram.Caddyfile"
echo "NOTE: ensure /etc/caddy/Caddyfile imports it, e.g.:"
echo "    import /etc/caddy/engram.Caddyfile"

# ── engramd systemd unit ─────────────────────────────────────────────────────

install -m 0644 "$REPO_ROOT/deploy/systemd/engramd.service" /etc/systemd/system/engramd.service

# Vault data directory (owned by the service user if it exists).
install -d -m 0750 /var/lib/engram /var/lib/engram/vault
if id engram >/dev/null 2>&1; then
  chown -R engram:engram /var/lib/engram
else
  echo "WARNING: 'engram' user does not exist; create it before starting the service:"
  echo "    useradd --system --home /var/lib/engram --shell /usr/sbin/nologin engram"
fi

systemctl daemon-reload
systemctl enable --now engramd
echo "engramd service enabled and started (check: journalctl -u engramd -f)"

# ── Validate & reload Caddy ──────────────────────────────────────────────────

if command -v caddy >/dev/null 2>&1; then
  caddy validate --config /etc/caddy/Caddyfile
  systemctl reload caddy
  echo "Caddy config validated and reloaded."
else
  echo "caddy not found on PATH; skipping validation and reload."
fi

echo "Done. Site: https://engram.ellmstack.dev (vault UI at /app)"
