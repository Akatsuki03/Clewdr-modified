# ── Stage 1: Frontend build ─────────────────────────────────────────────────
FROM node:22-slim AS frontend-builder
WORKDIR /build/frontend
RUN npm install -g pnpm
COPY frontend/ .
# --frozen-lockfile ensures reproducible installs
RUN pnpm install --frozen-lockfile && pnpm run build

# ── Stage 2: Rust dependency cache (cargo-chef) ──────────────────────────────
FROM docker.io/lukemathwalker/cargo-chef:latest-rust-trixie AS chef
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: Rust build ───────────────────────────────────────────────────────
FROM chef AS backend-builder
# CARGO_INCREMENTAL=0: no benefit inside Docker layers, speeds up builds
ARG CARGO_INCREMENTAL=0
ARG CARGO_NET_RETRY=5
ARG CARGO_HTTP_TIMEOUT=120
ENV CARGO_INCREMENTAL=${CARGO_INCREMENTAL} \
    CARGO_NET_RETRY=${CARGO_NET_RETRY} \
    CARGO_HTTP_TIMEOUT=${CARGO_HTTP_TIMEOUT}

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    clang \
    libclang-dev \
    perl \
    pkg-config \
    upx-ucl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=planner /build/recipe.json recipe.json
# Cache layer: build only dependency crates (with mimalloc)
RUN cargo chef cook --release --no-default-features \
    --features embed-resource,xdg,mimalloc \
    --recipe-path recipe.json

# Build the actual application
COPY . .
COPY --from=frontend-builder /build/static/ ./static
# BUG FIX: mimalloc was missing here in the original Dockerfile
# (cook had it, build didn't → allocator wasn't actually enabled)
RUN cargo build --release --no-default-features \
    --features embed-resource,xdg,mimalloc \
    --bin clewdr \
    && upx --best --lzma ./target/release/clewdr \
    && install -Dm755 ./target/release/clewdr /build/clewdr \
    && mkdir -p /etc/clewdr/log \
    && touch /etc/clewdr/clewdr.toml

# ── Stage 4: Minimal runtime image ───────────────────────────────────────────
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libgcc-s1 \
    libstdc++6 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for security
RUN groupadd -r clewdr && useradd -r -g clewdr -d /etc/clewdr clewdr

COPY --from=backend-builder /build/clewdr        /usr/local/bin/clewdr
COPY --from=backend-builder /etc/clewdr          /etc/clewdr
COPY docker-entrypoint.sh                        /usr/local/bin/docker-entrypoint.sh

# Set ownership before VOLUME so the mount inherits correct permissions
RUN chmod +x /usr/local/bin/docker-entrypoint.sh \
    && chown -R clewdr:clewdr /etc/clewdr

# Default env — all overridable at runtime via Zeabur dashboard or docker -e
ENV CLEWDR_IP=0.0.0.0
ENV CLEWDR_PORT=8484
ENV CLEWDR_CHECK_UPDATE=FALSE
ENV CLEWDR_AUTO_UPDATE=FALSE
# NOTE: Do NOT set CLEWDR_PASSWORD or CLEWDR_ADMIN_PASSWORD here.
# Baking in empty strings would cause figment to override the persisted TOML
# password with "", triggering random password regeneration on every restart.
# Instead, set these ONLY in the Zeabur dashboard (or via docker -e) when you
# want a fixed password. If unset, the password from /etc/clewdr/clewdr.toml
# (generated on first start) persists across restarts via the volume.

# Zeabur (and many platforms) inject $PORT at runtime.
# docker-entrypoint.sh maps PORT → CLEWDR_PORT automatically.
EXPOSE 8484

# Health check: any HTTP response (even 401) means the server is alive.
# curl exits non-zero only on connection refused / timeout.
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD ["sh", "-c", "curl -s http://localhost:${CLEWDR_PORT:-8484}/v1/models -o /dev/null"]

VOLUME ["/etc/clewdr"]
USER clewdr
ENTRYPOINT ["docker-entrypoint.sh"]
