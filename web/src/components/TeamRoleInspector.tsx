import {
  Check,
  CircleHelp,
  Eye,
  Settings,
  Trash2,
  User,
  UserRound,
  X,
  Zap,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { useMemo } from "react";
import type { RoleModality, RosterRole } from "../api/types";
import { primaryLeadRoleId, resolveRoleAccent } from "../lib/teamRosterColors";
import { RoleAvatar } from "./team/RoleAvatar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/** Matches agent info panel mock (navy + slate labels). */
const ink = "text-[#1A2B4B]";
const labelMuted = "text-[#94A3B8]";
const cardFill = "bg-[#F8FAFC] dark:bg-muted/40";

export type TeamRoleInspectorProps = {
  role: RosterRole;
  roles: RosterRole[];
  teamAccent: string;
  onChange: (patch: Partial<RosterRole>) => void;
  onRemove: () => void;
  onDismiss?: () => void;
};

function SectionLabel({
  icon: Icon,
  children,
  help,
}: {
  icon: LucideIcon;
  children: string;
  help?: string;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <Icon className={cn("size-3.5 shrink-0", labelMuted)} strokeWidth={2} aria-hidden />
      <span
        className={cn(
          "text-[10px] font-black uppercase tracking-[0.12em]",
          labelMuted,
        )}
      >
        {children}
      </span>
      {help ? (
        <span title={help} className="inline-flex cursor-help">
          <CircleHelp
            className={cn("size-3.5 shrink-0 opacity-70", labelMuted)}
            aria-hidden
          />
        </span>
      ) : null}
    </div>
  );
}

function FieldBlock({
  label,
  children,
}: {
  label: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="space-y-2">
      {label}
      {children}
    </div>
  );
}

const LEAD_CAPS = [
  "SET PROJECT BRIEF",
  "PROPOSE TASKS",
  "EXECUTE & COMPLETE TASKS",
  "AUTONOMOUS REASONING",
  "DELIVER PROJECT",
] as const;

const SUB_CAPS = [
  "EXECUTE ASSIGNED TASKS",
  "SYNC WITH LEAD",
  "CONTRIBUTE OUTPUTS",
  "FOLLOW TEAM POLICIES",
  "ESCALATE WHEN BLOCKED",
] as const;

export function TeamRoleInspector({
  role,
  roles,
  teamAccent,
  onChange,
  onRemove,
  onDismiss,
}: TeamRoleInspectorProps) {
  const leadId = primaryLeadRoleId(roles);
  const isLead = leadId !== null && role.id === leadId;
  const accent = resolveRoleAccent(role, roles, teamAccent);
  const parentOptions = roles.filter((r) => r.id !== role.id);

  const idTrimmed = role.id.trim();
  const idEmpty = idTrimmed.length === 0;
  const idDuplicate = useMemo(() => {
    if (!idTrimmed) return false;
    return roles.filter((r) => r.id.trim() === idTrimmed).length > 1;
  }, [roles, idTrimmed]);

  const parentLabel = role.parent_id
    ? roles.find((r) => r.id === role.parent_id)?.name || role.parent_id
    : null;

  const headerTitle = isLead ? "Lead agent info" : "Agent info";
  const modality = (role.modality ?? "text") as RoleModality;
  const caps = isLead ? LEAD_CAPS : SUB_CAPS;

  return (
    <div
      className={cn(
        "flex h-full min-h-0 w-full flex-col overflow-hidden border-border bg-white dark:bg-card",
        "animate-in fade-in slide-in-from-right-3 duration-300 fill-mode-both",
      )}
    >
      <div className="border-border flex shrink-0 items-center justify-between gap-3 border-b px-4 py-3">
        <div className="flex min-w-0 items-center gap-3">
          <div className="border-border/60 shrink-0 rounded-xl border bg-muted/30 p-0.5 shadow-sm">
            <RoleAvatar kind={isLead ? "lead" : "sub"} color={accent} size={40} />
          </div>
          <div className="min-w-0">
            <h3
              className={cn(
                "truncate text-[11px] font-black uppercase tracking-[0.14em]",
                ink,
              )}
            >
              {headerTitle}
            </h3>
            <p className={cn("truncate text-[10px] font-medium", labelMuted)}>
              {parentLabel ? `Reports to ${parentLabel}` : "Top-level role"}
            </p>
          </div>
        </div>
        {onDismiss ? (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className={cn(
              "text-[#94A3B8] hover:bg-[#F1F5F9] hover:text-[#1A2B4B] dark:hover:bg-muted",
            )}
            onClick={onDismiss}
            aria-label="Close role details"
          >
            <X className="size-[18px]" strokeWidth={2} />
          </Button>
        ) : null}
      </div>

      <div className="task-board-scroll flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-4 py-5">
        <FieldBlock
          label={
            <SectionLabel icon={User} help="Display name for this role in the roster and planner.">
              Name
            </SectionLabel>
          }
        >
          <Input
            value={role.name}
            onChange={(e) => onChange({ name: e.target.value })}
            placeholder="Role name"
            className={cn(
              "h-auto rounded-xl border-zinc-200/90 bg-white px-3 py-2.5 text-sm font-bold shadow-none dark:border-border dark:bg-background",
              ink,
            )}
          />
        </FieldBlock>

        <FieldBlock
          label={
            <SectionLabel
              icon={Settings}
              help="Templates declare output modality; the orchestrator resolves the concrete model when you start a goal."
            >
              LLM model
            </SectionLabel>
          }
        >
          <div
            className={cn(
              "flex items-center gap-2 rounded-full border border-zinc-200/90 bg-white px-1 py-1 pr-2 dark:border-border dark:bg-background",
            )}
          >
            <select
              aria-label="Output modality"
              className={cn(
                "min-w-0 flex-1 cursor-pointer appearance-none rounded-full border-0 bg-transparent py-1.5 pl-3 font-mono text-xs font-semibold outline-none",
                ink,
              )}
              value={modality}
              onChange={(e) => {
                const v = e.target.value as RoleModality;
                onChange({ modality: v });
              }}
            >
              <option value="text">text · orchestrator default</option>
              <option value="audio" disabled title="Not supported yet">
                audio
              </option>
              <option value="video" disabled title="Not supported yet">
                video
              </option>
              <option value="image" disabled title="Not supported yet">
                image
              </option>
            </select>
          </div>
        </FieldBlock>

        <FieldBlock
          label={
            <SectionLabel icon={Eye} help="Shown to the planner when building tasks for this role.">
              Description
            </SectionLabel>
          }
        >
          <textarea
            value={role.description ?? ""}
            onChange={(e) => onChange({ description: e.target.value })}
            placeholder="What this role does"
            rows={5}
            className={cn(
              "border-input w-full min-w-0 resize-none rounded-2xl border border-zinc-100 px-3.5 py-3 text-xs font-medium leading-relaxed outline-none transition-colors",
              cardFill,
              ink,
              "italic placeholder:not-italic placeholder:text-zinc-400 dark:border-border",
              "focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40",
            )}
          />
        </FieldBlock>

        <div className="space-y-2">
          <SectionLabel icon={Zap} help="Typical responsibilities for this position in the template.">
            Capabilities
          </SectionLabel>
          <ul
            className={cn(
              "space-y-2.5 rounded-2xl border border-zinc-100/90 px-3.5 py-3.5 dark:border-border",
              cardFill,
            )}
          >
            {caps.map((item) => (
              <li key={item} className="flex items-start gap-2.5">
                <span
                  className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border border-zinc-200/90 bg-white dark:border-border dark:bg-background"
                  aria-hidden
                >
                  <Check className="size-3 text-zinc-500" strokeWidth={2.5} />
                </span>
                <span
                  className={cn(
                    "text-[10px] font-black uppercase tracking-[0.06em] leading-snug",
                    ink,
                  )}
                >
                  {item}
                </span>
              </li>
            ))}
          </ul>
        </div>

        <div className="space-y-2">
          <SectionLabel
            icon={UserRound}
            help="When enabled on a process, tasks pause for your review before completion. Template view only; configure enforcement when starting a process."
          >
            Supervision
          </SectionLabel>
          <div
            className="flex items-center justify-between gap-3 rounded-2xl border border-sky-200/90 bg-sky-50/90 px-3.5 py-3 dark:border-sky-900/50 dark:bg-sky-950/30"
            title="Human-in-the-loop is enforced per process in the orchestrator."
          >
            <div className="min-w-0 flex-1">
              <p className="text-[11px] font-black uppercase tracking-wide text-sky-900 dark:text-sky-100">
                Human-in-the-loop
              </p>
              <p className="text-muted-foreground mt-0.5 text-[10px] font-medium leading-snug">
                Agent must request your validation before completing any task.
              </p>
            </div>
            <div
              className="relative h-7 w-12 shrink-0 rounded-full bg-sky-500 shadow-inner dark:bg-sky-600"
              role="img"
              aria-label="Human-in-the-loop (orchestrator default when enabled on runs)"
            >
              <span className="absolute top-0.5 right-0.5 size-6 rounded-full bg-white shadow-sm" />
            </div>
          </div>
        </div>

        <div className="border-border space-y-4 border-t pt-2">
          <p className={cn("text-[9px] font-black uppercase tracking-widest", labelMuted)}>
            Template & graph
          </p>

          <FieldBlock
            label={
              <SectionLabel icon={Settings} help="Stable id for edges and planner references; must be unique.">
                Role id
              </SectionLabel>
            }
          >
            <Input
              value={role.id}
              onChange={(e) => onChange({ id: e.target.value })}
              aria-invalid={idEmpty || idDuplicate}
              className={cn(
                "h-auto rounded-xl border-zinc-200/90 bg-[#F8FAFC] px-3 py-2 font-mono text-xs font-medium dark:border-border dark:bg-muted/30",
                ink,
                (idEmpty || idDuplicate) && "border-destructive",
              )}
            />
            {idEmpty && (
              <p className="text-destructive text-[9px] font-bold uppercase tracking-tight">
                Role id is required
              </p>
            )}
            {idDuplicate && !idEmpty && (
              <p className="text-destructive text-[9px] font-bold uppercase tracking-tight">
                Another role uses this id
              </p>
            )}
          </FieldBlock>

          <FieldBlock
            label={
              <SectionLabel icon={User} help="Hierarchy link between nodes on the roster graph.">
                Parent role
              </SectionLabel>
            }
          >
            <select
              className="border-input bg-background h-10 w-full rounded-xl border border-zinc-200/90 px-3 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:border-border"
              value={role.parent_id ?? ""}
              onChange={(e) =>
                onChange({ parent_id: e.target.value ? e.target.value : null })
              }
            >
              <option value="">(none)</option>
              {parentOptions.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name || o.id}
                </option>
              ))}
            </select>
          </FieldBlock>

          <FieldBlock
            label={
              <SectionLabel icon={Settings} help="Accent for nodes in the roster graph.">
                Accent (hex)
              </SectionLabel>
            }
          >
            <Input
              value={role.accent_color ?? ""}
              onChange={(e) =>
                onChange({ accent_color: e.target.value.trim() || null })
              }
              placeholder="#2563eb"
              className="h-auto rounded-xl border-zinc-200/90 bg-[#F8FAFC] px-3 py-2 font-mono text-xs dark:border-border dark:bg-muted/30"
            />
          </FieldBlock>
        </div>
      </div>

      <div className="border-border shrink-0 border-t bg-[#FAFAFA] px-4 py-3 dark:bg-muted/20">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="text-destructive hover:text-destructive h-auto w-full gap-2 rounded-xl py-2.5 text-[10px] font-black uppercase tracking-widest hover:bg-red-50 dark:hover:bg-red-950/30"
          onClick={onRemove}
        >
          <Trash2 className="size-3.5" aria-hidden />
          Remove from team
        </Button>
      </div>
    </div>
  );
}

export function TeamRoleInspectorPlaceholder() {
  return (
    <div
      className={cn(
        "flex h-full min-h-[14rem] w-full flex-col items-center justify-center gap-2 border-border bg-white px-6 text-center dark:bg-card",
        "animate-in fade-in duration-300",
      )}
    >
      <div className="border-border/60 flex size-12 items-center justify-center rounded-xl border bg-muted/30 p-1">
        <User className="text-muted-foreground size-7" strokeWidth={1.5} aria-hidden />
      </div>
      <p className={cn("text-[10px] font-black uppercase tracking-[0.12em]", labelMuted)}>
        Select a role
      </p>
      <p className="text-muted-foreground max-w-[14rem] text-[11px] font-medium leading-relaxed">
        Click a node on the flow to view and edit agent details.
      </p>
    </div>
  );
}
