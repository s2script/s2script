import type { Hook } from "@s2script/sdk/interfaces";

/**
 * @s2script/mapchooser — the end-of-map vote's state, for plugins that run their own map vote
 * (RTV). Protocol 2. Methods describe CURRENT state for the current map.
 */
export interface Contract {
  methods: {
    /** True while the end-of-map vote is open. */
    isVoteInProgress(): boolean;
    /** True once this map's end-of-map vote has chosen the next map (an Extend win resets it). */
    hasEndOfMapVoteFinished(): boolean;
  };
  forwards: {
    /** Fired before the end-of-map vote opens. Return Handled to postpone it; it retries in about a second. */
    OnMapVoteStart: Hook<{ currentMap: string }>;
  };
}
