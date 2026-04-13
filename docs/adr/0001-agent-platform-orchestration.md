# ADR 0001 (Draft): Agent platform — orchestration, observation, and boundaries

**Status:** Draft — intended for further research before implementation  
**Date:** 2026-04-10  
**Deciders:** (fill when decided)  
**Tags:** multi-agent, HITL, Ollama, OpenAI-compatible proxy, reliability

---

## 1. Context

You already expose models through an **OpenAI-compatible** surface (`/v1/chat/completions`) via **llm-orchestrator**, with **Ollama-first** routing and optional cloud providers. The missing product is not another LLM proxy: it is an **agentic orchestration layer** that implements a narrow, high-value loop:

**user goal → planner proposes a team (roles + task DAG) → human reviews/edits → subagents execute → aggregated results + trace.**

Prior experiments (e.g. Portal Plexus) demonstrated that:

- Coupling this loop to **simulation editing** (world objects, POIs, layout, mirror/brain sync) consumed disproportionate time relative to orchestration outcomes.
- **Live client ↔ server** channels without a **durable, pollable source of truth** led to **stuck runs** after drops or partial failures.
- “Parity chasing” a reference UX (e.g. The Delegation) across a **second stack** multiplies cost without guaranteeing the core loop.

A reference product such as [The Delegation](https://github.com/arturitu/the-delegation) is useful for **interaction patterns** (team view, review, logs), not as a substrate: it centers **Gemini**, **WebGPU/Three**, and embodied simulation — different constraints than an **Ollama-first**, **simulation-optional** platform.

This ADR records **architectural choices and forks** so you can research each branch deliberately instead of rewriting mid-flight.

---

## 2. Problem statement

Design a system that:

1. **Decomposes** a goal into a **structured team proposal** (subagents, responsibilities, optional per-agent model hints, **dependency edges**).
2. **Blocks execution** on **human approval** (with optional edits to the proposal).
3. **Executes** subagents according to the DAG (parallelism where dependencies allow).
4. **Aggregates** outputs and preserves an **audit trail** (what ran, errors, timings).
5. **Survives** UI refresh, tab sleep, and network loss: the **server** remains authoritative; the client **reconciles** by reading state, not by owning the run.

Non-problems for v1: 3D offices, navmesh, asset pipelines, or “bots that write arbitrary code” without a sandbox and policy story. The deliverable is a **lightweight 2D interface** (simple agent tracking with cards, containers, and logs). 3D simulation concepts are strictly deferred to future stages once the core AI logic is proven.

---

## 3. Goals and explicit non-goals

### 3.1 Goals

- **Thin coupling to llm-orchestrator:** All model traffic uses the existing proxy and its **model aliases** / env configuration.
- **Human-in-the-loop & Guardrails:** Core flows have an approval gate (approve or edit-then-approve). The system must inherently support an **"Auto-approve"** fast-path for trusted execution.
- **Cost & Token Tracking:** Persist token and usage cost metadata for every agent invocation, aggregated per-run.
- **Observable runs (SSE):** Operators can view live LLM traces and tool calls via Server-Sent Events, creating a reactive UI bridging a state manager (e.g. Zustand) with streaming AI triggers.
- **Context Management:** Overcome context window limits via a **"Planner-Executor" split**: breaking large prompts into smaller parallel "sprints" of work instead of a monolithic context thread.

### 3.2 Non-goals (v1)

- **3D environments / simulation objects** — Focus entirely on the core "brain" and UX logic. V1 uses a simple 2D view.
- **Arbitrary tool execution** without allowlists, budgets, and (later) explicit risk review.
- **Guaranteed** multimodal artifact pipelines (video, durable asset stores) — track separately if needed.
- **Feature parity** with any reference UI; steal patterns, not scope.

---

## 4. Constraints and assumptions

- **Ollama-first** for cost and locality; cloud models are **optional** via the same proxy.
- **Structured Output Reliability:** Do NOT just prompt for JSON. Leverage Ollama's `format: "json"` strictly, or integrate grammar-based sampling at the client level.
- Local models may emit **invalid or partial JSON** for structured planner output despite schema configurations — design must assume **validation + retry + fallback model** for planning output.
- The orchestrator is **stateless per request**; **all run state** for the agent platform lives in **your** persistence layer.
- Host port preferences (e.g. uncommon localhost ports) apply to **how** you deploy, not to this ADR’s logic.

---

## 5. Alternatives considered

Use this section as a **research matrix**. Nothing here commits you to a library until you mark **Decision** (Section 6).

### 5.1 Orchestration runtime

| Approach | Strengths | Risks / costs | When it wins |
|----------|-----------|----------------|--------------|
| **Custom FSM** (explicit statuses + transitions in your service) | Minimal deps, easy idempotent HTTP mapping, full control | You own persistence, recovery, and DAG execution correctness | Small team, v1 scope fixed (one gate, modest DAG) |
| **LangGraph** (or similar graph + interrupts) | Native **human-in-the-loop** patterns, checkpointing story, graph visualization concepts | Learning curve, dependency weight, you still must integrate with your DB and API | Many interrupts, branching, retries, long-running graphs |
| **Workflow engines** (Temporal, Windmill, etc.) | Durability, ops tooling | Heavy ops; often overkill for a first vertical slice | Multi-day runs, many workers, enterprise SLOs |
| **Frameworks** (CrewAI, AgentScope, smolagents) | High-performance concurrency (AgentScope), sequential/hierarchical "Process" logic (CrewAI), or robust Code-Agent execution (smolagents). | Opinionated; can fight your minimal surface goals. You may end up reimplementing the executor. | Rapid prototyping, or extracting specific architectural features (e.g., Message Hub). |

**Research prompts:** “LangGraph human in the loop interrupt,” “orchestration vs application state machine,” “CrewAI vs custom for OpenAI-compatible API.”

### 5.2 Observation protocol (UI ↔ backend)

| Approach | Strengths | Risks | When it wins |
|----------|-----------|-------|--------------|
| **HTTP poll** `GET /runs/:id` | Dead simple, cache-friendly, survives disconnects | Latency vs load if polled too fast | **Default** for correctness |
| **SSE** | Push log lines / step changes; still one-way server→client | Proxies, timeouts, reconnect rules | When you want “live” without WS complexity |
| **WebSocket** | Bidirectional, low latency | Often becomes **sole** source of truth by accident → stuck UIs | Optional **after** poll works; great for streaming tokens, not for lifecycle truth |

**Invariant to adopt:** **Correctness = server-stored state machine.** Streams are **projections**, not authority.

### 5.3 Persistence

| Store | Strengths | Risks | When it wins |
|-------|-----------|-------|--------------|
| **SQLite / OpenClaw Pattern** | "State Lives on Disk": checkpoints stored iteratively to survive process exits. Strong for v1. | WAL concurrency needs discipline if multiple background threads write. | Deep long-running background tasks, preventing context blowup, solo deploy. |
| **Postgres** | Robust concurrency, future multi-instance | More ops | Shared DB with other services or HA |

**Research prompts:** “SQLite WAL mode concurrent writes,” “event sourcing vs CRUD for agent runs.”

### 5.4 Where the service lives

| Option | Strengths | Risks |
|--------|-----------|-------|
| **Separate process** (recommended default in this ADR) | Clear boundary: orchestrator stays a **proxy**; agent platform owns runs | One more deployable |
| **Mounted inside FastAPI** of llm-orchestrator | Single binary | Blurs responsibilities; proxy upgrades risk agent state |

---

## 6. Proposed decision (default recommendation)

This is a **default** you can accept, revise, or reject after research. It is consistent with the goals above and with lessons from prior experiments.

1. **Orchestration:** Start with a **custom explicit state machine** and a **small internal DAG executor**. Execute the DAG level-by-level via a topological sort, isolating outputs on a "Blackboard" or explicit edges rather than sharing one giant context array.

2. **LLM access:** **Only** via **llm-orchestrator** OpenAI-compatible **`/v1/chat/completions`**, using **model aliases** from orchestrator config and a single auth story (`ORCHESTRATOR_MASTER_KEY`; legacy `LITELLM_MASTER_KEY` accepted).

3. **Planner output:** Leveraged via strict JSON schema (or Ollama's `format: json`). Implement validate → optional repair pass → fail closed.

4. **Persistence:** Use the **State Lives on Disk** pattern with **SQLite** WAL for run rows, cost/token metrics, and an append-only event log.

5. **Observation:** **HTTP-first** for full state reconciliation; **SSE strongly recommended** for streaming real-time LLM traces and intermediate agent steps to the React Flow UI.

6. **Reliability:** Timeouts on planning/subagents; global run timeout; explicit cancel endpoint; terminal states.

7. **Security posture (v1):** Subagents are **prompt-only** unless tools are explicitly enabled later under **ADR 0002 (tools)** with allowlists and budgets.

---

## 7. Consequences

### 7.1 Positive

- Clear **separation of concerns**: proxy routes traffic; agent platform routes **intent and state**.
- **Debuggable** failures: stuck runs surface as **failed/timeout** with server-side reason, not silent UI desync.
- **Incremental UX**: you can add React Flow **read-only** graphs fed by the same `GET` payload without introducing a second authority.

### 7.2 Negative / costs

- You must implement **DAG correctness** (cycle detection, topological order, parallel boundaries) yourself if you stay on a custom executor.
- **Two services** to deploy unless you consciously collapse them (trade clarity for convenience).
- **Structured output** from small local models may require **prompt engineering + retries** — plan time for that.

---

## 8. Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Invalid planner JSON | Schema validation, repair prompt, fallback model; never advance state on bad data |
| Long-running subagents | Per-call timeout; global run timeout; cancel; visible “running since …” |
| UI shows “stuck” | Server-owned states; poll reconciliation; no “optimistic only” transitions |
| Scope creep (simulation, editors) | Keep non-goals in ADR; separate ADR for “embodiment” if ever |
| Tool misuse later | Separate ADR: allowlist, budgets, audit, optional MCP bridge |

---

## 9. Open questions (answer before coding)

1. **Minimum viable DAG (ANSWERED):** The DAG requires **parallel subagents** executing layer-by-layer (e.g. via iterative Planner-Executor split) to maximize isolated Ollama sprint tasks. Linear chain is insufficient for scaling multi-agent tasks safely within confined contexts.
2. **Identity and tenancy (ANSWERED):** **Single-user local only** for v1, driven by `ORCHESTRATOR_MASTER_KEY` (legacy `LITELLM_MASTER_KEY`).
3. **Aggregation (ANSWERED):** A **dedicated “Synthesizer” subagent template**. Relying on the "last node" mathematically breaks down when multiple parallel workers produce independent insights. The planner injects a sink node.
4. **Edit scope at approval (ANSWERED):** **Full proposal replace (JSON overwrite).** Patch-by-id merges are too burdensome for a v1 local tool.
5. **Retention (ANSWERED):** Indefinite storage in **SQLite checkpoints**, cleaned up manually by user command to preserve their valuable OpenClaw-like on-disk history.

---

## 10. Research backlog (curated)

Use this as a reading list; reorder by your risk tolerance.

- **Human-in-the-loop** patterns in graph frameworks: LangGraph docs (interrupts, persistence).
- **Structured output** from small models: JSON schema constraints, “repair” passes, and failure UX.
- **Idempotency** in HTTP APIs for approve/cancel (client retries safe).
- **SSE vs WebSocket** for **server-sent** updates behind reverse proxies (nginx, CORS).
- **SQLite** concurrency model (WAL) if you run parallel background tasks writing the same DB.
- **Architectural patterns (Zustand, AgentScope message hubs, smolagents Code-Agents)** to conceptualize logic execution.
- **Context optimization techniques:** GPt Researcher's Planner-Executor mapping across multiple small Ollama models.
- **Reference UX** (The Delegation, others): list **behaviors** to emulate (team proposal, review modals, logs) without importing their engine stack.

---

## 11. Relationship to other documents

- **Product brainstorm / roadmap** in the repo (e.g. `.cursor/plans/`) — product intent; **this ADR** is architecture.
- **llm-orchestrator** — transport and provider routing only; agent logic does not belong in `ui/app/routes/llm.py`.
- **[ADR 0002](./0002-ui-stack-react-typescript-vite.md)** — web shell (React+TS+Vite, TanStack Query, @xyflow/react).
- **[ADR 0003](./0003-future-3d-simulation-boundary.md)** — optional lazy-loaded 3D module; authority stays server-side.
- **Tools / MCP** — `app/tools_policy.py` today; a dedicated ADR when executable tools ship beyond prompts.

---

## 12. Decision log

| Date | Change |
|------|--------|
| 2026-04-10 | Initial draft: FSM + HTTP-first + SQLite + orchestrator-only LLM |

When you accept or reject Section 6, update **Status** to **Accepted** or **Superseded**, fill **Deciders**, and add a row here.
