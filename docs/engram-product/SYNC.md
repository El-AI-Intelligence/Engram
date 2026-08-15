# Engram Sync — End-to-End Encrypted Multi-Device Sync

## Overview

Engram Sync keeps your memory vault synchronized across all your devices
with **zero-knowledge encryption**. The sync server never sees your plaintext
data — all encryption, decryption, and integrity verification happens
client-side before data leaves your machine.

```
┌──────────┐                    ┌──────────┐
│ Device A │ ──── HTTPS ────────▶│  Sync    │◀──── HTTPS ──── │ Device B │
│ (laptop) │ ◀─── pushed ───────│  Server  │──── pulled ────▶ │ (desktop)│
└──────────┘                    │ (dumb)   │                  └──────────┘
                                └──────────┘
```

## Security Model

### Encryption

- **Algorithm:** AES-256-GCM (authenticated encryption)
- **Key derivation:** Argon2id (64 MiB, 3 iterations, 4 lanes) from vault passphrase
- **Domain separation:** Encryption and HMAC keys are derived from separate salts
  (`axiom-sync-enc-v2` and `axiom-sync-hmac-v2`) so the vault encryption key
  and sync keys are cryptographically independent
- **Nonces:** Random 96-bit nonce per blob, prepended to ciphertext on disk

### Integrity

- **HMAC-SHA256** covers all blob metadata: `vault_id`, `memory_id`, `device_id`,
  `vector_clock`, `ciphertext`, `deleted` flag, and `created_at` timestamp
- **Constant-time comparison** prevents timing side-channel attacks on HMAC
  verification
- **Server-side verification:** The sync server checks the HMAC matches the
  stored ciphertext before accepting a push (prevents bit-flip attacks)

### What the Server Sees

| Field | Visible? | Notes |
|-------|----------|-------|
| `vault_id` | ✅ | Hashed vault identifier |
| `memory_id` | ✅ | Random UUID, no semantic meaning |
| `device_id` | ✅ | Random UUID, reveals device count |
| `vector_clock` | ✅ | Integer counter |
| `ciphertext` | ✅ | **Encrypted** — server cannot read |
| `hmac` | ✅ | Integrity tag only |
| `created_at` | ✅ | Server timestamp |
| `deleted` | ✅ | Boolean flag |
| **Memory content** | ❌ | Never leaves your device in plaintext |
| **Tags, context, metadata** | ❌ | Encrypted inside ciphertext |

## Conflict Resolution: Last-Write-Wins

Engram uses **monotonic vector clocks** for conflict resolution:

1. Each device maintains a monotonically-increasing counter (the vector clock)
2. Every push increments the clock and stamps it on the blob
3. The server accepts a blob only if its clock is **strictly greater** than
   the previously-stored clock for that memory
4. On pull, the client bumps its local clock to `max(local, remote_max)` so
   subsequent local writes win

This means: **the most recent edit wins.** Two devices editing the same memory
simultaneously will converge to the edit with the higher clock value.

## Setup

### 1. Start a sync server

The sync server is a lightweight Rust binary (`engramd-sync`) that stores
encrypted blobs in SQLite:

```bash
# Install
cargo install engramd-sync

# Run (loopback only, no auth required)
engramd-sync --data-dir ./sync-data --bind 127.0.0.1:8788

# Run (public, requires API keys)
SYNC_API_KEYS="my-secret-key-32chars:100" engramd-sync \
  --data-dir ./sync-data \
  --bind 0.0.0.0:8788
```

### 2. Configure your vault

Add sync settings to `~/.engram/vault/config.json`:

```json
{
  "sync": {
    "enabled": true,
    "server_url": "http://localhost:8788",
    "api_key": "my-secret-key-32chars",
    "interval_secs": 60
  }
}
```

Or use the API:

```bash
curl -X PATCH http://localhost:8787/config \
  -H 'Content-Type: application/json' \
  -d '{
    "sync": {
      "enabled": true,
      "server_url": "http://localhost:8788",
      "api_key": "my-secret-key-32chars",
      "interval_secs": 60
    }
  }'
```

### 3. Restart engramd

The sync loop starts when engramd boots with sync enabled in the config
and a passphrase is provided:

```bash
engramd --vault ~/.engram/vault --passphrase "your-passphrase"
```

### 4. Verify sync is working

```bash
curl http://localhost:8787/sync/status
```

```json
{
  "configured": true,
  "running": true,
  "last_pull": "2026-08-12T14:30:00Z",
  "last_push": "2026-08-12T14:29:55Z",
  "local_clock": 42,
  "pending_deletions": 0,
  "device_id": "a1b2c3d4-...",
  "device_name": "my-laptop",
  "remote_device_count": 2,
  "remote_reachable": true,
  "pending_push_count": 3
}
```

## Multi-Device Setup

1. **Device A:** Set up vault with `engram init`, configure sync, start daemon
2. **Device B:** Run `engram init` with the **same passphrase**, then:

```bash
# Copy config from Device A (or configure via API)
curl -X PATCH http://localhost:8787/config \
  -H 'Content-Type: application/json' \
  -d '{"sync": {"enabled": true, "server_url": "...", "api_key": "...", "interval_secs": 60}}'

# Restart and wait — memories from Device A will appear within 60 seconds
```

Each device gets a unique `device_id` (stored in `device.json`) so vector
clocks are correctly scoped per-device.

### Edits propagate

The push cursor tracks `modified_at`, not `created_at`: editing an old
memory (patch, ground, mark-noise, dedupe bump, link) re-pushes it on the
next cycle, and the receiving device preserves the original `modified_at`
so it doesn't echo the blob back. Reading a memory (retrievals) and vault
hygiene/consolidation do **not** re-push.

## Shared Vaults (Teams v0)

A **team** in Engram v0 is just a set of devices that share two things:

1. The same vault **passphrase** — sync keys derive from it alone
2. The same **`vault_id`** and sync server in their sync config

There are no accounts, invitations, roles, or revocation in v0 — joining is
purely cryptographic. The sync server stays a dumb relay: it sees device IDs
and blob counts, never names or content.

### Joining a team

**Initiator** (first member) — configure sync with a shared vault id:

```json
{
  "sync": {
    "enabled": true,
    "server_url": "https://sync.example.com",
    "api_key": "...",
    "vault_id": "team-acme",     // any string the team agrees on
    "name": "Acme Core Team"     // optional display name
  }
}
```

**Teammate** — fresh vault, same passphrase, same `vault_id`:

```bash
# 1. Init a fresh vault with the SAME passphrase (each device gets its own device_id)
engram init --vault ~/.engram/vault

# 2. Point sync at the shared vault
curl -X PATCH http://localhost:8787/config \
  -H 'Content-Type: application/json' \
  -d '{"sync": {"enabled": true, "server_url": "https://sync.example.com", "api_key": "...", "vault_id": "team-acme"}}'

# 3. Restart with the shared passphrase
engramd --vault ~/.engram/vault --passphrase "<same passphrase>"

# 4. Force the first sync (or wait interval_secs)
curl -X POST http://localhost:8787/sync/now
```

Memories converge in both directions, and edits propagate (see above). A
teammate who only pulls is effectively a read-only member — nothing forces
them to push.

> **Gotcha — set `vault_id` explicitly.** When `vault_id` is unset, the
> daemon falls back to the vault **directory name**. Two devices pointed at
> the "same" shared vault but with different directory names (or different
> fallback derivations across binary versions) silently split into two
> vaults on the server: blobs land under different vault ids and never
> converge. Always PATCH an explicit `vault_id` before syncing.

### Seeing the team

- **Settings → Sync & Team** in the web UI: vault ID (copy button), team
  name, sync enable/URL/interval, save, "Sync now", device roster with
  `this device` badge, reachability, last push/pull cursors, and the
  honest-caveats list.
- `GET /teams/status` on each daemon aggregates the same data server-side,
  so the sync `api_key` never reaches the browser.
- `GET /v1/vaults/{vault_id}/devices` on the sync server lists devices that
  have pushed blobs to that vault.

## Self-Hosting

The sync server is designed to be self-hosted:

- **No external dependencies:** Just SQLite + HTTP
- **Stateless:** No user accounts, no sessions — just API keys
- **Resource-light:** ~20 MB RAM, minimal CPU
- **Docker:** `docker run -v ./data:/data -p 8788:8788 -e SYNC_API_KEYS=... ghcr.io/pixelphantomai/engramd-sync:latest`

### Production Deployment

```nginx
# Example nginx reverse proxy for HTTPS
server {
    listen 443 ssl;
    server_name sync.example.com;

    ssl_certificate /etc/letsencrypt/live/sync.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/sync.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8788;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        client_max_body_size 10M;
    }
}
```

## Deletion & Tombstones

When you delete a memory on one device, a **tombstone** is created and pushed
to the sync server. Other devices pull the tombstone and delete the
corresponding memory locally.

- Tombstones are tracked in `~/.engram/vault/tombstones.jsonl`
- The sync server retains tombstones for **30 days**, then physically removes them
- After 30 days, a new device syncing for the first time will not receive
  the deletion — but the original device still has the memory deleted

## Troubleshooting

### Sync not working

1. Check sync is enabled: `GET /config` → `sync.enabled: true`
2. Check sync status: `GET /sync/status` → `remote_reachable`
3. Check engramd was started with `--passphrase` — sync requires it
4. Check the sync server is running and reachable
5. Look at engramd logs for sync errors

### "HMAC verification failed"

This means a blob was tampered with in transit or your passphrase differs
between devices. Verify:
- Both devices use the **exact same passphrase** (case-sensitive)
- The sync server's SSL certificate is valid (no MITM)

### Memories not appearing on second device

- Confirm both devices point to the same sync server
- Check the second device's clock: `GET /sync/status` → `local_clock`
  (should be > 0 after first pull)
- First sync may take up to `interval_secs` (default 60s)

## API Reference

### `GET /sync/status`

Returns current sync state and remote server health. No authentication required
(local loopback only).

### `GET /config` (sync section)

```json
{
  "sync": {
    "enabled": true,
    "server_url": "http://localhost:8788",
    "api_key": "••••••••",
    "interval_secs": 60
  }
}
```

### `PATCH /config`

Update sync settings. The `api_key` field accepts plaintext on write but is
always masked (`••••••••`) on read. The `sync` block merges field-wise:
partial patches never erase `vault_id` or `api_key`.

### `GET /teams/status`

Team roster + reachability, aggregated server-side (see Shared Vaults).

### `GET /v1/vaults/{vault_id}/devices` (sync server)

Device roster for a vault. Requires `Authorization: Bearer <api_key>`.

## Migration from Other Memory Systems

If you're migrating from another memory system:

1. Export your existing memories as JSONL (one JSON object per line)
2. Import into Engram: `engram import --file memories.jsonl`
3. Configure sync — imported memories will be pushed to the server
4. Set up other devices — they'll pull the imported memories

## Limitations

- **No partial sync:** The entire vault syncs. Selective sync (by project/tag)
  is planned for a future release.
- **No offline conflict resolution:** LWW is simple but can lose data if two
  devices edit the same memory offline and then sync.
- **Single sync server:** Multi-server replication is not yet supported.
  Run the sync server behind a load balancer for HA.
- **Shared vaults are trust-based:** membership = shared passphrase. There
  is no revocation, per-member audit, or role separation yet (teams v0). If
  someone leaves a team with the passphrase, the team must re-key a new
  vault.
- **Any sync-server API-key holder can read any vault on that server**
  (blobs stay E2E-encrypted, but the key authenticates all vault operations).
- **Machine-keyed vaults cannot sync:** a passphrase is required at daemon
  startup.
- **Device roster counts pushes only:** a teammate who has only pulled
  appears in the roster after their first push.
- **Hygiene/consolidation deltas don't re-push:** hygiene deletes and weekly
  consolidation promotions change rows without bumping `modified_at`, so
  those specific deltas don't propagate until the row changes again
  (deliberate — avoids bulk re-push storms).
- **Global push cursor can strand older edits:** push selects memories
  strictly after a single `last_push` timestamp, and pull advances that
  cursor past the newest pulled blob. If an older un-pushed memory sits
  behind a pulled blob's `modified_at` (e.g. after a cursor rewind or a
  re-configured vault), it is skipped on the next cycle and stays local.
  A per-memory push cursor (or push retry) is the proper fix — see
  `engramd/sync_client.rs`.
