# syntax=docker/dockerfile:1

FROM rust:bookworm AS builder

WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        perl \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY . .

# Headless Server-only image; Runner is a separate native service.
# Configuration comes from server.toml or the backward-compatible env.
ARG WEBCODEX_GIT_COMMIT
ARG WEBCODEX_GIT_DIRTY
ARG WEBCODEX_BUILT_AT
RUN WEBCODEX_GIT_COMMIT="$WEBCODEX_GIT_COMMIT" \
    WEBCODEX_GIT_DIRTY="$WEBCODEX_GIT_DIRTY" \
    WEBCODEX_BUILT_AT="$WEBCODEX_BUILT_AT" \
    cargo build --locked --release --bin webcodex-server -p webcodex

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        libgcc-s1 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 webcodex \
    && useradd --system --uid 10001 --gid webcodex \
        --home-dir /var/lib/webcodex webcodex \
    && install -d -o webcodex -g webcodex -m 0700 /var/lib/webcodex

COPY --from=builder /src/target/release/webcodex-server /usr/local/bin/webcodex-server

ENV WEBCODEX_ADDR=0.0.0.0:8080 \
    WEBCODEX_DATA=/var/lib/webcodex \
    RUST_LOG=info

USER webcodex:webcodex
WORKDIR /var/lib/webcodex

EXPOSE 8080
VOLUME ["/var/lib/webcodex"]

HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=5 \
    CMD curl -fsS http://127.0.0.1:8080/healthz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/webcodex-server"]
