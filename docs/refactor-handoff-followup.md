# Refactor Handoff: Follow-up Checklist

This document is the handoff for continuing the project-wide cleanup in a new chat.

## What is already done

### Backend service extraction
- Extracted process/task mutation helpers into:
  - `app/services/process_mutation_service.py`
  - `app/services/task_result_service.py`
  - `app/services/process_runtime_service.py`
  - `app/services/review_assignment_service.py`
  - `app/services/subdag_service.py`
  - `app/services/planner_runtime_service.py`
  - `app/services/event_log_service.py`
- `app/orchestrator.py` now delegates major persistence/state transitions to services.
- `app/process_routes.py` now delegates task review/retry mutation branches to service helpers.

### Frontend store cleanup
- Extracted task/history logic from `coreStore` into helpers:
  - `flow-ui/src/integration/store/coreTaskMutations.ts`
  - `flow-ui/src/integration/store/coreHistoryMutations.ts`
- Extracted shared UI state utilities:
  - `flow-ui/src/integration/ui/projectSideTabStorage.ts`
  - `flow-ui/src/integration/hooks/useHeartbeatNow.ts`
  - `flow-ui/src/integration/hooks/useProjectsReachabilityPolling.ts`

### Contract/hygiene updates
- Added repo hygiene check script:
  - `scripts/check_repo_hygiene.py`
- Unified package-manager docs toward `pnpm`.
- Introduced canonical key naming support:
  - `AGENT_PLATFORM_MASTER_KEY` (legacy alias still supported).
- Updated docs/compose defaults around DB and architecture consistency.

### Tests added in this pass
- `app/tests/test_process_mutation_service.py`
- `app/tests/test_task_result_service.py`
- `app/tests/test_process_runtime_service.py`
- `app/tests/test_subdag_service.py`
- `app/tests/test_planner_runtime_service.py`
- `app/tests/test_event_log_service.py`
- Updated: `app/tests/test_llm_proxy_env.py`

## Remaining high-impact updates

## 1) Finish backend decomposition of `DAGExecutor`
- Goal: keep `DAGExecutor` as coordinator only.
- Remaining candidates to extract:
  - sub-DAG expansion preflight logic (cap/depth/spec checks)
  - topological ready-task selection logic from `execute_dag`
  - run-loop branch handling into reusable helpers (cancelled/failed/timeout/deadlock/review gate)

Suggested files:
- new `app/services/dag_runtime_service.py`
- possibly `app/services/subdag_policy_service.py`

## 2) Datetime deprecation cleanup
- Current warnings still mention `datetime.utcnow()` usage.
- Replace with one consistent helper in critical runtime paths first:
  - `app/orchestrator.py`
  - `app/services/task_result_service.py`
  - optionally model defaults in `app/models.py` (larger migration impact; do separately if needed)

Notes:
- For DB compatibility, apply a single pattern consistently (aware UTC converted to existing storage shape if needed).
- Re-run tests after each small batch.

## 3) Route-layer cleanup (thin controllers)
- `app/process_routes.py` still owns significant orchestration branching.
- Continue moving business logic into services:
  - sync/retry/cancel flow branches
  - approval/task-review transition orchestration

## 4) Frontend: continue reducing `coreStore` size
- Split remaining action groups into focused modules:
  - execution state mutations
  - finance/token ledger mutations
  - multimodal delivery state transitions
- Keep `coreStore` as composition layer.

## 5) Optional consistency pass
- Standardize import style to `@/...` in frontend touched files.
- Keep `scripts/check_repo_hygiene.py` as part of CI/local checks.

## Suggested follow-up execution order

1. `DAGExecutor` extraction (small behavior-preserving chunks)
2. datetime cleanup in runtime paths
3. process route thinning
4. frontend store decomposition continuation
5. docs + final regression sweep

## Validation checklist for follow-up chat

Backend:
- `pytest app/tests/test_dag_executor.py`
- `pytest app/tests/test_process_mutation_service.py`
- `pytest app/tests/test_task_result_service.py`
- `pytest app/tests/test_process_runtime_service.py`
- `pytest app/tests/test_subdag_service.py`
- `pytest app/tests/test_planner_runtime_service.py`
- `pytest app/tests/test_event_log_service.py`

Frontend:
- `pnpm --dir web typecheck`

Hygiene:
- `python scripts/check_repo_hygiene.py`

## Prompt you can paste in the next chat

Use this as the starter prompt:

1. Continue the refactor from `docs/refactor-handoff-followup.md` only.
2. Prioritize backend `DAGExecutor` decomposition and `datetime.utcnow()` cleanup.
3. Keep behavior unchanged; do small safe commits of logic extraction.
4. After each chunk, run targeted pytest + `pnpm --dir web typecheck`.
5. Do not edit plan files; produce a concise progress summary with touched files and remaining items.
