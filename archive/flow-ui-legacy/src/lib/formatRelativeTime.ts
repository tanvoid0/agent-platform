/** Browser `Intl.RelativeTimeFormat` helper for ISO-ish timestamps. */
export function formatRelativeTimeFromMs(pastMs: number): string {
  const now = Date.now();
  const diffSec = Math.round((now - pastMs) / 1000);
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  const abs = Math.abs(diffSec);
  if (abs < 60) return rtf.format(-diffSec, "second");
  const diffMin = Math.round(diffSec / 60);
  if (Math.abs(diffMin) < 60) return rtf.format(-diffMin, "minute");
  const diffHr = Math.round(diffMin / 60);
  if (Math.abs(diffHr) < 48) return rtf.format(-diffHr, "hour");
  const diffDay = Math.round(diffHr / 24);
  return rtf.format(-diffDay, "day");
}

export function formatRelativeTimeFromIso(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  return formatRelativeTimeFromMs(t);
}
