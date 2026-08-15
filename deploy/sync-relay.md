# Engram Sync Relay — public deploy runbook

`sync.ellmstack.dev` → Hetzner CX23 (138.199.144.93, server id 125572177)
→ Caddy :443 → `engramd-sync` on 127.0.0.1:8788.

Deployed + verified 2026-08-15: two-device bidirectional round trip through the
public URL; relay DB contains only vault/device/memory IDs, vector clocks,
ciphertext, HMAC — zero plaintext (milestone 1.1 verification vault
`deploytest`, 2 devices, 2 blobs). This file is the runbook for redeploying,
upgrading, and recovering the relay. Secrets: `SYNC_API_KEYS` lives ONLY in
`/etc/engram-sync/engramd-sync.env` (0600, root) — never print, never commit.

## Layout on the box

| Path | What |
|---|---|
| `/usr/local/bin/engramd-sync` | release binary (scp from `target/release/engramd-sync`) |
| `/var/lib/engram-sync/` | data dir (sync.db + WAL), owned by `engram-sync` |
| `/etc/engram-sync/engramd-sync.env` | `SYNC_API_KEYS=...` (0600 root) |
| `/etc/systemd/system/engramd-sync.service` | unit (from `deploy/systemd/`) — loopback bind, sandboxed |
| `/etc/caddy/sync.Caddyfile` | site (from `deploy/caddy/`), imported by `/etc/caddy/Caddyfile` |
| `/usr/local/sbin/engram-sync-backup.sh` | nightly WAL-safe snapshot → `/var/backups/engram-sync` (7-day retention; timer 03:17 UTC) |

Firewall: ufw, only 22/80/443 open. sshd: key-only (`PasswordAuthentication no`,
`PermitRootLogin prohibit-password`). Deploy key: `/home/e/.ssh/engram_hetzner_ed25519`
(Hetzner account key id 117207480, "engram-relay-deploy"); local SSH alias `hetzner-sync`.

## Upgrade procedure

```bash
cargo build --release -p engramd-sync          # locally
ssh hetzner-sync 'systemctl stop engramd-sync'
scp target/release/engramd-sync hetzner-sync:/usr/local/bin/engramd-sync
ssh hetzner-sync 'systemctl start engramd-sync && curl -s 127.0.0.1:8788/health'
curl -s https://sync.engram.ellmstack.dev/health
```

## Add / rotate an API key

On the box, append to `SYNC_API_KEYS` in the env file (comma-separated entries;
format `key[:rate[:vault1;vault2[+admin]]]`, ≥16 chars), then
`systemctl restart engramd-sync`. Generate with `openssl rand -hex 24`.

## Recovery via Hetzner rescue (proven procedure)

If key auth ever fails (e.g. `authorized_keys` wiped):

```bash
# 1. Enable rescue WITH the deploy key, then reboot — BOTH are required.
#    enable_rescue alone does NOT boot rescue (server keeps running the normal OS).
curl -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"type":"linux64","ssh_keys":[117207480]}' \
  https://api.hetzner.cloud/v1/servers/125572177/actions/enable_rescue
curl -X POST -H "Authorization: Bearer $TOKEN" \
  https://api.hetzner.cloud/v1/servers/125572177/actions/reboot

# 2. SSH in (host key changes to the rescue image's key — clear it first), then:
mkdir -p /mnt/disk && mount /dev/sda1 /mnt/disk
echo "<deploy pubkey>" >> /mnt/disk/root/.ssh/authorized_keys
chroot /mnt/disk /bin/bash -c "chage -d $(date +%F) root"   # clear forced password change
umount /mnt/disk

# 3. Leave rescue and boot normal (again: disable_rescue + reboot, both).
```

## Hetzner gotchas (all hit 2026-08-15)

- **Rebuild with `ssh_keys` can silently not inject**: the rebuild action reports
  success but `/root/.ssh/authorized_keys` ends up empty (0 bytes) and the server
  resource's `ssh_keys` field is null. ALWAYS verify by SSHing after a rebuild;
  fall back to the rescue procedure above.
- **`enable_rescue` does not reboot the server.** The rescue config activates on
  the NEXT boot — you must POST `/actions/reboot` yourself. (Symptom: normal OS
  sshd still answering; host key unchanged.)
- **Forced password change blocks key logins**: a rebuilt Hetzner Ubuntu marks
  the root password expired; even with a working key in `authorized_keys`,
  non-TTY sessions die with "Password change required but no TTY available".
  Clear it with `chage -d <today> root` from rescue (no password ever enters a
  transcript).
- **Caddy waits on DNS**: LE issuance retries automatically (60s backoff, 30-day
  window); the moment the A record exists at Cloudflare the cert lands with no
  action needed. Verify from an external resolver — checking the box itself
  hides nothing but saves a round trip.

## DNS

`sync` A record → 138.199.144.93, **DNS only** (grey cloud), in the
`ellmstack.dev` Cloudflare zone. Proxy off keeps Cloudflare out of the sync
path (LE validates directly; the E2E property holds edge-to-edge). The
`sync.engram.ellmstack.dev` alias in the Caddyfile activates automatically
if that record is ever added.

## Verification checklist (milestone 1.1) — all passed 2026-08-15

- [x] `https://sync.ellmstack.dev/health` → `{"status":"ok",...}`
- [x] Two fresh vaults, same vault_id + passphrase, converge through the public URL
      (both directions: A→B and B→A; devices endpoint lists both, 1 blob each)
- [x] Server-side only sees ciphertext: relay DB rows contain no plaintext
      (spot-check `sqlite3 /var/lib/engram-sync/sync.db 'select * from sync_blobs limit 3'`)
- [x] Snapshot timer ran (`/var/backups/engram-sync` has a db snapshot)
