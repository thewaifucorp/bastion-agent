"""Domain models for the proactive-engine skill."""

from __future__ import annotations

import uuid
from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, Field, field_validator

EventType = Literal["inactivity", "memory_staleness", "cve", "temporal_pattern"]


class DetectionEvent(BaseModel):
    id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    type: EventType
    persona: str
    payload: dict[str, Any]
    timestamp: datetime  # UTC, timezone-aware
    processed: bool = False

    @field_validator("timestamp")
    @classmethod
    def must_be_timezone_aware(cls, v: datetime) -> datetime:
        if v.tzinfo is None:
            raise ValueError("timestamp must be timezone-aware (UTC)")
        return v


class ProactiveSuggestion(BaseModel):
    id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    event_id: str | None  # None for emergent suggestions without a direct event
    text: str
    event_type: EventType | None
    persona: str
    timestamp: datetime  # UTC, timezone-aware
    model_used: str
    is_fallback: bool = False

    @field_validator("timestamp")
    @classmethod
    def must_be_timezone_aware(cls, v: datetime) -> datetime:
        if v.tzinfo is None:
            raise ValueError("timestamp must be timezone-aware (UTC)")
        return v
