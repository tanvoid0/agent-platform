# Reliability invariants: ADR 0001 Section 6 vs implementation

This document tracks [ADR 0001 Section 6](adr/0001-agent-platform-orchestration.md) (“Proposed decision”) against the current **agent-platform** backend and the native desktop client. It includes an **idempotency** audit of process HTTP routes.

> Re-checked 2026-08-04. The client references below were the deleted `web/` React app; they now point at the iced app ([ADR 0005](adr/0005-native-iced-desktop-headless-server.md)). The invariants themselves are unchanged — both clients reconcile over HTTP and treat SSE as a hint.

## Section 6 checklist

| # | ADR decision | Implementation status | Notes |
|---|----------------|----------------------|--------|
| 1 | Custom FSM + small DAG executor | **Aligned** | [`DAGExecutor`](../app/orchestrator.py) runs the planner DAG with topological layers; task state on [`Process` / `TaskNode`](../app/models.py). |
| 2 | LLM only via embedded proxy `/v1/chat/completions` | **Aligned** | [`llm_client.py`](../app/llm_client.py) uses `llm_proxy_base_url_v1()` + `llm_proxy_master_key()`. Stateless chat: [`chat_routes.py`](../app/chat_routes.py). |
| 3 | Planner: validate → repair/fail closed | **Aligned** | `validate_planner_dag`, env-driven planner retries (see tests e.g. `test_planner_retries.py`). |
| 4 | SQLite + “state lives on disk” | **Aligned** | [`database.py`](../app/database.py): `AGENT_PLATFORM_DB_PATH` (default `data/agent_platform.db`), `PRAGMA journal_mode=WAL`, `synchronous=NORMAL`. |
| 5 | HTTP-first reconciliation; SSE for live traces | **Aligned** | `GET /processes/{id}` and `GET /processes/{id}/events` are authoritative; [`processes.rs`](../desktop/crates/app/src/processes.rs) polls detail at 800ms while a run is live and 4s once settled, and treats an SSE frame as a trigger to refetch rather than as state. SSE: `GET /processes/{id}/stream` — comment in route matches ADR (“correctness remains on GET”). |
| 6 | Timeouts, cancel, terminal states | **Aligned** | Env: `AGENT_PLATFORM_PLAN_TIMEOUT_SECONDS`, `AGENT_PLATFORM_RUN_MAX_SECONDS`, `AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS`; `POST /processes/{id}/cancel`; terminal: `completed`, `failed`, `cancelled`. |
| 7 | Subagents prompt-only until tools ADR | **Aligned** | Tools policy / allowlists in [`tools_policy.py`](../app/tools_policy.py). |

### Intentional ADR nuance

- **Observation wording:** ADR mentions “SSE strongly recommended”; the UI uses SSE where appropriate and **always** falls back to polling so refresh/reconnect stays correct. The iced client only opens the stream for live runs — a terminal run replaying a backlog closes without a sentinel and would otherwise reconnect forever.
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

Planning and DAG execution run via FastAPI `BackgroundTasks` (27 call sites). If the **process exits** mid-task, work can stall until the client calls **`POST /processes/{id}/sync`** — or until `app/services/startup_recovery.py` cleans up on the next boot. This matches the lightweight model.

It is also the binding constraint on running more than one worker, alongside the process-local rate limiter (`app/api_tokens/rate_limiter.py`) and the bare poll loop in `app/workflows/engine.py`. All three are answered by the same durable job table — see [backend-perf-plan.md](backend-perf-plan.md), *What this does not fix*.

## See also

- [backend-perf-plan.md](backend-perf-plan.md) — the multi-worker constraints above, and the pooling / event-loop fixes already landed.
