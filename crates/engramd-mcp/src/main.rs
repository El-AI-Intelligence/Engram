//! Engram MCP Server — Model Context Protocol interface for AI assistants.
//!
//! This server implements the MCP JSON-RPC 2.0 protocol over stdio, allowing
//! Claude Desktop, Cursor, and other MCP-compatible AI tools to interact with
//! a local Engram memory vault.
//!
//! ## Protocol
//! - Reads JSON-RPC requests from stdin (one per line)
//! - Writes JSON-RPC responses to stdout (one per line)
//! - Errors and logging go to stderr
//!
//! ## Usage
//! ```bash
//! engramd-mcp --engramd-url http://localhost:8787
//! ```
//!
//! ## Claude Desktop config
//! ```json
//! {
//!   "mcpServers": {
//!     "engram": {
//!       "command": "engramd-mcp",
//!       "args": ["--engramd-url", "http://localhost:8787"]
//!     }
//!   }
//! }
//! ```

use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// MCP server configuration.
#[derive(Parser, Debug)]
#[command(name = "engramd-mcp", version, about = "MCP server for Engram memory vault")]
struct Cli {
    /// Engramd API base URL
    #[arg(long, default_value = "http://127.0.0.1:8787", env = "ENGRAMD_URL")]
    engramd_url: String,
}

// ── JSON-RPC 2.0 types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

const JSONRPC_VERSION: &str = "2.0";
const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "engram-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn ok(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: Option<Value>, code: i32, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
        }),
    }
}

// ── Tool definitions ──────────────────────────────────────────────────────

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "engram_search",
                "description": "Search your Engram memory vault. Returns memories matching the query with relevance scores. Use this to recall past decisions, bugs, decisions, and context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query (natural language or keywords)" },
                        "limit": { "type": "integer", "description": "Max results to return (default: 10, max: 50)", "default": 10 },
                        "layer": { "type": "string", "enum": ["episodic", "semantic", "imagined"], "description": "Filter by memory layer" }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "engram_capture",
                "description": "Capture a new memory into your Engram vault. Use this to remember important facts, decisions, bugs, or context for future sessions.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "The content to remember" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags for categorization" },
                        "source": { "type": "string", "description": "Source of the memory (e.g., \"claude\", \"cursor\")" },
                        "project": { "type": "string", "description": "Project name to group memories" },
                        "layer": { "type": "string", "enum": ["episodic", "semantic", "imagined"], "description": "Memory layer (default: episodic)"}
                    },
                    "required": ["content"]
                }
            },
            {
                "name": "engram_get",
                "description": "Retrieve a specific memory by its ID. Use this to read the full details of a memory found via search.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "The memory ID (e.g., \"mem_abc123\")" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "engram_context",
                "description": "Assemble a context window from your memory vault. Returns the most relevant memories for a given task or query, ready to inject into an LLM context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What you're working on — used to find relevant memories" },
                        "budget": { "type": "integer", "description": "Max tokens for the assembled context (default: 8192)" }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "engram_health",
                "description": "Check the health and status of your Engram memory vault. Returns vault stats, uptime, and connection status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "engram_decay",
                "description": "Trigger memory hygiene (decay and strengthening) on your vault. Useful after a long session to let Engram determine which memories are important.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

// ── Tool handlers ─────────────────────────────────────────────────────────

struct McpServer {
    http: reqwest::Client,
    engramd_url: String,
}

impl McpServer {
    fn new(engramd_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            engramd_url: engramd_url.trim_end_matches('/').to_string(),
        }
    }

    async fn call_tool(&self, tool_name: &str, args: &Value) -> Result<Value, String> {
        match tool_name {
            "engram_search" => self.search(args).await,
            "engram_capture" => self.capture(args).await,
            "engram_get" => self.get(args).await,
            "engram_context" => self.context(args).await,
            "engram_health" => self.health().await,
            "engram_decay" => self.decay().await,
            _ => Err(format!("Unknown tool: {tool_name}")),
        }
    }

    async fn search(&self, args: &Value) -> Result<Value, String> {
        let query = get_str(args, "query")?;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10).min(50);

        let body = json!({
            "query": query,
            "limit": limit,
            "search_mode": "fts5",
            "min_strength": 0.0,
        });

        let resp = self
            .http
            .post(format!("{}/memories/search", self.engramd_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("engramd unreachable: {e}"))?
            .json::<Value>()
            .await
            .map_err(|e| format!("parse error: {e}"))?;

        // Format results for LLM consumption
        let results = resp.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let formatted: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "id": r.get("id"),
                    "content": r.get("content"),
                    "layer": r.get("layer"),
                    "tags": r.get("tags"),
                    "strength": r.get("strength"),
                    "created_at": r.get("created_at"),
                    "project": r.get("project"),
                })
            })
            .collect();

        Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&json!({
                    "count": formatted.len(),
                    "results": formatted,
                })).unwrap_or_else(|_| "{}".into())
            }]
        }))
    }

    async fn capture(&self, args: &Value) -> Result<Value, String> {
        let content = get_str(args, "content")?;
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("claude");
        let project = args.get("project").and_then(|v| v.as_str());
        let layer = args.get("layer").and_then(|v| v.as_str()).unwrap_or("episodic");

        let mut body = json!({
            "content": content,
            "tags": tags,
            "source": source,
            "layer": layer,
        });
        if let Some(p) = project {
            body["project"] = json!(p);
        }

        let resp = self
            .http
            .post(format!("{}/memories", self.engramd_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("engramd unreachable: {e}"))?
            .json::<Value>()
            .await
            .map_err(|e| format!("parse error: {e}"))?;

        let id = resp.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");

        Ok(json!({
            "content": [{
                "type": "text",
                "text": format!("Memory captured successfully. ID: {}", id)
            }]
        }))
    }

    async fn get(&self, args: &Value) -> Result<Value, String> {
        let id = get_str(args, "id")?;

        let resp = self
            .http
            .get(format!("{}/memories/{}", self.engramd_url, id))
            .send()
            .await
            .map_err(|e| format!("engramd unreachable: {e}"))?;

        if resp.status() == 404 {
            return Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Memory not found: {}", id)
                }]
            }));
        }

        let memory = resp
            .json::<Value>()
            .await
            .map_err(|e| format!("parse error: {e}"))?;

        Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&memory).unwrap_or_else(|_| "{}".into())
            }]
        }))
    }

    async fn context(&self, args: &Value) -> Result<Value, String> {
        let query = get_str(args, "query")?;
        let budget = args.get("budget").and_then(|v| v.as_u64()).unwrap_or(8192);

        let body = json!({
            "query": query,
            "budget": budget,
            "dimensions": ["file_aware", "error_aware"],
            "use_vector": false,
        });

        let resp = self
            .http
            .post(format!("{}/context/assemble", self.engramd_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("engramd unreachable: {e}"))?;

        let result = resp
            .json::<Value>()
            .await
            .map_err(|e| format!("parse error: {e}"))?;

        // Extract the assembled messages for LLM injection
        let messages = result
            .get("assembled")
            .and_then(|a| a.get("messages"));

        Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(messages.unwrap_or(&json!([])))
                    .unwrap_or_else(|_| "[]".into())
            }]
        }))
    }

    async fn health(&self) -> Result<Value, String> {
        let resp = self
            .http
            .get(format!("{}/health", self.engramd_url))
            .send()
            .await
            .map_err(|e| format!("engramd unreachable: {e}"))?;

        let health = resp
            .json::<Value>()
            .await
            .unwrap_or(json!({"error": "could not parse health response"}));

        Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&health).unwrap_or_else(|_| "{}".into())
            }]
        }))
    }

    async fn decay(&self) -> Result<Value, String> {
        let resp = self
            .http
            .post(format!("{}/consolidate/decay", self.engramd_url))
            .json(&json!({"mode": "decay_only"}))
            .send()
            .await
            .map_err(|e| format!("engramd unreachable: {e}"))?;

        let result = resp
            .json::<Value>()
            .await
            .unwrap_or(json!({"status": "ok"}));

        let strengthened = result.get("strengthened").and_then(|v| v.as_u64()).unwrap_or(0);
        let decayed = result.get("decayed").and_then(|v| v.as_u64()).unwrap_or(0);

        Ok(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Memory hygiene complete. Strengthened: {}, Decayed: {}.",
                    strengthened, decayed
                )
            }]
        }))
    }
}

fn get_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing required parameter: {}", key))
}

// ── Main loop ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    eprintln!(
        "Engram MCP server v{} starting (engramd: {})",
        SERVER_VERSION, cli.engramd_url
    );

    let server = McpServer::new(cli.engramd_url);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("stdin read error: {e}");
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("JSON parse error: {e}");
                let resp = err(None, -32700, &format!("Parse error: {e}"));
                emit(&stdout, &resp);
                continue;
            }
        };

        let response = match request.method.as_str() {
            "initialize" => {
                let client_name = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("clientInfo"))
                    .and_then(|c| c.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                eprintln!("MCP client connected: {}", client_name);
                ok(
                    request.id,
                    json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {
                            "tools": {}
                        },
                        "serverInfo": {
                            "name": SERVER_NAME,
                            "version": SERVER_VERSION,
                        }
                    }),
                )
            }
            "tools/list" => ok(request.id, tools_list()),
            "tools/call" => {
                let params = match request.params {
                    Some(p) => p,
                    None => {
                        emit(&stdout, &err(request.id, -32602, "Missing params"));
                        continue;
                    }
                };
                let tool_name = match params.get("name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => {
                        emit(&stdout, &err(request.id, -32602, "Missing tool name"));
                        continue;
                    }
                };
                let args = params.get("arguments").cloned().unwrap_or(json!({}));

                match server.call_tool(tool_name, &args).await {
                    Ok(result) => ok(request.id, result),
                    Err(e) => {
                        // Return error as tool result content (MCP convention)
                        ok(
                            request.id,
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Error: {}", e)
                                }],
                                "isError": true,
                            }),
                        )
                    }
                }
            }
            "notifications/initialized" => {
                // No response needed for notifications
                continue;
            }
            "ping" => ok(request.id, json!({})),
            _ => err(
                request.id,
                -32601,
                &format!("Method not found: {}", request.method),
            ),
        };

        emit(&stdout, &response);
    }

    eprintln!("Engram MCP server shutting down.");
}

fn emit(stdout: &std::io::Stdout, response: &JsonRpcResponse) {
    let mut out = stdout.lock();
    let json = serde_json::to_string(response).unwrap_or_else(|_| {
        json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {"code": -32603, "message": "Internal error — failed to serialize response"}
        })
        .to_string()
    });
    if let Err(e) = writeln!(out, "{}", json) {
        // If stdout is broken, write to stderr so it appears in MCP logs
        eprintln!("stdout write error: {e}");
    }
    // Flush every message so the MCP client receives it immediately
    let _ = out.flush();
}
