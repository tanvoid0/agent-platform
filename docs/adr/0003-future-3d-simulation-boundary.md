# ADR 0003: Future optional 3D simulation — module boundary and authority

**Status:** Boundary **still holds**; mechanism **superseded** by [ADR 0005](0005-native-iced-desktop-headless-server.md)  
**Date:** 2026-04-11 (status revised 2026-08-04)  
**Tags:** 3D, R3F, Three.js, Babylon, simulation, lazy-loading, pixel-art, canvas-2d

> **Revision, 2026-08-04.** The *authority* half of this ADR is unchanged and load-bearing: a
> simulation view is a renderer of server state and never an owner of run state. The
> *mechanism* half is dead — dynamic `import()`, bundle splitting, `main.tsx`, and every
> `web/src/features/…` path below refer to the React/Vite app deleted by ADR 0005. The pixel
> office described in the addendum was dropped with it and was not reimplemented in
> `iced::canvas`; see [native-desktop-migration.md](../native-desktop-migration.md).
>
> If spatial visualization is ever revived, it lands in the native app and this ADR needs a
> successor stating the iced-side boundary. The rule to carry forward: whatever draws it reads
> `GET /processes/{id}`, and writes go through the same REST actions every other screen uses.

## Context

Long-term product ideas may include **spatial / embodied** visualization (office metaphor, avatars, environment). Prior experiments show that **coupling** orchestration to simulation editing or client-side authority causes **scope explosion** and **stuck-state** bugs. The platform already separates **LLM routing** (embedded proxy `/v1`) from **run lifecycle** (agent-platform API).

This ADR defines how a **future** 3D viewport may attach **without** entangling Phase 1–2 orchestration UI or run state.

## Decision

1. **Lazy loading (mandatory):** Any 3D or heavy renderer loads via **dynamic `import()`** in a dedicated route or feature folder (e.g. `web/src/features/simulation/`). The **default** orchestration bundle must not statically import Three, Babylon, or R3F.

2. **Default engine (when an ADR chooses to implement):** Prefer **React Three Fiber (@react-three/fiber) + Three.js** inside the lazy chunk—**one** component model (React) from forms → graph → scene. Use `@react-three/drei` only as needed. **WebGPU** depth (e.g. Three’s WebGPU renderer) remains an **incremental** choice inside that module.

3. **Babylon.js:** Allowed **only** after a **short spike ADR** documenting: team familiarity, bundle size, feature need (e.g. tooling unique to Babylon), and **single** engine policy—**two** full engines in one app is a non-default outcome.

4. **Authority (non-negotiable):**
   - **Run lifecycle** (planning, approval, execution, terminal states) remains on the **agent-platform** HTTP API and persistence—**never** owned by the 3D client.
   - **Simulation tick / world truth** (if any) is **server-side** or a **dedicated simulation service** with explicit APIs (snapshots, commands)—mirroring the Brain vs Mirror lesson from Portal Plexus: the browser is **presentation + input**, not the source of truth for “what run state are we in?”

5. **Contract:** The 3D module consumes **JSON** (run summaries, optional future `WorldSnapshot`-style DTOs) over the same **poll-first** patterns as the rest of the UI. Optional **WebSocket or binary** streams are **presentation-only** (e.g. pose interpolation), not approval gates.

6. **Spike (minimal):** A **placeholder** lazy chunk proves bundle split and route wiring without adding engine dependencies—see `web/src/features/simulation/` (placeholder component). Replacing the placeholder with R3F is a **later** task once product prioritizes 3D.

7. **Pixel art / 2D activity visualization (optional, reference—not a fork):** Embodied “office” metaphors and **activity-mapped** character animation are well illustrated by the open-source **[pixel-agents](https://github.com/pablodelucca/pixel-agents)** project (MIT): Canvas 2D, game loop, pathfinding, and a small state machine (idle → walk → type/read), with sprites driven by **observed** agent activity (today: Claude Code JSONL transcripts). That stack is a **UX and animation pattern** reference only.

   **How agent-platform should differ (when/if we go beyond status tiles):**

   - **Authority:** Drive animation from **agent-platform run and task state** (HTTP poll + SSE + persisted DAG/task records)—not from log-file heuristics on a specific CLI.
   - **Scope:** Keep the same **lazy** boundary: any Canvas 2D or sprite atlas that grows beyond trivial UI lives under `web/src/features/pixel/` (or a sibling lazy chunk), not in the default graph bundle.
   - **Assets:** If we add manifests or sprite packs later, follow the same **modular asset** idea (declarative manifests, integer zoom for crisp pixels)—without vendoring upstream art unless license and product intent align.

   Current UI ships a minimal **server-driven** strip (`PixelRunStrip`) as a thin, authoritative preview—not a port of pixel-agents.

## Non-goals (until promoted)

- NavMesh, asset pipelines, or avatar rigging in the orchestration repo.
- Client-side **authoritative** pathfinding or POI claim logic tied to human approval.

## Consequences

- Product and engineering can **defer** all 3D work until orchestration UX is stable.
- CI/bundle analysis can **fail** a PR that adds static imports of Three/Babylon to `main.tsx` or the default graph route—enforce via review or lint rule when needed.

## Related

- [0002-ui-stack-react-typescript-vite.md](./0002-ui-stack-react-typescript-vite.md)
- Portal Plexus ADR on navigation: external reference for “authoritative pathfinding on server” mindset.
- External pattern reference (MIT): [pablodelucca/pixel-agents](https://github.com/pablodelucca/pixel-agents) — pixel office, activity-mapped characters; compare §7 above for how we bind state.

## Decision log

| Date | Change |
|------|--------|
| 2026-04-11 | Initial: lazy boundary, R3F+Three default if implemented, Babylon requires ADR, server authority |
| 2026-04-11 | §7: pixel-agents as 2D/animation reference; authoritative run/task events vs CLI JSONL |
\