# Engram by El AI Intelligence — Competitive Analysis

**Date:** 2026-08-05  
**Status:** Complete

---

## Executive Summary

The 2026 AI agent memory market is crowded at the top (mem0, Zep, Letta) but **completely empty** in ELLM Engrams' target quadrant: local-first + encrypted at rest + biology-inspired decay + imagined-memory quarantine + deterministic retrieval. No competitor offers more than two of these properties. The market is also rapidly commoditizing as Anthropic, OpenAI, and Google ship platform-native memory — making differentiation on encryption, decay, and determinism the only durable strategy.

---

## Detailed Competitor Profiles

### mem0 — the default memory layer

- **24K-61K stars, $24M Series A, Apache 2.0**
- Dual-store: vector DB + optional knowledge graph. Four scopes (conversation/session/user/organizational)
- **No encryption** in local mode. No decay. No real-vs-imagined distinction
- **No typed links** (entity relations only, $249/mo Pro tier)
- **No priority-tiered context assembly** — just token-aware retrieval
- Free tier (10K memories) / $19/mo / $249/mo
- **Weakness:** CVSS 8.1 vulnerability disclosed April 2026; vendor benchmarks failed independent reproduction (claimed 94.4%, measured 73.8%)

### Zep — the temporal knowledge graph

- **20K-29K stars, YC-backed, enterprise-focused**
- Bi-temporal knowledge graph: Episodes → Entities → Communities with validity windows
- **Best-in-class encryption** in cloud (AES-256-GCM + BYOK + CloudTrail). **None** in self-host
- **Best-in-class temporal model** — "what was true on date X" queries. No causal/analogical links
- **No decay** (validity-based supersession, not forgetting curve). No imagined-memory quarantine
- Free / $25/mo / Enterprise ($125-1,250+/mo)
- **Weakness:** No local-first option; self-host encryption is entirely on the customer

### Letta (formerly MemGPT) — the agent runtime

- **21K-24K stars, $10M seed, Apache 2.0**
- OS-inspired: Core (RAM) / Recall (disk) / Archival (cold). Agent self-edits memory
- **No encryption** documented. **No decay** (agent judgment decides). **No typed links**
- Dream Agents generate synthetic memories — **no quarantine**, merge directly into memory blocks
- Agent-managed token budget (non-deterministic, model-dependent)
- Free / $20-200/mo
- **Weakness:** Memory quality depends on model quality; every memory op costs inference tokens

### LangChain / LangGraph / LangMem

- **Flat key-value + optional vector index.** No layers, no graph, no encryption
- Checkpointer (short-term) + Store (long-term) + LangMem (extraction tools)
- **No decay, no typed links, no imagined quarantine**
- Free (MIT)
- **Weakness:** 59.8s p95 latency in third-party tests; no priority-tiered assembly

### Emerging local-first players (ELLM's direct cohort)

- **Basic Memory:** Markdown + SQLite FTS5 + sqlite-vec, MCP-native. No encryption, no decay
- **Eidetic OS:** SQLite + BM25/vector RRF, Ed25519 hash-chain verification. Strongest audit story among locals. No decay
- **GBrain:** Git-versioned markdown, HNSW + tsvector. No encryption, no decay
- **mnemo:** DuckDB, HMAC-chained writes, provenance-signed reads. Strong audit, no encryption-at-rest

### Platform commoditization threat

- **Anthropic:** CLAUDE.md + Persistent Memory beta + "Dreaming" (hippocampal consolidation, 6x task-completion claim)
- **OpenAI:** ChatGPT persistent memory + `file_search` vector-store retrieval
- **Google:** Memory Bank (identity-scoped persistence in Gemini Enterprise)
- **1M-token context is now free** (Claude Opus 4.6, GPT-5.4, Gemini 3.1) — "just stuff it in context" is cheaper than a memory stack below ~500K tokens

### Biology-inspired decay (emerging niche)

2026 academic systems (YourMemory, SuperLocalMemory V3.3, FSFM) prove the decay approach is credible and competitive (YourMemory: +16pp over mem0 on LoCoMo). **None of the major platforms have shipped it.** None pair it with encryption, quarantine, and determinism.

---

## Summary Matrix

| Dimension | ELLM Engrams | mem0 | Zep | Letta | LangChain |
|---|---|---|---|---|---|
| Layers | 3 (Epi/Sem/Imagined) | 4 scopes | 3-tier KG | 3 (Core/Recall/Archival) | Flat KV |
| Encryption at rest | **SQLCipher** | None | Cloud only | None | DB-dependent |
| Local-first | **Yes** | Yes | No | No | Yes |
| Decay/forgetting | **Ebbinghaus + Hebbian** | None | Validity windows | Agent self-edit | None |
| Real vs imagined | **Quarantined** | No | No | No | No |
| Typed links | **4 types** | Entity (Pro) | Temporal edges | None | None |
| Context assembly | **Priority tiers + budget + compaction** | Token-aware | Token-efficient | Agent-managed | Manual |
| Deterministic | **Content-addressed** | Stable hash (local) | Provenance chain | Git-versioned | State snapshots |
| Temporal patterns | **Day/time detection** | No | As-of queries | No | No |
| SDKs | **None yet** | Python/JS/Go | Python/TS/Go | Python/TS | Python/JS |
| MCP server | **No** | Yes | Yes | Yes | No |
| Benchmarks | None | 73.8% contested | 63.8% | ~74% | n/a |

---

## Positioning Recommendation

### Lead with the unclaimed quadrant

"The only local-first agent memory that is encrypted at rest, deterministic, and biologically modeled." No competitor can answer all three claims.

### Unique selling points (in priority order)

1. **Encrypted at rest, local-first** — the single cleanest gap in the market
2. **Imagined-memory quarantine** — safety feature no vendor markets ("agents never act on ungrounded memories")
3. **Ebbinghaus decay** — answer to context rot and stale-memory degradation (documented 2026 problem: 13% vs 39% accuracy)
4. **Deterministic audit** — content-addressed frames, traceable retrieval (for regulated domains)
5. **Priority-tiered context assembly** — the question every RAG pipeline gets wrong

### Do not compete on

- Integrations (no SDKs yet)
- Benchmarks (none published)
- Graph multi-hop (Zep's strongest feature)
- Price (platform memory is free)

### Urgent roadmap

1. **Python SDK + MCP server** — table stakes; fastest path to adoption
2. **Published benchmarks** (LongMemEval minimum) — without these marketing claims are discounted
3. **Standalone deployment story** — so it's not perceived as axiom-daemon-only
4. **Bi-temporal validity windows** or "as-of" query to answer Zep's strongest feature
