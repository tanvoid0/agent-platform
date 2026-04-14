import type {
  ApiErrorBody,
  ApproveDagResponse,
  CancelProcessResponse,
  ChatCompletionMessage,
  CreateProcessResponse,
  ProcessDetailResponse,
  ProcessEventsResponse,
  ProcessesListResponse,
  ProjectSummary,
  ProjectsListResponse,
  RetryProcessResponse,
  RetryTaskResponse,
  SyncProcessResponse,
  ReviewTaskBody,
  ReviewTaskResponse,
  TeamRoster,
  TeamTemplateDetail,
  TeamsListResponse,
  ProcessListProjectFilter,
} from "./types";

export class ApiError extends Error {
  readonly status: number;
  readonly body: unknown;

  constructor(message: string, status: number, body: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
  }
}

/**
 * API origin for fetches. In production the Flow UI is served from the same host as FastAPI, so relative `/processes` works.
 * In Vite dev, call the agent-platform API directly (default `http://127.0.0.1:18410`) so traffic does not rely on the dev-server proxy.
 * Override with `VITE_API_ORIGIN` (e.g. another port or `http://host.docker.internal:18410`).
 * If the value is a full URL with a path (e.g. `http://host:18410/flow`), only the origin is used so paths like `/projects/` do not become `/flow/projects/` (which 404s).
 */
function apiOrigin(): string {
  const raw = import.meta.env.VITE_API_ORIGIN as string | undefined;
  if (typeof raw === "string" && raw.trim() !== "") {
    const t = raw.trim();
    try {
      return new URL(t).origin;
    } catch {
      return t.replace(/\/$/, "");
    }
  }
  if (import.meta.env.DEV) {
    return "http://127.0.0.1:18410";
  }
  return "";
}

export function apiUrl(path: string): string {
  const p = path.startsWith("/") ? path : `/${path}`;
  const origin = apiOrigin();
  return origin ? `${origin}${p}` : p;
}

/** Merged into Flow API requests when `VITE_AGENT_PLATFORM_MASTER_KEY` is set. */
export function agentPlatformAuthHeaders(): Record<string, string> {
  const key = import.meta.env.VITE_AGENT_PLATFORM_MASTER_KEY as string | undefined;
  if (typeof key === "string" && key.trim() !== "") {
    return { Authorization: `Bearer ${key.trim()}` };
  }
  return {};
}

function withAuthHeaders(init?: RequestInit): RequestInit {
  const auth = agentPlatformAuthHeaders();
  if (Object.keys(auth).length === 0) return init ?? {};
  const headers = new Headers(init?.headers);
  for (const [k, v] of Object.entries(auth)) {
    if (!headers.has(k)) headers.set(k, v);
  }
  return { ...init, headers };
}

async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  const p = path.startsWith("/") ? path : `/${path}`;
  return fetch(apiUrl(p), withAuthHeaders(init));
}

async function parseJson<T>(r: Response): Promise<T> {
  const text = await r.text();
  if (!text) return {} as T;
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new ApiError("Invalid JSON from server", r.status, text);
  }
}

function detailMessage(data: unknown): string {
  if (data && typeof data === "object" && "detail" in data) {
    const d = (data as ApiErrorBody).detail;
    if (typeof d === "string") return d;
    if (Array.isArray(d)) return d.map(String).join("; ");
  }
  return "Request failed";
}

export async function fetchProcessesList(
  limit = 50,
  projectFilter: ProcessListProjectFilter = "all",
): Promise<ProcessesListResponse> {
  const sp = new URLSearchParams();
  sp.set("limit", String(limit));
  if (projectFilter === "unassigned") {
    sp.set("unassigned_only", "true");
  } else if (typeof projectFilter === "number") {
    sp.set("project_id", String(projectFilter));
  }
  const r = await apiFetch(`/processes?${sp.toString()}`);
  const data = await parseJson<ProcessesListResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ProcessesListResponse;
}

export async function fetchProcessDetail(processId: number): Promise<ProcessDetailResponse> {
  const r = await apiFetch(`/processes/${encodeURIComponent(String(processId))}`);
  const data = await parseJson<ProcessDetailResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ProcessDetailResponse;
}

export async function fetchProcessEvents(
  processId: number,
  opts?: { eventType?: string; limit?: number; afterId?: number },
): Promise<ProcessEventsResponse> {
  const sp = new URLSearchParams();
  if (opts?.eventType?.trim()) sp.set("event_type", opts.eventType.trim());
  if (opts?.limit != null) sp.set("limit", String(opts.limit));
  if (opts?.afterId != null) sp.set("after_id", String(opts.afterId));
  const q = sp.toString();
  const path = `/processes/${encodeURIComponent(String(processId))}/events${q ? `?${q}` : ""}`;
  const r = await apiFetch(path);
  const data = await parseJson<ProcessEventsResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ProcessEventsResponse;
}

export async function createProcess(
  goal: string,
  options: { autoApprove?: boolean; teamTemplateId: number; projectId?: number | null },
): Promise<CreateProcessResponse> {
  const body: {
    goal: string;
    auto_approve?: boolean;
    team_template_id: number;
    project_id?: number;
  } = { goal, team_template_id: options.teamTemplateId };
  if (options.autoApprove) {
    body.auto_approve = true;
  }
  if (options.projectId != null && options.projectId > 0) {
    body.project_id = options.projectId;
  }
  const r = await apiFetch("/processes", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<CreateProcessResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as CreateProcessResponse;
}

export async function approveDag(processId: number, dagJson: string): Promise<ApproveDagResponse> {
  const r = await apiFetch(`/processes/${encodeURIComponent(String(processId))}/approve`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ dag_json: dagJson }),
  });
  const data = await parseJson<ApproveDagResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ApproveDagResponse;
}

export async function cancelProcess(processId: number): Promise<CancelProcessResponse> {
  const r = await apiFetch(`/processes/${encodeURIComponent(String(processId))}/cancel`, {
    method: "POST",
  });
  const data = await parseJson<CancelProcessResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as CancelProcessResponse;
}

export async function retryProcess(processId: number): Promise<RetryProcessResponse> {
  const r = await apiFetch(`/processes/${encodeURIComponent(String(processId))}/retry`, {
    method: "POST",
  });
  const data = await parseJson<RetryProcessResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as RetryProcessResponse;
}

export async function syncProcess(processId: number): Promise<SyncProcessResponse> {
  const r = await apiFetch(`/processes/${encodeURIComponent(String(processId))}/sync`, {
    method: "POST",
  });
  const data = await parseJson<SyncProcessResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as SyncProcessResponse;
}

export async function retryFailedTask(
  processId: number,
  taskId: number,
): Promise<RetryTaskResponse> {
  const r = await apiFetch(
    `/processes/${encodeURIComponent(String(processId))}/tasks/${encodeURIComponent(String(taskId))}/retry`,
    { method: "POST" },
  );
  const data = await parseJson<RetryTaskResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as RetryTaskResponse;
}

export async function reviewTask(
  processId: number,
  taskId: number,
  body: ReviewTaskBody,
): Promise<ReviewTaskResponse> {
  const r = await apiFetch(
    `/processes/${encodeURIComponent(String(processId))}/tasks/${encodeURIComponent(String(taskId))}/review`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
  );
  const data = await parseJson<ReviewTaskResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ReviewTaskResponse;
}

const TEAMS_PREFIX = "/teams/";

export async function fetchTeamsList(): Promise<TeamsListResponse> {
  const r = await apiFetch(TEAMS_PREFIX);
  const data = await parseJson<TeamsListResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as TeamsListResponse;
}

export async function fetchTeamDetail(teamId: number): Promise<TeamTemplateDetail> {
  const r = await apiFetch(`${TEAMS_PREFIX}${encodeURIComponent(String(teamId))}`);
  const data = await parseJson<TeamTemplateDetail | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as TeamTemplateDetail;
}

export async function createTeam(body: {
  name: string;
  description?: string | null;
  color?: string | null;
  category?: string | null;
  roster: TeamRoster;
}): Promise<TeamTemplateDetail> {
  const r = await apiFetch(TEAMS_PREFIX, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<TeamTemplateDetail | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as TeamTemplateDetail;
}

export async function updateTeam(
  teamId: number,
  body: Partial<{
    name: string;
    description: string | null;
    color: string | null;
    category: string | null;
    roster: TeamRoster;
  }>,
): Promise<TeamTemplateDetail> {
  const r = await apiFetch(`${TEAMS_PREFIX}${encodeURIComponent(String(teamId))}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<TeamTemplateDetail | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as TeamTemplateDetail;
}

export async function deleteTeam(teamId: number): Promise<{ ok: boolean }> {
  const r = await apiFetch(`${TEAMS_PREFIX}${encodeURIComponent(String(teamId))}`, {
    method: "DELETE",
  });
  const data = await parseJson<{ ok?: boolean } | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { ok: boolean };
}

const PROJECTS_PREFIX = "/projects/";

export async function fetchProjectsList(): Promise<ProjectsListResponse> {
  const r = await apiFetch(PROJECTS_PREFIX);
  const data = await parseJson<ProjectsListResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ProjectsListResponse;
}

export async function fetchProjectDetail(projectId: number): Promise<ProjectSummary> {
  const r = await apiFetch(`${PROJECTS_PREFIX}${encodeURIComponent(String(projectId))}`);
  const data = await parseJson<ProjectSummary | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ProjectSummary;
}

export async function createProject(body: {
  name: string;
  description?: string | null;
  color?: string | null;
}): Promise<ProjectSummary> {
  const r = await apiFetch(PROJECTS_PREFIX, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<ProjectSummary | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ProjectSummary;
}

export async function updateProject(
  projectId: number,
  body: Partial<{ name: string; description: string | null; color: string | null }>,
): Promise<ProjectSummary> {
  const r = await apiFetch(`${PROJECTS_PREFIX}${encodeURIComponent(String(projectId))}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<ProjectSummary | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ProjectSummary;
}

export async function deleteProject(projectId: number): Promise<{ ok: boolean }> {
  const r = await apiFetch(`${PROJECTS_PREFIX}${encodeURIComponent(String(projectId))}`, {
    method: "DELETE",
  });
  const data = await parseJson<{ ok?: boolean } | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { ok: boolean };
}

function extractChatAssistantContent(data: unknown): string {
  if (data && typeof data === "object") {
    const o = data as {
      choices?: Array<{ message?: { content?: string } }>;
      raw?: unknown;
    };
    if (Array.isArray(o.choices) && o.choices.length > 0) {
      const c = o.choices[0]?.message?.content;
      if (typeof c === "string") return c;
    }
    if (typeof o.raw === "string") return o.raw;
  }
  return "";
}

/** Stateless chat completion via the embedded LLM proxy (POST /api/v1/chat). */
export async function postChatCompletion(body: {
  messages: ChatCompletionMessage[];
  model?: string | null;
  temperature?: number | null;
  max_tokens?: number | null;
}): Promise<{ content: string; raw: unknown }> {
  const payload: Record<string, unknown> = {
    messages: body.messages.map((m) => ({ role: m.role, content: m.content })),
  };
  const m = body.model?.trim();
  if (m) payload.model = m;
  if (body.temperature != null) payload.temperature = body.temperature;
  if (body.max_tokens != null) payload.max_tokens = body.max_tokens;

  const r = await apiFetch("/api/v1/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return { content: extractChatAssistantContent(data), raw: data };
}
