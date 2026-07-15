"""
Property-based tests for the Life Log skill.

**Validates: Requirements 5.1, 5.2, 5.3, 5.5**

Properties tested:
  - Property 10: Life log registra todos os campos obrigatórios (Req 5.1)
  - Property 11: Life log round-trip — serialização SQLite (Req 5.3)
  - Property 12: Busca semântica retorna apenas resultados acima do threshold (Req 5.2)
  - Property 13: get_persona_summary retorna apenas interações dentro do período (Req 5.5)
"""

from __future__ import annotations

import asyncio
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from hypothesis import given, settings, assume
from hypothesis import strategies as st

from db.protocols import InteractionRecord
from db.sqlite_adapter import SQLiteLifeLogAdapter, _cosine_similarity
from life_log_helpers import InMemoryLifeLogAdapter

# ---------------------------------------------------------------------------
# Hypothesis strategies
# ---------------------------------------------------------------------------

_slug = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=30,
)

_intent = st.text(
    alphabet=st.characters(whitelist_categories=("Lu", "Ll", "Nd", "Zs")),
    min_size=1,
    max_size=50,
)

_tool_name = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
    min_size=1,
    max_size=30,
)

_tools_list = st.lists(_tool_name, min_size=0, max_size=5)

# Fixed-dimension embedding (dim=4) — small enough for fast tests
_embedding_dim = 4
_nonzero_float = st.floats(
    min_value=-1.0,
    max_value=1.0,
    allow_nan=False,
    allow_infinity=False,
).filter(lambda x: abs(x) > 1e-6)

_embedding = st.lists(
    _nonzero_float,
    min_size=_embedding_dim,
    max_size=_embedding_dim,
)

_threshold = st.floats(
    min_value=0.0,
    max_value=1.0,
    allow_nan=False,
    allow_infinity=False,
)

_days_positive = st.integers(min_value=1, max_value=365)


def _run(coro):
    """Run a coroutine synchronously (test helper)."""
    return asyncio.run(coro)


# ---------------------------------------------------------------------------
# Property 10 — Life log registra todos os campos obrigatórios
# Validates: Requirements 5.1
# ---------------------------------------------------------------------------

REQUIRED_FIELDS = ("id", "persona", "intent", "tools", "embedding", "timestamp")


@given(
    persona=_slug,
    intent=_intent,
    tools=_tools_list,
    embedding=_embedding,
)
@settings(max_examples=100)
def test_property10_log_interaction_stores_all_required_fields(
    persona: str,
    intent: str,
    tools: list[str],
    embedding: list[float],
) -> None:
    """
    **Property 10: Life log registra todos os campos obrigatórios**

    For any interaction logged, the stored record must contain all required
    fields: id, persona, intent, tools, embedding, and timestamp.

    **Validates: Requirements 5.1**
    """
    adapter = InMemoryLifeLogAdapter()
    ts = datetime.now(tz=timezone.utc)

    interaction_id = _run(
        adapter.log_interaction(
            persona=persona,
            intent=intent,
            tools=tools,
            embedding=embedding,
            timestamp=ts,
        )
    )

    # ID must be returned and non-empty
    assert interaction_id, "log_interaction must return a non-empty ID"

    # Exactly one record must be stored
    assert len(adapter.all_records) == 1

    record = adapter.all_records[0]

    # All required fields must be present and non-None
    for field_name in REQUIRED_FIELDS:
        value = getattr(record, field_name, None)
        assert value is not None, f"Required field '{field_name}' is None"

    # Field-level type and value checks
    assert record.id == interaction_id
    assert record.persona == persona
    assert record.intent == intent
    assert record.tools == tools
    assert record.embedding == embedding
    assert record.timestamp == ts


@given(
    interactions=st.lists(
        st.tuples(_slug, _intent, _tools_list, _embedding),
        min_size=1,
        max_size=10,
    )
)
@settings(max_examples=50)
def test_property10_all_logged_interactions_have_required_fields(
    interactions: list[tuple],
) -> None:
    """
    **Property 10 (batch variant)**

    Every record in the log must have all required fields, regardless of
    how many interactions were logged.

    **Validates: Requirements 5.1**
    """
    adapter = InMemoryLifeLogAdapter()
    ts = datetime.now(tz=timezone.utc)

    _run(
        asyncio.gather(
            *(
                adapter.log_interaction(
                    persona=persona,
                    intent=intent,
                    tools=tools,
                    embedding=embedding,
                    timestamp=ts,
                )
                for persona, intent, tools, embedding in interactions
            )
        )
    )

    assert len(adapter.all_records) == len(interactions)

    for record in adapter.all_records:
        for field_name in REQUIRED_FIELDS:
            assert getattr(record, field_name, None) is not None, (
                f"Field '{field_name}' is None in record {record.id}"
            )


# ---------------------------------------------------------------------------
# Property 11 — Life log round-trip (serialização SQLite)
# Validates: Requirements 5.3
# ---------------------------------------------------------------------------


@given(
    persona=_slug,
    intent=_intent,
    tools=_tools_list,
    embedding=_embedding,
)
@settings(max_examples=100)
def test_property11_sqlite_round_trip_preserves_all_fields(
    persona: str,
    intent: str,
    tools: list[str],
    embedding: list[float],
) -> None:
    """
    **Property 11: Life log round-trip (serialização SQLite)**

    For any interaction stored via SQLiteLifeLogAdapter, retrieving it must
    produce an object with the same values for all fields as originally stored.

    **Validates: Requirements 5.3**
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test-life-log.db"
        adapter = SQLiteLifeLogAdapter(db_path=db_path)
        ts = datetime.now(tz=timezone.utc).replace(microsecond=0)

        interaction_id = _run(
            adapter.log_interaction(
                persona=persona,
                intent=intent,
                tools=tools,
                embedding=embedding,
                timestamp=ts,
            )
        )

        # Retrieve via get_persona_summary (covers the full read path)
        records = _run(adapter.get_persona_summary(persona=persona, days=1))

        assert len(records) == 1, (
            f"Expected 1 record, got {len(records)}"
        )

        record = records[0]

        # ID round-trip
        assert record.id == interaction_id

        # String fields round-trip
        assert record.persona == persona
        assert record.intent == intent

        # Tools list round-trip (JSON serialisation)
        assert record.tools == tools

        # Embedding round-trip (float32 BLOB — allow small precision loss)
        assert len(record.embedding) == len(embedding), (
            f"Embedding dimension mismatch: {len(record.embedding)} != {len(embedding)}"
        )
        for orig, stored in zip(embedding, record.embedding):
            assert abs(orig - stored) < 1e-5, (
                f"Embedding value mismatch: original={orig}, stored={stored}"
            )

        # Timestamp round-trip (UTC ISO 8601)
        stored_ts = record.timestamp.astimezone(timezone.utc).replace(microsecond=0)
        assert stored_ts == ts, (
            f"Timestamp mismatch: stored={stored_ts}, original={ts}"
        )


@given(
    interactions=st.lists(
        st.tuples(_slug, _intent, _tools_list, _embedding),
        min_size=2,
        max_size=5,
    )
)
@settings(max_examples=50)
def test_property11_sqlite_round_trip_multiple_records(
    interactions: list[tuple],
) -> None:
    """
    **Property 11 (multi-record variant)**

    All N stored records must be retrievable with their original field values.
    Uses get_persona_summary per unique persona to retrieve all records.

    **Validates: Requirements 5.3**
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test-life-log.db"
        adapter = SQLiteLifeLogAdapter(db_path=db_path)
        ts = datetime.now(tz=timezone.utc).replace(microsecond=0)

        # Log all interactions concurrently
        stored_ids: list[str] = _run(
            asyncio.gather(
                *(
                    adapter.log_interaction(
                        persona=persona,
                        intent=intent,
                        tools=tools,
                        embedding=embedding,
                        timestamp=ts,
                    )
                    for persona, intent, tools, embedding in interactions
                )
            )
        )
        personas_used = {persona for persona, _, _, _ in interactions}

        # Retrieve all records via get_persona_summary for each unique persona concurrently
        summaries: list[list[InteractionRecord]] = _run(
            asyncio.gather(
                *(adapter.get_persona_summary(persona=p, days=1) for p in personas_used)
            )
        )
        retrieved_ids: set[str] = {r.id for records in summaries for r in records}

        # All stored IDs must be retrievable
        for iid in stored_ids:
            assert iid in retrieved_ids, f"Record {iid} not found after round-trip"


# ---------------------------------------------------------------------------
# Property 12 — Busca semântica retorna apenas resultados acima do threshold
# Validates: Requirements 5.2
# ---------------------------------------------------------------------------


@given(
    data=st.data(),
    n_records=st.integers(min_value=1, max_value=10),
    threshold=_threshold,
    query_embedding=_embedding,
)
@settings(max_examples=100)
def test_property12_search_similar_only_returns_results_above_threshold(
    data: st.DataObject,
    n_records: int,
    threshold: float,
    query_embedding: list[float],
) -> None:
    """
    **Property 12: Busca semântica retorna apenas resultados acima do threshold**

    For any search query with a given threshold, ALL returned results must
    have cosine similarity >= threshold. No result below the threshold may
    be returned.

    **Validates: Requirements 5.2**
    """
    adapter = InMemoryLifeLogAdapter()
    ts = datetime.now(tz=timezone.utc)

    # Draw N embeddings using st.data() — correct Hypothesis pattern
    embeddings = data.draw(st.lists(_embedding, min_size=n_records, max_size=n_records))

    _run(
        asyncio.gather(
            *(
                adapter.log_interaction(
                    persona="test-persona",
                    intent=f"intent-{i}",
                    tools=[],
                    embedding=emb,
                    timestamp=ts,
                )
                for i, emb in enumerate(embeddings)
            )
        )
    )

    results = _run(
        adapter.search_similar(
            query_embedding=query_embedding,
            persona=None,
            limit=n_records,
            threshold=threshold,
        )
    )

    # Every returned result must satisfy the threshold
    for record in results:
        sim = _cosine_similarity(query_embedding, record.embedding)
        assert sim >= threshold - 1e-9, (
            f"Result with similarity {sim:.4f} is below threshold {threshold:.4f}"
        )


@given(
    embeddings=st.lists(_embedding, min_size=2, max_size=10),
    threshold=st.floats(
        min_value=0.65,
        max_value=1.0,
        allow_nan=False,
        allow_infinity=False,
    ),
)
@settings(max_examples=100)
def test_property12_default_threshold_065_enforced(
    embeddings: list[list[float]],
    threshold: float,
) -> None:
    """
    **Property 12 (default threshold variant)**

    With threshold=0.65 (the spec default), no result with similarity < 0.65
    is ever returned.

    **Validates: Requirements 5.2**
    """
    adapter = InMemoryLifeLogAdapter()
    ts = datetime.now(tz=timezone.utc)

    _run(
        asyncio.gather(
            *(
                adapter.log_interaction(
                    persona="p",
                    intent=f"i-{i}",
                    tools=[],
                    embedding=emb,
                    timestamp=ts,
                )
                for i, emb in enumerate(embeddings)
            )
        )
    )

    query = embeddings[0]
    results = _run(
        adapter.search_similar(
            query_embedding=query,
            persona=None,
            limit=len(embeddings),
            threshold=threshold,
        )
    )

    for record in results:
        sim = _cosine_similarity(query, record.embedding)
        assert sim >= threshold - 1e-9, (
            f"Result similarity {sim:.4f} is below threshold {threshold:.4f}"
        )


@given(
    n_results=st.integers(min_value=1, max_value=10),
    embeddings=st.lists(_embedding, min_size=1, max_size=20),
)
@settings(max_examples=50)
def test_property12_limit_is_respected(
    n_results: int,
    embeddings: list[list[float]],
) -> None:
    """
    **Property 12 (limit variant)**

    search_similar must never return more than *limit* results.

    **Validates: Requirements 5.2**
    """
    adapter = InMemoryLifeLogAdapter()
    ts = datetime.now(tz=timezone.utc)

    _run(
        asyncio.gather(
            *(
                adapter.log_interaction(
                    persona="p",
                    intent=f"i-{i}",
                    tools=[],
                    embedding=emb,
                    timestamp=ts,
                )
                for i, emb in enumerate(embeddings)
            )
        )
    )

    results = _run(
        adapter.search_similar(
            query_embedding=embeddings[0],
            persona=None,
            limit=n_results,
            threshold=0.0,  # accept all to test limit independently
        )
    )

    assert len(results) <= n_results, (
        f"search_similar returned {len(results)} results, limit was {n_results}"
    )


# ---------------------------------------------------------------------------
# Property 13 — get_persona_summary retorna apenas interações dentro do período
# Validates: Requirements 5.5
# ---------------------------------------------------------------------------


@given(
    days=_days_positive,
    n_inside=st.integers(min_value=0, max_value=5),
    n_outside=st.integers(min_value=0, max_value=5),
    embedding=_embedding,
)
@settings(max_examples=100)
def test_property13_get_persona_summary_only_returns_records_within_period(
    days: int,
    n_inside: int,
    n_outside: int,
    embedding: list[float],
) -> None:
    """
    **Property 13: get_persona_summary retorna apenas interações dentro do período**

    For any persona and number of days N, get_persona_summary must return
    only interactions with timestamp within the last N days. Records older
    than N days must never appear in the result.

    **Validates: Requirements 5.5**
    """
    assume(n_inside + n_outside > 0)

    adapter = InMemoryLifeLogAdapter()
    now = datetime.now(tz=timezone.utc)

    # Records INSIDE the window (timestamp within last `days` days)
    inside_results = _run(
        asyncio.gather(
            *(
                adapter.log_interaction(
                    persona="target",
                    intent=f"inside-{i}",
                    tools=[],
                    embedding=embedding,
                    timestamp=now - timedelta(days=days / 2, hours=i),
                )
                for i in range(n_inside)
            )
        )
    )
    inside_ids = set(inside_results)

    # Records OUTSIDE the window (timestamp older than `days` days)
    outside_results = _run(
        asyncio.gather(
            *(
                adapter.log_interaction(
                    persona="target",
                    intent=f"outside-{i}",
                    tools=[],
                    embedding=embedding,
                    timestamp=now - timedelta(days=days + 1, hours=i),
                )
                for i in range(n_outside)
            )
        )
    )
    outside_ids = set(outside_results)

    results = _run(adapter.get_persona_summary(persona="target", days=days))
    result_ids = {r.id for r in results}

    # All inside records must be present
    for iid in inside_ids:
        assert iid in result_ids, (
            f"Record {iid} (inside window) is missing from get_persona_summary"
        )

    # No outside records may appear
    for iid in outside_ids:
        assert iid not in result_ids, (
            f"Record {iid} (outside window) appeared in get_persona_summary"
        )


@given(
    days=_days_positive,
    embedding=_embedding,
)
@settings(max_examples=50)
def test_property13_different_personas_are_isolated(
    days: int,
    embedding: list[float],
) -> None:
    """
    **Property 13 (persona isolation variant)**

    get_persona_summary must only return records for the requested persona,
    never records from other personas.

    **Validates: Requirements 5.5**
    """
    adapter = InMemoryLifeLogAdapter()
    now = datetime.now(tz=timezone.utc)

    # Log records for two different personas concurrently
    _run(
        asyncio.gather(
            adapter.log_interaction(
                persona="persona-a",
                intent="action-a",
                tools=[],
                embedding=embedding,
                timestamp=now,
            ),
            adapter.log_interaction(
                persona="persona-b",
                intent="action-b",
                tools=[],
                embedding=embedding,
                timestamp=now,
            ),
        )
    )

    # Retrieve summaries concurrently
    results_a, results_b = _run(
        asyncio.gather(
            adapter.get_persona_summary(persona="persona-a", days=days),
            adapter.get_persona_summary(persona="persona-b", days=days),
        )
    )

    # Each summary must only contain records for the requested persona
    for record in results_a:
        assert record.persona == "persona-a", (
            f"persona-a summary contains record from persona '{record.persona}'"
        )

    for record in results_b:
        assert record.persona == "persona-b", (
            f"persona-b summary contains record from persona '{record.persona}'"
        )


@given(
    days=_days_positive,
    embedding=_embedding,
)
@settings(max_examples=50)
def test_property13_empty_result_for_persona_with_no_history(
    days: int,
    embedding: list[float],
) -> None:
    """
    **Property 13 (empty result variant)**

    get_persona_summary must return an empty list for a persona with no
    interactions in the requested period — not an error.

    **Validates: Requirements 5.5**
    """
    adapter = InMemoryLifeLogAdapter()

    results = _run(adapter.get_persona_summary(persona="nonexistent-persona", days=days))

    assert results == [], (
        f"Expected empty list for persona with no history, got {results}"
    )
