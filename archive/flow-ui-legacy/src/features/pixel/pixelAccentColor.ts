/** Normalize user/snapshot hex to lowercase `#rrggbb` (supports `#rgb` shorthand). */
export function normalizeSnapshotHex(raw: string): string | null {
  let s = raw.trim();
  if (!s) return null;
  if (!s.startsWith("#")) s = `#${s}`;
  if (/^#[0-9a-fA-F]{3}$/.test(s)) {
    const a = s.slice(1);
    return `#${a[0]}${a[0]}${a[1]}${a[1]}${a[2]}${a[2]}`.toLowerCase();
  }
  if (/^#[0-9a-fA-F]{6}$/.test(s)) {
    return s.toLowerCase();
  }
  return null;
}

/**
 * Build a lookup from roster role **name** or **id** (lowercased) to `#rrggbb` for pixel torso tint.
 * The planner’s `subagent.role` may match either field; both keys point to the same color.
 * Accepts `RunRecord.team_snapshot_json` as stored by the API.
 */
export function accentMapFromTeamSnapshotJson(json: string | null | undefined): Map<string, string> {
  const m = new Map<string, string>();
  if (!json?.trim()) return m;
  try {
    const data = JSON.parse(json) as {
      roster?: {
        roles?: Array<{ id?: string; name?: string; accent_color?: string | null }>;
      };
    };
    const roles = data.roster?.roles;
    if (!Array.isArray(roles)) return m;
    for (const r of roles) {
      const raw = typeof r.accent_color === "string" ? r.accent_color.trim() : "";
      const hex = normalizeSnapshotHex(raw);
      if (!hex) continue;

      const keys = new Set<string>();
      const name = typeof r.name === "string" ? r.name.trim().toLowerCase() : "";
      const id = typeof r.id === "string" ? r.id.trim().toLowerCase() : "";
      if (name) keys.add(name);
      if (id) keys.add(id);

      for (const k of keys) {
        m.set(k, hex);
      }
    }
  } catch {
    /* ignore invalid snapshot */
  }
  return m;
}

/** When the roster has no `accent_color`, derive a distinct shirt color per agent UUID. */
export function fallbackTorsoAccentFromClientUuid(clientUuid: string): string {
  let h = 0;
  for (let i = 0; i < clientUuid.length; i++) {
    h = Math.imul(31, h) + clientUuid.charCodeAt(i);
    h |= 0;
  }
  const u = h >>> 0;
  const hue = u % 360;
  const sat = 42 + (u % 20);
  const light = 48 + (u % 15);
  return `hsl(${hue} ${sat}% ${light}%)`;
}

/** Theme / UA accent for torso tint (shared by pixel canvases). */
export function readTshirtAccentColor(): string {
  const root = document.documentElement;
  const cs = getComputedStyle(root);
  const ua = cs.accentColor?.trim().toLowerCase();
  if (ua && ua !== "auto" && ua !== "") {
    return cs.accentColor;
  }
  const fromTheme =
    cs.getPropertyValue("--color-accent").trim() ||
    cs.getPropertyValue("--accent").trim();
  if (fromTheme) return fromTheme;
  return "#64748b";
}
