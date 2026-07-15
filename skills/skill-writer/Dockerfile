# skill-writer MCP server — cria e edita SKILL.md por NL (SKWR-01..06)
FROM python:3.12-slim

# Non-root user — MCP-03
RUN groupadd -g 1001 bastion && useradd -u 1001 -g bastion -m bastion

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . .
RUN chown -R bastion:bastion /app

# skills volume mounted at /skills (rw) — skill-writer writes SKILL.md and .versions/
# data volume at /data — pending_distillations.jsonl queue
VOLUME ["/skills", "/data"]

HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD python -c "import socket,sys; s=socket.socket(); s.settimeout(4); sys.exit(s.connect_ex(('localhost',8002)))" || exit 1

USER bastion
EXPOSE 8002

ENV SKILL_WRITER_PORT=8002
ENV SKILLS_DIR=/skills
ENV SKILL_WRITER_PENDING_FILE=/data/pending_distillations.jsonl
ENV CORE_GATEWAY_URL=http://core:3000/api/infer
ENV MEMUPALACE_URL=http://memupalace:8001/mcp

CMD ["python", "mcp_server.py"]
