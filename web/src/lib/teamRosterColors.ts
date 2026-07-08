import type { RosterRole } from "../api/types";

/** Delegation-style distinct accents when a role has no `accent_color`. */
export const ROSTER_ACCENT_PALETTE = [
  "#2563eb",
  "#16a34a",
  "#9333ea",
  "#ca8a04",
  "#dc2626",
  "#0ea5e9",
] as const;

/** First root in roster order — matches `rosterLineagePositions` (invalid parent id ⇒ root). */
export function primaryLeadRoleId(
  roles: Pick<RosterRole, "id" | "parent_id">[],
): string | null {
  const byId = new Map(roles.map((r) => [r.id, r]));
  const roots = roles.filter((r) => !r.parent_id || !byId.has(r.parent_id));
  if (roots.length === 0) return roles[0]?.id ?? null;
  return roots[0]!.id;
}

/** Resolve border/avatar tint: explicit hex, else team color for primary lead, else palette slot. */
export function resolveRoleAccent(
  role: RosterRole,
  roles: RosterRole[],
  teamAccent: string,
): string {
  const explicit = role.accent_color?.trim();
  if (explicit) return explicit;
  const leadId = primaryLeadRoleId(roles);
  if (role.id === leadId) return teamAccent;
  const idx = roles.findIndex((r) => r.id === role.id);
  const slot = idx >= 0 ? idx : 0;
  return ROSTER_ACCENT_PALETTE[slot % ROSTER_ACCENT_PALETTE.length]!;
}
