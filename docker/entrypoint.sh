#!/bin/sh
set -eu

MODE="${AGENT_PLATFORM_CONTAINER_MODE:-backend}"
WORKERS="${AGENT_PLATFORM_UVICORN_WORKERS:-1}"
DB_PATH="${AGENT_PLATFORM_DB_PATH:-/app/data/agent_platform.db}"
KEEPALIVE="${AGENT_PLATFORM_UVICORN_KEEPALIVE_SECONDS:-30}"
UI_PORT="${AGENT_PLATFORM_UI_PORT:-18408}"
API_PORT="${AGENT_PLATFORM_API_PORT:-18410}"

clamp_sqlite_workers() {
  case "$DB_PATH" in
    *.db|*.sqlite|*.sqlite3)
      if [ "$WORKERS" != "1" ]; then
        echo "WARNING: SQLite ($DB_PATH) — clamping uvicorn workers to 1 ($WORKERS requested). Set AGENT_PLATFORM_UVICORN_WORKERS=1 or use Postgres for multi-worker."
        WORKERS=1
      fi
      ;;
  esac
}

start_backend() {
  clamp_sqlite_workers
  exec uvicorn main:app \
    --host 0.0.0.0 \
    --port "$API_PORT" \
    --workers "$WORKERS" \
    --timeout-keep-alive "$KEEPALIVE"
}

start_ui_nginx() {
  conf="/etc/nginx/agent-platform-ui-static.conf"
  if [ "$MODE" = "all" ]; then
    conf="/etc/nginx/agent-platform-ui-gateway.conf"
  fi
  sed \
    -e "s/__UI_PORT__/${UI_PORT}/g" \
    -e "s/__API_PORT__/${API_PORT}/g" \
    "$conf" > /etc/nginx/conf.d/default.conf
  exec nginx -g "daemon off;"
}

case "$MODE" in
  backend)
    start_backend
    ;;
  ui)
    start_ui_nginx
    ;;
  all)
    clamp_sqlite_workers
    sed \
      -e "s/__UI_PORT__/${UI_PORT}/g" \
      -e "s/__API_PORT__/${API_PORT}/g" \
      /etc/nginx/agent-platform-ui-gateway.conf > /etc/nginx/conf.d/default.conf
    nginx
    exec uvicorn main:app \
      --host 0.0.0.0 \
      --port "$API_PORT" \
      --workers "$WORKERS" \
      --timeout-keep-alive "$KEEPALIVE"
    ;;
  *)
    echo "Unknown AGENT_PLATFORM_CONTAINER_MODE=$MODE (expected backend, ui, or all)" >&2
    exit 1
    ;;
esac
