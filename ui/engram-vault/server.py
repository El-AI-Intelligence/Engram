#!/usr/bin/env python3
"""
Engram Memory Vault — mock `engramd` server.

Implements the full API contract from docs/engram-product/KIMI_K3_PROMPT.md
with deterministic-ish seeded in-memory data, plus static file serving for
the UI in this directory. Python 3 standard library only.

Run:  python3 server.py   →   http://localhost:8787
"""

import hashlib
import json
import mimetypes
import os
import random
import re
import time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

PORT = 8787
BASE_DIR = os.path.dirname(os.path.abspath(__file__))
LANDING_DIR = os.path.join(os.path.dirname(BASE_DIR), "landing")
START_TIME = time.time()
NOW = datetime.now(timezone.utc)

_rng = random.Random(20260805)


# --------------------------------------------------------------------------
# Seed data
# --------------------------------------------------------------------------

def _iso(dt):
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _mid(seed):
    return "eng_" + hashlib.sha1(str(seed).encode()).hexdigest()[:12]


# (content, layer, source, scope, tags, valence, strength, days_ago, project, extra)
SEED = [
    # ---- Episodic --------------------------------------------------------
    ("User asked about Rust async trait bounds. Explained Send + Sync requirements "
     "for Arc<Mutex<T>> and the smc_kernel refactor approach. Discussed how trait "
     "bounds propagate through nested generic types and the compiler errors that "
     "result from missing bounds.",
     "episodic", "interaction", "moment",
     ["rust", "async", "traits", "send-sync"], 0.7, 0.87, 0, "axiom-os",
     {"retrieval_count": 4, "context": {"session_id": "sess_a91f", "turn": 3, "topic": "rust_async_traits"}}),

    ("Debug session — fixed cache key collision in QEM. Two engrams with similar "
     "embeddings were evicting each other; switched key to blake3(id + created_at).",
     "episodic", "interaction", "episode",
     ["qem", "cache", "debugging"], -0.2, 1.05, 0, "ellm",
     {"retrieval_count": 6, "context": {"session_id": "sess_a91f", "turn": 11}}),

    ("Window capture: code — smc_kernel/mod.rs, editing the page fault handler. "
     "User had 3 terminals open, one running cargo test --release.",
     "episodic", "window", "moment",
     ["smc_kernel", "code", "window-capture"], 0.0, 0.62, 0, "axiom-os", {}),

    ("User prefers dark mode, vim keybindings, and terse commit messages. Mentioned "
     "they hate verbose changelogs.",
     "episodic", "chat", "moment",
     ["preferences", "user-profile"], 0.4, 1.3, 1, "personal",
     {"retrieval_count": 12}),

    ("Conversation about ELLM council design: three agents vote on red-line "
     "decisions, majority wins, dissent is logged to the vault as an episodic memory.",
     "episodic", "chat", "episode",
     ["ellm", "council", "design"], 0.6, 0.94, 1, "ellm",
     {"retrieval_count": 3, "context": {"session_id": "sess_b20c", "turn": 7}}),

    ("Window capture: browser — docs.rs/tokio, reading about tokio::select! and "
     "cancellation semantics.",
     "episodic", "window", "moment",
     ["rust", "tokio", "reading"], 0.1, 0.41, 1, "axiom-os", {}),

    ("Agent run: nightly consolidation promoted 3 episodes about ELLM architecture "
     "into a semantic rule. 7 imagined engrams pruned.",
     "episodic", "consolidation", "episode",
     ["consolidation", "ellm", "nightly"], 0.0, 0.78, 2, "ellm", {}),

    ("User was frustrated with flaky integration tests in ellm-guardrail. Spent 90 "
     "minutes tracing a race in the console-ui websocket reconnect logic.",
     "episodic", "interaction", "episode",
     ["guardrail", "testing", "debugging", "websocket"], -0.65, 0.9, 2, "ellm",
     {"retrieval_count": 2, "occurred_offset_hours": 3}),

    ("Mic note (transcribed): idea for engram federation — two vaults exchange "
     "signed semantic rules over a relay, episodic stays local.",
     "episodic", "mic", "moment",
     ["federation", "idea", "voice-note"], 0.5, 0.55, 3, "personal", {}),

    ("Research session: compared text-embedding-3-small vs local bge-small for the "
     "QEM index. Local model is 4x slower but keeps everything offline.",
     "episodic", "research", "episode",
     ["embeddings", "qem", "offline"], 0.2, 0.72, 3, "ellm", {}),

    ("Window capture: code — engram/src/decay.rs, tuning the exponential decay "
     "curve. Half-life set to 14 days for episodic, 90 for semantic.",
     "episodic", "window", "moment",
     ["decay", "engram", "code"], 0.0, 0.38, 4, "ellm", {}),

    ("User asked how valence scoring works. Explained: sources tag valence at "
     "capture, consolidation re-scores on promotion, range -1.0 to 1.0.",
     "episodic", "interaction", "moment",
     ["valence", "explain"], 0.15, 0.66, 4, "ellm", {"retrieval_count": 1}),

    ("System event: vault backup skipped — no backup target configured.",
     "episodic", "system", "moment",
     ["system", "backup", "warning"], -0.3, 0.25, 5, None, {}),

    ("Pairing session: wired the /context/assemble endpoint to the council "
     "orchestrator. Token budget enforcement happens before priority packing.",
     "episodic", "agent", "episode",
     ["context-assembly", "council", "tokens"], 0.55, 1.1, 5, "ellm",
     {"retrieval_count": 5}),

    ("User reviewed the engram-vault UI mockups. Liked the force-directed graph "
     "with typed edges; asked for a legend and focus search.",
     "episodic", "chat", "moment",
     ["ui", "graph", "feedback"], 0.75, 0.8, 6, "axiom-os", {}),

    ("Long debugging session on SSE stream drops in /context/stream. Root cause: "
     "proxy buffering; fixed with X-Accel-Buffering: no header.",
     "episodic", "interaction", "episode",
     ["sse", "debugging", "streaming"], -0.4, 0.7, 7, "ellm",
     {"retrieval_count": 2}),

    ("Window capture: writing — weekly review notes. User outlined goals for the "
     "ELLM guardrail beta launch.",
     "episodic", "window", "moment",
     ["weekly-review", "writing", "goals"], 0.3, 0.35, 8, "personal", {}),

    ("User asked about engram export format. Showed the ndjson layout: one "
     "MemoryEntry per line, links preserved by id.",
     "episodic", "interaction", "moment",
     ["export", "ndjson"], 0.1, 0.5, 9, "ellm", {}),

    ("Chat about naming: user settled on 'Engram Memory Vault' over 'memory "
     "console'. Vault metaphor tested well.",
     "episodic", "chat", "moment",
     ["naming", "product"], 0.45, 0.58, 10, "axiom-os", {}),

    ("Sensor event: ambient light dropped at 18:40 — user usually switches to "
     "reading mode around this time.",
     "episodic", "sensor", "moment",
     ["sensor", "ambient"], 0.0, 0.12, 11, None, {}),

    ("Agent run: pattern detector found 'debugging' peaks on Thursdays in the "
     "evening (35% of samples). Stored as temporal pattern.",
     "episodic", "agent", "episode",
     ["patterns", "debugging", "temporal"], 0.2, 0.64, 12, "ellm", {}),

    ("Early conversation: user described the three-layer memory model — episodic "
     "for what happened, semantic for what was learned, imagined for what was "
     "dreamed. Quarantine for ungrounded imagined engrams.",
     "episodic", "interaction", "narrative",
     ["architecture", "layers", "origin"], 0.8, 1.5, 16, "ellm",
     {"retrieval_count": 9}),

    # ---- Semantic --------------------------------------------------------
    ("Rust async trait patterns: Send bounds propagate through Arc, Sync through "
     "Mutex. Common pitfall: holding a MutexGuard across an .await point.",
     "semantic", "consolidation", "rule",
     ["rust", "async", "patterns"], 0.0, 1.2, 3, "axiom-os",
     {"retrieval_count": 14}),

    ("ELLM architecture rule: all memory writes go through the vault; agents never "
     "share raw episodic memory, only promoted semantic rules.",
     "semantic", "consolidation", "rule",
     ["ellm", "architecture", "rule"], 0.1, 1.6, 5, "ellm",
     {"retrieval_count": 21}),

    ("QEM design decision: embedding cache keyed by blake3(id + created_at); warm "
     "limit 1000 entries; LRU eviction with strength-weighted priority.",
     "semantic", "consolidation", "rule",
     ["qem", "design", "cache"], 0.0, 1.4, 6, "ellm",
     {"retrieval_count": 8}),

    ("User preference: concise answers, code over prose, no cheerleading. Dark "
     "mode everywhere. Monospace for IDs and timestamps.",
     "semantic", "consolidation", "rule",
     ["preferences", "user-profile", "style"], 0.5, 1.7, 8, "personal",
     {"retrieval_count": 30}),

    ("Decay schedule: episodic half-life 14 days, semantic 90 days, imagined 7 "
     "days unless grounded. Nightly run at 03:00.",
     "semantic", "system", "rule",
     ["decay", "schedule", "config"], 0.0, 1.1, 9, "ellm", {}),

    ("Guardrail testing lesson: websocket reconnect tests must use a fake clock; "
     "real timers make the suite flaky under CI load.",
     "semantic", "consolidation", "rule",
     ["guardrail", "testing", "lesson"], -0.1, 0.95, 10, "ellm",
     {"retrieval_count": 4}),

    ("Context assembly: token budget is enforced bottom-up — Required slots first, "
     "then High, Normal, Low. Reserve 60% of budget for Required+High.",
     "semantic", "consolidation", "rule",
     ["context-assembly", "tokens", "priority"], 0.2, 1.35, 11, "ellm",
     {"retrieval_count": 11}),

    ("Narrative: the ELLM project grew from a single-agent experiment into a "
     "council architecture with an encrypted local memory vault as its backbone.",
     "semantic", "consolidation", "narrative",
     ["ellm", "narrative", "history"], 0.65, 1.5, 14, "ellm", {}),

    ("SSE hardening: always send periodic heartbeats, disable proxy buffering, and "
     "treat client disconnects as normal shutdown.",
     "semantic", "consolidation", "rule",
     ["sse", "streaming", "lesson"], 0.0, 0.85, 13, "ellm", {}),

    ("Offline-first rule: if a feature needs the network, it is optional. The "
     "vault, embeddings, and consolidation must all work with no WAN.",
     "semantic", "consolidation", "rule",
     ["offline", "architecture", "principle"], 0.3, 1.55, 15, "ellm",
     {"retrieval_count": 17}),

    ("Export format contract: ndjson, one MemoryEntry per line, ids stable across "
     "export/import, links re-resolved by id on import.",
     "semantic", "consolidation", "rule",
     ["export", "ndjson", "contract"], 0.0, 0.9, 12, "ellm", {}),

    ("Valence semantics: >= 0.5 joyful, 0.1..0.5 positive, -0.3..0.1 neutral, "
     "< -0.3 challenging. Valence tints graph nodes, never filters retrieval.",
     "semantic", "system", "rule",
     ["valence", "semantics"], 0.0, 1.0, 7, "ellm", {}),

    # ---- Imagined --------------------------------------------------------
    ("Scenario: moving QEM from HashMap to a disk-backed store with write-through "
     "cache and LRU eviction. Imagined trade-offs: slower warm-up, unbounded size.",
     "imagined", "imagined", "episode",
     ["qem", "refactor", "architecture"], 0.1, 0.35, 0, "ellm",
     {"grounded": False}),

    ("Scenario: a mobile companion app that shows the memory graph as a "
     "constellation you can pinch-zoom. Imagined interaction model only.",
     "imagined", "imagined", "episode",
     ["mobile", "graph", "ui"], 0.6, 0.42, 1, "axiom-os",
     {"grounded": False}),

    ("Scenario: engram federation protocol — vaults exchange signed semantic "
     "rules over a relay. Grounded against the mic note from Aug 2.",
     "imagined", "imagined", "episode",
     ["federation", "protocol"], 0.4, 0.68, 3, "ellm",
     {"grounded": True}),

    ("Scenario: what if decay were valence-aware — challenging memories decay "
     "slower so lessons are kept, joyful ones are reinforced on retrieval?",
     "imagined", "imagined", "moment",
     ["decay", "valence", "idea"], 0.3, 0.3, 5, "ellm",
     {"grounded": False}),

    ("Scenario: QEM warm cache shared across council agents via mmap. Imagined "
     "concurrency hazards: torn reads during eviction.",
     "imagined", "imagined", "episode",
     ["qem", "council", "concurrency"], -0.2, 0.22, 8, "ellm",
     {"grounded": False}),

    ("Scenario: a 'memory palimpsest' view — slider to scrub the vault back in "
     "time and watch strengths change. Grounded: decay math confirmed feasible.",
     "imagined", "imagined", "episode",
     ["ui", "time-travel", "decay"], 0.7, 0.75, 6, "axiom-os",
     {"grounded": True}),

    ("Scenario: vault-to-vault gossip for discovered temporal patterns, so two "
     "agents notice complementary routines. Privacy implications unclear.",
     "imagined", "imagined", "moment",
     ["federation", "patterns", "privacy"], -0.4, 0.18, 10, "ellm",
     {"grounded": False}),

    # ---- A few extra recent episodic for the "new today" feed ------------
    ("User asked the vault console to show a live preview of context assembly — "
     "'the killer demo' per the design brief.",
     "episodic", "chat", "moment",
     ["context-assembly", "ui", "demo"], 0.65, 0.92, 0, "axiom-os",
     {"retrieval_count": 1, "context": {"session_id": "sess_c41d", "turn": 2}}),

    ("Window capture: code — ui/engram-vault/graph.js, hand-rolling a force "
     "simulation with velocity-Verlet integration, no d3.",
     "episodic", "window", "moment",
     ["graph", "code", "ui"], 0.2, 0.7, 0, "axiom-os", {}),

    ("System event: QEM cache reached 85% hit rate after warm-up. 1000 entries "
     "resident.",
     "episodic", "system", "moment",
     ["qem", "cache", "system"], 0.3, 0.6, 0, None, {}),

    ("Debug session: explorer search debounce was firing twice; AbortController "
     "now cancels stale requests.",
     "episodic", "interaction", "moment",
     ["debugging", "ui", "search"], -0.15, 0.5, 0, "axiom-os", {}),
]

# (src_idx, tgt_idx, link_type, weight)
SEED_LINKS = [
    (0, 22, "causal", 0.9),       # async question -> async patterns rule
    (1, 24, "causal", 0.85),      # cache collision fix -> QEM design decision
    (3, 25, "associative", 0.7),  # preferences chat -> preferences rule
    (4, 29, "temporal", 0.6),     # council chat -> offline rule
    (6, 23, "causal", 0.8),       # consolidation run -> architecture rule
    (7, 27, "causal", 0.75),      # flaky tests -> guardrail lesson
    (8, 34, "causal", 0.9),       # mic note -> federation scenario
    (9, 29, "associative", 0.5),  # embeddings research -> offline rule
    (10, 26, "temporal", 0.65),   # decay.rs capture -> decay schedule
    (13, 28, "causal", 0.8),      # pairing -> context assembly rule
    (15, 30, "causal", 0.7),      # SSE debug -> SSE lesson
    (17, 32, "associative", 0.55),# export question -> export contract
    (20, 34, "temporal", 0.5),    # pattern detector -> federation gossip
    (21, 23, "temporal", 0.95),   # origin convo -> architecture rule
    (33, 24, "analogical", 0.6),  # QEM scenario -> QEM design
    (35, 26, "analogical", 0.5),  # valence decay idea -> decay schedule
    (36, 24, "associative", 0.45),# mmap scenario -> QEM design
    (37, 26, "causal", 0.65),     # palimpsest -> decay schedule
    (38, 20, "associative", 0.4), # gossip -> pattern detector
    (0, 1, "temporal", 0.5),
    (0, 5, "associative", 0.4),
    (22, 23, "associative", 0.6),
    (40, 28, "temporal", 0.5),
    (41, 34, "associative", 0.4),
    (43, 30, "analogical", 0.35),
    (2, 0, "temporal", 0.45),
    (11, 33, "associative", 0.3),
    (14, 39, "temporal", 0.5),
    (19, 21, "associative", 0.35),
    (31, 21, "temporal", 0.6),
]

STATE = {
    "memories": {},   # id -> MemoryEntry dict
    "history": [],    # consolidation runs
    "config": {
        "vault_path": "~/.engram/vaults/default",
        "encryption": "SQLCipher (machine-ID bound)",
        "decay_schedule": "daily@03:00",
        "consolidation_schedule": "weekly@sun-04:00",
        "pattern_schedule": "weekly@sun-05:00",
        "qem": {"warm_limit": 1000, "hit_rate": 0.85},
        "context": {
            "default_budget": 8192,
            "high_priority_reserve": 0.6,
            "max_recent_turns": 12,
            "max_engrams": 10,
            "include_world_context": True,
            "include_narratives": True,
            "include_imagined": False,
            "summarization_mode": "extractive",
            "llm_model": "claude-haiku-4-5-20251001",
            "priorities": {
                "SystemPrompt": "required",
                "CharacterBias": "required",
                "CurrentTurn": "required",
                "CouncilInstruction": "required",
                "RedLineWarning": "required",
                "EngramRetrieval": "high",
                "WorldContext": "high",
                "PurposeVector": "high",
                "RecentHistory": "normal",
                "CompactedHistory": "low",
            },
        },
        "embedding": {
            "mode": "local",
            "model": "text-embedding-3-small",
            "dimensions": 1536,
            "auto_embed_on_capture": True,
        },
        "auth": {"mode": "local", "keys": []},
    },
    "last_decay": None,
    "last_consolidation": None,
}


def seed_memories():
    for i, row in enumerate(SEED):
        (content, layer, source, scope, tags, valence, strength,
         days_ago, project, extra) = row
        jitter_h = _rng.uniform(0, 9)
        created = NOW - timedelta(days=days_ago, hours=jitter_h,
                                  minutes=_rng.randint(0, 59))
        occurred_off = extra.pop("occurred_offset_hours", None)
        mem = {
            "id": _mid(i),
            "layer": layer,
            "source": source,
            "scope": scope,
            "content": content,
            "strength": round(strength, 3),
            "valence": valence,
            "imagined": layer == "imagined",
            "grounded": extra.pop("grounded", layer != "imagined"),
            "retrieval_count": extra.pop("retrieval_count", _rng.randint(0, 3)),
            "tags": list(tags),
            "project": project,
            "links_out": [],
            "created_at": _iso(created),
            "last_retrieved": _iso(created + timedelta(hours=_rng.uniform(1, 20)))
            if _rng.random() < 0.6 else None,
            "occurred_at": _iso(created - timedelta(hours=occurred_off or 0)),
            "context": extra.pop("context", {}),
        }
        STATE["memories"][mem["id"]] = mem

    ids = [_mid(i) for i in range(len(SEED))]
    for s, t, lt, w in SEED_LINKS:
        STATE["memories"][ids[s]]["links_out"].append(
            {"target_id": ids[t], "weight": w, "link_type": lt})


def seed_history():
    kinds = ["decay", "decay", "weekly", "decay", "weekly",
             "decay", "decay", "weekly"]
    for i, kind in enumerate(kinds):
        run_at = NOW - timedelta(days=len(kinds) - i - 1, hours=3)
        run = {
            "id": "run_" + hashlib.sha1(f"run{i}".encode()).hexdigest()[:8],
            "type": "weekly" if kind == "weekly" else "decay",
            "run_at": _iso(run_at),
            "episodes_processed": _rng.randint(30, 90),
            "semantics_created": _rng.choice([0, 0, 2, 3, 5, 8]) if kind == "weekly" else 0,
            "engrams_decayed": _rng.randint(150, 420),
            "strengthened": _rng.randint(4, 18),
            "pruned": _rng.randint(0, 9) if kind == "weekly" else _rng.randint(0, 2),
        }
        STATE["history"].append(run)
    STATE["last_decay"] = STATE["history"][-1]["run_at"] \
        if STATE["history"][-1]["type"] == "decay" else STATE["history"][-2]["run_at"]
    weekly = [r for r in STATE["history"] if r["type"] == "weekly"]
    STATE["last_consolidation"] = weekly[-1]["run_at"]


seed_memories()
seed_history()


# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

def tokens_of(text):
    return max(1, len(text) // 4)


def rel_desc(iso_ts):
    dt = datetime.strptime(iso_ts, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    delta = NOW - dt
    if delta.days >= 7:
        return f"{delta.days // 7} week{'s' if delta.days // 7 > 1 else ''} ago"
    if delta.days >= 1:
        return f"{delta.days} day{'s' if delta.days > 1 else ''} ago"
    hours = delta.seconds // 3600
    if hours >= 1:
        return f"{hours} hour{'s' if hours > 1 else ''} ago"
    return "just now"


def search_memories(query="", layer=None, tags=None, min_strength=0.0,
                    sort_by="relevance", limit=50, offset=0, scope=None,
                    source=None):
    q = (query or "").strip().lower()
    terms = q.split()
    results = []
    for m in STATE["memories"].values():
        if layer and m["layer"] != layer:
            continue
        if scope and m["scope"] != scope:
            continue
        if source and m["source"] != source:
            continue
        if m["strength"] < min_strength:
            continue
        if tags and not all(t.lower() in [x.lower() for x in m["tags"]] for t in tags):
            continue
        score = 0.0
        if terms:
            hay = (m["content"] + " " + " ".join(m["tags"])).lower()
            if not all(t in hay for t in terms):
                continue
            score = sum(hay.count(t) for t in terms) + m["strength"]
        results.append((score, m))

    if sort_by == "strength":
        results.sort(key=lambda sm: sm[1]["strength"], reverse=True)
    elif sort_by == "recency":
        results.sort(key=lambda sm: sm[1]["created_at"], reverse=True)
    elif sort_by == "valence":
        results.sort(key=lambda sm: sm[1]["valence"], reverse=True)
    else:  # relevance
        results.sort(key=lambda sm: (sm[0], sm[1]["strength"]), reverse=True)

    total = len(results)
    page = [m for _, m in results[offset:offset + limit]]
    return page, total


def assemble_context(body):
    t0 = time.time()
    cfg = dict(STATE["config"]["context"])
    cfg.update(body.get("config") or {})
    budget = int(body.get("token_budget") or cfg["default_budget"])
    query = body.get("query") or ""
    system_prompt = body.get("system_prompt") or \
        "You are a helpful AI assistant with persistent long-term memory."
    history = body.get("recent_history") or []
    world = body.get("world_context")

    t1 = time.time()
    # retrieve engrams
    terms = query.lower().split()
    cands = []
    for m in STATE["memories"].values():
        if m["layer"] == "imagined" and not (cfg["include_imagined"] and m["grounded"]):
            continue
        if m["scope"] == "narrative" and not cfg["include_narratives"]:
            continue
        hay = (m["content"] + " " + " ".join(m["tags"])).lower()
        score = sum(hay.count(t) for t in terms) if terms else 0
        if terms and score == 0:
            continue
        cands.append((score + m["strength"] * 0.5 + m["retrieval_count"] * 0.05, m))
    cands.sort(key=lambda sm: sm[0], reverse=True)
    engrams = [m for _, m in cands[:int(cfg["max_engrams"])]]
    retrieval_ms = round((time.time() - t1) * 1000, 2)

    messages = [{"role": "system", "content": system_prompt}]
    if world and cfg["include_world_context"]:
        messages.append({
            "role": "system",
            "content": "[WORLD CONTEXT] Active app: {app} · Window: {title} · Local time: {time}".format(
                app=world.get("app", "unknown"),
                title=world.get("title", "unknown"),
                time=world.get("time", NOW.strftime("%H:%M"))),
        })
    for m in engrams:
        tag = m["layer"].upper()
        if m["layer"] == "imagined":
            tag += " · GROUNDED" if m["grounded"] else " · UNVERIFIED"
        messages.append({
            "role": "system",
            "content": f"[MEMORY · {tag} · {rel_desc(m['created_at'])}] {m['content']}",
        })

    max_turns = int(cfg["max_recent_turns"])
    if len(history) > max_turns:
        older = history[:-max_turns]
        recent = history[-max_turns:]
        if cfg["summarization_mode"] == "abstractive":
            summary = ("[SUMMARY of %d earlier turns · %s] Earlier discussion covered: %s…"
                       % (len(older), cfg.get("llm_model", "llm"),
                          " ".join(m.get("content", "")[:60] for m in older[:3])))
        else:
            summary = ("[SUMMARY of %d earlier turns · extractive] %s"
                       % (len(older), " / ".join(
                           (m.get("content", "")[:80]) for m in older[:2])))
        messages.append({"role": "system", "content": summary})
    else:
        recent = history
    messages.extend({"role": m.get("role", "user"),
                     "content": m.get("content", "")} for m in recent)
    messages.append({"role": "user", "content": query})

    # budget enforcement: trim oldest memory messages until under reserve
    reserve = float(cfg.get("high_priority_reserve", 0.6))
    effective_budget = int(budget * (1.0 - (1.0 - reserve) * 0.5))  # soft cap
    def total_tokens(msgs):
        return sum(tokens_of(m["content"]) for m in msgs)
    while total_tokens(messages) > effective_budget and engrams:
        engrams.pop()  # drop lowest-ranked engram
        # remove its message (memory messages sit after system/world prefix)
        for i, msg in enumerate(messages):
            if msg["role"] == "system" and msg["content"].startswith("[MEMORY"):
                del messages[i]
                break

    total = total_tokens(messages)
    return {
        "messages": messages,
        "metadata": {
            "total_tokens": total,
            "budget": budget,
            "utilization": round(total / budget, 4) if budget else 0,
            "engrams_retrieved": len(engrams),
            "retrieval_took_ms": retrieval_ms,
            "assembly_took_ms": round((time.time() - t0) * 1000, 2),
        },
    }


# --------------------------------------------------------------------------
# HTTP handler
# --------------------------------------------------------------------------

class Handler(BaseHTTPRequestHandler):
    server_version = "engramd-mock/0.1.0"
    protocol_version = "HTTP/1.1"

    # -- plumbing ----------------------------------------------------------

    def log_message(self, fmt, *args):
        pass  # quiet

    def _cors(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods",
                         "GET, POST, PATCH, DELETE, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")

    def _json(self, obj, status=200):
        body = json.dumps(obj, indent=None).encode()
        self.send_response(status)
        self._cors()
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _text(self, body, status=200, ctype="text/plain; charset=utf-8"):
        data = body.encode()
        self.send_response(status)
        self._cors()
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _body(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        return raw

    def _json_body(self):
        raw = self._body()
        try:
            return json.loads(raw or b"{}")
        except json.JSONDecodeError:
            return {}

    def _404(self, msg="not found"):
        self._json({"error": msg}, 404)

    def do_OPTIONS(self):
        self.send_response(204)
        self._cors()
        self.send_header("Content-Length", "0")
        self.end_headers()

    # -- routing -----------------------------------------------------------

    def do_GET(self):
        u = urlparse(self.path)
        path, qs = u.path, parse_qs(u.query)
        m = re.fullmatch(r"/memories/([\w-]+)", path)
        ml = re.fullmatch(r"/memories/([\w-]+)/links", path)
        mr = re.fullmatch(r"/memories/([\w-]+)/related", path)

        if path == "/health":
            self._json({
                "status": "ok",
                "version": "0.1.0-mock",
                "vault": "default",
                "uptime_secs": int(time.time() - START_TIME),
                "memories_total": len(STATE["memories"]),
                "qem_hit_rate": STATE["config"]["qem"]["hit_rate"],
                "db_size_bytes": self._vault_size(),
            })
        elif path == "/analytics/stats":
            self._json(self._stats())
        elif path == "/analytics/patterns":
            self._json(self._pattern(qs.get("query", [""])[0]))
        elif path == "/config":
            self._json(STATE["config"])
        elif path == "/memories":
            limit = int(qs.get("limit", ["50"])[0])
            page, total = search_memories(
                query=qs.get("q", [""])[0],
                layer=qs.get("layer", [None])[0],
                min_strength=float(qs.get("min_strength", ["0"])[0]),
                sort_by=qs.get("sort_by", ["recency"])[0],
                limit=limit,
                offset=int(qs.get("offset", ["0"])[0]),
            )
            self._json({"results": page, "total": total,
                        "search_type": "scan", "took_ms": 1.2})
        elif ml:
            self._links(ml.group(1))
        elif mr:
            self._related(mr.group(1), int(qs.get("limit", ["10"])[0]))
        elif m:
            mem = STATE["memories"].get(m.group(1))
            self._json(mem) if mem else self._404("memory not found")
        elif path == "/consolidate/history":
            self._json({"runs": list(reversed(STATE["history"]))})
        elif path == "/context/stream":
            self._stream(qs)
        elif path == "/export":
            self._export({})
        else:
            self._static(path)

    def do_POST(self):
        u = urlparse(self.path)
        path = u.path
        mg = re.fullmatch(r"/memories/([\w-]+)/ground", path)

        if path == "/memories/search":
            b = self._json_body()
            t0 = time.time()
            page, total = search_memories(
                query=b.get("query", ""),
                layer=b.get("layer"),
                tags=b.get("tags"),
                min_strength=float(b.get("min_strength") or 0),
                sort_by=b.get("sort_by", "relevance"),
                limit=int(b.get("limit") or 50),
                offset=int(b.get("offset") or 0),
                scope=b.get("scope"),
                source=b.get("source"),
            )
            self._json({
                "results": page,
                "total": total,
                "search_type": "fts5" if b.get("query") else "scan",
                "took_ms": round((time.time() - t0) * 1000, 2),
            })
        elif path == "/memories":
            self._create_memory(self._json_body())
        elif path == "/memories/link":
            self._create_link(self._json_body())
        elif mg:
            self._ground(mg.group(1))
        elif path == "/context/assemble":
            self._json(assemble_context(self._json_body()))
        elif path == "/consolidate/decay":
            self._json(self._run_decay())
        elif path == "/consolidate/weekly":
            self._json(self._run_weekly())
        elif path == "/analytics/patterns":
            b = self._json_body()
            self._json(self._pattern(b.get("query", ""), b.get("min_engrams", 5)))
        elif path == "/export":
            self._export(self._json_body())
        elif path == "/import":
            self._import()
        else:
            self._404()

    def do_PATCH(self):
        u = urlparse(self.path)
        m = re.fullmatch(r"/memories/([\w-]+)", u.path)
        if u.path == "/config":
            b = self._json_body()
            self._deep_merge(STATE["config"], b)
            self._json(STATE["config"])
        elif m:
            mem = STATE["memories"].get(m.group(1))
            if not mem:
                return self._404("memory not found")
            b = self._json_body()
            for k in ("tags", "valence", "project", "strength", "content"):
                if k in b:
                    mem[k] = b[k]
            self._json(mem)
        else:
            self._404()

    def do_DELETE(self):
        m = re.fullmatch(r"/memories/([\w-]+)", urlparse(self.path).path)
        if m and m.group(1) in STATE["memories"]:
            mid = m.group(1)
            del STATE["memories"][mid]
            for mem in STATE["memories"].values():
                mem["links_out"] = [l for l in mem["links_out"]
                                    if l["target_id"] != mid]
            self.send_response(204)
            self._cors()
            self.send_header("Content-Length", "0")
            self.end_headers()
        else:
            self._404("memory not found")

    # -- endpoint implementations ------------------------------------------

    def _vault_size(self):
        return 2_415_919 + len(STATE["memories"]) * 3800

    def _stats(self):
        mems = list(STATE["memories"].values())
        by_layer, by_source, by_scope = {}, {}, {}
        total_links = 0
        for m in mems:
            by_layer[m["layer"]] = by_layer.get(m["layer"], 0) + 1
            by_source[m["source"]] = by_source.get(m["source"], 0) + 1
            by_scope[m["scope"]] = by_scope.get(m["scope"], 0) + 1
            total_links += len(m["links_out"])
        today = NOW.strftime("%Y-%m-%d")
        new_today = sum(1 for m in mems if m["created_at"].startswith(today))
        decaying = sum(1 for m in mems if m["strength"] < 0.2)
        quarantined = sum(1 for m in mems
                          if m["layer"] == "imagined" and not m["grounded"])
        return {
            "total_memories": len(mems),
            "by_layer": by_layer,
            "by_source": by_source,
            "by_scope": by_scope,
            "total_links": total_links,
            "avg_strength": round(sum(m["strength"] for m in mems) / max(1, len(mems)), 3),
            "avg_valence": round(sum(m["valence"] for m in mems) / max(1, len(mems)), 3),
            "qem_hit_rate": STATE["config"]["qem"]["hit_rate"],
            "total_embeddings": len(mems),
            "vault_size_bytes": self._vault_size(),
            "new_today": new_today,
            "decayed_last_night": STATE["history"][-1]["engrams_decayed"] if STATE["history"] else 0,
            "decaying": decaying,
            "quarantined": quarantined,
            "last_consolidation": STATE["last_consolidation"],
            "last_decay": STATE["last_decay"],
        }

    def _pattern(self, query, min_engrams=5):
        q = (query or "debugging").lower()
        matches = [m for m in STATE["memories"].values()
                   if q in (m["content"] + " " + " ".join(m["tags"])).lower()]
        days = ["Mondays", "Tuesdays", "Wednesdays", "Thursdays",
                "Fridays", "Saturdays", "Sundays"]
        periods = ["morning", "afternoon", "evening", "late night"]
        h = int(hashlib.sha1(q.encode()).hexdigest(), 16)
        day = days[h % 7]
        period = periods[(h >> 4) % 4]
        day_strength = round(0.22 + ((h >> 8) % 30) / 100.0, 2)
        period_strength = round(0.3 + ((h >> 12) % 45) / 100.0, 2)
        n = len(matches)
        if n < min_engrams:
            desc = (f'"{q}" — not enough engrams ({n}) to detect a pattern.')
            day, period = None, None
            day_strength = period_strength = 0.0
        else:
            desc = (f'"{q}" — tends to happen on {day} '
                    f'({int(day_strength * 100)}% of the time), '
                    f'usually in the {period}.')
        return {"pattern": {
            "query": q,
            "sample_size": n,
            "peak_day": day,
            "day_strength": day_strength,
            "peak_period": period,
            "period_strength": period_strength,
            "description": desc,
        }}

    def _create_memory(self, b):
        if not b.get("content"):
            return self._json({"error": "content required"}, 400)
        mid = "eng_" + hashlib.sha1(
            f"{time.time()}{b['content']}".encode()).hexdigest()[:12]
        layer = b.get("layer", "episodic")
        mem = {
            "id": mid,
            "layer": layer,
            "source": b.get("source", "interaction"),
            "scope": b.get("scope", "moment"),
            "content": b["content"],
            "strength": float(b.get("strength", 1.0)),
            "valence": float(b.get("valence", 0.0)),
            "imagined": layer == "imagined",
            "grounded": layer != "imagined",
            "retrieval_count": 0,
            "tags": list(b.get("tags") or []),
            "project": b.get("project"),
            "links_out": [],
            "created_at": _iso(datetime.now(timezone.utc)),
            "last_retrieved": None,
            "occurred_at": _iso(datetime.now(timezone.utc)),
            "context": b.get("context") or {},
        }
        STATE["memories"][mid] = mem
        for link in b.get("links_to") or []:
            if link.get("target_id") in STATE["memories"]:
                mem["links_out"].append({
                    "target_id": link["target_id"],
                    "weight": float(link.get("weight", 0.5)),
                    "link_type": link.get("link_type", "associative"),
                })
        self._json({"id": mid, "qem_code": "QEM_OK",
                    "strength": mem["strength"],
                    "created_at": mem["created_at"]}, 201)

    def _create_link(self, b):
        src = STATE["memories"].get(b.get("source_id", ""))
        tgt = STATE["memories"].get(b.get("target_id", ""))
        if not src or not tgt:
            return self._json({"error": "source_id and target_id must exist"}, 400)
        lt = b.get("link_type", "associative")
        if lt not in ("associative", "causal", "analogical", "temporal"):
            return self._json({"error": "invalid link_type"}, 400)
        src["links_out"] = [l for l in src["links_out"]
                            if not (l["target_id"] == tgt["id"] and l["link_type"] == lt)]
        src["links_out"].append({"target_id": tgt["id"],
                                 "weight": float(b.get("weight", 0.5)),
                                 "link_type": lt})
        self._json({"ok": True, "source_id": src["id"], "target_id": tgt["id"],
                    "link_type": lt}, 201)

    def _links(self, mid):
        mem = STATE["memories"].get(mid)
        if not mem:
            return self._404("memory not found")
        outgoing = []
        for l in mem["links_out"]:
            tgt = STATE["memories"].get(l["target_id"])
            if tgt:
                outgoing.append({**l, "target": tgt})
        incoming = []
        for other in STATE["memories"].values():
            for l in other["links_out"]:
                if l["target_id"] == mid:
                    incoming.append({"source_id": other["id"],
                                     "weight": l["weight"],
                                     "link_type": l["link_type"],
                                     "source": other})
        self._json({"outgoing": outgoing, "incoming": incoming})

    def _related(self, mid, limit):
        if mid not in STATE["memories"]:
            return self._404("memory not found")
        seen, out, frontier = {mid}, [], [mid]
        while frontier and len(out) < limit:
            nxt = []
            for cur in frontier:
                mem = STATE["memories"].get(cur)
                if not mem:
                    continue
                neighbors = [l["target_id"] for l in mem["links_out"]]
                neighbors += [o["id"] for o in STATE["memories"].values()
                              if any(l["target_id"] == cur for l in o["links_out"])]
                for nid in neighbors:
                    if nid not in seen and nid in STATE["memories"]:
                        seen.add(nid)
                        out.append(STATE["memories"][nid])
                        nxt.append(nid)
                        if len(out) >= limit:
                            break
            frontier = nxt
        self._json({"results": out, "total": len(out)})

    def _ground(self, mid):
        mem = STATE["memories"].get(mid)
        if not mem:
            return self._404("memory not found")
        if mem["layer"] != "imagined":
            return self._json({"error": "only imagined memories can be grounded"}, 400)
        mem["grounded"] = True
        mem["strength"] = min(2.0, round(mem["strength"] + 0.3, 3))
        self._json(mem)

    def _run_decay(self):
        t0 = time.time()
        strengthened = decayed = pruned = 0
        to_delete = []
        half_life = {"episodic": 14, "semantic": 90, "imagined": 7}
        for m in STATE["memories"].values():
            if m["retrieval_count"] > 2:
                m["strength"] = min(2.0, round(m["strength"] * 1.02, 4))
                strengthened += 1
            else:
                hl = half_life[m["layer"]]
                factor = 0.5 ** (1.0 / hl)
                m["strength"] = round(m["strength"] * factor, 4)
                decayed += 1
                if m["strength"] < 0.05:
                    to_delete.append(m["id"])
        for mid in to_delete:
            del STATE["memories"][mid]
            pruned += 1
        took = round((time.time() - t0) * 1000, 2)
        STATE["last_decay"] = _iso(datetime.now(timezone.utc))
        STATE["history"].append({
            "id": "run_" + hashlib.sha1(str(time.time()).encode()).hexdigest()[:8],
            "type": "decay", "run_at": STATE["last_decay"],
            "episodes_processed": len(STATE["memories"]),
            "semantics_created": 0, "engrams_decayed": decayed,
            "strengthened": strengthened, "pruned": pruned,
        })
        return {"strengthened": strengthened, "decayed": decayed,
                "pruned": pruned, "took_ms": took}

    def _run_weekly(self):
        promoted = pruned_imagined = 0
        # promote: strongest episodic with high retrieval -> semantic copy
        cands = sorted(
            (m for m in STATE["memories"].values()
             if m["layer"] == "episodic" and m["retrieval_count"] >= 3),
            key=lambda m: m["strength"], reverse=True)[:2]
        for src in cands:
            mid = "eng_" + hashlib.sha1(
                f"promote{time.time()}{src['id']}".encode()).hexdigest()[:12]
            STATE["memories"][mid] = {
                "id": mid, "layer": "semantic", "source": "consolidation",
                "scope": "rule",
                "content": "Consolidated rule: " + src["content"][:180],
                "strength": min(2.0, src["strength"] + 0.2),
                "valence": src["valence"], "imagined": False, "grounded": True,
                "retrieval_count": 0, "tags": list(dict.fromkeys(
                    src["tags"] + ["consolidated"])),
                "project": src["project"],
                "links_out": [{"target_id": src["id"], "weight": 0.9,
                               "link_type": "causal"}],
                "created_at": _iso(datetime.now(timezone.utc)),
                "last_retrieved": None,
                "occurred_at": _iso(datetime.now(timezone.utc)),
                "context": {"promoted_from": src["id"]},
            }
            promoted += 1
        # prune weakest ungrounded imagined
        weak = sorted(
            (m for m in STATE["memories"].values()
             if m["layer"] == "imagined" and not m["grounded"]),
            key=lambda m: m["strength"])[:1]
        for m in weak:
            if m["strength"] < 0.25:
                del STATE["memories"][m["id"]]
                pruned_imagined += 1
        narratives_updated = 1 if any(
            m["scope"] == "narrative" for m in STATE["memories"].values()) else 0
        STATE["last_consolidation"] = _iso(datetime.now(timezone.utc))
        STATE["history"].append({
            "id": "run_" + hashlib.sha1(f"w{time.time()}".encode()).hexdigest()[:8],
            "type": "weekly", "run_at": STATE["last_consolidation"],
            "episodes_processed": len(STATE["memories"]),
            "semantics_created": promoted, "engrams_decayed": 0,
            "strengthened": 0, "pruned": pruned_imagined,
        })
        return {"promoted_to_semantic": promoted,
                "pruned_imagined": pruned_imagined,
                "narratives_updated": narratives_updated}

    def _export(self, b):
        layer, project = b.get("layer"), b.get("project")
        tags = b.get("tags")
        min_strength = float(b.get("min_strength") or 0)
        lines = []
        for m in STATE["memories"].values():
            if layer and m["layer"] != layer:
                continue
            if project and m["project"] != project:
                continue
            if tags and not all(t in m["tags"] for t in tags):
                continue
            if m["strength"] < min_strength:
                continue
            lines.append(json.dumps(m))
        body = "\n".join(lines) + "\n"
        self.send_response(200)
        self._cors()
        self.send_header("Content-Type", "application/x-ndjson; charset=utf-8")
        self.send_header("Content-Disposition",
                         'attachment; filename="engram-export.ndjson"')
        self.send_header("Content-Length", str(len(body.encode())))
        self.end_headers()
        self.wfile.write(body.encode())

    def _import(self):
        raw = self._body().decode("utf-8", "replace")
        imported = skipped = errors = 0
        existing_contents = {m["content"] for m in STATE["memories"].values()}
        for line in raw.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                m = json.loads(line)
            except json.JSONDecodeError:
                errors += 1
                continue
            if not isinstance(m, dict) or not m.get("content"):
                errors += 1
                continue
            if m["content"] in existing_contents or \
               m.get("id") in STATE["memories"]:
                skipped += 1
                continue
            mid = "eng_" + hashlib.sha1(
                f"imp{time.time()}{m['content'][:40]}".encode()).hexdigest()[:12]
            layer = m.get("layer", "episodic")
            STATE["memories"][mid] = {
                "id": mid,
                "layer": layer,
                "source": m.get("source", "interaction"),
                "scope": m.get("scope", "moment"),
                "content": m["content"],
                "strength": float(m.get("strength", 1.0)),
                "valence": float(m.get("valence", 0.0)),
                "imagined": layer == "imagined",
                "grounded": bool(m.get("grounded", layer != "imagined")),
                "retrieval_count": int(m.get("retrieval_count", 0)),
                "tags": list(m.get("tags") or []),
                "project": m.get("project"),
                "links_out": [],
                "created_at": m.get("created_at") or _iso(datetime.now(timezone.utc)),
                "last_retrieved": m.get("last_retrieved"),
                "occurred_at": m.get("occurred_at"),
                "context": m.get("context") or {},
            }
            existing_contents.add(m["content"])
            imported += 1
        self._json({"imported": imported, "skipped": skipped, "errors": errors})

    def _stream(self, qs):
        budget = int(qs.get("token_budget", ["8192"])[0])
        assembled = assemble_context({"query": qs.get("query", [""])[0],
                                      "token_budget": budget})
        self.send_response(200)
        self._cors()
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("X-Accel-Buffering", "no")
        self.end_headers()

        def send(event, data):
            self.wfile.write(f"event: {event}\n".encode())
            self.wfile.write(f"data: {json.dumps(data)}\n\n".encode())
            self.wfile.flush()

        try:
            send("snapshot", assembled)
            time.sleep(0.3)
            send("delta", {"added_tokens": tokens_of("heartbeat"),
                           "note": "world context refreshed",
                           "total_tokens": assembled["metadata"]["total_tokens"] + 2})
            time.sleep(0.3)
            send("delta", {"added_tokens": 0, "note": "stream idle"})
            send("done", {"ok": True})
        except (BrokenPipeError, ConnectionResetError):
            pass

    def _static(self, path):
        # Landing page (and its assets) live in the sibling ui/landing/ dir;
        # the vault UI is at /app (mirrors the live Caddy setup).
        if path in ("/", "") or path.startswith("/js/landing"):
            base = LANDING_DIR
            rel = os.path.normpath(path).lstrip("/") or "index.html"
        elif path == "/app":
            base = BASE_DIR
            rel = "index.html"
        else:
            base = BASE_DIR
            rel = os.path.normpath(path).lstrip("/")
        full = os.path.join(base, rel)
        if not full.startswith(base) or not os.path.isfile(full):
            # SPA-ish fallback: unknown non-API paths get index.html
            full = os.path.join(BASE_DIR, "index.html")
        ctype = mimetypes.guess_type(full)[0] or "application/octet-stream"
        if full.endswith(".js"):
            ctype = "text/javascript"
        try:
            with open(full, "rb") as f:
                data = f.read()
        except OSError:
            return self._404("file not found")
        self.send_response(200)
        self._cors()
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    @staticmethod
    def _deep_merge(dst, src):
        for k, v in src.items():
            if isinstance(v, dict) and isinstance(dst.get(k), dict):
                Handler._deep_merge(dst[k], v)
            else:
                dst[k] = v


def main():
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"engramd-mock listening on http://localhost:{PORT}")
    print(f"  UI:     http://localhost:{PORT}/")
    print(f"  Health: http://localhost:{PORT}/health")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
