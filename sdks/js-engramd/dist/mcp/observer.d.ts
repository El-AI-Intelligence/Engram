import type { MemoryVault } from "../client.js";
/** Shape of an MCP tool-call request params */
export interface ToolCallParams {
    name: string;
    arguments?: Record<string, unknown>;
}
/** The raw MCP request as it arrives at CallToolRequestSchema */
export interface MCPCallRequest {
    params: ToolCallParams;
}
/** Handler signature — what the MCP server's setRequestHandler callback receives */
export type ToolCallResult = {
    content: Array<{
        type: string;
        text: string;
    }>;
};
export type ToolHandler = (request: MCPCallRequest) => Promise<ToolCallResult>;
export declare class AutoCapture {
    private vault;
    private seen;
    private maxWindow;
    private enabled;
    constructor(vault: MemoryVault, maxWindow?: number);
    /** Wrap an MCP tool handler so that non-engram calls are auto-captured. */
    wrapHandler(inner: ToolHandler): ToolHandler;
    /** Observe a tool call and auto-capture if novel. */
    observe(toolName: string, args: Record<string, unknown>): Promise<void>;
    /** Stable content hash for dedup */
    private contentHash;
    /** Produce a one-line summary of the tool call for the memory content */
    private summarize;
    /** Number of hashes currently in the dedup window */
    get seenCount(): number;
    /** Clear the dedup window (useful for testing) */
    reset(): void;
}
//# sourceMappingURL=observer.d.ts.map