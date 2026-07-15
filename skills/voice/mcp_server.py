"""voice MCP server — local STT/TTS tools via streamable-http (VOICE-01).

Mirrors ``skills/memupalace/mcp_server.py`` in shape: a fastmcp server exposing two
tools over streamable-http on port 8004 (or ``VOICE_PORT``). All ML inference runs
in-process in this sidecar and the container has NO route to the public internet
(``bastion-net: internal`` in docker-compose.yml), so audio never leaves the host —
VOICE-01's core promise ("áudio nunca sai para a nuvem").

Tools:
    voice_transcribe(audio_b64) -> {"text": ...}
        whisper.cpp STT via pywhispercpp (default language: Brazilian Portuguese).
    voice_speak(text, voice) -> {"audio_b64": ..., "sample_rate": ...}
        Kokoro-82M TTS via kokoro-onnx (default voice: pf_dora, native pt-BR).

The heavy ML dependencies (``pywhispercpp``, ``kokoro_onnx``) are imported LAZILY
inside the engine getters — never at module import time. This lets the module (and
its tests, which inject mock engines into the ``_whisper``/``_kokoro`` globals) import
cleanly without the real packages or their build-time-baked model weights present.
"""

from __future__ import annotations

import base64
import io
import logging
import os
import struct
import tempfile
import wave
from typing import TYPE_CHECKING, Iterable

from fastmcp import FastMCP

if TYPE_CHECKING:  # resolved lazily at runtime — see _get_whisper/_get_kokoro
    from kokoro_onnx import Kokoro
    from pywhispercpp.model import Model

logger = logging.getLogger(__name__)

mcp = FastMCP("voice")

# Lazy singletons — initialized on first tool call (or injected by tests).
_whisper: Model | None = None
_kokoro: Kokoro | None = None


def _get_whisper() -> Model:
    """Return (or lazily load) the singleton whisper.cpp model.

    Default model size is ``small`` (good CPU speed/accuracy balance, supports pt-BR).
    Set ``VOICE_WHISPER_MODEL_SIZE=medium`` as an upgrade path if pt-BR WER is too high
    (10-RESEARCH.md Assumption A1). Weights are baked into the image at build time and
    loaded from ``VOICE_WHISPER_MODEL_DIR`` — no runtime download, no egress.
    """
    global _whisper
    if _whisper is None:
        from pywhispercpp.model import Model

        model_size = os.getenv("VOICE_WHISPER_MODEL_SIZE", "small")
        models_dir = os.getenv("VOICE_WHISPER_MODEL_DIR", "/models/whisper")
        _whisper = Model(model_size, models_dir=models_dir)
    return _whisper


def _get_kokoro() -> Kokoro:
    """Return (or lazily load) the singleton Kokoro TTS engine.

    Model + voice-style files are baked into the image at build time and loaded from
    ``VOICE_KOKORO_MODEL_PATH``/``VOICE_KOKORO_VOICES_PATH`` — no runtime download.
    """
    global _kokoro
    if _kokoro is None:
        from kokoro_onnx import Kokoro

        _kokoro = Kokoro(
            model_path=os.getenv(
                "VOICE_KOKORO_MODEL_PATH", "/models/kokoro/kokoro-v1.0.onnx"
            ),
            voices_path=os.getenv(
                "VOICE_KOKORO_VOICES_PATH", "/models/kokoro/voices-v1.0.bin"
            ),
        )
    return _kokoro


def _validate_str(name: str, value: object) -> str:
    """Guard: raises ValueError if value is not a non-empty, non-whitespace string."""
    if not isinstance(value, str) or not str(value).strip():
        raise ValueError(
            f"Parameter '{name}' must be a non-empty, non-whitespace string."
        )
    return str(value)


def _pcm_float_to_wav(samples: Iterable[float], sample_rate: int) -> bytes:
    """Encode float PCM samples in [-1.0, 1.0] to a 16-bit mono WAV byte buffer.

    Works on any iterable of floats (a numpy float32 array at runtime, a plain list in
    tests) — no numpy import here, keeping the module and its tests dependency-light.
    """
    frames = bytearray()
    for sample in samples:
        clamped = max(-1.0, min(1.0, float(sample)))
        frames += struct.pack("<h", int(round(clamped * 32767.0)))
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(int(sample_rate))
        wav.writeframes(bytes(frames))
    return buffer.getvalue()


# ---------------------------------------------------------------------------
# Tool: voice_transcribe
# ---------------------------------------------------------------------------


@mcp.tool()
def voice_transcribe(audio_b64: str) -> dict:
    """Transcribe a base64-encoded WAV clip to text via whisper.cpp (STT).

    Runs entirely in this local sidecar — the audio never leaves the host. Language
    defaults to Brazilian Portuguese (``VOICE_WHISPER_LANG``, default ``pt``).
    """
    _validate_str("audio_b64", audio_b64)
    audio_bytes = base64.b64decode(audio_b64)
    language = os.getenv("VOICE_WHISPER_LANG", "pt")
    with tempfile.NamedTemporaryFile(suffix=".wav") as tmp:
        tmp.write(audio_bytes)
        tmp.flush()
        segments = _get_whisper().transcribe(tmp.name, language=language)
    text = "".join(segment.text for segment in segments).strip()
    return {"text": text}


# ---------------------------------------------------------------------------
# Tool: voice_speak
# ---------------------------------------------------------------------------


@mcp.tool()
def voice_speak(text: str, voice: str = "pf_dora") -> dict:
    """Synthesize speech from text via Kokoro (TTS), returning a base64 WAV clip.

    Runs entirely in this local sidecar — nothing leaves the host. ``voice`` defaults
    to a native Brazilian Portuguese voice (``pf_dora``); the phonemization language
    defaults to ``pt-br`` (``VOICE_KOKORO_LANG`` — an espeak-ng code passed straight to
    kokoro-onnx's phonemizer). Returns 16-bit PCM mono WAV at the engine's own sample
    rate (Kokoro-82M emits 24000 Hz).
    """
    _validate_str("text", text)
    _validate_str("voice", voice)
    language = os.getenv("VOICE_KOKORO_LANG", "pt-br")
    samples, sample_rate = _get_kokoro().create(text, voice=voice, lang=language)
    wav_bytes = _pcm_float_to_wav(samples, sample_rate)
    return {
        "audio_b64": base64.b64encode(wav_bytes).decode("ascii"),
        "sample_rate": int(sample_rate),
    }


# ---------------------------------------------------------------------------
# Entrypoint
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    port = int(os.getenv("VOICE_PORT", "8004"))
    mcp.run(transport="streamable-http", host="0.0.0.0", port=port)
