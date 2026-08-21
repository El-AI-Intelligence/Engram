# Engram by El AI Intelligence — Your AI deserves a memory.

[![GitHub release](https://img.shields.io/github/v/release/El-AI-Intelligence/engram?label=latest%20release)](https://github.com/El-AI-Intelligence/engram/releases)
[![GitHub stars](https://img.shields.io/github/stars/El-AI-Intelligence/engram)](https://github.com/El-AI-Intelligence/engram)

A **local-first, end-to-end encrypted memory vault** that gives AI agents persistent recall across sessions. Your AI remembers conversations, decisions, and context — without the memory ever leaving your machine in plaintext.

Built on the Axiom-OS kernel: FTS5 + vector + QEM holographic memory with biological decay, strengthening, and consolidation.

> **Status:** early product, actively developed. The core is MIT-licensed and stays that way — the paid layers (cloud sync, team memory) are optional services on top, never required.

## Why Engram by El AI Intelligence

AI agents forget everything between sessions. Engram by El AI Intelligence fixes that:

- **Zero-knowledge encryption** — AES-256 (SQLCipher). Memories never leave your machine in plaintext; even a self-hosted sync server can't read them.
- **Semantic search** — auto-downloads a local BERT embedding model (all-MiniLM-L6-v2, ~23MB) on first run. Find memories by meaning, not just keywords. Zero config, no Ollama.
- **AI-native API** — REST + WebSocket + MCP server. Claude Code, Cursor, Windsurf — one command to connect your AI tools.
- **Memory hygiene** — automatic decay, strengthening, and consolidation. Important memories get stronger; noise fades away.
- **Weekly digest** — "what your AI learned about you this week": new/reinforced/fading memories, themes clustered from local embeddings. Fully local and free; optional AI-written prose via your own BYO-key endpoint (`engram digest`, or the Digest tab in the vault UI).
- **Cross-device sync** — E2E encrypted sync (AES-256-GCM + HMAC) with vector-clock conflict resolution. Self-host the relay (`engramd-sync`) or use the managed one at `sync.ellmstack.dev`.
- **Privacy controls** — full audit trail, purge by criteria, retention policies. You control what's stored and for how long.

## Quick start

Binaries are published on **GitHub Releases** — the primary download location
for all platforms (Linux, macOS, Windows), each with SHA-256 checksums:
https://github.com/El-AI-Intelligence/engram/releases
The first release, v0.1.0, lands with the public launch.

The one-liner below is the same download, wrapped for convenience (it pulls
the latest release from GitHub):

```bash
curl -fsSL https://engram.ellmstack.dev/install.sh | bash
engram onboarding    # 5-minute setup: vault, first memory, running daemon
engram link          # link this machine to your account — browser opens, one click
```

Windows (PowerShell, no admin needed — auto-starts via Task Scheduler):

```powershell
iex (irm https://engram.ellmstack.dev/install.ps1)
engram init          # answer Y to install the background service
```

The release pipeline signs and notarizes macOS binaries (Apple Developer ID).
All platforms: the daemon auto-starts at login (enable it with `engram init`)
and, once linked, syncs through your Engram by El AI Intelligence account.

Your daemon runs at `http://localhost:8787` (REST/WS API). For the visual
vault UI, open https://engram.ellmstack.dev/app — it connects to your local
daemon.

For AI tools, connect via MCP — one command writes the config for every
supported editor (Claude Desktop, Cursor, Windsurf), merging with whatever
MCP servers you already have:

```bash
engram mcp install   # writes configs + prints snippets for anything undetected
engram mcp status    # binary, daemon, and per-editor state
```

Claude Code: `claude mcp add --scope user engram -- engramd-mcp --engramd-url http://127.0.0.1:8787`.
Full guide: [docs/engram-product/MCP.md](docs/engram-product/MCP.md).

Or from source: `cargo install --path crates/engramd` (also provides the `engram` CLI).

## Repository layout

| Path | What it is |
|---|---|
| `crates/axiom-engram/` | The kernel: schema, FTS5 + vector store, QEM holographic memory, decay/consolidation, E2E sync primitives |
| `crates/axiom-inference/` | Inference backends (ONNX Runtime, llama.cpp — optional features) |
| `crates/engramd/` | The daemon: axum REST + WebSocket API, auth, privacy routes, sync client, static UI serving (`--ui-dir`) |
| `crates/engramd-sync/` | Sync relay server — a stateless, encrypted-blob "dumb pipe" (self-hostable) |
| `crates/engramd-mcp/` | MCP server exposing capture/search/context tools to AI agents |
| `ui/engram-vault/` | The vault web UI (vanilla JS SPA, no build step) |
| `ui/landing/` | Landing page |
| `sdks/python-engramd/` | Python client + MCP server with passive auto-capture observer |
| `sdks/js-engramd/` | TypeScript client + MCP server (npm) |
| `deploy/` | Caddy config, systemd unit, deploy script for the live site |
| `docs/engram-product/` | API surface, deploy/install guides, sync spec, UI spec, revenue roadmap |

## Docs

- [DEPLOY.md](docs/engram-product/DEPLOY.md) — deploy the live site (Caddy + systemd + API key auth)
- [SYNC.md](docs/engram-product/SYNC.md) — E2E sync protocol and self-hosting the relay
- [MCP.md](docs/engram-product/MCP.md) — connect Claude, Cursor, and Windsurf to your vault
- [API_SURFACE.md](docs/engram-product/API_SURFACE.md) — REST/WS API contract
- [REVENUE_ROADMAP.md](docs/engram-product/REVENUE_ROADMAP.md) — the plan for paid layers (cloud sync, team memory) on top of the MIT core

Live site: https://engram.ellmstack.dev — and if Engram by El AI Intelligence is useful, a ⭐ on GitHub is the best signal: [github.com/El-AI-Intelligence/engram](https://github.com/El-AI-Intelligence/engram)

## License

MIT © 2026 Pixel Phantom AI — see [LICENSE](LICENSE).
