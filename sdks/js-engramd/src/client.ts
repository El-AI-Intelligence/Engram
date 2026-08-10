// ── Engram Memory Vault — TypeScript client ─────────────────────────────
// Mirrors the Python MemoryVault client exactly (427-line client.py).

import {
  MemoryVaultOptions,
  CaptureOptions,
  SearchOptions,
  UpdateOptions,
  LinkOptions,
  AssembleContextOptions,
  ExportOptions,
  PatternOptions,
  Memory,
  VaultHealth,
  Stats,
  ContextAssembly,
  ConsolidationResult,
  TemporalPattern,
  ImportResult,
} from "./types.js";

const DEFAULT_BASE = process.env.ENGRAMD_URL ?? "http://localhost:8787";

// ── Error classes ───────────────────────────────────────────────────────

export class APIError extends Error {
  status: number;
  detail: string;

  constructor(status: number, detail: string) {
    super(`[${status}] ${detail}`);
    this.name = "APIError";
    this.status = status;
    this.detail = detail;
  }
}

export class ConnectionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConnectionError";
  }
}

// ── Client ──────────────────────────────────────────────────────────────

export class MemoryVault {
  readonly baseUrl: string;
  readonly apiKey: string | undefined;
  readonly timeout: number;

  constructor(opts: MemoryVaultOptions = {}) {
    this.baseUrl = (opts.baseUrl ?? DEFAULT_BASE).replace(/\/$/, "");
    this.apiKey = opts.apiKey;
    this.timeout = opts.timeout ?? 30;
  }

  // ── HTTP helpers ──────────────────────────────────────────────────────

  private async _req<T>(
    method: string,
    path: string,
    body?: unknown
  ): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = {};
    if (body !== undefined) {
      headers["Content-Type"] = "application/json";
    }
    if (this.apiKey) {
      headers["Authorization"] = `Bearer ${this.apiKey}`;
    }

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeout * 1000);

    let res: Response;
    try {
      res = await fetch(url, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });
    } catch (e: unknown) {
      clearTimeout(timer);
      if (e instanceof Error && e.name === "AbortError") {
        throw new ConnectionError("Request timed out");
      }
      throw new ConnectionError(
        e instanceof Error ? e.message : String(e)
      );
    }
    clearTimeout(timer);

    if (!res.ok) {
      let detail: string;
      try {
        const errBody = (await res.json()) as Record<string, unknown>;
        detail = (errBody.error as string) ?? JSON.stringify(errBody);
      } catch {
        detail = await res.text().catch(() => `HTTP ${res.status}`);
      }
      throw new APIError(res.status, detail);
    }

    const text = await res.text();
    if (!text) return undefined as T;
    return JSON.parse(text) as T;
  }

  private _get<T>(path: string): Promise<T> {
    return this._req<T>("GET", path);
  }

  private _post<T>(path: string, body: Record<string, unknown> = {}): Promise<T> {
    return this._req<T>("POST", path, body);
  }

  // ── Health ────────────────────────────────────────────────────────────

  async health(): Promise<VaultHealth> {
    const data = await this._get<Record<string, unknown>>("/health");
    return {
      status: data.status as string,
      version: (data.version as string) ?? "unknown",
      vault: (data.vault as string) ?? "",
      uptime_secs: (data.uptime_secs as number) ?? 0,
      memories_total: (data.memories_total as number) ?? 0,
      qem_hit_rate: (data.qem_hit_rate as number | null) ?? null,
      db_size_bytes: (data.db_size_bytes as number | null) ?? null,
    };
  }

  // ── Memories CRUD ─────────────────────────────────────────────────────

  async capture(content: string, opts: CaptureOptions = {}): Promise<Memory> {
    const body: Record<string, unknown> = {
      content,
      layer: opts.layer ?? "episodic",
      source: opts.source ?? "interaction",
      context: opts.context ?? {},
      tags: opts.tags ?? [],
      valence: opts.valence ?? 0.0,
      privacy_level: opts.privacy_level ?? "cloud_first",
      project: opts.project ?? null,
      imagined: opts.imagined ?? false,
      scope: opts.scope ?? "moment",
      content_type: opts.content_type ?? "text",
    };
    if (opts.occurred_at) {
      body.occurred_at = opts.occurred_at;
    }
    return this._post<Memory>("/memories", body);
  }

  async get(memoryId: string): Promise<Memory> {
    return this._get<Memory>(`/memories/${memoryId}`);
  }

  async search(
    query?: string | null,
    opts: SearchOptions = {}
  ): Promise<Memory[]> {
    const body: Record<string, unknown> = {
      limit: opts.limit ?? 20,
      offset: opts.offset ?? 0,
    };
    if (query) body.query = query;
    if (opts.layer) body.layer = opts.layer;
    if (opts.tags) body.tags = opts.tags;
    return this._post<Memory[]>("/memories/search", body);
  }

  async list(limit = 20, offset = 0): Promise<Memory[]> {
    return this.search(null, { limit, offset });
  }

  async update(memoryId: string, opts: UpdateOptions): Promise<Memory> {
    const body: Record<string, unknown> = {};
    if (opts.content !== undefined) body.content = opts.content;
    if (opts.tags !== undefined) body.tags = opts.tags;
    if (opts.valence !== undefined) body.valence = opts.valence;
    if (opts.layer !== undefined) body.layer = opts.layer;
    if (opts.project !== undefined) body.project = opts.project;
    if (opts.privacy_level !== undefined) body.privacy_level = opts.privacy_level;
    if (opts.scope !== undefined) body.scope = opts.scope;
    if (opts.content_type !== undefined) body.content_type = opts.content_type;
    if (opts.occurred_at !== undefined) body.occurred_at = opts.occurred_at;
    return this._req<Memory>("PATCH", `/memories/${memoryId}`, body);
  }

  async delete(memoryId: string): Promise<boolean> {
    await this._req("DELETE", `/memories/${memoryId}`);
    return true;
  }

  // ── Links ─────────────────────────────────────────────────────────────

  async link(
    sourceId: string,
    targetId: string,
    opts: LinkOptions = {}
  ): Promise<boolean> {
    await this._post("/memories/link", {
      source_id: sourceId,
      target_id: targetId,
      weight: opts.weight ?? 0.5,
      link_type: opts.link_type ?? "associative",
    });
    return true;
  }

  async getLinks(memoryId: string): Promise<import("./types.js").EngramLink[]> {
    return this._get(`/memories/${memoryId}/links`);
  }

  async getRelated(memoryId: string, limit = 10): Promise<Memory[]> {
    return this._get<Memory[]>(`/memories/${memoryId}/related?limit=${limit}`);
  }

  async ground(memoryId: string): Promise<Memory> {
    return this._post<Memory>(`/memories/${memoryId}/ground`);
  }

  // ── Context assembly ──────────────────────────────────────────────────

  async assembleContext(
    query: string,
    opts: AssembleContextOptions = {}
  ): Promise<ContextAssembly> {
    const data = await this._post<Record<string, unknown>>("/context/assemble", {
      query,
      token_budget: opts.token_budget ?? 8192,
      max_engrams: opts.max_engrams ?? 12,
      max_recent_turns: opts.max_recent_turns ?? 5,
    });
    return {
      messages: data.messages as Record<string, unknown>[],
      token_count: data.token_count as number,
      engrams_retrieved: data.engrams_retrieved as number,
      took_ms: data.took_ms as number,
    };
  }

  async *contextStream(
    sessionId = "default"
  ): AsyncGenerator<Record<string, unknown>> {
    const url = `${this.baseUrl}/context/stream?session_id=${sessionId}`;
    const headers: Record<string, string> = {};
    if (this.apiKey) {
      headers["Authorization"] = `Bearer ${this.apiKey}`;
    }

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeout * 1000);

    try {
      const res = await fetch(url, { headers, signal: controller.signal });
      if (!res.ok || !res.body) {
        throw new APIError(
          res.status,
          `context_stream failed: HTTP ${res.status}`
        );
      }

      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        clearTimeout(timer);

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";

        let eventType = "";
        let eventData = "";

        for (const line of lines) {
          if (line.startsWith("event: ")) {
            eventType = line.slice(7).trim();
          } else if (line.startsWith("data: ")) {
            eventData = line.slice(6).trim();
          } else if (line === "") {
            if (eventType === "done") return;
            if (eventData) {
              try {
                yield JSON.parse(eventData);
              } catch {
                // skip unparseable events
              }
            }
            eventType = "";
            eventData = "";
          }
        }
      }
    } finally {
      clearTimeout(timer);
    }
  }

  // ── Consolidation ─────────────────────────────────────────────────────

  async runDecay(): Promise<ConsolidationResult> {
    const data = await this._post<Record<string, unknown>>("/consolidate/decay");
    return {
      ok: (data.ok as boolean) ?? false,
      strengthened: data.strengthened as number | null | undefined,
      decayed: data.decayed as number | null | undefined,
      message: data.message as string | null | undefined,
    };
  }

  async runConsolidation(): Promise<ConsolidationResult> {
    const data = await this._post<Record<string, unknown>>("/consolidate/weekly");
    return {
      ok: (data.ok as boolean) ?? false,
      promoted: data.promoted as number | null | undefined,
      pruned: data.pruned as number | null | undefined,
      message: data.message as string | null | undefined,
    };
  }

  async consolidationHistory(): Promise<Record<string, unknown>[]> {
    return this._get("/consolidate/history");
  }

  // ── Analytics ─────────────────────────────────────────────────────────

  async stats(): Promise<Stats> {
    const data = await this._get<Record<string, unknown>>("/analytics/stats");
    return {
      total: data.total as number,
      by_layer: (data.by_layer as Record<string, number>) ?? {},
      qem_hit_rate: (data.qem_hit_rate as number | null) ?? null,
      db_size_bytes: (data.db_size_bytes as number | null) ?? null,
    };
  }

  async detectPatterns(
    query = "",
    opts: PatternOptions = {}
  ): Promise<TemporalPattern> {
    const data = await this._post<Record<string, unknown>>("/analytics/patterns", {
      query,
      min_engrams: opts.min_engrams ?? 5,
    });
    return {
      found: (data.found as boolean) ?? false,
      description: (data.description as string) ?? "",
      peak_day: data.peak_day as string | null | undefined,
      peak_period: data.peak_period as string | null | undefined,
      day_strength: data.day_strength as number | null | undefined,
      period_strength: data.period_strength as number | null | undefined,
      sample_size: data.sample_size as number | null | undefined,
    };
  }

  // ── Export / Import ───────────────────────────────────────────────────

  async export(opts: ExportOptions = {}): Promise<Record<string, unknown>> {
    const body: Record<string, unknown> = { limit: opts.limit ?? 10_000 };
    if (opts.layer) body.layer = opts.layer;
    if (opts.tags) body.tags = opts.tags;
    return this._post("/export", body);
  }

  async importMemories(memories: Record<string, unknown>[]): Promise<ImportResult> {
    return this._post<ImportResult>("/import", { memories });
  }

  // ── Config ────────────────────────────────────────────────────────────

  async getConfig(): Promise<Record<string, unknown>> {
    return this._get("/config");
  }

  async updateConfig(
    config: Record<string, unknown>
  ): Promise<Record<string, unknown>> {
    return this._req("PATCH", "/config", config);
  }
}
