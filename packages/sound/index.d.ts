/** @s2script/sound — engine-generic sound: emit a named SoundEvent + register custom precache paths. */
import type { EntityRef } from "@s2script/entity";

export interface SoundEmitOptions {
  /** Source entity (serial-gated; a stale ref emits nothing and returns 0). Omitted -> worldspawn
   *  (a global/2D sound). */
  entity?: EntityRef;
  /** Recipient player slots. Omitted -> every valid client. Bot slots are always skipped
   *  (no netchannel); an all-bot-skipped set still emits to nobody (a real, safe engine call),
   *  whereas requesting NO recipients (an empty array) returns 0 without emitting. */
  recipients?: number[];
  /** Volume in [0, 1]. Default 1.0 (out-of-range/NaN clamps to 1.0). */
  volume?: number;
}

/** Block-scoped precache context — valid ONLY during the onPrecache dispatch. Synchronous use
 *  only: a stashed context used after the handler returns (or past an await) is a no-op `false`
 *  (the engine manifest is gone). */
export interface PrecacheContext {
  /** Add a resource path (e.g. "soundevents/mypack.vsndevts") to the session resource manifest.
   *  True iff the engine accepted the add. */
  add(path: string): boolean;
}

export declare const Sound: {
  /** Play a named SoundEvent (the engine resolves name->hash; built-in soundevents need no
   *  precache). Returns the engine sound GUID (nonzero) or 0 on failure (unresolved engine
   *  function / stale source entity / an empty `recipients` array). An all-bot-skipped recipient
   *  set still emits to nobody (a real engine call, may return a nonzero GUID). */
  emit(name: string, opts?: SoundEmitOptions): number;
  /** Subscribe to the session resource-manifest build (fires at map load / mapchange). Register
   *  custom .vsndevts/.vsnd content here so a later emit can play it. A plugin hot-loaded mid-map
   *  gets its first fire at the NEXT map change. */
  onPrecache(handler: (ctx: PrecacheContext) => void): void;
};
