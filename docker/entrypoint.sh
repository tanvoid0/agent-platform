#!/bin/sh
# Container entry point for `agent-platformd`.
#
# This used to launch uvicorn with a worker count, and clamp that count to 1
# whenever the database was SQLite. Neither applies now: the server is one Rust
# process serving on a tokio pool, so there are no worker processes to size and
# nothing to clamp. `AGENT_PLATFORM_UVICORN_WORKERS` and
# `AGENT_PLATFORM_UVICORN_KEEPALIVE_SECONDS` are dead — a compose file still
# setting them is harmless, and both are gone from the one in this repo.
set -eu

# 0.0.0.0, not the daemon's own 127.0.0.1 default: a container that binds
# loopback publishes a port nothing can reach, which looks like a crashed
# server from outside.
export AGENT_PLATFORM_HOST="${AGENT_PLATFORM_HOST:-0.0.0.0}"
export AGENT_PLATFORM_PORT="${AGENT_PLATFORM_PORT:-${AGENT_PLATFORM_API_PORT:-18410}}"
export AGENT_PLATFORM_DB_PATH="${AGENT_PLATFORM_DB_PATH:-/app/data/agent_platform.db}"

# The daemon refuses to start with DATABASE_URL set (it is SQLite-only until the
# `any` pool migration finishes — see AppState). An empty value is not "unset"
# to it, so clear it rather than leaving the empty string compose passes down.
if [ -z "${DATABASE_URL:-}" ]; then
  unset DATABASE_URL || true
fi

exec agent-platformd
