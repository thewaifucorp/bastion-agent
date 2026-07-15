"""Pytest fixtures for memupalace tests."""

from __future__ import annotations

import os
import tempfile
from typing import Generator
from unittest.mock import MagicMock

import pytest


@pytest.fixture
def tmp_chroma_path(tmp_path: object) -> str:
    """Temporary directory for ChromaDB persistence."""
    chroma_dir = tmp_path / "chroma"  # type: ignore[operator]
    chroma_dir.mkdir(parents=True, exist_ok=True)
    return str(chroma_dir)


@pytest.fixture
def tmp_sqlite_path(tmp_path: object) -> str:
    """Temporary SQLite file path for KnowledgeGraph."""
    return str(tmp_path / "knowledge.db")  # type: ignore[operator]


@pytest.fixture
def mock_embedder() -> MagicMock:
    """Mock embedder that returns fixed 384-dimensional embeddings."""
    import math

    embedder = MagicMock()

    def _embed(text: str) -> list[float]:
        # Deterministic 384-dim embedding based on text hash
        seed = hash(text) % (2**31)
        vec = []
        for i in range(384):
            # Simple deterministic values
            val = math.sin(seed + i) * 0.1 + 0.01
            vec.append(val)
        # L2 normalize
        norm = math.sqrt(sum(x * x for x in vec))
        return [x / norm for x in vec]

    def _embed_batch(texts: list[str]) -> list[list[float]]:
        return [_embed(t) for t in texts]

    embedder.embed.side_effect = _embed
    embedder.embed_batch.side_effect = _embed_batch

    return embedder
