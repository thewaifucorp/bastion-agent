"""memupalace MCP server — exposes 6 memory tools via streamable-http (MUPL-01).

Replaces MemupalaceMCPServer (manual dispatch) with fastmcp HTTP server.
Transport: streamable-http, porta 8001 (or MEMUPALACE_PORT env var).

Tools: memory_add, memory_search, memory_list_locations, memory_delete,
       memory_embed, memory_invalidate (D-03).
"""

from __future__ import annotations

import logging
import os

from fastmcp import FastMCP

from skills.memupalace import query_sanitizer
from skills.memupalace.factory import Memupalace
from skills.memupalace.insight_cache import InsightCache
from skills.memupalace.models import MemupalaceSettings

logger = logging.getLogger(__name__)

mcp = FastMCP("memupalace")

# Singleton instance — initialized lazily on first tool call
_mp: Memupalace | None = None

# MUPL-02: TTL cache — avoids redundant LLM insight round-trips
_insight_cache: InsightCache = InsightCache()


def _get_mp() -> Memupalace:
    """Return (or initialize) the singleton Memupalace instance."""
    global _mp
    if _mp is None:
        settings = MemupalaceSettings.from_env()
        from skills.memupalace.factory import create_memupalace

        _mp = create_memupalace(settings)
    return _mp


def _validate_str(name: str, value: object) -> str:
    """Guard: raises ValueError if value is not a non-empty, non-whitespace string."""
    if not isinstance(value, str) or not str(value).strip():
        raise ValueError(
            f"Parameter '{name}' must be a non-empty, non-whitespace string."
        )
    return str(value)


# ---------------------------------------------------------------------------
# Tool: memory_add
# ---------------------------------------------------------------------------


@mcp.tool()
def memory_add(
    content: str,
    wing: str = "general",
    hall: str | None = None,
    room: str | None = None,
    rust_belief_id: str | None = None,
) -> dict:
    """Add a memory to memupalace. rust_belief_id links to Rust SQLite belief (D-03).

    MUPL-02: checks InsightCache first — if content+wing was recently cached,
    returns the cached result without a redundant store.add() call.
    """
    _validate_str("content", content)
    # MUPL-02: key the cache-aside on the FULL memory identity (content + wing +
    # placement + belief link). Keying on content alone would silently collapse
    # two distinct memories that differ only in hall/room/rust_belief_id into one
    # store and return a stale id (data loss).
    key_material = f"{content}::{hall}::{room}::{rust_belief_id}"
    cache_key = InsightCache.make_key(key_material, wing)
    cached = _insight_cache.get(cache_key)
    if cached is not None:
        logger.debug("memory_add: insight cache hit (wing=%s)", wing)
        return {"id": cached, "operation": "cache_hit"}

    result = _get_mp().add(
        content, wing=wing, hall=hall, room=room, rust_belief_id=rust_belief_id
    )
    _insight_cache.set(cache_key, result.id)
    return {"id": result.id, "operation": result.operation}


# ---------------------------------------------------------------------------
# Tool: memory_search
# ---------------------------------------------------------------------------


@mcp.tool()
def memory_search(
    query: str,
    wing: str | None = None,
    hall: str | None = None,
    room: str | None = None,
    limit: int = 5,
) -> list[dict]:
    """Search memories by semantic similarity.

    Applies query_sanitizer before embedding to strip system-prompt prefixes (D-14/MUPL-05).
    """
    _validate_str("query", query)
    sanitized = query_sanitizer.sanitize(query)
    if sanitized.was_sanitized:
        logger.debug(
            "query_sanitized method=%s original_len=%d clean_len=%d",
            sanitized.method,
            len(query),
            len(sanitized.clean_query),
        )
    results = _get_mp().search(
        sanitized.clean_query, wing=wing, hall=hall, room=room, limit=limit
    )
    return [
        {
            "id": r.id,
            "content": r.content,
            "score": r.salience_score,
            "wing": r.wing,
            "hall": r.hall,
            "room": r.room,
        }
        for r in results
    ]


# ---------------------------------------------------------------------------
# Tool: memory_list_locations
# ---------------------------------------------------------------------------


@mcp.tool()
def memory_list_locations() -> dict:
    """List all wings in the memupalace."""
    locations = _get_mp().list_locations()
    return {"wings": locations}


# ---------------------------------------------------------------------------
# Tool: memory_delete
# ---------------------------------------------------------------------------


@mcp.tool()
def memory_delete(memory_id: str) -> dict:
    """Delete a memory by its ChromaDB ID."""
    _validate_str("memory_id", memory_id)
    _get_mp().delete(memory_id)
    return {"deleted": memory_id}


# ---------------------------------------------------------------------------
# Tool: memory_embed
# ---------------------------------------------------------------------------


@mcp.tool()
def memory_embed(text: str) -> list[float]:
    """Return the embedding vector for a text (for debugging/similarity)."""
    _validate_str("text", text)
    mp = _get_mp()
    return mp._embedder.embed(text)


# ---------------------------------------------------------------------------
# Tool: memory_invalidate
# ---------------------------------------------------------------------------


@mcp.tool()
def memory_invalidate(rust_belief_id: str) -> dict:
    """Invalidate embedding + KG nodes for a revoked Rust belief (D-03).

    Called by the Rust core when a belief's weight drops to 0 (contestation/revocation).
    Removes the ChromaDB embedding and marks KG entities valid_to = now.
    """
    _validate_str("rust_belief_id", rust_belief_id)
    mp = _get_mp()
    chroma_id = mp._store.invalidate(rust_belief_id)
    kg_entities: list[str] = []
    if hasattr(mp._kg, "invalidate_by_memory"):
        kg_entities = mp._kg.invalidate_by_memory(rust_belief_id)
    return {
        "invalidated_chroma_id": chroma_id,
        "invalidated_kg_entities": kg_entities,
        "rust_belief_id": rust_belief_id,
    }


# ---------------------------------------------------------------------------
# Entrypoint
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    port = int(os.getenv("MEMUPALACE_PORT", "8001"))
    mcp.run(transport="streamable-http", host="0.0.0.0", port=port)
