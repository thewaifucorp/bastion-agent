"""Data models for the memupalace skill."""

from __future__ import annotations

import re
from datetime import datetime
from typing import Literal

from pydantic import BaseModel, field_validator

LOCATION_PATTERN = re.compile(r"^[a-zA-Z0-9_-]+$")


class Memory(BaseModel):
    id: str  # UUID v4
    content: str  # Verbatim content
    wing: str
    hall: str | None = None
    room: str | None = None
    created_at: datetime
    reinforcement_count: int = 0
    last_reinforced_at: datetime

    @field_validator("wing", "hall", "room", mode="before")
    @classmethod
    def validate_location(cls, v: object) -> object:
        if v is not None and not LOCATION_PATTERN.match(str(v)):
            raise ValueError(f"Location value '{v}' contains invalid characters")
        return v

    @field_validator("content")
    @classmethod
    def validate_content(cls, v: str) -> str:
        if not v or not v.strip():
            raise ValueError("Content cannot be empty or whitespace-only")
        return v


class AddResult(BaseModel):
    id: str
    operation: Literal["created", "reinforced"]


class SearchResult(BaseModel):
    id: str
    content: str
    wing: str
    hall: str | None
    room: str | None
    salience_score: float
    reinforcement_count: int
    last_reinforced_at: datetime


class CorrelationId(BaseModel):
    """Links a Rust SQLite belief to its ChromaDB embedding and KG entities (D-03).

    Stored as ChromaDB metadata field 'rust_belief_id' on the embedding.
    Used to propagate revocation from the Rust core to memupalace (D-03).
    """

    rust_belief_id: str  # UUID from Rust SQLite (source of truth)
    chroma_id: str  # ChromaDB UUID returned by store.add()
    kg_entity_ids: list[str] = []  # 0..N KG entity IDs (populated when KG is active)


class MemupalaceSettings(BaseModel):
    chroma_path: str = "db/memupalace/chroma"
    sqlite_path: str = "db/memupalace/knowledge.db"
    onnx_model_path: str = "models/embedder.onnx"
    recency_decay_days: int = 30
    duplicate_threshold: float = 0.95
    tokenizer_name: str = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
    core_gateway_url: str = "http://core:3000/api/infer"
    insight_cache_ttl: int = 3600

    @field_validator("duplicate_threshold")
    @classmethod
    def validate_duplicate_threshold(cls, v: float) -> float:
        if not (0.0 <= v <= 1.0):
            raise ValueError(
                f"duplicate_threshold must be in range [0.0, 1.0], got {v}"
            )
        return v

    @field_validator("recency_decay_days")
    @classmethod
    def validate_recency_decay_days(cls, v: int) -> int:
        if v < 1:
            raise ValueError(
                f"recency_decay_days must be at least 1, got {v}"
            )
        return v

    @classmethod
    def from_env(cls) -> "MemupalaceSettings":
        """Load settings from environment variables with defaults."""
        import os

        return cls(
            chroma_path=os.getenv("MEMUPALACE_CHROMA_PATH", "db/memupalace/chroma"),
            sqlite_path=os.getenv("MEMUPALACE_SQLITE_PATH", "db/memupalace/knowledge.db"),
            onnx_model_path=os.getenv("MEMUPALACE_ONNX_MODEL_PATH", "models/embedder.onnx"),
            recency_decay_days=int(os.getenv("MEMUPALACE_RECENCY_DECAY_DAYS", "30")),
            duplicate_threshold=float(os.getenv("MEMUPALACE_DUPLICATE_THRESHOLD", "0.95")),
            tokenizer_name=os.getenv(
                "MEMUPALACE_TOKENIZER_NAME",
                "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2",
            ),
            core_gateway_url=os.getenv("CORE_GATEWAY_URL", "http://core:3000/api/infer"),
            insight_cache_ttl=int(os.getenv("MEMUPALACE_INSIGHT_CACHE_TTL", "3600")),
        )
