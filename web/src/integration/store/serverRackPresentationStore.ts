import { create } from 'zustand';

/** Logical tiers inside the single embedded rack (Blender decks under {@link TD_DECK_OBJECT_NAMES}). */
export type ServerDeckId = 'llm' | 'backend' | 'database';

export const SERVER_DECK_IDS: ServerDeckId[] = ['llm', 'backend', 'database'];

/**
 * How each deck reads in the scene (emissive on meshes under that deck’s Blender empty).
 * UI / product logic maps real backend state to these modes however you like.
 */
export type ServerDeckVisualMode = 'off' | 'idle' | 'active' | 'alert';

type DeckModeMap = Record<ServerDeckId, ServerDeckVisualMode>;

const defaultDeckModes = (): DeckModeMap => ({
  llm: 'idle',
  backend: 'idle',
  database: 'idle',
});

interface ServerRackPresentationState {
  deckMode: DeckModeMap;
  /**
   * Names of `AnimationClip`s baked into `office.glb` for this rack (e.g. `TD_Rack_FanLoop`).
   * All listed clips are played as looping actions on the rack root via `AnimationMixer`.
   */
  loopingAnimationClipNames: string[];
  setDeckMode: (deck: ServerDeckId, mode: ServerDeckVisualMode) => void;
  setDeckModes: (next: Partial<DeckModeMap>) => void;
  setLoopingAnimationClips: (names: string[]) => void;
  resetPresentation: () => void;
}

export const useServerRackPresentationStore = create<ServerRackPresentationState>()((set) => ({
  deckMode: defaultDeckModes(),
  loopingAnimationClipNames: [],
  setDeckMode: (deck, mode) =>
    set((s) => ({
      deckMode: { ...s.deckMode, [deck]: mode },
    })),
  setDeckModes: (next) =>
    set((s) => ({
      deckMode: { ...s.deckMode, ...next },
    })),
  setLoopingAnimationClips: (names) => set({ loopingAnimationClipNames: [...names] }),
  resetPresentation: () =>
    set({
      deckMode: defaultDeckModes(),
      loopingAnimationClipNames: [],
    }),
}));
