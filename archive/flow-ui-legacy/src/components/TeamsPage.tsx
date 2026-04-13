import { Copy, Plus, Settings } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import type { RosterRole, TeamRoster, TeamTemplateSummary } from "../api/types";
import { useDefaultTeamTemplateId } from "../hooks/useDefaultTeamTemplateId";
import {
  useCreateTeamMutation,
  useTeamDetailQuery,
  useTeamsListQuery,
  useUpdateTeamMutation,
} from "../hooks/useTeamQueries";
import { parseTeamPathSegment } from "../lib/teamPath";
import { primaryLeadRoleId } from "../lib/teamRosterColors";
import { TEAM_TEMPLATE_PRESETS } from "../lib/teamTemplatePresets";
import { AppTopNav } from "./AppTopNav";
import { TeamRosterGraph } from "./TeamRosterGraph";
import {
  TeamRoleInspector,
  TeamRoleInspectorPlaceholder,
} from "./TeamRoleInspector";
import { TeamEditDialog } from "./TeamEditDialog";
import { TeamsLibrarySidebar } from "./TeamsLibrarySidebar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

function newRoleId(): string {
  if (typeof crypto !== "undefined" && crypto.randomUUID) {
    return `role-${crypto.randomUUID().slice(0, 8)}`;
  }
  return `role-${Date.now()}`;
}

function emptyRoster(): TeamRoster {
  return {
    roles: [
      {
        id: newRoleId(),
        name: "Lead",
        description: "Coordinates the work.",
        modality: "text",
        parent_id: null,
        accent_color: null,
      },
    ],
  };
}

type FormState = {
  name: string;
  description: string;
  color: string;
  category: string;
  roster: TeamRoster;
};

export function TeamsPage() {
  const navigate = useNavigate();
  const { teamId: teamIdParam } = useParams();

  const { data: listData, isPending: listPending, error: listError } = useTeamsListQuery();
  const [selectedId, setSelectedId] = useState<number | "new" | null>(() =>
    parseTeamPathSegment(teamIdParam),
  );
  const [highlightRoleId, setHighlightRoleId] = useState<string | null>(null);
  const [linkCopied, setLinkCopied] = useState(false);
  const [editTeam, setEditTeam] = useState<TeamTemplateSummary | null>(null);
  const [categoryFilter, setCategoryFilter] = useState("");

  const detailId =
    selectedId === "new" || selectedId === null ? null : selectedId;
  const { data: detail, isPending: detailPending } = useTeamDetailQuery(detailId);

  const [form, setForm] = useState<FormState>(() => ({
    name: "",
    description: "",
    color: "#6366f1",
    category: "",
    roster: emptyRoster(),
  }));

  const createMut = useCreateTeamMutation();
  const updateMut = useUpdateTeamMutation();
  const [defaultTeamId, setDefaultTeamId] = useDefaultTeamTemplateId();

  useEffect(() => {
    setSelectedId(parseTeamPathSegment(teamIdParam));
    setHighlightRoleId(null);
  }, [teamIdParam]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      const el = e.target as HTMLElement | null;
      if (el?.closest("input, textarea, select, [contenteditable=true]")) return;
      setHighlightRoleId(null);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (selectedId === "new") {
      setForm({
        name: "New team",
        description: "",
        color: "#6366f1",
        category: "",
        roster: emptyRoster(),
      });
      return;
    }
    if (typeof selectedId === "number" && detail && detail.id === selectedId) {
      setForm({
        name: detail.name,
        description: detail.description ?? "",
        color: detail.color ?? "#6366f1",
        category: detail.category?.trim() ?? "",
        roster: {
          roles: detail.roster.roles.map((r) => ({
            id: r.id,
            name: r.name,
            description: r.description ?? "",
            modality: r.modality ?? "text",
            parent_id: r.parent_id ?? null,
            accent_color: r.accent_color ?? null,
          })),
        },
      });
    }
  }, [selectedId, detail]);

  const roleById = useMemo(() => {
    const m = new Map<string, RosterRole>();
    for (const r of form.roster.roles) m.set(r.id, r);
    return m;
  }, [form.roster.roles]);

  const inspectedRole = highlightRoleId ? roleById.get(highlightRoleId) : undefined;

  const onRosterRoleClick = useCallback((roleId: string) => {
    setHighlightRoleId(roleId);
  }, []);

  const setRoleById = useCallback((roleId: string, patch: Partial<RosterRole>) => {
    setForm((f) => {
      const idx = f.roster.roles.findIndex((r) => r.id === roleId);
      if (idx < 0) return f;
      const roles = [...f.roster.roles];
      const current = roles[idx]!;
      const oldId = current.id;
      const merged = { ...current, ...patch };
      if (patch.id !== undefined && patch.id !== oldId) {
        const newId = patch.id;
        for (let i = 0; i < roles.length; i++) {
          if (roles[i]!.parent_id === oldId) {
            roles[i] = { ...roles[i]!, parent_id: newId };
          }
        }
        roles[idx] = merged;
      } else {
        roles[idx] = merged;
      }
      return { ...f, roster: { roles } };
    });
    if (patch.id !== undefined && patch.id !== roleId) {
      setHighlightRoleId((h) => (h === roleId ? patch.id! : h));
    }
  }, []);

  const removeRoleById = useCallback((roleId: string) => {
    setForm((f) => {
      const index = f.roster.roles.findIndex((r) => r.id === roleId);
      if (index < 0) return f;
      const removed = f.roster.roles[index]!.id;
      const roles = f.roster.roles
        .filter((_, i) => i !== index)
        .map((r) => (r.parent_id === removed ? { ...r, parent_id: null } : r));
      if (roles.length === 0) {
        return { ...f, roster: emptyRoster() };
      }
      return { ...f, roster: { roles } };
    });
    setHighlightRoleId((h) => (h === roleId ? null : h));
  }, []);

  const addRole = useCallback((parentId: string | null = null) => {
    const id = newRoleId();
    setForm((f) => ({
      ...f,
      roster: {
        roles: [
          ...f.roster.roles,
          {
            id,
            name: "Role",
            description: "",
            modality: "text",
            parent_id: parentId,
            accent_color: null,
          },
        ],
      },
    }));
    setHighlightRoleId(id);
  }, []);

  const addSubordinate = useCallback(() => {
    const lead = primaryLeadRoleId(form.roster.roles);
    const parent =
      highlightRoleId && roleById.has(highlightRoleId) ? highlightRoleId : lead;
    addRole(parent);
  }, [addRole, form.roster.roles, highlightRoleId, roleById]);

  async function onSave() {
    const body = {
      name: form.name.trim(),
      description: form.description.trim() || null,
      color: form.color.trim() || null,
      category: form.category.trim() || null,
      roster: form.roster,
    };
    if (!body.name) return;
    if (selectedId === "new") {
      const created = await createMut.mutateAsync(body);
      navigate(`/teams/${created.id}`, { replace: true });
      return;
    }
    if (typeof selectedId === "number") {
      await updateMut.mutateAsync({ teamId: selectedId, body });
    }
  }

  async function copyTeamDeepLink() {
    if (selectedId === null) return;
    const path =
      selectedId === "new" ? "/flow/teams/new" : `/flow/teams/${selectedId}`;
    const url = `${window.location.origin}${path}`;
    try {
      await navigator.clipboard.writeText(url);
      setLinkCopied(true);
      window.setTimeout(() => setLinkCopied(false), 2000);
    } catch {
      /* ignore */
    }
  }

  const teams = listData?.teams ?? [];

  useEffect(() => {
    if (listPending) return;
    if (defaultTeamId == null) return;
    if (teams.length === 0) return;
    if (!teams.some((t) => t.id === defaultTeamId)) {
      setDefaultTeamId(null);
    }
  }, [listPending, teams, defaultTeamId, setDefaultTeamId]);

  const filteredTeams = useMemo(() => {
    const q = categoryFilter.trim().toLowerCase();
    if (!q) return teams;
    return teams.filter((t) => (t.category ?? "").toLowerCase().includes(q));
  }, [teams, categoryFilter]);
  const saving = createMut.isPending || updateMut.isPending;
  const err = createMut.error ?? updateMut.error ?? listError ?? null;

  const accent = form.color?.trim() || "#6366f1";

  const rosterInvalid = useMemo(() => {
    const ids = form.roster.roles.map((r) => r.id.trim()).filter((x) => x.length > 0);
    if (ids.length !== form.roster.roles.length) return true;
    return new Set(ids).size !== ids.length;
  }, [form.roster.roles]);

  function applyPreset(presetId: string) {
    const preset = TEAM_TEMPLATE_PRESETS.find((p) => p.id === presetId);
    if (!preset) return;
    const hasName = form.name.trim().length > 0 && form.name.trim() !== "New team";
    if (
      hasName &&
      !window.confirm(
        "Replace the current name, description, color, category, and roles with this example?",
      )
    ) {
      return;
    }
    setForm({
      name: preset.name,
      description: preset.description,
      color: preset.color,
      category: preset.category?.trim() ?? "",
      roster: {
        roles: preset.roster.roles.map((r) => ({
          ...r,
          accent_color: r.accent_color ?? null,
        })),
      },
    });
    setHighlightRoleId(null);
  }

  const editorOpen = selectedId === "new" || typeof selectedId === "number";

  return (
    <div className="flex min-h-screen flex-col">
      <AppTopNav committedProcessId={null} />
      <div className="flex min-h-0 w-full flex-1 flex-col md:h-[calc(100vh-3.5rem)] md:flex-row">
        <TeamsLibrarySidebar
          teams={filteredTeams}
          listPending={listPending}
          categoryFilter={categoryFilter}
          onCategoryFilterChange={setCategoryFilter}
          selectedId={selectedId}
          onSelectTeam={(id) => {
            setSelectedId(id);
            navigate(`/teams/${id}`);
            setHighlightRoleId(null);
          }}
          onCreateNew={() => {
            setSelectedId("new");
            navigate("/teams/new");
            setHighlightRoleId(null);
          }}
          onEditTeam={(team) => setEditTeam(team)}
          defaultTeamId={defaultTeamId}
          onDefaultTeamChange={setDefaultTeamId}
        />

        <TeamEditDialog
          open={editTeam != null}
          onOpenChange={(open) => {
            if (!open) setEditTeam(null);
          }}
          team={editTeam}
          onDeleted={(id) => {
            setEditTeam(null);
            if (selectedId === id) {
              navigate("/teams");
            }
          }}
        />

      <div className="bg-muted/25 flex min-h-0 min-h-[28rem] flex-1 flex-col md:min-h-0">
        <header className="border-border flex h-14 shrink-0 items-center justify-between gap-3 border-b bg-card px-4 md:px-6">
          <div className="flex min-w-0 items-center gap-2">
            <Settings className="text-foreground size-[18px] shrink-0" strokeWidth={2} aria-hidden />
            <h2 className="text-foreground truncate text-xs font-black uppercase tracking-[0.2em]">
              Manage teams
            </h2>
          </div>
          <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
            {editorOpen && (!detailPending || selectedId === "new") && (
              <Button
                type="button"
                size="sm"
                className="h-8 rounded-xl px-3 text-[10px] font-black uppercase tracking-[0.08em] shadow-sm"
                disabled={saving || !form.name.trim() || rosterInvalid}
                onClick={() => void onSave()}
              >
                {saving
                  ? "Saving…"
                  : selectedId === "new"
                    ? "Create team"
                    : "Save roster"}
              </Button>
            )}
            {editorOpen && (
              <Button
                type="button"
                size="sm"
                variant="outline"
                className="h-8 gap-1.5 rounded-xl text-xs font-semibold"
                onClick={() => void copyTeamDeepLink()}
              >
                <Copy className="size-3.5" aria-hidden />
                {linkCopied ? "Copied" : "Copy link"}
              </Button>
            )}
          </div>
        </header>

        {selectedId === null && (
          <div className="text-muted-foreground flex flex-1 flex-col items-center justify-center gap-2 px-6 py-16 text-center">
            <p className="text-sm font-medium">Select a team from the library</p>
            <p className="text-muted-foreground/80 max-w-sm text-xs leading-relaxed">
              Or use <span className="font-mono text-foreground/90">Create new team</span> to
              draft a template. Deep links use paths like{" "}
              <span className="font-mono text-foreground/90">/flow/teams/&lt;id&gt;</span>.
            </p>
          </div>
        )}

        {editorOpen && (
          <>
            {detailPending && selectedId !== "new" && (
              <div className="text-muted-foreground border-border border-b bg-card px-6 py-3 text-sm">
                Loading template…
              </div>
            )}

            {!detailPending || selectedId === "new" ? (
              <>
                {(rosterInvalid || err != null) && (
                  <div className="border-border shrink-0 border-b bg-card px-4 py-2 md:px-6">
                    {rosterInvalid && (
                      <p className="text-destructive text-xs font-medium">
                        Fix duplicate or empty role ids before saving.
                      </p>
                    )}
                    {err != null && (
                      <p className="text-destructive text-xs font-medium">
                        {err instanceof Error ? err.message : String(err)}
                      </p>
                    )}
                  </div>
                )}

                <div className="border-border shrink-0 border-b bg-card px-4 py-2 md:px-6">
                  <label className="text-muted-foreground mb-1 block text-[9px] font-black uppercase tracking-[0.12em]">
                    Category (optional)
                  </label>
                  <Input
                    value={form.category}
                    onChange={(e) =>
                      setForm((f) => ({ ...f, category: e.target.value }))
                    }
                    placeholder="e.g. Engineering, Content"
                    className="max-w-md h-8 rounded-lg text-sm"
                    autoComplete="off"
                    aria-label="Team template category"
                  />
                </div>

                <div className="flex min-h-0 flex-1 flex-col lg:flex-row">
                  <div className="relative min-h-[min(42vh,24rem)] min-w-0 flex-1 lg:min-h-0">
                    <TeamRosterGraph
                      roles={form.roster.roles}
                      accentColor={accent}
                      highlightRoleId={highlightRoleId}
                      onRoleClick={onRosterRoleClick}
                      onPaneClick={() => setHighlightRoleId(null)}
                      className="h-full min-h-[min(42vh,24rem)] w-full lg:min-h-0"
                    />
                    <div className="pointer-events-none absolute bottom-4 left-1/2 z-10 flex -translate-x-1/2 flex-col items-center gap-2">
                      <div className="pointer-events-auto flex max-w-[min(100vw-2rem,36rem)] flex-wrap items-center justify-center gap-2">
                        <select
                          aria-label="Load example preset"
                          className="border-input bg-background h-9 max-w-[11rem] rounded-xl border border-zinc-200/80 px-2.5 text-[10px] font-semibold shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:border-border"
                          value=""
                          onChange={(e) => {
                            const v = e.target.value;
                            e.target.value = "";
                            if (v) applyPreset(v);
                          }}
                        >
                          <option value="">Load example…</option>
                          {TEAM_TEMPLATE_PRESETS.map((p) => (
                            <option key={p.id} value={p.id}>
                              {p.name}
                            </option>
                          ))}
                        </select>
                        <Button
                          type="button"
                          size="sm"
                          variant="secondary"
                          className="h-9 rounded-xl text-[10px] font-black uppercase tracking-widest shadow-sm"
                          onClick={() => addRole(null)}
                        >
                          <Plus className="size-3.5" aria-hidden />
                          Add role
                        </Button>
                        <Button
                          type="button"
                          size="sm"
                          className="h-9 rounded-xl text-[10px] font-black uppercase tracking-widest shadow-md"
                          onClick={() => addSubordinate()}
                        >
                          <Plus className="size-3.5" aria-hidden />
                          Add subordinate
                        </Button>
                      </div>
                      <p className="text-muted-foreground max-w-[18rem] text-center text-[9px] font-medium leading-snug">
                        Subordinates attach to the selected node, or to the lead if none is
                        selected.
                      </p>
                    </div>
                  </div>

                  <div
                    className={cn(
                      "border-border flex w-full shrink-0 flex-col border-t bg-white lg:w-[min(100%,24rem)] lg:border-t-0 lg:border-l dark:bg-card",
                    )}
                  >
                    {inspectedRole ? (
                      <TeamRoleInspector
                        key={inspectedRole.id}
                        role={inspectedRole}
                        roles={form.roster.roles}
                        teamAccent={accent}
                        onChange={(patch) => setRoleById(inspectedRole.id, patch)}
                        onRemove={() => removeRoleById(inspectedRole.id)}
                        onDismiss={() => setHighlightRoleId(null)}
                      />
                    ) : (
                      <TeamRoleInspectorPlaceholder />
                    )}
                  </div>
                </div>
              </>
            ) : null}
          </>
        )}
      </div>
    </div>
    </div>
  );
}
