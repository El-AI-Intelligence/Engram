"""Engram MCP auto-capture observer.

Wraps MCP tool handlers to passively capture agent activity as memories.

Every non-engram tool call becomes a memory. The observer hashes tool
name + args and skips duplicates within a rolling window to prevent
redundant captures. engram_* tools pass through unchanged (explicit
captures already produce memories).

Config via env:
  ENGRAM_AUTO_CAPTURE=true|false   (default: true)
  ENGRAM_AUTO_CAPTURE_WINDOW=100   dedup hash window size
"""

import hashlib
import json
import os
from typing import Any

from ..client import MemoryVault

AUTO_CAPTURE = os.environ.get("ENGRAM_AUTO_CAPTURE", "true").lower() != "false"
MAX_WINDOW = int(os.environ.get("ENGRAM_AUTO_CAPTURE_WINDOW", "100"))


class AutoCapture:
    """Observes MCP tool calls and auto-captures novel ones as memories."""

    def __init__(self, vault: MemoryVault, max_window: int = MAX_WINDOW):
        self.vault = vault
        self.seen: set[str] = set()
        self.max_window = max_window
        self.enabled = AUTO_CAPTURE

    # ── Public API ──────────────────────────────────────────────────────────

    def observe(self, tool_name: str, arguments: dict[str, Any]) -> None:
        """Observe a tool call and auto-capture if novel.

        Call this after the real handler succeeds. Fire-and-forget —
        failures are silent to never break the MCP loop.
        """
        if not self.enabled:
            return

        # engram_* tools are explicit captures — don't double-capture
        if tool_name.startswith("engram_"):
            return

        # Hash the observation to check novelty
        content_hash = self._content_hash(tool_name, arguments)
        if content_hash in self.seen:
            return

        # Mark as seen (LRU eviction if needed)
        self.seen.add(content_hash)
        if len(self.seen) > self.max_window:
            oldest = next(iter(self.seen))
            self.seen.discard(oldest)

        # Build a human-readable summary
        content = self._summarize(tool_name, arguments)

        # Auto-capture as an observation (fire-and-forget)
        try:
            self.vault.capture(
                content=content,
                source="observation",
                layer="episodic",
                scope="moment",
                content_type="text",
                tags=["auto", tool_name],
            )
        except Exception:
            pass  # Never break the MCP loop for a capture failure

    # ── Internal helpers ────────────────────────────────────────────────────

    @staticmethod
    def _content_hash(tool_name: str, args: dict[str, Any]) -> str:
        """Stable SHA-256 content hash for dedup."""
        payload = json.dumps([tool_name, args], sort_keys=True, default=str)
        return hashlib.sha256(payload.encode()).hexdigest()

    @staticmethod
    def _summarize(tool_name: str, args: dict[str, Any]) -> str:
        """Produce a one-line summary of the tool call for the memory content."""
        keys = list(args.keys())

        if tool_name == "engram_assemble_context":
            query = str(args.get("query", ""))
            return f"Agent assembled context for query: {query[:200]}"

        # For tools with a "query" or "content" arg, use that
        if "query" in keys:
            q = str(args.get("query", ""))
            return f"Agent called {tool_name}: {q[:200]}"
        if "content" in keys:
            c = str(args.get("content", ""))
            return f"Agent called {tool_name}: {c[:200]}"

        # For tools with path/URL args
        if "path" in keys or "url" in keys:
            loc = str(args.get("path", args.get("url", "")))
            return f"Agent called {tool_name} on {loc[:200]}"

        # Generic: list the arg keys so the memory isn't empty
        if keys:
            return f"Agent called {tool_name} with fields: {', '.join(keys)}"

        return f"Agent called {tool_name}"

    # ── Introspection (for testing) ─────────────────────────────────────────

    @property
    def seen_count(self) -> int:
        """Number of hashes currently in the dedup window."""
        return len(self.seen)

    def reset(self) -> None:
        """Clear the dedup window (useful for testing)."""
        self.seen.clear()
