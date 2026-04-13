/**
 * Office simulation tuning — NPC autonomy, break behaviors, etc.
 *
 * Edit this file (repo root) instead of scattered magic numbers in drivers.
 */
export const SIMULATION_CONFIG = {
  npc: {
    /**
     * Standing non-lead agents: probability each idle decision to walk to the procedural
     * coffee POI before chair / area wander. 0 = never, 1 = always when the POI is free and path works.
     */
    coffeeBreakChance: 0.9,

    /**
     * When seated (`sit_idle`): probability to stay put and replay seated idle vs standing up
     * and moving on the next idle decision.
     */
    seatedStayChance: 0.1,

    /**
     * Standing: probability (same roll as seated branch above) to seek a random `sit_idle` chair.
     */
    seekChairChance: 0.4,

    /**
     * After coffee / chair branches: probability to wander toward an `area-*` POI.
     */
    wanderAreaChance: 0.7,
  },
} as const;
