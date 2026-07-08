import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { AppTopNav } from "../components/AppTopNav";
import { RecentProcessesList } from "../components/RecentProcessesList";
import { ProcessMainPane, type OptionalProcessViz } from "../components/ProcessMainPane";
import {
  useCreateProcessMutation,
  useProcessEventStreamEnabled,
  useProcessesListQuery,
} from "../hooks/useProcessQueries";
import { useDefaultProjectId } from "../hooks/useDefaultProjectId";
import { useTeamsListQuery } from "../hooks/useTeamQueries";
import { useProjectsListQuery } from "../hooks/useProjectQueries";
import { useProcessEventStream } from "../hooks/useProcessEventStream";
import { useDefaultTeamTemplateId } from "../hooks/useDefaultTeamTemplateId";
import {
  usePixelStripTileMode,
  type PixelStripTileMode,
} from "../hooks/usePixelStripTileMode";
import {
  DEFAULT_VIEW_MODE,
  parseProcessIdParam,
  processWorkspacePath,
  viewModeFromPathname,
  type ViewMode,
} from "../lib/processWorkspaceRoutes";
import { projectTagFromGoal, uniqueSortedProjectTags } from "../lib/projectGoalTag";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Tabs } from "@/components/ui/tabs";

const SimulationSpike = lazy(() => import("../features/simulation/SimulationSpike"));
const PixelHomeTeaser = lazy(() => import("../features/pixel/PixelHomeTeaser"));

const selectClassName =
  "h-8 min-w-[9rem] max-w-full rounded-lg border border-input bg-transparent px-2.5 py-1 text-sm outline-none transition-colors focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30";

function processListFilterFromSelect(v: string): ProcessListProjectFilter {
  if (v === "all") return "all";
  if (v === "unassigned") return "unassigned";
  if (v.startsWith("project:")) {
    const n = parseInt(v.slice(8), 10);
    return Number.isFinite(n) && n > 0 ? n : "all";
  }
  return "all";
}

function createProjectIdFromSelect(v: string): number | null {
  if (!v.startsWith("project:")) return null;
  const n = parseInt(v.slice(8), 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

export function ProcessesPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const { processId: processIdParam } = useParams();

  const committedProcessId = parseProcessIdParam(processIdParam);
  const pathView = viewModeFromPathname(location.pathname);
  const viewMode: ViewMode = pathView ?? DEFAULT_VIEW_MODE;

  const [processIdInput, setProcessIdInput] = useState("");
  const [newProcessGoal, setNewProcessGoal] = useState("");
  const [autoApproveNewProcess, setAutoApproveNewProcess] = useState(false);
  const [projectSelectValue, setProjectSelectValue] = useState("all");
  const [legacyTag, setLegacyTag] = useState("");
  const [teamTemplateId, setTeamTemplateId] = useState<number | null>(null);
  const [defaultTeamId, setDefaultTeamId] = useDefaultTeamTemplateId();
  const [defaultProjectId] = useDefaultProjectId();
  const appliedDefaultTeamRef = useRef(false);
  const appliedDefaultProjectRef = useRef(false);
  const [optionalViz, setOptionalViz] = useState<OptionalProcessViz>("none");
  const [stripTileMode, setStripTileMode] = usePixelStripTileMode();

  useEffect(() => {
    setProcessIdInput(committedProcessId != null ? String(committedProcessId) : "");
  }, [committedProcessId]);

  const streamEnabled = useProcessEventStreamEnabled(committedProcessId);
  useProcessEventStream(committedProcessId, streamEnabled);

  const createProcessMutation = useCreateProcessMutation();
  const { data: teamsData, isPending: teamsLoading } = useTeamsListQuery();
  const { data: projectsData, isPending: projectsLoading } = useProjectsListQuery();
  const teams = teamsData?.teams ?? [];
  const apiProjects = projectsData?.projects ?? [];

  useEffect(() => {
    if (teamsLoading || appliedDefaultTeamRef.current) return;
    if (teams.length === 0) return;
    appliedDefaultTeamRef.current = true;
    if (
      defaultTeamId != null &&
      teams.some((t) => t.id === defaultTeamId)
    ) {
      setTeamTemplateId((prev) => (prev == null ? defaultTeamId : prev));
    }
  }, [teamsLoading, teams, defaultTeamId]);

  useEffect(() => {
    if (teamsLoading) return;
    if (defaultTeamId == null) return;
    if (teams.length === 0) return;
    if (!teams.some((t) => t.id === defaultTeamId)) {
      setDefaultTeamId(null);
    }
  }, [teamsLoading, teams, defaultTeamId, setDefaultTeamId]);

  useEffect(() => {
    if (projectsLoading || appliedDefaultProjectRef.current) return;
    if (apiProjects.length === 0) return;
    appliedDefaultProjectRef.current = true;
    if (
      defaultProjectId != null &&
      apiProjects.some((p) => p.id === defaultProjectId)
    ) {
      setProjectSelectValue(`project:${defaultProjectId}`);
    }
  }, [projectsLoading, apiProjects, defaultProjectId]);

  const projectListFilter = useMemo(
    () => processListFilterFromSelect(projectSelectValue),
    [projectSelectValue],
  );

  const {
    data: processesListData,
    isPending: processesListLoading,
    error: processesListError,
  } = useProcessesListQuery(30, projectListFilter);
  const processes = processesListData?.processes ?? [];

  const { data: allProcessesForTags } = useProcessesListQuery(120, "all", {
    refetchInterval: 60_000,
  });
  const legacyTagOptions = useMemo(
    () => uniqueSortedProjectTags(allProcessesForTags?.processes ?? []),
    [allProcessesForTags],
  );

  const scopeForCreate = createProjectIdFromSelect(projectSelectValue);
  const showLegacyTagControls =
    projectSelectValue === "all" || projectSelectValue === "unassigned";

  const listErr = processesListError instanceof Error ? processesListError : null;

  function navigateToProcess(processId: number | null) {
    navigate(processWorkspacePath(viewMode, processId));
  }

  function setViewMode(next: ViewMode) {
    navigate(processWorkspacePath(next, committedProcessId), { replace: true });
  }

  function loadProcess() {
    const n = parseInt(processIdInput.trim(), 10);
    if (Number.isFinite(n) && n > 0) navigateToProcess(n);
  }

  function startNewProcess() {
    const g = newProcessGoal.trim();
    if (!g || teamTemplateId == null) return;
    let goal = g;
    let projectId: number | null = null;
    if (scopeForCreate != null) {
      projectId = scopeForCreate;
    } else {
      const tag = legacyTag.trim();
      if (tag && projectTagFromGoal(goal) !== tag) {
        goal = `[${tag}] ${goal}`;
      }
    }
    createProcessMutation.mutate(
      {
        goal,
        autoApprove: autoApproveNewProcess || undefined,
        teamTemplateId,
        projectId,
      },
      {
        onSuccess: (res) => {
          navigate(processWorkspacePath(viewMode, res.process_id));
          setNewProcessGoal("");
        },
      },
    );
  }

  return (
    <Tabs
      value={viewMode}
      onValueChange={(v) => setViewMode(v as ViewMode)}
      className="flex min-h-screen flex-col gap-0"
    >
      <AppTopNav committedProcessId={committedProcessId} />

      <>
        <Card className="rounded-none border-x-0 border-t-0 shadow-none">
          <CardHeader className="pb-2">
            <CardTitle>Process controls</CardTitle>
            <CardDescription>
              Pick a project (or All / Unassigned), choose a team template, then enter a goal. For rows
              without a server project, you can still use a legacy{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-xs">[tag]</code> prefix when scope is
              All or Unassigned.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4 pt-0">
            {committedProcessId == null && optionalViz === "pixel" ? (
              <Suspense fallback={null}>
                <PixelHomeTeaser />
              </Suspense>
            ) : null}

            <div className="space-y-2">
              <div className="text-sm font-semibold">New process</div>
              <div className="flex flex-wrap items-end gap-3">
                <div className="flex min-w-0 flex-col gap-1">
                  <label htmlFor="home-project" className="text-xs text-muted-foreground">
                    Project
                  </label>
                  <select
                    id="home-project"
                    aria-label="Project"
                    className={selectClassName}
                    value={projectSelectValue}
                    onChange={(e) => setProjectSelectValue(e.target.value)}
                    disabled={projectsLoading}
                  >
                    <option value="all">All processes</option>
                    <option value="unassigned">Unassigned</option>
                    {apiProjects.map((p) => (
                      <option key={p.id} value={`project:${p.id}`}>
                        {p.name}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="flex min-w-0 flex-col gap-1">
                  <label htmlFor="home-team" className="text-xs text-muted-foreground">
                    Team template
                  </label>
                  <select
                    id="home-team"
                    aria-label="Team template"
                    className={selectClassName}
                    value={teamTemplateId ?? ""}
                    onChange={(e) => {
                      const v = e.target.value;
                      setTeamTemplateId(v === "" ? null : parseInt(v, 10));
                    }}
                    disabled={teamsLoading || teams.length === 0}
                    required
                  >
                    <option value="">Select a team…</option>
                    {teams.map((t) => (
                      <option key={t.id} value={t.id}>
                        {t.name}
                      </option>
                    ))}
                  </select>
                </div>
              </div>
              {showLegacyTagControls ? (
                <div className="flex min-w-0 max-w-md flex-col gap-1">
                  <label htmlFor="home-legacy-tag" className="text-xs text-muted-foreground">
                    Legacy goal tag (optional)
                  </label>
                  <select
                    id="home-legacy-tag"
                    aria-label="Legacy goal tag"
                    className={selectClassName}
                    value={legacyTag}
                    onChange={(e) => setLegacyTag(e.target.value)}
                  >
                    <option value="">None</option>
                    {legacyTag && !legacyTagOptions.includes(legacyTag) ? (
                      <option value={legacyTag}>{legacyTag}</option>
                    ) : null}
                    {legacyTagOptions.map((t) => (
                      <option key={t} value={t}>
                        {t}
                      </option>
                    ))}
                  </select>
                </div>
              ) : null}
              {!teamsLoading && teams.length === 0 ? (
                <p className="text-destructive m-0 text-sm">No team templates exist — create one under Teams first.</p>
              ) : null}
              <div className="flex flex-wrap items-center gap-2">
                <Input
                  type="text"
                  placeholder="Goal"
                  value={newProcessGoal}
                  onChange={(e) => setNewProcessGoal(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && startNewProcess()}
                  className="min-w-[10rem] flex-1"
                />
                <Button
                  type="button"
                  onClick={startNewProcess}
                  disabled={
                    createProcessMutation.isPending ||
                    !newProcessGoal.trim() ||
                    teamTemplateId == null ||
                    teams.length === 0
                  }
                >
                  {createProcessMutation.isPending ? "Starting…" : "Start process"}
                </Button>
              </div>
              <label className="flex cursor-pointer items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={autoApproveNewProcess}
                  onChange={(e) => setAutoApproveNewProcess(e.target.checked)}
                  className="size-4 rounded border-input"
                />
                Auto-approve (skip human gate after planning)
              </label>
              {createProcessMutation.error && (
                <p className="text-sm text-destructive">
                  {createProcessMutation.error instanceof Error
                    ? createProcessMutation.error.message
                    : String(createProcessMutation.error)}
                </p>
              )}
            </div>

            <Separator />

            <div id="recent-processes" className="scroll-mt-20">
              <RecentProcessesList
                processes={processes}
                listLoading={processesListLoading}
                listError={listErr}
                projectTagFilter=""
                selectedProcessId={committedProcessId}
                onPickProcess={(id) => navigateToProcess(id)}
              />
            </div>

            <Separator />

            <div className="flex flex-wrap items-center gap-2">
              <Input
                type="text"
                placeholder="Process id"
                value={processIdInput}
                onChange={(e) => setProcessIdInput(e.target.value)}
                className="w-32"
              />
              <Button type="button" onClick={loadProcess}>
                Load
              </Button>
            </div>

            <div className="space-y-2">
              <div className="flex flex-col gap-1.5 sm:flex-row sm:flex-wrap sm:items-center sm:gap-3">
                <label htmlFor="optional-viz" className="text-sm font-medium">
                  Optional visualization
                </label>
                <select
                  id="optional-viz"
                  aria-label="Optional visualization"
                  className={selectClassName}
                  value={optionalViz}
                  onChange={(e) => setOptionalViz(e.target.value as OptionalProcessViz)}
                >
                  <option value="none">Off</option>
                  <option value="pixel">
                    Pixel preview{committedProcessId != null ? " (workspace strip)" : ""}
                  </option>
                  <option value="sim3d">3D boundary spike (lazy)</option>
                </select>
              </div>
              <div className="flex flex-col gap-1.5 sm:flex-row sm:flex-wrap sm:items-center sm:gap-3">
                <label htmlFor="strip-tile-mode" className="text-sm font-medium">
                  Strip task tiles
                </label>
                <select
                  id="strip-tile-mode"
                  aria-label="Strip task tiles"
                  className={selectClassName}
                  value={stripTileMode}
                  onChange={(e) => setStripTileMode(e.target.value as PixelStripTileMode)}
                >
                  <option value="css">CSS chibi (default)</option>
                  <option value="raster">Raster sprites (pixel-agents)</option>
                </select>
              </div>
              <p className="text-muted-foreground m-0 text-xs leading-snug">
                Only one at a time: pixel art (home teaser or expandable office below) or the 3D
                boundary spike — not both.
              </p>
              {optionalViz === "sim3d" && (
                <Suspense fallback={<p className="text-sm text-muted-foreground">Loading chunk…</p>}>
                  <SimulationSpike processId={committedProcessId} />
                </Suspense>
              )}
            </div>
          </CardContent>
        </Card>

        <div
          id="process-workspace"
          className="flex min-h-0 flex-1 scroll-mt-20 flex-col space-y-4 px-6 pt-2"
        >
          <ProcessMainPane
            processId={committedProcessId}
            viewMode={viewMode}
            optionalViz={optionalViz}
            stripTileMode={stripTileMode}
          />
        </div>
      </>
    </Tabs>
  );
}
