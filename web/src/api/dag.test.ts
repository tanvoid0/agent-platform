import { describe, expect, it } from "vitest";

import { parsePlannerDag, plannerTopologicalUuids, validatePlannerDag } from "./dag";
import type { PlannerDag } from "./types";

describe("validatePlannerDag", () => {
  it("accepts a minimal valid DAG", () => {
    const raw = {
      team_name: "T",
      goal_restatement: "G",
      subagents: [
        {
          client_uuid: "a",
          role: "A",
          system_prompt: "s",
          instructions: "i",
          dependencies: [],
        },
        {
          client_uuid: "b",
          role: "B",
          system_prompt: "s",
          instructions: "i",
          dependencies: ["a"],
        },
      ],
    };
    const r = validatePlannerDag(raw);
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.dag.subagents).toHaveLength(2);
      expect(plannerTopologicalUuids(r.dag)).toEqual(["a", "b"]);
    }
  });

  it("rejects unknown dependency", () => {
    const r = validatePlannerDag({
      team_name: "T",
      goal_restatement: "G",
      subagents: [
        {
          client_uuid: "a",
          role: "A",
          system_prompt: "s",
          instructions: "i",
          dependencies: ["missing"],
        },
      ],
    });
    expect(r.ok).toBe(false);
    if (r.ok === false) {
      expect(r.errors.some((e) => e.includes("Unknown dependency"))).toBe(true);
    }
  });

  it("rejects cycles", () => {
    const r = validatePlannerDag({
      team_name: "T",
      goal_restatement: "G",
      subagents: [
        {
          client_uuid: "a",
          role: "A",
          system_prompt: "s",
          instructions: "i",
          dependencies: ["b"],
        },
        {
          client_uuid: "b",
          role: "B",
          system_prompt: "s",
          instructions: "i",
          dependencies: ["a"],
        },
      ],
    });
    expect(r.ok).toBe(false);
    if (r.ok === false) {
      expect(r.errors.some((e) => e.includes("cycle"))).toBe(true);
    }
  });

  it("rejects duplicate client_uuid", () => {
    const r = validatePlannerDag({
      team_name: "T",
      goal_restatement: "G",
      subagents: [
        {
          client_uuid: "a",
          role: "A",
          system_prompt: "s",
          instructions: "i",
        },
        {
          client_uuid: "a",
          role: "B",
          system_prompt: "s",
          instructions: "i",
        },
      ],
    });
    expect(r.ok).toBe(false);
  });
});

describe("parsePlannerDag", () => {
  it("returns null for invalid json", () => {
    expect(parsePlannerDag("{")).toBeNull();
  });

  it("returns null when structure invalid", () => {
    expect(parsePlannerDag(JSON.stringify({ subagents: [] }))).toBeNull();
  });

  it("parses valid string", () => {
    const dag: PlannerDag = {
      team_name: "T",
      goal_restatement: "G",
      subagents: [
        {
          client_uuid: "x",
          role: "R",
          system_prompt: "",
          instructions: "do",
        },
      ],
    };
    const p = parsePlannerDag(JSON.stringify(dag));
    expect(p).not.toBeNull();
    expect(p?.subagents[0].client_uuid).toBe("x");
  });
});
