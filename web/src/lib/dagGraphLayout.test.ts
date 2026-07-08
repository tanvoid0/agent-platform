import { describe, expect, it } from "vitest";

import {
  lineageLayoutPositions,
  maxDepthForVisibility,
  maxLineageDepth,
  parentHint,
  visibleSubagentUuids,
} from "./dagGraphLayout";
import type { SubagentNode, TaskNodeRecord } from "../api/types";

function task(
  client_uuid: string,
  parent: string | null,
  role: string,
): TaskNodeRecord {
  return {
    id: 1,
    process_id: 1,
    client_uuid,
    parent_client_uuid: parent,
    role,
    system_prompt: "",
    instructions: "",
    llm_model: null,
    dependencies_json: "[]",
    status: "pending",
    output: null,
    tokens_used: 0,
    started_at: null,
    completed_at: null,
  };
}

function sub(id: string, deps: string[] = []): SubagentNode {
  return {
    client_uuid: id,
    role: `R-${id}`,
    system_prompt: "",
    instructions: "",
    dependencies: deps,
  };
}

describe("dagGraphLayout", () => {
  it("caps visibility by max depth", () => {
    const subagents = [sub("a"), sub("b", ["a"])];
    const tasks: TaskNodeRecord[] = [
      task("a", null, "root"),
      task("b", "a", "child"),
    ];
    const all = visibleSubagentUuids(subagents, tasks, null);
    expect(all.has("a") && all.has("b")).toBe(true);
    const roots = visibleSubagentUuids(subagents, tasks, 0);
    expect(roots.has("a")).toBe(true);
    expect(roots.has("b")).toBe(false);
  });

  it("places nodes in depth rows", () => {
    const subagents = [sub("a"), sub("b", ["a"])];
    const tasks: TaskNodeRecord[] = [task("a", null, "root"), task("b", "a", "child")];
    const vis = visibleSubagentUuids(subagents, tasks, null);
    const pos = lineageLayoutPositions(subagents, tasks, vis);
    expect(pos.get("a")!.y).toBe(0);
    expect(pos.get("b")!.y).toBe(130);
  });

  it("parentHint uses parent role when available", () => {
    const tasks: TaskNodeRecord[] = [task("a", null, "Planner"), task("b", "a", "Worker")];
    expect(parentHint(tasks, "b")).toBe("↑ Planner");
  });

  it("maxLineageDepth reads taskDepthByUuid", () => {
    const tasks: TaskNodeRecord[] = [
      task("a", null, "r"),
      task("b", "a", "r"),
      task("c", "b", "r"),
    ];
    expect(maxLineageDepth(tasks)).toBe(2);
  });

  it("maxDepthForVisibility matches presets", () => {
    expect(maxDepthForVisibility("all")).toBeNull();
    expect(maxDepthForVisibility("depth_le_1")).toBe(1);
    expect(maxDepthForVisibility("roots")).toBe(0);
  });
});
