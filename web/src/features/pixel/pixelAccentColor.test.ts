import { describe, expect, it } from "vitest";

import { accentMapFromTeamSnapshotJson, normalizeSnapshotHex } from "./pixelAccentColor";

describe("normalizeSnapshotHex", () => {
  it("expands 3-digit shorthand", () => {
    expect(normalizeSnapshotHex("#abc")).toBe("#aabbcc");
    expect(normalizeSnapshotHex("ABC")).toBe("#aabbcc");
  });

  it("normalizes 6-digit hex", () => {
    expect(normalizeSnapshotHex("#Aa00Ff")).toBe("#aa00ff");
  });

  it("rejects invalid input", () => {
    expect(normalizeSnapshotHex("")).toBeNull();
    expect(normalizeSnapshotHex("#abcd")).toBeNull();
    expect(normalizeSnapshotHex("gg")).toBeNull();
    expect(normalizeSnapshotHex("#gg")).toBeNull();
  });
});

describe("accentMapFromTeamSnapshotJson", () => {
  it("returns empty map for null/empty", () => {
    expect(accentMapFromTeamSnapshotJson(null).size).toBe(0);
    expect(accentMapFromTeamSnapshotJson("").size).toBe(0);
  });

  it("maps role names to hex (case-insensitive lookup via stored keys)", () => {
    const json = JSON.stringify({
      roster: {
        roles: [
          { name: "Planner", accent_color: "#336699" },
          { name: "Coder", accent_color: "ff00aa" },
        ],
      },
    });
    const m = accentMapFromTeamSnapshotJson(json);
    expect(m.get("planner")).toBe("#336699");
    expect(m.get("coder")).toBe("#ff00aa");
  });

  it("registers both id and name when both present", () => {
    const json = JSON.stringify({
      roster: {
        roles: [{ id: "lead", name: "Lead Engineer", accent_color: "#112233" }],
      },
    });
    const m = accentMapFromTeamSnapshotJson(json);
    expect(m.get("lead")).toBe("#112233");
    expect(m.get("lead engineer")).toBe("#112233");
  });

  it("accepts 3-digit accent_color", () => {
    const json = JSON.stringify({
      roster: {
        roles: [{ name: "Y", accent_color: "#abc" }],
      },
    });
    const m = accentMapFromTeamSnapshotJson(json);
    expect(m.get("y")).toBe("#aabbcc");
  });

  it("ignores invalid hex and roles with no keys", () => {
    const json = JSON.stringify({
      roster: {
        roles: [
          { name: "", id: "", accent_color: "#336699" },
          { name: "X", accent_color: "not-a-color" },
        ],
      },
    });
    const m = accentMapFromTeamSnapshotJson(json);
    expect(m.size).toBe(0);
  });
});
