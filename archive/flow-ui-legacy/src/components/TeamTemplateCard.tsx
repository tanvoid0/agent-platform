import { Edit2, FileText, Users } from "lucide-react";
import type { TeamTemplateSummary } from "../api/types";
import { formatRelativeTimeFromIso } from "../lib/formatRelativeTime";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

function teamSubtitle(team: TeamTemplateSummary): string {
  const d = team.description?.trim();
  if (d) {
    const first = d.split(/[.\n]/)[0]?.trim() ?? "";
    if (!first) return "TEAM TEMPLATE";
    const upper = first.toUpperCase();
    return upper.length > 52 ? `${upper.slice(0, 48)}…` : upper;
  }
  return "TEAM TEMPLATE";
}

function agentLabel(count: number): string {
  return count === 1 ? "1 AGENT" : `${count} AGENTS`;
}

export function TeamTemplateCard({
  team,
  selected,
  onSelect,
  onEdit,
  isDefaultTeam,
  onSetProcessDefault,
}: {
  team: TeamTemplateSummary;
  selected: boolean;
  onSelect: () => void;
  /** Opens quick-edit for name, description, and color (e.g. modal). */
  onEdit?: () => void;
  /** When true, this template is the persisted default for the Processes page. */
  isDefaultTeam: boolean;
  /** Make this team the one used for new processes on the Processes page (stopPropagation handled here). */
  onSetProcessDefault: () => void;
}) {
  const color = team.color ?? "#14b8a6";
  const title = (team.name || "Untitled").toUpperCase();
  const subtitle = teamSubtitle(team);
  const desc =
    team.description?.trim() ||
    "No description yet. Add one when you edit this template.";
  const n = team.role_count;
  const updatedRel = formatRelativeTimeFromIso(team.updated_at);
  const category = team.category?.trim();

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      className={cn(
        "group relative w-full cursor-pointer rounded-2xl p-4 text-left transition-all",
        "focus-visible:ring-ring focus-visible:ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-offset-2",
        selected
          ? "border-4 bg-white shadow-sm"
          : "border border-zinc-200/90 bg-white shadow-[0_1px_2px_rgba(15,23,42,0.06)] hover:border-zinc-300/90 hover:shadow-[0_2px_8px_rgba(15,23,42,0.08)]",
      )}
      style={selected ? { borderColor: color } : undefined}
    >
      {selected && onEdit && (
        <Button
          type="button"
          variant="secondary"
          onClick={(e) => {
            e.stopPropagation();
            onEdit();
          }}
          className="text-foreground absolute top-3.5 right-3.5 z-10 flex items-center gap-1.5 rounded-xl bg-zinc-100 px-3 py-1.5 text-[9px] font-black uppercase tracking-widest opacity-0 transition-all hover:bg-zinc-200 group-hover:opacity-100 dark:bg-zinc-800 dark:hover:bg-zinc-700"
        >
          <Edit2 className="size-3" strokeWidth={2.5} aria-hidden />
          Edit team
        </Button>
      )}

      <div className="flex gap-3">
        <div
          className="flex h-10 shrink-0 flex-row items-center justify-center gap-1.5 rounded-xl px-2.5 shadow-sm shadow-black/10"
          style={{ backgroundColor: color }}
          aria-hidden
        >
          <Users className="size-4 shrink-0 text-white opacity-95" strokeWidth={2.25} />
          <span className="text-[11px] font-black tabular-nums leading-none text-white">
            {n}
          </span>
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <h4
              className={cn(
                "min-w-0 truncate text-[11px] font-black uppercase tracking-[0.06em] text-zinc-800",
                !team.name && "text-zinc-400",
              )}
            >
              {title}
            </h4>
            {category && (
              <Badge
                variant="secondary"
                className="h-5 max-w-[10rem] shrink truncate px-1.5 py-0 text-[8px] font-bold uppercase tracking-wide"
                title={category}
              >
                {category}
              </Badge>
            )}
          </div>
          <p className="text-muted-foreground mt-0.5 line-clamp-2 text-[9px] font-semibold uppercase tracking-[0.12em]">
            {subtitle}
          </p>
          <div className="text-muted-foreground mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[9px] font-semibold uppercase tracking-[0.1em]">
            <span className="font-mono tabular-nums text-zinc-600 dark:text-zinc-400">#{team.id}</span>
            {updatedRel && (
              <span title={team.updated_at}>
                Updated {updatedRel}
              </span>
            )}
          </div>
        </div>
      </div>

      <div className="mt-3 flex min-h-[4.25rem] items-stretch rounded-xl bg-zinc-100/90 dark:bg-zinc-900/40">
        <div className="flex min-w-0 flex-1 flex-col justify-center gap-1.5 px-3 py-2.5">
          <div className="flex flex-wrap items-center gap-1.5">
            <FileText
              className="text-muted-foreground size-3 shrink-0"
              strokeWidth={2}
              aria-hidden
            />
            <span className="text-[10px] font-black uppercase tracking-wide text-zinc-800">
              TEXT
            </span>
          </div>
          <span
            className="inline-flex w-fit items-center gap-1 rounded-full bg-white/80 px-1.5 py-0.5 dark:bg-zinc-950/30"
            title="Runs from this template use the platform default approval settings."
          >
            <span className="size-1.5 shrink-0 rounded-full bg-emerald-500" aria-hidden />
            <span className="text-[8px] font-bold uppercase tracking-wider text-zinc-500">
              Auto approve
            </span>
          </span>
        </div>
        <div
          className="bg-zinc-300/90 my-2 w-px shrink-0 self-stretch dark:bg-zinc-600"
          aria-hidden
        />
        <div className="flex min-w-0 shrink-0 flex-col justify-center px-3 py-2.5 text-right">
          <p className="text-[8px] font-semibold uppercase tracking-[0.14em] text-zinc-400">
            Generation model
          </p>
          <p className="text-foreground mt-0.5 truncate text-[11px] font-bold tabular-nums tracking-tight">
            Default
          </p>
        </div>
      </div>

      <p className="text-muted-foreground mt-3 line-clamp-3 text-[11px] font-medium leading-relaxed">
        {desc}
      </p>

      <div className="mt-3 flex flex-col gap-2">
        <div
          className="flex items-center justify-between gap-2"
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => e.stopPropagation()}
        >
          <span className="text-muted-foreground text-[9px] font-bold uppercase tracking-[0.12em]">
            Processes page team
          </span>
          {isDefaultTeam ? (
            <span className="bg-foreground text-background shrink-0 rounded-lg px-3 py-1.5 text-[9px] font-black uppercase tracking-[0.12em] shadow-sm">
              Default
            </span>
          ) : (
            <Button
              type="button"
              size="sm"
              variant="secondary"
              className="h-7 rounded-lg px-3 text-[9px] font-black uppercase tracking-[0.14em] shadow-sm"
              aria-label={`Use ${team.name || "this team"} for new processes on the Processes page`}
              onClick={(e) => {
                e.stopPropagation();
                onSetProcessDefault();
              }}
            >
              Switch
            </Button>
          )}
        </div>
        <div className="flex items-end justify-between gap-2">
          <span className="inline-flex items-center gap-1.5 rounded-full bg-zinc-100 px-2.5 py-1 text-[9px] font-black uppercase tracking-wider text-zinc-500 dark:bg-zinc-900/50">
            <Users className="size-3 opacity-70" strokeWidth={2.25} aria-hidden />
            {agentLabel(n)}
          </span>
          {selected && (
            <span className="bg-foreground text-background shrink-0 rounded-lg px-3 py-1.5 text-[9px] font-black uppercase tracking-[0.12em] shadow-sm">
              Editing
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
