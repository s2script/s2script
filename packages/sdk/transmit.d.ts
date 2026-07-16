/** @s2script/transmit — per-client entity visibility filtering (Source2 CheckTransmit).
 *  Declarative rules: a plugin says "this entity is visible only to these viewer slots" and the
 *  engine-side hook enforces it every snapshot — zero JS runs in the per-client hot path. */
import type { EntityRef } from "./entity";

export interface TransmitStats {
  /** CheckTransmit invocations observed since load/reset. */
  snapshots: number;
  /** Live rule entries in the native table. */
  entries: number;
  /** Total transmit bits cleared since load/reset. */
  bitsCleared: number;
  /** Nanoseconds spent in the post-hook, last invocation. */
  nsLast: number;
  /** Worst single invocation (ns) since load/reset. */
  nsMax: number;
}

export declare const Transmit: {
  /** Replace this plugin's visibility rule for `entity`: it is transmitted ONLY to the given viewer
   *  slots (an empty array = hidden from everyone). Multiple plugins AND-merge — an entity reaches a
   *  viewer only if every plugin holding a rule on it allows that viewer. Returns false if the ref is
   *  stale or the capability is unavailable/disabled. Throws RangeError on a slot outside [0, 64). */
  setVisibleTo(entity: EntityRef, viewers: readonly number[]): boolean;
  /** Remove this plugin's rule for `entity` (visible to all again, as far as this plugin is
   *  concerned). Returns false if the ref is stale or the capability is unavailable. */
  reset(entity: EntityRef): boolean;
  /** Remove all of this plugin's rules. */
  resetAll(): void;
  /** Hot-path counters for measurement/debugging. */
  stats(): TransmitStats;
};
