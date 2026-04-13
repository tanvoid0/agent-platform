import { ChevronDown, FolderPlus, Pencil, RefreshCw } from 'lucide-react';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  getCachedProjectId,
  hasRemoteProjectBackend,
  listRemoteProjects,
  patchRemoteProjectMeta,
  type ProjectListEntry,
} from '../integration/api/projectRemoteApi';
import {
  createAndSwitchToNewProject,
  isProjectBootstrapComplete,
  switchActiveProject,
} from '../integration/projectPersistence';
import {
  isProjectTitlePromptAbort,
  requestNewProjectCreationTitle,
} from '../integration/projectTitlePrompt';
import {
  armConsultantWorkshopAfterNewProject,
  consultantFirstProjectPlaceholderTitle,
} from '../integration/consultantWorkshopChat';
import { type AgenticSystem, getAgentSet } from '../data/agents';
import { useTeamStore } from '../integration/store/teamStore';
import { useSceneManager } from '../simulation/SceneContext';

function teamLabel(teamId: string, customSystems: AgenticSystem[]): string {
  try {
    return getAgentSet(teamId, customSystems).teamName || teamId;
  } catch {
    return teamId || '—';
  }
}

export const ProjectSwitcher: React.FC = () => {
  const scene = useSceneManager();
  const customSystems = useTeamStore((s) => s.customSystems);
  const [open, setOpen] = useState(false);
  const [projects, setProjects] = useState<ProjectListEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState('');
  const [menuError, setMenuError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  const enabled = hasRemoteProjectBackend();
  const activeId = getCachedProjectId();

  const refresh = useCallback(async () => {
    if (!enabled) return;
    setLoading(true);
    try {
      const { projects: list } = await listRemoteProjects(100, 0);
      setProjects(list);
    } catch (e) {
      console.warn('[ProjectSwitcher] list', e);
    } finally {
      setLoading(false);
    }
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
  }, [enabled, refresh]);

  useEffect(() => {
    if (!open) return;
    setMenuError(null);
    void refresh();
  }, [open, refresh]);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
        setEditingId(null);
      }
    };
    document.addEventListener('mousedown', onDoc);
    return () => document.removeEventListener('mousedown', onDoc);
  }, [open]);

  const currentTitle = useMemo(() => {
    if (!activeId) return 'Project';
    const row = projects.find((p) => p.id === activeId);
    return row?.meta.title || (activeId.length > 24 ? `${activeId.slice(0, 22)}…` : activeId);
  }, [activeId, projects]);

  const ready = isProjectBootstrapComplete();

  const handleSwitch = async (id: string) => {
    if (id === activeId) {
      setOpen(false);
      return;
    }
    setBusy(true);
    try {
      await switchActiveProject(id);
      scene?.resetScene();
      setOpen(false);
      await refresh();
    } catch (e) {
      console.warn('[ProjectSwitcher] switch', e);
    } finally {
      setBusy(false);
    }
  };

  const handleNew = async (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setMenuError(null);
    setBusy(true);
    try {
      const result = await requestNewProjectCreationTitle('New project');
      const title =
        'title' in result ? result.title : consultantFirstProjectPlaceholderTitle();
      await createAndSwitchToNewProject(title);
      scene?.resetScene();
      setOpen(false);
      await refresh();
      if (result.discussWithConsultant) {
        armConsultantWorkshopAfterNewProject();
      }
    } catch (err) {
      if (isProjectTitlePromptAbort(err)) {
        return;
      }
      const msg = err instanceof Error ? err.message : String(err);
      console.warn('[ProjectSwitcher] new', err);
      setMenuError(msg || 'Could not create project. Is the Agent Platform API reachable?');
    } finally {
      setBusy(false);
    }
  };

  const startRename = (p: ProjectListEntry) => {
    setEditingId(p.id);
    setEditTitle(p.meta.title);
  };

  const saveRename = async (id: string) => {
    try {
      await patchRemoteProjectMeta(id, editTitle);
      setEditingId(null);
      await refresh();
    } catch (e) {
      console.warn('[ProjectSwitcher] rename', e);
    }
  };

  if (!enabled || !ready) return null;

  return (
    <div ref={rootRef} className="relative ml-3 min-w-0 max-w-[min(100%,280px)]">
      <Button
        type="button"
        variant="outline"
        disabled={busy}
        onClick={() => setOpen((o) => !o)}
        className="flex h-auto max-w-full items-center gap-2 rounded-xl border-zinc-200 bg-zinc-50/80 px-3 py-1.5 text-left font-normal hover:bg-zinc-100"
      >
        <span className="text-[10px] font-black uppercase tracking-widest text-zinc-400 shrink-0">Project</span>
        <span className="min-w-0 truncate text-xs font-bold text-darkDelegation">{currentTitle}</span>
        <ChevronDown size={14} className={`shrink-0 text-zinc-400 transition-transform ${open ? 'rotate-180' : ''}`} />
      </Button>

      {open && (
        <div
          className="absolute left-0 top-full mt-1 w-[min(100vw-2rem,320px)] max-h-[70vh] overflow-hidden flex flex-col rounded-xl border border-zinc-200 bg-white shadow-xl z-50"
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-zinc-100 bg-zinc-50/50">
            <span className="text-[9px] font-black uppercase tracking-widest text-zinc-500">All projects</span>
            <div className="flex items-center gap-1">
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                onClick={() => void refresh()}
                className="text-zinc-400 hover:bg-white hover:text-darkDelegation"
                title="Refresh list"
              >
                <RefreshCw size={12} className={loading ? 'animate-spin' : ''} />
              </Button>
              <Button
                type="button"
                disabled={busy}
                onClick={(e) => void handleNew(e)}
                className="flex h-auto items-center gap-1 rounded-lg bg-darkDelegation px-2 py-1 text-[9px] font-black uppercase tracking-wider text-white hover:bg-black disabled:opacity-50"
              >
                <FolderPlus size={12} />
                New
              </Button>
            </div>
          </div>
          {menuError ? (
            <p className="text-[11px] text-red-700 px-3 py-2 border-b border-red-100 bg-red-50/80 max-h-40 overflow-y-auto whitespace-pre-line leading-snug">
              {menuError}
            </p>
          ) : null}
          <div className="overflow-y-auto flex-1 p-1">
            {projects.length === 0 && !loading ? (
              <p className="text-xs text-zinc-400 px-2 py-4 text-center">No projects yet.</p>
            ) : (
              projects.map((p) => (
                <div
                  key={p.id}
                  className={`rounded-lg mb-0.5 ${p.id === activeId ? 'bg-amber-50 border border-amber-100' : 'hover:bg-zinc-50'}`}
                >
                  {editingId === p.id ? (
                    <div className="flex flex-col gap-1 p-2">
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
                      <div className="flex justify-end gap-1">
                        <Button
                          type="button"
                          variant="ghost"
                          className="h-auto px-2 text-[9px] font-bold uppercase text-zinc-500 hover:bg-transparent"
                          onClick={() => setEditingId(null)}
                        >
                          Cancel
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          className="h-auto px-2 text-[9px] font-black uppercase text-darkDelegation hover:bg-transparent"
                          onClick={() => void saveRename(p.id)}
                        >
                          Save
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <div className="flex items-start gap-2 p-2">
                      <Button
                        type="button"
                        variant="ghost"
                        disabled={busy}
                        onClick={() => void handleSwitch(p.id)}
                        className="h-auto min-w-0 flex-1 justify-start text-left font-normal"
                      >
                        <div className="truncate text-xs font-bold text-darkDelegation">{p.meta.title}</div>
                        <div className="mt-0.5 truncate text-[10px] text-zinc-400">
                          {teamLabel(p.meta.teamId, customSystems)} · {p.meta.phase}
                        </div>
                        <div className="mt-0.5 truncate font-mono text-[9px] text-zinc-300">{p.id}</div>
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        className="shrink-0 text-zinc-400 hover:bg-white hover:text-darkDelegation"
                        title="Rename"
                        onClick={() => startRename(p)}
                      >
                        <Pencil size={12} />
                      </Button>
                    </div>
                  )}
                </div>
              ))
            )}
          </div>
          <div className="border-t border-zinc-100 px-2 py-2 bg-zinc-50/30">
            <Link
              to="/projects"
              onClick={() => setOpen(false)}
              className="block text-center text-[9px] font-black uppercase tracking-wider text-darkDelegation hover:underline py-1"
            >
              Projects page
            </Link>
          </div>
        </div>
      )}
    </div>
  );
};
