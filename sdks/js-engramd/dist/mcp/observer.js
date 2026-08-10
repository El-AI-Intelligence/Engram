// ── Engram MCP auto-capture observer ────────────────────────────────────
// Wraps MCP tool handlers to passively capture agent activity as memories.
//
// Every non-engram tool call becomes a memory. The observer hashes tool
// name + args and skips duplicates within a rolling window to prevent
// redundant captures. engram_* tools pass through unchanged (explicit
// captures already produce memories).
//
// Config via env:
//   ENGRAM_AUTO_CAPTURE=true|false   (default: true)
//   ENGRAM_AUTO_CAPTURE_WINDOW=100   dedup hash window size
import { createHash } from "node:crypto";
// ── Config ────────────────────────────────────────────────────────────────
const AUTO_CAPTURE = (process.env.ENGRAM_AUTO_CAPTURE ?? "true") !== "false";
const MAX_WINDOW = parseInt(process.env.ENGRAM_AUTO_CAPTURE_WINDOW ?? "100", 10);
// ── Observer ──────────────────────────────────────────────────────────────
export class AutoCapture {
    vault;
    seen;
    maxWindow;
    enabled;
    constructor(vault, maxWindow = MAX_WINDOW) {
        this.vault = vault;
        this.seen = new Set();
        this.maxWindow = maxWindow;
        this.enabled = AUTO_CAPTURE;
    }
    // ── Public API ──────────────────────────────────────────────────────────
    /** Wrap an MCP tool handler so that non-engram calls are auto-captured. */
    wrapHandler(inner) {
        const self = this;
        return async function (request) {
            const { name, arguments: args } = request.params;
            // Run the real handler first — if it throws, no capture
            const result = await inner(request);
            // Auto-capture after the handler succeeds (fire-and-forget)
            if (self.enabled) {
                self.observe(name, args ?? {}).catch(() => {
                    // auto-capture failures are silent — never break the MCP loop
                });
            }
            return result;
        };
    }
    /** Observe a tool call and auto-capture if novel. */
    async observe(toolName, args) {
        // Disabled — bail early
        if (!this.enabled)
            return;
        // engram_* tools are explicit captures — don't double-capture
        if (toolName.startsWith("engram_"))
            return;
        // Hash the observation to check novelty
        const hash = this.contentHash(toolName, args);
        if (this.seen.has(hash))
            return;
        // Mark as seen (LRU eviction if needed)
        this.seen.add(hash);
        if (this.seen.size > this.maxWindow) {
            const oldest = this.seen.values().next().value;
            if (oldest)
                this.seen.delete(oldest);
        }
        // Build a human-readable summary of what happened
        const content = this.summarize(toolName, args);
        // Auto-capture as an observation
        await this.vault.capture(content, {
            source: "observation",
            layer: "episodic",
            scope: "moment",
            content_type: "text",
            tags: ["auto", toolName],
        });
    }
    // ── Internal helpers ────────────────────────────────────────────────────
    /** Stable content hash for dedup */
    contentHash(toolName, args) {
        const payload = JSON.stringify([toolName, args]);
        return createHash("sha256").update(payload).digest("hex");
    }
    /** Produce a one-line summary of the tool call for the memory content */
    summarize(toolName, args) {
        // Pick the most descriptive field for the summary
        const keys = Object.keys(args);
        if (toolName === "engram_assemble_context") {
            const query = String(args["query"] ?? "");
            return `Agent assembled context for query: ${query.slice(0, 200)}`;
        }
        // For tools with a "query" or "content" arg, use that
        if (keys.includes("query")) {
            const q = String(args["query"] ?? "");
            return `Agent called ${toolName}: ${q.slice(0, 200)}`;
        }
        if (keys.includes("content")) {
            const c = String(args["content"] ?? "");
            return `Agent called ${toolName}: ${c.slice(0, 200)}`;
        }
        // For tools with path/URL args
        if (keys.includes("path") || keys.includes("url")) {
            const loc = String(args["path"] ?? args["url"] ?? "");
            return `Agent called ${toolName} on ${loc.slice(0, 200)}`;
        }
        // Generic: list the arg keys so the memory isn't empty
        if (keys.length > 0) {
            return `Agent called ${toolName} with fields: ${keys.join(", ")}`;
        }
        return `Agent called ${toolName}`;
    }
    // ── Introspection (for testing) ─────────────────────────────────────────
    /** Number of hashes currently in the dedup window */
    get seenCount() {
        return this.seen.size;
    }
    /** Clear the dedup window (useful for testing) */
    reset() {
        this.seen.clear();
    }
}
//# sourceMappingURL=observer.js.map