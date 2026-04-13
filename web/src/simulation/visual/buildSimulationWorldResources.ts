import type * as THREE from 'three/webgpu';
import { describeLlmSetup } from '../../core/llm/llmFacade';
import { LlmServerRackVisual } from '../world/LlmServerRackVisual';
import { type LoungeKitchenLayoutConfig, LoungeKitchenVisual } from '../rooms/lounge';
import { MeetingRoomFurnitureVisual, MeetingWhiteboardVisual } from '../rooms/meeting';
import { OfficeReferenceMatVisual } from '../world/OfficeReferenceMatVisual';
import type { PoiManager } from '../world/PoiManager';
import type { SimulationTheme } from './SimulationTheme';
import type { ISimulationWorldResource } from './WorldResource';

export interface SimulationWorldResourceContext {
  scene: THREE.Scene;
  getOffice: () => THREE.Group | null;
  camera: THREE.PerspectiveCamera;
  getWorkClusterAnchor: () => THREE.Vector3 | null;
  getLoungeAnchorWorld: () => THREE.Vector3 | null;
  /** Animation clips baked into `office.glb` (e.g. Blender actions on the AI rack). */
  getOfficeAnimations: () => readonly THREE.AnimationClip[];
  poiManager: PoiManager;
  theme: SimulationTheme;
  /**
   * Overrides {@link LoungeKitchenVisualTheme.layout} from the active theme (defaults in `loungeKitchen.config.ts`)
   * for {@link LoungeKitchenVisual}.
   */
  loungeKitchenLayout?: LoungeKitchenLayoutConfig;
}

/**
 * Factory for procedural world props. Add new `ISimulationWorldResource` implementations here
 * so `SceneManager` stays a thin orchestrator.
 */
export function buildSimulationWorldResources(ctx: SimulationWorldResourceContext): ISimulationWorldResource[] {
  const out: ISimulationWorldResource[] = [];

  out.push(new OfficeReferenceMatVisual(ctx.scene, ctx.getOffice, ctx.theme));

  if (describeLlmSetup().showServerChatHealth) {
    out.push(
      new LlmServerRackVisual(
        ctx.getOffice,
        ctx.camera,
        ctx.getWorkClusterAnchor,
        ctx.getLoungeAnchorWorld,
        ctx.getOfficeAnimations,
        ctx.theme.llmRack
      )
    );
  }

  out.push(
    new LoungeKitchenVisual(
      ctx.scene,
      ctx.getOffice,
      ctx.getLoungeAnchorWorld,
      ctx.getWorkClusterAnchor,
      ctx.poiManager,
      ctx.theme,
      ctx.loungeKitchenLayout ?? ctx.theme.loungeKitchen.layout
    )
  );

  out.push(new MeetingWhiteboardVisual(ctx.getOffice, ctx.theme));

  if (ctx.theme.meetingRoom.useProceduralMeetingFurniture) {
    out.push(new MeetingRoomFurnitureVisual(ctx.scene, ctx.getOffice, ctx.poiManager, ctx.theme));
  }

  return out;
}
