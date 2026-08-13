# Kimi K3 — Engram Memory Vault UI Prompt

Build a standalone web application (desktop-first, responsive, dark mode default) for the **Engram Memory Vault** — a local-first, encrypted memory layer for AI agents. It's a developer tool: the user browses what their AI remembers, configures how memories are assembled into context windows, and observes the memory lifecycle (decay, consolidation, patterns).

**API:** REST + SSE at `localhost:8787` (see API contract below). Build against the live API — all data comes from the server.

---

## Visual Vocabulary

### Colors
- **Episodic** (what happened): amber/warm gold `#F59E0B`
- **Semantic** (what was learned): blue/teal `#3B82F6`
- **Imagined** (what was dreamed): violet/purple `#8B5CF6`
- **Valence:** Joyful `#10B981` / Positive `#14B8A6` / Neutral `#64748B` / Challenging `#F59E0B`
- **Grounded:** green `#10B981` / **Quarantined:** violet `#8B5CF6` / **Decaying:** slate `#94A3B8`

### Icons
- Episodic = solid circle ● | Semantic = diamond ◆ | Imagined = sparkles ✦
- Sources: Interaction 💬 | Window 🖥️ | Agent 🤖 | System ⚙️ | Chat 💭 | Consolidation 🌙 | Imagined ✦

### Link types (graph edges)
- Associative = dotted line | Causal = solid arrow | Analogical = dashed line | Temporal = dotted arrow

### Typography
Monospace for IDs, timestamps, code. Sans-serif for content and UI chrome.

---

## Screens (7 routes)

### 1. Vault Dashboard (`/`)
Stats cards (total memories, QEM hit rate, decayed last night, new today), strength distribution bar chart, layer breakdown (Epi/Semantic/Imagined counts + %), recent captures feed (last 5 memories with layer icons, source, content preview, relative time), vault health indicators (encrypted, size, QEM warm, last backup).

### 2. Engram Explorer (`/memories`)
Search bar (debounced) + filter pills: layer (All/Episodic/Semantic/Imagined), scope (All/Moment/Episode/Narrative/Rule), source dropdown, tag input, sort dropdown (Relevance/Strength/Recency/Valence), min-strength slider. Results as scrollable cards showing: layer icon (colored), scope badge, content preview (2-3 lines), strength bar (mini horizontal, green→amber→red), valence indicator (colored dot), tags (clickable, filter on click), link count, quarantine warning, relative timestamp. Click navigates to detail. Keyboard nav: j/k/Enter.

### 3. Engram Detail (`/memories/:id`)
Full content, strength bar, valence label (Joyful/Positive/Neutral/Challenging), all metadata (created, occurred, last retrieved, retrieval count, source, project, ID). Links panel: outgoing links with type + target preview, incoming links. Evidence panel. Context JSON (collapsible). Actions: edit tags, update valence, ground memory (imagined only), delete (with confirmation).

### 4. Memory Graph (`/graph`)
Force-directed graph visualization (use React Flow or d3-force). Nodes = memories colored by layer, sized by strength, border-glow by valence. Edges = links styled by type (dotted/solid/dashed/arrow). Filters: layer, link type, min strength. Focus search to center on a node. Click node → detail panel; double-click → navigate to detail. Zoom/pan/fit. Legend.

### 5. Context Assembly Config (`/context`)
Token budget slider (1024–32768), high-priority reserve %, priority table (each source row with dropdown: Required/High/Normal/Low), retrieval config (max engrams, max recent turns, include world/narrative/imagined toggles), summarization mode (extractive/abstractive). **Live preview panel:** test query input + "Assemble" button → shows resulting OpenAI-format messages array with token count and metadata (engrams retrieved, took_ms). This is the killer demo.

### 6. Consolidation History (`/consolidation`)
Timeline of decay/consolidation runs, decay curve visualization for a sample engram, promotion activity chart, detected temporal patterns ("you tend to do X on Thursdays, usually in the evening"). Buttons: Run decay now, Run consolidation.

### 7. Settings (`/settings`)
Vault info (name, path, encryption status, size), export/import (JSONL, file picker), embedding model config, schedule (decay/consolidation timings), API key management (if remote access enabled).

---

## API Contract

Base URL: `http://localhost:8787`

### Core endpoints
- `GET /health` → `{ status, version, vault, uptime_secs, memories_total, qem_hit_rate, db_size_bytes }`
- `GET /analytics/stats` → `{ total_memories, by_layer, by_source, by_scope, total_links, avg_strength, avg_valence, qem_hit_rate, total_embeddings, vault_size_bytes, last_consolidation, last_decay }`
- `GET /config` / `PATCH /config` → `{ vault_path, encryption, decay_schedule, consolidation_schedule, qem: { warm_limit, ... }, context: { default_budget, ... } }`

### Memories
- `POST /memories` → capture. Body: `{ content, layer?, source?, tags?, valence?, project?, context?, links_to? }`. Returns `{ id, qem_code, strength, created_at }`
- `GET /memories/:id` → full MemoryEntry
- `POST /memories/search` → Body: `{ query, layer?, tags?, min_strength?, sort_by?, limit, offset }`. Returns `{ results: [MemoryEntry], total, search_type, took_ms }`
- `PATCH /memories/:id` → update tags/valence/project
- `DELETE /memories/:id` → 204
- `POST /memories/link` → `{ source_id, target_id, link_type, weight }`
- `GET /memories/:id/links` → `{ outgoing, incoming }`
- `GET /memories/:id/related?limit=10` → related memories by link traversal
- `POST /memories/:id/ground` → ground an imagined memory

### Context
- `POST /context/assemble` → Body: `{ query, system_prompt?, token_budget?, config?: { max_engrams, max_recent_turns, ... }, session_id?, recent_history?: [{role, content}], world_context?: {app, title, time} }`. Returns `{ messages: [{role, content}], metadata: { total_tokens, budget, utilization, engrams_retrieved, retrieval_took_ms, assembly_took_ms } }`
- `GET /context/stream?session_id=...&token_budget=...` → SSE: `snapshot` event then `delta` events

### Consolidation
- `POST /consolidate/decay` → `{ strengthened, decayed, pruned, took_ms }`
- `POST /consolidate/weekly` → `{ promoted_to_semantic, pruned_imagined, narratives_updated }`
- `GET /consolidate/history` → `{ runs: [{ id, type, run_at, episodes_processed, semantics_created, engrams_decayed }] }`
- `POST /analytics/patterns` → Body: `{ query, min_engrams }`. Returns `{ pattern: { query, sample_size, peak_day, day_strength, peak_period, period_strength, description } }`

### Import/Export
- `POST /export` → Body: `{ format: "jsonl", layer?, project?, tags?, min_strength? }`. Returns ndjson
- `POST /import` → ndjson body. Returns `{ imported, skipped, errors }`

### MemoryEntry type
```typescript
{
  id: string;           // "eng_a1b2c3..."
  layer: string;        // "episodic" | "semantic" | "imagined"
  source: string;       // "interaction" | "chat" | "window" | "agent" | ...
  scope: string;        // "moment" | "episode" | "narrative" | "rule"
  content: string;
  strength: number;     // 0.0–2.0
  valence: number;      // -1.0–1.0
  imagined: boolean;
  grounded: boolean;
  retrieval_count: number;
  tags: string[];
  project?: string;
  links_out: Array<{ target_id: string, weight: number, link_type: string }>;
  created_at: string;   // ISO 8601
  last_retrieved?: string;
  occurred_at?: string;
  context: object;      // arbitrary JSON
}
```

---

## Design Priorities

1. **Memory card density** — Explorer is the most-used screen. Cards must be scannable.
2. **Graph is the wow factor** — typed edges by link type, colored nodes by layer. No other memory product shows associative structure.
3. **Context preview is the sales tool** — the live "Assemble" pane shows exactly what the LLM receives. A developer who sees their own memories assembled into a coherent context window understands the product instantly.
4. **Dark mode default** — this is a developer tool.
5. **Responsive but desktop-first** — mobile is read-only (browse, check stats).
6. **Fast** — local vault. Optimistic UI for mutations. No spinners for local reads.

---

## Navigation Shell
```
◆ ELLM Engram Vault                    [vault selector ▾]  ⚙️
Dashboard | Explorer | Graph | Context | Consolidation
──────────────────────────────────────────────────────────
[page content]
──────────────────────────────────────────────────────────
QEM: 85% hit rate | 1,423 memories | 2.3 MB | Encrypted ✓
```
