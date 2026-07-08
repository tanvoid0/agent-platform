import { describe, expect, it } from "vitest";
import { taskStatusColor, TASK_STATUS_COLORS } from "./taskStatusVisual";

describe("taskStatusColor", () => {
  it("maps known statuses", () => {
    expect(taskStatusColor("running")).toBe(TASK_STATUS_COLORS.running);
    expect(taskStatusColor("awaiting_review")).toBe(TASK_STATUS_COLORS.awaiting_review);
  });

  it("normalizes unknown to pending palette", () => {
    expect(taskStatusColor("weird")).toBe(TASK_STATUS_COLORS.pending);
    expect(taskStatusColor(undefined)).toBe(TASK_STATUS_COLORS.pending);
  });
});
