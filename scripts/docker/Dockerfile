# syntax=docker/dockerfile:1.6
#
# Multi-stage build for any oracle family. Cluster is selected at build time
# via CARGO_FEATURES (devnet → `sla-escrow-api/devnet` compile-time program id).
# Deploy one image per family per cluster on dual-cluster hosts, e.g.:
#   oracle-onchain-transfer-devnet:current
#   oracle-onchain-transfer-mainnet:current
#   oracle-file-delivery-devnet:current
#
# Build (devnet example):
#   cd /path/to/oracles
#   docker build -f scripts/docker/Dockerfile \
#       --build-arg FAMILY=onchain-transfer \
#       --build-arg CARGO_FEATURES=devnet \
#       -t oracle-onchain-transfer-devnet:$(git rev-parse --short HEAD) .
#   docker build -f scripts/docker/Dockerfile \
#       --build-arg FAMILY=file-delivery \
#       --build-arg CARGO_FEATURES=devnet \
#       -t oracle-file-delivery-devnet:$(git rev-parse --short HEAD) .
#
# Run via the systemd units under scripts/docker/ — never `docker run`
# the image directly in production.

ARG FAMILY=onchain-transfer
ARG CARGO_FEATURES=
ARG RUST_VERSION=1.92.0

# ----- Builder stage ---------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY . .

ARG FAMILY
ARG CARGO_FEATURES

RUN if [ -n "$CARGO_FEATURES" ]; then \
        echo "[build] oracle-${FAMILY} --features ${CARGO_FEATURES}"; \
        cargo build --release -p "oracle-${FAMILY}" --features "${CARGO_FEATURES}"; \
    else \
        echo "[build] oracle-${FAMILY} (mainnet/default program id)"; \
        cargo build --release -p "oracle-${FAMILY}"; \
    fi

# ----- Runtime stage ---------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl3 \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ARG FAMILY
COPY --from=builder /build/target/release/oracle-${FAMILY} /usr/local/bin/oracle

LABEL org.opencontainers.image.title="oracle-${FAMILY}" \
      org.opencontainers.image.source="https://github.com/miraland-labs/oracles" \
      org.opencontainers.image.licenses="Apache-2.0"

ENTRYPOINT ["/usr/local/bin/oracle"]
