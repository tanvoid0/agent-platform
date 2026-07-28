import { Box, Cpu, Play, Plus, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";

import { AppTopNav } from "../components/AppTopNav";
import {
  useCreateModelProjectMutation,
  useModelBuildJobQuery,
  useModelProjectQuery,
  useModelProjectsQuery,
  useModelRegistryQuery,
  useOllamaModelsQuery,
  usePullOllamaModelMutation,
  useStartModelBuildJobMutation,
} from "../hooks/useModelOpsQueries";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

const STAGES = ["prepare", "train", "export", "eval"] as const;

function statusVariant(status: string): "default" | "secondary" | "destructive" | "outline" {
  if (status === "succeeded") return "default";
  if (status === "failed" || status === "cancelled") return "destructive";
  if (status === "running") return "secondary";
  return "outline";
}

export function ModelOpsPage() {
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [activeJobId, setActiveJobId] = useState<number | null>(null);
  const [newName, setNewName] = useState("");
  const [pullName, setPullName] = useState("");
  const [stages, setStages] = useState<string[]>(["prepare", "export", "eval"]);

  const { data: listData, isPending: listPending } = useModelProjectsQuery();
  const projects = listData?.projects ?? [];
  const { data: detail } = useModelProjectQuery(selectedName);
  const { data: job, refetch: refetchJob } = useModelBuildJobQuery(activeJobId);
  const { data: ollamaData, refetch: refetchOllama } = useOllamaModelsQuery();
  const { data: registryData } = useModelRegistryQuery();

  const createMut = useCreateModelProjectMutation();
  const startMut = useStartModelBuildJobMutation();
  const pullMut = usePullOllamaModelMutation();

  const ollamaModels = ollamaModelsSorted(ollamaData?.models ?? []);
  const registry = registryData?.entries ?? [];

  const err = createMut.error ?? startMut.error ?? pullMut.error;

  async function onCreateProject() {
    const name = newName.trim();
    if (!name) return;
    const created = await createMut.mutateAsync({ name });
    setNewName("");
    setSelectedName(created.name);
  }

  async function onStartJob() {
    if (!selectedName) return;
    const out = await startMut.mutateAsync({
      project: selectedName,
      stages,
      offline_eval: !stages.includes("eval") || stages.length <= 1,
    });
    setActiveJobId(out.id);
  }

  async function onPullModel() {
    const name = pullName.trim();
    if (!name) return;
    const out = await pullMut.mutateAsync(name);
    setActiveJobId(out.id);
    setPullName("");
  }

  function toggleStage(stage: string) {
    setStages((prev) =>
      prev.includes(stage) ? prev.filter((s) => s !== stage) : [...prev, stage],
    );
  }

  const manifestTag = useMemo(() => {
    const tag = detail?.manifest?.ollama_tag;
    return typeof tag === "string" ? tag : selectedName;
  }, [detail, selectedName]);

  return (
    <div className="flex min-h-dvh flex-col bg-background">
      <AppTopNav committedProcessId={null} />
      <main className="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-4 p-4 sm:p-6">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h1 className="text-lg font-semibold tracking-tight">Model ops</h1>
            <p className="text-sm text-muted-foreground">
              Training projects, build jobs, Ollama models, and registry.
            </p>
          </div>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" onClick={() => void refetchOllama()}>
              <RefreshCw className="size-3.5" />
              Refresh Ollama
            </Button>
            <Button variant="outline" size="sm" nativeButton={false} render={<a href="/docs#/model-ops" />}>
              API docs
            </Button>
          </div>
        </div>

        {err ? (
          <div className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {err instanceof Error ? err.message : "Request failed"}
          </div>
        ) : null}

        <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]">
          <section className="space-y-4 rounded-xl border border-border bg-card p-4">
            <div className="flex items-center gap-2 text-sm font-medium">
              <Box className="size-4" />
              Training projects
            </div>
            <div className="flex gap-2">
              <Input
                placeholder="new-project-name"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                className="h-8 text-sm"
              />
              <Button size="sm" onClick={() => void onCreateProject()} disabled={createMut.isPending}>
                <Plus className="size-3.5" />
                Create
              </Button>
            </div>
            <ul className="max-h-48 space-y-1 overflow-auto text-sm">
              {listPending ? (
                <li className="text-muted-foreground">Loading…</li>
              ) : projects.length === 0 ? (
                <li className="text-muted-foreground">No projects yet.</li>
              ) : (
                projects.map((p) => (
                  <li key={p.id}>
                    <button
                      type="button"
                      className={cn(
                        "w-full rounded-md px-2 py-1.5 text-left hover:bg-muted",
                        selectedName === p.name && "bg-muted font-medium",
                      )}
                      onClick={() => setSelectedName(p.name)}
                    >
                      {p.name}
                    </button>
                  </li>
                ))
              )}
            </ul>

            {selectedName ? (
              <div className="space-y-3 border-t border-border pt-3">
                <div className="text-sm">
                  <span className="font-medium">{selectedName}</span>
                  {manifestTag ? (
                    <span className="ml-2 text-muted-foreground">→ {manifestTag}</span>
                  ) : null}
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {STAGES.map((stage) => (
                    <Button
                      key={stage}
                      type="button"
                      size="sm"
                      variant={stages.includes(stage) ? "default" : "outline"}
                      className="h-7 text-xs capitalize"
                      onClick={() => toggleStage(stage)}
                    >
                      {stage}
                    </Button>
                  ))}
                </div>
                <Button
                  size="sm"
                  onClick={() => void onStartJob()}
                  disabled={startMut.isPending || stages.length === 0}
                >
                  <Play className="size-3.5" />
                  Start build job
                </Button>
              </div>
            ) : null}
          </section>

          <section className="space-y-4 rounded-xl border border-border bg-card p-4">
            <div className="flex items-center gap-2 text-sm font-medium">
              <Cpu className="size-4" />
              Jobs &amp; Ollama
            </div>

            <div className="flex gap-2">
              <Input
                placeholder="llama3.2:latest"
                value={pullName}
                onChange={(e) => setPullName(e.target.value)}
                className="h-8 text-sm"
              />
              <Button size="sm" variant="secondary" onClick={() => void onPullModel()} disabled={pullMut.isPending}>
                Pull (async)
              </Button>
            </div>

            {job ? (
              <div className="space-y-2 rounded-lg border border-border bg-muted/30 p-3 text-sm">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">Job #{job.id}</span>
                  <Badge variant={statusVariant(job.status)}>{job.status}</Badge>
                  <span className="text-muted-foreground">{job.job_type}</span>
                  {job.current_stage ? (
                    <span className="text-muted-foreground">· {job.current_stage}</span>
                  ) : null}
                  <Button variant="ghost" size="sm" className="ml-auto h-7" onClick={() => void refetchJob()}>
                    Refresh
                  </Button>
                </div>
                {job.error_message ? (
                  <p className="text-destructive">{job.error_message}</p>
                ) : null}
                {job.log_tail ? (
                  <pre className="max-h-40 overflow-auto rounded bg-background p-2 text-xs whitespace-pre-wrap">
                    {job.log_tail}
                  </pre>
                ) : null}
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">Start a build or pull job to track progress here.</p>
            )}

            <div>
              <div className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                Ollama models ({ollamaModels.length})
              </div>
              <ul className="max-h-36 space-y-1 overflow-auto text-sm">
                {ollamaModels.length === 0 ? (
                  <li className="text-muted-foreground">No models listed.</li>
                ) : (
                  ollamaModels.map((m) => (
                    <li key={m.name} className="truncate font-mono text-xs">
                      {m.name}
                    </li>
                  ))
                )}
              </ul>
            </div>

            {registry.length > 0 ? (
              <div>
                <div className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                  Registry
                </div>
                <ul className="space-y-1 text-sm">
                  {registry.slice(0, 8).map((e) => (
                    <li key={e.id} className="flex items-center gap-2">
                      <span className="font-mono text-xs">{e.ollama_tag}</span>
                      {e.is_active ? <Badge variant="default">active</Badge> : null}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
          </section>
        </div>
      </main>
    </div>
  );
}

function ollamaModelsSorted(models: { name: string }[]) {
  return [...models].sort((a, b) => a.name.localeCompare(b.name));
}
