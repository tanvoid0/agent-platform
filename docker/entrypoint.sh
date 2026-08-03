#!/bin/sh
set -eu

WORKERS="${AGENT_PLATFORM_UVICORN_WORKERS:-1}"
DB_PATH="${AGENT_PLATFORM_DB_PATH:-/app/data/agent_platform.db}"
KEEPALIVE="${AGENT_PLATFORM_UVICORN_KEEPALIVE_SECONDS:-30}"
API_PORT="${AGENT_PLATFORM_API_PORT:-18410}"

case "$DB_PATH" in
  *.db|*.sqlite|*.sqlite3)
    if [ "$WORKERS" != "1" ]; then
      echo "WARNING: SQLite ($DB_PATH) — clamping uvicorn workers to 1 ($WORKERS requested). Set AGENT_PLATFORM_UVICORN_WORKERS=1 or use Postgres for multi-worker."
      WORKERS=1
    fi
    ;;
esac

exec uvicorn main:app \
  --host 0.0.0.0 \
  --port "$API_PORT" \
  --workers "$WORKERS" \
  --timeout-keep-alive "$KEEPALIVE"
