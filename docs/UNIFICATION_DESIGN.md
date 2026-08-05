# Engram Memory Vault — Unification Design

**Date:** 2026-08-05  
**Status:** Draft for review  
**Goal:** Merge three independent memory systems into one layered architecture

---

## 1. Current State: Three Islands

```
┌─ ELLM KERNEL ─────────────────────────────────────────┐
│                                                        │
│  QEM (458 lines)                                       │
│  HashMap<QemCode, Entry> + HashMap<QemCode, Assoc>     │
│  32-bit XOR holographic codes                          │
│  NoveltyFilter (prediction-error gating)               │
│  Volatile — dies with process                          │
│  No persistence, no encryption, no link graph          │
│                                                        │
│  AutobiographicalMemory (1,225 lines)                  │
│  JSONL files: episodes.jsonl + narratives.jsonl        │
│  In-memory EpisodeIndex (3 HashMaps)                   │
│  Episode → Narrative distillation (manual)             │
│  Linear file scan for episode loading                  │
│  No encryption, no decay, no link graph                │
│                                                        │
│  ENGRAM_* relations (5 RelationIds: 316-320)           │
│  EngramEffectHandler bridges to Axiom                  │
└────────────────────┬───────────────────────────────────┘
                     │ EXEC frame protocol
┌─ AXIOM-OS ─────────┼───────────────────────────────────┐
│                     │                                   │
│  EngramStore (1,417 lines)                              │
│  SQLCipher SQLite: engrams + links + embeddings         │
│  3 layers: Episodic / Semantic / Imagined              │
│  FTS5 + vector (brute-force cosine) + LIKE fallback    │
│  Ebbinghaus decay + Hebbian strengthening              │
│  Weekly consolidation (promotion + pruning)            │
│  Typed links (4 types)                                 │
│  Temporal pattern detection                            │
│  Embedded library — no server, no SDK                  │
│                                                        │
│  ContextAssembler (759 lines)                           │
│  Priority-tiered slots: Required→High→Normal→Low       │
│  Token budget management + extractive compaction       │
│  OpenAI-compatible message output                      │
│  Library only — doesn't own retrieval                  │
└────────────────────────────────────────────────────────┘
```

**The problem:** Three implementations, no shared interface, overlapping responsibilities (all three do "memory retrieval"), none can talk to each other directly. QEM is the best cache but can't warm from EngramStore. AutobiographicalMemory has the richest episode model but can't benefit from EngramStore's encryption or decay. ContextAssembler assembles context but doesn't own retrieval.

---

## 2. Target State: One Layered Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  UNIFIED MEMORY VAULT                     │
│                                                           │
│  ┌─────────────────────────────────────────────────────┐ │
│  │ L0: MemoryInterface (trait)                          │ │
│  │                                                     │ │
│  │ trait MemoryBackend {                                │ │
│  │   async fn capture(&self, entry: MemoryEntry) -> ID; │ │
│  │   async fn retrieve(&self, id: &ID) -> Option<Entry>;│ │
│  │   async fn search(&self, query: Query) -> Vec<Entry>;│ │
│  │   async fn link(&self, src: ID, tgt: ID, LinkType); │ │
│  │   async fn consolidate(&self) -> ConsolidationReport;│ │
│  │   async fn decay(&self) -> DecayReport;              │ │
│  │ }                                                    │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │                                 │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │ L1: QemCache (wraps L2, implements MemoryBackend)    │ │
│  │                                                     │ │
│  │ 32-bit XOR holographic codes                        │ │
│  │ NoveltyFilter with configurable window               │ │
│  │ write-through to L2 on capture                       │ │
│  │ warm from L2 on startup (replay recent N entries)    │ │
│  │ O(1) lookup for hot path                             │ │
│  │ Associative: subject XOR relation → object           │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │ miss                            │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │ L2: VaultStore (SQLCipher SQLite, implements Backend) │ │
│  │                                                     │ │
│  │ Unified schema: memories + episodes + narratives    │ │
│  │ 3 layers: Episodic / Semantic / Imagined            │ │
│  │ FTS5 + vector (with ANN upgrade path) + LIKE        │ │
│  │ Ebbinghaus decay + Hebbian strengthening            │ │
│  │ Typed links (Associative/Causal/Analogical/Temporal)│ │
│  │ Consolidation: episodic→semantic + imagined pruning │ │
│  │ Temporal pattern detection                          │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │                                 │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │ L3: ContextAssembler (reads from MemoryInterface)     │ │
│  │                                                     │ │
│  │ Owns retrieval: query → L1→L2→FTS5→vector→assemble │ │
│  │ Priority-tiered slots: Required→High→Normal→Low     │ │
│  │ Token budget + extractive/abstractive compaction    │ │
│  │ Streaming incremental updates                       │ │
│  │ OpenAI + Anthropic message format output            │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  ┌─────────────────────────────────────────────────────┐ │
│  │ HTTP + gRPC API surface                               │ │
│  │ /memories, /context, /consolidation, /export          │ │
│  └─────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

---

## 3. The Unified MemoryEntry

### 3.1 Why one type

Currently three different structs represent "a memory":
- `QemEntry` — code, name, quant, source
- `Episode` — episode_id, session_id, frames, turns, outcome_quality
- `Engram` — id, layer, source, content, context, links, strength, valence

All three are a memory with: identity, content, source, confidence/strength, temporal metadata, and typed connections to other memories. The differences are presentation, not substance.

### 3.2 Unified type

```rust
/// A single memory entry — the universal unit of the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    // ── Identity ─────────────────────────────────────────
    pub id: MemoryId,                    // UUID v4 or content-addressed
    pub layer: MemoryLayer,              // Episodic | Semantic | Imagined

    // ── Content ─────────────────────────────────────────
    pub content: String,                 // Human-readable content
    pub content_type: ContentType,       // Text | Frames | Conversation | Context

    // ── Classification ──────────────────────────────────
    pub source: MemorySource,            // Interaction | Window | Agent | etc.
    pub scope: MemoryScope,              // Moment | Episode | Narrative | Rule
    pub tags: Vec<String>,
    pub project: Option<String>,

    // ── Confidence & affect ─────────────────────────────
    pub strength: f64,                   // 0.0–2.0, Ebbinghaus-decayed
    pub valence: f64,                    // -1.0–1.0 emotional charge
    pub imagined: bool,                  // true → quarantine applies
    pub grounded: bool,                  // true → quarantine cleared

    // ── Evidence & provenance ──────────────────────────
    pub evidence: Vec<EvidenceRef>,      // Links to supporting memories
    pub retrieval_count: u32,            // Hebbian strengthening counter

    // ── Links to other memories ────────────────────────
    pub links_out: Vec<MemoryLink>,      // Outgoing typed links

    // ── Temporal ────────────────────────────────────────
    pub created_at: DateTime<Utc>,
    pub last_retrieved: Option<DateTime<Utc>>,
    pub occurred_at: Option<DateTime<Utc>>, // When the event happened (≠ created_at)

    // ── Context (structured metadata) ──────────────────
    pub context: serde_json::Value,

    // ── QEM code (computed, not stored) ─────────────────
    // qem_code: QemCode — derived from content hash
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryLayer {
    Episodic,    // Direct experience — what happened
    Semantic,    // Distilled abstraction — what was learned
    Imagined,    // AI-generated — quarantine applies
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryScope {
    Moment,      // A single observation (QEM entry, context capture)
    Episode,     // A multi-turn session (AutobiographicalMemory episode)
    Narrative,   // A durable storyline across episodes
    Rule,        // A crystallized rule from consolidation
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType {
    Text,           // Plain text (most engrams)
    Frames,         // ELLM frame graph (reasoning traces)
    Conversation,   // Multi-turn chat (session episodes)
    Context,        // Environment state (window titles, etc.)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLink {
    pub target_id: MemoryId,
    pub weight: f64,
    pub link_type: LinkType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkType {
    Associative,  // "reminds me of"
    Causal,       // "led to"
    Analogical,   // "is like"
    Temporal,     // "happened before/after"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub memory_id: MemoryId,
    pub relationship: String,  // "supports", "contradicts", "context_for"
}
```

### 3.3 Migration: old types → MemoryEntry

| Old type | Scope | ContentType | Notes |
|----------|-------|-------------|-------|
| `QemEntry` | `Moment` | `Text` | QemCode derived from content hash; `source` = `DreamDistilled/MicroConsolidated` → `MemorySource::Consolidation` |
| `Episode` | `Episode` | `Conversation` | `frames` serialized to `context.frames`; `turns` → `context.turns`; `outcome_quality.0` → `strength * 255` |
| `Narrative` | `Narrative` | `Text` | `storyline` → `content`; `open_loops` → `tags`; `dominant_domains` → `tags` |
| `Engram` | `Moment` (default) | `Text` (default) | Direct field mapping; `links` Vec → `links_out` Vec |
| Window context | `Moment` | `Context` | Existing context capture |

---

## 4. Layer 1: QemCache

### 4.1 Design

```rust
/// L1 cache wrapping an L2 backend.
/// All reads check QEM first. All writes go through to L2.
pub struct QemCache<B: MemoryBackend> {
    /// The L2 backend (VaultStore)
    backend: B,

    /// Direct lookup: QemCode → cached entry summary
    by_code: HashMap<QemCode, CachedEntry>,

    /// Associative lookup: subject XOR relation → object
    associations: HashMap<QemCode, CachedAssociation>,

    /// Novelty filter for prediction-error gating
    novelty: NoveltyFilter,

    /// Configuration
    config: QemConfig,
}

struct CachedEntry {
    memory_id: MemoryId,
    qem_code: QemCode,
    strength: f64,
    source: QemSource,
}

struct CachedAssociation {
    subject_code: QemCode,
    relation_code: QemCode,
    object_code: QemCode,
    object_name: String,
    memory_id: MemoryId,
    strength: f64,
}
```

### 4.2 Startup warm

```rust
impl<B: MemoryBackend> QemCache<B> {
    /// Warm the cache from the L2 backend on startup.
    /// Loads the `warm_limit` most recent high-strength entries.
    pub async fn warm(&mut self, warm_limit: usize) -> Result<()> {
        let recent = self.backend
            .query(Query::new()
                .min_strength(self.config.warm_strength_min)
                .sort_by(SortKey::Strength)
                .limit(warm_limit))
            .await?;

        for entry in recent {
            let code = QemEncoder::encode(&entry.content, entry.layer.into());
            self.by_code.insert(code, CachedEntry {
                memory_id: entry.id.clone(),
                qem_code: code,
                strength: entry.strength,
                source: QemSource::Encoded,
            });
            self.novelty.observe(code); // pre-populate filter
        }
        Ok(())
    }
}
```

### 4.3 Write-through

```rust
impl<B: MemoryBackend> MemoryBackend for QemCache<B> {
    async fn capture(&self, entry: MemoryEntry) -> Result<MemoryId> {
        // 1. Write to L2 first (durability)
        let id = self.backend.capture(entry.clone()).await?;

        // 2. Populate L1 cache
        let code = QemEncoder::encode(&entry.content, entry.layer.into());
        let surprise = self.novelty.observe(code);
        let quant = self.novelty.surprise_to_quant(surprise);
        self.by_code.insert(code, CachedEntry {
            memory_id: id.clone(),
            qem_code: code,
            strength: entry.strength,
            source: QemSource::Encoded,
        });

        Ok(id)
    }

    async fn retrieve(&self, id: &MemoryId) -> Result<Option<MemoryEntry>> {
        // 1. Check L1 by scanning cached entries
        if let Some(cached) = self.by_code.values().find(|e| &e.memory_id == id) {
            self.backend.retrieve(id).await // L1 only has summaries, fetch full from L2
        } else {
            // 2. Miss — go to L2
            self.backend.retrieve(id).await
        }
    }

    async fn search(&self, query: Query) -> Result<Vec<MemoryEntry>> {
        // 1. Try associative lookup if query has subject+relation
        if let (Some(subject), Some(relation)) = (query.subject_code, query.relation_code) {
            let key = subject.bind(relation);
            if let Some(assoc) = self.associations.get(&key) {
                if assoc.strength > self.config.min_association_strength {
                    // Cache hit — fetch full entry from L2
                    if let Some(entry) = self.backend.retrieve(&assoc.memory_id).await? {
                        return Ok(vec![entry]);
                    }
                }
            }
        }

        // 2. Fall through to L2 search
        self.backend.search(query).await
    }
}
```

### 4.4 Configuration

```rust
pub struct QemConfig {
    /// Minimum strength for entries loaded during warm
    pub warm_strength_min: f64,         // default: 0.3
    /// Number of entries to warm on startup
    pub warm_limit: usize,              // default: 1000
    /// Novelty filter window size
    pub novelty_window: usize,          // default: 100
    /// Minimum surprise for high-confidence storage
    pub surprise_min: f32,              // default: 0.2
    /// Minimum association strength for cache hit
    pub min_association_strength: f64,  // default: 0.1
    /// Maximum entries in L1 (evict LRU beyond this)
    pub max_entries: usize,             // default: 10_000
}
```

---

## 5. Layer 2: VaultStore

### 5.1 Unified schema

The existing `axiom-engram` schema is extended to absorb AutobiographicalMemory's episode/narrative model:

```sql
-- Existing table, extended
ALTER TABLE engrams ADD COLUMN scope TEXT NOT NULL DEFAULT 'moment'
    CHECK(scope IN ('moment', 'episode', 'narrative', 'rule'));

ALTER TABLE engrams ADD COLUMN content_type TEXT NOT NULL DEFAULT 'text'
    CHECK(content_type IN ('text', 'frames', 'conversation', 'context'));

ALTER TABLE engrams ADD COLUMN occurred_at TEXT;  -- when the event happened

-- New: evidence references (provenance chain)
CREATE TABLE IF NOT EXISTS memory_evidence (
    memory_id    TEXT NOT NULL REFERENCES engrams(id) ON DELETE CASCADE,
    evidence_id  TEXT NOT NULL REFERENCES engrams(id) ON DELETE CASCADE,
    relationship TEXT NOT NULL DEFAULT 'supports',
    PRIMARY KEY (memory_id, evidence_id)
);

-- New: consolidation run history (moved from separate table)
-- consolidation_runs table already exists, unchanged
```

### 5.2 Episode storage

Episodes become `MemoryEntry` with `scope = Episode`, `content_type = Conversation`:

```rust
impl From<Episode> for MemoryEntry {
    fn from(ep: Episode) -> Self {
        MemoryEntry {
            id: MemoryId::from_u64(ep.episode_id),
            layer: MemoryLayer::Episodic,
            content: ep.turns.iter()
                .map(|t| format!("[{}]: {}", t.speaker, t.raw_text))
                .collect::<Vec<_>>()
                .join("\n"),
            content_type: ContentType::Conversation,
            source: MemorySource::Interaction,
            scope: MemoryScope::Episode,
            tags: ep.domains,
            project: None,
            strength: ep.outcome_quality.0 as f64 / 255.0 * 2.0,
            valence: if ep.is_success() { 1.0 } else if ep.is_failure() { -1.0 } else { 0.0 },
            imagined: false,
            grounded: true,
            evidence: vec![],
            retrieval_count: 0,
            links_out: vec![],
            created_at: /* from ep.started_at */,
            last_retrieved: None,
            occurred_at: /* from ep.started_at */,
            context: serde_json::json!({
                "session_id": ep.session_id,
                "frames": ep.frames,
                "turns": ep.turns,
                "outcome_quality": ep.outcome_quality.0,
                "goal_achieved": ep.goal_achieved,
                "turn_count": ep.turn_count,
            }),
        }
    }
}
```

### 5.3 Narrative storage

Narratives become `MemoryEntry` with `scope = Narrative`, `content_type = Text`:

```rust
impl From<Narrative> for MemoryEntry {
    fn from(n: Narrative) -> Self {
        MemoryEntry {
            id: MemoryId::from_string(&n.narrative_id),
            layer: MemoryLayer::Semantic,  // narratives are distilled → semantic
            content: n.storyline,
            content_type: ContentType::Text,
            source: MemorySource::Consolidation,
            scope: MemoryScope::Narrative,
            tags: n.dominant_domains,
            project: Some(n.project_key),
            strength: (n.success_count + 1) as f64 / (n.success_count + n.failure_count + 1) as f64 * 2.0,
            valence: 0.0,  // narratives are neutral by default
            imagined: false,
            grounded: true,
            evidence: n.supporting_episode_ids.iter()
                .map(|id| EvidenceRef {
                    memory_id: MemoryId::from_u64(*id),
                    relationship: "supports".to_string(),
                })
                .collect(),
            retrieval_count: 0,
            links_out: vec![],
            // ...
            context: serde_json::json!({
                "narrative_id": n.narrative_id,
                "scope": n.scope.as_str(),
                "session_id": n.session_id,
                "open_loops": n.open_loops,
                "goal": n.goal,
            }),
        }
    }
}
```

### 5.4 EpisodeIndex → database queries

The in-memory `EpisodeIndex` (3 HashMaps) is replaced with indexed SQL queries:

```sql
-- by_topic: WHERE context->>'$.topic' = ?
-- Already fast with json_each or extracted column if we denormalize

-- by_topic_name: WHERE tags LIKE '%"topic_name"%'
-- Already indexed via FTS5

-- by_entity: WHERE id IN (
--   SELECT memory_id FROM memory_evidence WHERE evidence_id = ?
-- )
```

### 5.5 Vector search upgrade path

Current brute-force approach: load all embeddings, compute cosine in Rust. Fine for <10K engrams.

Upgrade path (not in v1, but the interface supports it):
1. **Option A:** `sqlite-vec` extension — vector index in SQLite, zero external deps
2. **Option B:** `usearch` — single-header C library, HNSW index, disk-backed
3. **Option C:** Separate vector DB (Qdrant/Milvus) — overkill for local-first product

The `MemoryBackend` trait's `search()` method accepts a `Query` with optional embedding, so the implementation can be swapped without API changes.

---

## 6. Layer 3: ContextAssembler

### 6.1 Owns retrieval

Current: ContextAssembler receives pre-selected slots. Caller decides which engrams to include.

Target: ContextAssembler queries the vault directly:

```rust
impl ContextAssembler {
    /// Assemble a context window for a query, owning the full retrieval pipeline.
    pub async fn assemble_for_query<B: MemoryBackend>(
        &self,
        backend: &B,
        query: &str,
        session_id: &str,
    ) -> Result<AssembledContext> {
        // 1. Build required slots (system prompt, character, current turn)
        let mut slots = self.build_required_slots(query);

        // 2. Retrieve relevant memories from vault
        let query_embedding = self.embed(query).await?;
        let memories = backend.search(
            Query::new()
                .text(query)
                .embedding(query_embedding)
                .min_strength(0.1)
                .exclude_session(session_id)
                .limit(10)
        ).await?;

        // 3. Convert memories to context slots
        for memory in memories {
            let priority = match memory.layer {
                MemoryLayer::Semantic => ContextPriority::High,
                MemoryLayer::Episodic => ContextPriority::Normal,
                MemoryLayer::Imagined => ContextPriority::Low,
            };
            slots.push(ContextSlot::new(
                ContextRole::System,
                format_memory_for_context(&memory),
                ContextSource::EngramRetrieval,
            ).with_priority(priority));
        }

        // 4. Add recent conversation history
        slots.extend(self.load_recent_history(session_id).await?);

        // 5. Add world context (current window, time, etc.)
        slots.extend(self.load_world_context().await?);

        // 6. Assemble with token budget
        self.assemble(slots)
    }
}
```

### 6.2 Streaming updates

New: SSE endpoint for incremental context updates as new engrams are captured:

```rust
pub struct ContextStream {
    /// Current assembled context
    current: AssembledContext,
    /// Last update timestamp
    last_update: DateTime<Utc>,
    /// Backend reference
    backend: Arc<dyn MemoryBackend>,
}

impl ContextStream {
    /// Poll for new memories since last update, incrementally update context.
    pub async fn poll(&mut self) -> Result<Option<ContextDelta>> {
        let new_memories = self.backend
            .query(Query::new()
                .created_after(self.last_update)
                .min_strength(0.3)
                .limit(5))
            .await?;

        if new_memories.is_empty() {
            return Ok(None);
        }

        let new_slots: Vec<ContextSlot> = new_memories.iter()
            .map(|m| ContextSlot::new(
                ContextRole::System,
                format_memory_for_context(m),
                ContextSource::EngramRetrieval,
            ).with_priority(ContextPriority::High))
            .collect();

        self.last_update = Utc::now();

        Ok(Some(ContextDelta {
            added_slots: new_slots,
            new_total_tokens: self.current.total_tokens,
            budget_remaining: self.current.budget - self.current.total_tokens,
        }))
    }
}
```

### 6.3 Abstractive summarization (optional, gated on LLM config)

The current extractive summarizer is zero-dependency and fast. For higher quality with an LLM backend:

```rust
impl ConversationSummarizer {
    /// Abstractive summary using configured LLM backend.
    /// Falls back to extractive if no LLM is configured.
    pub async fn summarize_with_llm(
        &self,
        slots: &[ContextSlot],
        llm: Option<&dyn LlmBackend>,
    ) -> ContextSlot {
        if let Some(llm) = llm {
            let prompt = format!(
                "Summarize this conversation history in 2-3 sentences. Focus on decisions, goals, and open questions:\n\n{}",
                slots.iter().map(|s| format!("[{}]: {}", s.role.as_str(), s.content)).collect::<Vec<_>>().join("\n")
            );
            if let Ok(summary) = llm.complete(&prompt, 200).await {
                return ContextSlot::new(
                    ContextRole::System,
                    format!("[SUMMARY]: {}", summary),
                    ContextSource::CompactedHistory,
                ).with_priority(ContextPriority::Low);
            }
        }
        // Fallback to extractive
        self.summarize_slots(slots)
    }
}
```

---

## 7. The MemoryBackend trait

```rust
/// Universal interface implemented by QemCache (L1) and VaultStore (L2).
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Capture a new memory entry. Returns the assigned ID.
    async fn capture(&self, entry: MemoryEntry) -> Result<MemoryId>;

    /// Retrieve a single memory by ID.
    async fn retrieve(&self, id: &MemoryId) -> Result<Option<MemoryEntry>>;

    /// Search memories by structured query.
    async fn search(&self, query: Query) -> Result<Vec<MemoryEntry>>;

    /// Create a typed link between two memories.
    async fn link(&self, source: &MemoryId, target: &MemoryId, link_type: LinkType, weight: f64) -> Result<()>;

    /// Get all outgoing links from a memory.
    async fn get_links(&self, id: &MemoryId) -> Result<Vec<MemoryLink>>;

    /// Find memories related to the given one by following links.
    async fn related(&self, id: &MemoryId, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Apply Ebbinghaus decay + Hebbian strengthening.
    /// Returns (strengthened_count, decayed_count).
    async fn apply_decay(&self) -> Result<DecayReport>;

    /// Run weekly consolidation: episodic→semantic promotion, imagined pruning.
    /// Returns (promoted_count, pruned_count).
    async fn consolidate(&self) -> Result<ConsolidationReport>;

    /// Surface relevant memories without explicit search (proactive recall).
    async fn surface(&self, context: &str, limit: usize) -> Result<Vec<(MemoryEntry, f64)>>;

    /// Detect temporal patterns in memory access/creation.
    async fn detect_patterns(&self, query: &str, min_samples: usize) -> Result<Option<TemporalPattern>>;

    /// Total number of memories stored.
    async fn count(&self) -> Result<u64>;

    /// Store an embedding vector for a memory.
    async fn store_embedding(&self, id: &MemoryId, embedding: &[f64]) -> Result<()>;

    /// Vector similarity search.
    async fn vector_search(&self, embedding: &[f64], limit: usize) -> Result<Vec<(MemoryEntry, f64)>>;
}

#[derive(Debug, Clone)]
pub struct Query {
    pub text: Option<String>,
    pub embedding: Option<Vec<f64>>,
    pub layer: Option<MemoryLayer>,
    pub scope: Option<MemoryScope>,
    pub source: Option<MemorySource>,
    pub tags: Vec<String>,
    pub project: Option<String>,
    pub min_strength: Option<f64>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub exclude_session: Option<String>,
    pub sort_by: SortKey,
    pub limit: usize,
    pub offset: usize,
    // QEM-specific: associative lookup
    pub subject_code: Option<QemCode>,
    pub relation_code: Option<QemCode>,
}

#[derive(Debug, Clone)]
pub enum SortKey {
    Strength,
    Recency,
    Valence,
    RetrievalCount,
    Relevance, // combined score
}

#[derive(Debug, Clone)]
pub struct DecayReport {
    pub strengthened: u32,
    pub decayed: u32,
    pub pruned: u32, // entries dropped below threshold
}

#[derive(Debug, Clone)]
pub struct ConsolidationReport {
    pub promoted_to_semantic: u32,
    pub pruned_imagined: u32,
    pub narratives_updated: u32,
    pub rules_crystallized: u32,
}
```

---

## 8. Migration Path

### Phase 1: Define the trait (no behavior change)
1. Add `MemoryBackend` trait to `axiom-engram`
2. Implement it for `EngramStore` (existing methods map directly)
3. Add `MemoryEntry` as the public type, with `From<Engram>` conversion
4. Existing code continues to use `EngramStore` directly — no breakage

### Phase 2: Absorb AutobiographicalMemory
1. Add `scope` and `content_type` columns to engrams table
2. Write migration: scan episodes.jsonl → `MemoryEntry` → EngramStore
3. Replace `EpisodeIndex` with SQL queries
4. Episode storage goes through `MemoryBackend::capture()` instead of direct JSONL append

### Phase 3: Add QemCache
1. Implement `QemCache<EngramStore>` as a wrapper
2. Wire startup warm from EngramStore
3. Replace direct `EngramStore` lookups in hot paths with `QemCache` lookups
4. Measure hit rate, tune `QemConfig`

### Phase 4: ContextAssembler owns retrieval
1. Add `assemble_for_query()` method that queries `MemoryBackend`
2. Add streaming SSE endpoint via `ContextStream`
3. Optional: add abstractive summarization behind LLM config gate

### Phase 5: Deduplicate and remove old code
1. Remove QEM's standalone entry storage (replaced by QemCache)
2. Remove AutobiographicalMemory's JSONL I/O (replaced by EngramStore)
3. Remove `EpisodeIndex` HashMaps (replaced by SQL queries)
4. EngramStore is the single source of truth for all memory persistence

---

## 9. Backward Compatibility

- **QEM:** The `QemStore` type continues to work as-is. `QemCache` wraps it and adds persistence.
- **AutobiographicalMemory:** The `Episode` and `Narrative` types continue to serialize/deserialize. Migration reads JSONL once and writes to EngramStore.
- **EngramStore:** The existing schema is extended with ALTER TABLE ADD COLUMN, not replaced. All existing queries continue to work.
- **ContextAssembler:** The `ContextBuilder` fluent API is unchanged. `assemble_for_query()` is a new method, not a replacement.
- **EngramEffectHandler:** Unchanged — it writes to `MemoryBackend` instead of `EngramStore` directly (via trait object).

---

## 10. Success Metrics

| Metric | Current | Target |
|--------|---------|--------|
| Memory systems in codebase | 3 independent | 1 unified |
| Retrieval paths | 5+ (QEM direct, QEM assoc, AM index, ES FTS5, ES vector) | 1 (QemCache → VaultStore) |
| QEM cache hit rate | 0% (not warmed) | >80% (warmed from vault) |
| Episode load time | O(n) linear scan of JSONL | O(log n) indexed SQL |
| Context assembly | Caller selects engrams | Assembler owns retrieval |
| Cross-system links | None (QEM can't link to EngramStore entries) | Unified link graph |
| Test coverage | Varies (QEM: good, AM: good, ES: partial) | >80% across all backend methods |
