import {
  ChevronRight,
  FilePlus,
  FolderOpen,
  FolderPlus,
  RefreshCw,
  Save,
  Trash2,
} from 'lucide-react';
import React, { useCallback, useEffect, useReducer, useState } from 'react';
import {
  ApiError,
  deleteWorkspacePath,
  fetchProcessesList,
  fetchWorkspaceFile,
  fetchWorkspaceInfo,
  fetchWorkspaceList,
  postEnsureProcessWorkspace,
  postWorkspaceMkdir,
  putWorkspaceFile,
} from '../api/client';
import type { ProcessRecord, WorkspaceListEntry } from '../api/types';
import { getCachedProjectId } from '../integration/api/projectRemoteApi';

/** Re-read active project id periodically — same-tab switches do not fire `storage` events. */
function useActiveProjectId(): string | null {
  const [, tick] = useReducer((x: number) => x + 1, 0);
  useEffect(() => {
    const id = window.setInterval(() => tick(), 1500);
    return () => window.clearInterval(id);
  }, []);
  useEffect(() => {
    const onFocus = () => tick();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, []);
  return getCachedProjectId();
}

function truncateGoal(g: string, max = 48): string {
  const t = g.replace(/\s+/g, ' ').trim();
  if (t.length <= max) return t;
  return `${t.slice(0, max - 1)}…`;
}

/** Windows: opens this folder when run in cmd.exe on the machine that hosts the files. */
function formatExplorerCommand(absolutePath: string): string {
  const q = absolutePath.replace(/"/g, '\\"');
  return `explorer "${q}"`;
}

export const DeliverablesPanel: React.FC = () => {
  const activeId = useActiveProjectId();
  const projectNum = activeId ? parseInt(activeId, 10) : NaN;
  const validProject = Number.isFinite(projectNum) && projectNum > 0;

  const [processRows, setProcessRows] = useState<ProcessRecord[]>([]);
  /** 'project' = browse full project tree; number = focus run folder processes/{id}/ */
  const [scopeProcessId, setScopeProcessId] = useState<number | 'project'>('project');

  const [currentDir, setCurrentDir] = useState('');
  const [entries, setEntries] = useState<WorkspaceListEntry[]>([]);
  const [listError, setListError] = useState<string | null>(null);
  const [loadingList, setLoadingList] = useState(false);

  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [editorContent, setEditorContent] = useState('');
  const [dirty, setDirty] = useState(false);
  const [fileLoading, setFileLoading] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [saveStatus, setSaveStatus] = useState<string | null>(null);

  const [pathCopied, setPathCopied] = useState<string | null>(null);
  const [explorerHint, setExplorerHint] = useState<string | null>(null);
  const [openingFolder, setOpeningFolder] = useState(false);

  useEffect(() => {
    if (!validProject) {
      setProcessRows([]);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const { processes } = await fetchProcessesList(80, projectNum);
        if (!cancelled) setProcessRows(processes);
      } catch {
        if (!cancelled) setProcessRows([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [validProject, projectNum]);

  const loadList = useCallback(async () => {
    if (!validProject) return;
    setLoadingList(true);
    setListError(null);
    try {
      const { entries: e } = await fetchWorkspaceList(projectNum, currentDir);
      setEntries(e);
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : String(err);
      setListError(msg);
      setEntries([]);
    } finally {
      setLoadingList(false);
    }
  }, [validProject, projectNum, currentDir]);

  useEffect(() => {
    void loadList();
  }, [loadList]);

  const applyScope = async (next: number | 'project') => {
    if (!validProject) return;
    setScopeProcessId(next);
    setSelectedPath(null);
    setEditorContent('');
    setDirty(false);
    setFileError(null);
    setSaveStatus(null);
    if (next === 'project') {
      setCurrentDir('');
      return;
    }
    try {
      await postEnsureProcessWorkspace(projectNum, next);
      setCurrentDir(`processes/${next}`);
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : String(err);
      setListError(msg);
    }
  };

  const openFile = async (path: string) => {
    if (!validProject) return;
    if (dirty) {
      const ok = window.confirm('Discard unsaved changes and open another file?');
      if (!ok) return;
    }
    setFileLoading(true);
    setFileError(null);
    setSaveStatus(null);
    try {
      const { content } = await fetchWorkspaceFile(projectNum, path);
      setSelectedPath(path);
      setEditorContent(content);
      setDirty(false);
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : String(err);
      setFileError(msg);
    } finally {
      setFileLoading(false);
    }
  };

  const saveFile = async () => {
    if (!validProject || !selectedPath) return;
    setSaveStatus(null);
    setFileError(null);
    try {
      await putWorkspaceFile(projectNum, selectedPath, editorContent);
      setDirty(false);
      setSaveStatus('Saved');
      void loadList();
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : String(err);
      setFileError(msg);
    }
  };

  const newFile = () => {
    const name = window.prompt('New file name (relative to current folder)', 'untitled.txt');
    if (name == null || !name.trim()) return;
    const rel = currentDir ? `${currentDir}/${name.trim()}` : name.trim();
    setSelectedPath(rel);
    setEditorContent('');
    setDirty(true);
    setFileError(null);
    setSaveStatus(null);
  };

  const newFolder = async () => {
    if (!validProject) return;
    const name = window.prompt('New folder name (relative to current folder)', 'folder');
    if (name == null || !name.trim()) return;
    const rel = currentDir ? `${currentDir}/${name.trim()}` : name.trim();
    try {
      await postWorkspaceMkdir(projectNum, rel);
      void loadList();
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : String(err);
      setListError(msg);
    }
  };

  const deleteSelected = async () => {
    if (!validProject || !selectedPath) return;
    const ok = window.confirm(`Delete "${selectedPath}"?`);
    if (!ok) return;
    try {
      await deleteWorkspacePath(projectNum, selectedPath);
      setSelectedPath(null);
      setEditorContent('');
      setDirty(false);
      void loadList();
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : String(err);
      setFileError(msg);
    }
  };

  const copyFolderPathForOs = async () => {
    if (!validProject) return;
    setOpeningFolder(true);
    setPathCopied(null);
    setExplorerHint(null);
    try {
      const info = await fetchWorkspaceInfo(projectNum, currentDir);
      await navigator.clipboard.writeText(info.absolute_path);
      setPathCopied(info.absolute_path);
      setExplorerHint(formatExplorerCommand(info.absolute_path));
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : String(err);
      setListError(msg);
    } finally {
      setOpeningFolder(false);
    }
  };

  if (!validProject) {
    return (
      <div className="rounded-xl border border-dashed border-zinc-200 bg-zinc-50/50 px-4 py-8 text-center">
        <p className="text-xs text-zinc-500 leading-relaxed">
          Select a server-backed project (Projects / project switcher) to use the file sandbox. Deliverables are stored
          per project on the Agent Platform API.
        </p>
      </div>
    );
  }

  const scopeRootDir = scopeProcessId === 'project' ? '' : `processes/${scopeProcessId}`;
  let relForBreadcrumb = currentDir;
  if (scopeProcessId !== 'project' && currentDir.startsWith(scopeRootDir)) {
    relForBreadcrumb = currentDir.slice(scopeRootDir.length).replace(/^\//, '');
  }
  const tailParts = relForBreadcrumb ? relForBreadcrumb.split('/').filter(Boolean) : [];

  return (
    <div className="flex flex-col gap-3 min-h-0 text-left">
      <div className="space-y-1.5">
        <p className="text-[10px] font-bold uppercase tracking-widest text-zinc-400">Workspace</p>
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:flex-wrap">
          <label className="flex flex-col gap-0.5 text-[10px] text-zinc-500 min-w-0 flex-1 sm:max-w-[280px]">
            <span className="font-semibold uppercase tracking-wider">Browse by project / run</span>
            <select
              className="rounded-lg border border-zinc-200 bg-white px-2 py-1.5 text-[11px] text-zinc-800 font-medium"
              value={scopeProcessId === 'project' ? 'project' : String(scopeProcessId)}
              onChange={(ev) => {
                const v = ev.target.value;
                if (v === 'project') void applyScope('project');
                else void applyScope(parseInt(v, 10));
              }}
            >
              <option value="project">Project (all folders)</option>
              {processRows.map((p) => (
                <option key={p.id} value={p.id}>
                  Run #{p.id} — {truncateGoal(p.goal || '')}
                </option>
              ))}
            </select>
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              className="inline-flex items-center gap-1.5 rounded-lg border border-emerald-200 bg-emerald-50 px-2.5 py-1.5 text-[10px] font-bold uppercase tracking-wider text-emerald-900 hover:bg-emerald-100 disabled:opacity-50"
              onClick={() => void copyFolderPathForOs()}
              disabled={openingFolder}
              title="Copy the server folder path for this view (open in Explorer/Finder on the host that stores files)"
            >
              <FolderOpen className="size-4 shrink-0" strokeWidth={2} />
              {openingFolder ? '…' : 'Open folder'}
            </button>
          </div>
        </div>
        {pathCopied && (
          <div className="rounded-lg border border-emerald-100 bg-emerald-50/80 px-3 py-2 text-[10px] text-emerald-900 space-y-1.5">
            <p>
              <span className="font-bold">Path copied.</span> Browsers cannot launch the OS directly. On the{' '}
              <strong>machine where the API stores files</strong> (often your server), paste into File Explorer’s address
              bar, or run:
            </p>
            {explorerHint && (
              <code className="block break-all rounded bg-white/80 px-2 py-1 font-mono text-[9px] text-zinc-800 border border-emerald-100">
                {explorerHint}
              </code>
            )}
            <p className="text-emerald-800/90">
              Runs live under <span className="font-mono">processes/&lt;run id&gt;/</span>; DAG tools write there when a
              process is linked to this project.
            </p>
          </div>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2 text-[10px] text-zinc-500">
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded border border-zinc-200 bg-white px-2 py-1 font-semibold uppercase tracking-wider hover:bg-zinc-50"
          onClick={() => void loadList()}
          disabled={loadingList}
        >
          <RefreshCw className={`size-3 ${loadingList ? 'animate-spin' : ''}`} />
          Refresh
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded border border-zinc-200 bg-white px-2 py-1 font-semibold uppercase tracking-wider hover:bg-zinc-50"
          onClick={newFile}
        >
          <FilePlus className="size-3" />
          New file
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded border border-zinc-200 bg-white px-2 py-1 font-semibold uppercase tracking-wider hover:bg-zinc-50"
          onClick={() => void newFolder()}
        >
          <FolderPlus className="size-3" />
          New folder
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-1 text-[11px] text-zinc-600 font-mono border-b border-zinc-100 pb-2">
        <button type="button" className="hover:underline text-violet-600" onClick={() => setCurrentDir(scopeRootDir)}>
          {scopeProcessId === 'project' ? 'Project' : `Run #${scopeProcessId}`}
        </button>
        {tailParts.map((part, i) => {
          const subRel = tailParts.slice(0, i + 1).join('/');
          const subPath =
            scopeProcessId !== 'project' && scopeRootDir
              ? `${scopeRootDir}/${subRel}`
              : subRel;
          return (
            <React.Fragment key={subPath}>
              <ChevronRight className="size-3 text-zinc-300 inline" />
              <button
                type="button"
                className="hover:underline text-violet-600"
                onClick={() => setCurrentDir(subPath)}
              >
                {part}
              </button>
            </React.Fragment>
          );
        })}
      </div>

      {listError && <p className="text-[11px] text-red-600">{listError}</p>}

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 min-h-0 flex-1">
        <div className="rounded-lg border border-zinc-200 bg-zinc-50/80 min-h-[140px] max-h-[220px] overflow-y-auto">
          {entries.length === 0 && !loadingList ? (
            <p className="text-[11px] text-zinc-400 p-3">Empty folder</p>
          ) : (
            <ul className="p-1">
              {entries.map((e) => (
                <li key={e.path}>
                  <button
                    type="button"
                    className={`w-full text-left px-2 py-1.5 rounded text-[11px] font-mono hover:bg-white ${
                      selectedPath === e.path ? 'bg-violet-100 text-violet-900' : 'text-zinc-700'
                    }`}
                    onClick={() => {
                      if (e.type === 'dir') {
                        setCurrentDir(e.path);
                      } else {
                        void openFile(e.path);
                      }
                    }}
                  >
                    {e.type === 'dir' ? '📁' : '📄'} {e.name}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="flex flex-col gap-2 min-h-0">
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              disabled={!selectedPath || fileLoading}
              className="inline-flex items-center gap-1 rounded bg-violet-600 text-white px-3 py-1.5 text-[11px] font-bold uppercase tracking-wider disabled:opacity-40"
              onClick={() => void saveFile()}
            >
              <Save className="size-3.5" />
              Save
            </button>
            <button
              type="button"
              disabled={!selectedPath}
              className="inline-flex items-center gap-1 rounded border border-zinc-200 bg-white px-3 py-1.5 text-[11px] font-bold uppercase tracking-wider text-zinc-600 disabled:opacity-40"
              onClick={() => void deleteSelected()}
            >
              <Trash2 className="size-3.5" />
              Delete
            </button>
            {saveStatus && <span className="text-[10px] text-emerald-600 self-center">{saveStatus}</span>}
          </div>
          {selectedPath && (
            <p className="text-[10px] font-mono text-zinc-500 truncate" title={selectedPath}>
              {selectedPath}
            </p>
          )}
          {fileError && <p className="text-[11px] text-red-600">{fileError}</p>}
          <textarea
            className="flex-1 min-h-[160px] w-full rounded-lg border border-zinc-200 bg-white p-2 text-[12px] font-mono text-zinc-800 leading-relaxed resize-y"
            placeholder={selectedPath ? 'Edit file…' : 'Select a file or create a new one'}
            value={editorContent}
            disabled={fileLoading}
            onChange={(ev) => {
              setEditorContent(ev.target.value);
              setDirty(true);
              setSaveStatus(null);
            }}
          />
          {dirty && <p className="text-[10px] text-amber-700">Unsaved changes</p>}
        </div>
      </div>
    </div>
  );
};
