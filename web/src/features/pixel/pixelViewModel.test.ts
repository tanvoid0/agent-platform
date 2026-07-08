import { describe, expect, it } from "vitest";
import type { EventLogRecord, PlannerDag, TaskNodeRecord } from "../../api/types";
import { buildPixelViewModel, buildWorkHintByClientUuid, workHintFromTypeList } from "./pixelViewModel";

const dag: PlannerDag = {
  team_name: "T",
  goal_restatement: "G",
  subagents: [
    {
      client_uuid: "u1",
      role: "Planner",
      system_prompt: "s",
      instructions: "i",
      dependencies: [],
    },
    {
      client_uuid: "u2",
      role: "Coder",
      system_prompt: "s",
      instructions: "i",
      dependencies: ["u1"],
    },
  ],
};

function task(
  client_uuid: string,
  status: string,
  overrides: Partial<TaskNodeRecord> = {},
): TaskNodeRecord {
  return {
    id: 1,
    process_id: 1,
    client_uuid,
    role: client_uuid,
    system_prompt: "s",
    instructions: "i",
    llm_model: null,
    dependencies_json: "[]",
    status,
    output: null,
    tokens_used: 0,
    started_at: null,
    completed_at: null,
    ...overrides,
  };
}

describe("buildPixelViewModel", () => {
  it("maps running column to work when process is running", () => {
    const tasks = [task("u1", "completed"), task("u2", "running")];
    const vm = buildPixelViewModel("running", dag, tasks);
    const coder = vm.agents.find((a) => a.client_uuid === "u2");
    expect(coder?.activity).toBe("work");
    expect(coder?.slotIndex).toBe(1);
    expect(coder?.deskSlot).toBe(1);
  });

  it("maps pending to walk when process is running", () => {
    const tasks = [task("u1", "completed"), task("u2", "pending")];
    const vm = buildPixelViewModel("running", dag, tasks);
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.activity).toBe("walk");
  });

  it("maps pending to idle when upstream dependencies are not completed", () => {
    const tasks = [
      task("u1", "pending", { id: 1 }),
      task("u2", "pending", { id: 2, dependencies_json: JSON.stringify(["u1"]) }),
    ];
    const vm = buildPixelViewModel("running", dag, tasks);
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.activity).toBe("idle");
  });

  it("maps awaiting_review to attention", () => {
    const tasks = [task("u1", "awaiting_review")];
    const vm = buildPixelViewModel("running", dag, tasks);
    expect(vm.agents[0]?.activity).toBe("attention");
  });

  it("maps assigned reviewer peer to work while task awaits review", () => {
    const tasks = [
      task("u1", "awaiting_review", { id: 1, reviewer_client_uuid: "u2" }),
      task("u2", "completed", { id: 2 }),
    ];
    const vm = buildPixelViewModel("task_review_required", dag, tasks);
    expect(vm.agents.find((a) => a.client_uuid === "u1")?.activity).toBe("attention");
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.activity).toBe("work");
  });

  it("maps approval_required pending to attention", () => {
    const tasks = [task("u1", "pending"), task("u2", "pending")];
    const vm = buildPixelViewModel("approval_required", dag, tasks);
    expect(vm.agents.every((a) => a.activity === "attention")).toBe(true);
  });

  it("forces idle when process is terminal", () => {
    const tasks = [task("u1", "running")];
    const vm = buildPixelViewModel("completed", dag, tasks);
    expect(vm.agents[0]?.activity).toBe("idle");
  });

  it("works with null dag using tasks only", () => {
    const tasks = [task("x", "running")];
    const vm = buildPixelViewModel("running", null, tasks);
    expect(vm.agents).toHaveLength(1);
    expect(vm.agents[0]?.activity).toBe("work");
  });

  it("sets workHint typing when recent events are tool_call only", () => {
    const tasks = [task("u2", "running")];
    const vm = buildPixelViewModel("running", dag, tasks, {
      recentEventTypes: ["tool_call", "status_change"],
    });
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.workHint).toBe("typing");
  });

  it("sets workHint reading when recent events are trace only", () => {
    const tasks = [task("u2", "running")];
    const vm = buildPixelViewModel("running", dag, tasks, {
      recentEventTypes: ["trace", "trace", "status_change"],
    });
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.workHint).toBe("reading");
  });

  it("does not set workHint when activity is not work", () => {
    const tasks = [task("u2", "pending")];
    const vm = buildPixelViewModel("running", dag, tasks, {
      recentEventTypes: ["tool_call"],
    });
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.workHint).toBeUndefined();
  });

  it("prefers per-client workHint over global when both agents work", () => {
    const tasks = [task("u1", "running", { id: 10 }), task("u2", "running", { id: 20 })];
    const vm = buildPixelViewModel("running", dag, tasks, {
      recentEventTypes: ["trace"],
      workHintByClientUuid: { u1: "typing", u2: "reading" },
    });
    expect(vm.agents.find((a) => a.client_uuid === "u1")?.workHint).toBe("typing");
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.workHint).toBe("reading");
  });

  it("sets torsoAccent from team snapshot when role name matches", () => {
    const snapshot = JSON.stringify({
      roster: {
        roles: [
          { name: "Planner" },
          { name: "Coder", accent_color: "#ff5500" },
        ],
      },
    });
    const tasks = [task("u1", "completed"), task("u2", "running")];
    const vm = buildPixelViewModel("running", dag, tasks, undefined, snapshot);
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.torsoAccent).toBe("#ff5500");
    expect(vm.agents.find((a) => a.client_uuid === "u1")?.torsoAccent).toMatch(/^hsl\(/);
  });

  it("uses roster length for agents when snapshot is present (unmatched roles are pending placeholders)", () => {
    const snapshot = JSON.stringify({
      roster: {
        roles: [
          { id: "planner", name: "Planner" },
          { id: "coder", name: "Coder" },
          { id: "qa", name: "QA" },
        ],
      },
    });
    const tasks = [task("u1", "completed"), task("u2", "running")];
    const vm = buildPixelViewModel("running", dag, tasks, undefined, snapshot);
    expect(vm.agents).toHaveLength(3);
    expect(vm.agents[0]?.client_uuid).toBe("u1");
    expect(vm.agents[1]?.client_uuid).toBe("u2");
    expect(vm.agents[2]?.client_uuid).toBe("roster:qa");
    expect(vm.agents[2]?.column).toBe("pending");
    expect(vm.agents[2]?.spriteCharIndex).toBeGreaterThanOrEqual(0);
  });

  it("matches planner role string to roster id when name differs", () => {
    const dagById: PlannerDag = {
      ...dag,
      subagents: [
        dag.subagents[0]!,
        { ...dag.subagents[1]!, role: "coder" },
      ],
    };
    const snapshot = JSON.stringify({
      roster: {
        roles: [{ id: "coder", name: "Implementation specialist", accent_color: "#00aa66" }],
      },
    });
    const tasks = [task("u1", "completed"), task("u2", "running")];
    const vm = buildPixelViewModel("running", dagById, tasks, undefined, snapshot);
    expect(vm.agents.find((a) => a.client_uuid === "u2")?.torsoAccent).toBe("#00aa66");
  });
});

describe("workHintFromTypeList", () => {
  it("matches global heuristics", () => {
    expect(workHintFromTypeList(["tool_call"])).toBe("typing");
    expect(workHintFromTypeList(["trace"])).toBe("reading");
    expect(workHintFromTypeList(["status_change"])).toBeUndefined();
  });
});

function ev(taskId: number, eventType: string, id = 1): EventLogRecord {
  return {
    id,
    process_id: 1,
    task_id: taskId,
    event_type: eventType,
    content: "",
    created_at: "",
  };
}

describe("buildWorkHintByClientUuid", () => {
  it("maps last events per task to client_uuid", () => {
    const tasks = [task("u1", "running", { id: 10 }), task("u2", "running", { id: 20 })];
    const hints = buildWorkHintByClientUuid(
      [ev(10, "status_change", 1), ev(10, "tool_call", 2), ev(20, "trace", 3)],
      tasks,
    );
    expect(hints.u1).toBe("typing");
    expect(hints.u2).toBe("reading");
  });
});
