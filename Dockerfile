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
# Keep source builds usable on developer machines with limited RAM. CI or
# high-core builders can override this with --build-arg CARGO_BUILD_JOBS=N.
ARG CARGO_BUILD_JOBS=2
RUN cargo build --jobs "${CARGO_BUILD_JOBS}" --locked --release --no-default-features --features channels-extra,mcp-server --bin bastion

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 bastion \
    && useradd --uid 1000 --gid bastion --no-create-home --shell /usr/sbin/nologin bastion

COPY --from=builder /build/target/release/bastion /usr/local/bin/bastion

USER bastion:bastion
EXPOSE 8080 3000
HEALTHCHECK --interval=15s --timeout=5s --start-period=15s --retries=5 \
    CMD curl --fail --silent http://127.0.0.1:8080/healthz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/bastion"]
CMD ["daemon"]
