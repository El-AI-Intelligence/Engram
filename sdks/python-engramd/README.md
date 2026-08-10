# Engram Memory Vault — Python SDK

`pip install engramd` — Python client for the Engram Memory Vault.

## Quick start

```python
from engramd import MemoryVault

vault = MemoryVault()  # defaults to http://localhost:8787

# Health check
print(vault.health())

# Capture a memory
mem = vault.capture(
    content="The Q3 deployment uses PostgreSQL 16 with pgvector 0.7",
    tags=["deployment", "postgres"],
    layer="episodic",
)

# Search memories
results = vault.search("PostgreSQL")
for m in results:
    print(f"[{m.layer}] {m.content}")

# Assemble context for an LLM
ctx = vault.assemble_context("database setup for Q3")
print(f"Context: {ctx.system_prompt[:200]}...")

# Run decay
report = vault.run_decay()
print(f"Strengthened: {report.strengthened}, Decayed: {report.decayed}")
```

## MCP Server

The package includes an MCP server for AI editors:

```bash
pip install engramd[mcp]
engramd-mcp
```

Or add to Claude Desktop's MCP config:

```json
{"engramd": {"command": "python", "args": ["-m", "engramd.mcp.server"]}}
```

## API Reference

See the full API surface in [docs/API_SURFACE.md](https://github.com/El-AI-Intelligence/Engram/blob/main/docs/API_SURFACE.md).
