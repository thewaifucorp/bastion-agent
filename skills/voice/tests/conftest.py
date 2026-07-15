"""Pytest fixtures for the voice sidecar — inject mock STT/TTS engines.

Mirrors memupalace's ``_inject_mp`` pattern: mocks are wired into ``mcp_server``'s
module-level ``_whisper``/``_kokoro`` singletons, so the tools run without the real
``pywhispercpp``/``kokoro-onnx`` packages or their build-time-baked model weights —
no model download, no ML inference at CI time.
"""

from __future__ import annotations

from typing import Iterator
from unittest.mock import MagicMock

import pytest

import skills.voice.mcp_server as srv


class _FakeSegment:
    """Minimal stand-in for a pywhispercpp Segment (only ``.text`` is used)."""

    def __init__(self, text: str) -> None:
        self.text = text


@pytest.fixture
def mock_whisper() -> MagicMock:
    """Mock whisper model whose transcribe() returns two fixed pt-BR segments."""
    engine = MagicMock()
    engine.transcribe.return_value = [_FakeSegment("olá "), _FakeSegment("mundo")]
    return engine


@pytest.fixture
def mock_kokoro() -> MagicMock:
    """Mock Kokoro engine whose create() returns a tiny float32 PCM clip + 24 kHz rate."""
    engine = MagicMock()
    # create(text, voice=..., lang=...) -> (samples: iterable[float], sample_rate: int)
    engine.create.return_value = ([0.0, 0.5, -0.5, 1.0, -1.0], 24000)
    return engine


@pytest.fixture(autouse=True)
def _reset_engines() -> Iterator[None]:
    """Reset the module singletons around every test so nothing leaks between tests."""
    srv._whisper = None
    srv._kokoro = None
    yield
    srv._whisper = None
    srv._kokoro = None


@pytest.fixture
def inject_engines(mock_whisper: MagicMock, mock_kokoro: MagicMock) -> tuple[MagicMock, MagicMock]:
    """Wire both mock engines into the mcp_server singletons (the ``_inject_mp`` analogue)."""
    srv._whisper = mock_whisper
    srv._kokoro = mock_kokoro
    return mock_whisper, mock_kokoro
