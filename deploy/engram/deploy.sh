#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# deploy.sh — install Engram by El AI Intelligence artifacts on this host.
#
#   - Syncs the static landing page and vault UI into /srv/engram/
#   - Publishes scripts/install.sh and release binaries (verified downloads)
#   - Installs the Caddy site config (imported from the main Caddyfile)
#   - Installs the engramd systemd unit (ONLY if none exists — an existing
#     unit is host-specific and is never overwritten)
#   - Optionally ships target/release/engramd when a release build exists
#   - Validates and reloads Caddy (if installed)
#
# Usage: sudo ./deploy.sh [-y]
#   -y   skip the confirmation prompt
#
# This script only touches this machine — it does not deploy anywhere remote.
# It does NOT create vault data directories: the vault path is whatever the
# systemd unit declares (this box uses /root/engram/vault, not /var/lib/engram).
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
  read -r -p "Deploy Engram by El AI Intelligence (landing, vault UI, Caddyfile, engramd unit if absent) to this host? [y/N] " reply
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

# Staged AFTER rsync --delete: these artifacts live outside ui/landing in the
# repo, so rsync would wipe them on every deploy if they weren't re-staged.
# ── One-command installer + public release binaries ──────────────────────────

install -m 0644 "$REPO_ROOT/scripts/install.sh" /srv/engram/landing/install.sh
install -m 0644 "$REPO_ROOT/scripts/install.ps1" /srv/engram/landing/install.ps1
echo "Published install.sh + install.ps1 (https://engram.ellmstack.dev/install.sh)"

# Optional: stage the CI-built Windows zip on the mirror (the server can't
# build Windows binaries itself):
#   ENGRAM_WINDOWS_ZIP_VERSION=v0.2.0 ./deploy/engram/deploy.sh -y
if [[ -n "${ENGRAM_WINDOWS_ZIP_VERSION:-}" ]]; then
  install -d -m 0755 /srv/engram/landing/releases
  curl -fsSL "https://github.com/El-AI-Intelligence/engram/releases/download/${ENGRAM_WINDOWS_ZIP_VERSION}/engramd-windows-x86_64.zip" \
    -o /srv/engram/landing/releases/engramd-windows-x86_64.zip
  curl -fsSL "https://github.com/El-AI-Intelligence/engram/releases/download/${ENGRAM_WINDOWS_ZIP_VERSION}/engramd-windows-x86_64.zip.sha256" \
    -o /srv/engram/landing/releases/engramd-windows-x86_64.zip.sha256
  echo "Staged Windows release zip (${ENGRAM_WINDOWS_ZIP_VERSION}) on the mirror."
fi

if [[ -f "$REPO_ROOT/target/release/engram" && -f "$REPO_ROOT/target/release/engramd" && -f "$REPO_ROOT/target/release/engramd-mcp" ]]; then
  install -d -m 0755 /srv/engram/landing/releases
  install -m 0755 "$REPO_ROOT/target/release/engram" /srv/engram/landing/releases/engram-linux-x86_64
  install -m 0755 "$REPO_ROOT/target/release/engramd" /srv/engram/landing/releases/engramd-linux-x86_64
  install -m 0755 "$REPO_ROOT/target/release/engramd-mcp" /srv/engram/landing/releases/engramd-mcp-linux-x86_64
  ( cd /srv/engram/landing/releases \
    && sha256sum engram-linux-x86_64 > engram-linux-x86_64.sha256 \
    && sha256sum engramd-linux-x86_64 > engramd-linux-x86_64.sha256 \
    && sha256sum engramd-mcp-linux-x86_64 > engramd-mcp-linux-x86_64.sha256 )
  # GitHub-layout tarball: the exact asset shape scripts/install.sh downloads
  # by default from GitHub Releases (three binaries at archive root + .sha256
  # sidecar naming the tarball). Staging the same layout here keeps the
  # ENGRAM_RELEASE_BASE site-mirror override working.
  tar czf /srv/engram/landing/releases/engramd-linux-x86_64.tar.gz \
    -C "$REPO_ROOT/target/release" engram engramd engramd-mcp
  ( cd /srv/engram/landing/releases \
    && sha256sum engramd-linux-x86_64.tar.gz > engramd-linux-x86_64.tar.gz.sha256 )
  echo "Published release binaries + SHA-256 checksums (the installer verifies them)."
else
  echo "NOTE: release binaries not found (run: cargo build --release -p engramd -p engramd-mcp)."
  echo "      install.sh is live but downloads will 404 until releases are staged."
fi

# ── Caddy site config ────────────────────────────────────────────────────────

install -d -m 0755 /etc/caddy
install -m 0644 "$REPO_ROOT/deploy/caddy/engram.Caddyfile" /etc/caddy/engram.Caddyfile
echo "Installed /etc/caddy/engram.Caddyfile"
echo "NOTE: ensure /etc/caddy/Caddyfile imports it, e.g.:"
echo "    import /etc/caddy/engram.Caddyfile"

# ── engramd systemd unit ─────────────────────────────────────────────────────

if [[ -f /etc/systemd/system/engramd.service ]]; then
  echo "NOTE: /etc/systemd/system/engramd.service already exists — leaving it"
  echo "      untouched (host-specific unit; deploy does not overwrite it)."
else
  install -m 0644 "$REPO_ROOT/deploy/systemd/engramd.service" /etc/systemd/system/engramd.service
  echo "Installed /etc/systemd/system/engramd.service (new host — review its"
  echo "ExecStart paths before enabling)."
fi

# Report the vault path the unit actually declares (the box's real location,
# whatever it is — this script creates no data directories).
if [[ -f /etc/systemd/system/engramd.service ]]; then
  vault_line="$(grep -oE '\-\-vault [^ ]+' /etc/systemd/system/engramd.service | head -1 || true)"
  echo "Detected unit vault path: ${vault_line:-'(not set — unit uses its default)'}"
fi

systemctl daemon-reload
if systemctl list-unit-files engramd.service >/dev/null 2>&1; then
  systemctl enable --now engramd
  echo "engramd service enabled and started (check: journalctl -u engramd -f)"
else
  echo "NOTE: no engramd unit present; skipping enable/start."
fi

# ── engramd binary (optional — only when a release build exists) ─────────────

if [[ -f "$REPO_ROOT/target/release/engramd" ]]; then
  install -m 0755 "$REPO_ROOT/target/release/engramd" /usr/local/bin/engramd
  if [[ -f "$REPO_ROOT/target/release/engram" ]]; then
    install -m 0755 "$REPO_ROOT/target/release/engram" /usr/local/bin/engram
  fi
  echo "Installed target/release/engramd (and engram CLI) to /usr/local/bin/"
  if systemctl list-unit-files engramd.service >/dev/null 2>&1; then
    systemctl restart engramd
    echo "Restarted engramd with the new binary."
  fi
else
  echo "NOTE: target/release/engramd not found (run: cargo build --release -p engramd)."
  echo "      Static assets deployed; the running daemon keeps its existing binary."
fi

# ── Validate & reload Caddy ──────────────────────────────────────────────────

if command -v caddy >/dev/null 2>&1; then
  caddy validate --config /etc/caddy/Caddyfile
  systemctl reload caddy
  echo "Caddy config validated and reloaded."
else
  echo "caddy not found on PATH; skipping validation and reload."
fi

echo "Done. Site: https://engram.ellmstack.dev (vault UI at /app)"
