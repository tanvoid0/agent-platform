import { Library, Plus } from "lucide-react";
import type { TeamTemplateSummary } from "../api/types";
import { TeamTemplateCard } from "./TeamTemplateCard";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export type TeamsLibrarySidebarProps = {
  teams: TeamTemplateSummary[];
  listPending: boolean;
  /** Case-insensitive substring match on `category`. */
  categoryFilter: string;
  onCategoryFilterChange: (value: string) => void;
  selectedId: number | "new" | null;
  onSelectTeam: (id: number) => void;
  onCreateNew: () => void;
  onEditTeam?: (team: TeamTemplateSummary) => void;
  /** Persisted default team template for new processes (browser). */
  defaultTeamId: number | null;
  onDefaultTeamChange: (id: number | null) => void;
};

export function TeamsLibrarySidebar({
  teams,
  listPending,
  categoryFilter,
  onCategoryFilterChange,
  selectedId,
  onSelectTeam,
  onCreateNew,
  onEditTeam,
  defaultTeamId,
  onDefaultTeamChange,
}: TeamsLibrarySidebarProps) {
  return (
    <aside
      className={cn(
        "border-sidebar-border bg-sidebar text-sidebar-foreground flex h-full min-h-0 w-full min-w-0 shrink-0 flex-col md:w-[22rem] md:border-r",
      )}
    >
      <header className="border-border bg-card flex h-14 shrink-0 items-center gap-2 border-b px-4 md:px-6">
        <Library className="text-foreground size-[18px] shrink-0" strokeWidth={2} aria-hidden />
        <h2 className="text-foreground truncate text-xs font-black uppercase tracking-[0.2em]">
          Team library
        </h2>
      </header>

      <div className="border-border shrink-0 border-b px-4 py-2 md:px-6">
        <Input
          type="search"
          value={categoryFilter}
          onChange={(e) => onCategoryFilterChange(e.target.value)}
          placeholder="Filter by category…"
          className="h-8 rounded-lg text-xs"
          aria-label="Filter team templates by category"
        />
      </div>

      <div className="task-board-scroll flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-4 pb-3 md:px-6">
        {selectedId === "new" && (
          <div className="rounded-2xl border-4 border-[#6366f1] bg-card p-4 shadow-sm">
            <p className="text-[10px] font-black uppercase tracking-[0.14em] text-indigo-700 dark:text-indigo-400">
              New template
            </p>
            <p className="text-muted-foreground mt-2 text-[11px] leading-relaxed">
              Fill in details in the editor and save to add this team to the library.
            </p>
          </div>
        )}

        {listPending && (
          <p className="text-muted-foreground py-6 text-center text-sm font-medium">
            Loading…
          </p>
        )}
        {!listPending && teams.length === 0 && selectedId !== "new" && (
          <p className="text-muted-foreground py-10 text-center text-sm leading-relaxed">
            No saved templates yet. Use{" "}
            <span className="font-semibold text-foreground/90">Create new team</span> below.
          </p>
        )}

        {!listPending &&
          teams.map((t) => (
            <TeamTemplateCard
              key={t.id}
              team={t}
              selected={selectedId === t.id}
              onSelect={() => onSelectTeam(t.id)}
              onEdit={onEditTeam ? () => onEditTeam(t) : undefined}
              isDefaultTeam={defaultTeamId === t.id}
              onSetProcessDefault={() => onDefaultTeamChange(t.id)}
            />
          ))}
      </div>

      <div className="border-border bg-card shrink-0 border-t px-4 py-3 md:px-6">
        <Button
          type="button"
          onClick={onCreateNew}
          className="h-auto w-full gap-2 rounded-xl bg-zinc-900 py-3.5 text-[10px] font-black uppercase tracking-[0.14em] text-white shadow-md hover:bg-zinc-800 dark:bg-zinc-950 dark:hover:bg-zinc-900"
        >
          <Plus className="size-4" strokeWidth={2.5} aria-hidden />
          Create new team
        </Button>
      </div>
    </aside>
  );
}
