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
# `PORT` is Cloud Run's contract: it picks the port and the container must bind
# that one or the revision never goes healthy. It sits between our own two
# names and the default, so an explicit AGENT_PLATFORM_PORT still wins locally.
export AGENT_PLATFORM_PORT="${AGENT_PLATFORM_PORT:-${PORT:-${AGENT_PLATFORM_API_PORT:-18410}}}"
export AGENT_PLATFORM_DB_PATH="${AGENT_PLATFORM_DB_PATH:-/app/data/agent_platform.db}"

# A DSN is honoured now (the `any` pool migration finished — see AppState), so
# DATABASE_URL is how the Cloud Run revision reaches Postgres. But an empty
# value is not "unset" to `env_opt`, and compose passes the empty string down,
# which would be read as a DSN of "". Clear it rather than pass it on.
if [ -z "${DATABASE_URL:-}" ]; then
  unset DATABASE_URL || true
fi

exec agent-platformd
