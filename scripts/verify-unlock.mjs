#!/usr/bin/env node
// verify-unlock.mjs — crypto parity harness for the browser unlock flow.
//
// Proves the browser-crypto path (vendored hash-wasm Argon2id + WebCrypto
// AES-GCM/HMAC-SHA256) is byte-exact with the Rust daemon's sync encryption
// BEFORE the unlock UI ships. Read-only: pulls ciphertext from the relay and
// verifies/decrypts locally, exactly like the browser will.
//
// Inputs are env-only and NEVER printed:
//   RELAY_URL     relay base URL            (default https://sync.ellmstack.dev)
//   RELAY_API_KEY an API key scoped to the vault (dogfood config.json sync.api_key)
//   VAULT_ID      vault to pull             (default engram-local)
//   PASSPHRASE    vault passphrase          (from /home/e/.engram/env)
//
// Prints counts only; exits 1 if any HMAC fails or memory count < 20.
//
// Usage: source /home/e/.engram/env; RELAY_API_KEY=<from config> node scripts/verify-unlock.mjs

import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
// Vendored hash-wasm ESM entry (self-contained, wasm base64-embedded).
const hashWasmPath = new URL('../ui/engram-vault/js/vendor/hash-wasm/index.esm.js', import.meta.url).href;
const { argon2id } = await import(hashWasmPath);

const RELAY_URL = process.env.RELAY_URL ?? 'https://sync.ellmstack.dev';
const API_KEY = process.env.RELAY_API_KEY ?? '';
const VAULT_ID = process.env.VAULT_ID ?? 'engram-local';
const PASSPHRASE = process.env.PASSPHRASE ?? process.env.ENGRAM_PASSPHRASE ?? '';

const ENC_TAG = 'axiom-sync-enc-v2';
const HMAC_TAG = 'axiom-sync-hmac-v2';
const KDF = { parallelism: 4, iterations: 3, memorySize: 65536, hashLength: 32 }; // Argon2id v1.3 — matches Rust
const MIN_MEMORIES = 20;

if (!API_KEY || !PASSPHRASE) {
  console.error('verify-unlock: RELAY_API_KEY and PASSPHRASE env vars are required');
  process.exit(2);
}

// ── KDF: byte-exact with engramd sync_client.rs derive_sync_key ─────────────
async function deriveKey(tag) {
  const salt = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(tag))).slice(0, 16);
  const key = await argon2id({ password: PASSPHRASE, salt, outputType: 'binary', ...KDF });
  return new Uint8Array(key);
}

// ── base64 (standard, padded) — same as atob in the browser ─────────────────
function b64decode(s) {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// ── HMAC verify: byte-exact with sync_client.rs compute_hmac ─────────────────
// vault_id ‖ memory_id ‖ device_id ‖ vector_clock(LE64) ‖ ciphertext b64 ‖ deleted(u8) ‖ created_at
async function hmacOk(hmacKey, blob) {
  const te = new TextEncoder();
  const parts = [
    te.encode(blob.vault_id),
    te.encode(blob.memory_id),
    te.encode(blob.device_id),
    (() => { const b = new Uint8Array(8); new DataView(b.buffer).setBigUint64(0, BigInt(blob.vector_clock), true); return b; })(),
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

// ── Pull loop: mirror of the browser path (limit 1000, since = last created_at)
async function pullAll() {
  const blobs = new Map();
  let since = null;
  let stall = 0;
  for (;;) {
    const q = new URLSearchParams({ limit: '1000' });
    if (since) q.set('since', since);
    const res = await fetch(`${RELAY_URL}/v1/vaults/${VAULT_ID}/pull?${q}`, {
      headers: { Authorization: `Bearer ${API_KEY}` },
    });
    if (!res.ok) throw new Error(`pull HTTP ${res.status}: ${await safeBody(res)}`);
    const page = await res.json();
    let added = 0;
    for (const b of page.blobs ?? []) {
      const id = `${b.memory_id}|${b.vector_clock}|${b.device_id}|${b.created_at}|${b.deleted}`;
      if (!blobs.has(id)) { blobs.set(id, b); added++; }
    }
    if (added === 0 && stall++ >= 3) throw new Error('pull stall: since not advancing');
    if (added > 0) stall = 0;
    if (page.blobs?.length) since = page.blobs[page.blobs.length - 1].created_at;
    if (!page.has_more) break;
  }
  return [...blobs.values()];
}

async function safeBody(res) {
  try { return (await res.text()).slice(0, 200); } catch { return '(unreadable)'; }
}

// ── main ────────────────────────────────────────────────────────────────────
const encKeyRaw = await deriveKey(ENC_TAG);
const hmacKeyRaw = await deriveKey(HMAC_TAG);
const encKey = await crypto.subtle.importKey('raw', encKeyRaw, { name: 'AES-GCM' }, false, ['decrypt']);
const hmacKey = await crypto.subtle.importKey('raw', hmacKeyRaw, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
encKeyRaw.fill(0); hmacKeyRaw.fill(0);

const blobs = await pullAll();

// HMAC-verify every blob; on mismatch count + skip (never decrypt).
let verified = 0, hmacFailed = 0;
const good = [];
for (const blob of blobs) {
  if (await hmacOk(hmacKey, blob)) { verified++; good.push(blob); }
  else hmacFailed++;
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
let memories = 0, tombstones = 0;
for (const b of winner.values()) {
  if (b.deleted) { tombstones++; continue; }
  const bytes = b64decode(b.ciphertext);
  const nonce = bytes.subarray(0, 12);
  const pt = new Uint8Array(await crypto.subtle.decrypt({ name: 'AES-GCM', iv: nonce }, encKey, bytes.subarray(12)));
  const env = JSON.parse(new TextDecoder().decode(pt));
  if (env.id && !env.deleted) memories++;
}

const result = { fetched: blobs.length, verified, hmacFailed, memories, tombstones };
console.log(JSON.stringify(result));

if (hmacFailed !== 0) { console.error(`verify-unlock: ${hmacFailed} HMAC failures — crypto mismatch`); process.exit(1); }
if (memories < MIN_MEMORIES) { console.error(`verify-unlock: only ${memories} memories (< ${MIN_MEMORIES}) — wrong passphrase or partial sync?`); process.exit(1); }
console.log(`verify-unlock: OK — ${memories} memories, ${verified} blobs verified`);
