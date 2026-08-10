export type MemoryLayer = "episodic" | "semantic" | "imagined";
export type MemoryScope = "moment" | "episode" | "narrative" | "rule";
export type ContentType = "text" | "frames" | "conversation" | "context";
export type MemorySource = "interaction" | "sensor" | "consolidation" | "chat" | "window" | "mic" | "agent" | "research" | "system" | "observation";
export type PrivacyLevel = "strict_local" | "hybrid" | "cloud_first" | "enterprise";
export type LinkType = "associative" | "causal" | "analogical" | "temporal";
export type EvidenceRelationship = "supports" | "contradicts" | "context_for";
export interface EngramLink {
    target_id: string;
    weight: number;
    link_type: LinkType;
}
export interface EvidenceRef {
    memory_id: string;
    relationship: EvidenceRelationship;
}
export interface Memory {
    id: string;
    layer: MemoryLayer;
    source: MemorySource;
    privacy_level: PrivacyLevel;
    content: string;
    context: Record<string, unknown>;
    strength: number;
    valence: number;
    retrievals: number;
    imagined: boolean;
    grounded: boolean;
    created_at: string;
    last_retrieved: string | null;
    project: string | null;
    tags: string[];
    links: EngramLink[];
    scope: MemoryScope;
    content_type: ContentType;
    occurred_at: string | null;
    evidence: EvidenceRef[];
}
export interface VaultHealth {
    status: string;
    version: string;
    vault: string;
    uptime_secs: number;
    memories_total: number;
    qem_hit_rate: number | null;
    db_size_bytes: number | null;
    qem_cache_entries?: number;
}
export interface Stats {
    total: number;
    by_layer: Record<string, number>;
    qem_hit_rate: number | null;
    db_size_bytes: number | null;
}
export interface ContextAssembly {
    messages: Record<string, unknown>[];
    token_count: number;
    engrams_retrieved: number;
    took_ms: number;
}
export interface ConsolidationResult {
    ok: boolean;
    strengthened?: number | null;
    decayed?: number | null;
    promoted?: number | null;
    pruned?: number | null;
    message?: string | null;
}
export interface TemporalPattern {
    found: boolean;
    description: string;
    peak_day?: string | null;
    peak_period?: string | null;
    day_strength?: number | null;
    period_strength?: number | null;
    sample_size?: number | null;
}
export interface MemoryVaultOptions {
    baseUrl?: string;
    apiKey?: string;
    timeout?: number;
}
export interface CaptureOptions {
    layer?: MemoryLayer;
    source?: MemorySource;
    context?: Record<string, unknown>;
    tags?: string[];
    valence?: number;
    privacy_level?: PrivacyLevel;
    project?: string | null;
    imagined?: boolean;
    scope?: MemoryScope;
    content_type?: ContentType;
    occurred_at?: string | null;
}
export interface SearchOptions {
    layer?: MemoryLayer;
    tags?: string[];
    limit?: number;
    offset?: number;
}
export interface UpdateOptions {
    content?: string;
    tags?: string[];
    valence?: number;
    layer?: MemoryLayer;
    project?: string | null;
    privacy_level?: PrivacyLevel;
    scope?: MemoryScope;
    content_type?: ContentType;
    occurred_at?: string | null;
}
export interface LinkOptions {
    weight?: number;
    link_type?: LinkType;
}
export interface AssembleContextOptions {
    token_budget?: number;
    max_engrams?: number;
    max_recent_turns?: number;
}
export interface ExportOptions {
    layer?: MemoryLayer;
    tags?: string[];
    limit?: number;
}
export interface ImportResult {
    imported: number;
    skipped: number;
}
export interface PatternOptions {
    min_engrams?: number;
}
//# sourceMappingURL=types.d.ts.map