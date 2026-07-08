# Flow UI (`web/`)

Canonical React + Vite workspace for agent-platform.

## Local dev

From repo root (API + web together):

```bash
pnpm dev
```

API only (this folder still proxies `/api`, `/processes`, etc. to `:18410` when you run Vite alone):

```bash
# terminal 1 — from repo root
pnpm dev:server

# terminal 2 — from web/
pnpm dev
```

Open `http://127.0.0.1:3333/app/`.

Production build is baked into root `Dockerfile` and served at `/app/` on `:18408` when `AGENT_PLATFORM_CONTAINER_MODE=all`.
