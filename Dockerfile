# syntax=docker/dockerfile:1

FROM rust:bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Core crates are pinned Git dependencies, so this context is fully standalone.
COPY Cargo.toml Cargo.lock ./
RUN cargo fetch --locked

COPY src ./src
# routes.rs embeds the frozen OpenAPI contract via include_str!.
COPY docs/en/contracts ./docs/en/contracts
# Keep source builds usable on developer machines with limited RAM. CI or
# high-core builders can override this with --build-arg CARGO_BUILD_JOBS=N.
ARG CARGO_BUILD_JOBS=2
RUN cargo build --jobs "${CARGO_BUILD_JOBS}" --locked --release --no-default-features --features channels-extra,mcp-server --bin bastion

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libssl3 nodejs npm \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 bastion \
    && useradd --uid 1000 --gid bastion --create-home --shell /bin/bash bastion \
    && npm install --global acpx @anthropic-ai/claude-code @openai/codex opencode-ai

COPY --from=builder /build/target/release/bastion /usr/local/bin/bastion

USER bastion:bastion
ENV HOME=/home/bastion
EXPOSE 8080 3000
HEALTHCHECK --interval=15s --timeout=5s --start-period=15s --retries=5 \
    CMD curl --fail --silent http://127.0.0.1:8080/healthz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/bastion"]
CMD ["daemon"]
