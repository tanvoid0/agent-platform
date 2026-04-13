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
  /** Must match server `AGENT_PLATFORM_API_KEY` when the API requires Bearer auth. */
  readonly VITE_AGENT_PLATFORM_API_KEY?: string;
  /** When `1` or `true`, use cloud (Gemini) for agent chat during `vite` dev (default: server chat via Agent Platform). */
  readonly VITE_USE_GEMINI_IN_DEV?: string;
  /** Google Gemini API key for chat (when backend is Gemini) and cloud media. Not read from browser storage. */
  readonly VITE_GEMINI_API_KEY?: string;
  /** Model tag for local-stack chat in dev (orchestrator alias), e.g. `gemma4`. */
  readonly VITE_OLLAMA_MODEL?: string;
}
