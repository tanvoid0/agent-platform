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
  WorkspaceListResponse,
  WorkspaceInfoResponse,
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

/** Origin used for Agent Platform API calls (for settings UI). Same rules as `apiUrl` (dev default, `VITE_API_ORIGIN`, or same-origin). */
export function getAgentPlatformApiOriginForDisplay(): string {
  const o = apiOrigin();
  if (o) return o;
  if (typeof window !== "undefined") return window.location.origin;
  return "";
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

/** Max 24h — avoids accidental huge env values. */
const MAX_TIMEOUT_MS = 86_400_000;
const DEFAULT_FETCH_TIMEOUT_MS = 120_000;
const DEFAULT_CHAT_TIMEOUT_MS = 600_000;

function readEnvTimeoutMs(key: string, fallback: number): number {
  try {
    const raw = import.meta.env[key] as string | undefined;
    if (raw === undefined || raw === "") return fallback;
    const n = Number(raw);
    if (!Number.isFinite(n) || n <= 0) return fallback;
    return Math.min(Math.floor(n), MAX_TIMEOUT_MS);
  } catch {
    return fallback;
  }
}

function defaultApiFetchTimeoutMs(): number {
  return readEnvTimeoutMs("VITE_API_FETCH_TIMEOUT_MS", DEFAULT_FETCH_TIMEOUT_MS);
}

function defaultChatFetchTimeoutMs(): number {
  return readEnvTimeoutMs("VITE_API_CHAT_TIMEOUT_MS", DEFAULT_CHAT_TIMEOUT_MS);
}

/**
 * Combines an optional caller `signal` with an AbortSignal.timeout so requests do not hang indefinitely.
 */
function withFetchTimeout(init: RequestInit | undefined, timeoutMs: number): RequestInit {
  const base = init ?? {};
  const userSignal = base.signal;
  const timeoutSignal = AbortSignal.timeout(timeoutMs);
  if (!userSignal) {
    return { ...base, signal: timeoutSignal };
  }
  const controller = new AbortController();
  const forward = () => controller.abort();
  userSignal.addEventListener("abort", forward, { once: true });
  timeoutSignal.addEventListener("abort", forward, { once: true });
  return { ...base, signal: controller.signal };
}

async function apiFetchWithTimeout(
  path: string,
  init: RequestInit | undefined,
  timeoutMs: number,
): Promise<Response> {
  const p = path.startsWith("/") ? path : `/${path}`;
  const merged = withFetchTimeout(withAuthHeaders(init), timeoutMs);
  return fetch(apiUrl(p), merged);
}

async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  return apiFetchWithTimeout(path, init, defaultApiFetchTimeoutMs());
}

async function apiFetchChat(path: string, init?: RequestInit): Promise<Response> {
  return apiFetchWithTimeout(path, init, defaultChatFetchTimeoutMs());
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

const workspaceBase = (projectId: number) =>
  `${PROJECTS_PREFIX}${encodeURIComponent(String(projectId))}/workspace`;

export async function fetchWorkspaceInfo(
  projectId: number,
  path = "",
): Promise<WorkspaceInfoResponse> {
  const sp = new URLSearchParams();
  if (path) sp.set("path", path);
  const q = sp.toString();
  const r = await apiFetch(`${workspaceBase(projectId)}/info${q ? `?${q}` : ""}`);
  const data = await parseJson<WorkspaceInfoResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as WorkspaceInfoResponse;
}

export async function postEnsureProcessWorkspace(
  projectId: number,
  processId: number,
): Promise<{ ok: boolean; absolute_path: string; relative_prefix: string }> {
  const r = await apiFetch(`${workspaceBase(projectId)}/ensure-process`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ process_id: processId }),
  });
  const data = await parseJson<
    { ok?: boolean; absolute_path?: string; relative_prefix?: string } | ApiErrorBody
  >(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { ok: boolean; absolute_path: string; relative_prefix: string };
}

export async function fetchWorkspaceList(
  projectId: number,
  path = "",
): Promise<WorkspaceListResponse> {
  const sp = new URLSearchParams();
  if (path) sp.set("path", path);
  const q = sp.toString();
  const r = await apiFetch(`${workspaceBase(projectId)}/list${q ? `?${q}` : ""}`);
  const data = await parseJson<WorkspaceListResponse | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as WorkspaceListResponse;
}

export async function fetchWorkspaceFile(
  projectId: number,
  path: string,
): Promise<{ path: string; content: string }> {
  const sp = new URLSearchParams();
  sp.set("path", path);
  const r = await apiFetch(`${workspaceBase(projectId)}/file?${sp.toString()}`);
  const data = await parseJson<{ path: string; content: string } | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { path: string; content: string };
}

export async function putWorkspaceFile(
  projectId: number,
  path: string,
  content: string,
): Promise<{ ok: boolean; path: string }> {
  const r = await apiFetch(`${workspaceBase(projectId)}/file`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path, content }),
  });
  const data = await parseJson<{ ok?: boolean; path?: string } | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { ok: boolean; path: string };
}

export async function deleteWorkspacePath(
  projectId: number,
  path: string,
): Promise<{ ok: boolean }> {
  const sp = new URLSearchParams();
  sp.set("path", path);
  const r = await apiFetch(`${workspaceBase(projectId)}/file?${sp.toString()}`, {
    method: "DELETE",
  });
  const data = await parseJson<{ ok?: boolean } | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { ok: boolean };
}

export async function postWorkspaceMkdir(
  projectId: number,
  path: string,
): Promise<{ ok: boolean; path: string }> {
  const r = await apiFetch(`${workspaceBase(projectId)}/mkdir`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path }),
  });
  const data = await parseJson<{ ok?: boolean; path?: string } | ApiErrorBody>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { ok: boolean; path: string };
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

  const r = await apiFetchChat("/api/v1/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return { content: extractChatAssistantContent(data), raw: data };
}

/** Effective chat provider/model from Agent Platform (embedded proxy defaults). Same auth as other /api/v1 routes. */
export async function fetchChatResolvedDefaults(): Promise<{ provider: string; model: string }> {
  const r = await apiFetch("/api/v1/chat/resolved-defaults");
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  const body = data as { provider?: string; model?: string };
  return {
    provider: typeof body.provider === "string" ? body.provider : "",
    model: typeof body.model === "string" ? body.model : "",
  };
}

/** Server-driven LLM provider lists and status (falls back to `model-config.ts` if fetch fails). */
export type LlmUiCatalogProviderJson = {
  id: string;
  label: string;
  configured: boolean;
  reachable: boolean | null;
  chat: { default_model: string; options: string[] };
};

export type LlmUiCatalogMediaSliceJson = {
  default_model: string;
  options: string[];
};

export type LlmUiCatalogJson = {
  resolved_defaults: { provider: string; model: string };
  providers: LlmUiCatalogProviderJson[];
  gemini_media: {
    image: LlmUiCatalogMediaSliceJson;
    music: LlmUiCatalogMediaSliceJson;
    video: LlmUiCatalogMediaSliceJson;
  };
};

export async function fetchLlmUiCatalog(): Promise<LlmUiCatalogJson> {
  const r = await apiFetch("/api/v1/llm/ui-catalog");
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as LlmUiCatalogJson;
}

/** Embedded OpenAI-compatible proxy: server env + config (Settings → LLM proxy). */
export type LlmProxyEnvKeyMeta =
  | { set: boolean; masked: string }
  | { set: boolean; value: string };

export async function fetchLlmProxyEnv(): Promise<{
  keys: Record<string, LlmProxyEnvKeyMeta>;
  effective_defaults?: { OLLAMA_API_BASE?: string; LM_STUDIO_API_BASE?: string; AIMLAPI_OPENAI_BASE?: string };
  /** Same provider/model the embedded proxy uses for unqualified requests (config.yaml + env + first configured provider). */
  resolved_defaults?: { provider: string; model: string };
}> {
  const r = await apiFetch("/api/v1/llm-proxy/env");
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  const body = data as {
    keys?: Record<string, LlmProxyEnvKeyMeta>;
    effective_defaults?: { OLLAMA_API_BASE?: string; LM_STUDIO_API_BASE?: string; AIMLAPI_OPENAI_BASE?: string };
    resolved_defaults?: { provider: string; model: string };
  };
  return {
    keys: body.keys ?? {},
    effective_defaults: body.effective_defaults,
    resolved_defaults: body.resolved_defaults,
  };
}

export async function postLlmProxyEnv(body: {
  AGENT_PLATFORM_MASTER_KEY?: string | null;
  GEMINI_API_KEY?: string | null;
  AIMLAPI_API_KEY?: string | null;
  AIMLAPI_OPENAI_BASE?: string | null;
  OLLAMA_API_BASE?: string | null;
  LM_STUDIO_API_BASE?: string | null;
  LM_STUDIO_API_KEY?: string | null;
  DEFAULT_PROVIDER?: string | null;
  DEFAULT_MODEL?: string | null;
}): Promise<{ ok: boolean; message?: string }> {
  const r = await apiFetch("/api/v1/llm-proxy/env", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { ok: boolean; message?: string };
}

export async function fetchLlmProxyConfigYaml(): Promise<{ content: string }> {
  const r = await apiFetch("/api/v1/llm-proxy/config-yaml");
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  const body = data as { content?: string };
  return { content: body.content ?? "" };
}

export async function fetchLlmProxySnippet(): Promise<{ public_base: string; snippet: string }> {
  const r = await apiFetch("/api/v1/llm-proxy/snippet");
  const data = await parseJson<unknown>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  const body = data as { public_base?: string; snippet?: string };
  return { public_base: body.public_base ?? "", snippet: body.snippet ?? "" };
}
