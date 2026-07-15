# memupalace MCP server — ONNX local, zero egress cloud (D-02/MUPL-01)
# Transport: streamable-http, port 8001 (internal Docker network only)
FROM python:3.12-slim

# Non-root user — D-02/MCP-03
RUN groupadd -g 1001 bastion && useradd -u 1001 -g bastion -m bastion

WORKDIR /app

# Layer cache: deps first
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Download multilingual ONNX model at build time (not runtime)
# Ensures zero egress at runtime — model is baked into the image
RUN python -c "from huggingface_hub import snapshot_download; snapshot_download(repo_id='Qdrant/paraphrase-multilingual-MiniLM-L12-v2-onnx-Q', local_dir='/models', ignore_patterns=['*.msgpack','*.h5','*.bin'])"

# Copy code under its package path so the absolute `skills.memupalace.*` imports
# used throughout this module resolve in-container (skills/ is a PEP-420 namespace
# package; memupalace/ carries its own __init__.py). Run via `python -m` below.
COPY . ./skills/memupalace/
RUN chown -R bastion:bastion /app

# Healthcheck: ensure MCP endpoint responds
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=3 \
    CMD python -c "import socket,sys; s=socket.socket(); s.settimeout(4); sys.exit(s.connect_ex(('localhost',8001)))" || exit 1

USER bastion

# Port 8001 — internal Docker network only (never exposed to host)
EXPOSE 8001

# Env defaults (overridden by docker-compose)
ENV MEMUPALACE_PORT=8001
ENV MEMUPALACE_CHROMA_PATH=/db/memupalace/chroma
# model_optimized.onnx is the actual filename shipped by the Qdrant repo (was model_quantized.onnx
# → FileNotFoundError). Tokenizer loads from the local /models dir (baked above) + offline flags,
# so runtime makes ZERO HF calls — required for the "zero egress cloud" claim and to survive an
# SSL-inspecting proxy (Zscaler). These ENVs are AFTER the snapshot_download RUN, so build still fetches.
ENV MEMUPALACE_ONNX_MODEL_PATH=/models/model_optimized.onnx
ENV MEMUPALACE_TOKENIZER_NAME=/models
ENV HF_HUB_OFFLINE=1
ENV TRANSFORMERS_OFFLINE=1
ENV CORE_GATEWAY_URL=http://core:3000/api/infer
ENV PYTHONPATH=/app

CMD ["python", "-m", "skills.memupalace.mcp_server"]
