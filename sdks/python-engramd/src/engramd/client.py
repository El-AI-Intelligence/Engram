"""Engram Memory Vault — Python client."""

import json
import os
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Iterator, Optional
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

DEFAULT_BASE = os.environ.get("ENGRAMD_URL", "http://localhost:8787")


@dataclass
class EngramLink:
    target_id: str
    weight: float
    link_type: str  # associative | causal | analogical | temporal


@dataclass
class EvidenceRef:
    memory_id: str
    relationship: str  # supports | contradicts | context_for


@dataclass
class Memory:
    """A single memory entry in the vault."""

    id: str
    layer: str  # episodic | semantic | imagined
    source: str
    privacy_level: str
    content: str
    context: dict[str, Any]
    strength: float
    valence: float
    retrievals: int
    imagined: bool
    grounded: bool
    created_at: str
    last_retrieved: Optional[str]
    project: Optional[str]
    tags: list[str]
    links: list[EngramLink] = field(default_factory=list)
    scope: str = "moment"  # moment | episode | narrative | rule
    content_type: str = "text"  # text | frames | conversation | context
    occurred_at: Optional[str] = None  # when the event actually happened
    evidence: list[EvidenceRef] = field(default_factory=list)


@dataclass
class VaultHealth:
    status: str
    version: str
    vault: str
    uptime_secs: int
    memories_total: int
    qem_hit_rate: Optional[float]
    db_size_bytes: Optional[int]


@dataclass
class Stats:
    total: int
    by_layer: dict[str, int]
    qem_hit_rate: Optional[float]
    db_size_bytes: Optional[int]


@dataclass
class ContextAssembly:
    messages: list[dict]
    token_count: int
    engrams_retrieved: int
    took_ms: int


@dataclass
class ConsolidationResult:
    ok: bool
    strengthened: Optional[int] = None
    decayed: Optional[int] = None
    promoted: Optional[int] = None
    pruned: Optional[int] = None
    message: Optional[str] = None


@dataclass
class TemporalPattern:
    found: bool
    description: str
    peak_day: Optional[str] = None
    peak_period: Optional[str] = None
    day_strength: Optional[float] = None
    period_strength: Optional[float] = None
    sample_size: Optional[int] = None


class MemoryVault:
    """Client for the Engram Memory Vault API (engramd).

    Usage:
        vault = MemoryVault()                        # localhost:8787
        vault = MemoryVault("https://engram.example.com")
        vault = MemoryVault(api_key="sk-...")        # remote with auth

        vault.capture("User asked about Rust async trait bounds.", tags=["rust", "async"])
        memories = vault.search("async", layer="episodic")
    """

    def __init__(
        self,
        base_url: str = DEFAULT_BASE,
        api_key: Optional[str] = None,
        timeout: int = 30,
    ):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout

    # ── HTTP helpers ────────────────────────────────────────────────────────

    def _req(self, method: str, path: str, body: Any = None) -> Any:
        url = f"{self.base_url}{path}"
        data = json.dumps(body).encode() if body is not None else None
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        req = Request(url, data=data, headers=headers, method=method)
        try:
            with urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read()
                if not raw:
                    return None
                return json.loads(raw)
        except HTTPError as e:
            body_text = e.read().decode(errors="replace")
            try:
                err = json.loads(body_text).get("error", body_text)
            except Exception:
                err = body_text
            raise APIError(e.code, err) from e
        except URLError as e:
            raise ConnectionError(str(e.reason)) from e

    def _get(self, path: str) -> Any:
        return self._req("GET", path)

    def _post(self, path: str, body: dict[str, Any] | None = None) -> Any:
        return self._req("POST", path, body or {})

    # ── Health ──────────────────────────────────────────────────────────────

    def health(self) -> VaultHealth:
        data = self._get("/health")
        return VaultHealth(
            status=data["status"],
            version=data.get("version", "unknown"),
            vault=data.get("vault", ""),
            uptime_secs=data.get("uptime_secs", 0),
            memories_total=data.get("memories_total", 0),
            qem_hit_rate=data.get("qem_hit_rate"),
            db_size_bytes=data.get("db_size_bytes"),
        )

    # ── Memories CRUD ───────────────────────────────────────────────────────

    def capture(
        self,
        content: str,
        *,
        layer: str = "episodic",
        source: str = "interaction",
        context: dict[str, Any] | None = None,
        tags: list[str] | None = None,
        valence: float = 0.0,
        privacy_level: str = "cloud_first",
        project: str | None = None,
        imagined: bool = False,
        scope: str = "moment",
        content_type: str = "text",
        occurred_at: str | None = None,
    ) -> Memory:
        body: dict[str, Any] = {
            "content": content,
            "layer": layer,
            "source": source,
            "context": context or {},
            "tags": tags or [],
            "valence": valence,
            "privacy_level": privacy_level,
            "project": project,
            "imagined": imagined,
            "scope": scope,
            "content_type": content_type,
        }
        if occurred_at:
            body["occurred_at"] = occurred_at
        return Memory(**self._post("/memories", body))

    def get(self, memory_id: str) -> Memory:
        return Memory(**self._get(f"/memories/{memory_id}"))

    def search(
        self,
        query: str | None = None,
        *,
        layer: str | None = None,
        tags: list[str] | None = None,
        limit: int = 20,
        offset: int = 0,
    ) -> list[Memory]:
        body: dict[str, Any] = {"limit": limit, "offset": offset}
        if query:
            body["query"] = query
        if layer:
            body["layer"] = layer
        if tags:
            body["tags"] = tags
        return [Memory(**m) for m in self._post("/memories/search", body)]

    def list(self, limit: int = 20, offset: int = 0) -> list[Memory]:
        return self.search(limit=limit, offset=offset)

    def update(
        self,
        memory_id: str,
        *,
        content: str | None = None,
        tags: list[str] | None = None,
        valence: float | None = None,
        layer: str | None = None,
        project: str | None = None,
        privacy_level: str | None = None,
        scope: str | None = None,
        content_type: str | None = None,
        occurred_at: str | None = None,
    ) -> Memory:
        body: dict[str, Any] = {}
        if content is not None:
            body["content"] = content
        if tags is not None:
            body["tags"] = tags
        if valence is not None:
            body["valence"] = valence
        if layer is not None:
            body["layer"] = layer
        if project is not None:
            body["project"] = project
        if privacy_level is not None:
            body["privacy_level"] = privacy_level
        if scope is not None:
            body["scope"] = scope
        if content_type is not None:
            body["content_type"] = content_type
        if occurred_at is not None:
            body["occurred_at"] = occurred_at

        # Use the same write-then-get pattern as the Rust server
        return Memory(**self._req("PATCH", f"/memories/{memory_id}", body))

    def delete(self, memory_id: str) -> bool:
        self._req("DELETE", f"/memories/{memory_id}")
        return True

    # ── Links ───────────────────────────────────────────────────────────────

    def link(
        self,
        source_id: str,
        target_id: str,
        weight: float = 0.5,
        link_type: str = "associative",
    ) -> bool:
        self._post("/memories/link", {
            "source_id": source_id,
            "target_id": target_id,
            "weight": weight,
            "link_type": link_type,
        })
        return True

    def get_links(self, memory_id: str) -> list[EngramLink]:
        data = self._get(f"/memories/{memory_id}/links")
        return [EngramLink(**l) for l in data]

    def get_related(self, memory_id: str, limit: int = 10) -> list[Memory]:
        data = self._get(f"/memories/{memory_id}/related?limit={limit}")
        return [Memory(**m) for m in data]

    def ground(self, memory_id: str) -> Memory:
        """Ground an imagined memory (mark as verified)."""
        return Memory(**self._post(f"/memories/{memory_id}/ground"))

    # ── Context assembly ────────────────────────────────────────────────────

    def assemble_context(
        self,
        query: str,
        token_budget: int = 8192,
        max_engrams: int = 12,
        max_recent_turns: int = 5,
    ) -> ContextAssembly:
        data = self._post("/context/assemble", {
            "query": query,
            "token_budget": token_budget,
            "max_engrams": max_engrams,
            "max_recent_turns": max_recent_turns,
        })
        return ContextAssembly(
            messages=data["messages"],
            token_count=data["token_count"],
            engrams_retrieved=data["engrams_retrieved"],
            took_ms=data["took_ms"],
        )

    def context_stream(self, session_id: str = "default") -> Iterator[dict]:
        """Yields memory events from SSE stream. Blocking iterator."""
        import sseclient  # optional dependency: pip install sseclient-py

        url = f"{self.base_url}/context/stream?session_id={session_id}"
        headers = {}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        req = Request(url, headers=headers)
        with urlopen(req, timeout=self.timeout) as resp:
            for event in sseclient.SSEClient(resp).events():
                if event.event == "done":
                    break
                yield json.loads(event.data)

    # ── Consolidation ───────────────────────────────────────────────────────

    def run_decay(self) -> ConsolidationResult:
        data = self._post("/consolidate/decay")
        return ConsolidationResult(
            ok=data.get("ok", False),
            strengthened=data.get("strengthened"),
            decayed=data.get("decayed"),
            message=data.get("message"),
        )

    def run_consolidation(self) -> ConsolidationResult:
        data = self._post("/consolidate/weekly")
        return ConsolidationResult(
            ok=data.get("ok", False),
            promoted=data.get("promoted"),
            pruned=data.get("pruned"),
            message=data.get("message"),
        )

    def consolidation_history(self) -> list[dict]:
        return self._get("/consolidate/history")

    # ── Analytics ───────────────────────────────────────────────────────────

    def stats(self) -> Stats:
        data = self._get("/analytics/stats")
        return Stats(
            total=data["total"],
            by_layer=data.get("by_layer", {}),
            qem_hit_rate=data.get("qem_hit_rate"),
            db_size_bytes=data.get("db_size_bytes"),
        )

    def detect_patterns(
        self, query: str = "", min_engrams: int = 5
    ) -> TemporalPattern:
        data = self._post("/analytics/patterns", {
            "query": query,
            "min_engrams": min_engrams,
        })
        return TemporalPattern(
            found=data.get("found", False),
            description=data.get("description", ""),
            peak_day=data.get("peak_day"),
            peak_period=data.get("peak_period"),
            day_strength=data.get("day_strength"),
            period_strength=data.get("period_strength"),
            sample_size=data.get("sample_size"),
        )

    # ── Export / Import ─────────────────────────────────────────────────────

    def export(
        self,
        layer: str | None = None,
        tags: list[str] | None = None,
        limit: int = 10_000,
    ) -> dict:
        body: dict[str, Any] = {"limit": limit}
        if layer:
            body["layer"] = layer
        if tags:
            body["tags"] = tags
        return self._post("/export", body)

    def import_memories(self, memories: list[dict]) -> dict:
        """Import a list of memory objects. Returns {"imported": N, "skipped": N}."""
        return self._post("/import", {"memories": memories})

    # ── Config ──────────────────────────────────────────────────────────────

    def get_config(self) -> dict:
        return self._get("/config")

    def update_config(self, config: dict) -> dict:
        return self._req("PATCH", "/config", config)


class APIError(Exception):
    """Error returned by the engramd API."""

    def __init__(self, status: int, detail: str):
        self.status = status
        self.detail = detail
        super().__init__(f"[{status}] {detail}")


class ConnectionError(Exception):
    """Could not connect to engramd."""

    pass
