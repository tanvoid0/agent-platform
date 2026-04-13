# Pixel / embodied preview (lazy-friendly)

This folder holds **lightweight** pixel-style UI tied to **orchestrator run state** (API-backed).

- **`PixelProcessStrip`** (`PixelRunStrip` re-export) — status tiles from `PlannerDag` / task records (read-only).
- **`PixelHomeTeaser`** — static demo strip when no run is loaded.

For **boundary rules** (lazy chunks, server authority, optional 3D later), see `docs/adr/0003-future-3d-simulation-boundary.md`.

**Animation / office metaphor reference (external, MIT):** [pixel-agents](https://github.com/pablodelucca/pixel-agents) shows Canvas 2D, a small activity state machine, and sprite-driven characters. This repo should drive any future animation from **agent-platform run events and task status**, not from transcript log scraping.
