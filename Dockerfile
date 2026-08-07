# syntax=docker/dockerfile:1
# Agent Platform — backend-only image (the API server; the desktop app is the UI).
#
# Build from this repo root:
#   docker build -f Dockerfile -t agent-platform:latest .
#
# This was a `python:3.11-slim` image that installed FastAPI and copied `app/`.
# The server is Rust now (`agent-platformd`, ADR 0007), so the image is a build
# stage plus a runtime with a single static-ish binary in it — and the runtime
# carries no interpreter at all.
#
# **The model-ops build pipeline is not in this image.** It needs torch and a
# GPU, it always ran as its own process, and `Dockerfile.train` is where that
# lives. Point `MODEL_OPS_PYTHON` and `MODEL_OPS_WORKER_PATH` at an interpreter
# and a copy of `worker/` if you want build jobs to run from a container.

FROM rust:1-bookworm AS build

WORKDIR /src

# cmake and libclang are for the optional local-inference feature; the default
# build does not link llama.cpp, but the workspace still configures for it.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake libclang-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY desktop/Cargo.toml desktop/Cargo.lock ./desktop/
COPY desktop/crates ./desktop/crates

# Only the server: `agent-platform-desktop` is an iced GUI and must not be in a
# cloud image, which is the constraint that ruled out the one-binary shape in
# ADR 0006.
RUN cargo build --release --manifest-path desktop/Cargo.toml -p agent-platform-server

FROM debian:bookworm-slim AS runtime

WORKDIR /app

# ca-certificates: every upstream provider call is HTTPS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /app/data/llm

COPY --from=build /src/desktop/target/release/agent-platformd /usr/local/bin/agent-platformd
COPY config/ /app/config/
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 18410

ENTRYPOINT ["/entrypoint.sh"]
