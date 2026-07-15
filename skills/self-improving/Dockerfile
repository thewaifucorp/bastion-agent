# self-improving MCP server — observes usage, suggests skill improvements (SELF-01/SELF-02)
# Transport: streamable-http, port 8003 (internal Docker network only)
FROM python:3.12-slim

# Non-root user — MCP-03
RUN groupadd -g 1001 bastion && useradd -u 1001 -g bastion -m bastion

WORKDIR /app

# Layer cache: deps first
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy skill code
COPY . .
RUN chown -R bastion:bastion /app

# /skills mounted read-only — SELF never writes skills (D-10)
# /data for suggestions.jsonl queue (writable by container)
VOLUME ["/skills", "/data"]

HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD python -c "import socket,sys; s=socket.socket(); s.settimeout(4); sys.exit(s.connect_ex(('localhost',8003)))" || exit 1

USER bastion

# Port 8003 — internal Docker network only
EXPOSE 8003

# Env defaults (overridden by docker-compose)
ENV SELF_IMPROVING_PORT=8003
ENV SKILLS_DIR=/skills
ENV SELF_SUGGESTIONS_FILE=/data/suggestions.jsonl
ENV MEMUPALACE_URL=http://memupalace:8001/mcp

CMD ["python", "mcp_server.py"]
