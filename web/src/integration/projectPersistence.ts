import {
  clearCachedProjectId,
  createRemoteProject,
  deleteRemoteProject,
  getCachedProjectId,
  getOrCreateProjectId,
  hasRemoteProjectBackend,
  listRemoteProjects,
  setCachedProjectId,
} from './api/projectRemoteApi';
import {
  isProjectTitlePromptAbort,
  requestProjectTitle,
} from './projectTitlePrompt';
import { reconcileProjectStoreAfterLoad, useCoreStore } from './store/coreStore';
import { clearProjectScopedUi } from './store/uiStore';

let bootstrapComplete = false;
const afterBootstrapQueue: Array<() => void> = [];
const EMPTY_PROJECTS_RETRY_DELAY_MS = 500;

/**
 * Runs after project bootstrap (server rehydration). When the API is off, runs immediately so
 * localStorage rehydration is the only source of truth.
 */
export function runAfterProjectBootstrap(fn: () => void): void {
  if (!hasRemoteProjectBackend()) {
    fn();
    return;
  }
  if (bootstrapComplete) {
    fn();
    return;
  }
  afterBootstrapQueue.push(fn);
}

function flushAfterBootstrapQueue(): void {
  while (afterBootstrapQueue.length > 0) {
    const f = afterBootstrapQueue.shift();
    try {
      f?.();
    } catch (e) {
      console.warn('[project] after-bootstrap callback', e);
    }
  }
}

export function isProjectBootstrapComplete(): boolean {
  return bootstrapComplete;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

async function pickOrCreateProjectId(): Promise<string> {
  try {
    const { projects } = await listRemoteProjects(100, 0);
    const cached = getCachedProjectId();
    if (projects.length === 0) {
      // Startup can briefly return an empty list before the catalog is fully ready.
      // Prefer a known cached id and avoid prompting until we confirm emptiness.
      if (cached) return cached;
      await sleep(EMPTY_PROJECTS_RETRY_DELAY_MS);
      const retry = await listRemoteProjects(100, 0);
      if (retry.projects.length > 0) {
        return retry.projects[0].id;
      }
      const title = await requestProjectTitle('Name your first project');
      const { id } = await createRemoteProject(title);
      if (!id) throw new Error('Server did not return project id');
      return id;
    }
    if (cached && projects.some((p) => p.id === cached)) {
      return cached;
    }
    return projects[0].id;
  } catch (e) {
    if (isProjectTitlePromptAbort(e)) {
      clearCachedProjectId();
      throw e;
    }
    console.warn('[project] catalog', e);
    return getOrCreateProjectId();
  }
}

/**
 * Saves current in-memory project state for `oldId`, activates `newId`, loads persisted payload from Agent Platform.
 */
export async function switchActiveProject(newId: string): Promise<void> {
  if (!hasRemoteProjectBackend()) return;
  try {
    setCachedProjectId(newId);
    await useCoreStore.persist.rehydrate();
    clearProjectScopedUi();
    useCoreStore.setState({ lastSyncedServerUpdatedAt: Date.now() });
  } catch (e) {
    console.warn('[project] switch project', e);
    useCoreStore.getState().resetProject();
    clearProjectScopedUi();
  } finally {
    reconcileProjectStoreAfterLoad();
  }
}

/**
 * Removes a project on the server. If it was the active project, switches to another listing entry or clears local state.
 */
export async function deleteProjectById(projectId: string): Promise<void> {
  if (!hasRemoteProjectBackend()) throw new Error('Remote project API disabled');
  const wasActive = getCachedProjectId() === projectId;
  await deleteRemoteProject(projectId);
  if (!wasActive) return;
  const { projects } = await listRemoteProjects(100, 0);
  if (projects.length > 0) {
    await switchActiveProject(projects[0].id);
  } else {
    clearCachedProjectId();
    useCoreStore.getState().resetProject();
  }
  useCoreStore.getState().bumpSimSceneReset();
}

export async function createAndSwitchToNewProject(userTitle: string): Promise<string> {
  if (!hasRemoteProjectBackend()) throw new Error('Remote project API disabled');
  const { id } = await createRemoteProject(userTitle);
  if (!id) throw new Error('Server did not return project id');
  setCachedProjectId(id);
  useCoreStore.getState().resetProject();
  useCoreStore.setState({ lastSyncedServerUpdatedAt: Date.now() });
  useCoreStore.getState().bumpSimSceneReset();
  return id;
}

/** After a local workspace reset, persisted state is saved via zustand + project-scoped storage. */
export async function persistClearedProjectWorkspace(): Promise<void> {}

/**
 * Loads project from Agent Platform after core store hydration and ensures an active project id exists.
 */
export function initProjectPersistence(): () => void {
  if (!hasRemoteProjectBackend()) return () => {};

  void Promise.resolve(useCoreStore.persist.rehydrate()).then(async () => {
    try {
      try {
        const id = await pickOrCreateProjectId();
        setCachedProjectId(id);
      } catch (e) {
        if (!isProjectTitlePromptAbort(e)) {
          console.warn('[project] bootstrap', e);
        }
      }
    } finally {
      bootstrapComplete = true;
      reconcileProjectStoreAfterLoad();
      flushAfterBootstrapQueue();
    }
  });
  return () => {};
}
