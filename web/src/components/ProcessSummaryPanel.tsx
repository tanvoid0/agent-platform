import { memo, useEffect, useMemo, useState } from "react";

import { validatePlannerDag } from "../api/dag";
import type { ProcessRecord } from "../api/types";
import { PlannerDagPreview } from "./PlannerDagPreview";
import { formatFailureDebugJson } from "../lib/formatFailureDebugJson";
import { processStatusBadgeVariant } from "../lib/processStatusBadge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const textareaClass = cn(
  "mt-1 w-full min-w-0 rounded-lg border border-input bg-transparent px-2.5 py-2 font-mono text-xs transition-colors outline-none",
  "placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
  "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
  "dark:bg-input/30",
);

type Props = {
  process: ProcessRecord | undefined;
  onRetry?: () => void;
  retryPending?: boolean;
  /** When a per-task retry is in flight, disable full process retry to avoid overlapping mutations. */
  taskRetryPending?: boolean;
  retryError?: Error | null;
  onApproveDag?: (dagJson: string) => void;
  approvePending?: boolean;
  approveError?: Error | null;
  onCancel?: () => void;
  cancelPending?: boolean;
  cancelError?: Error | null;
  /** Recover stuck planning/execution (server may reset in-flight tasks). */
  onSync?: () => void;
  syncPending?: boolean;
  syncError?: Error | null;
  /** Last successful sync message from the API (`detail`). */
  syncFeedback?: string | null;
  /** First failed task in the run (for structured failure debug in the summary column). */
  failedTaskClientUuid?: string | null;
  failedTaskFailureDebugJson?: string | null;
};

function panelPropsSignature(p: Props): string {
  const {
    process,
    onRetry,
    retryPending,
    taskRetryPending,
    retryError,
    onApproveDag,
    approvePending,
    approveError,
    onCancel,
    cancelPending,
    cancelError,
    onSync,
    syncPending,
    syncError,
    syncFeedback,
    failedTaskClientUuid,
    failedTaskFailureDebugJson,
  } = p;
  if (!process) {
    return JSON.stringify({
      process: null,
      hasRetry: !!onRetry,
      retryPending: !!retryPending,
      taskRetryPending: !!taskRetryPending,
      retryError: retryError?.message ?? null,
      hasApprove: !!onApproveDag,
      approvePending: !!approvePending,
      approveError: approveError?.message ?? null,
      hasCancel: !!onCancel,
      cancelPending: !!cancelPending,
      cancelError: cancelError?.message ?? null,
      hasSync: !!onSync,
      syncPending: !!syncPending,
      syncError: syncError?.message ?? null,
      syncFeedback: syncFeedback ?? null,
      failedTaskClientUuid: failedTaskClientUuid ?? null,
      failedTaskFailureDebugJson: failedTaskFailureDebugJson ?? null,
    });
  }
  return JSON.stringify({
    id: process.id,
    status: process.status,
    failure_reason: process.failure_reason,
    total_tokens: process.total_tokens,
    total_cost: process.total_cost,
    hasRetry: !!onRetry,
    retryPending: !!retryPending,
    taskRetryPending: !!taskRetryPending,
    retryError: retryError?.message ?? null,
    hasApprove: !!onApproveDag,
    approvePending: !!approvePending,
    approveError: approveError?.message ?? null,
    hasCancel: !!onCancel,
    cancelPending: !!cancelPending,
    cancelError: cancelError?.message ?? null,
    hasSync: !!onSync,
    syncPending: !!syncPending,
    syncError: syncError?.message ?? null,
    syncFeedback: syncFeedback ?? null,
    failedTaskClientUuid: failedTaskClientUuid ?? null,
    failedTaskFailureDebugJson: failedTaskFailureDebugJson ?? null,
  });
}

function processSummaryPropsEqual(a: Props, b: Props): boolean {
  return panelPropsSignature(a) === panelPropsSignature(b);
}

function ProcessSummaryPanelInner({
  process,
  onRetry,
  retryPending,
  taskRetryPending,
  retryError,
  onApproveDag,
  approvePending,
  approveError,
  onCancel,
  cancelPending,
  cancelError,
  onSync,
  syncPending,
  syncError,
  syncFeedback,
  failedTaskClientUuid,
  failedTaskFailureDebugJson,
}: Props) {
  const [dagEdit, setDagEdit] = useState("");
  const [dagClientError, setDagClientError] = useState<string | null>(null);

  const dagParse = useMemo(() => {
    try {
      const raw = JSON.parse(dagEdit) as unknown;
      return validatePlannerDag(raw);
    } catch (e) {
      return {
        ok: false as const,
        errors: [e instanceof Error ? e.message : "Invalid JSON"],
      };
    }
  }, [dagEdit]);

  useEffect(() => {
    if (process?.status !== "approval_required" || process.dag_json == null) return;
    try {
      const parsed = JSON.parse(process.dag_json);
      setDagEdit(JSON.stringify(parsed, null, 2));
    } catch {
      setDagEdit(process.dag_json);
    }
    setDagClientError(null);
  }, [process?.id, process?.status, process?.dag_json]);

  if (!process) return null;

  const failedTaskDebugFormatted = formatFailureDebugJson(failedTaskFailureDebugJson ?? null);
  const showCost = process.total_cost > 0;
  const failed = process.status === "failed";
  const needsTaskReview = process.status === "task_review_required";
  const needsApproval = process.status === "approval_required" && !!onApproveDag;
  const canRetry = failed && onRetry;
  const canCancel = !!onCancel;

  return (
    <div className="mt-2 space-y-2">
      {needsApproval && (
        <div
          role="status"
          className={cn(
            "rounded-lg border border-sky-500/40 bg-sky-500/10 px-3 py-2 text-sm text-sky-950",
            "dark:border-sky-500/35 dark:bg-sky-500/15 dark:text-sky-100",
          )}
        >
          <strong>DAG approval required:</strong> edit the planner JSON if needed, then approve to
          materialize tasks and start execution. The server validates the DAG (schema, acyclicity,
          references).
        </div>
      )}
      {needsTaskReview && (
        <div
          role="status"
          className={cn(
            "rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm text-amber-950",
            "dark:border-amber-500/30 dark:bg-amber-500/15 dark:text-amber-100",
          )}
        >
          <strong>Task review required:</strong> select a subagent that is awaiting review, then use{" "}
          <strong>Review task</strong> in the sidebar or the board icon to open the review modal (approve,
          reject, or request changes).
        </div>
      )}
      {failed && process.failure_reason && (
        <div
          role="alert"
          className="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm whitespace-pre-wrap text-destructive"
        >
          <strong>Failed:</strong> {process.failure_reason}
        </div>
      )}
      {failed && failedTaskDebugFormatted && (
        <details className="rounded-lg border border-border bg-muted/30 px-3 py-2 text-sm">
          <summary className="cursor-pointer font-medium text-foreground">
            Task failure details
            {failedTaskClientUuid ? (
              <span className="ml-1 font-mono text-xs text-muted-foreground">
                ({failedTaskClientUuid})
              </span>
            ) : null}
          </summary>
          <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words font-mono text-xs text-muted-foreground">
            {failedTaskDebugFormatted}
          </pre>
        </details>
      )}

      <p className="my-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <span className="inline-flex items-center gap-2">
          status:{" "}
          <Badge variant={processStatusBadgeVariant(process.status)} className="font-mono text-xs">
            {process.status}
          </Badge>
        </span>
        <span className="text-muted-foreground">·</span>
        <span>tokens: {process.total_tokens}</span>
        {showCost && (
          <>
            <span className="text-muted-foreground">·</span>
            <span>cost: {process.total_cost.toFixed(4)}</span>
          </>
        )}
      </p>

      {needsApproval && (
        <div className="space-y-2">
          {dagParse.ok ? (
            <PlannerDagPreview dag={dagParse.dag} />
          ) : (
            <div
              role="alert"
              className="rounded-lg border border-destructive/35 bg-destructive/10 px-3 py-2 text-sm text-destructive"
            >
              <p className="font-medium">Cannot preview planner DAG yet</p>
              <ul className="mt-1 list-inside list-disc text-xs leading-relaxed">
                {dagParse.errors.map((err, i) => (
                  <li key={`${i}-${err}`}>{err}</li>
                ))}
              </ul>
            </div>
          )}
          <label className="text-xs text-muted-foreground">
            Raw planner JSON (edit to adjust before approval)
            <textarea
              value={dagEdit}
              onChange={(e) => {
                setDagEdit(e.target.value);
                setDagClientError(null);
              }}
              rows={12}
              spellCheck={false}
              className={textareaClass}
              aria-invalid={!!dagClientError}
            />
          </label>
          {dagClientError && (
            <p className="text-sm text-destructive" role="alert">
              {dagClientError}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="secondary"
              size="sm"
              disabled={approvePending}
              onClick={() => {
                try {
                  const parsed = JSON.parse(dagEdit);
                  setDagEdit(JSON.stringify(parsed, null, 2));
                  setDagClientError(null);
                } catch (e) {
                  setDagClientError(e instanceof Error ? e.message : "Invalid JSON");
                }
              }}
            >
              Format JSON
            </Button>
            <Button
              type="button"
              disabled={approvePending}
              onClick={() => {
                let normalized: string;
                try {
                  normalized = JSON.stringify(JSON.parse(dagEdit));
                } catch (e) {
                  setDagClientError(e instanceof Error ? e.message : "Invalid JSON");
                  return;
                }
                const validated = validatePlannerDag(JSON.parse(normalized));
                if (!validated.ok) {
                  setDagClientError(validated.errors.join(" "));
                  return;
                }
                setDagClientError(null);
                onApproveDag!(normalized);
              }}
            >
              {approvePending ? "Approving…" : "Approve DAG"}
            </Button>
          </div>
          {approveError && (
            <p className="text-sm text-destructive">{approveError.message}</p>
          )}
        </div>
      )}

      {canCancel && (
        <div className="space-y-1.5">
          <div className="flex flex-wrap items-center gap-2">
            {onSync && (
              <Button
                type="button"
                variant="secondary"
                disabled={syncPending || cancelPending}
                onClick={() => {
                  if (
                    !confirm(
                      "Sync / recover this process? Use if the run looks stuck (e.g. after a restart). The server will re-queue planning or execution. For a running process, any tasks still marked “running” are reset to pending and re-run—do not use while work is healthy.",
                    )
                  ) {
                    return;
                  }
                  onSync();
                }}
              >
                {syncPending ? "Syncing…" : "Sync / recover"}
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              disabled={cancelPending}
              onClick={() => {
                if (
                  !confirm(
                    "Cancel this process? Execution stops and the process becomes cancelled.",
                  )
                ) {
                  return;
                }
                onCancel();
              }}
            >
              {cancelPending ? "Cancelling…" : "Cancel process"}
            </Button>
          </div>
          {syncFeedback && (
            <p className="text-muted-foreground max-w-prose text-xs">{syncFeedback}</p>
          )}
          {syncError && <p className="text-sm text-destructive">{syncError.message}</p>}
          {cancelError && <p className="text-sm text-destructive">{cancelError.message}</p>}
        </div>
      )}

      {canRetry && (
        <div className="mt-1.5 space-y-1">
          <Button
            type="button"
            onClick={onRetry}
            disabled={retryPending || taskRetryPending}
          >
            {retryPending ? "Retrying…" : "Retry process"}
          </Button>
          {retryError && (
            <p className="mt-1.5 text-sm text-destructive">{retryError.message}</p>
          )}
        </div>
      )}
    </div>
  );
}

export const ProcessSummaryPanel = memo(ProcessSummaryPanelInner, processSummaryPropsEqual);
