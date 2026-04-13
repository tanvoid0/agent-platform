import type { EventLogRecord, PlannerDag, ProcessStatus, TaskNodeRecord } from "../../api/types";
import {
  areTaskDependenciesSatisfied,
  boardRowsForPixelStrip,
  type BoardColumn,
  type BoardRow,
} from "../../lib/dagTasks";
import { accentMapFromTeamSnapshotJson, fallbackTorsoAccentFromClientUuid } from "./pixelAccentColor";
import { DESK_SLOT_MOD } from "./pixelOfficeLayout";
import { spriteCharIndexFromClientUuid } from "./pixelSpriteConstants";

/** Coarse animation bucket (pixel-agents–style state machine, API-driven only). */
export type PixelActivity = "idle" | "walk" | "work" | "attention";

/** Refines `work` sprite frames from recent process events (global stream; best-effort). */
export type WorkAnimationHint = "typing" | "reading";

/** Newest-first event types; same heuristics as {@link workHintFromRecentEventTypes}. */
export function workHintFromTypeList(types: readonly string[]): WorkAnimationHint | undefined {
  const slice = types.slice(0, 8);
  if (!slice.length) return undefined;
  const hasTool = slice.includes("tool_call");
  const hasTrace = slice.includes("trace");
  if (hasTool && !hasTrace) return "typing";
  if (hasTrace && !hasTool) return "reading";
  if (hasTool && hasTrace) return "typing";
  return undefined;
}

/**
 * Maps recent events per task to `client_uuid` so each working agent can show typing vs reading
 * from that task’s stream (falls back to global hints when absent).
 */
export function buildWorkHintByClientUuid(
  events: readonly EventLogRecord[],
  tasks: readonly TaskNodeRecord[],
): Record<string, WorkAnimationHint> {
  const taskIdToUuid = new Map<number, string>();
  for (const t of tasks) taskIdToUuid.set(t.id, t.client_uuid);

  const typesByTask = new Map<number, string[]>();
  for (const e of events) {
    if (e.task_id == null) continue;
    const arr = typesByTask.get(e.task_id) ?? [];
    arr.push(e.event_type);
    typesByTask.set(e.task_id, arr);
  }

  const out: Record<string, WorkAnimationHint> = {};
  for (const [taskId, types] of typesByTask) {
    const uuid = taskIdToUuid.get(taskId);
    if (!uuid) continue;
    const newestFirst = types.slice(-8).reverse();
    const hint = workHintFromTypeList(newestFirst);
    if (hint) out[uuid] = hint;
  }
  return out;
}

export interface PixelAgent {
  client_uuid: string;
  role: string;
  /** Stable order in the board strip (0..n-1). */
  slotIndex: number;
  /** Which MIT sprite sheet row to use (0..PA_CHAR_COUNT-1), stable per `client_uuid`. */
  spriteCharIndex: number;
  /** Cosmetic desk index in the mini-office (cycles). */
  deskSlot: number;
  column: BoardColumn;
  activity: PixelActivity;
  /** When `activity === "work"`, may choose typing vs reading frames from `recentEventTypes`. */
  workHint?: WorkAnimationHint;
  /** Torso tint from team roster `accent_color` when snapshot matches `role` to roster `name` or `id`. */
  torsoAccent?: string;
}

export interface PixelSceneState {
  processStatus: ProcessStatus;
  agents: PixelAgent[];
}

/**
 * Optional bounded hints for finer-grained animation later (e.g. last event types from the parent).
 * When provided, may refine `activity`; orchestration stays server-authoritative.
 */
export interface PixelEventHints {
  /** Newest-first event_type strings; keep small (e.g. ≤32) for predictable work. */
  recentEventTypes?: string[];
  /** Per-agent hint when `task_id` on events maps to this `client_uuid` (overrides global). */
  workHintByClientUuid?: Record<string, WorkAnimationHint>;
}

function isTerminalProcess(status: ProcessStatus): boolean {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function workHintFromRecentEventTypes(hints?: PixelEventHints): WorkAnimationHint | undefined {
  return workHintFromTypeList(hints?.recentEventTypes ?? []);
}

/** Peers currently assigned as reviewer for an awaiting_review task (server-driven). */
function reviewerClientUuidSet(tasks: readonly TaskNodeRecord[]): Set<string> {
  const s = new Set<string>();
  for (const t of tasks) {
    const ru = t.reviewer_client_uuid?.trim();
    if (t.status === "awaiting_review" && ru) s.add(ru);
  }
  return s;
}

function resolvePixelActivity(
  row: BoardRow,
  processStatus: ProcessStatus,
  tasks: TaskNodeRecord[],
  peerReviewers: Set<string>,
): PixelActivity {
  if (isTerminalProcess(processStatus)) {
    return "idle";
  }

  if (
    peerReviewers.has(row.subagent.client_uuid) &&
    (processStatus === "task_review_required" || processStatus === "running")
  ) {
    return "work";
  }

  const col = row.column;

  if (col === "running") return "work";
  if (col === "awaiting_review") return "attention";
  if (col === "completed" || col === "failed") return "idle";

  if (col === "pending") {
    if (processStatus === "approval_required" || processStatus === "task_review_required") {
      return "attention";
    }
    if (!row.task) {
      return "idle";
    }
    if (processStatus !== "running") {
      return "idle";
    }
    if (!areTaskDependenciesSatisfied(row.task, tasks)) {
      return "idle";
    }
    return "walk";
  }

  return "idle";
}

/**
 * Pure projection: process + DAG + tasks → per-agent layout and activity. No side effects.
 */
export function buildPixelViewModel(
  processStatus: ProcessStatus,
  dag: PlannerDag | null,
  tasks: TaskNodeRecord[],
  hints?: PixelEventHints,
  teamSnapshotJson?: string | null,
): PixelSceneState {
  const rows = boardRowsForPixelStrip(dag, tasks, teamSnapshotJson);
  const peerReviewers = reviewerClientUuidSet(tasks);
  const globalHint = workHintFromRecentEventTypes(hints);
  const accentByRole = accentMapFromTeamSnapshotJson(teamSnapshotJson ?? undefined);
  const agents: PixelAgent[] = rows.map((r, slotIndex) => {
    const activity = resolvePixelActivity(r, processStatus, tasks, peerReviewers);
    const id = r.subagent.client_uuid;
    const perAgent = hints?.workHintByClientUuid?.[id];
    const workHint =
      activity === "work" ? (perAgent ?? globalHint) : undefined;
    const roleKey = r.subagent.role.trim().toLowerCase();
    const rosterAccent = accentByRole.get(roleKey);
    return {
      client_uuid: id,
      role: r.subagent.role,
      slotIndex,
      spriteCharIndex: spriteCharIndexFromClientUuid(id),
      deskSlot: slotIndex % DESK_SLOT_MOD,
      column: r.column,
      activity,
      workHint,
      torsoAccent: rosterAccent ?? fallbackTorsoAccentFromClientUuid(id),
    };
  });

  return { processStatus, agents };
}
