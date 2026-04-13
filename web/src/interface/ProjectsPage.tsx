import { ArrowLeft, FolderPlus, Loader2, Pencil, Trash2, Wallet } from 'lucide-react';
import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { type AgenticSystem, getAgentSet } from '../data/agents';
import {
  createRemoteProject,
  getCachedProjectId,
  listRemoteProjects,
  patchRemoteProjectMeta,
  type ProjectListEntry,
} from '../integration/api/projectRemoteApi';
import {
  createAndSwitchToNewProject,
  deleteProjectById,
  switchActiveProject,
} from '../integration/projectPersistence';
import {
  armConsultantWorkshopAfterNewProject,
  consultantFirstProjectPlaceholderTitle,
} from '../integration/consultantWorkshopChat';
import {
  isProjectTitlePromptAbort,
  requestNewProjectCreationTitle,
} from '../integration/projectTitlePrompt';
import { useCoreStore } from '../integration/store/coreStore';
import { useTeamStore } from '../integration/store/teamStore';
import ConfirmModal from './components/ConfirmModal';
import { formatTokens } from './formatTokens';
import { showToast } from '../integration/store/toastStore';

function teamLabel(teamId: string, customSystems: AgenticSystem[]): string {
  try {
    return getAgentSet(teamId, customSystems).teamName || teamId;
  } catch {
    return teamId || '—';
  }
}

export const ProjectsPage: React.FC = () => {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const customSystems = useTeamStore((s) => s.customSystems);

  const [projects, setProjects] = useState<ProjectListEntry[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState('');
  const [actionError, setActionError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ProjectListEntry | null>(null);
  const [focusedProjectId, setFocusedProjectId] = useState<string | null>(null);

  const activeId = getCachedProjectId();

  const refresh = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const { projects: list } = await listRemoteProjects(100, 0);
      setProjects(list);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const focus = searchParams.get('focusProject')?.trim();
    if (!focus) return;
    setFocusedProjectId(focus);
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete('focusProject');
      return next;
    }, { replace: true });
  }, [searchParams, setSearchParams]);

  const rows = useMemo(() => projects, [projects]);

  const handleOpen = async (id: string) => {
    setBusyId(id);
    setActionError(null);
    try {
      await switchActiveProject(id);
      useCoreStore.getState().bumpSimSceneReset();
      navigate('/');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyId(null);
    }
  };

  const handleAdd = async () => {
    setActionError(null);
    setBusyId('__new__');
    try {
      const result = await requestNewProjectCreationTitle('New project');
      if (result.discussWithConsultant) {
        await createAndSwitchToNewProject(consultantFirstProjectPlaceholderTitle());
        armConsultantWorkshopAfterNewProject();
        navigate('/');
        await refresh();
      } else {
        const { id } = await createRemoteProject('title' in result ? result.title : '');
        if (!id) throw new Error('Server did not return project id');
        await refresh();
      }
    } catch (err) {
      if (isProjectTitlePromptAbort(err)) return;
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const saveRename = async (id: string) => {
    setBusyId(id);
    setActionError(null);
    try {
      await patchRemoteProjectMeta(id, editTitle);
      setEditingId(null);
      await refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyId(null);
    }
  };

  const openDeleteConfirm = (p: ProjectListEntry) => {
    setDeleteTarget(p);
  };

  const deleteBusy = deleteTarget !== null && busyId === deleteTarget.id;

  const runDeleteProject = async () => {
    if (!deleteTarget) return;
    const id = deleteTarget.id;
    setBusyId(id);
    setActionError(null);
    try {
      await deleteProjectById(id);
      showToast('Project deleted', 'success');
      setDeleteTarget(null);
      await refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyId(null);
    }
  };

  const startRename = (p: ProjectListEntry) => {
    setEditingId(p.id);
    setEditTitle(p.meta.title);
  };

  return (
    <div className="min-h-screen bg-zinc-50 flex flex-col">
      <ConfirmModal
        isOpen={deleteTarget !== null}
        onClose={() => {
          if (!deleteBusy) setDeleteTarget(null);
        }}
        onConfirm={() => void runDeleteProject()}
        title="Delete project?"
        description={
          deleteTarget ? (
            <>
              Delete project <span className="font-semibold text-zinc-700">&quot;{deleteTarget.meta.title}&quot;</span>
              ? This removes its saved workspace data on the server.
            </>
          ) : null
        }
        variant="danger"
        confirmLabel="Delete project"
        cancelLabel="Cancel"
        busy={deleteBusy}
        zIndexClass="z-[120]"
      />

      <header className="h-14 border-b border-zinc-200 bg-white flex items-center justify-between px-4 sm:px-6 shrink-0 gap-3">
        <div className="flex items-center gap-3 min-w-0">
          <Link
            to="/"
            className="flex items-center gap-2 text-zinc-500 hover:text-darkDelegation text-xs font-bold uppercase tracking-wide shrink-0"
          >
            <ArrowLeft size={16} />
            Simulation
          </Link>
          <span className="text-zinc-200">|</span>
          <h1 className="text-sm font-black text-darkDelegation uppercase tracking-widest truncate">Projects</h1>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <Link
            to="/finance"
            className="flex items-center gap-2 px-3 py-1 border border-zinc-200 hover:border-zinc-300 text-zinc-600 hover:text-darkDelegation rounded-lg transition-all h-9"
            title="Portfolio usage and spend"
          >
            <Wallet size={14} />
            <span className="text-[10px] font-black uppercase tracking-wider hidden sm:inline">Finance</span>
          </Link>
          <Link
            to="/finance/project"
            className="text-[10px] font-bold text-emerald-700 hover:text-emerald-800 uppercase tracking-tight hidden sm:inline"
          >
            Current project detail
          </Link>
        </div>
      </header>

      <main className="flex-1 max-w-5xl w-full mx-auto px-4 py-8">
        {actionError && (
          <p className="text-sm text-red-600 mb-4 bg-red-50 border border-red-100 rounded-xl px-4 py-2">{actionError}</p>
        )}

        <div className="flex flex-wrap items-center justify-between gap-3 mb-6">
          <p className="text-sm text-zinc-600">
            Create and organize projects. Open sets the active project for the simulation workspace.
          </p>
          <Button
            type="button"
            disabled={busyId !== null}
            onClick={() => void handleAdd()}
            className="flex items-center gap-2 rounded-xl bg-darkDelegation px-4 py-2 text-[10px] font-black uppercase tracking-wider text-white shadow-lg shadow-black/10 hover:bg-black disabled:pointer-events-none disabled:opacity-40"
          >
            {busyId === '__new__' ? <Loader2 size={14} className="animate-spin" /> : <FolderPlus size={14} />}
            New project
          </Button>
        </div>

        {loadError && <p className="text-sm text-red-600 mb-4">{loadError}</p>}

        {loading ? (
          <div className="flex items-center gap-2 text-zinc-500 text-sm">
            <Loader2 size={18} className="animate-spin" />
            Loading projects…
          </div>
        ) : (
          <div className="bg-white border border-zinc-200 rounded-xl overflow-hidden shadow-sm overflow-x-auto">
            <table className="w-full text-left text-sm min-w-[640px]">
              <thead>
                <tr className="border-b border-zinc-100 bg-zinc-50/80">
                  <th className="px-4 py-3 font-black text-[10px] uppercase tracking-widest text-zinc-500">Project</th>
                  <th className="px-4 py-3 font-black text-[10px] uppercase tracking-widest text-zinc-500">Team</th>
                  <th className="px-4 py-3 font-black text-[10px] uppercase tracking-widest text-zinc-500">Phase</th>
                  <th className="px-4 py-3 font-black text-[10px] uppercase tracking-widest text-zinc-500 text-right">
                    Tokens
                  </th>
                  <th className="px-4 py-3 font-black text-[10px] uppercase tracking-widest text-zinc-500 text-right">
                    Est. cost
                  </th>
                  <th className="px-4 py-3 font-black text-[10px] uppercase tracking-widest text-zinc-500">Updated</th>
                  <th className="px-4 py-3 font-black text-[10px] uppercase tracking-widest text-zinc-500 text-right">
                    Actions
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((p) => {
                  const fin = p.meta.finance ?? { estimatedCostUsd: 0, totalTokens: 0 };
                  const isActive = activeId === p.id;
                  const rowBusy = busyId === p.id;
                  const isEditing = editingId === p.id;
                  return (
                    <tr
                      key={p.id}
                      id={`project-row-${p.id}`}
                      className={`border-b border-zinc-50 hover:bg-zinc-50/50 align-top ${focusedProjectId === p.id ? 'bg-indigo-50/60' : ''}`}
                    >
                      <td className="px-4 py-3">
                        {isEditing ? (
                          <div className="flex flex-col gap-2 max-w-xs">
                            <Input
                              value={editTitle}
                              onChange={(e) => setEditTitle(e.target.value)}
                              className="h-auto w-full rounded-lg border-zinc-200 px-2 py-1.5 text-xs"
                              autoFocus
                              onKeyDown={(e) => {
                                if (e.key === 'Enter') void saveRename(p.id);
                                if (e.key === 'Escape') setEditingId(null);
                              }}
                            />
                            <div className="flex justify-end gap-2">
                              <Button
                                type="button"
                                variant="ghost"
                                className="h-auto px-0 text-[9px] font-bold uppercase text-zinc-500 hover:bg-transparent"
                                onClick={() => setEditingId(null)}
                              >
                                Cancel
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                className="h-auto px-0 text-[9px] font-black uppercase text-darkDelegation hover:bg-transparent"
                                onClick={() => void saveRename(p.id)}
                                disabled={rowBusy}
                              >
                                Save
                              </Button>
                            </div>
                          </div>
                        ) : (
                          <div>
                            <span className="font-bold text-zinc-800">{p.meta.title}</span>
                            {isActive && (
                              <span className="ml-2 text-[9px] font-black uppercase text-emerald-600">Active</span>
                            )}
                            {p.meta.briefPreview ? (
                              <p className="text-[10px] text-zinc-400 mt-1 line-clamp-2 max-w-md">{p.meta.briefPreview}</p>
                            ) : null}
                          </div>
                        )}
                      </td>
                      <td className="px-4 py-3 text-zinc-600 text-xs">{teamLabel(p.meta.teamId, customSystems)}</td>
                      <td className="px-4 py-3 text-zinc-600 capitalize">{p.meta.phase}</td>
                      <td className="px-4 py-3 text-right font-mono text-zinc-800">{formatTokens(fin.totalTokens)}</td>
                      <td className="px-4 py-3 text-right font-mono text-emerald-700 font-bold">
                        ${fin.estimatedCostUsd.toFixed(4)}
                      </td>
                      <td className="px-4 py-3 text-zinc-500 text-xs font-mono">
                        {typeof p.updatedAt === 'string' ? p.updatedAt.slice(0, 10) : '—'}
                      </td>
                      <td className="px-4 py-3 text-right">
                        <div className="flex flex-wrap items-center justify-end gap-1">
                          <Button
                            type="button"
                            variant="secondary"
                            disabled={rowBusy || busyId === '__new__'}
                            onClick={() => void handleOpen(p.id)}
                            className="rounded-lg bg-zinc-100 px-2 py-1 text-[9px] font-black uppercase text-zinc-700 hover:bg-zinc-200 disabled:opacity-40"
                          >
                            {rowBusy ? '…' : 'Open'}
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            disabled={rowBusy || busyId === '__new__' || isEditing}
                            onClick={() => startRename(p)}
                            className="text-zinc-400 hover:bg-zinc-100 hover:text-darkDelegation disabled:opacity-30"
                            title="Rename"
                          >
                            <Pencil size={14} />
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            disabled={rowBusy || busyId === '__new__' || isEditing}
                            onClick={() => openDeleteConfirm(p)}
                            className="text-zinc-400 hover:bg-red-50 hover:text-red-600 disabled:opacity-30"
                            title="Delete"
                          >
                            <Trash2 size={14} />
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </main>
    </div>
  );
};
