import { describe, expect, it } from "vitest";
import type { PlannerDag } from "../api/types";
import { computeWaves, normalizeTaskStatus, taskDepthByUuid, taskMapByUuid } from "./dagTasks";

const sampleDag: PlannerDag = {
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

describe("computeWaves", () => {
  it("orders linear deps into two waves", () => {
    expect(computeWaves(sampleDag)).toEqual([["a"], ["b"]]);
  });
});

describe("normalizeTaskStatus", () => {
  it("maps known statuses", () => {
    expect(normalizeTaskStatus("Running")).toBe("running");
    expect(normalizeTaskStatus("awaiting_review")).toBe("awaiting_review");
  });

  it("defaults unknown to pending", () => {
    expect(normalizeTaskStatus("weird")).toBe("pending");
  });
});

describe("taskMapByUuid", () => {
  it("indexes by client_uuid", () => {
    const tasks = [
      {
        id: 1,
        process_id: 1,
        client_uuid: "a",
        role: "A",
        system_prompt: "s",
        instructions: "i",
        llm_model: null,
        dependencies_json: "[]",
        status: "completed",
        output: null,
        tokens_used: 0,
        started_at: null,
        completed_at: null,
      },
    ];
    const m = taskMapByUuid(tasks);
    expect(m.get("a")?.status).toBe("completed");
  });
});

describe("taskDepthByUuid", () => {
  it("counts parent chain length", () => {
    const tasks = [
      {
        id: 1,
        process_id: 1,
        client_uuid: "root",
        parent_client_uuid: null,
        role: "R",
        system_prompt: "s",
        instructions: "i",
        llm_model: null,
        dependencies_json: "[]",
        status: "completed",
        output: null,
        tokens_used: 0,
        started_at: null,
        completed_at: null,
      },
      {
        id: 2,
        process_id: 1,
        client_uuid: "c1",
        parent_client_uuid: "root",
        role: "C1",
        system_prompt: "s",
        instructions: "i",
        llm_model: null,
        dependencies_json: "[]",
        status: "pending",
        output: null,
        tokens_used: 0,
        started_at: null,
        completed_at: null,
      },
      {
        id: 3,
        process_id: 1,
        client_uuid: "c2",
        parent_client_uuid: "c1",
        role: "C2",
        system_prompt: "s",
        instructions: "i",
        llm_model: null,
        dependencies_json: "[]",
        status: "pending",
        output: null,
        tokens_used: 0,
        started_at: null,
        completed_at: null,
      },
    ];
    const d = taskDepthByUuid(tasks);
    expect(d.get("root")).toBe(0);
    expect(d.get("c1")).toBe(1);
    expect(d.get("c2")).toBe(2);
  });
});
