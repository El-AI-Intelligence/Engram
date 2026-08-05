# Engram Memory Systems — Code Quality Audit

**Date:** 2026-08-05  
**Scope:** 9 files, ~3,900 lines across ELLM kernel + Axiom-OS  
**Auditor:** Automated agent (Explore type, 26 tool uses, full file reads + cross-repo verification)

---

## Critical Findings

### CRITICAL-1: Vault key derivation uses unstable hash → total data loss on toolchain upgrade

**File:** `axiom-os/crates/axiom-engram/src/store.rs:34-61`

The module docs and inline comments claim `Key = hex(SHA-256(machine_id || ":" || APP_SALT))`. **This is false.** The implementation uses `std::collections::hash_map::DefaultHasher` (SipHash-1-3 with fixed keys). The std docs explicitly state DefaultHasher's algorithm is NOT specified and hashes MUST NOT be relied upon across releases.

**Impact:** When the Rust compiler/std changes the SipHash implementation (has happened historically), every previously-encrypted `engrams.db` stops decrypting. All engram history becomes unreadable. The error surfaces as "file is not a database" with no indication it's a key mismatch.

**Fix:** Replace DefaultHasher with `sha2::Sha256` or `blake3`. The comment says these were avoided because "not yet in the engram crate" — add the dependency.

### CRITICAL-2: Encryption threat model is defeated

**File:** `store.rs:34-61, :9, :30-33`

The header says "the vault cannot be trivially read by copying the .db to another machine" and the threat is "filesystem access from another user." But `APP_SALT` is a public constant in the source, and `machine-id` is world-readable (mode 0444) on every Linux box. **Any local user can reconstruct the key in seconds and decrypt the vault.** The only thing this defends is raw disk copying to a different machine.

**Fix:** Either strengthen key derivation (hardware-bound via TPM/secure enclave, or user-provided passphrase) or correct the documented threat model to "defense against offline disk cloning only."

---

## High-Severity Findings

### HIGH-1: `INSERT OR REPLACE` cascades deletion of all links

**File:** `store.rs:177` + `schema.rs:28-29`

SQLite's `INSERT OR REPLACE` on PK conflict does DELETE-then-INSERT, which fires `ON DELETE CASCADE` on `engram_links`. Re-writing an engram with the same id silently wipes all its links.

**Fix:** Use `INSERT ... ON CONFLICT(id) DO UPDATE` instead.

### HIGH-2: Timestamp format mismatch breaks hygiene queries

**File:** `store.rs:653, 660`

RFC3339 timestamps (e.g., `2026-08-04T23:59:59+00:00`) are compared against SQLite `datetime('now','-1 day')` output (`2026-08-04 23:59:59`, space-separated, no offset) **lexicographically**. `'T'` (0x54) > `' '` (0x20), so comparisons invert within the cutoff day — the strengthen query over-matches and the decay query under-matches by up to ~24h.

**Fix:** Store timestamps in SQLite-compatible format (space-separated, no timezone) or use `strftime` consistently.

### HIGH-3: `retrievals` and `last_retrieved` never updated → consolidation is dead

**Verified by cross-repo grep.** Nothing executes `UPDATE engrams SET retrievals = retrievals + 1` or sets `last_retrieved`. Consequences:
- `apply_weekly_consolidation` — `WHERE retrievals >= 5` **can never promote an episodic engram to semantic**. Promotion is dead in production.
- `apply_daily_hygiene` — the strengthen branch (`last_retrieved >= datetime('now', '-1 day')`) matches nothing (all NULL). Decay always falls back to `created_at`.

**The "Ebbinghaus decay + Hebbian strengthening" headline feature is only half implemented.** Memory only decays; it never strengthens.

**Fix:** Add `retrievals += 1` and `last_retrieved = now()` to `get()`, `search_by_content()`, `surface_relevant()`, and `vector_search()`.

### HIGH-4: Link graph is write-only

**File:** `store.rs` — every row mapper at :211, :301, etc.

`Engram.links` is always hardcoded `Vec::new()`. The `engram_links` table is populated by `link()` but links are never returned through the API. **The entire typed-link feature (Associative/Causal/Analogical/Temporal) is write-only.** `search_related()` works internally but the public `Engram` struct never surfaces links.

**Fix:** Populate `links` in row mappers by joining `engram_links`.

### HIGH-5: QEM `lookup_association` has zero callers → O(1) retrieval is dead

**File:** `ellm-kernel/src/qem/mod.rs` — verified by cross-repo grep

The daemon only *stores* QEM associations. `lookup_association`, `lookup_concept`, `store_concept`, `associations_for_subject` have **no callers anywhere**. The "O(1) fact retrieval" headline feature never runs outside unit tests. QEM is a write-only cache.

**Fix:** Wire QEM lookups into the kernel's query path. The QemCache design in UNIFICATION_DESIGN.md addresses this.

### HIGH-6: `privacy_level` column is dead schema

**File:** `schema.rs:13`

The column exists with CHECK + DEFAULT but `EngramStore` never reads or writes it. Every engram is permanently `'cloud_first'` regardless of the user's privacy setting.

**Fix:** Either wire it up or remove it from the schema until the multi-tenancy privacy model is implemented.

---

## Medium-Severity Findings

- `search_by_tags` empty input → invalid SQL (`WHERE ` with no clause)
- `get()` maps all errors to `NotFound` — disk errors indistinguishable from missing entries
- FTS→LIKE fallback swallows real DB errors (`Err(_)` catches everything)
- `vector_search()` holds global mutex for full-table scan — works but doesn't scale
- Blocking rusqlite on tokio workers, single serialized connection, no WAL
- AutobiographicalMemory: full JSONL re-read per retrieve; unbounded narrative duplicates; episode ID regression on out-of-order store
- QEM: 24-bit collision space with silent overwrite; NoveltyFilter `counts` map unbounded growth
- `engram_effect_handler.rs`: colon in captured content → silent truncation
- Dead API surface: `update_coherence`, `store_embedding`, `detect_temporal_patterns`, `Goal`, `ConsolidationRun`, and several AutobiographicalMemory methods
- No integration tests for `axiom-engram` or `axiom-memory` crates
- Zero tests for: vault key derivation, `search_by_tags`, hygiene/consolidation math, EXEC parser, `ConversationSummarizer`

---

## What's Clean

- **Zero `unsafe` blocks** in all 9 files
- **No SQL injection.** All user input is parameterized. `search_by_tags` builds query structure with `format!` but binds values. `PRAGMA key` is hex-only by construction.
- **No deadlocks.** `EngramStore` uses a single `tokio::Mutex<Connection>` never held across `.await`. `AutobiographicalMemory` uses `std::sync::RwLock` with no async under lock.
- **Proper error types.** `EngramError` and `MemoryError` are well-structured thiserror enums.
- **`effect_block_on` is safe and well-tested.** Dedicated effect runtime with mpsc channel, never trips tokio's block-in-async guard.
- **Good test coverage in `axiom-memory`** (9 tests covering budget/priority/compaction)
- **Good test coverage in `ellm-kernel/src/memory/`** (8 tests covering success/failure, quality scoring, index retrieval, context frames, tier2 records)

---

## Summary: What must be fixed before product launch

1. **Replace DefaultHasher with SHA-256/BLAKE3** for vault key derivation (CRITICAL — data loss)
2. **Update retrievals/last_retrieved** on every read path (HIGH — core feature dead)
3. **Fix INSERT OR REPLACE → ON CONFLICT DO UPDATE** (HIGH — data loss on re-write)
4. **Fix timestamp format** for hygiene queries (HIGH — decay math broken)
5. **Populate links in Engram struct** (HIGH — typed links are write-only)
6. **Wire QEM lookups into query path** (HIGH — O(1) retrieval dead)
7. **Fix or document encryption threat model** (CRITICAL — security claim overstated)
8. **Fix colon-truncation in EXEC parser** (MEDIUM — silent content loss)
9. **Add tests for the above fixes** — especially vault key derivation, hygiene math, and EXEC parsing
