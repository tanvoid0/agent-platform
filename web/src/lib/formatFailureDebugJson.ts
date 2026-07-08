/** Pretty-print stored task failure JSON for sidebar display. */
export function formatFailureDebugJson(raw: string | null | undefined): string | null {
  if (raw == null || raw === "") return null;
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
