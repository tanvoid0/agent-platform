# Reliability invariants: ADR 0001 Section 6 vs implementation

This document tracks [ADR 0001 Section 6](adr/0001-agent-platform-orchestration.md) (“Proposed decision”) against the current **agent-platform** backend and web client. It includes an **idempotency** audit of process HTTP routes.

## Section 6 checklist

| # | ADR decision | Implementation status | Notes |
|---|----------------|----------------------|--------|
| 1 | Custom FSM + small DAG executor | **Aligned** | [`DAGExecutor`](../app/orchestrator.py) runs the planner DAG with topological layers; task state on [`Process` / `TaskNode`](../app/models.py). |
| 2 | LLM only via embedded proxy `/v1/chat/completions` | **Aligned** | [`llm_client.py`](../app/llm_client.py) uses `llm_proxy_base_url_v1()` + `llm_proxy_master_key()`. Stateless chat: [`chat_routes.py`](../app/chat_routes.py). |
| 3 | Planner: validate → repair/fail closed | **Aligned** | `validate_planner_dag`, env-driven planner retries (see tests e.g. `test_planner_retries.py`). |
| 4 | SQLite + “state lives on disk” | **Aligned** | [`database.py`](../app/database.py): `AGENT_PLATFORM_DB_PATH` (default `data/agent_platform.db`), `PRAGMA journal_mode=WAL`, `synchronous=NORMAL`. |
| 5 | HTTP-first reconciliation; SSE for live traces | **Aligned** | `GET /processes/{id}` and `GET /processes/{id}/events` are authoritative; [`useProcessQueries.ts`](../flow-ui/src/agent-platform/hooks/useProcessQueries.ts) polls with faster interval when SSE is off, slower backup when SSE is on. SSE: `GET /processes/{id}/stream` — comment in route matches ADR (“correctness remains on GET”). |
| 6 | Timeouts, cancel, terminal states | **Aligned** | Env: `AGENT_PLATFORM_PLAN_TIMEOUT_SECONDS`, `AGENT_PLATFORM_RUN_MAX_SECONDS`, `AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS`; `POST /processes/{id}/cancel`; terminal: `completed`, `failed`, `cancelled`. |
| 7 | Subagents prompt-only until tools ADR | **Aligned** | Tools policy / allowlists in [`tools_policy.py`](../app/tools_policy.py). |

### Intentional ADR nuance

- **Observation wording:** ADR mentions “SSE strongly recommended”; the UI uses SSE where appropriate and **always** falls back to polling so refresh/reconnect stays correct.
- **Workflow engines:** ADR defers Temporal; the agent-platform does **not** use Temporal or Redis queues — durability is **SQLite + process rows + background tasks** on the API process.

## Idempotency audit (HTTP)

Safe **client retries** depend on handlers returning success or a clear duplicate signal without double work.

| Route | Idempotent / safe retry? | Behavior |
|-------|--------------------------|----------|
| `POST /processes` | **No** | Each call creates a **new** process. Clients must not auto-retry blindly; use user action or a client-generated idempotency key (not yet a first-class API feature). |
| `POST /processes/{id}/approve` | **Yes** (see below) | Returns `{ idempotent: true }` when status is already `running`, `completed`, or **`approved`** (duplicate approve after success). |
| `POST /processes/{id}/cancel` | **Yes** | Terminal states return `{ idempotent: true }`. |
| `POST /processes/{id}/tasks/{tid}/review` | **Partial** | `approve` when task already `completed` returns `{ idempotent: true }`. Other decisions are one-shot. |
| `POST /processes/{id}/retry` | **Yes** | Only accepts `failed`; second retry fails with 400 if already moved out of `failed`. |
| `POST /processes/{id}/tasks/{tid}/retry` | **Yes** | Same pattern as process retry. |
| `POST /processes/{id}/sync` | **Best-effort** | May **re-schedule** planning or execution; response warns about possible duplicate work if planning was already active. Use when recovering from stuck state, not as a generic retry button. |

### Related: background tasks

Planning and DAG execution run via FastAPI `BackgroundTasks`. If the **process exits** mid-task, work can stall until the client calls **`POST /processes/{id}/sync`** (or restarts and syncs). This matches the lightweight model; full queue recovery would require an external worker (see The Delegation’s optional BullMQ doc).

## See also

- [The Delegation — Redis / BullMQ reliability](../../../../javascript/the-delegation/docs/redis-bullmq-reliability.md) (optional Redis queue used by that app, not by agent-platform core).
