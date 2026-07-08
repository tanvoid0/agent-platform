import { afterEach, describe, expect, it, vi } from "vitest";
import { apiUrl, retryFailedTask, retryProcess, syncProcess } from "./client";

describe("apiUrl", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("targets the backend port when the UI runs on the Vite dev host", () => {
    vi.stubGlobal("window", {
      location: {
        protocol: "http:",
        hostname: "127.0.0.1",
        port: "3333",
      },
    });

    expect(apiUrl("/teams/")).toBe("http://127.0.0.1:18410/teams/");
    expect(apiUrl("processes?limit=30")).toBe("http://127.0.0.1:18410/processes?limit=30");
  });
});

describe("retryProcess", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("POSTs /processes/:id/retry and parses JSON", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ process_id: 7, status: "planning", retry: "planning" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const r = await retryProcess(42);
    expect(r.process_id).toBe(7);
    expect(r.retry).toBe("planning");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(String(url)).toContain("/processes/42/retry");
    expect(init.method).toBe("POST");
  });
});

describe("retryFailedTask", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("POSTs /processes/:id/tasks/:taskId/retry and parses JSON", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ process_id: 3, task_id: 12, status: "approved", retry: "task" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const r = await retryFailedTask(3, 12);
    expect(r.process_id).toBe(3);
    expect(r.task_id).toBe(12);
    expect(r.retry).toBe("task");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(String(url)).toContain("/processes/3/tasks/12/retry");
    expect(init.method).toBe("POST");
  });
});

describe("syncProcess", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("POSTs /processes/:id/sync and parses JSON", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          process_id: 5,
          process_status: "running",
          action: "requeued_execution",
          detail: "DAG execution was scheduled again.",
          reset_running_tasks: 0,
          task_counts: { pending: 1 },
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const r = await syncProcess(5);
    expect(r.action).toBe("requeued_execution");
    expect(r.detail).toContain("scheduled");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(String(url)).toContain("/processes/5/sync");
    expect(init.method).toBe("POST");
  });
});
