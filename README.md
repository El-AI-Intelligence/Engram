# Engram Memory Vault

**Local-first, encrypted memory for AI agents.**

Engram captures what your AI experiences, remembers it with biology-inspired decay, and assembles the optimal context window for every inference call — all encrypted on your machine.

```
engramd                      # start the vault
→ POST /memories             # capture what happened
→ POST /context/assemble     # get the optimal context window
→ drop into any LLM call
```

## Architecture

```
┌─────────────────────────────────────────┐
│  engramd (REST + SSE)                   │
│  ┌───────────────────────────────────┐  │
│  │ L1: QEM Cache (O(1) associative)  │  │
│  │ L2: SQLCipher vault (encrypted)   │  │
│  │ L3: Context assembler (priority)  │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

## Quick Start

```sh
# Start the vault
cargo run -p engramd

# Capture a memory
curl -X POST localhost:8787/memories \
  -H 'content-type: application/json' \
  -d '{"content":"User is refactoring the ELLM kernel","tags":["rust","ellm"]}'

# Search memories
curl -X POST localhost:8787/memories/search \
  -H 'content-type: application/json' \
  -d '{"query":"ELLM kernel","limit":5}'

# Get the optimal context window
curl -X POST localhost:8787/context/assemble \
  -H 'content-type: application/json' \
  -d '{"query":"What should I work on?","token_budget":4096}'
```

## What makes it different

| Capability | Engram | mem0 | Zep | Letta |
|---|---|---|---|---|
| Encrypted at rest, local-first | ✅ | ❌ | ❌ | ❌ |
| Imagined-memory quarantine | ✅ | ❌ | ❌ | ❌ |
| Biology-inspired decay | ✅ Ebbinghaus | ❌ | ❌ | ❌ |
| Typed memory links | ✅ 4 types | ❌ | Entity (Pro) | ❌ |
| Priority-tiered context assembly | ✅ | ❌ | ❌ | ❌ |
| Deterministic, auditable retrieval | ✅ | ❌ | ❌ | ❌ |

## Project Structure

```
crates/
  engram-core/     — types, encrypted storage, decay biology
  engram-memory/   — priority-tiered context window assembly
  engramd/         — standalone REST + SSE server
sdks/
  python-engramd/  — Python client + MCP server
ui/                — web dashboard (7 screens)
docs/              — design docs, API reference, audit
```

## Status

- [x] Encrypted SQLCipher vault with SHA-256 key derivation
- [x] Three memory layers: Episodic, Semantic, Imagined
- [x] Ebbinghaus decay + Hebbian strengthening
- [x] FTS5 full-text search + vector embeddings
- [x] Typed links: Associative, Causal, Analogical, Temporal
- [x] Priority-tiered context assembly with token budget
- [ ] Standalone engramd server (in progress)
- [ ] Python SDK + MCP server
- [ ] Web dashboard UI
