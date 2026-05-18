# syntax=docker/dockerfile:1.6
#
# Multi-stage build for any oracle family. The image is cluster-agnostic:
# the same image runs against devnet, mainnet, or testnet — runtime
# config (env file) chooses the cluster.
#
# Build:
#   cd /path/to/oracles
#   docker build -f scripts/docker/Dockerfile \
#       --build-arg FAMILY=onchain-transfer \
#       -t oracle-onchain-transfer:$(git rev-parse --short HEAD) .
#
# Run via the systemd units under scripts/docker/ — never `docker run`
# the image directly in production.

ARG FAMILY=onchain-transfer
ARG RUST_VERSION=1.92.0

# ----- Builder stage ---------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-bookworm AS builder

# postgres-openssl + openssl crates need libssl-dev + pkg-config to find
# the system OpenSSL. No perl needed — we link against system OpenSSL,
# not vendored.
RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the workspace. The workspace-root .dockerignore keeps target/, .git/,
# .kiro/, and any keypair files out of the build context.
COPY . .

# Re-declare ARG so it's visible inside this stage.
ARG FAMILY

# Build the family-specific binary in release mode. The workspace's
# Cargo.toml [profile.release] handles LTO + strip for a small final binary.
RUN cargo build --release -p oracle-${FAMILY}

# ----- Runtime stage ---------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Runtime needs only libssl3 + ca-certificates for postgres TLS and any
# HTTPS evidence-registry fetches.
RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl3 \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ARG FAMILY
COPY --from=builder /build/target/release/oracle-${FAMILY} /usr/local/bin/oracle

# Container runs as root inside the rootfs. Why this is fine:
#
# 1. The systemd unit invokes `docker run` with `--read-only`, so the
#    rootfs is immutable; only a 64 MiB tmpfs at /tmp is writable.
# 2. The keypair is the only host-mounted path, mounted `:ro`.
# 3. `--network host` shares the host's network namespace, so a
#    non-root container UID adds no firewall benefit.
# 4. The threat model is "the binary panics," not "the binary is
#    malicious." Trusted code in a sealed read-only container with no
#    writable mounts has no meaningful escape vector from a root UID.
#
# Dropping to a non-root UID would force the host's keypair file to be
# group-readable by a synthetic GID 65532 — measurable complexity for
# zero practical security gain in this model. Future operators who want
# rootless containers can swap this with userns-remap or Podman.
LABEL org.opencontainers.image.title="oracle-${FAMILY}" \
      org.opencontainers.image.source="https://github.com/miraland-labs/oracles" \
      org.opencontainers.image.licenses="Apache-2.0"

# All configuration comes from --env-file at `docker run` time; no CMD args.
ENTRYPOINT ["/usr/local/bin/oracle"]
