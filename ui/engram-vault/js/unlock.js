// ==========================================================================
// Unlock — read-only browser view of a synced vault.
//
// Pulls the account's ENCRYPTED blobs from the sync relay and decrypts them
// client-side with the vault passphrase. The box and the relay never see
// plaintext; the passphrase and derived keys live in this module's memory
// only and are wiped on lock()/sign-out.
//
// Crypto is byte-exact with the daemon (crates/engramd/src/sync_client.rs):
//   KDF    Argon2id v1.3 (hash-wasm), m=65536 KiB, t=3, p=4, 32-byte output;
//          salt = SHA-256(domain_tag)[0..16] — tags axiom-sync-enc-v2 /
//          axiom-sync-hmac-v2. Keys derive from the passphrase ALONE.
//   Cipher AES-256-GCM, 12-byte random nonce PREPENDED to ct+tag, base64.
//   HMAC   HMAC-SHA256 over vault_id ‖ memory_id ‖ device_id ‖
//          vector_clock(LE64) ‖ ciphertext b64 ‖ deleted(u8) ‖ created_at.
//
// Self-contained: main.js imports this module, never the reverse. The one
// shared key with main.js is the session token storage key.
// ==========================================================================

import { argon2id } from './vendor/hash-wasm/index.esm.js';

const ENC_TAG = 'axiom-sync-enc-v2';
const HMAC_TAG = 'axiom-sync-hmac-v2';
const KDF = { parallelism: 4, iterations: 3, memorySize: 65536, hashLength: 32 };
const SESSION_KEY = 'engram-sync-session';  // same key as main.js account client
const PULL_LIMIT = 1000;

// In-memory only — never persisted. null = locked.
let state = null;

export function isUnlocked() { return !!state; }
export function getMeta() { return state ? state.meta : null; }

export function getMemories() {
  if (!state) return [];
  return [...state.memories.values()].sort((a, b) =>
    (b.created_at || '').localeCompare(a.created_at || ''));
}

export function getMemory(id) {
  return state ? state.memories.get(id) || null : null;
}

export function lock() {
  state = null;
}

// ── Relay fetch (session token) ────────────────────────────────────────────
// Errors carry .status; 401 means the account session expired — the token is
// cleared so the caller can prompt a fresh sign-in. Error bodies are read
// defensively: the relay has two shapes ({error: msg} on pull routes,
// {error: {code, error}} on account routes).

async function relayFetch(relay, path) {
  const headers = {};
  const token = localStorage.getItem(SESSION_KEY);
  if (token) headers['Authorization'] = 'Bearer ' + token;
  let r;
  try {
    r = await fetch(String(relay).replace(/\/+$/, '') + path, { headers });
  } catch (e) {
    throw new Error('Cannot reach the sync relay.');
  }
  if (!r.ok) {
    let msg = `${r.status} ${r.statusText}`;
    try {
      const b = await r.json();
      if (typeof b?.error === 'string') msg = b.error;
      else if (b?.error?.error) msg = b.error.error;
    } catch {}
    const e = new Error(msg);
    e.status = r.status;
    if (r.status === 401) localStorage.removeItem(SESSION_KEY);
    throw e;
  }
  return r.json();
}

// Vaults visible to the signed-in account (GET /account/vaults, session auth).
export async function listVaults(relay) {
  const data = await relayFetch(relay, '/account/vaults');
  return (data && data.vaults) || [];
}

// ── KDF ────────────────────────────────────────────────────────────────────

async function deriveKey(tag, passphrase) {
  const salt = new Uint8Array(
    await crypto.subtle.digest('SHA-256', new TextEncoder().encode(tag))
  ).slice(0, 16);
  // Binary Uint8Array of the 32 raw key bytes.
  return new Uint8Array(await argon2id({ password: passphrase, salt, outputType: 'binary', ...KDF }));
}

// ── base64 (standard, padded) ──────────────────────────────────────────────

function b64decode(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// ── HMAC verify — byte-exact with compute_hmac in sync_client.rs ───────────

async function hmacOk(hmacKey, blob) {
  const te = new TextEncoder();
  const clock = new Uint8Array(8);
  new DataView(clock.buffer).setBigUint64(0, BigInt(blob.vector_clock), true);
  const parts = [
    te.encode(blob.vault_id),
    te.encode(blob.memory_id),
    te.encode(blob.device_id),
    clock,
    te.encode(blob.ciphertext),
    new Uint8Array([blob.deleted ? 1 : 0]),
    te.encode(blob.created_at),
  ];
  const data = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let off = 0;
  for (const p of parts) { data.set(p, off); off += p.length; }
  const sig = new Uint8Array(await crypto.subtle.sign('HMAC', hmacKey, data));
  const expected = b64decode(blob.hmac);
  return sig.length === expected.length && sig.every((b, i) => b === expected[i]);
}

// ── Pull loop — limit 1000, since = last created_at, dedup + stall guard ───

async function pullAll(relay, vaultId, onProgress) {
  const blobs = new Map();
  let since = null;
  let stall = 0;
  for (;;) {
    const q = new URLSearchParams({ limit: String(PULL_LIMIT) });
    if (since) q.set('since', since);
    const page = await relayFetch(relay, `/v1/vaults/${encodeURIComponent(vaultId)}/pull?${q}`);
    let added = 0;
    for (const b of page.blobs || []) {
      const id = `${b.memory_id}|${b.vector_clock}|${b.device_id}|${b.created_at}|${b.deleted}`;
      if (!blobs.has(id)) { blobs.set(id, b); added++; }
    }
    if (added === 0 && stall++ >= 3) throw new Error('Sync pull stalled — the relay keeps returning the same page.');
    if (added > 0) stall = 0;
    if (page.blobs && page.blobs.length) since = page.blobs[page.blobs.length - 1].created_at;
    if (onProgress) onProgress({ stage: 'pulling', fetched: blobs.size });
    if (!page.has_more) break;
  }
  return [...blobs.values()];
}

// ── Unlock ─────────────────────────────────────────────────────────────────

export async function unlock(relay, vaultId, passphrase, onProgress) {
  if (!relay || !vaultId || !passphrase) throw new Error('Relay, vault and passphrase are required.');

  const blobs = await pullAll(relay, vaultId, onProgress);

  // Two Argon2id derivations SEQUENTIALLY — 2×64 MiB concurrent would peak at
  // 128 MiB of WASM memory on low-end devices.
  if (onProgress) onProgress({ stage: 'deriving', step: 1 });
  const encRaw = await deriveKey(ENC_TAG, passphrase);
  if (onProgress) onProgress({ stage: 'deriving', step: 2 });
  const hmacRaw = await deriveKey(HMAC_TAG, passphrase);

  const encKey = await crypto.subtle.importKey('raw', encRaw, { name: 'AES-GCM' }, false, ['decrypt']);
  const hmacKey = await crypto.subtle.importKey('raw', hmacRaw, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  encRaw.fill(0); hmacRaw.fill(0);

  // Verify every blob; a mismatch is counted and skipped — one corrupt blob
  // must not brick the view.
  let verified = 0, hmacFailed = 0;
  const good = [];
  for (let i = 0; i < blobs.length; i++) {
    if (await hmacOk(hmacKey, blobs[i])) { verified++; good.push(blobs[i]); }
    else hmacFailed++;
    if (onProgress && (i % 20 === 19 || i === blobs.length - 1)) {
      onProgress({ stage: 'verifying', done: i + 1, total: blobs.length });
    }
  }

  // Every blob failing the HMAC = the passphrase can't be right.
  if (blobs.length > 0 && hmacFailed === blobs.length) {
    throw new Error('Passphrase does not match this vault.');
  }

  // LWW per memory_id on max vector_clock; tie: created_at, then device_id.
  const winner = new Map();
  for (const b of good) {
    const cur = winner.get(b.memory_id);
    if (!cur || b.vector_clock > cur.vector_clock ||
        (b.vector_clock === cur.vector_clock && (b.created_at > cur.created_at ||
         (b.created_at === cur.created_at && b.device_id > cur.device_id)))) {
      winner.set(b.memory_id, b);
    }
  }

  // Decrypt winners; tombstones dropped.
  let tombstones = 0, corrupt = 0;
  const memories = new Map();
  for (const b of winner.values()) {
    if (b.deleted) { tombstones++; continue; }
    try {
      const bytes = b64decode(b.ciphertext);
      const pt = new Uint8Array(await crypto.subtle.decrypt(
        { name: 'AES-GCM', iv: bytes.subarray(0, 12) }, encKey, bytes.subarray(12)));
      const env = JSON.parse(new TextDecoder().decode(pt));
      if (env && env.id && !env.deleted) memories.set(env.id, env);
      else tombstones++;
    } catch {
      corrupt++;  // HMAC passed but decrypted garbage — treat as corrupt, skip
    }
  }

  const meta = {
    vaultId,
    fetched: blobs.length,
    verified,
    hmacFailed,
    corrupt,
    tombstones,
    memoryCount: memories.size,
  };
  state = { vaultId, memories, meta };
  return meta;
}
