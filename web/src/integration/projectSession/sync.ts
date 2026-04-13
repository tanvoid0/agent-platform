import { useCoreStore } from '../store/coreStore';
import { useUiStore } from '../store/uiStore';
import { decodeProjectSessionField, encodeProjectSessionWire } from './codec';
import type { ProjectSessionWire } from './types';

/** Apply `payload.session` after core project fields are written. Always replaces both maps. */
export function applyProjectSessionToStores(sessionRaw: unknown): void {
  const { poses, orchestration } = decodeProjectSessionField(sessionRaw);
  useUiStore.setState({ agentStatuses: orchestration });
  useCoreStore.getState().replaceSessionPosesFromAuthority(poses);
}

export function readProjectSessionFromStores(): ProjectSessionWire {
  const core = useCoreStore.getState();
  const ui = useUiStore.getState();
  return encodeProjectSessionWire({
    poses: { ...core.sessionPoseByAgent },
    orchestration: { ...ui.agentStatuses },
  });
}
