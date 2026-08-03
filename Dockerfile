# syntax=docker/dockerfile:1
# Agent Platform — backend-only image (FastAPI API; no browser UI).
#
# Build from this repo root:
#   docker build -f Dockerfile -t agent-platform:latest .

FROM python:3.11-slim AS runtime

WORKDIR /app

COPY app/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app/ .
COPY docker/entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh \
    && mkdir -p /app/data/llm

EXPOSE 18410

ENTRYPOINT ["/entrypoint.sh"]
