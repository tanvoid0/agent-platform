import { describe, expect, it } from "vitest";
import { processEligibleForEventStream } from "./useProcessQueries";

describe("processEligibleForEventStream", () => {
  it("is false for terminal and human gates", () => {
    expect(processEligibleForEventStream(undefined)).toBe(false);
    expect(processEligibleForEventStream("completed")).toBe(false);
    expect(processEligibleForEventStream("approval_required")).toBe(false);
    expect(processEligibleForEventStream("task_review_required")).toBe(false);
  });

  it("is true for in-flight execution-related statuses", () => {
    expect(processEligibleForEventStream("planning")).toBe(true);
    expect(processEligibleForEventStream("running")).toBe(true);
    expect(processEligibleForEventStream("approved")).toBe(true);
  });
});
