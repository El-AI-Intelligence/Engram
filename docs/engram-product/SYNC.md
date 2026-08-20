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

#### Managed relay (no self-hosting)

A managed relay runs at **https://sync.ellmstack.dev** — same dumb-pipe
binary, same zero-knowledge properties (deployed 2026-08-15; operational
runbook in [`deploy/sync-relay.md`](../../deploy/sync-relay.md)). Create an
account in the vault UI (Settings → Account, passkey sign-in), mint an API
key there, and connect this device in one click. Set `server_url` to the URL
above and `api_key` to your key — everything else in this document applies
unchanged. See [Accounts & Passkeys](#accounts--passkeys) for the account
model.

#### Linking a device (`engram link`, WARP-style, recommended)

Copying API keys by hand is the manual path. The smooth path is one-click
machine linking, like Cloudflare WARP — no codes to type:

1. Make sure the machine has an Engram account you can sign into (create one
   from the vault login screen — "New here? Create an account").
2. On the machine:

```bash
engram link
```

Your browser opens a "Link this machine" page. Sign in (if prompted) and
click the one button. The CLI finishes on its own — no key pasting.

The account API key never travels in plaintext: the CLI mints an ephemeral
X25519 keypair, the relay seals the freshly issued key to it
(ChaCha20-Poly1305, AAD-bound to the intent), and the CLI decrypts it
exactly once (single-shot delivery, 10-minute TTL). A leaked confirm URL
yields only an undecryptable blob. Full details in
[AUTHENTICATION.md](AUTHENTICATION.md#device-linking-engram-link).

Both flows write the sync block into `~/.engram/vault/config.json` (merging
with an existing config — they work on existing vaults, not just fresh
ones). Restart the daemon with `ENGRAM_PASSPHRASE` set and the device
appears in the roster after its first push.

#### Pairing a device (headless / SSH)

Machines without a browser can still use a one-time pairing code:

1. In the vault UI, open **Settings → Account & Sync**, sign in, and click
   **Pair a device (headless)**. The site shows a single-use code
   (`ENG-XXXX-XXXX-XXXX`, expires in 10 minutes).
2. On the machine:

```bash
engram pair ENG-XXXX-XXXX-XXXX
```

The device's roster label is taken from `--name` if given, otherwise the
vault's sync name, otherwise the machine hostname — both commands replace
a placeholder label (`unknown`) in the vault's `device.json` automatically.
`engram link` refuses to re-link a vault that already has a sync key unless
you pass `--force` (the old key stays active until revoked in
Account & Sync → API keys).

Notes: pairing codes are single-use, 10-minute TTL, and stored server-side
only as sha256 — the plaintext is shown once. Issued keys are **unscoped in
v1** (account-wide); the vault UI can also mint per-vault keys manually.
Machine-keyed vaults (created without a passphrase) cannot link or pair —
sync keys derive from the passphrase, so both commands refuse them.

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

The cursor is **per-memory** (schema v6): each row carries a `synced_at`
stamp and re-pushes whenever `modified_at` moves past it. Pulling a blob
stamps `synced_at` on the local row at apply time, and the pull path never
touches the push filter — so pulled blobs can't strand older local edits
(a failure mode of the previous global `last_push` cursor).

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

**Teammate** — join with `engram join`:

```bash
# 1. Fresh vault + passphrase + sync preset in one step.
#    Omit --vault-id to derive it from the passphrase (same id ⇒ same vault).
engram join --server-url https://sync.example.com

# 2. Start the daemon with the shared passphrase
engramd --vault ~/.engram/vault --passphrase "<same passphrase>"

# 3. Force the first sync (or wait interval_secs)
curl -X POST http://localhost:8787/sync/now
```

`engram join` refuses to touch a vault that already has memories — it is
for fresh vaults only (each device gets its own `device_id` on first
daemon start). To sync an existing vault, configure the sync block via
Settings → Sync & Team (or edit its `config.json`) instead. Without the
CLI, the same flow works manually:

```bash
# 1. Init a fresh vault with the SAME passphrase
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
> daemon derives it from the sync passphrase (same passphrase ⇒ same id,
> so teammates converge without configuration) and pins the derived value
> into `config.json` on first sync. Early binaries derived it from the
> vault **directory name** instead — different directory names across
> devices/binary versions silently split one team into two vaults on the
> server. Explicit `vault_id` still wins over the fallback, and is
> recommended for anything you want to name.

### Seeing the team

- **Settings → Sync & Team** in the web UI: vault ID (copy button), team
  name, sync enable/URL/interval, save, "Sync now", device roster with
  `this device` badge, reachability, last push/pull cursors, and the
  honest-caveats list.
- `GET /teams/status` on each daemon aggregates the same data server-side,
  so the sync `api_key` never reaches the browser.
- `GET /v1/vaults/{vault_id}/devices` on the sync server lists devices that
  have pushed blobs to that vault.

## Accounts & Passkeys

The relay has **standalone passkey accounts** (shipped 2026-08-15, milestone
1.2). They are deliberately minimal and pseudonymous:

- **Registration IS sign-up.** There is no email, name, or PII — an account
  is an opaque UUID plus its passkeys. (Billing, which needs an email,
  arrives in roadmap 1.3 as a separate private service that keys accounts by
  the opaque id.)
- **Login is the same ceremony.** The browser offers its passkeys for the
  relay's RP ID; with none registered, login returns 409 `no_passkeys` and
  the UI prompts to register.
- **Sessions are Bearer tokens in `localStorage`** (key
  `engram-sync-session`). Cross-site cookies won't stick between the vault
  UI origin and the relay origin, so the SPA calls the relay directly (CORS
  allows GET/POST/DELETE + `Content-Type`/`Authorization`). Tokens expire
  after 7 days; the relay stores only their sha256, and logout revokes them.

The account panel lives in the vault SPA: **Settings → Account** (passkey
register/sign-in, quota bars, API key list, "Connect this device").

### Account API keys

Minted at `POST /account/keys` (optionally scoped to one `vault_id`):

- Format `en_` + 43 base64url chars. The **full key is returned exactly
  once**; the relay stores only `sha256(full_key)` and a prefix, so keys
  cannot be recovered from a DB leak.
- Revocation is soft (`DELETE /account/keys/{key_id}`) — the row stays as
  an audit trail, the hash stops authenticating.
- Keys authenticate exactly like static keys (`Authorization: Bearer
  <key>`). An account key scoped to a vault also administers that vault
  (device revocation).

### Quota semantics

Accounts have two quota flags — **devices** and **stored bytes** — enforced
on push:

- **0 = unlimited.** New accounts inherit the relay's defaults
  (`--quota-devices` / `--quota-bytes`); per-account overrides live in the
  `accounts` table and will be set by billing (1.3).
- **Usage is measured pre-insert** over the account's vaults: distinct
  active device ids, and the sum of stored ciphertext bytes. Devices count
  **active devices only** — a revoked device drops out.
- **The whole push batch is rejected with 402** before anything is written
  when a projected write would exceed a limit:
  `{"error":{"code":"quota_exceeded","detail":"devices"|"bytes","limit":N,"used":N}}`.
  Projection is REPLACE-aware: overwriting a blob with a smaller one frees
  space (accepted), with a larger one can be rejected.
- **Static `SYNC_API_KEYS` entries are exempt** — quotas apply to account
  keys only.
- A 402 surfaces in the daemon as `last_push_error` in `sync_state.json`,
  visible in the Settings → Sync & Team panel — sync otherwise stays silent.

### RP ID and origins

WebAuthn credentials bind to a **Relying Party ID**, not a URL. The relay
serves passkeys for `--rp-id`, and only accepts ceremonies whose browser
`origin` (the vault UI's `window.location.origin`) is in its `--origin`
allow-list.

- Local dev defaults: `--rp-id localhost --origin http://localhost:8787`.
- Managed relay: `--rp-id ellmstack.dev --origin https://engram.ellmstack.dev`
  (a registrable domain suffix of the UI origin).
- **Changing `--rp-id` orphans every existing passkey** — pick it once.
  A self-hoster serving the vault UI on another domain needs a different
  passkey for that domain, even against the same relay.

### Wildcard loopback

For quick local starts, the relay historically treated **keyless loopback
requests as a superuser**. That wildcard is now narrow: it applies only
while there are no static keys **and** no unrevoked account keys. **The
first minted account key flips the relay to require Bearer auth on
loopback too** (default-secure). Static env keys are unaffected.

## Browser Unlock (read-only)

The web vault UI can open a **read-only mirror** of a synced vault: the
browser pulls the account's encrypted blobs from the relay and decrypts
them client-side with the vault passphrase. The box and the relay never
see plaintext — the passphrase and derived keys live in the tab's memory
only and are wiped on lock, sign-out, or reload.

- **Auth:** pull accepts the account **session token** (Bearer). Vault
  visibility is derived from the account's unrevoked API keys — exactly the
  vaults a device of that account could pull. Sessions cannot push, mint
  keys, or touch device/stat routes; a session's pull rate is capped at the
  account's highest key rate, and rate-limit rejections never fall through
  to another auth path.
- **KDF:** Argon2id (64 MiB, 3 iterations, 4 lanes) with SHA-256
  domain-tag salts — byte-identical to the daemon (tags `axiom-sync-enc-v2`
  / `axiom-sync-hmac-v2`; hash-wasm in the browser, `argon2` crate in
  engramd).
- **Integrity:** every blob is HMAC-verified before decrypt. A mismatch is
  counted and skipped — one corrupt blob never bricks the view; a
  passphrase error (100% mismatch) fails cleanly with "does not match".
- **Merge:** same last-write-wins as the daemon — max `vector_clock` per
  memory, ties broken by `created_at` then `device_id`; deletion tombstones
  drop the memory.
- **Threat model:** a stolen session token can download ciphertext only —
  decryption needs the passphrase, which never leaves the browser. Unlock
  state is per-tab and per-navigation (reload clears it).
- **v1 scope:** read-only (no capture/edit), vaults listed by
  `GET /account/vaults` (`blob_count` counts encrypted blob *versions*,
  not memories), and the hosted shell's CSP pins its relay
  (`sync.ellmstack.dev`) — custom-relay users unlock from their own
  self-hosted UI.

## Self-Hosting

The sync server is designed to be self-hosted:

- **No external dependencies:** Just SQLite + HTTP
- **Accounts are optional:** run with only `SYNC_API_KEYS` for the original
  key-only behavior — passkey accounts, sessions, and quota enforcement
  activate on top of it without changing anything for static keys
- **Resource-light:** ~20 MB RAM, minimal CPU
- **Docker:** `docker run -v ./data:/data -p 8788:8788 -e SYNC_API_KEYS=... ghcr.io/el-ai-intelligence/engramd-sync:latest` (sync image not published by the v0.1.0 release workflow yet — deploy from source, see deploy/sync-relay.md)

### API key scoping & revocation (teams v1)

`SYNC_API_KEYS` entries accept an optional vault scope:

```bash
SYNC_API_KEYS="team-key-abcdefgh123456:100:team-acme;team-alpha+admin"
```

- `key` / `key:rate` — unscoped: all vaults (original behavior)
- `key:rate:vault1;vault2` — only the listed vaults (403 elsewhere)
- `vault+admin` — also grants device revocation on that vault

Revoke a device (admin-scoped key required):

```bash
curl -X DELETE https://sync.example.com/v1/vaults/team-acme/devices/{device_id} \
  -H "Authorization: Bearer <team-key>"
```

The relay then blocks that device's pushes and flags it `revoked`
in the device roster; the revoked device's daemon logs a warning.

> **Honest zero-knowledge boundary:** pulls are vault-scoped, not
> device-scoped, so the relay cannot stop a revoked device from reading
> blobs or decrypting what it already holds. Full removal means a vault
> re-key (new passphrase ⇒ new vault) or per-member key revocation —
> the hosted control plane's job.

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

### Pushes stopped with 402

A quota rejection: `Settings → Sync & Team` shows the exact
`last_push_error` (devices or bytes, limit and used). Delete blobs or
revoke a device to go back under the limit — the next successful push
clears the error. Static `SYNC_API_KEYS` entries are never quota-limited.

### Passkey registration fails in the browser

- The relay validates the browser's origin against its `--origin` list;
  a mismatch surfaces as "RP ID/origin mismatch" in the UI. The vault UI
  must be served from an origin on the relay's allow-list (self-hosters:
  restart the relay with your origin in `--origin`).
- Passkeys bind to the relay's `--rp-id` — an account's passkey works
  from any origin allow-listed for that RP ID, and never from a
  different RP ID.

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
Each device carries `device_id`, `last_seen`, `blob_count`, `revoked`, and
`label` (null until the device registers one).

### `POST /v1/vaults/{vault_id}/devices/register` (sync server)

Upsert this device's label — `{"device_id": "...", "label": "..."}`,
label ≤ 128 chars. Requires an API key scoped to (or superseding) the
vault. The daemon calls this automatically at sync-loop start using the
`label` field in `device.json` (falls back to `"unknown"`, and stays
silent when the relay is older than 1.2). Registering makes a device
appear in the roster even before its first push.

### Account endpoints (sync server)

`POST /auth/register/start|finish`, `POST /auth/login/start|finish`,
`POST /auth/logout`, `GET /account`, `GET /account/vaults`,
`POST /account/keys`, `DELETE /account/keys/{key_id}` — contracts in
API_SURFACE.md, section 3.9.

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
- **Unscoped API keys can read any vault on that server** (blobs stay
  E2E-encrypted, but the key authenticates all vault operations). Scoped
  keys (`key:rate:vault1;vault2`) fix this — see Self-Hosting.
- **Machine-keyed vaults cannot sync:** a passphrase is required at daemon
  startup.
- **Device roster counts pushes only:** a teammate who has only pulled
  appears in the roster after their first push. Registering a device label
  makes it appear immediately (see below).
- **Hygiene/consolidation deltas don't re-push:** hygiene deletes and weekly
  consolidation promotions change rows without bumping `modified_at`, so
  those specific deltas don't propagate until the row changes again
  (deliberate — avoids bulk re-push storms).
- **Deleting `sync_state.json` re-pushes everything:** the per-memory
  cursor resets when no persisted push state exists, so removing the state
  file (or a fresh device joining) re-pushes the full vault on the next
  cycle. Harmless — the first pull bumps the local clock above the remote
  max, so the server accepts — but it is a burst of traffic on large
  vaults.
