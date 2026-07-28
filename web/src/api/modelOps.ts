import { ApiError, agentPlatformAuthHeaders, apiUrl } from "./client";

const PREFIX = "/api/v1/model-ops";

async function parseJson<T>(r: Response): Promise<T> {
  const text = await r.text();
  if (!text) return {} as T;
  return JSON.parse(text) as T;
}

function detailMessage(data: unknown): string {
  if (data && typeof data === "object" && "detail" in data) {
    const d = (data as { detail: unknown }).detail;
    if (typeof d === "string") return d;
    if (Array.isArray(d)) return d.map(String).join("; ");
  }
  return "Request failed";
}

async function modelOpsFetch(path: string, init?: RequestInit): Promise<Response> {
  const p = path.startsWith("/") ? path : `/${path}`;
  const headers = new Headers(init?.headers);
  for (const [k, v] of Object.entries(agentPlatformAuthHeaders())) {
    if (!headers.has(k)) headers.set(k, v);
  }
  return fetch(apiUrl(`${PREFIX}${p}`), { ...init, headers });
}

export type ModelProjectOut = {
  id: number;
  name: string;
  description?: string | null;
  manifest: Record<string, unknown>;
  registry_entries: ModelRegistryEntryOut[];
};

export type ModelRegistryEntryOut = {
  id: number;
  project_id: number;
  project_name?: string | null;
  version: string;
  ollama_tag: string;
  base_model?: string | null;
  eval_score?: number | null;
  is_active: boolean;
};

export type ModelBuildJobOut = {
  id: number;
  job_type: string;
  project_id?: number | null;
  project_name?: string | null;
  stages: string[];
  status: string;
  current_stage?: string | null;
  register_alias?: string | null;
  result: Record<string, unknown>;
  error_message?: string | null;
  log_tail?: string | null;
  poll_url: string;
  stream_url: string;
  created_at: string;
  started_at?: string | null;
  finished_at?: string | null;
};

export type OllamaModelSummary = {
  name: string;
  size?: number;
  modified_at?: string;
  details?: Record<string, unknown>;
};

export async function fetchModelProjects(): Promise<{ projects: ModelProjectOut[] }> {
  const r = await modelOpsFetch("/projects");
  const data = await parseJson<{ projects: ModelProjectOut[] } | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { projects: ModelProjectOut[] };
}

export async function createModelProject(body: {
  name: string;
  description?: string | null;
  base_model?: string | null;
  ollama_tag?: string | null;
}): Promise<ModelProjectOut> {
  const r = await modelOpsFetch("/projects", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<ModelProjectOut | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ModelProjectOut;
}

export async function fetchModelProject(name: string): Promise<ModelProjectOut> {
  const r = await modelOpsFetch(`/projects/${encodeURIComponent(name)}`);
  const data = await parseJson<ModelProjectOut | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ModelProjectOut;
}

export async function startModelBuildJob(body: {
  project: string;
  stages: string[];
  register_alias?: string | null;
  offline_eval?: boolean;
  process_id?: number | null;
}): Promise<ModelBuildJobOut> {
  const r = await modelOpsFetch("/jobs", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await parseJson<ModelBuildJobOut | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ModelBuildJobOut;
}

export async function fetchModelBuildJob(jobId: number): Promise<ModelBuildJobOut> {
  const r = await modelOpsFetch(`/jobs/${jobId}`);
  const data = await parseJson<ModelBuildJobOut | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ModelBuildJobOut;
}

export async function fetchOllamaModels(): Promise<{ models: OllamaModelSummary[] }> {
  const r = await modelOpsFetch("/ollama/models");
  const data = await parseJson<{ models: OllamaModelSummary[] } | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { models: OllamaModelSummary[] };
}

export async function pullOllamaModelAsync(name: string): Promise<ModelBuildJobOut> {
  const r = await modelOpsFetch("/ollama/models/pull", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, async: true }),
  });
  const data = await parseJson<ModelBuildJobOut | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as ModelBuildJobOut;
}

export async function fetchModelRegistry(): Promise<{ entries: ModelRegistryEntryOut[] }> {
  const r = await modelOpsFetch("/registry");
  const data = await parseJson<{ entries: ModelRegistryEntryOut[] } | { detail?: unknown }>(r);
  if (!r.ok) throw new ApiError(detailMessage(data), r.status, data);
  return data as { entries: ModelRegistryEntryOut[] };
}
