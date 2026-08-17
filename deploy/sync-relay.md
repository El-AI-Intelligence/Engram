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
| `/etc/engram-sync/engramd-sync.env` | `SYNC_API_KEYS=...` (0600 root) — operator/static keys only |
| `/etc/systemd/system/engramd-sync.service` | unit (from `deploy/systemd/`) — loopback bind, sandboxed, managed-relay flags (`--rp-id ellmstack.dev --origin https://engram.ellmstack.dev --quota-devices 1 --quota-bytes 1 GiB`) |
| `/etc/caddy/sync.Caddyfile` | site (from `deploy/caddy/`), imported by `/etc/caddy/Caddyfile` |
| `/usr/local/sbin/engram-sync-backup.sh` | nightly WAL-safe snapshot → `/var/backups/engram-sync` (7-day retention; timer 03:17 UTC) |

Firewall: ufw, only 22/80/443 open. sshd: key-only (`PasswordAuthentication no`,
`PermitRootLogin prohibit-password`). Deploy key: `/home/e/.ssh/engram_hetzner_ed25519`
(Hetzner account key id 117207480, "engram-relay-deploy"); local SSH alias `hetzner-sync`.

## Upgrade procedure

```bash
cargo build --release -p engramd-sync          # locally
scp deploy/systemd/engramd-sync.service hetzner-sync:/etc/systemd/system/
ssh hetzner-sync 'systemctl daemon-reload && systemctl stop engramd-sync'
scp target/release/engramd-sync hetzner-sync:/usr/local/bin/engramd-sync
ssh hetzner-sync 'systemctl start engramd-sync && sleep 2 && curl -s 127.0.0.1:8788/health'
curl -s https://sync.ellmstack.dev/health
```

The unit on the box must match `deploy/systemd/engramd-sync.service` — it
carries the managed-relay flags (`--rp-id`, `--origin`, `--quota-*`).
Schema migrations are automatic on startup (new tables only, no ALTERs);
the DB is safe to open on an older binary, which just ignores the extra
tables. A DB restore (backup snapshot) is therefore compatible with both
old and new binaries.

## Accounts (milestone 1.2, shipped 2026-08-15)

- **Passkeys:** registration IS sign-up; sessions are Bearer tokens in the
  browser's localStorage (7-day TTL, sha256 at rest). Ceremony state is an
  in-memory store with a 300s TTL — a relay **restart drops in-flight
  ceremonies**, users just start over. This also means passkey auth needs a
  **single-instance** relay (the managed relay is one process; a HA
  deployment would need shared ceremony state).
- **RP ID is `ellmstack.dev`** — a registrable domain suffix of the vault
  UI origin (`https://engram.ellmstack.dev`). Passkeys bind to the RP ID:
  **do not change `--rp-id`** unless you accept orphaning every passkey.
  Adding a new UI origin to `--origin` (comma-separated) is safe.
- **Quotas** apply to account-minted keys only: default 1 device + 1 GiB
  per account (server defaults from the unit flags; per-account overrides
  in the `accounts` table await billing, 1.3). Exceeding them rejects the
  whole push batch with 402 and surfaces in the user's Sync & Team panel as
  `last_push_error`. Static `SYNC_API_KEYS` entries are exempt.
- **Loopback wildcard:** keyless loopback requests are superuser only while
  no unrevoked account keys exist; the first `/account/keys` mint flips the
  relay to require Bearer auth everywhere. On this box the relay binds
  loopback anyway, so the wildcard was never remotely reachable.
- **Key hygiene:** `/account/keys` shows the full key once; the DB stores
  only `sha256(key)` — a DB leak cannot recover account keys. Revocation is
  soft (row kept as audit trail).
- **Sessions can pull (2026-08-17, read-only):** pull accepts account
  sessions on API-key 401 only (vault visibility = the account's unrevoked
  keys; 429 never falls through), plus `GET /account/vaults` for the
  browser unlock picker. Sessions still can't push or manage keys.
- **Browser e2e** (human step, on this box): open
  https://engram.ellmstack.dev (branded login screen), Settings → Account →
  Register passkey → Sign out → Sign in → mint a key → "Connect this device"
  → restart engramd → Sync & Team shows this device's label and a green
  push.

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

## Verification checklist (milestone 1.2) — 2026-08-15

- [x] `https://sync.ellmstack.dev/health` green with the 1.2 binary + unit flags
- [x] Static-key regression: existing routes (push/pull/stats/devices) still green
      with the operator key — plus the new label-register route (vault
      `deploycheck-12`, device `d12`, 1 blob)
- [x] Schema on box: `sqlite3 /var/lib/engram-sync/sync.db '.tables'` lists
      `accounts passkeys sessions api_keys device_labels`; aggregates (0
      accounts / 0 keys) match `/account` 401-until-registered
- [ ] Browser e2e (human): register → sign out → sign in → mint key → connect
      device → restart daemon → push/pull green + device label in panel
- [ ] Quota: second device on an account key gets 402, visible as
      `last_push_error` in Sync & Team; static key unaffected

Caddy: **no changes** — the relay stays behind the existing TLS site; the
account flow is browser→relay CORS calls with a Bearer token in
localStorage, nothing new terminates at Caddy.
