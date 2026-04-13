import { ApiError, fetchWorkspaceFile, fetchWorkspaceList, putWorkspaceFile } from '../../../api/client';
import { getCachedProjectId } from '../../../integration/api/projectRemoteApi';
import { useCoreStore } from '../../../integration/store/coreStore';
import type { AgentActionContext } from '../ToolRegistry';

function parseProjectId(): number | null {
  const raw = getCachedProjectId();
  if (!raw) return null;
  const n = parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function log(agent: AgentActionContext, message: string) {
  useCoreStore.getState().addLogEntry({
    agentIndex: agent.data.index,
    action: message,
    taskId: undefined,
  });
}

/**
 * List sandbox files (server-backed project). Same behavior as DAG workspace_list.
 */
export async function workspaceListTool(
  agent: AgentActionContext,
  args: { path?: string },
): Promise<boolean> {
  const pid = parseProjectId();
  if (pid == null) {
    log(agent, 'workspace_list skipped — no active server project.');
    return true;
  }
  const path = typeof args.path === 'string' ? args.path : '';
  try {
    const { entries } = await fetchWorkspaceList(pid, path);
    const summary = entries.length
      ? entries.map((e) => `${e.type}:${e.name}`).join(', ')
      : '(empty)';
    log(agent, `workspace_list "${path || '.'}" → ${summary}`);
    return true;
  } catch (e) {
    const msg = e instanceof ApiError ? e.message : String(e);
    log(agent, `workspace_list failed: ${msg}`);
    return true;
  }
}

export async function workspaceReadTool(
  agent: AgentActionContext,
  args: { path?: string },
): Promise<boolean> {
  const pid = parseProjectId();
  if (pid == null) {
    log(agent, 'workspace_read skipped — no active server project.');
    return true;
  }
  const path = typeof args.path === 'string' ? args.path.trim() : '';
  if (!path) {
    log(agent, 'workspace_read failed — path required.');
    return true;
  }
  try {
    const { content } = await fetchWorkspaceFile(pid, path);
    const preview = content.length > 400 ? `${content.slice(0, 400)}…` : content;
    log(agent, `workspace_read "${path}" (${content.length} chars): ${preview}`);
    return true;
  } catch (e) {
    const msg = e instanceof ApiError ? e.message : String(e);
    log(agent, `workspace_read failed: ${msg}`);
    return true;
  }
}

export async function workspaceWriteTool(
  agent: AgentActionContext,
  args: { path?: string; content?: string },
): Promise<boolean> {
  const pid = parseProjectId();
  if (pid == null) {
    log(agent, 'workspace_write skipped — no active server project.');
    return true;
  }
  const path = typeof args.path === 'string' ? args.path.trim() : '';
  const content = typeof args.content === 'string' ? args.content : '';
  if (!path) {
    log(agent, 'workspace_write failed — path required.');
    return true;
  }
  try {
    await putWorkspaceFile(pid, path, content);
    log(agent, `workspace_write "${path}" (${content.length} bytes)`);
    return true;
  } catch (e) {
    const msg = e instanceof ApiError ? e.message : String(e);
    log(agent, `workspace_write failed: ${msg}`);
    return true;
  }
}
