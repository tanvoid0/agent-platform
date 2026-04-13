/**
 * ADR 0003: lazy-loaded R3F + Three — presentation only; process lifecycle stays on HTTP APIs.
 */
import { Canvas, useFrame } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import { useMemo, useRef } from "react";
import type { Mesh } from "three";

import { useProcessDetailQuery } from "@/hooks/useProcessQueries";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

function statusToColor(status: string | undefined): string {
  if (!status) return "#64748b";
  switch (status) {
    case "completed":
      return "#22c55e";
    case "failed":
      return "#ef4444";
    case "cancelled":
      return "#94a3b8";
    case "approval_required":
    case "task_review_required":
      return "#f59e0b";
    case "pending":
    case "planning":
    case "running":
    case "approved":
      return "#3b82f6";
    default:
      return "#a855f7";
  }
}

function AuthorityBox({ status }: { status: string | undefined }) {
  const ref = useRef<Mesh>(null);
  const color = useMemo(() => statusToColor(status), [status]);

  useFrame((_, delta) => {
    const m = ref.current;
    if (m) m.rotation.y += delta * 0.38;
  });

  return (
    <mesh ref={ref} position={[0, 0.65, 0]} castShadow>
      <boxGeometry args={[1.15, 1.15, 1.15]} />
      <meshStandardMaterial
        color={color}
        metalness={0.22}
        roughness={0.48}
      />
    </mesh>
  );
}

function SpikeScene({ status }: { status: string | undefined }) {
  return (
    <>
      <color attach="background" args={["#0f172a"]} />
      <ambientLight intensity={0.55} />
      <directionalLight castShadow position={[4.5, 7, 5]} intensity={1.15} />
      <mesh receiveShadow rotation={[-Math.PI / 2, 0, 0]} position={[0, 0, 0]}>
        <planeGeometry args={[10, 10]} />
        <meshStandardMaterial color="#1e293b" roughness={0.92} metalness={0.05} />
      </mesh>
      <AuthorityBox status={status} />
      <OrbitControls
        enableDamping
        dampingFactor={0.08}
        minDistance={2.2}
        maxDistance={9}
        maxPolarAngle={Math.PI / 2 - 0.08}
      />
    </>
  );
}

type Props = {
  processId: number | null;
};

export default function SimulationSpike({ processId }: Props) {
  const detail = useProcessDetailQuery(processId);
  const status = detail.data?.process.status;
  const idLabel =
    processId != null ? `Process #${processId}` : "No process loaded";

  return (
    <Card className="mt-2 border-dashed bg-muted/30 shadow-none">
      <CardHeader className="pb-2">
        <CardTitle className="text-base">3D boundary spike (R3F)</CardTitle>
        <p className="text-muted-foreground m-0 text-xs font-normal">
          {idLabel}
          {status != null ? ` · ${status}` : ""}
          {detail.isLoading && processId != null ? " · loading…" : ""}
        </p>
      </CardHeader>
      <CardContent className="space-y-2 pt-0 text-sm text-muted-foreground">
        <p className="m-0 text-xs leading-snug">
          Read-only tint follows{" "}
          <code className="rounded bg-muted px-1 py-0.5 font-mono text-[10px] text-foreground">
            GET /processes/:id
          </code>{" "}
          status. Orchestration and approvals are never driven from this viewport.
        </p>
        <div className="bg-background relative h-[220px] w-full overflow-hidden rounded-md border">
          <Canvas
            shadows
            camera={{ position: [2.9, 2.1, 4.1], fov: 45 }}
            gl={{ antialias: true, alpha: false }}
          >
            <SpikeScene status={status} />
          </Canvas>
        </div>
      </CardContent>
    </Card>
  );
}
