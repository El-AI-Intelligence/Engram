import { MemoryVaultOptions, CaptureOptions, SearchOptions, UpdateOptions, LinkOptions, AssembleContextOptions, ExportOptions, PatternOptions, Memory, VaultHealth, Stats, ContextAssembly, ConsolidationResult, TemporalPattern, ImportResult } from "./types.js";
export declare class APIError extends Error {
    status: number;
    detail: string;
    constructor(status: number, detail: string);
}
export declare class ConnectionError extends Error {
    constructor(message: string);
}
export declare class MemoryVault {
    readonly baseUrl: string;
    readonly apiKey: string | undefined;
    readonly timeout: number;
    constructor(opts?: MemoryVaultOptions);
    private _req;
    private _get;
    private _post;
    health(): Promise<VaultHealth>;
    capture(content: string, opts?: CaptureOptions): Promise<Memory>;
    get(memoryId: string): Promise<Memory>;
    search(query?: string | null, opts?: SearchOptions): Promise<Memory[]>;
    list(limit?: number, offset?: number): Promise<Memory[]>;
    update(memoryId: string, opts: UpdateOptions): Promise<Memory>;
    delete(memoryId: string): Promise<boolean>;
    link(sourceId: string, targetId: string, opts?: LinkOptions): Promise<boolean>;
    getLinks(memoryId: string): Promise<import("./types.js").EngramLink[]>;
    getRelated(memoryId: string, limit?: number): Promise<Memory[]>;
    ground(memoryId: string): Promise<Memory>;
    assembleContext(query: string, opts?: AssembleContextOptions): Promise<ContextAssembly>;
    contextStream(sessionId?: string): AsyncGenerator<Record<string, unknown>>;
    runDecay(): Promise<ConsolidationResult>;
    runConsolidation(): Promise<ConsolidationResult>;
    consolidationHistory(): Promise<Record<string, unknown>[]>;
    stats(): Promise<Stats>;
    detectPatterns(query?: string, opts?: PatternOptions): Promise<TemporalPattern>;
    export(opts?: ExportOptions): Promise<Record<string, unknown>>;
    importMemories(memories: Record<string, unknown>[]): Promise<ImportResult>;
    getConfig(): Promise<Record<string, unknown>>;
    updateConfig(config: Record<string, unknown>): Promise<Record<string, unknown>>;
}
//# sourceMappingURL=client.d.ts.map