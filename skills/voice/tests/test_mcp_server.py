"""Tests for the voice sidecar fastmcp tools — mocked STT/TTS engines (VOICE-01).

Calls the fastmcp tool functions directly, injecting mock whisper/Kokoro engines via
the module-level ``_whisper``/``_kokoro`` globals (mirroring memupalace's ``_inject_mp``
pattern), so no real model download or ML inference happens in CI.
"""

from __future__ import annotations

import asyncio
import base64
import io
import wave

import pytest

import skills.voice.mcp_server as srv
from skills.voice.mcp_server import voice_speak, voice_transcribe


def _fake_audio_b64() -> str:
    """Return base64 for arbitrary bytes — the mock whisper engine ignores the content."""
    return base64.b64encode(b"RIFF-fake-wav-bytes").decode("ascii")


# ---------------------------------------------------------------------------
# Tool surface
# ---------------------------------------------------------------------------


def test_mcp_server_exposes_exactly_two_tools() -> None:
    """The fastmcp server must expose exactly voice_transcribe and voice_speak."""
    tools = asyncio.run(srv.mcp.list_tools())
    assert {t.name for t in tools} == {"voice_transcribe", "voice_speak"}


# ---------------------------------------------------------------------------
# voice_transcribe
# ---------------------------------------------------------------------------


def test_voice_transcribe_returns_joined_text(inject_engines) -> None:
    """voice_transcribe joins the whisper segments and returns {"text": ...}."""
    result = voice_transcribe(audio_b64=_fake_audio_b64())
    assert result == {"text": "olá mundo"}


def test_voice_transcribe_uses_pt_language_by_default(inject_engines) -> None:
    """voice_transcribe must pass language="pt" to the whisper engine (pt-BR default)."""
    mock_whisper, _ = inject_engines
    voice_transcribe(audio_b64=_fake_audio_b64())
    mock_whisper.transcribe.assert_called_once()
    assert mock_whisper.transcribe.call_args.kwargs.get("language") == "pt"


def test_voice_transcribe_empty_raises(inject_engines) -> None:
    """voice_transcribe with an empty string must raise ValueError."""
    with pytest.raises(ValueError, match="non-empty"):
        voice_transcribe(audio_b64="")


def test_voice_transcribe_whitespace_raises(inject_engines) -> None:
    """voice_transcribe with a whitespace-only string must raise ValueError."""
    with pytest.raises(ValueError, match="non-empty"):
        voice_transcribe(audio_b64="   ")


# ---------------------------------------------------------------------------
# voice_speak
# ---------------------------------------------------------------------------


def test_voice_speak_returns_valid_wav(inject_engines) -> None:
    """voice_speak returns a base64 16-bit mono WAV at the engine's sample rate."""
    result = voice_speak(text="olá mundo")
    assert result["sample_rate"] == 24000
    wav_bytes = base64.b64decode(result["audio_b64"])
    with wave.open(io.BytesIO(wav_bytes), "rb") as wav:
        assert wav.getnchannels() == 1
        assert wav.getsampwidth() == 2
        assert wav.getframerate() == 24000
        assert wav.getnframes() == 5  # 5 mock samples -> 5 frames


def test_voice_speak_defaults_to_ptbr_voice_and_lang(inject_engines) -> None:
    """voice_speak defaults to the pt-BR voice pf_dora and lang pt-br."""
    _, mock_kokoro = inject_engines
    voice_speak(text="olá")
    mock_kokoro.create.assert_called_once()
    call = mock_kokoro.create.call_args
    assert call.kwargs.get("voice") == "pf_dora"
    assert call.kwargs.get("lang") == "pt-br"


def test_voice_speak_honours_custom_voice(inject_engines) -> None:
    """voice_speak forwards an explicit voice id to the Kokoro engine."""
    _, mock_kokoro = inject_engines
    voice_speak(text="olá", voice="pm_santa")
    assert mock_kokoro.create.call_args.kwargs.get("voice") == "pm_santa"


def test_voice_speak_empty_raises(inject_engines) -> None:
    """voice_speak with empty text must raise ValueError."""
    with pytest.raises(ValueError, match="non-empty"):
        voice_speak(text="")
