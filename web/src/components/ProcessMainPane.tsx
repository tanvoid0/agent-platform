import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { ApiError } from "../api/client";
import { downloadProcessExport } from "../lib/downloadProcessDetailJson";
import { parsePlannerDag } from "../api/dag";
import {
  clampInspectorWidthPx,
  persistInspectorWidthPx,
  readStoredInspectorWidth,
} from "../lib/processWorkspaceRail";
import {
  useApproveDagMutation,
  useCancelProcessMutation,
  useRetryProcessMutation,
  useRetryTaskMutation,
  useProcessDetailQuery,
  useProcessEventsQuery,
  useSyncProcessMutation,
} from "../hooks/useProcessQueries";
import { buildWorkHintByClientUuid } from "@/features/pixel/pixelViewModel";
import type { PixelStripTileMode } from "@/hooks/usePixelStripTileMode";
import { DagGraphView } from "./DagGraphView";
import { ProcessSummaryPanel } from "./ProcessSummaryPanel";
import { SubagentInspector } from "./SubagentInspector";
import { ProcessEventsView } from "./ProcessEventsView";
import { TaskBoardView } from "./TaskBoardView";
import { TaskReviewModal } from "./TaskReviewModal";
import { TaskTimelineView } from "./TaskTimelineView";
import { Button } from "@/components/ui/button";
import { TabsContent } from "@/components/ui/tabs";
import type { ViewMode } from "../lib/processWorkspaceRoutes";

const PixelProcessStrip = lazy(() => import("../features/pixel"));

/** Mutually exclusive optional visuals from Process controls (pixel vs 3D boundary spike). */
export type OptionalProcessViz = "none" | "pixel" | "sim3d";

type Props = {
  processId: number | null;
  viewMode: ViewMode;
  optionalViz?: OptionalProcessViz;
  stripTileMode?: PixelStripTileMode;
};

export function ProcessMainPane({
  processId,
  viewMode: _viewMode,
  optionalViz = "none",
  stripTileMode = "css",
}: Props) {
  const [selectedUuid, setSelectedUuid] = useState<string | null>(null);
  const [reviewModalOpen, setReviewModalOpen] = useState(false);
  const skipCloseReviewModalRef = useRef(false);
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const [syncFeedback, setSyncFeedback] = useState<string | null>(null);
  const [inspectorWidthPx, setInspectorWidthPx] = useState(readStoredInspectorWidth);
  const inspectorWidthRef = useRef(inspectorWidthPx);
  const inspectorResizeRef = useRef({ active: false, startX: 0, startWidth: inspectorWidthPx });
  inspectorWidthRef.current = inspectorWidthPx;

  const detail = useProcessDetailQuery(processId);
  const retryMutation = useRetryProcessMutation();
  const retryTaskMutation = useRetryTaskMutation();
  const approveMutation = useApproveDagMutation();
  const cancelMutation = useCancelProcessMutation();
  const syncMutation = useSyncProcessMutation();

  const dagJson = detail.data?.process.dag_json ?? null;
  const tasks = detail.data?.tasks ?? [];
  const failedTask = tasks.find((t) => t.status === "failed");

  const dag = useMemo(() => parsePlannerDag(dagJson), [dagJson]);

  const eventsQuery = useProcessEventsQuery(processId, "all", detail.data?.process.status);
  const pixelEventHints = useMemo(() => {
    const ev = eventsQuery.data?.events;
    if (!ev?.length) return undefined;
    return {
      recentEventTypes: ev
        .slice(-24)
        .reverse()
        .map((e) => e.event_type),
      workHintByClientUuid: buildWorkHintByClientUuid(ev, tasks),
    };
  }, [eventsQuery.data?.events, tasks]);

  useEffect(() => {
    setSelectedUuid(null);
    setSyncFeedback(null);
    setReviewModalOpen(false);
  }, [processId]);

  useEffect(() => {
    if (skipCloseReviewModalRef.current) {
      skipCloseReviewModalRef.current = false;
      return;
    }
    setReviewModalOpen(false);
  }, [selectedUuid]);

  const onRetry = useCallback(() => {
    if (processId != null) retryMutation.mutate(processId);
  }, [processId, retryMutation]);

  const onApproveDag = useCallback(
    (dagJson: string) => {
      if (processId != null) approveMutation.mutate({ processId, dagJson });
    },
    [processId, approveMutation],
  );

  const onCancelProcess = useCallback(() => {
    if (processId != null) cancelMutation.mutate(processId);
  }, [processId, cancelMutation]);

  const onSyncProcess = useCallback(() => {
    if (processId == null) return;
    syncMutation.mutate(processId, {
      onSuccess: (data) => setSyncFeedback(data.detail),
    });
  }, [processId, syncMutation]);

  const openTaskReview = useCallback((uuid: string) => {
    skipCloseReviewModalRef.current = true;
    setSelectedUuid(uuid);
    setReviewModalOpen(true);
  }, []);

  const selectedTask = useMemo(
    () => (selectedUuid ? tasks.find((t) => t.client_uuid === selectedUuid) : undefined),
    [tasks, selectedUuid],
  );

  const selectedRoleLabel = useMemo(() => {
    if (!selectedUuid || !dag) return null;
    return dag.subagents.find((s) => s.client_uuid === selectedUuid)?.role ?? null;
  }, [dag, selectedUuid]);

  const stopInspectorResize = useCallback(() => {
    if (!inspectorResizeRef.current.active) return;
    inspectorResizeRef.current.active = false;
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
    persistInspectorWidthPx(
      clampInspectorWidthPx(inspectorWidthRef.current, window.innerWidth),
    );
  }, []);

  const startInspectorResize = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    inspectorResizeRef.current = {
      active: true,
      startX: e.clientX,
      startWidth: inspectorWidthRef.current,
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!inspectorResizeRef.current.active) return;
      const { startX, startWidth } = inspectorResizeRef.current;
      const delta = startX - e.clientX;
      const next = clampInspectorWidthPx(startWidth + delta, window.innerWidth);
      setInspectorWidthPx(next);
    };
    const onUp = () => stopInspectorResize();
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [stopInspectorResize]);

  return (
    <>
      {detail.error && (
        <p className="text-destructive">
          {detail.error instanceof ApiError
            ? `${detail.error.message} (${detail.error.status})`
            : String(detail.error)}
        </p>
      )}

      {detail.data && (
        <Suspense fallback={null}>
          <PixelProcessStrip
            processStatus={detail.data.process.status}
            dag={dag}
            tasks={tasks}
            eventHints={pixelEventHints}
            teamSnapshotJson={detail.data.process.team_snapshot_json}
            pixelOfficeLocked={optionalViz === "sim3d"}
            stripTileMode={stripTileMode}
          />
        </Suspense>
      )}

      {detail.data && (
        <div className="mt-1 flex flex-col gap-1 px-0">
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-8 text-xs"
              disabled={exporting || processId == null}
              onClick={() => {
                if (processId == null) return;
                setExportError(null);
                setExporting(true);
                void downloadProcessExport(processId)
                  .catch((e: unknown) => {
                    setExportError(e instanceof Error ? e.message : String(e));
                  })
                  .finally(() => setExporting(false));
              }}
            >
              {exporting ? "Exporting…" : "Export JSON"}
            </Button>
            <span className="text-muted-foreground text-[10px]">
              Process {detail.data.process.id} + tasks + events (paginated full export).
            </span>
          </div>
          {exportError && (
            <p className="text-destructive text-[10px]" role="alert">
              {exportError}
            </p>
          )}
        </div>
      )}

      {detail.data && (
        <ProcessSummaryPanel
          process={detail.data.process}
          onRetry={detail.data.process.status === "failed" ? onRetry : undefined}
          retryPending={retryMutation.isPending}
          taskRetryPending={retryTaskMutation.isPending}
          retryError={retryMutation.error instanceof Error ? retryMutation.error : null}
          onApproveDag={
            detail.data.process.status === "approval_required" ? onApproveDag : undefined
          }
          approvePending={approveMutation.isPending}
          approveError={approveMutation.error instanceof Error ? approveMutation.error : null}
          onCancel={
            !["completed", "failed", "cancelled"].includes(detail.data.process.status)
              ? onCancelProcess
              : undefined
          }
          cancelPending={cancelMutation.isPending}
          cancelError={cancelMutation.error instanceof Error ? cancelMutation.error : null}
          onSync={
            !["completed", "failed", "cancelled"].includes(detail.data.process.status)
              ? onSyncProcess
              : undefined
          }
          syncPending={syncMutation.isPending}
          syncError={syncMutation.error instanceof Error ? syncMutation.error : null}
          syncFeedback={syncFeedback}
          failedTaskClientUuid={failedTask?.client_uuid ?? null}
          failedTaskFailureDebugJson={failedTask?.failure_debug_json ?? null}
        />
      )}

      <div className="flex min-h-0 flex-1 border-t border-border">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <TabsContent
            value="graph"
            className="mt-0 flex min-h-0 flex-1 flex-col p-0 outline-none data-[hidden]:hidden"
          >
            <DagGraphView
              dagJson={dagJson}
              tasks={tasks}
              selectedUuid={selectedUuid}
              onSelectUuid={setSelectedUuid}
            />
          </TabsContent>
          <TabsContent
            value="board"
            className="mt-0 flex min-h-0 flex-1 flex-col p-0 outline-none data-[hidden]:hidden"
          >
            <TaskBoardView
              dag={dag}
              tasks={tasks}
              selectedUuid={selectedUuid}
              onSelectUuid={setSelectedUuid}
              onOpenTaskReview={openTaskReview}
              processStatus={detail.data?.process.status ?? null}
              onRetryFailedTask={
                processId != null && detail.data?.process.status === "failed"
                  ? (taskId) => retryTaskMutation.mutate({ processId, taskId })
                  : undefined
              }
              retryTaskPending={retryTaskMutation.isPending}
              retryProcessPending={retryMutation.isPending}
            />
          </TabsContent>
          <TabsContent
            value="timeline"
            className="mt-0 flex min-h-0 flex-1 flex-col p-0 outline-none data-[hidden]:hidden"
          >
            <TaskTimelineView
              dag={dag}
              tasks={tasks}
              selectedUuid={selectedUuid}
              onSelectUuid={setSelectedUuid}
            />
          </TabsContent>
          <TabsContent
            value="events"
            className="mt-0 flex min-h-0 flex-1 flex-col p-0 outline-none data-[hidden]:hidden"
          >
            <ProcessEventsView
              processId={processId}
              processStatus={detail.data?.process.status}
              tasks={tasks}
              selectedUuid={selectedUuid}
              onSelectUuid={setSelectedUuid}
            />
          </TabsContent>
        </div>
        {processId != null && (
          <div className="flex h-[calc(100vh-140px)] max-h-[calc(100vh-140px)] shrink-0">
            <div
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize inspector sidebar"
              title="Drag to resize sidebar"
              onMouseDown={startInspectorResize}
              className="border-border bg-muted/30 hover:bg-muted/60 relative z-40 w-2 shrink-0 cursor-col-resize border-l"
            />
            <div
              className="flex min-h-0 min-w-0 flex-col overflow-hidden"
              style={{ width: inspectorWidthPx }}
            >
              <SubagentInspector
                processId={processId}
                process={detail.data?.process ?? null}
                processLoading={detail.isPending}
                processError={detail.error instanceof Error ? detail.error : null}
                processStatus={detail.data?.process.status ?? null}
                dagJson={dagJson}
                tasks={tasks}
                selectedUuid={selectedUuid}
                onSelectUuid={setSelectedUuid}
                onClose={() => setSelectedUuid(null)}
                onRequestReview={() => setReviewModalOpen(true)}
                onRetryFailedTask={
                  processId != null && detail.data?.process.status === "failed"
                    ? (taskId) => retryTaskMutation.mutate({ processId, taskId })
                    : undefined
                }
                retryTaskPending={retryTaskMutation.isPending}
                retryProcessPending={retryMutation.isPending}
                retryTaskError={
                  retryTaskMutation.error instanceof Error ? retryTaskMutation.error : null
                }
              />
            </div>
          </div>
        )}
      </div>

      <TaskReviewModal
        open={reviewModalOpen && selectedTask?.status === "awaiting_review"}
        onOpenChange={setReviewModalOpen}
        processId={processId}
        task={selectedTask}
        roleLabel={selectedRoleLabel}
      />
    </>
  );
}
