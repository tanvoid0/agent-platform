import { describe, expect, it, vi, afterEach } from "vitest";
import {
  boardStatusLabel,
  filterBoardRows,
  matchesBoardSearch,
  relativeTaskActivity,
  rowMatchesNeedsAttention,
  type BoardRow,
} from "./taskBoardFilter";
import type { SubagentNode, TaskNodeRecord } from "../api/types";

function sub(over: Partial<SubagentNode> = {}): SubagentNode {
  return {
    client_uuid: "abc-def-ghi",
    role: "Researcher",
    system_prompt: "sys",
    instructions: "Find sources on climate",
    ...over,
  };
}

function task(over: Partial<TaskNodeRecord> = {}): TaskNodeRecord {
  return {
    id: 1,
    process_id: 1,
    client_uuid: "abc-def-ghi",
    role: "Researcher",
    system_prompt: "sys",
    instructions: "Find sources",
    llm_model: null,
    dependencies_json: "[]",
    status: "pending",
    output: null,
    tokens_used: 0,
    started_at: null,
    completed_at: null,
    ...over,
  };
}

function row(p: Partial<BoardRow> = {}): BoardRow {
  return {
    subagent: sub(),
    task: task(),
    column: "pending",
    ...p,
  };
}

describe("matchesBoardSearch", () => {
  it("matches role case-insensitively", () => {
    expect(matchesBoardSearch(row(), "research")).toBe(true);
    expect(matchesBoardSearch(row(), "nope")).toBe(false);
  });

  it("matches uuid substring", () => {
    expect(matchesBoardSearch(row(), "abc-def")).toBe(true);
  });

  it("matches instructions and system_prompt", () => {
    expect(matchesBoardSearch(row({ subagent: sub({ instructions: "alpha beta" }) }), "beta")).toBe(
      true,
    );
    expect(matchesBoardSearch(row({ subagent: sub({ system_prompt: "gamma" }) }), "gamma")).toBe(
      true,
    );
  });

  it("empty query passes all", () => {
    expect(matchesBoardSearch(row(), "")).toBe(true);
    expect(matchesBoardSearch(row(), "   ")).toBe(true);
  });
});

describe("rowMatchesNeedsAttention", () => {
  it("awaiting_review matches", () => {
    expect(rowMatchesNeedsAttention(row({ column: "awaiting_review" }), null)).toBe(true);
  });

  it("pending matches only when process awaits DAG approval", () => {
    expect(rowMatchesNeedsAttention(row({ column: "pending" }), "approval_required")).toBe(true);
    expect(rowMatchesNeedsAttention(row({ column: "pending" }), "running")).toBe(false);
  });

  it("other columns false", () => {
    expect(rowMatchesNeedsAttention(row({ column: "running" }), "approval_required")).toBe(false);
  });
});

describe("filterBoardRows", () => {
  const rows: BoardRow[] = [
    row({ subagent: sub({ client_uuid: "u1", role: "A" }), column: "pending" }),
    row({
      subagent: sub({ client_uuid: "u2", role: "B", instructions: "special" }),
      column: "awaiting_review",
    }),
  ];

  it("applies search and needs-attention together", () => {
    const out = filterBoardRows(rows, {
      searchQuery: "special",
      needsAttentionOnly: true,
      processStatus: null,
    });
    expect(out).toHaveLength(1);
    expect(out[0].subagent.client_uuid).toBe("u2");
  });
});

describe("boardStatusLabel", () => {
  it("returns short labels", () => {
    expect(boardStatusLabel("awaiting_review")).toBe("Review");
    expect(boardStatusLabel("completed")).toBe("Done");
  });
});

describe("relativeTaskActivity", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns Finished with completed_at for terminal", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-15T12:00:00Z"));
    const r = row({
      column: "completed",
      task: task({ completed_at: "2026-01-15T11:30:00Z" }),
    });
    const s = relativeTaskActivity(r);
    expect(s).toBeTruthy();
    expect(s).toMatch(/^Finished /);
  });

  it("returns Started with started_at for non-terminal", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-15T12:00:00Z"));
    const r = row({
      column: "running",
      task: task({ started_at: "2026-01-15T11:45:00Z" }),
    });
    const s = relativeTaskActivity(r);
    expect(s).toBeTruthy();
    expect(s).toMatch(/^Started /);
  });

  it("returns null without timestamps", () => {
    expect(relativeTaskActivity(row({ column: "pending", task: task() }))).toBe(null);
  });
});
