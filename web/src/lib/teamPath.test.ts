import { describe, expect, it } from "vitest";
import { parseTeamPathSegment } from "./teamPath";

describe("parseTeamPathSegment", () => {
  it("returns null when absent", () => {
    expect(parseTeamPathSegment(undefined)).toBeNull();
    expect(parseTeamPathSegment("")).toBeNull();
  });

  it("parses new", () => {
    expect(parseTeamPathSegment("new")).toBe("new");
  });

  it("parses positive ids", () => {
    expect(parseTeamPathSegment("42")).toBe(42);
  });

  it("rejects invalid", () => {
    expect(parseTeamPathSegment("0")).toBeNull();
    expect(parseTeamPathSegment("-1")).toBeNull();
    expect(parseTeamPathSegment("abc")).toBeNull();
  });
});
