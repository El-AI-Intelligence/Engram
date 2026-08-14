# Handoff: Make the Memory Graph Meaningful (Link Inference)

**For:** whoever picks up graph work (handed off 2026-08-13)
**Repo:** github.com/El-AI-Intelligence/engram (main @ `b7b0aa8`)

## Symptom

The Graph view (`#/graph` in the vault UI) loads nodes but is functionally
useless: 21 isolated dots, no edges, and node expansion adds nothing. Explorer
works — the graph renderer itself is fine. The problem is that **the graph has
no edges to draw**, and the expansion endpoint can't invent any.

## Root cause (three layers, verified)

1. **Nothing in the pipeline ever creates links.** Capture stores content,
   FTS tokens, and (since 2026-08-13) an embedding — but no link rows.
   Consolidation doesn't create links either. `engram_links` is always empty
   unless a client calls the link API explicitly.
2. **`search_related` is links-only.** `GET /memories/{id}/related` →
   `store.rs::search_related` (line 1375) reads `get_links(id)` and fetches
   the target engrams. Zero links ⇒ zero related ⇒ graph expansion is a no-op.
   It has **no vector-similarity fallback** even though embeddings exist.
3. **The live box has no embeddings yet.** MiniLM (all-MiniLM-L6-v2, 384-dim,
   ~90 MB) downloads lazily on first embed with a 60s retry backoff; no write
   has happened on the box since embed-at-write shipped, so every memory there
   predates embedding storage. Local vaults (8799/8787) have the model loaded.

## Key code locations

| What | Where |
|---|---|
| Link table schema | `crates/axiom-engram/src/schema.rs:29` — `engram_links(source_id, target_id, weight REAL DEFAULT 0.5, link_type CHECK IN ('associative','causal','analogical','temporal'), PRIMARY KEY(source_id, target_id), ON DELETE CASCADE)`. Directed; one row per direction. |
| Link store API | `crates/axiom-engram/src/store.rs` — `link()` (1253), `get_links()` (1352) |
| Related endpoint | `crates/axiom-engram/src/store.rs::search_related` (1375, links-only) + route `crates/engramd/src/routes/memories.rs::get_related` (659) |
| Vector search (reuse this) | `crates/axiom-engram/src/store.rs::vector_search` (1546) — brute-force cosine over all embeddings in SQLite; `cosine_similarity` helper (1886). Fine at 10³–10⁴ memories; no ANN needed. |
| Embedding write path | `crates/axiom-engram/src/store.rs::write_inner` (~540), embedding INSERT (~643); capture handler embeds before write in `crates/engramd/src/routes/memories.rs` (~322) |
| Embedder | `crates/axiom-engram/src/embed.rs` — `OnnxEmbedder`, `dimensions()` always 384, `embed()` lazy-loads with 60s retry (`last_failure`) |
| Graph UI | `ui/engram-vault/js/main.js` — `route('/graph')` (1108), `loadGraph` (~1186, sends `query:''` + `layer`); expansion: `mg-node-expand` listener → `api.memories.related(id)` then `api.memories.links(id)` per node. Renderer: `ui/engram-vault/js/graph.js`. |
| Config | vault `config.json` — add a `links` section (pattern: existing `qem`, `noise`, `sync` sections) |
| Sync | `crates/axiom-engram/src/sync.rs` — serializes the `Engram` struct (which carries `links`); **verify links round-trip through push/pull as an acceptance criterion below**, don't assume. |

## Work plan (ordered; 1–3 are one commit each, 4 is ops, 5 is optional polish)

### 1. Auto link inference at write

After the embedding is stored in `write_inner` (or immediately after the
embedding INSERT), compute top-k nearest existing embeddings and insert
`associative` links. Recommended shape:

- k = 5, minimum cosine similarity 0.35 (make both config: `links.max_links`,
  `links.min_similarity`), config section default-on (`links.auto_infer: true`)
- Insert **both directions** (rows (new→old) and (old→new), weight = similarity)
  so `related` works from either side. PK dedupes naturally.
- Reuse the `vector_search` brute-force path; skip if embedder failed
  (no embedding ⇒ no links, capture still succeeds — graceful offline behavior)
- **Quarantine rule:** never auto-link a live memory to a quarantined one
  (`imagined && !grounded`). Filter candidate targets through
  `QuarantineFilter::LiveOnly` semantics.
- Bounded cost: one vector scan per write. Acceptable at this scale.

### 2. `search_related` vector fallback

Explicit links first (existing behavior), then fill the remainder of `limit`
with `vector_search` neighbors of the memory's own stored embedding (dedupe,
exclude self, apply the same quarantine rule). Behavior change is purely
additive: vaults with links see no difference.

### 3. Backfill pass for existing memories

One-shot, idempotent (`INSERT OR REPLACE` / PK collision-safe):
iterate memories that have embeddings and create links per §1 rules.
Options: `engramd` CLI subcommand (e.g. `engramd backfill-links`) and/or an
admin POST endpoint. O(n²) pairwise at current scale is fine; chunk by
`created_at` so it can be resumed.

### 4. Warm the box model (ops, one-time)

After this ships, the first embed attempt on engram.ellmstack.dev downloads
MiniLM (~90 MB) from HuggingFace; the 60s retry means a transient failure
self-heals on the next attempt. Trigger it with any semantic capture
(`POST /memories` with `layer: "semantic"`, content > 100 chars), then confirm
`/analytics/stats` → `total_embeddings` increments.

### 5. Graph UX polish (separate, optional)

Edge weight → line thickness; filter by `link_type`; minimum-similarity
slider. Only after edges exist.

## Acceptance criteria

1. **Write-time inference:** on a vault with ≥ 2 similar semantic notes,
   capturing a new similar note makes `GET /memories/{id}/links` non-empty
   (both directions present, weight = similarity).
2. **Related fallback:** `GET /memories/{id}/related` returns similar memories
   on a fresh vault with **zero explicit links**.
3. **Backfill idempotent:** running it twice creates no duplicates.
4. **Quarantine:** related/search results never include quarantined targets;
   links to a memory later quarantined are excluded from UI-facing related.
5. **Sync:** create links on 8799 → push → verify they exist on 8787 after
   pull (and vice versa). If links don't round-trip, fix the sync payload —
   do not ship without this.
6. **Tests:** unit tests for similarity thresholding, bidirectionality,
   quarantine filtering, and backfill idempotency. `cargo test --workspace`
   green before each commit; commit + push to main per repo convention.
7. **Live check:** graph on https://engram.ellmstack.dev/app/#/graph shows
   connected components (not isolated dots); clicking a node expands related
   memories.
8. **Constraints (standing):** inference is local-only (MiniLM on device) —
   no external LLM/embedding API calls, ever; flat pricing (no per-request
   metering); credentials never committed. The vault's E2E-encryption and
   sync encryption are unaffected — links are stored inside the encrypted DB
   and sync as part of the encrypted blob.

## Current data state (for verification baselines)

- Box (8787, `/root/engram/vault`): 21 memories, 0 link rows, 0 embeddings.
- Local 8799 (`/home/e/.engram/vault`): ~7 live + ~100 quarantined, 2
  embeddings stored, 0 links.
- Local 8787 (`/tmp/engram-data`): sync mirror of 8799 via engramd-sync on
  8788 (vault_id `engram-local`).

## Shipped (2026-08-13)

- Write-time semantic linking (`generate_semantic_links`, B6b), bidirectional,
  weight = cosine similarity, `LinkInferenceConfig { max_links: 5,
  min_similarity: 0.35 }`; config `links.auto_infer: false` disables.
- `search_related`: explicit links first, then vector-similarity neighbors of
  the memory's own embedding (min 0.35), seen-set dedupe.
- `engramd backfill-links --vault PATH [--max-links N] [--min-similarity X]`:
  idempotent one-shot linker.
- Sync blobs now carry `links` + `embedding` + `embedding_model`, so the graph
  and vector fallback round-trip across devices (verified 8799 → 8787: links
  and embeddings identical). Pulled links to not-yet-synced targets are
  skipped (EXISTS guard) — the reverse blob carries the same link.
- B6 (temporal/tag-overlap) now snapshots pre-existing pairs and only fills
  gaps; it no longer clobbers explicit or B6b semantic links.
- Quarantined memories (`imagined && !grounded`) never receive auto-links.

**Known gap (pre-existing, out of scope here):** sync push/pull cursors are
`created_at`-based, so EDITS to already-synced memories do not re-push or
re-pull (PATCH does not bump a modified timestamp). New memories sync
correctly, including their links and embeddings. Fixing edit propagation
needs a `modified_at` column plus cursor changes on both client and server —
tracked for the sync layer roadmap.

## Graph view parked (2026-08-14)

The graph UI (canvas force-sim in `ui/engram-vault/js/graph.js`) is shelved as
a **future idea**: the execution (layout stability, readability at scale,
interaction model) needs a rethink before it earns a place in the product.
Entry points are hidden — nav link in `index.html` and the tour CTA in
`js/main.js` (both commented, with re-enable notes). The route (`#/graph`),
renderer, and all backend link machinery stay shipped and tested — so when
the UX is revisited, the data layer is ready. Nothing in the link-inference
backend (write-time links, vector fallback, backfill, sync round-trip) is
affected by the park.
