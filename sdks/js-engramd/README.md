# Engram Memory Vault — JavaScript/TypeScript SDK

`npm install engramd` — TypeScript SDK + CLI for the Engram Memory Vault.

## Quick start

### SDK (JavaScript/TypeScript)

```typescript
import { MemoryVault } from "engramd";

const vault = new MemoryVault(); // defaults to http://localhost:8787

// Health check
const health = await vault.health();
console.log(`${health.memories_total} memories`);

// Capture a memory
const mem = await vault.capture("The Q3 deployment uses PostgreSQL 16 with pgvector 0.7", {
  tags: ["deployment", "postgres"],
  layer: "episodic",
});

// Search memories
const results = await vault.search("PostgreSQL");
for (const m of results) {
  console.log(`[${m.layer}] ${m.content}`);
}

// Assemble context for an LLM
const ctx = await vault.assembleContext("database setup for Q3");
console.log(`Retrieved ${ctx.engrams_retrieved} memories, ${ctx.token_count} tokens`);

// Run decay
const report = await vault.runDecay();
console.log(`Strengthened: ${report.strengthened}, Decayed: ${report.decayed}`);
```

### CLI

```bash
npm install -g engramd
engram init          # Interactive setup
engram capture "..." # Capture a memory
engram search "query" # Search your vault
engram daemon        # Start the vault server
engram today         # Today's memories
engram demo          # Seed sample memories
```

### MCP Server

```bash
npm install -g engramd
engramd-mcp           # Starts MCP server on stdio
```

Or add to Claude Desktop's MCP config:

```json
{
  "engramd": {
    "command": "npx",
    "args": ["engramd-mcp"]
  }
}
```

Point to a remote vault:

```bash
ENGRAMD_URL=https://your-vault.example.com engramd-mcp
```

## API Reference

### `MemoryVault`

```typescript
class MemoryVault {
  constructor(opts?: { baseUrl?: string; apiKey?: string; timeout?: number })

  // Health
  health(): Promise<VaultHealth>

  // Memories CRUD
  capture(content: string, opts?: CaptureOptions): Promise<Memory>
  get(memoryId: string): Promise<Memory>
  search(query?: string | null, opts?: SearchOptions): Promise<Memory[]>
  list(limit?: number, offset?: number): Promise<Memory[]>
  update(memoryId: string, opts: UpdateOptions): Promise<Memory>
  delete(memoryId: string): Promise<boolean>

  // Links
  link(sourceId: string, targetId: string, opts?: LinkOptions): Promise<boolean>
  getLinks(memoryId: string): Promise<EngramLink[]>
  getRelated(memoryId: string, limit?: number): Promise<Memory[]>
  ground(memoryId: string): Promise<Memory>

  // Context
  assembleContext(query: string, opts?: AssembleContextOptions): Promise<ContextAssembly>
  contextStream(sessionId?: string): AsyncGenerator<Record<string, unknown>>

  // Consolidation
  runDecay(): Promise<ConsolidationResult>
  runConsolidation(): Promise<ConsolidationResult>
  consolidationHistory(): Promise<Record<string, unknown>[]>

  // Analytics
  stats(): Promise<Stats>
  detectPatterns(query?: string, opts?: PatternOptions): Promise<TemporalPattern>

  // Export/Import
  export(opts?: ExportOptions): Promise<Record<string, unknown>>
  importMemories(memories: Record<string, unknown>[]): Promise<{ imported: number; skipped: number }>

  // Config
  getConfig(): Promise<Record<string, unknown>>
  updateConfig(config: Record<string, unknown>): Promise<Record<string, unknown>>
}
```

## Supported platforms

| Platform | Arch | CLI binary | SDK |
|----------|------|------------|-----|
| macOS | x86_64 | ✅ | ✅ |
| macOS | arm64 (Apple Silicon) | ✅ | ✅ |
| Linux | x86_64 | ✅ | ✅ |
| Linux | arm64 | ✅ | ✅ |

## Alternative install methods

```bash
cargo install engramd          # Rust (crates.io)
brew install engramd            # macOS (Homebrew)
pip install engramd             # Python SDK
docker pull ghcr.io/pixelphantomai/engramd  # Docker
```
