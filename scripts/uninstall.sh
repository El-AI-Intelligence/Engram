#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# uninstall.sh — one-command uninstaller for Engram by El AI Intelligence.
#
#     curl -fsSL https://engram.ellmstack.dev/uninstall.sh | bash
#
# Removes engram / engramd / engramd-mcp from ~/.local/bin (override with
# INSTALL_DIR — the same override install.sh honors), tears down the
# background service that guided `engram onboarding` installed (systemd
# --user unit on Linux, launchd agent on macOS), and asks before touching
# ~/.engram. Never deletes vault data without an explicit "y". Editor MCP
# configs written by `engram mcp install` are left alone.
# Staged on the live site by deploy/engram/deploy.sh.
#
# ENGRAM_DRY_RUN=1 prints every action without doing it.
#
# No sudo, no secrets, no telemetry. Everything installed is MIT open source.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
DRY_RUN="${ENGRAM_DRY_RUN:-}"
os="$(uname -s | tr '[:upper:]' '[:lower:]')"

run() {
  if [[ -n "$DRY_RUN" ]]; then echo "  [dry-run] $*"; else "$@"; fi
}

echo
echo "Engram by El AI Intelligence — uninstaller"
echo "─────────────────────────────────────────"
echo

# ── Binaries ─────────────────────────────────────────────────────────────────
removed=0
for bin in engram engramd engramd-mcp; do
  if [[ -f "$INSTALL_DIR/$bin" ]]; then
    run rm -f "$INSTALL_DIR/$bin"
    echo "✓ Removed $INSTALL_DIR/$bin"
    removed=1
  else
    echo "· Not found: $INSTALL_DIR/$bin"
  fi
done
if [[ "$removed" = 1 ]] && rmdir "$INSTALL_DIR" 2>/dev/null; then
  echo "✓ Removed empty directory $INSTALL_DIR"
fi

# ── Background service (installed by `engram onboarding`) ────────────────────
case "$os" in
  linux)
    if command -v systemctl >/dev/null 2>&1; then
      if systemctl --user list-unit-files engramd.service >/dev/null 2>&1; then
        run systemctl --user disable --now engramd >/dev/null 2>&1 || true
        echo "✓ Disabled engramd systemd user service"
      fi
      if [[ -f "$HOME/.config/systemd/user/engramd.service" ]]; then
        run rm -f "$HOME/.config/systemd/user/engramd.service"
        run systemctl --user daemon-reload >/dev/null 2>&1 || true
        echo "✓ Removed engramd systemd unit file"
      else
        echo "· No systemd user unit found"
      fi
    fi
    ;;
  darwin)
    plist="$HOME/Library/LaunchAgents/com.ellmstack.engramd.plist"
    if [[ -f "$plist" ]]; then
      run launchctl bootout "gui/$UID/com.ellmstack.engramd" >/dev/null 2>&1 \
        || run launchctl unload "$plist" >/dev/null 2>&1 || true
      run rm -f "$plist"
      echo "✓ Removed engramd launchd agent"
    else
      echo "· No launchd agent found"
    fi
    ;;
esac

# ── Vault data (optional — never without an explicit "y") ────────────────────
echo
if [[ -d "$HOME/.engram" ]]; then
  answer="n"
  if [[ -n "$DRY_RUN" ]]; then
    echo "  [dry-run] Would ask: remove vault data at $HOME/.engram? [y/N]"
  else
    printf "Remove vault data at %s (memories, passphrase, config)? [y/N] " "$HOME/.engram"
    read -r answer
  fi
  case "$answer" in
    y|Y) run rm -rf "$HOME/.engram"
         echo "✓ Removed $HOME/.engram" ;;
    *)   echo "· Kept $HOME/.engram (run this again and answer y to remove it)" ;;
  esac
else
  echo "· No vault data at $HOME/.engram"
fi

echo
echo "Done. Editor integrations (engram mcp install) were left untouched."
echo
