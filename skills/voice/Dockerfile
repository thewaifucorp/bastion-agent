# voice MCP server — local STT (whisper.cpp) + TTS (Kokoro), zero egress at runtime (VOICE-01/D-11)
# Transport: streamable-http, port 8004 (internal Docker network only)
FROM python:3.12-slim

# Non-root user — matches docker-compose.yml's user: "1001:1001" convention (D-02/MCP-03)
RUN groupadd -g 1001 bastion && useradd -u 1001 -g bastion -m bastion

WORKDIR /app

# Layer cache: deps first
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Bake both ML models at BUILD time (never at runtime). The running container has NO
# internet route (bastion-net: internal), so every weight must live inside the image —
# this is the actual enforcement of VOICE-01's "áudio nunca sai para a nuvem".
# whisper.cpp ggml-small model (pt-BR capable) via pywhispercpp's own download helper:
RUN python -c "from pywhispercpp.utils import download_model; download_model('small', download_dir='/models/whisper')"
# Kokoro-82M ONNX model + voice styles (ships native pt-BR voices: pf_dora/pm_santa/pm_rocha):
RUN python -c "import os, urllib.request; os.makedirs('/models/kokoro', exist_ok=True); [urllib.request.urlretrieve(u, os.path.join('/models/kokoro', u.rsplit('/', 1)[1])) for u in ('https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx', 'https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin')]"
# NOTE (10-RESEARCH.md Pitfall 6): deliberately NO OS-level espeak-ng package here.
# kokoro-onnx's pip deps (espeakng-loader + phonemizer-fork) bundle a working espeak-ng
# shared library; a duplicate system-wide install has caused version-incompatibility
# crashes in this exact class of project.

# Copy code under its package path so the absolute `skills.voice.*` imports resolve
# in-container (skills/ is a PEP-420 namespace package; voice/ carries its own
# __init__.py). Run via `python -m` below.
COPY . ./skills/voice/
RUN chown -R bastion:bastion /app /models

# Healthcheck: TCP liveness — fastmcp /mcp returns 406 to a plain GET, so an open port
# means alive. start-period 45s (> memupalace's 30s): loading two ML models is slower.
HEALTHCHECK --interval=10s --timeout=5s --start-period=45s --retries=3 \
    CMD python -c "import socket,sys; s=socket.socket(); s.settimeout(4); sys.exit(s.connect_ex(('localhost',8004)))" || exit 1

USER bastion

# Port 8004 — internal Docker network only (never exposed to host)
EXPOSE 8004

# Env defaults (overridden by docker-compose). Offline flags guarantee zero HF egress
# at runtime; models were already baked in the RUN steps above.
ENV VOICE_PORT=8004
ENV VOICE_WHISPER_MODEL_DIR=/models/whisper
ENV VOICE_WHISPER_MODEL_SIZE=small
ENV VOICE_KOKORO_MODEL_PATH=/models/kokoro/kokoro-v1.0.onnx
ENV VOICE_KOKORO_VOICES_PATH=/models/kokoro/voices-v1.0.bin
ENV HF_HUB_OFFLINE=1
ENV TRANSFORMERS_OFFLINE=1
ENV PYTHONPATH=/app

CMD ["python", "-m", "skills.voice.mcp_server"]
