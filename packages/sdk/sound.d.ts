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
  /** Add a resource path (e.g. "soundevents/mypack.vsndevts") to the session resource manifest.
   *  True iff the engine accepted the add. */
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
/** What an {@link Sound.onEmit} handler receives. */
export interface EmitSoundEvent {
  /** The soundevent name being emitted, e.g. `"Weapon_AK47.Single"`. */
  readonly name: string;
  /** Emitting entity index, or `0` for a global/2D sound (worldspawn). */
  readonly entIndex: number;
  /** Volume in `[0, 1]`. */
  readonly volume: number;
}

export declare const Sound: {
  /** Play a named SoundEvent (the engine resolves name->hash; built-in soundevents need no
   *  precache). Returns the engine sound GUID (nonzero) or 0 on failure (unresolved engine
   *  function / stale source entity / an empty `recipients` array). An all-bot-skipped recipient
   *  set still emits to nobody (a real engine call, may return a nonzero GUID). */
  emit(name: string, opts?: SoundEmitOptions): number;
  /**
   * Intercept sound emission (ModSharp's `OnEmitSound`). `name` is a soundevent name or `"*"`.
   *
   * SYNCHRONOUS and **blockable** — return {@link HookResult.Handled} or `Stop` to suppress the
   * sound entirely; return nothing (or `Continue`) to let it play. A handler that throws is logged
   * and treated as `Continue`, so a bug cannot silence the server.
   *
   * The engine hook is installed on the FIRST subscribe anywhere and never on a server whose
   * plugins do not use it — sound emission is one of the hottest paths in the game.
   *
   * CAVEAT: a sound emitted by {@link Sound.emit} from JS will NOT reach handlers. All JS runs with
   * the isolate borrowed, so the nested dispatch hits the framework's re-entrancy skip; the sound
   * still plays. This only affects plugin-originated sounds, not the game's own.
   *
   * @example
   * import { Sound } from "@s2script/sdk/sound";
   * import { HookResult } from "@s2script/sdk/events";
   * Sound.onEmit("Weapon_AK47.Single", () => HookResult.Handled);   // silence the AK
   */
  onEmit(name: string, handler: (ev: EmitSoundEvent) => number | void): { dispose(): void };
};
