import { ArrowLeft, Eye, FolderPlus, Loader2, Pencil, Trash2, Wallet } from 'lucide-react';
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

  const handleViewDetails = async (id: string) => {
    setBusyId(id);
    setActionError(null);
    try {
      await switchActiveProject(id);
      navigate('/finance/project');
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
        ) : rows.length === 0 ? (
          <div className="rounded-xl border border-zinc-200 bg-white p-10 text-center shadow-sm">
            <p className="text-sm text-zinc-500">No projects yet. Create your first one to get started.</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {rows.map((p) => {
              const fin = p.meta.finance ?? { estimatedCostUsd: 0, totalTokens: 0 };
              const isActive = activeId === p.id;
              const rowBusy = busyId === p.id;
              const isEditing = editingId === p.id;
              return (
                <article
                  key={p.id}
                  id={`project-row-${p.id}`}
                  className={`rounded-xl border bg-white p-4 shadow-sm transition-colors ${
                    focusedProjectId === p.id
                      ? 'border-indigo-300 bg-indigo-50/40'
                      : 'border-zinc-200 hover:border-zinc-300'
                  }`}
                >
                  <div className="mb-3 flex items-start justify-between gap-2">
                    {isEditing ? (
                      <div className="flex min-w-0 flex-1 flex-col gap-2">
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
                      <div className="min-w-0">
                        <h2 className="truncate text-sm font-bold text-zinc-800">{p.meta.title}</h2>
                        {p.meta.briefPreview ? (
                          <p className="mt-1 line-clamp-2 text-[10px] text-zinc-400">{p.meta.briefPreview}</p>
                        ) : null}
                      </div>
                    )}
                    {isActive ? (
                      <span className="rounded-md bg-emerald-50 px-2 py-0.5 text-[9px] font-black uppercase text-emerald-600">
                        Active
                      </span>
                    ) : null}
                  </div>

                  <div className="grid grid-cols-2 gap-2 rounded-lg border border-zinc-100 bg-zinc-50/70 p-2.5">
                    <div>
                      <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Team</p>
                      <p className="truncate text-[11px] font-semibold text-zinc-700">
                        {teamLabel(p.meta.teamId, customSystems)}
                      </p>
                    </div>
                    <div>
                      <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Phase</p>
                      <p className="text-[11px] font-semibold capitalize text-zinc-700">{p.meta.phase}</p>
                    </div>
                    <div>
                      <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Tokens</p>
                      <p className="text-[11px] font-mono font-semibold text-zinc-700">{formatTokens(fin.totalTokens)}</p>
                    </div>
                    <div>
                      <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">Est. cost</p>
                      <p className="text-[11px] font-mono font-bold text-emerald-700">${fin.estimatedCostUsd.toFixed(4)}</p>
                    </div>
                  </div>

                  <p className="mt-2 text-[10px] font-mono text-zinc-400">
                    Updated: {typeof p.updatedAt === 'string' ? p.updatedAt.slice(0, 10) : '—'}
                  </p>

                  <div className="mt-3 flex flex-wrap items-center justify-between gap-2">
                    <div className="flex items-center gap-1">
                      <Button
                        type="button"
                        variant="secondary"
                        disabled={rowBusy || busyId === '__new__'}
                        onClick={() => void handleOpen(p.id)}
                        className="rounded-lg bg-zinc-100 px-2.5 py-1 text-[9px] font-black uppercase text-zinc-700 hover:bg-zinc-200 disabled:opacity-40"
                      >
                        {rowBusy ? '…' : 'Open'}
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        disabled={rowBusy || busyId === '__new__'}
                        onClick={() => void handleViewDetails(p.id)}
                        className="rounded-lg px-2.5 py-1 text-[9px] font-black uppercase"
                      >
                        <Eye className="mr-1 size-3" />
                        View
                      </Button>
                    </div>
                    <div className="flex items-center gap-1">
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
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </main>
    </div>
  );
};
