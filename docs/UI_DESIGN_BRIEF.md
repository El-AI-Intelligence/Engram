# Engram Memory Vault — UI Design Brief for Kimi K3

**Date:** 2026-08-05  
**Target:** Standalone web application (desktop-first, responsive)
**API:** `engramd` REST + SSE (see API_SURFACE.md)
**Status:** Ready for design

---

## 0. Product Context

The Engram Memory Vault is a local-first, encrypted memory layer for AI agents. It's not a chat app or a companion — it's a **memory management console**. The user is a developer or power user managing what their AI remembers.

Three core jobs:
1. **Browse and search** what the AI remembers
2. **Configure** how memories are assembled into context windows
3. **Observe** the memory lifecycle (decay, consolidation, patterns)

---

## 1. Visual Vocabulary (established from existing iOS/desktop code)

### 1.1 Colors

```
Layer colors:
  Episodic  — Amber/Warm gold    #F59E0B  (what happened)
  Semantic  — Blue/Teal          #3B82F6  (what was learned)
  Imagined  — Violet/Purple      #8B5CF6  (what was dreamed)

Valence colors:
  Joyful    — Green              #10B981  (valence ≥ 0.5)
  Positive  — Teal               #14B8A6  (0.1 ≤ valence < 0.5)
  Neutral   — Slate              #64748B  (-0.3 ≤ valence < 0.1)
  Challenging — Amber            #F59E0B  (valence < -0.3)

Link type stroke styles:
  Associative — dotted
  Causal      — solid with arrow
  Analogical  — dashed
  Temporal    — dotted with arrow

Status colors:
  Grounded   — Green             #10B981
  Quarantined — Violet           #8B5CF6  (imagined, not yet grounded)
  Decaying   — Slate             #94A3B8  (strength < 0.2)
```

### 1.2 Icons

```
Layer (from iOS Engram.swift SF Symbols):
  Episodic  — ● circle.fill       (solid dot — concrete, happened)
  Semantic  — ◆ diamond.fill      (faceted — distilled, structured)
  Imagined  — ✦ sparkles          (ethereal — generated, uncertain)

Source:
  Interaction — 💬 message.circle
  Window      — 🖥️ rectangle.inset.filled
  Agent       — 🤖 bolt.circle
  System      — ⚙️ gear
  Sensor      — 📡 antenna.radiowaves.left.and.right
  Chat        — 💭 bubble.left
  Mic         — 🎤 mic.circle
  Research    — 🔬 magnifyingglass.circle
  Consolidation — 🌙 moon.zzz
  Imagined    — ✦ sparkles (same as layer, but as source)

Scope:
  Moment     — ● small dot
  Episode    — ⊟ stack of lines
  Narrative  — 📖 book
  Rule       — ◈ diamond with dot
```

### 1.3 Typography

Monospace for memory IDs, timestamps, code. Sans-serif for content, labels, UI chrome.

---

## 2. Screen Map

```
/                          → Vault Dashboard (overview)
/memories                  → Engram Explorer (browse, search, filter)
/memories/:id              → Engram Detail (full entry, links, evidence)
/graph                     → Memory Graph (force-directed link visualization)
/context                   → Context Assembly Config (tuning panel)
/consolidation             → Consolidation History (hygiene runs, patterns)
/settings                  → Vault Settings (config, import/export, auth)
```

---

## 3. Screen: Vault Dashboard (`/`)

### Purpose
Landing page. At-a-glance overview of vault health and recent activity.

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│  [nav: Dashboard | Explorer | Graph | Context | Consolidation │
│                                        [Vault: default ▼]   │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌────────┐ │
│  │ 1,423       │ │ 85%         │ │ 342         │ │ 12     │ │
│  │ Memories    │ │ Cache hit   │ │ Decayed     │ │ New     │ │
│  │             │ │ rate (QEM)  │ │ last night  │ │ today   │ │
│  └─────────────┘ └─────────────┘ └─────────────┘ └────────┘ │
│                                                               │
│  ┌──────────────────────────────┐ ┌──────────────────────────┐│
│  │ STRENGTH DISTRIBUTION        │ │ LAYER BREAKDOWN           ││
│  │ ████████████░░░░ 0.8-2.0 45% │ │ ● Episodic   1,200  84%  ││
│  │ ██████░░░░░░░░░░ 0.4-0.8 30% │ │ ◆ Semantic     180  13%  ││
│  │ ████░░░░░░░░░░░░ 0.1-0.4 20% │ │ ✦ Imagined      43   3%  ││
│  │ ██░░░░░░░░░░░░░░ <0.1    5%  │ │                          ││
│  └──────────────────────────────┘ └──────────────────────────┘│
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ RECENT CAPTURES                                  [View all]│ │
│  │                                                           │ │
│  │ ● just now  [chat]     User asked about Rust async trait… │ │
│  │ ◆ 2h ago   [agent]    Consolidated 3 episodes about ELLM… │ │
│  │ ● 3h ago   [window]   code — smc_kernel/mod.rs            │ │
│  │ ✦ 5h ago   [imagined] Scenario: refactoring QEM into…     │ │
│  │ ● 8h ago   [interact] Debug session — fixed cache key…    │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────┐ ┌──────────────────────────┐│
│  │ VAULT HEALTH                  │ │ UPCOMING                  ││
│  │ ● Vault encrypted ✓          │ │ Next decay: 03:00 AM      ││
│  │ ● 2.3 MB / no limit          │ │ Next consolidation: Sun   ││
│  │ ● QEM cache warm ✓ (1000)    │ │ Pending narratives: 3     ││
│  │ ● Last backup: never         │ │ Quarantined memories: 5   ││
│  └──────────────────────────────┘ └──────────────────────────┘│
└──────────────────────────────────────────────────────────────┘
```

### Data sources
- `GET /analytics/stats` — all KPIs
- `GET /memories?sort_by=recency&limit=5` — recent captures feed
- `GET /config` — vault health
- `GET /analytics/patterns` — optional pattern callout

---

## 4. Screen: Engram Explorer (`/memories`)

### Purpose
Browse, search, and filter all memories. The main working screen.

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│  [nav]                                                        │
├──────────────────────────────────────────────────────────────┤
│  🔍 [________________________________________] [Search]      │
│                                                               │
│  Filters:                                                     │
│  Layer: [All ▾] [Episodic] [Semantic] [Imagined]             │
│  Scope: [All ▾] [Moment] [Episode] [Narrative] [Rule]        │
│  Source:[All ▾] Tags: [rust ×] [async ×] [+ add]             │
│  Sort:  [Relevance ▾]  Min strength: [0.1 ===○====] 1.0     │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ Results: 142 memories (FTS5 search, 2.3ms)               │ │
│  │                                                           │ │
│  │ ┌──────────────────────────────────────────────────────┐  │ │
│  │ │ ● episodic  [interaction]  strength 0.87  valence +0.7│  │ │
│  │ │ User asked about Rust async trait bounds. Explained   │  │ │
│  │ │ Send + Sync requirements for Arc<Mutex<T>> and the    │  │ │
│  │ │ smc_kernel refactor approach…                         │  │ │
│  │ │ [rust] [async] [traits] [send-sync]   2 hours ago     │  │ │
│  │ │ → links to: 3 memories                                │  │ │
│  │ └──────────────────────────────────────────────────────┘  │ │
│  │                                                           │ │
│  │ ┌──────────────────────────────────────────────────────┐  │ │
│  │ │ ◆ semantic  [consolidation]  strength 1.2  valence 0  │  │ │
│  │ │ Rust async trait patterns: Send bounds propagate      │  │ │
│  │ │ through Arc, Sync through Mutex. Common pitfall: …    │  │ │
│  │ │ [rust] [async] [patterns]             3 days ago      │  │ │
│  │ │ → 5 supporting episodes                               │  │ │
│  │ └──────────────────────────────────────────────────────┘  │ │
│  │                                                           │ │
│  │ ┌──────────────────────────────────────────────────────┐  │ │
│  │ │ ✦ imagined  [imagined]  strength 0.35  ⚠ quarantined │  │ │
│  │ │ Scenario: moving QEM from HashMap to disk-backed      │  │ │
│  │ │ store with write-through cache and LRU eviction…      │  │ │
│  │ │ [qem] [refactor] [architecture]     5 hours ago      │  │ │
│  │ │ ⚠ Not grounded — may be speculative                  │  │ │
│  │ └──────────────────────────────────────────────────────┘  │ │
│  │                                                           │ │
│  │                    [Load more]                             │ │
│  └──────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

### Memory card component

Each card shows:
- **Layer icon** (colored by layer)
- **Scope badge** (subtle, right-aligned)
- **Content preview** (first 2-3 lines, expandable)
- **Strength bar** (miniature horizontal bar, colored green→amber→red)
- **Valence indicator** (colored dot or emoji)
- **Tags** (clickable, filter on click)
- **Link count** ("→ links to: N memories")
- **Quarantine warning** (for imagined + ungrounded only)
- **Timestamp** (relative: "2 hours ago")
- Click → navigates to `/memories/:id`

### Interactions
- **Search:** Debounced, hits `POST /memories/search`. Shows search type badge (FTS5/vector/hybrid/LIKE).
- **Filters:** Each filter updates the query. URL reflects filter state (bookmarkable).
- **Sort:** Toggle between Relevance, Strength, Recency, Valence.
- **Tag click:** Adds tag to filter.
- **Keyboard nav:** j/k for up/down, Enter to open detail, / to focus search.

---

## 5. Screen: Engram Detail (`/memories/:id`)

### Purpose
Full view of a single memory: content, metadata, links, evidence, timeline.

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│  ← Back to Explorer                                          │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ ● EPISODIC MEMORY                    strength 0.87 ━━━━  │ │
│  │                                                           │ │
│  │ User asked about Rust async trait bounds. Explained       │ │
│  │ Send + Sync requirements for Arc<Mutex<T>> and the        │ │
│  │ smc_kernel refactor approach. Discussed how trait         │ │
│  │ bounds propagate through nested generic types and the     │ │
│  │ compiler error messages that result from missing bounds.  │ │
│  │                                                           │ │
│  │ Valence: +0.7 😊 Positive                                 │ │
│  │ Created: 2026-08-05 14:30:01  ·  2 hours ago             │ │
│  │ Occurred: 2026-08-05 14:30:00                             │ │
│  │ Last retrieved: 2026-08-05 16:00:00                       │ │
│  │ Retrievals: 4                                             │ │
│  │                                                           │ │
│  │ Tags: [rust] [async] [traits] [send-sync]                 │ │
│  │ Project: axiom-os                                         │ │
│  │ Source: interaction                                       │ │
│  │ ID: eng_a1b2c3d4e5f6g7h8                                  │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────┐ ┌──────────────────────────────────┐│
│  │ LINKS (outgoing: 3)  │ │ EVIDENCE                         ││
│  │                      │ │                                  ││
│  │ ▶ eng_xyz  causal    │ │ ← supported by:                  ││
│  │   "Debug session…"   │ │   eng_prev — prior discussion    ││
│  │   weight: 0.8        │ │                                  ││
│  │                      │ │ → supports:                      ││
│  │ ·· eng_abc associative│ │   eng_next — follow-up question ││
│  │   "Async Rust…"      │ │                                  ││
│  │   weight: 0.5        │ │                                  ││
│  │                      │ │                                  ││
│  │ -- eng_def temporal  │ │                                  ││
│  │   "Earlier today…"   │ │                                  ││
│  │   weight: 0.6        │ │                                  ││
│  └──────────────────────┘ └──────────────────────────────────┘│
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ CONTEXT DATA (JSON)                          [Expand/Collapse]│
│  │ {                                                         │ │
│  │   "session_id": "sess_abc",                               │ │
│  │   "turn": 3,                                              │ │
│  │   "topic": "rust_async_traits"                            │ │
│  │ }                                                         │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  [Edit tags]  [Update valence]  [Ground memory]  [Delete]    │
└──────────────────────────────────────────────────────────────┘
```

### Interactions
- **Links:** Click navigates to linked memory detail. Link type shown as stroke style.
- **Ground memory:** Visible only for imagined memories. Calls `POST /memories/:id/ground`.
- **Edit tags:** Inline tag editor with autocomplete from existing tags.
- **Valence:** Click to cycle through or slider to set.
- **Delete:** Confirmation dialog. "This memory will be permanently removed from the vault."

---

## 6. Screen: Memory Graph (`/graph`)

### Purpose
Force-directed graph visualization of memory links. Explore the associative structure.

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│  [nav]                                                        │
├──────────────────────────────────────────────────────────────┤
│  Filters: Layer [All ▾]  Link type [All ▾]  Min strength [0.1│
│  Focus: [Search a memory…]          [Fit] [+] [-] [Reset]    │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│                        ✦                                     │
│                       / ╲ (dotted — associative)             │
│                      /   ╲                                   │
│                     ●─────●                                  │
│                    / ╲   ╱ ╲                                 │
│                   /   ╲ ╱   ╲                                │
│               ●──●     ◆     ●────●                          │
│               │            (solid arrow — causal)             │
│               ●                                              │
│              / ╲ (dashed — analogical)                       │
│             /   ╲                                             │
│            ✦     ●                                           │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ Legend:                                                    │ │
│  │ ● Episodic  ◆ Semantic  ✦ Imagined                        │ │
│  │ ·· Associative  → Causal  -- Analogical  ··> Temporal    │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ Selected: ● "Rust async trait bounds"                     │ │
│  │   → 3 outgoing links, 2 incoming                          │ │
│  │   [View detail] [Expand: 1 hop] [Center]                  │ │
│  └──────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

### Data model for graph

```typescript
interface GraphNode {
  id: string;
  label: string;           // truncated content (first 60 chars)
  layer: "episodic" | "semantic" | "imagined";
  strength: number;        // 0-2, determines node size
  valence: number;         // -1 to 1, determines border glow
  grounded: boolean;
  retrievalCount: number;
}

interface GraphEdge {
  source: string;
  target: string;
  linkType: "associative" | "causal" | "analogical" | "temporal";
  weight: number;          // determines edge thickness
}
```

### API contract

```
GET /memories?limit=100&min_strength=0.2  → nodes
GET /memories/:id/links for each node     → edges (can batch)
```

For large vaults, add a dedicated endpoint:

```
POST /graph/neighborhood
{ "center_id": "eng_xyz", "hops": 2, "min_strength": 0.2 }
→ { nodes: [...], edges: [...] }
```

### Interactions
- **Drag nodes** to reposition
- **Click node** → select, show detail panel (bottom bar)
- **Double-click node** → navigate to `/memories/:id`
- **Click edge** → show link details (type, weight)
- **Hover node** → tooltip with content preview
- **Search** → highlight and center matching node
- **Pinch/scroll** → zoom
- **Fit button** → fit all nodes in viewport
- **Reset** → re-run force simulation
- **Focus search** → type memory ID or content, centers on match

### Force simulation parameters
- **Repulsion:** All nodes repel each other (Coulomb's law)
- **Attraction:** Linked nodes attract (Hooke's law, spring constant proportional to `weight`)
- **Gravity:** Slight pull toward center to prevent drift
- **Layer clustering:** Nodes of same layer have slight additional attraction (optional, toggleable)
- **Damping:** Simulation cools over time

---

## 7. Screen: Context Assembly Config (`/context`)

### Purpose
Tune how memories are assembled into context windows. This is the "engine room."

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│  [nav]                                                        │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ TOKEN BUDGET                                              │ │
│  │                                                           │ │
│  │ Default: [8192 =========○========] 32768                  │ │
│  │                                                           │ │
│  │ High priority reserve: [60% ===○================] 100%    │ │
│  │   (Reserved for Required + High priority slots)           │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ PRIORITY WEIGHTS                                          │ │
│  │                                                           │ │
│  │ Source              Priority     Budget share    Adjust   │ │
│  │ ─────────────────   ──────────   ─────────────   ──────   │ │
│  │ SystemPrompt        Required     100% (fixed)    ———      │ │
│  │ CharacterBias       Required     100% (fixed)    ———      │ │
│  │ CurrentTurn         Required     100% (fixed)    ———      │ │
│  │ CouncilInstruction  Required     100% (fixed)    ———      │ │
│  │ RedLineWarning      Required     100% (fixed)    ———      │ │
│  │                                                           │ │
│  │ EngramRetrieval     High         ────○────       [▼ High] │ │
│  │ WorldContext        High         ────○────       [▼ High] │ │
│  │ PurposeVector       High         ────○────       [▼ High] │ │
│  │                                                           │ │
│  │ RecentHistory       Normal       ──○──────       [▼ Norm] │ │
│  │                                                           │ │
│  │ CompactedHistory    Low          ─○────────      [▼ Low]  │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ RETRIEVAL CONFIG                                          │ │
│  │                                                           │ │
│  │ Max engrams per assembly: [10 =====○=============] 50    │ │
│  │ Max recent turns:         [12 =====○=============] 50    │ │
│  │ Include world context:    [✓]                             │ │
│  │ Include narratives:       [✓]                             │ │
│  │ Include imagined:         [ ]  (quarantined by default)   │ │
│  │                                                           │ │
│  │ Summarization mode:                                       │ │
│  │   (●) Extractive (fast, zero-dependency)                  │ │
│  │   ( ) Abstractive (LLM-based, higher quality)             │ │
│  │         Model: [claude-haiku-4-5-20251001 ▾]              │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ LIVE PREVIEW                                              │ │
│  │                                                           │ │
│  │ Test query: [What should I work on next?    ] [Assemble]  │ │
│  │                                                           │ │
│  │ Result: 1,247 / 8,192 tokens (15%)  2 engrams retrieved   │ │
│  │                                                           │ │
│  │ ┌──────────────────────────────────────────────────────┐  │ │
│  │ │ [system] You are a helpful AI assistant with memory. │  │ │
│  │ │ [system] [MEMORY from last week]: You discussed…     │  │ │
│  │ │ [system] [MEMORY from 3 days ago]: Debug session…    │  │ │
│  │ │ [system] [SUMMARY of 8 turns]: decided to refactor…  │  │ │
│  │ │ [user] I'm working on the ELLM kernel refactor.      │  │ │
│  │ │ [assistant] Got it — focusing on smc_kernel.         │  │ │
│  │ │ [user] What should I work on next?                   │  │ │
│  │ └──────────────────────────────────────────────────────┘  │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  [Save config]  [Reset to defaults]                           │
└──────────────────────────────────────────────────────────────┘
```

### API contract

```typescript
// Load current config
GET /config → Config

// Update config
PATCH /config → Config

// Test assembly
POST /context/assemble → AssembledContext

// Types
interface Config {
  context: {
    default_budget: number;
    high_priority_reserve: number;
    max_recent_turns: number;
    max_engrams: number;
    include_world_context: boolean;
    include_narratives: boolean;
    include_imagined: boolean;
    summarization_mode: "extractive" | "abstractive";
    llm_model?: string;
  };
  qem: { ... };
  decay_schedule: string;
  consolidation_schedule: string;
}
```

### Interactions
- **Sliders:** All numeric values use range sliders with immediate preview update
- **Priority dropdowns:** Change a source's priority tier, preview shows effect
- **Live preview:** "Assemble" button calls the API with a test query and shows the result. The user sees exactly what would be sent to the LLM.
- **Save:** `PATCH /config`

---

## 8. Screen: Consolidation History (`/consolidation`)

### Purpose
Observe the memory lifecycle — decay, consolidation, patterns.

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│  [nav]                                                        │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ CONSOLIDATION TIMELINE                                    │ │
│  │                                                           │ │
│  │  Aug 5 03:00  ████████  Weekly consolidation              │ │
│  │               ↑ 3 promoted, 7 pruned, 342 decayed        │ │
│  │  Aug 4 03:00  ████████  Daily hygiene                     │ │
│  │               ↑ 12 strengthened, 245 decayed              │ │
│  │  Aug 3 03:00  ████████  Weekly consolidation              │ │
│  │               ↑ 5 promoted, 3 pruned, 410 decayed         │ │
│  │  Aug 2 03:00  ████████  Daily hygiene                     │ │
│  │               ↑ 8 strengthened, 198 decayed               │ │
│  │  Aug 1 03:00  ████████  Daily hygiene                     │ │
│  │               ↑ 15 strengthened, 220 decayed              │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────┐ ┌──────────────────────────┐│
│  │ DECAY CURVE (sample engram)  │ │ PROMOTION ACTIVITY        ││
│  │                              │ │                          ││
│  │ strength                     │ │ Episodic → Semantic       ││
│  │ 1.0 ┤╲                      │ │                          ││
│  │ 0.8 ┤ ╲╲                    │ │ Aug 5: ███ (3)           ││
│  │ 0.6 ┤   ╲╲__                │ │ Aug 3: █████ (5)         ││
│  │ 0.4 ┤      ╲╲╲___          │ │ Jul 27: ████████ (8)     ││
│  │ 0.2 ┤          ╲╲╲___      │ │                          ││
│  │ 0.0 ┼──────────────────    │ │ Total promoted: 128       ││
│  │      Jul29  Aug1  Aug3     │ │                          ││
│  └──────────────────────────────┘ └──────────────────────────┘│
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ DETECTED PATTERNS                                         │ │
│  │                                                           │ │
│  │ 🕐 "debugging" — you tend to do this on Thursdays (35%    │ │
│  │    of the time), usually in the evening                   │ │
│  │                                                           │ │
│  │ 🕐 "async traits" — peak activity on Mondays (40%),       │ │
│  │    usually in the afternoon                               │ │
│  │                                                           │ │
│  │ 🕐 "code review" — evenly distributed, no clear pattern   │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  [Run decay now]  [Run consolidation now]  [Detect patterns]  │
└──────────────────────────────────────────────────────────────┘
```

### API contract
- `GET /consolidate/history` → timeline
- `GET /analytics/stats` → promotion counts
- `POST /analytics/patterns` → temporal patterns
- `POST /consolidate/decay` → trigger decay
- `POST /consolidate/weekly` → trigger consolidation

---

## 9. Screen: Settings (`/settings`)

### Purpose
Vault management, import/export, auth configuration.

### Layout

```
┌──────────────────────────────────────────────────────────────┐
│  [nav]                                                        │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ VAULT                                                     │ │
│  │                                                           │ │
│  │ Vault name:  [default                ]                    │ │
│  │ Vault path:  ~/.engram/vaults/default                     │ │
│  │ Encryption:  SQLCipher (machine-ID bound)                 │ │
│  │ Vault size:  2.3 MB                                       │ │
│  │                                                           │ │
│  │ [Export vault]  [Import memories]  [Rekey vault]          │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ EMBEDDING MODEL                                           │ │
│  │                                                           │ │
│  │ (●) Local: text-embedding-3-small (1536d)                 │ │
│  │ ( ) API: OpenAI text-embedding-3-large (3072d)            │ │
│  │ ( ) None (disable vector search)                          │ │
│  │                                                           │ │
│  │ Auto-embed on capture: [✓]                                │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ SCHEDULE                                                  │ │
│  │                                                           │ │
│  │ Decay:           [Daily at 03:00  ▾]                      │ │
│  │ Consolidation:   [Weekly on Sun  ▾]                       │ │
│  │ Pattern detect:  [Weekly on Sun  ▾]                       │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ AUTH (for remote access)                                   │ │
│  │                                                           │ │
│  │ Mode: (●) Local-only (127.0.0.1, no auth)                 │ │
│  │       ( ) API key                                         │ │
│  │                                                           │ │
│  │ API Keys:                                                 │ │
│  │   engram_key_abc123  [admin]  created Aug 1  [revoke]    │ │
│  │   engram_key_def456  [read]   created Aug 3  [revoke]    │ │
│  │                                                           │ │
│  │ [Generate new key]                                        │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │ IMPORT / EXPORT                                            │ │
│  │                                                           │ │
│  │ Export format: [JSONL ▾]                                  │ │
│  │ Filter by project: [All ▾]                                │ │
│  │ [Export to file]                                          │ │
│  │                                                           │ │
│  │ Import from: [Choose file] or drag-and-drop               │ │
│  │ Format: [Auto-detect ▾] [JSONL] [Chat log (OpenAI)]       │ │
│  │ [Import]                                                  │ │
│  └──────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

---

## 10. Navigation & Shell

### Global navigation

```
┌──────────────────────────────────────────────────────────────┐
│  ◆ ELLM Engram Vault                    [default ▾]  ⚙️ 🔔  │
│                                                               │
│  Dashboard  Explorer  Graph  Context  Consolidation          │
├──────────────────────────────────────────────────────────────┤
```

### Vault selector
Dropdown in top bar. Lists all available vaults. "Manage vaults →" goes to admin.

### Notification bell
Shows:
- "Consolidation completed: 3 promoted, 7 pruned"
- "Decay cycle finished: 342 engrams decayed"
- "Temporal pattern detected: debugging peaks on Thursdays"

### Status bar (bottom, subtle)
```
QEM: 85% hit rate  |  1,423 memories  |  2.3 MB  |  Encrypted ✓  |  Next decay: 03:00
```

---

## 11. Data Flow Summary

```
┌─────────────────────────────────────────────────────────────┐
│  UI (Kimi K3)                                                │
│                                                               │
│  Dashboard ──→ GET /analytics/stats, GET /memories           │
│  Explorer  ──→ POST /memories/search, GET /memories/:id      │
│  Detail    ──→ GET /memories/:id, GET /memories/:id/links    │
│  Graph     ──→ POST /memories/search → nodes + links         │
│  Context   ──→ GET /config, PATCH /config, POST /context/assemble│
│  Consol.   ──→ GET /consolidate/history, POST /consolidate/* │
│  Settings  ──→ GET /config, PATCH /config, POST /export      │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │    engramd      │
                    │  (REST + SSE)   │
                    │  port 8787      │
                    └─────────────────┘
```

---

## 12. Design Priorities

1. **Memory card density.** The Explorer is the most-used screen. Cards should be scannable — layer icon, first line of content, strength bar, tags, time. Everything else is secondary.

2. **Graph is the wow factor.** The Memory Graph with typed edges is the visual differentiator. No other memory product shows associative structure. Make it beautiful.

3. **Context preview is the sales tool.** The live preview on the Config screen shows exactly what the LLM receives. A developer who sees their own memories assembled into a coherent context window understands the product instantly.

4. **Dark mode first.** This is a developer tool. Dark mode is the default.

5. **Responsive but desktop-first.** The primary user is at a desk with a large monitor. Mobile is read-only (browse memories, check stats).

6. **Fast.** Local vault. Sub-10ms searches. No spinners for local operations. Use optimistic UI for mutations.
