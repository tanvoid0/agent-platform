# Simulation viewport (optional 3D)

Lazy-loaded **R3F + Three** UI per [`docs/adr/0003-future-3d-simulation-boundary.md`](../../../docs/adr/0003-future-3d-simulation-boundary.md).

- **`SimulationSpike.tsx`** — minimal Canvas (rotating box tinted by read-only `GET /processes/:id` status). Heavy deps stay out of the main graph bundle via `React.lazy` in `ProcessesPage`.

Authority: run lifecycle remains **HTTP APIs** on the agent-platform service; any future simulation tick or world state belongs on the **server** or a dedicated sim service.
