# syntax=docker/dockerfile:1
# Agent Platform — unified image (FastAPI backend + Flow UI static assets).
#
# Build from this repo root:
#   docker build -f Dockerfile -t agent-platform:latest .
#
# Run modes (AGENT_PLATFORM_CONTAINER_MODE):
#   backend — API + config UI on :18410
#   ui      — Flow UI static on :18408 (set VITE_API_ORIGIN at build for external API)
#   all     — both in one container; nginx on :18408 proxies API to uvicorn on :18410

FROM node:22-bookworm-slim AS web-build

RUN corepack enable && corepack prepare pnpm@10 --activate

WORKDIR /web

COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY web/ ./

ARG VITE_API_ORIGIN=
ARG VITE_AGENT_PLATFORM_MASTER_KEY=
ENV VITE_API_ORIGIN=$VITE_API_ORIGIN
ENV VITE_AGENT_PLATFORM_MASTER_KEY=$VITE_AGENT_PLATFORM_MASTER_KEY

RUN pnpm run build

FROM python:3.11-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends nginx \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY app/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app/ .
COPY docker/entrypoint.sh /entrypoint.sh
COPY docker/nginx-ui-static.conf /etc/nginx/agent-platform-ui-static.conf
COPY docker/nginx-ui-gateway.conf /etc/nginx/agent-platform-ui-gateway.conf

COPY --from=web-build /web/dist /usr/share/nginx/html/app

RUN chmod +x /entrypoint.sh \
    && mkdir -p /app/data/llm /var/run/nginx

EXPOSE 18410 18408

ENTRYPOINT ["/entrypoint.sh"]
