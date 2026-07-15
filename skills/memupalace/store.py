"""MemoryStore — ChromaDB-backed vector store for memupalace."""

from __future__ import annotations

import uuid
from datetime import datetime, timezone
from pathlib import Path

from skills.memupalace.models import Memory


def _now_iso() -> str:
    return datetime.now(tz=timezone.utc).isoformat()


def _meta_to_memory(memory_id: str, document: str, meta: dict) -> Memory:
    """Convert ChromaDB metadata dict + document back to a Memory object."""
    return Memory(
        id=memory_id,
        content=document,
        wing=meta["wing"],
        hall=meta["hall"] if meta.get("hall") else None,
        room=meta["room"] if meta.get("room") else None,
        created_at=datetime.fromisoformat(meta["created_at"]),
        reinforcement_count=int(meta["reinforcement_count"]),
        last_reinforced_at=datetime.fromisoformat(meta["last_reinforced_at"]),
    )


class MemoryStore:
    """Persistent vector store backed by ChromaDB."""

    def __init__(self, chroma_path: str) -> None:
        import chromadb

        Path(chroma_path).mkdir(parents=True, exist_ok=True)
        self._client = chromadb.PersistentClient(path=chroma_path)
        self._col = self._client.get_or_create_collection(
            name="memories",
            metadata={"hnsw:space": "cosine"},
            embedding_function=None,  # embeddings provided manually
        )

    # ------------------------------------------------------------------
    # Write operations
    # ------------------------------------------------------------------

    def add(
        self,
        content: str,
        embedding: list[float],
        wing: str,
        hall: str | None,
        room: str | None,
        rust_belief_id: str | None = None,
    ) -> str:
        """Persist a new memory and return its UUID."""
        memory_id = str(uuid.uuid4())
        now = _now_iso()
        self._col.add(
            ids=[memory_id],
            embeddings=[embedding],
            documents=[content],
            metadatas=[
                {
                    "wing": wing,
                    "hall": hall if hall is not None else "",
                    "room": room if room is not None else "",
                    "created_at": now,
                    "reinforcement_count": 0,
                    "last_reinforced_at": now,
                    "rust_belief_id": rust_belief_id if rust_belief_id is not None else "",
                }
            ],
        )
        return memory_id

    def reinforce(self, memory_id: str) -> None:
        """Increment reinforcement_count and update last_reinforced_at."""
        result = self._col.get(ids=[memory_id], include=["metadatas"])
        if not result["ids"]:
            raise KeyError(memory_id)
        meta = dict(result["metadatas"][0])
        meta["reinforcement_count"] = int(meta["reinforcement_count"]) + 1
        meta["last_reinforced_at"] = _now_iso()
        self._col.update(ids=[memory_id], metadatas=[meta])

    def delete(self, memory_id: str) -> None:
        """Delete a memory by ID; raises KeyError if not found."""
        result = self._col.get(ids=[memory_id], include=[])
        if not result["ids"]:
            raise KeyError(memory_id)
        self._col.delete(ids=[memory_id])

    def invalidate(self, rust_belief_id: str) -> str | None:
        """Find and delete the ChromaDB embedding for a revoked Rust belief (D-03).

        Returns the chroma_id deleted, or None if no embedding found for this belief.
        Called by memupalace MCP tool 'memory_invalidate' when Rust revokes a belief.

        Guard: empty rust_belief_id returns None immediately (T-03-01-02).
        """
        if not rust_belief_id:
            return None
        result = self._col.get(
            where={"rust_belief_id": {"$eq": rust_belief_id}},
            include=["metadatas"],
        )
        if not result["ids"]:
            return None
        chroma_id = result["ids"][0]
        self._col.delete(ids=[chroma_id])
        return chroma_id

    # ------------------------------------------------------------------
    # Read operations
    # ------------------------------------------------------------------

    def get(self, memory_id: str) -> Memory:
        """Retrieve a memory by ID; raises KeyError if not found."""
        result = self._col.get(
            ids=[memory_id], include=["documents", "metadatas"]
        )
        if not result["ids"]:
            raise KeyError(memory_id)
        return _meta_to_memory(
            memory_id, result["documents"][0], result["metadatas"][0]
        )

    def check_duplicate(
        self, embedding: list[float], wing: str, threshold: float
    ) -> Memory | None:
        """Return the nearest neighbour in *wing* if similarity >= threshold."""
        # Need at least 1 document in the wing to query
        count = self._col.count()
        if count == 0:
            return None

        where: dict = {"wing": {"$eq": wing}}
        try:
            result = self._col.query(
                query_embeddings=[embedding],
                n_results=1,
                where=where,
                include=["documents", "metadatas", "distances"],
            )
        except Exception:
            # ChromaDB raises if n_results > number of items in filtered set
            return None

        ids = result.get("ids", [[]])[0]
        if not ids:
            return None

        distance = result["distances"][0][0]
        similarity = 1.0 - distance  # cosine distance ∈ [0,2] → similarity
        if similarity >= threshold:
            return _meta_to_memory(
                ids[0], result["documents"][0][0], result["metadatas"][0][0]
            )
        return None

    def vector_search(
        self,
        embedding: list[float],
        wing: str | None,
        hall: str | None,
        room: str | None,
        n_results: int,
    ) -> list[tuple[Memory, float]]:
        """Return (Memory, cosine_similarity) pairs ordered by similarity desc."""
        count = self._col.count()
        if count == 0:
            return []

        # Build $where filter
        conditions: list[dict] = []
        if wing is not None:
            conditions.append({"wing": {"$eq": wing}})
        if hall is not None:
            conditions.append({"hall": {"$eq": hall}})
        if room is not None:
            conditions.append({"room": {"$eq": room}})

        where: dict | None = None
        if len(conditions) == 1:
            where = conditions[0]
        elif len(conditions) > 1:
            where = {"$and": conditions}

        actual_n = min(n_results, count)
        try:
            kwargs: dict = dict(
                query_embeddings=[embedding],
                n_results=actual_n,
                include=["documents", "metadatas", "distances"],
            )
            if where is not None:
                kwargs["where"] = where
            result = self._col.query(**kwargs)
        except Exception:
            return []

        ids = result.get("ids", [[]])[0]
        if not ids:
            return []

        out: list[tuple[Memory, float]] = []
        for i, mid in enumerate(ids):
            distance = result["distances"][0][i]
            similarity = 1.0 - distance
            mem = _meta_to_memory(
                mid, result["documents"][0][i], result["metadatas"][0][i]
            )
            out.append((mem, similarity))

        # Already ordered by distance (ascending) = similarity (descending)
        return out

    def list_locations(self, wing: str | None, hall: str | None) -> list[str]:
        """
        - wing=None → distinct wings
        - wing set, hall=None → distinct halls in that wing
        - wing+hall set → distinct rooms in that wing+hall
        """
        all_meta = self._col.get(include=["metadatas"])["metadatas"]
        if not all_meta:
            return []

        if wing is None:
            return sorted({m["wing"] for m in all_meta if m.get("wing")})

        if hall is None:
            return sorted(
                {
                    m["hall"]
                    for m in all_meta
                    if m.get("wing") == wing and m.get("hall")
                }
            )

        return sorted(
            {
                m["room"]
                for m in all_meta
                if m.get("wing") == wing
                and m.get("hall") == hall
                and m.get("room")
            }
        )
