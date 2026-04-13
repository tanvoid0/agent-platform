import { Copy } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { parsePlannerDag } from "../api/dag";
import type { ProcessRecord, TaskNodeRecord } from "../api/types";
import { ProcessChatPanel } from "./chat/ProcessChatPanel";
import { formatFailureDebugJson } from "../lib/formatFailureDebugJson";
import { taskMapByUuid } from "../lib/dagTasks";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

type Props = {
  processId: number | null;
  /** Loaded process row; required for sidebar content except loading/error. */
  process: ProcessRecord | null;
  processLoading?: boolean;
  processError?: Error | null;
  processStatus: string | null;
  dagJson: string | null;
  tasks: TaskNodeRecord[];
  selectedUuid: string | null;
  onSelectUuid: (uuid: string) => void;
  onClose: () => void;
  onRetryFailedTask?: (taskId: number) => void;
  retryTaskPending?: boolean;
  retryProcessPending?: boolean;
  retryTaskError?: Error | null;
  /** Opens the focused review modal (approve / reject / request changes). */
  onRequestReview?: () => void;
};

function ProcessRailPlaceholder({ process }: { process: ProcessRecord }) {
  return (
    <div className="space-y-3 p-3 text-sm">
      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">Goal</span>
        <p className="leading-snug">{process.goal}</p>
      </div>
      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">Status</span>
        <p className="font-medium">{process.status}</p>
      </div>
      <p className="text-muted-foreground text-xs leading-relaxed">
        Select a task on the graph or board to inspect subagent details. Use the Chat tab for process-wide questions, or
        select a task for subagent-scoped chat.
      </p>
    </div>
  );
}

export function SubagentInspector({
  processId,
  process,
  processLoading,
  processError,
  processStatus,
  dagJson,
  tasks,
  selectedUuid,
  onSelectUuid,
  onClose,
  onRetryFailedTask,
  retryTaskPending,
  retryProcessPending,
  retryTaskError,
  onRequestReview,
}: Props) {
  const dag = useMemo(() => parsePlannerDag(dagJson), [dagJson]);
  const taskMap = useMemo(() => taskMapByUuid(tasks), [tasks]);
  const task = selectedUuid ? taskMap.get(selectedUuid) : undefined;

  const reviewerLabel = useMemo(() => {
    const ru = task?.reviewer_client_uuid?.trim();
    if (!ru) return null;
    const peer = taskMap.get(ru);
    return peer ? `${peer.role} (${ru})` : ru;
  }, [task?.reviewer_client_uuid, taskMap]);

  const [failureDetailsCopied, setFailureDetailsCopied] = useState(false);

  useEffect(() => {
    setFailureDetailsCopied(false);
  }, [selectedUuid]);

  const firstAwaitingReview = tasks.find((t) => t.status === "awaiting_review");
  const showReviewGateHint =
    processStatus === "task_review_required" && processId != null && !selectedUuid;

  const sub = dag?.subagents.find((s) => s.client_uuid === selectedUuid);
  const failureDetails =
    task?.status === "failed" ? formatFailureDebugJson(task.failure_debug_json) : null;

  async function copyFailureDetails() {
    if (!failureDetails) return;
    try {
      await navigator.clipboard.writeText(failureDetails);
      setFailureDetailsCopied(true);
      window.setTimeout(() => setFailureDetailsCopied(false), 2000);
    } catch {
      /* ignore */
    }
  }

  const awaiting = task?.status === "awaiting_review";
  const canReview = awaiting && processId != null && !!onRequestReview;

  const asideClass =
    "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden border-border bg-card";

  if (processId == null) {
    return null;
  }

  if (processLoading || !process) {
    return (
      <aside className={asideClass} aria-label="Process sidebar">
        <div
          className={`flex flex-1 items-center justify-center p-4 text-sm ${processError ? "text-destructive" : "text-muted-foreground"}`}
        >
          {processLoading
            ? "Loading process…"
            : processError
              ? processError.message
              : "No process data."}
        </div>
      </aside>
    );
  }

  const placeholderDetails = !selectedUuid && !showReviewGateHint;
  const title = placeholderDetails
    ? "Process overview"
    : showReviewGateHint
      ? "Task review"
      : "Task details";

  const detailsBody = (() => {
    if (placeholderDetails) {
      return <ProcessRailPlaceholder process={process} />;
    }
    if (showReviewGateHint) {
      return (
        <div className="space-y-3 p-3 text-sm">
          <p className="text-muted-foreground">
            This process is paused until you review at least one subagent output. Select a task in the{" "}
            <strong className="text-foreground">Review</strong> column or graph, then use{" "}
            <strong className="text-foreground">Review task</strong> in the sidebar to approve, reject, or request
            changes.
          </p>
          {firstAwaitingReview && (
            <Button
              type="button"
              className="w-full"
              onClick={() => onSelectUuid(firstAwaitingReview.client_uuid)}
            >
              Open first task awaiting review
            </Button>
          )}
        </div>
      );
    }
    if (!selectedUuid) return null;

    return (
      <ScrollArea className="min-h-0 min-w-0 flex-1">
        <div className="space-y-3 p-3">
          {sub && (
            <div className="space-y-1">
              <span className="text-xs text-muted-foreground">role</span>
              <div className="text-base font-semibold leading-snug text-foreground">{sub.role}</div>
            </div>
          )}

          {task && (
            <>
              {sub ? <Separator /> : null}
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">task status</span>
                <div className="text-sm">{task.status}</div>
              </div>
              {reviewerLabel && (
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">reviewer</span>
                  <div className="text-sm break-all">{reviewerLabel}</div>
                </div>
              )}
              {failureDetails && (
                <div className="space-y-1">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs text-muted-foreground">failure details</span>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      className="h-7 shrink-0 gap-1.5 px-2 text-xs"
                      aria-label="Copy failure details"
                      onClick={() => void copyFailureDetails()}
                    >
                      <Copy className="size-3.5" aria-hidden />
                      {failureDetailsCopied ? "Copied" : "Copy"}
                    </Button>
                  </div>
                  <pre className="bg-muted/40 border-border max-h-64 overflow-auto rounded-md border p-2 font-mono text-xs break-words whitespace-pre-wrap text-muted-foreground">
                    {failureDetails}
                  </pre>
                </div>
              )}
              {task.status === "failed" &&
                processStatus === "failed" &&
                processId != null &&
                onRetryFailedTask && (
                  <div className="space-y-1">
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      className="w-full"
                      disabled={retryTaskPending || retryProcessPending}
                      onClick={() => onRetryFailedTask(task.id)}
                    >
                      {retryTaskPending ? "Retrying…" : "Retry this task"}
                    </Button>
                    {retryTaskError && (
                      <p className="text-destructive text-xs">{retryTaskError.message}</p>
                    )}
                  </div>
                )}
              {task.parent_client_uuid ? (
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">subtask of</span>
                  <Button
                    type="button"
                    variant="link"
                    className="h-auto min-h-0 justify-start break-all p-0 text-sm"
                    onClick={() => onSelectUuid(task.parent_client_uuid!)}
                  >
                    {task.parent_client_uuid}
                  </Button>
                </div>
              ) : null}
              {(task.revision_count ?? 0) > 0 && (
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">revision_count</span>
                  <div className="text-sm">{task.revision_count}</div>
                </div>
              )}
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">LLM alias (orchestrator)</span>
                <div className="text-sm">{task.llm_model ?? "— (default from env / proxy)"}</div>
                <p className="text-muted-foreground text-xs leading-snug">
                  Per-task override for the proxy <code className="text-xs">model</code> field; not the same as{" "}
                  <span className="text-foreground">role</span> above.
                </p>
              </div>
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">tokens_used</span>
                <div className="text-sm">{task.tokens_used}</div>
              </div>
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">dependencies</span>
                <pre className="bg-muted/40 border-border max-h-32 overflow-auto rounded-md border p-2 text-xs break-words whitespace-pre-wrap">
                  {task.dependencies_json}
                </pre>
              </div>
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">started_at</span>
                <div className="text-sm">{task.started_at ?? "—"}</div>
              </div>
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">completed_at</span>
                <div className="text-sm">{task.completed_at ?? "—"}</div>
              </div>
              {task.draft_output && (
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">draft_output (previous attempt)</span>
                  <pre className="bg-muted/40 border-border max-h-48 overflow-auto rounded-md border p-2 text-xs break-words whitespace-pre-wrap">
                    {task.draft_output}
                  </pre>
                </div>
              )}
              {task.review_feedback && (
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">review_feedback</span>
                  <pre className="bg-muted/40 border-border max-h-48 overflow-auto rounded-md border p-2 text-xs break-words whitespace-pre-wrap">
                    {task.review_feedback}
                  </pre>
                </div>
              )}
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">output</span>
                <pre className="bg-muted/40 border-border max-h-56 overflow-auto rounded-md border p-2 text-xs break-words whitespace-pre-wrap">
                  {task.output ?? "—"}
                </pre>
              </div>
            </>
          )}

          <div className="space-y-1">
            <span className="text-xs text-muted-foreground">client_uuid</span>
            <pre className="bg-muted/40 border-border max-h-48 overflow-auto rounded-md border p-2 text-xs break-all whitespace-pre-wrap">
              {selectedUuid}
            </pre>
          </div>

          {sub && (
            <>
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">model (planner)</span>
                <div className="text-sm">{sub.model ?? "—"}</div>
              </div>
              {sub.requires_review && (
                <div className="space-y-1">
                  <span className="text-xs text-muted-foreground">requires_review</span>
                  <div className="text-sm">yes</div>
                </div>
              )}
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">system_prompt</span>
                <pre className="bg-muted/40 border-border max-h-48 overflow-auto rounded-md border p-2 text-xs break-words whitespace-pre-wrap">
                  {sub.system_prompt}
                </pre>
              </div>
              <div className="space-y-1">
                <span className="text-xs text-muted-foreground">instructions</span>
                <pre className="bg-muted/40 border-border max-h-48 overflow-auto rounded-md border p-2 text-xs break-words whitespace-pre-wrap">
                  {sub.instructions}
                </pre>
              </div>
            </>
          )}

          {!sub && !task && (
            <p className="text-muted-foreground text-sm">No planner or task data for this id.</p>
          )}
        </div>
      </ScrollArea>
    );
  })();

  return (
    <aside className={asideClass} aria-label="Process workspace sidebar">
      <Tabs defaultValue="details" className="flex min-h-0 flex-1 flex-col gap-0">
        <div className="bg-muted/50 flex shrink-0 flex-col gap-2 border-b border-border px-3 py-2">
          <div className="flex items-center justify-between gap-2">
            <h2 className="text-base leading-snug font-semibold">{title}</h2>
            {(selectedUuid || showReviewGateHint) && (
              <Button type="button" variant="ghost" size="sm" onClick={onClose}>
                Close
              </Button>
            )}
          </div>
          {canReview && (
            <Button type="button" className="w-full" size="sm" onClick={onRequestReview}>
              Review task
            </Button>
          )}
          <TabsList className="h-8 w-full max-w-full">
            <TabsTrigger value="details" className="flex-1 text-xs">
              Details
            </TabsTrigger>
            <TabsTrigger value="chat" className="flex-1 text-xs">
              Chat
            </TabsTrigger>
          </TabsList>
        </div>

        <TabsContent
          value="details"
          className="mt-0 flex min-h-0 flex-1 flex-col overflow-hidden outline-none data-[hidden]:hidden"
        >
          {detailsBody}
        </TabsContent>

        <TabsContent
          value="chat"
          className="mt-0 flex min-h-0 flex-1 flex-col overflow-hidden p-0 outline-none data-[hidden]:hidden"
        >
          <ProcessChatPanel
            processId={processId}
            process={process}
            mode={selectedUuid ? "subagent" : "process"}
            clientUuid={selectedUuid}
            subagent={sub}
            task={task}
          />
        </TabsContent>
      </Tabs>
    </aside>
  );
}
