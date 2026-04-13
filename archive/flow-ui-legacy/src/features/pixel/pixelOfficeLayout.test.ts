import { describe, expect, it } from "vitest";
import { footForAgent, OFFICE_DESKS } from "./pixelOfficeLayout";

describe("footForAgent", () => {
  it("seats work activity at assigned desk", () => {
    const foot = footForAgent({ slotIndex: 0, activity: "work" }, 0);
    expect(foot.sit).toBe(true);
    expect(foot.x).toBe(OFFICE_DESKS[0]!.seatX);
    expect(foot.y).toBe(OFFICE_DESKS[0]!.seatY);
  });

  it("staggers agents beyond desk count so feet differ", () => {
    const a = footForAgent({ slotIndex: 0, activity: "work" }, 0);
    const b = footForAgent({ slotIndex: 4, activity: "work" }, 0);
    expect(a.x).not.toBe(b.x);
  });

  it("walk oscillates between spawn and approach", () => {
    const a = footForAgent({ slotIndex: 0, activity: "walk" }, 0);
    const b = footForAgent({ slotIndex: 0, activity: "walk" }, 2000);
    expect(a.sit).toBe(false);
    expect(b.sit).toBe(false);
    expect(a.x !== b.x || a.y !== b.y).toBe(true);
  });
});
