import {
  createProject as apCreateProject,
  deleteProject as apDeleteProject,
  fetchProjectsList,
  updateProject as apUpdateProject,
} from '../../api/client';
import type { ProjectSummary } from '../../api/types';

const PROJECT_STORAGE_KEY = 'agent-platform-active-project-id';

let memoryProjectId: string | null = null;

function isValidCachedProjectId(id: string): boolean {
  if (/^\d{1,20}$/.test(id)) return true;
  return /^[a-zA-Z0-9_-]{8,128}$/.test(id);
}

function mapApProjectToListEntry(p: ProjectSummary): ProjectListEntry {
  return {
    id: String(p.id),
    updatedAt: p.updated_at,
    meta: {
      title: p.name,
      teamId: '',
      phase: '',
      briefPreview: p.description ?? '',
    },
  };
}

export interface ProjectFinanceMeta {
  estimatedCostUsd: number;
  totalTokens: number;
}

export interface ProjectListMeta {
  title: string;
  teamId: string;
  phase: string;
  briefPreview: string;
  finance?: ProjectFinanceMeta;
}

export interface ProjectListEntry {
  id: string;
  updatedAt: string;
  meta: ProjectListMeta;
}

/** Server-backed projects (Agent Platform REST) are always used when the app is configured. */
export function hasRemoteProjectBackend(): boolean {
  return true;
}

export function getCachedProjectId(): string | null {
  if (memoryProjectId && isValidCachedProjectId(memoryProjectId)) {
    return memoryProjectId;
  }
  const ls = localStorage.getItem(PROJECT_STORAGE_KEY);
  if (ls && isValidCachedProjectId(ls)) {
    memoryProjectId = ls;
    return ls;
  }
  return null;
}

export function setCachedProjectId(id: string): void {
  if (!isValidCachedProjectId(id)) {
    throw new Error('Invalid project id');
  }
  memoryProjectId = id;
  localStorage.setItem(PROJECT_STORAGE_KEY, id);
}

export function clearCachedProjectId(): void {
  memoryProjectId = null;
  try {
    localStorage.removeItem(PROJECT_STORAGE_KEY);
  } catch {
    /* ignore */
  }
}

/** Used when catalog bootstrap has not finished yet (no header). */
export function getOrCreateProjectId(): string {
  const existing = getCachedProjectId();
  if (existing) return existing;
  const id = `proj_${Date.now()}_${Math.random().toString(36).slice(2, 12)}`;
  setCachedProjectId(id);
  return id;
}

function normalizeNewProjectUserTitle(userTitle: string): string {
  const t = userTitle.trim().slice(0, 200);
  if (!t) throw new Error('Project name is required');
  return t;
}

export async function listRemoteProjects(limit = 100, skip = 0): Promise<{ projects: ProjectListEntry[] }> {
  const { projects } = await fetchProjectsList();
  const sliced = skip > 0 ? projects.slice(skip) : projects;
  const limited = limit > 0 ? sliced.slice(0, limit) : sliced;
  return { projects: limited.map(mapApProjectToListEntry) };
}

export async function createRemoteProject(userTitle: string): Promise<{ id: string; updatedAt?: string }> {
  const trimmed = normalizeNewProjectUserTitle(userTitle);
  const p = await apCreateProject({
    name: trimmed,
    description: null,
    color: null,
  });
  return { id: String(p.id), updatedAt: p.updated_at };
}

export async function patchRemoteProjectMeta(projectId: string, userTitle: string): Promise<void> {
  const id = parseInt(projectId, 10);
  if (!Number.isFinite(id) || id <= 0) throw new Error('Invalid project id');
  await apUpdateProject(id, { name: normalizeNewProjectUserTitle(userTitle) });
}

export async function deleteRemoteProject(projectId: string): Promise<void> {
  const id = parseInt(projectId, 10);
  if (!Number.isFinite(id) || id <= 0) throw new Error('Invalid project id');
  await apDeleteProject(id);
}
