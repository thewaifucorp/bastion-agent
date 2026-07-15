# syntax=docker/dockerfile:1
# Bastion v3 — Multi-stage static build, scratch final image (PKG-01, PKG-03, D-06)
# Stage 1: builder — rust:alpine + musl toolchain
# Stage 2: scratch image — zero OS overhead, static binary only
#
# Replaces the v2 `FROM ghcr.io/openclaw/openclaw` image (200MB+ Node runtime)
# with a single static Rust binary in a scratch container.

# ── Stage 1: builder ──────────────────────────────────────────────────────────
FROM rust:alpine AS builder

# musl-dev: musl libc headers; gcc: C compiler for rusqlite's bundled SQLite;
# ca-certificates: SSL bundle copied into the scratch stage;
# binutils: provides `readelf` for the static-linking gate below.
# NOTE: `musl-tools` is a Debian package and does NOT exist on Alpine — on
# rust:alpine the musl target is built with `musl-dev` + `gcc`.
RUN apk add --no-cache musl-dev gcc ca-certificates binutils

# Register the musl target (static linking target).
RUN rustup target add x86_64-unknown-linux-musl

# Force a fully static binary (crt-static). The musl target defaults to crt-static,
# but the rust:alpine image sets RUSTFLAGS via cargo config / CARGO_ENCODED_RUSTFLAGS,
# which silently DROPPED our setting and produced a libc-dynamic binary. We therefore
# set the flag at the highest-precedence source — CARGO_ENCODED_RUSTFLAGS (US-separated,
# \037) — directly on each build invocation, so no image-level config can override it.

WORKDIR /build

# Copy manifests first — this layer is cached if no deps change.
COPY Cargo.toml Cargo.lock ./

# Stub src for dependency pre-cache (avoids re-compiling deps on code-only changes).
RUN mkdir src && echo 'fn main(){}' > src/main.rs && \
    CARGO_ENCODED_RUSTFLAGS="$(printf '%s\037%s' '-C' 'target-feature=+crt-static')" \
    cargo build --release --target x86_64-unknown-linux-musl 2>/dev/null || true; \
    rm -rf src

# Copy actual source.
COPY src ./src

# Force rebuild of src (cargo detects the manifest/source timestamp).
RUN touch src/main.rs && \
    CARGO_ENCODED_RUSTFLAGS="$(printf '%s\037%s' '-C' 'target-feature=+crt-static')" \
    cargo build --release --target x86_64-unknown-linux-musl

# Verify the binary will run in FROM scratch — fail the build if it needs a loader.
# A PT_INTERP program header means the kernel requires an external dynamic loader
# (/lib/ld-musl-*.so.1), which scratch does NOT provide. A fully static binary (or a
# static-PIE) has NO PT_INTERP. This check is libc- and PIE-agnostic, unlike the glibc
# `ldd` "not a dynamic executable" string (musl's ldd never prints that phrase).
RUN BIN=target/x86_64-unknown-linux-musl/release/bastion; \
    readelf -l "$BIN" | grep -E "INTERP|NEEDED" || true; \
    if readelf -l "$BIN" | grep -q INTERP; then \
        echo "ERROR: binary has PT_INTERP — dynamically linked, will NOT run in scratch"; exit 1; \
    fi; \
    echo "OK: no PT_INTERP — static binary, scratch-ready"

# ── Stage 2: scratch — zero OS layer ──────────────────────────────────────────
FROM scratch

# SSL certificates required for HTTPS (Anthropic, OpenAI, Telegram long-poll).
# The scratch image has no cert store — must be copied explicitly.
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# The static binary — the only executable in the image besides the cert bundle.
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/bastion /bastion

# Point reqwest/rustls at the cert bundle.
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

# Port for the /api/infer gateway (Phase 3, D-08).
EXPOSE 3000

# Volume ownership (PKG-08): the scratch image has no shell — no chown entrypoint script
# is possible. Permissions are resolved via docker-compose.yml
# `user: "${BASTION_UID:-1000}:${BASTION_GID:-1000}"`. Named volumes are initialized
# empty; the first write by the configured UID creates correct ownership — zero
# manual chmod needed.

# Loop 3-D (docs/revamp/C3-cloud-ready-design.md): this SAME image runs
# local (bastion.toml's shipped paths) and hosted-like (paths/secrets
# injected via env — BASTION__SESSION__DB_PATH, BASTION__LOGGING__LOG_PATH,
# BASTION_SECRETS_DIR, ...) without a rebuild; nothing in this Dockerfile
# bakes in a path or secret value. /healthz and /readyz are served on the
# SAME webhook port as /webhook (BASTION_WEBHOOK_ADDR, default 8080 in
# docker-compose.yml) — a k8s liveness/readinessProbe (httpGet, executed by
# the kubelet OUTSIDE the container) can use them directly. A Docker/Compose
# HEALTHCHECK cannot: that instruction execs a command INSIDE the container,
# and this scratch image deliberately has no shell/curl (PKG-01/PKG-03) —
# accepted trade-off, unchanged by this loop.

ENTRYPOINT ["/bastion"]
# Default: daemon mode (long-running with channels active).
CMD ["daemon"]
