/** Parse teams route `:teamId` segment into selection state. */
export function parseTeamPathSegment(
  raw: string | undefined,
): number | "new" | null {
  if (raw == null || raw === "") return null;
  if (raw === "new") return "new";
  const n = parseInt(raw, 10);
  if (!Number.isFinite(n) || n <= 0) return null;
  return n;
}
