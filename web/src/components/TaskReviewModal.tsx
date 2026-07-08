import { useEffect, useState } from "react";

import type { TaskNodeRecord } from "../api/types";
import { useReviewTaskMutation } from "../hooks/useProcessQueries";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

const textareaClass = cn(
  "mt-1 w-full min-w-0 rounded-lg border border-input bg-transparent px-2.5 py-2 text-sm transition-colors outline-none",
  "placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
  "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
  "dark:bg-input/30",
);

function shortUuid(uuid: string): string {
  const t = uuid.replace(/-/g, "");
  return t.length >= 8 ? t.slice(0, 8) : uuid.slice(0, 8);
}

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  processId: number | null;
  task: TaskNodeRecord | undefined;
  roleLabel: string | null;
};

export function TaskReviewModal({ open, onOpenChange, processId, task, roleLabel }: Props) {
  const reviewMutation = useReviewTaskMutation();
  const [approveOutput, setApproveOutput] = useState("");
  const [feedback, setFeedback] = useState("");
  const [reviseInstructions, setReviseInstructions] = useState("");

  const eligible =
    !!task && task.status === "awaiting_review" && processId != null;

  useEffect(() => {
    if (open) {
      setApproveOutput("");
      setFeedback("");
      setReviseInstructions("");
    }
  }, [open, task?.id]);

  if (!task || !eligible) {
    return null;
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="flex max-h-[90vh] max-w-2xl flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl"
        showCloseButton
      >
        <DialogHeader className="border-border shrink-0 space-y-1 border-b px-6 py-4">
          <DialogTitle>Review task</DialogTitle>
          <p className="text-muted-foreground text-sm">
            <span className="text-foreground font-medium">{roleLabel ?? "Subagent"}</span>
            <span className="ml-2 font-mono text-xs">{shortUuid(task.client_uuid)}</span>
          </p>
        </DialogHeader>

        <ScrollArea className="min-h-0 max-h-[min(45vh,380px)]">
          <div className="space-y-4 px-6 py-4">
            {task.draft_output && (
              <div className="space-y-1">
                <span className="text-muted-foreground text-xs font-medium">Previous draft</span>
                <pre className="bg-muted/40 max-h-36 overflow-auto rounded-md border border-border p-2 text-xs whitespace-pre-wrap break-words">
                  {task.draft_output}
                </pre>
              </div>
            )}
            {task.review_feedback && (
              <div className="space-y-1">
                <span className="text-muted-foreground text-xs font-medium">Prior review feedback</span>
                <pre className="bg-muted/40 max-h-36 overflow-auto rounded-md border border-border p-2 text-xs whitespace-pre-wrap break-words">
                  {task.review_feedback}
                </pre>
              </div>
            )}
            <div className="space-y-1">
              <span className="text-muted-foreground text-xs font-medium">Output</span>
              <pre className="bg-muted/40 max-h-44 overflow-auto rounded-md border border-border p-2 text-xs whitespace-pre-wrap break-words">
                {task.output ?? "—"}
              </pre>
            </div>
          </div>
        </ScrollArea>

        <div className="border-border space-y-3 border-t px-6 py-4">
          <label className="text-muted-foreground text-xs">
            Optional: replace final output on approve
            <textarea
              value={approveOutput}
              onChange={(e) => setApproveOutput(e.target.value)}
              rows={3}
              className={textareaClass}
              placeholder="Leave empty to keep model output as-is"
            />
          </label>
          <label className="text-muted-foreground text-xs">
            Feedback (required for request changes)
            <textarea
              value={feedback}
              onChange={(e) => setFeedback(e.target.value)}
              rows={3}
              className={textareaClass}
              placeholder="What should change?"
            />
          </label>
          <label className="text-muted-foreground text-xs">
            Optional: replace task instructions on request changes
            <textarea
              value={reviseInstructions}
              onChange={(e) => setReviseInstructions(e.target.value)}
              rows={2}
              className={textareaClass}
              placeholder="Leave empty to keep current instructions"
            />
          </label>
        </div>

        <DialogFooter className="border-border shrink-0 flex-col gap-3 border-t px-6 py-4 sm:flex-col sm:items-stretch sm:justify-start">
          {reviewMutation.isError && (
            <p className="text-destructive text-sm" role="alert">
              {(reviewMutation.error as Error).message}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              className="min-w-[6rem]"
              disabled={reviewMutation.isPending}
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              className="min-w-[6rem]"
              disabled={reviewMutation.isPending}
              onClick={() => {
                reviewMutation.mutate(
                  {
                    processId: processId!,
                    taskId: task.id,
                    decision: "approve",
                    output: approveOutput.trim() ? approveOutput : undefined,
                  },
                  {
                    onSuccess: () => {
                      onOpenChange(false);
                    },
                  },
                );
              }}
            >
              {reviewMutation.isPending ? "…" : "Approve"}
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={reviewMutation.isPending}
              onClick={() => {
                if (!confirm("Reject this task and fail the process?")) return;
                reviewMutation.mutate(
                  { processId: processId!, taskId: task.id, decision: "reject" },
                  { onSuccess: () => onOpenChange(false) },
                );
              }}
            >
              Reject
            </Button>
            <Button
              type="button"
              variant="secondary"
              disabled={reviewMutation.isPending}
              onClick={() => {
                const fb = feedback.trim();
                if (!fb) {
                  alert("Feedback is required for request changes.");
                  return;
                }
                reviewMutation.mutate(
                  {
                    processId: processId!,
                    taskId: task.id,
                    decision: "request_changes",
                    feedback: fb,
                    instructions: reviseInstructions.trim() ? reviseInstructions : undefined,
                  },
                  { onSuccess: () => onOpenChange(false) },
                );
              }}
            >
              Request changes
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
