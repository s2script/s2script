/** @s2script/sound — engine-generic sound: emit a named SoundEvent + register custom precache paths. */
import type { EntityRef } from "./entity";

/** Options for {@link Sound.emit} — the source entity, recipient slots, and volume. */
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
  /**
   * Add a resource path to the game session manifest — a soundevents file, a model, a particle.
   *
   * @example
   * pc.add("soundevents/mypack.vsndevts");
   * pc.add("models/props/cs_office/microwave.vmdl");
   *
   * Returns true iff the engine ACCEPTED the string, which is not the same as the resource existing
   * or being loadable: it returns true for a path with no file behind it too. A model that was never
   * really there still spawns as the pink-and-black ERROR box, and the engine only says so later, at
   * spawn time, with "requested but is not in the system (Missing from a manifest?)". Treat a `false`
   * as fatal and a `true` as "not rejected".
   *
   * TIMING: the manifest is built once per map, before plugins that load later exist. A plugin loaded
   * AFTER that point — including on the boot map of a fresh server start — misses that map's
   * precache, and its resources stay unusable until the next map change.
   */
  add(path: string): boolean;
}

/**
 * Engine-generic sound entry point — play a named SoundEvent to some or all clients.
 * @example
 * import { Sound } from "@s2script/sdk/sound";
 * // global 2D broadcast to every valid client (worldspawn source):
 * const guid = Sound.emit("MyPack.Ping");
 * if (guid === 0) console.log("emit failed (unresolved name / stale source)");
 */
export declare const Sound: {
  /** Play a named SoundEvent (the engine resolves name->hash; built-in soundevents need no
   *  precache). Returns the engine sound GUID (nonzero) or 0 on failure (unresolved engine
   *  function / stale source entity / an empty `recipients` array). An all-bot-skipped recipient
   *  set still emits to nobody (a real engine call, may return a nonzero GUID). */
  emit(name: string, opts?: SoundEmitOptions): number;
  /**
   * Stop a named SoundEvent playing on an entity — the counterpart to {@link Sound.emit}.
   *
   * **`opts.entity` is required**, unlike `emit`. The engine call behind this is an instance method
   * on the entity, so there is no global/2D form to default to the way `emit` falls back to
   * worldspawn. `recipients` and `volume` do not apply: the engine stops the sound for everyone
   * hearing it.
   *
   * Equivalent to `entity.stopSound(name)`; this is the discoverable spelling next to `emit`.
   * `pawn.stopSound(name)` in `@s2script/cs2` is the pawn-shaped one.
   *
   * Returns false when `opts.entity` is absent or stale, or the engine op is unavailable.
   */
  stop(name: string, opts: { entity: EntityRef }): boolean;
};
