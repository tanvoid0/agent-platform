# ADR 0002: Web UI stack — React, TypeScript, Vite, server state, and @xyflow/react

**Status:** Accepted  
**Date:** 2026-04-11  
**Tags:** frontend, TanStack Query, React Flow, orchestration UI

## Context

The agent platform exposes **HTTP-first** run APIs (`GET /runs`, `GET /runs/{id}`, `POST …`, optional SSE). The browser must stay **consistent with server truth** after refresh, tab sleep, or dropped streams—patterns learned from prior work (fragile live-only clients).

The product roadmap calls for a **long-lived, reactive shell**: inspectors, timelines, DAG graphs, and **optional** heavier surfaces later—without rewriting the shell.

## Decision

1. **Framework:** **React 18+** with **TypeScript** for all new UI in `agent-platform/web/`. Single-page app built with **Vite** (fast HMR, native ESM, straightforward code-splitting).

2. **Graphs / team visualization:** **@xyflow/react** (React Flow) for DAG and team layout. This is the default graph library; alternatives require a short ADR.

3. **Server state (authoritative runs):** **TanStack Query (React Query)** for:
   - `GET /runs` and `GET /runs/{id}` with **typed** responses
   - **Refetch intervals** while a run is non-terminal (`planning`, `approval_required`, `running`, …)
   - **Invalidation** after mutations (`POST /runs`, `POST …/approve`, `POST …/cancel`)
   - **No** replacement for `GET` as source of truth—queries reconcile after SSE or focus events

4. **Streams (SSE):** **Additive.** A dedicated hook may subscribe to `GET /runs/{id}/stream` and **invalidate** the run query on events; the UI still renders from `GET /runs/{id}` data.

5. **Styling / composition:** Keep **colocated** state in feature folders; use **React Query** as the primary cross-panel cache. Avoid a global Redux-style store until multiple panels prove shared client-only state that Query cannot cover.

6. **Optional 3D / embodied simulation:** **Out of this ADR’s scope.** See [0003-future-3d-simulation-boundary.md](./0003-future-3d-simulation-boundary.md). The orchestration bundle does **not** depend on Three/Babylon.

## Non-goals

- WebSocket as the **only** lifecycle channel (forbidden by platform reliability goals).
- Bundling **Three.js, Babylon, or R3F** in the default chunk—lazy boundary only when 0003 is activated.
- Replacing the FastAPI `/ui` Jinja page for operators who want zero-JS—both can coexist.

## Consequences

- **Dependencies:** `@tanstack/react-query` in `web/package.json`; query client provided at app root.
- **API contract:** TypeScript types in `web/src/api/types.ts` must track FastAPI/SQLModel fields; drift is caught at build time.
- **Babylon.js** (or a second engine) is **not** adopted unless a future ADR overrides 0003’s default (R3F + Three as the React-aligned path for optional sim).

## Related

- [0001-agent-platform-orchestration.md](./0001-agent-platform-orchestration.md) — backend FSM and HTTP-first observation.
- [0003-future-3d-simulation-boundary.md](./0003-future-3d-simulation-boundary.md) — lazy 3D module and authority.

## Decision log

| Date | Change |
|------|--------|
| 2026-04-11 | Initial: React+TS+Vite+Query+xyflow; no 3D in default bundle |
