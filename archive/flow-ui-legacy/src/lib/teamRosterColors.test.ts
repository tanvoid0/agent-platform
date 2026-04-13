import { describe, expect, it } from "vitest";

import { primaryLeadRoleId, resolveRoleAccent, ROSTER_ACCENT_PALETTE } from "./teamRosterColors";

describe("teamRosterColors", () => {
  it("picks first root as primary lead (invalid parent counts as root)", () => {
    expect(
      primaryLeadRoleId([
        { id: "x", parent_id: "z" },
        { id: "y", parent_id: null },
      ]),
    ).toBe("x");
  });

  it("uses explicit accent when set", () => {
    const roles = [
      {
        id: "a",
        name: "A",
        parent_id: null as string | null,
        accent_color: "#abcdef",
      },
    ];
    expect(resolveRoleAccent(roles[0]!, roles, "#111111")).toBe("#abcdef");
  });

  it("uses team accent for primary lead and palette for others", () => {
    const team = "#6366f1";
    const roles = [
      { id: "lead", name: "L", parent_id: null, accent_color: undefined },
      { id: "sub", name: "S", parent_id: "lead", accent_color: undefined },
    ];
    expect(resolveRoleAccent(roles[0]!, roles, team)).toBe(team);
    expect(resolveRoleAccent(roles[1]!, roles, team)).toBe(
      ROSTER_ACCENT_PALETTE[1 % ROSTER_ACCENT_PALETTE.length],
    );
  });
});
