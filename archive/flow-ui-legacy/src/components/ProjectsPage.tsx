import { FolderOpen, Library, Plus, Star, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import type { ProjectSummary } from "../api/types";
import { useDefaultProjectId } from "../hooks/useDefaultProjectId";
import {
  useCreateProjectMutation,
  useDeleteProjectMutation,
  useProjectDetailQuery,
  useProjectsListQuery,
  useUpdateProjectMutation,
} from "../hooks/useProjectQueries";
import { parseProjectPathSegment } from "../lib/projectPath";
import { formatRelativeTimeFromIso } from "../lib/formatRelativeTime";
import { AppTopNav } from "./AppTopNav";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

type FormState = {
  name: string;
  description: string;
  color: string;
};

export function ProjectsPage() {
  const navigate = useNavigate();
  const { projectId: projectIdParam } = useParams();

  const [selectedId, setSelectedId] = useState<number | "new" | null>(() =>
    parseProjectPathSegment(projectIdParam),
  );
  const [nameFilter, setNameFilter] = useState("");
  const [deleteConfirm, setDeleteConfirm] = useState<ProjectSummary | null>(null);

  const { data: listData, isPending: listPending, error: listError } = useProjectsListQuery();
  const projects = listData?.projects ?? [];

  const detailId = selectedId === "new" || selectedId === null ? null : selectedId;
  const { data: detail, isPending: detailPending } = useProjectDetailQuery(detailId);

  const [form, setForm] = useState<FormState>(() => ({
    name: "",
    description: "",
    color: "#6366f1",
  }));

  useEffect(() => {
    setSelectedId(parseProjectPathSegment(projectIdParam));
  }, [projectIdParam]);

  useEffect(() => {
    if (selectedId === "new") {
      setForm({ name: "New project", description: "", color: "#6366f1" });
      return;
    }
    if (typeof selectedId === "number" && detail && detail.id === selectedId) {
      setForm({
        name: detail.name,
        description: detail.description ?? "",
        color: detail.color ?? "#6366f1",
      });
    }
  }, [selectedId, detail]);

  const createMut = useCreateProjectMutation();
  const updateMut = useUpdateProjectMutation();
  const deleteMut = useDeleteProjectMutation();
  const [defaultProjectId, setDefaultProjectId] = useDefaultProjectId();

  useEffect(() => {
    if (listPending) return;
    if (defaultProjectId == null) return;
    if (projects.length === 0) return;
    if (!projects.some((p) => p.id === defaultProjectId)) {
      setDefaultProjectId(null);
    }
  }, [listPending, projects, defaultProjectId, setDefaultProjectId]);

  const filteredProjects = useMemo(() => {
    const q = nameFilter.trim().toLowerCase();
    if (!q) return projects;
    return projects.filter((p) => p.name.toLowerCase().includes(q));
  }, [projects, nameFilter]);

  const saving = createMut.isPending || updateMut.isPending;
  const err =
    createMut.error ?? updateMut.error ?? deleteMut.error ?? listError ?? null;

  async function onSave() {
    const body = {
      name: form.name.trim(),
      description: form.description.trim() || null,
      color: form.color.trim() || null,
    };
    if (!body.name) return;
    if (selectedId === "new") {
      const created = await createMut.mutateAsync(body);
      navigate(`/projects/${created.id}`, { replace: true });
      return;
    }
    if (typeof selectedId === "number") {
      await updateMut.mutateAsync({ projectId: selectedId, body });
    }
  }

  const runDelete = useCallback(async () => {
    if (!deleteConfirm) return;
    await deleteMut.mutateAsync(deleteConfirm.id);
    setDeleteConfirm(null);
    if (selectedId === deleteConfirm.id) {
      navigate("/projects");
    }
  }, [deleteConfirm, deleteMut, navigate, selectedId]);

  function openWorkspace() {
    if (typeof selectedId !== "number") return;
    setDefaultProjectId(selectedId);
    navigate("/graph");
  }

  const editorOpen = selectedId === "new" || typeof selectedId === "number";

  return (
    <div className="flex min-h-screen flex-col">
      <AppTopNav committedProcessId={null} />
      <div className="flex min-h-0 w-full flex-1 flex-col md:h-[calc(100vh-3.5rem)] md:flex-row">
        <aside
          className={cn(
            "border-sidebar-border bg-sidebar text-sidebar-foreground flex h-full min-h-0 w-full min-w-0 shrink-0 flex-col md:w-[22rem] md:border-r",
          )}
        >
          <header className="border-border bg-card flex h-14 shrink-0 items-center gap-2 border-b px-4 md:px-6">
            <Library className="text-foreground size-[18px] shrink-0" strokeWidth={2} aria-hidden />
            <h2 className="text-foreground truncate text-xs font-black uppercase tracking-[0.2em]">
              Project library
            </h2>
          </header>
          <div className="border-border shrink-0 border-b px-4 py-2 md:px-6">
            <Input
              type="search"
              value={nameFilter}
              onChange={(e) => setNameFilter(e.target.value)}
              placeholder="Filter by name…"
              className="h-8 rounded-lg text-xs"
              aria-label="Filter projects by name"
            />
          </div>
          <div className="task-board-scroll flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-4 pb-3 md:px-6">
            {selectedId === "new" && (
              <div className="rounded-2xl border-4 border-indigo-500 bg-card p-4 shadow-sm">
                <p className="text-[10px] font-black uppercase tracking-[0.14em] text-indigo-700 dark:text-indigo-400">
                  New project
                </p>
                <p className="text-muted-foreground mt-2 text-[11px] leading-relaxed">
                  Enter a name and save to add it to the library.
                </p>
              </div>
            )}
            {listPending && (
              <p className="text-muted-foreground py-6 text-center text-sm font-medium">Loading…</p>
            )}
            {!listPending && projects.length === 0 && selectedId !== "new" && (
              <p className="text-muted-foreground py-10 text-center text-sm leading-relaxed">
                No projects yet. Use <span className="font-semibold text-foreground/90">Create project</span>{" "}
                below.
              </p>
            )}
            {!listPending &&
              filteredProjects.map((p) => {
                const color = p.color ?? "#6366f1";
                const selected = selectedId === p.id;
                const rel = formatRelativeTimeFromIso(p.updated_at);
                return (
                  <button
                    key={p.id}
                    type="button"
                    onClick={() => {
                      setSelectedId(p.id);
                      navigate(`/projects/${p.id}`);
                    }}
                    className={cn(
                      "w-full rounded-2xl border p-4 text-left transition-all",
                      "focus-visible:ring-ring focus-visible:ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-offset-2",
                      selected
                        ? "border-4 bg-card shadow-sm"
                        : "border border-border/80 bg-card hover:border-border hover:bg-muted/30",
                    )}
                    style={selected ? { borderColor: color } : undefined}
                  >
                    <div className="flex items-start gap-2">
                      <span
                        className="mt-0.5 size-3 shrink-0 rounded-full"
                        style={{ backgroundColor: color }}
                        aria-hidden
                      />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="truncate font-semibold text-sm">{p.name}</span>
                          {defaultProjectId === p.id && (
                            <Star
                              className="size-3.5 shrink-0 fill-amber-400 text-amber-500"
                              aria-label="Default for new processes"
                            />
                          )}
                        </div>
                        {rel && (
                          <span className="text-muted-foreground text-[10px] tabular-nums">{rel}</span>
                        )}
                      </div>
                    </div>
                  </button>
                );
              })}
          </div>
          <div className="border-border bg-card shrink-0 border-t px-4 py-3 md:px-6">
            <Button
              type="button"
              onClick={() => {
                setSelectedId("new");
                navigate("/projects/new");
              }}
              className="h-auto w-full gap-2 rounded-xl bg-zinc-900 py-3.5 text-[10px] font-black uppercase tracking-[0.14em] text-white shadow-md hover:bg-zinc-800 dark:bg-zinc-950 dark:hover:bg-zinc-900"
            >
              <Plus className="size-4" strokeWidth={2.5} aria-hidden />
              Create project
            </Button>
          </div>
        </aside>

        <div className="bg-muted/25 flex min-h-0 min-h-[28rem] flex-1 flex-col md:min-h-0">
          <header className="border-border flex h-14 shrink-0 items-center justify-between gap-3 border-b bg-card px-4 md:px-6">
            <div className="flex min-w-0 items-center gap-2">
              <FolderOpen className="text-foreground size-[18px] shrink-0" strokeWidth={2} aria-hidden />
              <h2 className="text-foreground truncate text-xs font-black uppercase tracking-[0.2em]">
                Manage projects
              </h2>
            </div>
            {typeof selectedId === "number" && (
              <div className="flex shrink-0 items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="text-xs"
                  onClick={() => setDefaultProjectId(defaultProjectId === selectedId ? null : selectedId)}
                >
                  <Star
                    className={cn(
                      "mr-1 size-3.5",
                      defaultProjectId === selectedId && "fill-amber-400 text-amber-500",
                    )}
                  />
                  {defaultProjectId === selectedId ? "Default" : "Set default"}
                </Button>
                <Button type="button" variant="default" size="sm" className="text-xs" onClick={openWorkspace}>
                  Open workspace
                </Button>
              </div>
            )}
          </header>

          {editorOpen ? (
            <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4 md:p-6">
              {typeof selectedId === "number" && detailPending && (
                <p className="text-muted-foreground text-sm">Loading project…</p>
              )}
              <div className="grid max-w-xl gap-3">
                <div className="space-y-1">
                  <label htmlFor="proj-name" className="text-xs font-medium text-muted-foreground">
                    Name
                  </label>
                  <Input
                    id="proj-name"
                    value={form.name}
                    onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
                    className="max-w-md"
                  />
                </div>
                <div className="space-y-1">
                  <label htmlFor="proj-desc" className="text-xs font-medium text-muted-foreground">
                    Description
                  </label>
                  <Input
                    id="proj-desc"
                    value={form.description}
                    onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
                    className="max-w-md"
                  />
                </div>
                <div className="space-y-1">
                  <label htmlFor="proj-color" className="text-xs font-medium text-muted-foreground">
                    Color
                  </label>
                  <Input
                    id="proj-color"
                    type="color"
                    value={form.color}
                    onChange={(e) => setForm((f) => ({ ...f, color: e.target.value }))}
                    className="h-10 w-24 cursor-pointer p-1"
                  />
                </div>
              </div>
              {err instanceof Error && <p className="text-destructive text-sm">{err.message}</p>}
              <div className="flex flex-wrap items-center gap-2">
                <Button type="button" onClick={onSave} disabled={saving || !form.name.trim()}>
                  {saving ? "Saving…" : "Save"}
                </Button>
                {typeof selectedId === "number" && (
                  <Button
                    type="button"
                    variant="destructive"
                    onClick={() => {
                      const p = projects.find((x) => x.id === selectedId);
                      if (p) setDeleteConfirm(p);
                    }}
                  >
                    <Trash2 className="mr-1 size-4" />
                    Delete
                  </Button>
                )}
              </div>
            </div>
          ) : (
            <div className="text-muted-foreground flex flex-1 items-center justify-center p-8 text-sm">
              Select a project or create a new one.
            </div>
          )}
        </div>
      </div>

      {deleteConfirm && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
          role="dialog"
          aria-modal="true"
          aria-labelledby="del-proj-title"
        >
          <div className="bg-card max-w-md rounded-xl border p-6 shadow-lg">
            <h3 id="del-proj-title" className="text-foreground font-semibold">
              Delete project?
            </h3>
            <p className="text-muted-foreground mt-2 text-sm">
              Processes in <strong>{deleteConfirm.name}</strong> will become unassigned. This cannot be
              undone.
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <Button type="button" variant="outline" onClick={() => setDeleteConfirm(null)}>
                Cancel
              </Button>
              <Button type="button" variant="destructive" onClick={() => void runDelete()}>
                Delete
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
