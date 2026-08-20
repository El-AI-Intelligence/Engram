# Engram MCP — Your Memory in Every AI Tool

`engramd-mcp` exposes your Engram vault to any Model Context Protocol
client over stdio. The assistant on the other end can search, capture, and
recall — memories you save in Claude show up in Cursor, and vice versa.

All data stays on your machine: the MCP server is a thin HTTP client in
front of your local `engramd` daemon. No cloud, no telemetry, no API keys.

## Tools

| Tool | What it does |
|---|---|
| `engram_search` | Search the vault (FTS5 + vector hybrid when embeddings are enabled) |
| `engram_capture` | Capture a memory. Duplicates/noise are **skipped and reported** — the tool never pretends a skipped capture was stored |
| `engram_get` | Read one memory by ID |
| `engram_context` | Assemble the most relevant memories into a context block ready to inject |
| `engram_health` | Vault stats, uptime, connection status |
| `engram_decay` | Run memory hygiene (strengthening + decay) after a long session |

## Install

```bash
# 1. The daemon must be running (it serves the API the MCP tools call)
engram daemon        # → http://localhost:8787

# 2. Install the MCP server binary
cargo install --path crates/engramd-mcp

# 3. Configure every supported AI tool on this machine
engram mcp install
```

`engram mcp install` is interactive: it shows what it will do, then asks
**yes/no per editor** before writing anything (default yes — press Enter
to accept). It:

- merges an `engram` entry into the configs it finds (Claude Desktop,
  Cursor, Windsurf) — it never clobbers other MCP servers you've
  configured,
- creates a fresh config where an editor is installed but unconfigured,
- runs `claude mcp add --scope user` for Claude Code when the `claude` CLI
  is on your PATH,
- prints the exact snippet for anything it can't detect.

Preflight checks warn you if `engramd-mcp` is missing from your PATH or
the daemon isn't answering at the URL.

Non-interactive use:

```bash
engram mcp install --yes       # skip the prompts, write everything
engram mcp install --dry-run   # print the plan, write nothing
```

A piped/scripted run without `--yes` prints the plan and exits 1 —
nothing is ever written silently.

Check state any time:

```bash
engram mcp status
```

## Per-editor setup

### Claude Desktop

- **macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Linux:** `~/.config/Claude/claude_desktop_config.json`
- **Windows:** `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "engram": {
      "command": "engramd-mcp",
      "args": ["--engramd-url", "http://127.0.0.1:8787"]
    }
  }
}
```

### Claude Code

`engram mcp install` adds the server for you via `claude mcp add --scope
user`. To do it manually (or after installing the `claude` CLI):

```bash
claude mcp add --scope user engram -- engramd-mcp --engramd-url http://127.0.0.1:8787
```

Claude Code manages its own config — don't hand-edit it. (Or run `claude
mcp add --scope project ...` inside a repo. The engram repo ships a
project-scoped [`.mcp.json`](../../.mcp.json) you can approve on first
use.)

### Cursor

`~/.cursor/mcp.json` — same shape as Claude Desktop:

```json
{
  "mcpServers": {
    "engram": {
      "command": "engramd-mcp",
      "args": ["--engramd-url", "http://127.0.0.1:8787"]
    }
  }
}
```

### Windsurf

`~/.codeium/windsurf/mcp_config.json` — same shape.

## Talking to a different daemon

Everything defaults to `http://127.0.0.1:8787`. Point elsewhere with the
`--url` flag (or the `ENGRAMD_URL` env var):

```bash
engram mcp install --url http://192.168.1.50:8787   # a daemon on another machine
```

## What a session looks like

```
You: "What did we decide about the deployment last week?"
  → engram_search("deployment decision")
You: "Remember: we're freezing the API for the beta."
  → engram_capture(content=..., tags=["beta", "api"])   # "Memory captured. ID: eng_…"
You: "Remember: we're freezing the API for the beta."   # (again)
  → "Already in your vault (matches eng_…) — not stored again."
```

## Troubleshooting

- **Tools error with "engramd unreachable"** — the daemon isn't running.
  Start it: `engram daemon` (it opens your init-created vault and loads
  the passphrase automatically).
- **Editor shows the server as failed/crashed** — `engramd-mcp` isn't on
  the PATH the editor spawns with. Use an absolute path in `command`, or
  run `cargo install --path crates/engramd-mcp` and restart the editor.
- **`engram mcp install` says config "is not valid JSON"** — it leaves the
  broken file untouched; fix it or delete it and re-run.
- **MCP logs** go to stderr (the editor usually surfaces them in its MCP
  panel): "MCP client connected: <editor>" confirms a healthy handshake.
- **Search feels keyword-only** — embeddings are off on the daemon.
  `PATCH /config` `embedding: {enabled: true, model: "all-MiniLM-L6-v2 (local ONNX)", dimensions: 384}`
  and restart; search then runs hybrid (vector + FTS5) automatically.
