import { describe, expect, it } from "vitest";

import { rosterLineagePositions } from "./teamRosterLayout";

describe("rosterLineagePositions", () => {
  it("lays out a simple parent chain", () => {
    const roles = [
      { id: "a", name: "A", parent_id: null as string | null },
      { id: "b", name: "B", parent_id: "a" },
      { id: "c", name: "C", parent_id: "b" },
    ];
    const pos = rosterLineagePositions(roles);
    expect(pos.get("a")!.y).toBeLessThan(pos.get("b")!.y);
    expect(pos.get("b")!.y).toBeLessThan(pos.get("c")!.y);
  });

  it("treats missing parent as root", () => {
    const roles = [
      { id: "x", name: "X", parent_id: "missing" },
      { id: "y", name: "Y", parent_id: "x" },
    ];
    const pos = rosterLineagePositions(roles);
    expect(pos.has("x")).toBe(true);
    expect(pos.has("y")).toBe(true);
    expect(pos.get("x")!.y).toBeLessThan(pos.get("y")!.y);
  });
});
