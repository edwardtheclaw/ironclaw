# Multi-stage Dockerfile for the IronClaw agent (cloud deployment).
#
# Build:
#   docker build --platform linux/amd64 -t ironclaw:latest .
#
# Run:
#   docker run --env-file .env -p 3000:3000 ironclaw:latest

# Stage 1: Build
# Pin by digest to prevent silent regressions from upstream tag mutations (QW-09 / Ops-03).
FROM rust:1.92-slim-bookworm@sha256:f1f73538ebe623fd3673a35aff3df358ae1084c64c55646516e5b17b321b6c9b AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev cmake gcc g++ \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add wasm32-wasip2 \
    && cargo install wasm-tools

WORKDIR /app

# Copy manifests first for layer caching
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Copy source, build script, tests, and supporting directories
COPY build.rs build.rs
COPY src/ src/
COPY tests/ tests/
COPY migrations/ migrations/
COPY registry/ registry/
COPY channels-src/ channels-src/
COPY wit/ wit/
COPY providers.json providers.json
# [[bench]] entries in Cargo.toml require bench sources to exist for cargo to parse the manifest
COPY benches/ benches/

RUN cargo build --release --bin ironclaw

# Stage 2: Runtime
FROM debian:bookworm-slim@sha256:f06537653ac770703bc45b4b113475bd402f451e85223f0f2837acbf89ab020a

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ironclaw /usr/local/bin/ironclaw
COPY --from=builder /app/migrations /app/migrations

# Non-root user
RUN useradd -m -u 1000 -s /bin/bash ironclaw
USER ironclaw

EXPOSE 3000

ENV RUST_LOG=ironclaw=info

ENTRYPOINT ["ironclaw"]
