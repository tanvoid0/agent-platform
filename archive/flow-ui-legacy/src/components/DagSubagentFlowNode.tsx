import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";

export type DagSubagentFlowData = {
  role: string;
  statusLine: string;
  uuidShort: string;
  parentHint: string | null;
  borderColor: string;
  backgroundColor?: string;
};

export function DagSubagentFlowNode({ data, selected }: NodeProps<Node<DagSubagentFlowData>>) {
  const border = selected ? "#1d4ed8" : data.borderColor;
  const width = selected ? 3 : 2;
  return (
    <div
      className={`dag-node max-w-[min(260px,70vw)] min-w-[176px] rounded-lg border-2 bg-card px-2.5 py-2 text-left shadow-sm ${
        selected ? "dag-node--selected" : ""
      }`}
      style={{
        borderColor: border,
        borderWidth: width,
        ...(data.backgroundColor ? { backgroundColor: data.backgroundColor } : {}),
      }}
    >
      <Handle type="target" position={Position.Top} className="!h-2 !w-2 !border-0 !bg-slate-400" />
      <div className="text-foreground font-semibold text-xs leading-snug">{data.role}</div>
      <div className="text-muted-foreground mt-0.5 font-mono text-[10px] leading-none">
        {data.uuidShort}
      </div>
      {data.parentHint ? (
        <div className="text-muted-foreground mt-1 text-[10px] leading-snug">{data.parentHint}</div>
      ) : null}
      <div className="text-muted-foreground mt-1.5 border-border/50 border-t pt-1.5 font-mono text-[10px] leading-snug">
        {data.statusLine}
      </div>
      <Handle
        type="source"
        position={Position.Bottom}
        className="!h-2 !w-2 !border-0 !bg-slate-400"
      />
    </div>
  );
}
