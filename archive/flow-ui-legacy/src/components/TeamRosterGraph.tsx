import {
  Background,
  Controls,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { memo, useEffect, useMemo, useRef } from "react";

import type { RosterRole } from "../api/types";
import { primaryLeadRoleId, resolveRoleAccent } from "../lib/teamRosterColors";
import { rosterLineagePositions } from "../lib/teamRosterLayout";
import { RoleAvatar } from "./team/RoleAvatar";

export type TeamRosterNodeData = {
  role: RosterRole;
  accent: string;
  isLead: boolean;
};

function cnRosterNode(selected: boolean, interactive: boolean): string {
  return [
    "team-roster-node",
    "team-roster-node--card",
    selected ? "team-roster-node--selected" : "",
    interactive ? "team-roster-node--interactive" : "",
  ]
    .filter(Boolean)
    .join(" ");
}

/** Match Delegation-style hierarchy: parent bottom → child top (see the-delegation TeamFlowModal / VisualFlowNode). */
const HANDLE_IN = "roster-in";
const HANDLE_OUT = "roster-out";

const TeamRosterRoleNode = memo(function TeamRosterRoleNode({
  data,
}: NodeProps<Node<TeamRosterNodeData>>) {
  const { role, accent, isLead } = data;
  const modality = (role.modality ?? "text").trim() || "text";
  return (
    <div className="relative flex max-w-[min(100%,280px)] min-w-[220px] items-start gap-2.5 px-2.5 py-2">
      <Handle
        type="target"
        position={Position.Top}
        id={HANDLE_IN}
        className="!h-2.5 !w-2.5 !border border-border/60 !bg-background"
        aria-hidden
      />
      <Handle
        type="source"
        position={Position.Bottom}
        id={HANDLE_OUT}
        className="!h-2.5 !w-2.5 !border border-border/60 !bg-background"
        aria-hidden
      />
      <div className="shrink-0 rounded-xl border border-border/60 bg-muted/30 p-0.5">
        <RoleAvatar kind={isLead ? "lead" : "sub"} color={accent} size={44} />
      </div>
      <div className="min-w-0 flex-1 space-y-1">
        <div
          className="truncate text-[12px] font-black leading-tight tracking-tight"
          style={{ color: accent }}
          title={role.name}
        >
          {role.name || "Role"}
        </div>
        <div className="flex flex-wrap gap-1">
          <span
            className="inline-flex h-4 items-center rounded border px-1.5 text-[8px] font-black uppercase leading-none tracking-tight"
            style={{
              backgroundColor: `${accent}20`,
              color: accent,
              borderColor: `${accent}44`,
            }}
          >
            {isLead ? "Lead agent" : "Sub-agent"}
          </span>
        </div>
        <div className="text-[9px] font-medium uppercase tracking-wide text-muted-foreground/90">
          Output · {modality}
        </div>
      </div>
    </div>
  );
});

const nodeTypes = { teamRoster: TeamRosterRoleNode };

function rosterToFlow(
  roles: RosterRole[],
  teamAccent: string,
  highlightRoleId: string | null,
  interactive: boolean,
): { nodes: Node[]; edges: Edge[] } {
  const pos = rosterLineagePositions(roles);
  const byId = new Map(roles.map((r) => [r.id, r]));
  const leadId = primaryLeadRoleId(roles);
  const nodes: Node[] = roles.map((r) => {
    const p = pos.get(r.id) ?? { x: 0, y: 0 };
    const selected = highlightRoleId === r.id;
    const accent = resolveRoleAccent(r, roles, teamAccent);
    const isLead = leadId !== null && r.id === leadId;
    return {
      id: r.id,
      position: p,
      type: "teamRoster",
      data: { role: r, accent, isLead } satisfies TeamRosterNodeData,
      className: cnRosterNode(selected, interactive),
      style: {
        borderWidth: selected ? 3 : 2,
        borderColor: selected ? accent : `${accent}99`,
        backgroundColor: "color-mix(in oklab, var(--card) 94%, transparent)",
        borderRadius: 16,
        padding: 0,
      },
    };
  });

  const edges: Edge[] = [];
  for (const r of roles) {
    const p = r.parent_id;
    if (!p || !byId.has(p)) continue;
    const parentRole = byId.get(p)!;
    const lineColor = resolveRoleAccent(parentRole, roles, teamAccent);
    edges.push({
      id: `${p}->${r.id}`,
      source: p,
      target: r.id,
      sourceHandle: HANDLE_OUT,
      targetHandle: HANDLE_IN,
      type: "smoothstep",
      markerEnd: { type: MarkerType.ArrowClosed, width: 18, height: 18, color: lineColor },
      style: {
        stroke: lineColor,
        strokeWidth: 2,
        strokeDasharray: "5 6",
        opacity: 0.88,
      },
    });
  }
  return { nodes, edges };
}

function FitOnRolesKey({ rolesKey, nodeCount }: { rolesKey: string; nodeCount: number }) {
  const { fitView } = useReactFlow();
  const fittedRef = useRef<string | null>(null);

  useEffect(() => {
    if (nodeCount <= 0) return;
    if (fittedRef.current === rolesKey) return;
    fittedRef.current = rolesKey;
    const id = requestAnimationFrame(() => {
      fitView({ padding: 0.2, duration: 200 });
    });
    return () => cancelAnimationFrame(id);
  }, [rolesKey, nodeCount, fitView]);

  return null;
}

export type TeamRosterGraphProps = {
  roles: RosterRole[];
  /** Team template accent (primary lead inherits when no per-role accent). */
  accentColor: string;
  highlightRoleId?: string | null;
  onRoleClick?: (roleId: string) => void;
  /** Clear selection when clicking the canvas background. */
  onPaneClick?: () => void;
  className?: string;
};

export function TeamRosterGraph({
  roles,
  accentColor,
  highlightRoleId = null,
  onRoleClick,
  onPaneClick,
  className,
}: TeamRosterGraphProps) {
  const interactive = !!onRoleClick;
  const flow = useMemo(
    () => rosterToFlow(roles, accentColor, highlightRoleId, interactive),
    [roles, accentColor, highlightRoleId, interactive],
  );
  const [nodes, setNodes, onNodesChange] = useNodesState(flow.nodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(flow.edges);

  const syncKey = useMemo(() => {
    return JSON.stringify({
      roles: roles.map((r) => ({
        id: r.id,
        name: r.name,
        parent_id: r.parent_id ?? null,
        modality: r.modality ?? "text",
        accent_color: r.accent_color ?? null,
      })),
      accent: accentColor,
      hi: highlightRoleId,
      interactive,
    });
  }, [roles, accentColor, highlightRoleId, interactive]);

  useEffect(() => {
    setNodes(flow.nodes);
    setEdges(flow.edges);
  }, [flow, setNodes, setEdges]);

  if (roles.length === 0) {
    return (
      <div
        className={
          className ??
          "flex min-h-[200px] items-center justify-center rounded-lg border border-dashed border-border/80 bg-muted/15 text-sm text-muted-foreground"
        }
      >
        Add roles to see the roster map.
      </div>
    );
  }

  return (
    <div className={className ?? "min-h-[260px] w-full min-w-0 flex-1"}>
      <ReactFlowProvider>
        <ReactFlow
          className="team-roster-flow h-full w-full rounded-lg border border-border/60 bg-muted/10"
          nodeTypes={nodeTypes}
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable={false}
          panOnScroll
          zoomOnScroll
          defaultEdgeOptions={{ type: "smoothstep" }}
          onNodeClick={(_, node) => onRoleClick?.(node.id)}
          onPaneClick={() => onPaneClick?.()}
        >
          <FitOnRolesKey rolesKey={syncKey} nodeCount={nodes.length} />
          <Background gap={16} size={1} />
          <MiniMap
            nodeStrokeWidth={2}
            nodeColor={(node) => {
              const d = node.data as TeamRosterNodeData | undefined;
              return d?.accent ?? accentColor;
            }}
            maskColor="var(--dag-minimap-mask, rgba(0,0,0,0.08))"
          />
          <Controls showInteractive={false} />
        </ReactFlow>
      </ReactFlowProvider>
    </div>
  );
}
