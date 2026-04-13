import { Check, CircleHelp, Pipette, Trash2, Users } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { TeamTemplateSummary } from "../api/types";
import { useDeleteTeamMutation, useUpdateTeamMutation } from "../hooks/useTeamQueries";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { toast } from "sonner";

const FOCUS_RING = "#7c3aed";

function MetaLabel({ children }: { children: string }) {
  return (
    <label className="text-[7px] font-black uppercase tracking-wide text-zinc-400 ml-1">{children}</label>
  );
}

export type TeamEditDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  team: TeamTemplateSummary | null;
  /** Called after the team is deleted successfully (e.g. clear selection / navigate). */
  onDeleted?: (teamId: number) => void;
};

export function TeamEditDialog({ open, onOpenChange, team, onDeleted }: TeamEditDialogProps) {
  const updateMut = useUpdateTeamMutation();
  const deleteMut = useDeleteTeamMutation();
  const colorInputRef = useRef<HTMLInputElement>(null);

  const [name, setName] = useState("");
  const [category, setCategory] = useState("");
  const [description, setDescription] = useState("");
  const [color, setColor] = useState("#6366f1");
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  useEffect(() => {
    if (!open || !team) return;
    setName(team.name);
    setDescription(team.description?.trim() ?? "");
    setColor(team.color?.trim() || "#6366f1");
    setCategory(team.category?.trim() ?? "");
  }, [open, team]);

  useEffect(() => {
    if (!open || !team) setDeleteConfirmOpen(false);
  }, [open, team]);

  const saving = updateMut.isPending || deleteMut.isPending;
  const err = updateMut.error;
  const accent = color || "#6366f1";

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!team) return;
    const n = name.trim();
    if (!n) return;
    await updateMut.mutateAsync({
      teamId: team.id,
      body: {
        name: n,
        description: description.trim() || null,
        color: color.trim() || null,
        category: category.trim() || null,
      },
    });
    onOpenChange(false);
  }

  async function confirmDeleteTeam() {
    if (!team) return;
    const t = team;
    try {
      await deleteMut.mutateAsync(t.id);
      setDeleteConfirmOpen(false);
      toast.success("Team deleted");
      onOpenChange(false);
      onDeleted?.(t.id);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Could not delete team");
    }
  }

  return (
    <>
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton
        className={cn(
          "gap-0 overflow-hidden p-0 sm:max-w-[420px]",
          "border-[3px] bg-zinc-50/80 shadow-xl",
        )}
        style={{ borderColor: accent }}
      >
        <form
          onSubmit={(e) => void onSubmit(e)}
          className="flex max-h-[min(90vh,720px)] flex-col"
        >
          <DialogHeader className="border-b border-zinc-100 px-5 pb-3 pt-5 pr-12">
            <DialogTitle className="text-[9px] font-black uppercase tracking-[0.1em] text-zinc-900">
              Edit team
            </DialogTitle>
          </DialogHeader>

          <div className="min-h-0 flex-1 space-y-2 overflow-y-auto px-5 py-4">
            <div className="space-y-1">
              <MetaLabel>Team color</MetaLabel>
              <div className="px-1">
                <input
                  ref={colorInputRef}
                  type="color"
                  value={/^#[0-9A-Fa-f]{6}$/.test(color) ? color : "#6366f1"}
                  onChange={(e) => setColor(e.target.value)}
                  className="sr-only"
                  aria-label="Team color"
                />
                <button
                  type="button"
                  onClick={() => colorInputRef.current?.click()}
                  className="flex size-9 shrink-0 items-center justify-center rounded-full shadow-md shadow-black/10 transition hover:opacity-95"
                  style={{ backgroundColor: accent }}
                >
                  <Pipette className="size-4 text-white" strokeWidth={2.5} aria-hidden />
                </button>
              </div>
            </div>

            <div className="space-y-1">
              <MetaLabel>Team name</MetaLabel>
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Team name"
                className="h-auto w-full rounded-xl border-zinc-100 bg-white px-2.5 py-1.5 text-[13px] font-medium"
                autoComplete="off"
                onFocus={(e) => {
                  e.target.style.borderColor = FOCUS_RING;
                }}
                onBlur={(e) => {
                  e.target.style.borderColor = "#f4f4f5";
                }}
              />
            </div>

            <div className="space-y-1">
              <MetaLabel>Category</MetaLabel>
              <Input
                value={category}
                onChange={(e) => setCategory(e.target.value)}
                placeholder="e.g. Engineering, Content"
                className="h-auto w-full rounded-xl border-zinc-100 bg-white px-2.5 py-1.5 text-[13px] font-medium"
                autoComplete="off"
                onFocus={(e) => {
                  e.target.style.borderColor = FOCUS_RING;
                }}
                onBlur={(e) => {
                  e.target.style.borderColor = "#f4f4f5";
                }}
              />
            </div>

            <div className="space-y-1">
              <MetaLabel>Description</MetaLabel>
              <div className="relative">
                <textarea
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder="What this team is for"
                  rows={4}
                  className={cn(
                    "border-input placeholder:text-muted-foreground w-full min-w-0 resize-y rounded-xl border border-zinc-100 bg-white p-2.5 pr-10 text-[13px] font-medium leading-snug outline-none transition-colors",
                    "focus-visible:border-[var(--focus)] focus-visible:ring-[var(--focus)]/25 focus-visible:ring-2",
                  )}
                  style={{ "--focus": FOCUS_RING } as React.CSSProperties}
                />
                {description.trim() !== "" && (
                  <div
                    className="pointer-events-none absolute bottom-2 right-2 flex size-5 items-center justify-center rounded-full bg-blue-500 text-white shadow-sm"
                    aria-hidden
                  >
                    <Check className="size-3" strokeWidth={3} />
                  </div>
                )}
              </div>
            </div>

            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <MetaLabel>Output type</MetaLabel>
                <select
                  disabled
                  title="Not configurable for team templates yet"
                  className="w-full cursor-not-allowed rounded-xl border border-zinc-100 bg-zinc-100/80 px-2.5 py-1.5 text-[11px] font-bold uppercase text-zinc-500 outline-none"
                  value="text"
                >
                  <option value="text">TEXT</option>
                </select>
              </div>
              <div className="space-y-1">
                <MetaLabel>Output model</MetaLabel>
                <select
                  disabled
                  title="Not configurable for team templates yet"
                  className="w-full cursor-not-allowed rounded-xl border border-zinc-100 bg-zinc-100/80 px-2.5 py-1.5 text-[10px] font-bold lowercase text-zinc-500 outline-none"
                  value="default"
                >
                  <option value="default">Orchestrator default</option>
                </select>
              </div>
            </div>

            <div
              className="mt-0.5 flex items-center justify-between rounded-xl border border-zinc-100/80 bg-zinc-50 p-2.5 opacity-80"
              title="Not configurable for team templates yet"
            >
              <div className="flex min-w-0 flex-col gap-0.5">
                <div className="flex items-center gap-1">
                  <span className="text-[8px] font-black uppercase tracking-wider text-zinc-900">
                    Auto-approve output
                  </span>
                  <span
                    className="inline-flex"
                    title="Not available for team templates yet; runs still use task review from the orchestrator."
                  >
                    <CircleHelp className="size-3 shrink-0 text-zinc-400" aria-hidden />
                  </span>
                </div>
                <span className="text-[7px] font-bold leading-tight text-zinc-400">
                  Generate asset without review
                </span>
              </div>
              <button
                type="button"
                disabled
                className="relative h-4 w-8 shrink-0 cursor-not-allowed rounded-full bg-zinc-200"
                aria-disabled
              >
                <span className="absolute top-0.5 left-1 size-3 rounded-full bg-white shadow-sm" />
              </button>
            </div>

            {err != null && (
              <p className="text-destructive rounded-lg border border-red-100 bg-red-50 px-2 py-1.5 text-[10px] font-bold uppercase tracking-tight">
                {err instanceof Error ? err.message : String(err)}
              </p>
            )}
          </div>

          <div className="border-t border-zinc-100/80 px-5 pb-4 pt-3">
            <Button
              type="submit"
              disabled={saving || !name.trim()}
              className="h-auto w-full rounded-xl bg-zinc-900 py-2.5 text-[10px] font-black uppercase tracking-[0.1em] text-white shadow-lg shadow-black/10 hover:bg-zinc-950 disabled:cursor-not-allowed disabled:bg-zinc-200 disabled:text-zinc-400 disabled:shadow-none"
            >
              {updateMut.isPending ? "Saving…" : "Save changes"}
            </Button>
          </div>

          <div className="flex items-center justify-between border-t border-zinc-100/50 px-5 py-3">
            <div className="flex items-center gap-1.5 rounded-lg bg-zinc-100/80 px-2 py-1 text-[8px] font-black uppercase text-zinc-400">
              <Users className="size-2.5" strokeWidth={3} aria-hidden />
              {team?.role_count === 1 ? "1 agent" : `${team?.role_count ?? 0} agents`}
            </div>
            <Button
              type="button"
              variant="ghost"
              disabled={saving || !team}
              onClick={() => setDeleteConfirmOpen(true)}
              className="text-destructive hover:text-destructive flex h-auto items-center gap-1.5 rounded-lg px-2 py-1 text-[8px] font-black uppercase tracking-widest hover:bg-red-50"
            >
              <Trash2 className="size-3" aria-hidden />
              Delete team
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>

    <AlertDialog open={deleteConfirmOpen} onOpenChange={setDeleteConfirmOpen}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete this team template?</AlertDialogTitle>
          <AlertDialogDescription>This cannot be undone.</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel type="button" disabled={deleteMut.isPending}>
            Cancel
          </AlertDialogCancel>
          <Button
            type="button"
            variant="destructive"
            disabled={deleteMut.isPending}
            onClick={() => void confirmDeleteTeam()}
          >
            {deleteMut.isPending ? "Deleting…" : "Delete"}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    </>
  );
}
