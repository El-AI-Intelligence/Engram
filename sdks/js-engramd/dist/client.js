// ── Engram Memory Vault — TypeScript client ─────────────────────────────
// Mirrors the Python MemoryVault client exactly (427-line client.py).
const DEFAULT_BASE = process.env.ENGRAMD_URL ?? "http://localhost:8787";
// ── Error classes ───────────────────────────────────────────────────────
export class APIError extends Error {
    status;
    detail;
    constructor(status, detail) {
        super(`[${status}] ${detail}`);
        this.name = "APIError";
        this.status = status;
        this.detail = detail;
    }
}
export class ConnectionError extends Error {
    constructor(message) {
        super(message);
        this.name = "ConnectionError";
    }
}
// ── Client ──────────────────────────────────────────────────────────────
export class MemoryVault {
    baseUrl;
    apiKey;
    timeout;
    constructor(opts = {}) {
        this.baseUrl = (opts.baseUrl ?? DEFAULT_BASE).replace(/\/$/, "");
        this.apiKey = opts.apiKey;
        this.timeout = opts.timeout ?? 30;
    }
    // ── HTTP helpers ──────────────────────────────────────────────────────
    async _req(method, path, body) {
        const url = `${this.baseUrl}${path}`;
        const headers = {};
        if (body !== undefined) {
            headers["Content-Type"] = "application/json";
        }
        if (this.apiKey) {
            headers["Authorization"] = `Bearer ${this.apiKey}`;
        }
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), this.timeout * 1000);
        let res;
        try {
            res = await fetch(url, {
                method,
                headers,
                body: body !== undefined ? JSON.stringify(body) : undefined,
                signal: controller.signal,
            });
        }
        catch (e) {
            clearTimeout(timer);
            if (e instanceof Error && e.name === "AbortError") {
                throw new ConnectionError("Request timed out");
            }
            throw new ConnectionError(e instanceof Error ? e.message : String(e));
        }
        clearTimeout(timer);
        if (!res.ok) {
            let detail;
            try {
                const errBody = (await res.json());
                detail = errBody.error ?? JSON.stringify(errBody);
            }
            catch {
                detail = await res.text().catch(() => `HTTP ${res.status}`);
            }
            throw new APIError(res.status, detail);
        }
        const text = await res.text();
        if (!text)
            return undefined;
        return JSON.parse(text);
    }
    _get(path) {
        return this._req("GET", path);
    }
    _post(path, body = {}) {
        return this._req("POST", path, body);
    }
    // ── Health ────────────────────────────────────────────────────────────
    async health() {
        const data = await this._get("/health");
        return {
            status: data.status,
            version: data.version ?? "unknown",
            vault: data.vault ?? "",
            uptime_secs: data.uptime_secs ?? 0,
            memories_total: data.memories_total ?? 0,
            qem_hit_rate: data.qem_hit_rate ?? null,
            db_size_bytes: data.db_size_bytes ?? null,
        };
    }
    // ── Memories CRUD ─────────────────────────────────────────────────────
    async capture(content, opts = {}) {
        const body = {
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
        return this._post("/memories", body);
    }
    async get(memoryId) {
        return this._get(`/memories/${memoryId}`);
    }
    async search(query, opts = {}) {
        const body = {
            limit: opts.limit ?? 20,
            offset: opts.offset ?? 0,
        };
        if (query)
            body.query = query;
        if (opts.layer)
            body.layer = opts.layer;
        if (opts.tags)
            body.tags = opts.tags;
        return this._post("/memories/search", body);
    }
    async list(limit = 20, offset = 0) {
        return this.search(null, { limit, offset });
    }
    async update(memoryId, opts) {
        const body = {};
        if (opts.content !== undefined)
            body.content = opts.content;
        if (opts.tags !== undefined)
            body.tags = opts.tags;
        if (opts.valence !== undefined)
            body.valence = opts.valence;
        if (opts.layer !== undefined)
            body.layer = opts.layer;
        if (opts.project !== undefined)
            body.project = opts.project;
        if (opts.privacy_level !== undefined)
            body.privacy_level = opts.privacy_level;
        if (opts.scope !== undefined)
            body.scope = opts.scope;
        if (opts.content_type !== undefined)
            body.content_type = opts.content_type;
        if (opts.occurred_at !== undefined)
            body.occurred_at = opts.occurred_at;
        return this._req("PATCH", `/memories/${memoryId}`, body);
    }
    async delete(memoryId) {
        await this._req("DELETE", `/memories/${memoryId}`);
        return true;
    }
    // ── Links ─────────────────────────────────────────────────────────────
    async link(sourceId, targetId, opts = {}) {
        await this._post("/memories/link", {
            source_id: sourceId,
            target_id: targetId,
            weight: opts.weight ?? 0.5,
            link_type: opts.link_type ?? "associative",
        });
        return true;
    }
    async getLinks(memoryId) {
        return this._get(`/memories/${memoryId}/links`);
    }
    async getRelated(memoryId, limit = 10) {
        return this._get(`/memories/${memoryId}/related?limit=${limit}`);
    }
    async ground(memoryId) {
        return this._post(`/memories/${memoryId}/ground`);
    }
    // ── Context assembly ──────────────────────────────────────────────────
    async assembleContext(query, opts = {}) {
        const data = await this._post("/context/assemble", {
            query,
            token_budget: opts.token_budget ?? 8192,
            max_engrams: opts.max_engrams ?? 12,
            max_recent_turns: opts.max_recent_turns ?? 5,
        });
        return {
            messages: data.messages,
            token_count: data.token_count,
            engrams_retrieved: data.engrams_retrieved,
            took_ms: data.took_ms,
        };
    }
    async *contextStream(sessionId = "default") {
        const url = `${this.baseUrl}/context/stream?session_id=${sessionId}`;
        const headers = {};
        if (this.apiKey) {
            headers["Authorization"] = `Bearer ${this.apiKey}`;
        }
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), this.timeout * 1000);
        try {
            const res = await fetch(url, { headers, signal: controller.signal });
            if (!res.ok || !res.body) {
                throw new APIError(res.status, `context_stream failed: HTTP ${res.status}`);
            }
            const reader = res.body.getReader();
            const decoder = new TextDecoder();
            let buffer = "";
            while (true) {
                const { done, value } = await reader.read();
                if (done)
                    break;
                clearTimeout(timer);
                buffer += decoder.decode(value, { stream: true });
                const lines = buffer.split("\n");
                buffer = lines.pop() ?? "";
                let eventType = "";
                let eventData = "";
                for (const line of lines) {
                    if (line.startsWith("event: ")) {
                        eventType = line.slice(7).trim();
                    }
                    else if (line.startsWith("data: ")) {
                        eventData = line.slice(6).trim();
                    }
                    else if (line === "") {
                        if (eventType === "done")
                            return;
                        if (eventData) {
                            try {
                                yield JSON.parse(eventData);
                            }
                            catch {
                                // skip unparseable events
                            }
                        }
                        eventType = "";
                        eventData = "";
                    }
                }
            }
        }
        finally {
            clearTimeout(timer);
        }
    }
    // ── Consolidation ─────────────────────────────────────────────────────
    async runDecay() {
        const data = await this._post("/consolidate/decay");
        return {
            ok: data.ok ?? false,
            strengthened: data.strengthened,
            decayed: data.decayed,
            message: data.message,
        };
    }
    async runConsolidation() {
        const data = await this._post("/consolidate/weekly");
        return {
            ok: data.ok ?? false,
            promoted: data.promoted,
            pruned: data.pruned,
            message: data.message,
        };
    }
    async consolidationHistory() {
        return this._get("/consolidate/history");
    }
    // ── Analytics ─────────────────────────────────────────────────────────
    async stats() {
        const data = await this._get("/analytics/stats");
        return {
            total: data.total,
            by_layer: data.by_layer ?? {},
            qem_hit_rate: data.qem_hit_rate ?? null,
            db_size_bytes: data.db_size_bytes ?? null,
        };
    }
    async detectPatterns(query = "", opts = {}) {
        const data = await this._post("/analytics/patterns", {
            query,
            min_engrams: opts.min_engrams ?? 5,
        });
        return {
            found: data.found ?? false,
            description: data.description ?? "",
            peak_day: data.peak_day,
            peak_period: data.peak_period,
            day_strength: data.day_strength,
            period_strength: data.period_strength,
            sample_size: data.sample_size,
        };
    }
    // ── Export / Import ───────────────────────────────────────────────────
    async export(opts = {}) {
        const body = { limit: opts.limit ?? 10_000 };
        if (opts.layer)
            body.layer = opts.layer;
        if (opts.tags)
            body.tags = opts.tags;
        return this._post("/export", body);
    }
    async importMemories(memories) {
        return this._post("/import", { memories });
    }
    // ── Config ────────────────────────────────────────────────────────────
    async getConfig() {
        return this._get("/config");
    }
    async updateConfig(config) {
        return this._req("PATCH", "/config", config);
    }
}
//# sourceMappingURL=client.js.map