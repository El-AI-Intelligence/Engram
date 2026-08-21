# Engram by El AI Intelligence — API Surface Design

**Date:** 2026-08-05  
**Status:** Draft for review  
**Service name:** `engramd`

---

## 1. Design Principles

1. **REST for control, streaming for context.** CRUD operations are RESTful JSON. Context assembly has both a request/response endpoint and a streaming SSE endpoint for real-time agents.
2. **One API, one backend.** All endpoints go through the `MemoryBackend` trait. The deployment can be QemCache→VaultStore (production) or VaultStore directly (testing).
3. **Default-local, optionally-remote.** The server listens on `127.0.0.1:8787` by default. TLS + auth are opt-in for remote access.
4. **OpenAI-compatible context output.** The context assembly endpoint returns `messages` arrays that drop directly into any OpenAI/Anthropic API call.
5. **No mandatory cloud dependency.** Everything works offline. Embedding generation is configurable (local model or API).

---

## 2. Authentication

Engram by El AI Intelligence is a **single-user, single-vault** daemon. There is no user system, no
JWT issuance, no `/auth/token` endpoint, and no admin API. Auth is exactly
one mechanism, implemented in `crates/engramd/src/auth.rs`:

### 2.1 Local mode (default)

No auth. Listens on loopback only. Single-user vault.

```
engramd                    # starts on 127.0.0.1:8787, no auth
```

### 2.2 Bearer-token mode

When the `ENGRAMD_API_KEY` environment variable is set, **every route except
`/health`** requires the header:

```
Authorization: Bearer <ENGRAMD_API_KEY>
```

There is no token minting — the key is the one you set. Requests without the
header (or with the wrong key) get `401 Unauthorized`. If the daemon binds to
a non-loopback address without `ENGRAMD_API_KEY` set, startup is refused.

### 2.3 Production deployment (engram.ellmstack.dev)

The daemon still binds loopback. A Caddy reverse proxy terminates TLS and
enforces **HTTP Basic auth** on the API paths. The vault SPA at `/app` is a
public shell showing a branded login screen (`#/login`) that sends an
explicit `Authorization: Basic` header — Caddy strips the `WWW-Authenticate`
challenge from API 401s, so the browser never shows the native popup.
Caddy injects `Authorization: Bearer <ENGRAMD_API_KEY>` on upstream
requests from the daemon's env file, so browsers authenticate once via the
login form and the daemon sees its own Bearer key. Clients should never
send the API key themselves.

### 2.4 Multi-tenancy

Not supported. One vault per daemon process; run multiple instances with
different `ENGRAM_VAULT` paths if you need isolated vaults.

---

## 3. REST API Reference

### 3.1 Memories

#### `POST /memories` — Capture a new memory

```json
// Request
{
  "content": "User asked about Rust async trait bounds. Explained Send + Sync requirements.",
  "layer": "episodic",        // "episodic" | "semantic" | "imagined"
  "source": "interaction",    // see MemorySource enum
  "scope": "moment",          // "moment" | "episode" | "narrative" | "rule"
  "content_type": "text",     // "text" | "frames" | "conversation" | "context"
  "tags": ["rust", "async", "traits"],
  "project": "axiom-os",
  "valence": 0.7,
  "context": {                 // arbitrary metadata
    "session_id": "sess_abc",
    "turn": 3
  },
  "links_to": [                // optional: create links on capture
    { "target_id": "mem_xyz", "link_type": "temporal", "weight": 0.8 }
  ],
  "occurred_at": "2026-08-05T14:30:00Z"  // optional, defaults to now
}

// Response 201
{
  "id": "eng_a1b2c3d4e5f6g7h8",
  "qem_code": "0x02A3F142",   // computed QEM code
  "strength": 1.0,
  "created_at": "2026-08-05T14:30:01Z"
}
```

#### `GET /memories/:id` — Retrieve a single memory

```json
// Response 200
{
  "id": "eng_a1b2c3d4e5f6g7h8",
  "layer": "episodic",
  "source": "interaction",
  "scope": "moment",
  "content_type": "text",
  "content": "User asked about Rust async trait bounds...",
  "strength": 0.87,
  "valence": 0.7,
  "imagined": false,
  "grounded": true,
  "retrieval_count": 4,
  "tags": ["rust", "async", "traits"],
  "project": "axiom-os",
  "links_out": [
    { "target_id": "mem_xyz", "weight": 0.8, "link_type": "temporal" }
  ],
  "evidence": [],
  "created_at": "2026-08-05T14:30:01Z",
  "modified_at": "2026-08-06T09:12:44Z",  // bumped on content mutations; sync push cursor
  "last_retrieved": "2026-08-05T16:00:00Z",
  "occurred_at": "2026-08-05T14:30:00Z",
  "context": { "session_id": "sess_abc", "turn": 3 }
}
```

`modified_at` is set to `created_at` on capture and bumped only by content
mutations — capture, `PATCH /memories/:id`, `POST /memories/:id/ground`,
`POST /memories/:id/mark-noise`, dedupe strength bumps, and
`POST /memories/link` (source row). Plain reads (`touch`) and vault
hygiene/consolidation do **not** bump it. It is the sync push cursor: an
edit to an old memory re-pushes it on the next cycle, and the receiving
device preserves the original value so blobs don't echo back.

#### `POST /memories/search` — Search memories

```json
// Request
{
  "query": "Rust async traits",       // text search (FTS5)
  "embedding": [0.1, 0.2, ...],       // optional: hybrid vector search
  "layer": "episodic",                 // optional filter
  "scope": null,
  "source": null,
  "tags": ["rust"],
  "project": "axiom-os",
  "min_strength": 0.2,
  "created_after": "2026-08-01T00:00:00Z",
  "exclude_session": "sess_current",
  "sort_by": "relevance",             // "strength" | "recency" | "valence" | "relevance"
  "limit": 20,
  "offset": 0
}

// Response 200
{
  "results": [ /* MemoryEntry objects */ ],
  "total": 142,
  "search_type": "fts5",              // "qem_cache" | "fts5" | "vector" | "hybrid" | "like"
  "took_ms": 2.3
}
```

#### `PATCH /memories/:id` — Update metadata

```json
// Request — all fields optional
{
  "tags": ["rust", "async", "traits", "send-sync"],
  "valence": 0.9,
  "project": "axiom-os"
}
// Response 200: full updated MemoryEntry
```

#### `DELETE /memories/:id` — Delete a memory

```
Response 204 (no content)
```

#### `POST /memories/link` — Link two memories

```json
// Request
{
  "source_id": "eng_a1b2",
  "target_id": "eng_c3d4",
  "link_type": "causal",
  "weight": 0.9
}
// Response 201
```

#### `GET /memories/:id/links` — Get links

```json
// Response 200
{
  "outgoing": [ { "target_id": "...", "weight": 0.8, "link_type": "causal" } ],
  "incoming": [ { "source_id": "...", "weight": 0.6, "link_type": "associative" } ]
}
```

#### `GET /memories/:id/related` — Related memories (link traversal)

```json
// Query: ?limit=10
// Response 200
{
  "source_id": "eng_a1b2",
  "related": [ /* MemoryEntry objects, sorted by link weight */ ]
}
```

#### `POST /memories/:id/ground` — Ground an imagined memory

```
Response 200: { "grounded": true, "strength": 1.0 }
```

---

### 3.2 Context Assembly

#### `POST /context/assemble` — Build a context window

The flagship endpoint. Takes a query + configuration, returns an OpenAI-compatible `messages` array.

```json
// Request
{
  "query": "What should I work on next?",
  "system_prompt": "You are a helpful AI assistant with memory.",
  "character_bias": null,             // optional VIA strengths
  "token_budget": 8192,
  "config": {
    "high_priority_reserve": 0.60,
    "max_recent_turns": 12,
    "include_compacted": true,
    "max_engrams": 10,
    "include_world_context": true,
    "include_narratives": true,
    "llm_summarization": false        // use extractive summarization
  },
  "session_id": "sess_current",       // exclude this session's own memories
  "recent_history": [                  // optional: provide recent turns
    { "role": "user", "content": "I'm working on the ELLM kernel refactor." },
    { "role": "assistant", "content": "Got it — focusing on the smc_kernel module." }
  ],
  "world_context": {                   // optional: current environment
    "app": "code",
    "title": "smc_kernel/mod.rs — axiom-os",
    "time": "2026-08-05T16:30:00Z"
  }
}

// Response 200
{
  "messages": [
    { "role": "system", "content": "You are a helpful AI assistant with memory." },
    { "role": "system", "content": "[MEMORY from last week]: You discussed Rust async trait bounds and the Send+Sync requirements for the smc_kernel..." },
    { "role": "system", "content": "[MEMORY from 3 days ago]: Debug session — anticipation cache key collisions were producing wrong answers..." },
    { "role": "system", "content": "[SUMMARY of 8 previous turns]: decided to refactor smc_kernel mod.rs, agreed on trait-based approach, plan to extract QEM..." },
    { "role": "user", "content": "I'm working on the ELLM kernel refactor." },
    { "role": "assistant", "content": "Got it — focusing on the smc_kernel module." },
    { "role": "user", "content": "What should I work on next?" }
  ],
  "metadata": {
    "total_tokens": 1247,
    "budget": 8192,
    "utilization": 0.15,
    "engrams_retrieved": 2,
    "narratives_retrieved": 1,
    "history_turns": 2,
    "slots_compacted": 3,
    "retrieval_took_ms": 4.7,
    "assembly_took_ms": 0.3
  }
}
```

#### `GET /context/stream` — Streaming context (SSE)

For real-time agents that need incremental context updates:

```
GET /context/stream?session_id=sess_current&token_budget=8192

// SSE events:
event: snapshot
data: { "messages": [...], "metadata": {...} }

event: delta
data: { "added": [{ "role": "system", "content": "[NEW MEMORY]: ..." }], "budget_remaining": 7000 }

event: delta
data: { "added": [{ "role": "system", "content": "[NEW MEMORY]: ..." }], "budget_remaining": 6500 }

event: heartbeat
data: { "total_memories": 1423, "qem_hit_rate": 0.85 }
```

---

### 3.3 Consolidation & Maintenance

#### `POST /consolidate/decay` — Trigger Ebbinghaus decay

```
Response 200:
{
  "strengthened": 12,       // recently retrieved, strengthened
  "decayed": 342,            // not accessed, decayed
  "pruned": 5,               // dropped below 0.01 threshold
  "took_ms": 45.2
}
```

#### `POST /consolidate/weekly` — Run weekly consolidation

```
Response 200:
{
  "promoted_to_semantic": 3,   // episodic with ≥5 retrievals
  "pruned_imagined": 7,        // imagined with strength < 0.05
  "narratives_updated": 1,     // new narratives distilled
  "rules_crystallized": 0,     // rules extracted (future)
  "took_ms": 120.5
}
```

#### `POST /consolidate/narratives` — Distill narratives from episodes

```json
// Request
{
  "project": "axiom-os",
  "min_episodes": 2,
  "llm_summarization": true
}
// Response 200: { "narratives_created": 2, "narratives_updated": 1 }
```

#### `GET /consolidate/history` — Consolidation run history

```json
// Response 200
{
  "runs": [
    {
      "id": "cons_20260805_030000",
      "type": "weekly",
      "run_at": "2026-08-05T03:00:00Z",
      "episodes_processed": 45,
      "semantics_created": 3,
      "engrams_decayed": 342,
      "notes": "Auto-run during idle cycle"
    }
  ]
}
```

---

### 3.4 Patterns & Analytics

#### `POST /analytics/patterns` — Detect temporal patterns

```json
// Request
{ "query": "debugging", "min_engrams": 5 }
// Response 200
{
  "pattern": {
    "query": "debugging",
    "sample_size": 23,
    "peak_day": "Thursday",
    "day_strength": 2.5,
    "peak_period": "evening",
    "period_strength": 3.1,
    "description": "I've noticed you tend to do this on Thursdays (35% of the time), usually in the evening"
  }
}
```

#### `GET /analytics/stats` — Vault statistics

```json
// Response 200
{
  "total_memories": 1423,
  "by_layer": { "episodic": 1200, "semantic": 180, "imagined": 43 },
  "by_source": { "interaction": 800, "window": 300, "agent": 200, "consolidation": 100, "chat": 23 },
  "by_scope": { "moment": 1100, "episode": 280, "narrative": 40, "rule": 3 },
  "total_links": 892,
  "avg_strength": 0.62,
  "avg_valence": 0.15,
  "qem_hit_rate": 0.85,
  "total_embeddings": 500,
  "vault_size_bytes": 2457600,
  "last_consolidation": "2026-08-05T03:00:00Z",
  "last_decay": "2026-08-05T03:00:00Z"
}
```

---

### 3.5 Import / Export

#### `POST /export` — Export memories

```json
// Request
{
  "format": "jsonl",       // "jsonl" | "json"
  "layer": null,            // optional filter
  "project": "axiom-os",   // optional filter
  "tags": ["rust"],
  "created_after": null,
  "min_strength": 0.1
}
// Response 200: application/x-ndjson
// {"id":"eng_a1","layer":"episodic","content":"...","strength":0.87,...}
// {"id":"eng_b2","layer":"semantic","content":"...","strength":1.2,...}
```

#### `POST /import` — Import memories

```
Content-Type: application/x-ndjson
Body: newline-delimited JSON (same format as export)

Response 200: { "imported": 500, "skipped": 12, "errors": [...] }
```

#### `POST /import/chat-log` — Import from chat logs

```json
// Request
{
  "format": "openai",                  // "openai" | "anthropic" | "axiom"
  "messages": [ /* OpenAI-format messages array */ ],
  "session_id": "imported_session_1",
  "project": "axiom-os",
  "tags": ["imported"]
}
// Response 200: { "memories_created": 45, "episode_created": "eng_ep_xyz" }
```

---

### 3.6 Health & Config

#### `GET /health` — Server health

```json
// Response 200
{
  "status": "ok",
  "version": "0.1.0",
  "vault": "default",
  "uptime_secs": 86400,
  "memories_total": 1423,
  "qem_hit_rate": 0.85,
  "db_size_bytes": 2457600
}
```

#### `GET /config` — Current configuration

```json
// Response 200
{
  "vault_path": "~/.engram/vaults/default",
  "encryption": "sqlcipher",
  "vector_model": "text-embedding-3-small",
  "vector_dimensions": 1536,
  "decay_schedule": "daily",
  "consolidation_schedule": "weekly",
  "qem": {
    "warm_limit": 1000,
    "novelty_window": 100,
    "max_entries": 10000
  },
  "context": {
    "default_budget": 8192,
    "high_priority_reserve": 0.60,
    "max_recent_turns": 12
  }
}
```

#### `PATCH /config` — Update configuration

```json
// Request — all fields optional
{
  "decay_schedule": "daily",
  "context": { "default_budget": 16384 }
}
// Response 200: updated config
```

The `sync` block merges **field-wise** (partial patches never erase
`vault_id` or `api_key`; an `api_key` equal to the read mask `••••••••` is
ignored). Supported fields: `enabled`, `server_url`, `api_key`,
`interval_secs`, `vault_id`, `name`.

The `digest` block also merges field-wise: `enabled` (bool), and `llm`
(either `null` to clear the block, or `{base_url, api_key, model}` merged
into the existing block — masked `api_key` round-trips are ignored, same as
sync).

### 3.7 Sync & Teams

Shared-vault v0 (see SYNC.md): a team is devices sharing one vault
passphrase + `vault_id`. No accounts or revocation.

#### `GET /teams/status` — Team roster + reachability (engramd)

Aggregated server-side so the sync `api_key` never reaches the browser.

```json
// Response 200 — 409 {"error":{"code":"sync_disabled",...}} when sync is off
{
  "vault_id": "team-acme",
  "name": "Acme Core Team",
  "server_url": "https://sync.example.com",
  "remote_reachable": true,
  "devices": [
    { "device_id": "d1…", "last_seen": "2026-08-14T09:00:00Z", "blob_count": 12, "label": "my-laptop", "is_self": true },
    { "device_id": "d2…", "last_seen": "2026-08-14T08:55:00Z", "blob_count": 9, "label": null, "is_self": false }
  ],
  "last_push": "2026-08-14T09:00:00Z",
  "last_pull": "2026-08-14T09:00:00Z",
  "last_push_error": null
}
```

`last_push_error` carries the relay's rejection text from the most recent
failed push (e.g. a 402 quota rejection) and is cleared by the next
successful push — see SYNC.md quota semantics.

#### `GET /v1/vaults/{vault_id}/devices` — Device roster (engramd-sync)

Requires `Authorization: Bearer <api_key>`. A device appears after its
first push **or** after registering a label.

```json
// Response 200
{
  "vault_id": "team-acme",
  "devices": [
    { "device_id": "d1…", "last_seen": "2026-08-14T09:00:00Z", "blob_count": 12, "revoked": false, "label": "my-laptop" }
  ]
}
```

#### `POST /v1/vaults/{vault_id}/devices/register` — Register device label (engramd-sync)

Upserts the calling device's label. Requires an API key scoped to (or
superseding) the vault. The daemon calls this automatically at sync-loop
start; older relays answer 404 and the daemon stays silent.

```json
// Request
{ "device_id": "a1b2c3d4-…", "label": "my-laptop" }   // label ≤ 128 chars, non-empty

// Response 200
{ "vault_id": "team-acme", "device_id": "a1b2c3d4-…", "label": "my-laptop", "registered": true }

// Errors: 400 (empty/oversized device_id or label), 401 (bad key),
//         403 (key not scoped to this vault)
```

### 3.8 Weekly Digest

`GET /digest/weekly` — "what your AI learned about you this week".
Deterministic and local: stats come straight from the vault, themes are
clustered from the vault's own embeddings (tag fallback when unembedded).
No third-party call is ever made unless explicitly requested.

#### `GET /digest/weekly` — Weekly digest

Query params: `days` (default 7, clamped 1–90), `prose` (`1`/`true` — see
below). Returns `409 {"error":{"code":"digest_disabled",...}}` when
`digest.enabled` is false.

```json
// Response 200 (prose omitted when not requested)
{
  "generated_at": "2026-08-15T09:00:00Z",
  "window_start": "2026-08-08T09:00:00Z",
  "window_end": "2026-08-15T09:00:00Z",
  "stats": {
    "live_total": 214,
    "new": 6,
    "reinforced": 18,
    "fading": 9,
    "quarantined": 12,
    "quarantined_new": 2
  },
  "themes": [
    {
      "label": "async stream design decision",
      "count": 3,
      "examples": ["async stream design decision", "tokio spawn best practice"]
    }
  ],
  "new_memories": [
    { "id": "eng_…", "content": "…", "layer": "semantic", "tags": ["work"], "strength": 1.4 }
  ],
  "reinforced": [ "…same shape…" ],
  "fading": [ "…same shape…" ],
  "prose": "This week your AI learned…",
  "llm_configured": true
}
```

Semantics: `new` = live memories created inside the window;
`reinforced` = created earlier but retrieved inside it (the AI actually
used them); `fading` = not retrieved in the window, weakest first;
`quarantined` = imagined-and-ungrounded rows (any age), with
`quarantined_new` = the subset filtered out this window. Memory slices are
capped at 50 each (highlights, not full dumps — the counts are exact).

**Prose (`?prose=1`) is opt-in per request.** It calls the user's own
OpenAI-compatible endpoint configured in `config.json`:

```json
{
  "digest": {
    "enabled": true,
    "llm": {
      "base_url": "http://localhost:11434/v1",
      "api_key": "…",
      "model": "llama3.1"
    }
  }
}
```

Because BYO-key calls bill the user's key, prose is never generated
automatically — only on the explicit request flag (or the UI's "Generate
prose" button). Without a usable `llm` block the request returns
`409 {"error":{"code":"llm_not_configured",...}}`; a failed provider call
returns `502 {"error":{"code":"llm_error",...}}`. `llm.api_key` is masked
(`••••••••`) in `/config` responses and never persisted on masked
round-trips. The LLM only phrases a deterministic prompt built from the
digest data — it never fabricates the numbers.

### 3.9 Accounts (engramd-sync)

Standalone passkey accounts, shipped 2026-08-15 (milestone 1.2). The
relay stores no PII: an account is an opaque UUID plus passkeys, session
tokens and API keys are stored only as sha256 hashes. The vault SPA
calls these endpoints directly with `Content-Type: application/json`
(CORS on the relay allows GET/POST/DELETE from any origin). Errors use
`{"error": {"code": ..., "error": msg}}`; successful auth calls use the
shapes below.

#### `POST /auth/register/start` — Begin passkey registration

```json
// Request
{ "origin": "http://localhost:8787" }   // must be in the relay's --origin allow-list

// Response 200 — challenge.publicKey is browser-API-compatible
// (camelCase options); challenge/user.id/allowCredentials[].id are
// base64url-no-pad and must be decoded to ArrayBuffers client-side.
// excludeCredentials is omitted while empty (new-account registration).
{
  "challenge_id": "7f4c…",
  "challenge": {
    "publicKey": {
      "rp": { "id": "localhost", "name": "Engram Sync" },
      "user": { "id": "…base64url…", "name": "engram-account", "displayName": "Engram Account" },
      "challenge": "…base64url…",
      "pubKeyCredParams": [ { "type": "public-key", "alg": -7 }, { "type": "public-key", "alg": -257 } ],
      "timeout": 300000,
      "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "required" },
      "attestation": "none",
      "extensions": {
        "credProps": true,
        "credentialProtectionPolicy": "userVerificationRequired",
        "enforceCredentialProtectionPolicy": false,
        "uvm": true
      }
    }
  }
}

// Errors: 401 {"code":"origin_not_allowed"} for origins outside the allow-list
```

A valid session on the request attaches the new passkey to that account;
without one, the ceremony creates a brand-new account.

#### `POST /auth/register/finish` — Complete registration

```json
// Request — registration is the browser's PublicKeyCredential JSON
// (id/rawId base64url, clientDataJSON/attestationObject base64).
{
  "origin": "http://localhost:8787",
  "challenge_id": "7f4c…",
  "registration": {
    "id": "…base64url…", "rawId": "…base64url…", "type": "public-key",
    "response": { "clientDataJSON": "…", "attestationObject": "…" }
  }
}

// Response 200 — session_token is the Bearer token for /account*
{
  "account_id": "5c1f…",
  "session_token": "…",
  "already_registered": true   // only when the same passkey was submitted twice
}

// Errors: 400 invalid_challenge | bad_registration | registration_failed
```

#### `POST /auth/login/start` — Begin passkey login

```json
// Request
{ "origin": "http://localhost:8787" }

// Response 200 — same nested shape as register: challenge.publicKey
// is browser-API-compatible (challenge/allowCredentials[].id are
// base64url-no-pad and need ArrayBuffer conversion client-side).
{
  "challenge_id": "9a2e…",
  "challenge": {
    "publicKey": {
      "challenge": "…base64url…",
      "timeout": 300000,
      "rpId": "localhost",
      "allowCredentials": [ { "type": "public-key", "id": "…base64url…" } ],
      "userVerification": "required"
    }
  }
}

// Errors: 409 {"code":"no_passkeys"} — no passkeys registered yet
```

#### `POST /auth/login/finish` — Complete login

```json
// Request — credential is the browser's PublicKeyCredential JSON.
{
  "origin": "http://localhost:8787",
  "challenge_id": "9a2e…",
  "credential": {
    "id": "…base64url…", "rawId": "…base64url…", "type": "public-key",
    "response": { "clientDataJSON": "…", "authenticatorData": "…", "signature": "…", "userHandle": "…base64url…" }
  }
}

// Response 200
{ "account_id": "5c1f…", "session_token": "…" }

// Errors: 400 invalid_challenge | bad_credential | unknown_credential,
//         401 auth_failed
```

#### `POST /auth/logout` — End a session

```json
// Request — Bearer session token.
// Response 200: { "logged_out": true }   (false when the token was already dead)
```

#### `GET /account` — Account + quotas + keys (Bearer session)

```json
// Response 200 — 401 {"code":"invalid_session"} when signed out
{
  "account_id": "5c1f…",
  "quota": {
    "devices": 1,        // 0 = unlimited; server default unless overridden
    "bytes": 1073741824, // per account, set by billing (1.3)
    "devices_used": 1,
    "bytes_used": 12345
  },
  "keys": [
    { "id": "…", "key_prefix": "en_", "rate": 100.0, "vault_id": "team-acme",
      "created_at": "2026-08-15T09:00:00Z", "revoked": false }
  ],
  "vaults": ["team-acme"]
}
```

`quota.*_used` is measured over all vaults the account's unrevoked keys
reach: distinct active devices and stored ciphertext bytes (see SYNC.md).

#### `GET /account/vaults` — Vaults this account can pull (Bearer session)

Powers the browser unlock view's vault picker. Same visibility rule as
`GET /account`: an unscoped unrevoked key means every vault; otherwise only
vaults named by unrevoked keys. Scoped vaults with zero blobs are omitted
(nothing to unlock).

```json
// Response 200 — 401 {"code":"invalid_session"} when signed out
{
  "vaults": [
    { "vault_id": "engram-local", "blob_count": 398,
      "latest_sync": "2026-08-17T09:00:00Z" }
  ]
}
```

`blob_count` counts stored encrypted blob **versions**, not memories;
`latest_sync` is the newest blob `created_at`.

#### Pull accepts account sessions (read-only)

`GET /v1/vaults/{vault_id}/pull` authenticates API keys first; **only on a
401** does it fall back to the account session, with vault visibility
derived as above (403 `"session is not authorized for this vault"` when
the account's keys don't cover it). Session pulls are rate-limited in a
separate bucket capped at the account's highest key rate — a 429 is never
retried against the other auth path. Sessions remain read-only on the
data plane: push, device and stats routes still require API keys. See
SYNC.md, "Browser Unlock (read-only)".

#### `POST /account/keys` — Mint an API key (Bearer session)

```json
// Request — vault_id optional; omitted/null = every vault the account reaches
{ "vault_id": "team-acme" }

// Response 200 — api_key is shown exactly once; the relay stores sha256 only
{
  "key_id": "…",
  "api_key": "en_…43 base64url chars…",
  "key_prefix": "en_",
  "rate": 100.0,
  "vault_id": "team-acme",
  "created_at": "2026-08-15T09:00:00Z"
}
```

#### `DELETE /account/keys/{key_id}` — Revoke an API key (Bearer session)

```json
// Response 200: { "key_id": "…", "revoked": true }
// Errors: 404 {"code":"key_not_found"} — not-found and not-yours are
// identical so key ids can't be enumerated across accounts.
```

#### `POST /devices/pair-codes` — Mint a pairing code (Bearer session)

WARP-style device onboarding (shipped 2026-08-16). Codes are single-use,
expire after 10 minutes, and are stored only as sha256 (`code_hash`).
Live codes are capped at 5 per account.

```json
// Response 200 — code is shown exactly once
{ "code": "ENG-4F7K-9Q2M-D8T3", "expires_in": 600 }

// Errors: 401 invalid_session | 409 too_many_codes
```

#### `POST /devices/pair` — Redeem a pairing code for an API key (no auth)

The code IS the credential. Redeeming consumes it and mints an account
API key (unscoped in v1 — every vault the account reaches; see the v1
caveat in SYNC.md). A global token bucket (burst 5, refill ~5/s) bounds
guessing attempts.

```json
// Request
{ "code": "ENG-4F7K-9Q2M-D8T3", "device_label": "my-laptop" }

// Response 200 — api_key is shown exactly once; the relay stores sha256 only
{
  "key_id": "…",
  "api_key": "en_…43 base64url chars…",
  "key_prefix": "en_",
  "rate": 100.0,
  "vault_id": null,
  "created_at": "2026-08-16T09:00:00Z"
}

// Errors: 401 invalid_pairing_code (unknown or already used) |
//         401 expired_pairing_code | 429 rate_limit_exceeded
```

---

## 4. gRPC API (for high-performance agent loops)

For agents making hundreds of memory lookups per second, REST is too chatty. gRPC provides:

```protobuf
service MemoryVault {
  // Capture
  rpc Capture(CaptureRequest) returns (CaptureResponse);

  // Retrieve
  rpc Retrieve(RetrieveRequest) returns (MemoryEntry);

  // Search (unary)
  rpc Search(SearchRequest) returns (SearchResponse);

  // Streaming context assembly
  rpc AssembleContext(stream ContextUpdate) returns (stream AssembledMessage);

  // Bulk operations
  rpc BulkCapture(stream CaptureRequest) returns (BulkCaptureResponse);
  rpc BulkLink(stream LinkRequest) returns (BulkLinkResponse);

  // Consolidation triggers
  rpc TriggerDecay(TriggerDecayRequest) returns (DecayReport);
  rpc TriggerConsolidation(TriggerConsolidationRequest) returns (ConsolidationReport);
}
```

---

## 5. SDK Sketch

### 5.1 Python

```python
from engramd import MemoryVault

vault = MemoryVault()  # connects to localhost:8787
# vault = MemoryVault(api_key="engram_api_key_xxx", base_url="https://vault.example.com")

# Capture
mem = vault.capture(
    content="User asked about Rust async traits.",
    layer="episodic",
    tags=["rust", "async"],
    valence=0.7,
)

# Search
results = vault.search("Rust async traits", limit=10)

# Context assembly — drop directly into any LLM call
messages = vault.assemble(
    query="What should I work on next?",
    system_prompt="You are a helpful AI assistant.",
    token_budget=8192,
    recent_history=[
        {"role": "user", "content": "I'm refactoring the ELLM kernel."},
    ],
)

import openai
response = openai.chat.completions.create(
    model="gpt-4o",
    messages=messages,  # ← engramd output, directly
)
```

### 5.2 TypeScript

```typescript
import { MemoryVault } from "@ellm/engramd";

const vault = new MemoryVault();
// const vault = new MemoryVault({ apiKey: "...", baseUrl: "https://..." });

// Capture
const mem = await vault.capture({
  content: "User asked about Rust async traits.",
  layer: "episodic",
  tags: ["rust", "async"],
});

// Context assembly
const { messages, metadata } = await vault.assembleContext({
  query: "What should I work on next?",
  systemPrompt: "You are a helpful AI assistant.",
  tokenBudget: 8192,
});

// Drop into Anthropic SDK
import { Anthropic } from "@anthropic-ai/sdk";
const anthropic = new Anthropic();
const response = await anthropic.messages.create({
  model: "claude-sonnet-5-20251001",
  messages: messages.filter(m => m.role !== "system"),
  system: messages.find(m => m.role === "system")?.content,
});
```

---

## 6. Deployment

### 6.1 Single binary

```
engramd                          # local-only, no auth
engramd --port 8787              # custom port
engramd --vault my-vault         # named vault
engramd --auth api-key           # enable API key auth
engramd --tls-cert cert.pem      # TLS for remote access
engramd --vector-model all-MiniLM-L6-v2  # local embedding model
```

### 6.2 Docker

```dockerfile
FROM rust:1.80 AS builder
# ... build engramd

FROM debian:bookworm-slim
COPY --from=builder /target/release/engramd /usr/local/bin/
VOLUME /root/.engram/vaults
EXPOSE 8787
CMD ["engramd", "--port", "8787"]
```

### 6.3 Systemd service

```ini
[Unit]
Description=ELLM Engram Memory Vault
After=network.target

[Service]
ExecStart=/usr/local/bin/engramd
Restart=always
User=engram
Environment=ENGRAM_VAULT_PATH=/var/lib/engram/vaults/default

[Install]
WantedBy=default.target
```

---

## 7. Rate Limiting & Quotas

| Tier | Memories | Searches/sec | Context assemblies/sec | Vector searches/sec | Vault size |
|------|----------|-------------|----------------------|--------------------|------------|
| Free (local) | Unlimited | Unlimited | Unlimited | 10/s | Unlimited |
| Free (cloud) | 10,000 | 10/s | 5/s | 5/s | 100 MB |
| Pro | 100,000 | 50/s | 20/s | 20/s | 1 GB |
| Enterprise | Unlimited | Custom | Custom | Custom | Unlimited |

Local mode has no limits — it's your hardware, your vault.
