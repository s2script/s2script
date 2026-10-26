/** @s2script/sound — engine-generic sound: emit a named SoundEvent + register custom precache paths. */
import type { EntityRef } from "./entity";
import type { Vector } from "./math";

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

/** Recipients for {@link SoundEvent.setParam} and {@link SoundEvent.stop}. */
export interface SoundEventRecipients {
  /** Player slots the change goes to. Omitted -> every valid client. Bots are skipped. */
  recipients?: number[];
}

/**
 * A sound started with {@link Sound.start}: the engine's event guid and stack hash. It is plain
 * data, so it is safe to keep. Once the sound has finished, `setParam` and `stop` still return true
 * but the engine ignores them.
 */
export interface SoundEvent {
  /** The engine sound event guid, the same number {@link Sound.emit} returns. */
  readonly guid: number;
  /** The event's stack hash, which the engine needs alongside the guid to set a parameter. */
  readonly stackHash: number;
  /**
   * Set a soundevent parameter on the playing sound, for example `"volume"`, `"pitch"` or
   * `"position"`, or one of the event's own `public.*` fields. A number sets a float field and a
   * Vector sets a three-float field.
   *
   * @example
   * const ev = Sound.start("MyPack.Siren", { entity: pawn.ref });
   * ev?.setParam("volume", 0.4);
   * ev?.setParam("position", pawn.origin);
   *
   * Returns true when the engine was called. The engine does not report whether the event has a
   * parameter by that name, and it drops a write to one that doesn't exist. Returns false for any
   * other value type, a non-finite number, or when the engine function did not resolve.
   */
  setParam(name: string, value: number | Vector, opts?: SoundEventRecipients): boolean;
  /** Stop this sound for the recipients. Returns false only when the engine function did not
   *  resolve. */
  stop(opts?: SoundEventRecipients): boolean;
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
  /** {@link Sound.emit}, keeping a handle so you can change the sound's parameters while it plays
   *  or stop that one instance. Returns null where `emit` would return 0. */
  start(name: string, opts?: SoundEmitOptions): SoundEvent | null;
  /**
   * The length of a sound event in seconds, as the engine's sound event manager reports it.
   *
   * @example
   * const len = Sound.duration("MyPack.RoundMusic");
   * if (len !== null) delay(len * 1000).then(() => Sound.emit("MyPack.RoundMusicEnd"));
   *
   * Returns null when the engine has no event by that name (check with {@link Sound.isValid}), or
   * when the engine function did not resolve.
   */
  duration(name: string): number | null;
  /** Whether the engine knows a sound event by that name: a loaded soundevents file defines it.
   *  Returns false when the engine function did not resolve. */
  isValid(name: string): boolean;
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
