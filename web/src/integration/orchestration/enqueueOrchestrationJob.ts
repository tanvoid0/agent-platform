export type ExecuteTaskOrchestrationPayload = {
  taskId: string;
  assignedAgentId: number;
  title?: string;
  /** True when this run is a user-driven retry after failure */
  isRetry?: boolean;
};

/**
 * In-browser orchestration (see `AgentSimulation`):
 * - **Per agent:** board tasks for one agent run **serially** (one in-flight execution at a time).
 * - **Across agents:** multiple agents can run **concurrently** — each `AgentHost` awaits its own LLM/tool work independently on the JS event loop.
 * - **Chat vs tasks:** user chat uses the same host but does not replace queued task work; `canChat` allows messages while status is `working` except explicit review holds.
 *
 * Server-side DAG runs (`app/orchestrator.py`) use `asyncio.gather` per ready batch; cap concurrency with `AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS` (unset = all ready tasks in parallel).
 */

/** Optional server-side durable queue was removed; hook kept for call sites. */
export function notifyExecuteTaskQueued(_payload: ExecuteTaskOrchestrationPayload): void {}

/** Optional server-side durable queue was removed; hook kept for call sites. */
export function notifySparkQueued(): void {}
