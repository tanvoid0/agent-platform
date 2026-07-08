import { fetchProcessDetail, fetchProcessEvents } from "../api/client";
import type { EventLogRecord, ProcessDetailResponse } from "../api/types";

/** Matches server cap in `list_process_events` (`process_routes.py`). */
const EVENTS_PAGE_SIZE = 2000;

/** Safety bound (~1M events) so a bug cannot loop forever. */
const MAX_EVENT_PAGES = 500;

export type ProcessExportPayload = {
  exported_at: string;
  process: ProcessDetailResponse["process"];
  tasks: ProcessDetailResponse["tasks"];
  events: EventLogRecord[];
};

function triggerJsonDownload(filename: string, data: unknown): void {
  const blob = new Blob([JSON.stringify(data, null, 2)], {
    type: "application/json",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.rel = "noopener";
  a.click();
  URL.revokeObjectURL(url);
}

async function fetchAllProcessEventsForExport(
  processId: number,
): Promise<EventLogRecord[]> {
  const all: EventLogRecord[] = [];
  let afterId = 0;
  for (let page = 0; page < MAX_EVENT_PAGES; page++) {
    const batch = await fetchProcessEvents(processId, {
      limit: EVENTS_PAGE_SIZE,
      ...(afterId > 0 ? { afterId } : {}),
    });
    const rows = batch.events;
    if (rows.length === 0) break;
    all.push(...rows);
    if (rows.length < EVENTS_PAGE_SIZE) break;
    afterId = rows[rows.length - 1]!.id;
  }
  return all;
}

export async function downloadProcessExport(processId: number): Promise<void> {
  const [detail, events] = await Promise.all([
    fetchProcessDetail(processId),
    fetchAllProcessEventsForExport(processId),
  ]);
  const payload: ProcessExportPayload = {
    exported_at: new Date().toISOString(),
    process: detail.process,
    tasks: detail.tasks,
    events,
  };
  triggerJsonDownload(`process-${processId}.json`, payload);
}
