import { describe, expect, it } from "vitest";
import {
  DEFAULT_VIEW_MODE,
  isProcessWorkspacePath,
  parseProcessIdParam,
  processWorkspacePath,
  viewModeFromPathname,
} from "./processWorkspaceRoutes";

describe("DEFAULT_VIEW_MODE", () => {
  it("is graph", () => {
    expect(DEFAULT_VIEW_MODE).toBe("graph");
  });
});

describe("viewModeFromPathname", () => {
  it("reads first segment for view paths", () => {
    expect(viewModeFromPathname("/graph")).toBe("graph");
    expect(viewModeFromPathname("/board")).toBe("board");
    expect(viewModeFromPathname("/timeline/42")).toBe("timeline");
    expect(viewModeFromPathname("/events/1")).toBe("events");
  });

  it("returns null for teams", () => {
    expect(viewModeFromPathname("/teams")).toBeNull();
    expect(viewModeFromPathname("/teams/3")).toBeNull();
  });
});

describe("processWorkspacePath", () => {
  it("builds paths without and with process id", () => {
    expect(processWorkspacePath("graph", null)).toBe("/graph");
    expect(processWorkspacePath("board", 7)).toBe("/board/7");
  });
});

describe("isProcessWorkspacePath", () => {
  it("matches view segments only", () => {
    expect(isProcessWorkspacePath("/graph")).toBe(true);
    expect(isProcessWorkspacePath("/graph/1")).toBe(true);
    expect(isProcessWorkspacePath("/board")).toBe(true);
    expect(isProcessWorkspacePath("/events/2")).toBe(true);
    expect(isProcessWorkspacePath("/teams")).toBe(false);
  });
});

describe("parseProcessIdParam", () => {
  it("returns null for missing or invalid", () => {
    expect(parseProcessIdParam(undefined)).toBeNull();
    expect(parseProcessIdParam("")).toBeNull();
    expect(parseProcessIdParam("0")).toBeNull();
    expect(parseProcessIdParam("-1")).toBeNull();
    expect(parseProcessIdParam("abc")).toBeNull();
  });

  it("parses positive integers", () => {
    expect(parseProcessIdParam("42")).toBe(42);
    expect(parseProcessIdParam("1")).toBe(1);
  });
});
