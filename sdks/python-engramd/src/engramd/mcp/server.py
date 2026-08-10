#!/usr/bin/env python3
"""Engram Memory Vault — MCP server.

Exposes the vault through the Model Context Protocol so AI agents can:
- Capture memories
- Retrieve memories
- Search by content, tags, layer
- Assemble context windows
- Run consolidation

Usage:
    pip install mcp
    python -m engramd.mcp.server

Or point to a remote vault:
    ENGRAMD_URL=https://engram.ellmstack.dev python -m engramd.mcp.server
"""

import json
import os
import sys
from typing import Any

from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

from ..client import DEFAULT_BASE, APIError, ConnectionError, MemoryVault
from .observer import AutoCapture


vault = MemoryVault(base_url=os.environ.get("ENGRAMD_URL", DEFAULT_BASE))
observer = AutoCapture(vault)
server = Server("engramd-mcp")


def _json(obj: Any) -> str:
    return json.dumps(obj, indent=2, default=str)


# ── Tool definitions ────────────────────────────────────────────────────────

@server.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="engram_capture",
            description="Capture a memory into the vault. Returns the stored memory with its ID.",
            inputSchema={
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "The memory content to store",
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["episodic", "semantic", "imagined"],
                        "description": "Memory layer: episodic (what happened), semantic (what was learned), imagined (AI-generated)",
                        "default": "episodic",
                    },
                    "source": {
                        "type": "string",
                        "description": "Source of the memory: interaction, chat, window, agent, system, consolidation, imagined",
                        "default": "interaction",
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Tags for categorization and retrieval",
                    },
                    "valence": {
                        "type": "number",
                        "minimum": -1.0,
                        "maximum": 1.0,
                        "description": "Emotional valence (-1 negative, 0 neutral, 1 positive)",
                        "default": 0.0,
                    },
                    "project": {
                        "type": "string",
                        "description": "Project or context identifier",
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["moment", "episode", "narrative", "rule"],
                        "description": "Temporal scope: moment (single observation), episode (session), narrative (storyline), rule (crystallized)",
                        "default": "moment",
                    },
                    "content_type": {
                        "type": "string",
                        "enum": ["text", "frames", "conversation", "context"],
                        "description": "Type of content: text, frames (reasoning traces), conversation (multi-turn), context (environment)",
                        "default": "text",
                    },
                    "occurred_at": {
                        "type": "string",
                        "description": "ISO 8601 timestamp of when the event actually happened (may differ from capture time)",
                    },
                    "privacy_level": {
                        "type": "string",
                        "enum": ["strict_local", "hybrid", "cloud_first", "enterprise"],
                        "description": "Privacy level: strict_local (never leaves device), hybrid (local models), cloud_first (default), enterprise",
                        "default": "cloud_first",
                    },
                    "imagined": {
                        "type": "boolean",
                        "description": "Whether this is an AI-generated/imagined memory (quarantine applies)",
                        "default": False,
                    },
                },
                "required": ["content"],
            },
        ),
        Tool(
            name="engram_search",
            description="Search memories by content, layer, or tags. Returns matching memories ordered by strength.",
            inputSchema={
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Free-text search query",
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["episodic", "semantic", "imagined"],
                        "description": "Filter by memory layer",
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter by tags (AND match)",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results to return",
                        "default": 10,
                    },
                },
            },
        ),
        Tool(
            name="engram_get",
            description="Retrieve a specific memory by its ID, including its links.",
            inputSchema={
                "type": "object",
                "properties": {
                    "memory_id": {
                        "type": "string",
                        "description": "The memory ID (e.g., eng_abc123def456)",
                    },
                },
                "required": ["memory_id"],
            },
        ),
        Tool(
            name="engram_link",
            description="Create a link between two memories.",
            inputSchema={
                "type": "object",
                "properties": {
                    "source_id": {
                        "type": "string",
                        "description": "Source memory ID",
                    },
                    "target_id": {
                        "type": "string",
                        "description": "Target memory ID",
                    },
                    "weight": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Link strength (0-1)",
                        "default": 0.5,
                    },
                    "link_type": {
                        "type": "string",
                        "enum": ["associative", "causal", "analogical", "temporal"],
                        "description": "Type of relationship",
                        "default": "associative",
                    },
                },
                "required": ["source_id", "target_id"],
            },
        ),
        Tool(
            name="engram_assemble_context",
            description="Assemble a context window of relevant memories for a query. Returns OpenAI-format messages with retrieved memories in the system prompt.",
            inputSchema={
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The user's current query to retrieve context for",
                    },
                    "token_budget": {
                        "type": "integer",
                        "description": "Maximum token budget for the context window",
                        "default": 8192,
                    },
                    "max_engrams": {
                        "type": "integer",
                        "description": "Maximum memories to retrieve",
                        "default": 12,
                    },
                },
                "required": ["query"],
            },
        ),
        Tool(
            name="engram_health",
            description="Check the vault server health and stats.",
            inputSchema={
                "type": "object",
                "properties": {},
            },
        ),
        Tool(
            name="engram_decay",
            description="Run daily hygiene: apply Ebbinghaus decay and Hebbian strengthening.",
            inputSchema={
                "type": "object",
                "properties": {},
            },
        ),
    ]


# ── Tool handlers ───────────────────────────────────────────────────────────

@server.call_tool()
async def call_tool(name: str, arguments: dict[str, Any]) -> list[TextContent]:
    try:
        if name == "engram_capture":
            mem = vault.capture(
                content=arguments["content"],
                layer=arguments.get("layer", "episodic"),
                source=arguments.get("source", "interaction"),
                tags=arguments.get("tags", []),
                valence=arguments.get("valence", 0.0),
                project=arguments.get("project"),
                scope=arguments.get("scope", "moment"),
                content_type=arguments.get("content_type", "text"),
                occurred_at=arguments.get("occurred_at"),
                privacy_level=arguments.get("privacy_level", "cloud_first"),
                imagined=arguments.get("imagined", False),
            )
            result = [TextContent(type="text", text=f"Memory captured: {mem.id}\n{_json(mem)}")]

        elif name == "engram_search":
            results = vault.search(
                query=arguments.get("query"),
                layer=arguments.get("layer"),
                tags=arguments.get("tags"),
                limit=arguments.get("limit", 10),
            )
            text = f"Found {len(results)} memories:\n\n"
            for m in results:
                text += f"### {m.id} [{m.layer}] strength={m.strength:.2f}\n{m.content[:200]}\n\n"
            result = [TextContent(type="text", text=text)]

        elif name == "engram_get":
            mem = vault.get(arguments["memory_id"])
            result = [TextContent(type="text", text=_json(mem))]

        elif name == "engram_link":
            vault.link(
                source_id=arguments["source_id"],
                target_id=arguments["target_id"],
                weight=arguments.get("weight", 0.5),
                link_type=arguments.get("link_type", "associative"),
            )
            result = [TextContent(type="text", text=f"Linked {arguments['source_id']} → {arguments['target_id']}")]

        elif name == "engram_assemble_context":
            ctx = vault.assemble_context(
                query=arguments["query"],
                token_budget=arguments.get("token_budget", 8192),
                max_engrams=arguments.get("max_engrams", 12),
            )
            result = [TextContent(type="text", text=_json(ctx))]

        elif name == "engram_health":
            h = vault.health()
            result = [TextContent(type="text", text=_json(h))]

        elif name == "engram_decay":
            r = vault.run_decay()
            result = [TextContent(type="text", text=f"Decay complete: {r.message}")]

        else:
            result = [TextContent(type="text", text=f"Unknown tool: {name}")]

        # Auto-capture non-engram tool calls as passive memories.
        # Fire-and-forget — capture failures must never break the MCP loop.
        if not name.startswith("engram_"):
            try:
                observer.observe(name, arguments)
            except Exception:
                pass

        return result

    except APIError as e:
        return [TextContent(type="text", text=f"API error [{e.status}]: {e.detail}")]
    except ConnectionError as e:
        return [TextContent(type="text", text=f"Connection error: {e}")]
    except Exception as e:
        return [TextContent(type="text", text=f"Error: {e}")]


# ── Entry point ─────────────────────────────────────────────────────────────

def main():
    """Run the MCP server via stdio."""
    import asyncio

    async def run():
        async with stdio_server() as (read, write):
            await server.run(read, write, server.create_initialization_options())

    asyncio.run(run())


if __name__ == "__main__":
    main()
