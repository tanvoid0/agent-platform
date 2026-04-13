/**
 * Contracts for agent-platform FastAPI JSON responses.
 * Keep aligned with `agent-platform/app/models.py` and route payloads.
 */

export type ProcessStatus =
  | "pending"
  | "planning"
  | "approval_required"
  | "approved"
  | "task_review_required"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

/** Planner DAG JSON (validated on the server; mirrors `app/dag_schema.py`). */
export interface SubagentNode {
  client_uuid: string;
  role: string;
  system_prompt: string;
  instructions: string;
  dependencies?: string[];
  /**
   * Optional llm-orchestrator chat `model` alias (same as OpenAI `model` on POST /v1/chat/completions).
   * Not the agent’s “persona” or skill — that is `role` / prompts. Omit to use server/env defaults.
   */
  model?: string | null;
  /** When true, backend may append child tasks after this node completes (env limits). */
  subdecompose?: boolean;
  /** When true, task pauses for review after LLM output. */
  requires_review?: boolean;
}

export interface PlannerDag {
  team_name: string;
  goal_restatement: string;
  subagents: SubagentNode[];
}

/** OpenAI-compatible chat message for POST /api/v1/chat. */
export interface ChatCompletionMessage {
  role: string;
  content: string;
}

export interface ProcessRecord {
  id: number;
  goal: string;
  status: ProcessStatus;
  dag_json: string | null;
  failure_reason: string | null;
  total_tokens: number;
  total_cost: number;
  tool_invocations_used?: number;
  team_template_id?: number | null;
  team_snapshot_json?: string | null;
  created_at: string;
  updated_at: string;
  /** User-facing project grouping; optional for legacy rows. */
  project_id?: number | null;
}

export interface ProjectSummary {
  id: number;
  name: string;
  description: string | null;
  color: string | null;
  created_at: string;
  updated_at: string;
}

export interface ProjectsListResponse {
  projects: ProjectSummary[];
}

/** Matches `queryKeys.processes.list` third segment for cache keys. */
export type ProcessListProjectFilter = "all" | "unassigned" | number;

/** Output kind for the role; server maps to concrete models later. Only `text` is supported today. */
export type RoleModality = "text" | "audio" | "video" | "image";

/** Mirrors `app/team_schema.py` roster JSON. */
export interface RosterRole {
  id: string;
  name: string;
  description?: string;
  /** Declared output modality; orchestrator resolves LLM later. Defaults to `text`. */
  modality?: RoleModality;
  parent_id?: string | null;
  /** Optional #hex for map chrome; planner ignores. */
  accent_color?: string | null;
}

export interface TeamRoster {
  roles: RosterRole[];
}

export interface TeamTemplateSummary {
  id: number;
  name: string;
  description: string | null;
  color: string | null;
  /** Optional library grouping (card chip, filter). */
  category: string | null;
  /** Roles in the roster (from list endpoint; 0 if roster JSON is invalid). */
  role_count: number;
  created_at: string;
  updated_at: string;
}

export interface TeamTemplateDetail extends TeamTemplateSummary {
  roster: TeamRoster;
}

export interface TeamsListResponse {
  teams: TeamTemplateSummary[];
}

export interface TaskNodeRecord {
  id: number;
  process_id: number;
  client_uuid: string;
  /** Present when this task was spawned by sub-DAG expansion under another node. */
  parent_client_uuid?: string | null;
  role: string;
  system_prompt: string;
  instructions: string;
  llm_model: string | null;
  dependencies_json: string;
  status: string;
  requires_review?: boolean;
  /** Peer subagent assigned to review this output (server picks idle, DAG-relevant peer). */
  reviewer_client_uuid?: string | null;
  review_feedback?: string | null;
  revision_count?: number;
  draft_output?: string | null;
  /** Server JSON string with exception details / traceback when status is failed. */
  failure_debug_json?: string | null;
  output: string | null;
  tokens_used: number;
  started_at: string | null;
  completed_at: string | null;
}

export interface ProcessDetailResponse {
  process: ProcessRecord;
  tasks: TaskNodeRecord[];
}

export interface EventLogRecord {
  id: number;
  process_id: number;
  task_id: number | null;
  event_type: string;
  content: string;
  created_at: string;
}

export interface ProcessEventsResponse {
  events: EventLogRecord[];
}

export interface ProcessesListResponse {
  processes: ProcessRecord[];
}

export interface CreateProcessResponse {
  process_id: number;
  status: string;
}

export interface ApproveDagResponse {
  status: string;
  message?: string;
  idempotent?: boolean;
}

export interface CancelProcessResponse {
  status: string;
  idempotent?: boolean;
}

export interface RetryProcessResponse {
  process_id: number;
  status: string;
  retry: "planning" | "execution";
}

/** POST /processes/{id}/sync — recover stuck background work or explain human gates. */
export interface SyncProcessResponse {
  process_id: number;
  process_status: ProcessStatus | string;
  action:
    | "none"
    | "blocked"
    | "aligned_status"
    | "requeued_plan"
    | "requeued_execution";
  detail: string;
  task_counts?: Record<string, number>;
  reset_running_tasks?: number;
}

export interface RetryTaskResponse {
  process_id: number;
  task_id: number;
  status: string;
  retry: "task";
}

export interface ApiErrorBody {
  detail?: string | string[];
}

export type ReviewDecision = "approve" | "reject" | "request_changes";

export interface ReviewTaskBody {
  decision: ReviewDecision;
  output?: string | null;
  feedback?: string | null;
  instructions?: string | null;
}

export interface ReviewTaskResponse {
  status: string;
  message?: string;
  revision_count?: number;
  idempotent?: boolean;
}
