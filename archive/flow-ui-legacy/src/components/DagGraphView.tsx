import {
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Edge,
  type Node,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";

import { parsePlannerDag, shortUuid } from "../api/dag";
import type { TaskNodeRecord } from "../api/types";
import {
  depthBackground,
  lineageLayoutPositions,
  maxDepthForVisibility,
  maxLineageDepth,
  parentHint,
  visibleSubagentUuids,
  type LineageVisibility,
} from "../lib/dagGraphLayout";
import { normalizeTaskStatus, taskDepthByUuid, taskMapByUuid } from "../lib/dagTasks";
import { taskStatusColor } from "../lib/taskStatusVisual";
import { DagSubagentFlowNode } from "./DagSubagentFlowNode";
import { Button } from "@/components/ui/button";

const DAG_NODE_TYPES = { dagSubagent: DagSubagentFlowNode };

function dagToFlow(
  dagJson: string | null,
  tasks: TaskNodeRecord[],
  selectedUuid: string | null,
  lineage: LineageVisibility,
): { nodes: Node[]; edges: Edge[] } {
  const dag = parsePlannerDag(dagJson);
  const taskMap = taskMapByUuid(tasks);
  if (!dag) return { nodes: [], edges: [] };
  const sub = dag.subagents;
  const maxD = maxDepthForVisibility(lineage);
  const visible = visibleSubagentUuids(sub, tasks, maxD);
  const pos = lineageLayoutPositions(sub, tasks, visible);
  const depthByUuid = taskDepthByUuid(tasks);

  const nodes: Node[] = [];
  for (const a of sub) {
    if (!visible.has(a.client_uuid)) continue;
    const task = taskMap.get(a.client_uuid);
    const st = task?.status ?? "pending";
    const selected = selectedUuid === a.client_uuid;
    const depth = depthByUuid.get(a.client_uuid) ?? 0;
    const hint = parentHint(tasks, a.client_uuid);
    const p = pos.get(a.client_uuid) ?? { x: 0, y: 0 };
    const bg = depthBackground(depth);
    const borderColor = selected ? "#1d4ed8" : taskStatusColor(st);
    nodes.push({
      id: a.client_uuid,
      position: p,
      type: "dagSubagent",
      data: {
        role: a.role,
        statusLine: `[${normalizeTaskStatus(st)}]`,
        uuidShort: shortUuid(a.client_uuid),
        parentHint: hint,
        borderColor,
        ...(bg ? { backgroundColor: bg } : {}),
      },
    });
  }

  const edges: Edge[] = [];
  for (const a of sub) {
    if (!visible.has(a.client_uuid)) continue;
    const targetSt = normalizeTaskStatus(taskMap.get(a.client_uuid)?.status ?? "pending");
    for (const dep of a.dependencies ?? []) {
      if (!visible.has(dep)) continue;
      edges.push({
        id: `${dep}->${a.client_uuid}`,
        source: dep,
        target: a.client_uuid,
        type: "smoothstep",
        animated: targetSt === "running",
        style: {
          stroke: targetSt === "running" ? taskStatusColor("running") : "#64748b",
          strokeWidth: targetSt === "running" ? 2 : 1.5,
          opacity: 0.9,
        },
      });
    }
  }
  return { nodes, edges };
}

export function flowSyncSignature(flow: { nodes: Node[]; edges: Edge[] }): string {
  const nodeParts = flow.nodes.map((n) => {
    const data =
      n.type === "dagSubagent" && n.data && typeof n.data === "object"
        ? JSON.stringify(n.data)
        : typeof n.data?.label === "string"
          ? n.data.label
          : String((n.data as { label?: unknown } | undefined)?.label ?? "");
    return {
      id: n.id,
      type: n.type,
      dataKey: data,
    };
  });
  const edgeParts = flow.edges.map((e) => {
    const es = e.style as { stroke?: string } | undefined;
    return {
      id: e.id,
      animated: e.animated,
      stroke: es?.stroke,
    };
  });
  return JSON.stringify({ nodes: nodeParts, edges: edgeParts });
}

function FitViewOnDagKey({
  dagJsonKey,
  visibleNodeCount,
}: {
  dagJsonKey: string;
  visibleNodeCount: number;
}) {
  const { fitView } = useReactFlow();
  const fittedForKeyRef = useRef<string | null>(null);

  useEffect(() => {
    if (visibleNodeCount <= 0) return;
    if (fittedForKeyRef.current === dagJsonKey) return;
    fittedForKeyRef.current = dagJsonKey;
    const id = requestAnimationFrame(() => {
      fitView({ padding: 0.2, duration: 200 });
    });
    return () => cancelAnimationFrame(id);
  }, [dagJsonKey, visibleNodeCount, fitView]);

  return null;
}

const LINEAGE_OPTIONS: { value: LineageVisibility; label: string; short: string }[] = [
  { value: "all", label: "All levels", short: "All" },
  { value: "depth_le_1", label: "Depth ≤ 1", short: "≤1" },
  { value: "roots", label: "Roots only", short: "Roots" },
];

type Props = {
  dagJson: string | null;
  tasks: TaskNodeRecord[];
  selectedUuid: string | null;
  onSelectUuid: (uuid: string | null) => void;
};

export function DagGraphView({ dagJson, tasks, selectedUuid, onSelectUuid }: Props) {
  const [lineage, setLineage] = useState<LineageVisibility>("all");
  const flow = useMemo(
    () => dagToFlow(dagJson, tasks, selectedUuid, lineage),
    [dagJson, tasks, selectedUuid, lineage],
  );
  const taskMap = useMemo(() => taskMapByUuid(tasks), [tasks]);
  const maxDepth = useMemo(() => maxLineageDepth(tasks), [tasks]);
  const [nodes, setNodes, onNodesChange] = useNodesState(flow.nodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(flow.edges);
  const syncSigRef = useRef("");

  useEffect(() => {
    const sig = flowSyncSignature(flow);
    if (sig === syncSigRef.current) return;
    syncSigRef.current = sig;
    setNodes(flow.nodes);
    setEdges(flow.edges);
  }, [flow, setNodes, setEdges]);

  const onNodeClick = useCallback(
    (_event: MouseEvent, node: Node) => {
      onSelectUuid(node.id);
    },
    [onSelectUuid],
  );

  const onPaneClick = useCallback(() => {
    onSelectUuid(null);
  }, [onSelectUuid]);

  const fitKey = `${dagJson ?? ""}|${lineage}|${nodes.length}`;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border/60 bg-muted/20 px-3 py-2">
        <span className="text-muted-foreground text-xs font-medium">Lineage</span>
        <div className="flex flex-wrap gap-1">
          {LINEAGE_OPTIONS.map((opt) => (
            <Button
              key={opt.value}
              type="button"
              size="sm"
              variant={lineage === opt.value ? "default" : "outline"}
              className="h-8"
              title={opt.label}
              aria-pressed={lineage === opt.value}
              onClick={() => setLineage(opt.value)}
            >
              {opt.short}
            </Button>
          ))}
        </div>
        {maxDepth > 0 && (
          <span className="text-muted-foreground max-w-[min(100%,28rem)] text-xs">
            Sub-DAG depth 1–{maxDepth}; primary tint = deeper node.
          </span>
        )}
      </div>

      <div className="h-[calc(100vh-180px)] min-h-[320px] w-full flex-1">
        <ReactFlowProvider>
          <ReactFlow
            className="dag-flow h-full w-full"
            nodeTypes={DAG_NODE_TYPES}
            nodes={nodes}
            edges={edges}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onNodeClick={onNodeClick}
            onPaneClick={onPaneClick}
            nodesDraggable={false}
            nodesConnectable={false}
            elementsSelectable
            defaultEdgeOptions={{ type: "smoothstep" }}
          >
            <FitViewOnDagKey dagJsonKey={fitKey} visibleNodeCount={nodes.length} />
            <Background gap={20} size={1} />
            <MiniMap
              nodeStrokeWidth={2}
              nodeColor={(n) => taskStatusColor(taskMap.get(n.id)?.status)}
              maskColor="var(--dag-minimap-mask)"
            />
            <Controls />
          </ReactFlow>
        </ReactFlowProvider>
      </div>
    </div>
  );
}
