/// <reference types="vite/client" />

interface Window {
  /**
   * Dev only: world-space position of the meeting desk + chairs `Group` (updated each frame).
   * Tune with `meetingRoom.groupPlacement` in `src/simulation/rooms/meeting/meetingRoom.config.ts` (+X = E, +Z = S).
   */
  __delegationMeetingRoomWorld?: { x: number; y: number; z: number };
  /** Dev only: world-space position of the meeting whiteboard wrapper (updated each frame). */
  __delegationMeetingWhiteboardWorld?: { x: number; y: number; z: number };
}

interface ImportMetaEnv {
  /** Agent Platform origin for fetches in Vite dev (e.g. `http://127.0.0.1:18410`). Same-origin in production. */
  readonly VITE_API_ORIGIN?: string;
  /** Optional wall-clock ms for Agent Platform REST fetches (default 120000). */
  readonly VITE_API_FETCH_TIMEOUT_MS?: string;
  /** Optional wall-clock ms for POST /api/v1/chat (default 600000). */
  readonly VITE_API_CHAT_TIMEOUT_MS?: string;
  /** Must match server `AGENT_PLATFORM_MASTER_KEY` when the API requires Bearer auth. */
  readonly VITE_AGENT_PLATFORM_MASTER_KEY?: string;
  /** Model tag for local-stack chat in dev (proxy alias), e.g. `gemma4`. */
  readonly VITE_OLLAMA_MODEL?: string;
  /** Optional override for LM Studio model id when that backend is active. */
  readonly VITE_LM_STUDIO_MODEL?: string;
}
