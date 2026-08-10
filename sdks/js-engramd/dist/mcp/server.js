#!/usr/bin/env node
// ── Engram Memory Vault — MCP server ────────────────────────────────────
// Exposes 7 tools over stdio using the Model Context Protocol.
//
// Usage:
//   ENGRAMD_URL=http://localhost:8787 npx engramd-mcp
//   node dist/mcp/server.js
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema, } from "@modelcontextprotocol/sdk/types.js";
import { MemoryVault, APIError, ConnectionError } from "../client.js";
function str(args, key, def = "") {
    return args?.[key] ?? def;
}
function num(args, key, def = 0) {
    return args?.[key] ?? def;
}
function bool(args, key, def = false) {
    return args?.[key] ?? def;
}
function arr(args, key) {
    return args?.[key] ?? [];
}
const vault = new MemoryVault({
    baseUrl: process.env.ENGRAMD_URL ?? "http://localhost:8787",
});
const server = new Server({ name: "engramd-mcp", version: "0.1.0" }, { capabilities: { tools: {} } });
// ── Tool definitions ────────────────────────────────────────────────────
server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [
        {
            name: "engram_capture",
            description: "Capture a memory into the vault. Returns the stored memory with its ID.",
            inputSchema: {
                type: "object",
                properties: {
                    content: {
                        type: "string",
                        description: "The memory content to store",
                    },
                    layer: {
                        type: "string",
                        enum: ["episodic", "semantic", "imagined"],
                        description: "Memory layer: episodic (what happened), semantic (what was learned), imagined (AI-generated)",
                        default: "episodic",
                    },
                    source: {
                        type: "string",
                        description: "Source of the memory: interaction, chat, window, agent, system, consolidation, imagined",
                        default: "interaction",
                    },
                    tags: {
                        type: "array",
                        items: { type: "string" },
                        description: "Tags for categorization and retrieval",
                    },
                    valence: {
                        type: "number",
                        minimum: -1.0,
                        maximum: 1.0,
                        description: "Emotional valence (-1 negative, 0 neutral, 1 positive)",
                        default: 0.0,
                    },
                    project: {
                        type: "string",
                        description: "Project or context identifier",
                    },
                    scope: {
                        type: "string",
                        enum: ["moment", "episode", "narrative", "rule"],
                        description: "Temporal scope: moment (single observation), episode (session), narrative (storyline), rule (crystallized)",
                        default: "moment",
                    },
                    content_type: {
                        type: "string",
                        enum: ["text", "frames", "conversation", "context"],
                        description: "Type of content: text, frames (reasoning traces), conversation (multi-turn), context (environment)",
                        default: "text",
                    },
                    occurred_at: {
                        type: "string",
                        description: "ISO 8601 timestamp of when the event actually happened (may differ from capture time)",
                    },
                    privacy_level: {
                        type: "string",
                        enum: ["strict_local", "hybrid", "cloud_first", "enterprise"],
                        description: "Privacy level: strict_local (never leaves device), hybrid (local models), cloud_first (default), enterprise",
                        default: "cloud_first",
                    },
                    imagined: {
                        type: "boolean",
                        description: "Whether this is an AI-generated/imagined memory (quarantine applies)",
                        default: false,
                    },
                },
                required: ["content"],
            },
        },
        {
            name: "engram_search",
            description: "Search memories by content, layer, or tags. Returns matching memories ordered by strength.",
            inputSchema: {
                type: "object",
                properties: {
                    query: {
                        type: "string",
                        description: "Free-text search query",
                    },
                    layer: {
                        type: "string",
                        enum: ["episodic", "semantic", "imagined"],
                        description: "Filter by memory layer",
                    },
                    tags: {
                        type: "array",
                        items: { type: "string" },
                        description: "Filter by tags (AND match)",
                    },
                    limit: {
                        type: "integer",
                        description: "Max results to return",
                        default: 10,
                    },
                },
            },
        },
        {
            name: "engram_get",
            description: "Retrieve a specific memory by its ID, including its links.",
            inputSchema: {
                type: "object",
                properties: {
                    memory_id: {
                        type: "string",
                        description: "The memory ID (e.g., eng_abc123def456)",
                    },
                },
                required: ["memory_id"],
            },
        },
        {
            name: "engram_link",
            description: "Create a link between two memories.",
            inputSchema: {
                type: "object",
                properties: {
                    source_id: {
                        type: "string",
                        description: "Source memory ID",
                    },
                    target_id: {
                        type: "string",
                        description: "Target memory ID",
                    },
                    weight: {
                        type: "number",
                        minimum: 0.0,
                        maximum: 1.0,
                        description: "Link strength (0-1)",
                        default: 0.5,
                    },
                    link_type: {
                        type: "string",
                        enum: ["associative", "causal", "analogical", "temporal"],
                        description: "Type of relationship",
                        default: "associative",
                    },
                },
                required: ["source_id", "target_id"],
            },
        },
        {
            name: "engram_assemble_context",
            description: "Assemble a context window of relevant memories for a query. Returns OpenAI-format messages with retrieved memories in the system prompt.",
            inputSchema: {
                type: "object",
                properties: {
                    query: {
                        type: "string",
                        description: "The user's current query to retrieve context for",
                    },
                    token_budget: {
                        type: "integer",
                        description: "Maximum token budget for the context window",
                        default: 8192,
                    },
                    max_engrams: {
                        type: "integer",
                        description: "Maximum memories to retrieve",
                        default: 12,
                    },
                },
                required: ["query"],
            },
        },
        {
            name: "engram_health",
            description: "Check the vault server health and stats.",
            inputSchema: {
                type: "object",
                properties: {},
            },
        },
        {
            name: "engram_decay",
            description: "Run daily hygiene: apply Ebbinghaus decay and Hebbian strengthening.",
            inputSchema: {
                type: "object",
                properties: {},
            },
        },
    ],
}));
// ── Tool handlers ───────────────────────────────────────────────────────
server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const { name, arguments: args } = request.params;
    try {
        switch (name) {
            case "engram_capture": {
                const mem = await vault.capture(str(args, "content"), {
                    layer: str(args, "layer", "episodic"),
                    source: str(args, "source", "interaction"),
                    tags: arr(args, "tags"),
                    valence: num(args, "valence", 0),
                    project: args?.project ?? undefined,
                    scope: str(args, "scope", "moment"),
                    content_type: str(args, "content_type", "text"),
                    occurred_at: args?.occurred_at ?? undefined,
                    privacy_level: str(args, "privacy_level", "cloud_first"),
                    imagined: bool(args, "imagined"),
                });
                return {
                    content: [
                        {
                            type: "text",
                            text: `Memory captured: ${mem.id}\n${JSON.stringify(mem, null, 2)}`,
                        },
                    ],
                };
            }
            case "engram_search": {
                const results = await vault.search(args?.query || null, {
                    layer: args?.layer ?? undefined,
                    tags: args?.tags ?? undefined,
                    limit: num(args, "limit", 10),
                });
                let text = `Found ${results.length} memories:\n\n`;
                for (const m of results) {
                    text += `### ${m.id} [${m.layer}] strength=${m.strength.toFixed(2)}\n${m.content.slice(0, 200)}\n\n`;
                }
                return { content: [{ type: "text", text }] };
            }
            case "engram_get": {
                const mem = await vault.get(str(args, "memory_id"));
                return {
                    content: [
                        { type: "text", text: JSON.stringify(mem, null, 2) },
                    ],
                };
            }
            case "engram_link": {
                await vault.link(str(args, "source_id"), str(args, "target_id"), {
                    weight: num(args, "weight", 0.5),
                    link_type: str(args, "link_type", "associative"),
                });
                return {
                    content: [
                        {
                            type: "text",
                            text: `Linked ${str(args, "source_id")} → ${str(args, "target_id")}`,
                        },
                    ],
                };
            }
            case "engram_assemble_context": {
                const ctx = await vault.assembleContext(str(args, "query"), {
                    token_budget: num(args, "token_budget", 8192),
                    max_engrams: num(args, "max_engrams", 12),
                });
                return {
                    content: [
                        { type: "text", text: JSON.stringify(ctx, null, 2) },
                    ],
                };
            }
            case "engram_health": {
                const h = await vault.health();
                return {
                    content: [
                        { type: "text", text: JSON.stringify(h, null, 2) },
                    ],
                };
            }
            case "engram_decay": {
                const r = await vault.runDecay();
                return {
                    content: [
                        { type: "text", text: `Decay complete: ${r.message}` },
                    ],
                };
            }
            default:
                return {
                    content: [{ type: "text", text: `Unknown tool: ${name}` }],
                };
        }
    }
    catch (e) {
        if (e instanceof APIError) {
            return {
                content: [
                    { type: "text", text: `API error [${e.status}]: ${e.detail}` },
                ],
            };
        }
        if (e instanceof ConnectionError) {
            return {
                content: [
                    { type: "text", text: `Connection error: ${e.message}` },
                ],
            };
        }
        return {
            content: [
                {
                    type: "text",
                    text: `Error: ${e instanceof Error ? e.message : String(e)}`,
                },
            ],
        };
    }
});
// ── Entry point ─────────────────────────────────────────────────────────
async function main() {
    const transport = new StdioServerTransport();
    await server.connect(transport);
}
main().catch((e) => {
    console.error("engramd-mcp fatal:", e);
    process.exit(1);
});
//# sourceMappingURL=server.js.map