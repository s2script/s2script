/**
 * Weapon hooks — pickup, switch and drop gates on CCSPlayer_WeaponServices. Hand-written: the views
 * are not the generic number-params shape hookgen emits.
 */
import type { HookResultValue } from "@s2script/sdk/events";
import type { Player } from "./index.d.ts";
import type { Weapon } from "./weapon";

/**
 * Block-scoped view of one pickup or switch check. Valid only during the handler: do not stash it
 * or read it after an `await`.
 */
export interface WeaponGateView {
  /** The player asking, or null when their weapon services matched no live pawn. */
  readonly player: Player | null;
  /** The weapon in question, or null when the engine passed none. */
  readonly weapon: Weapon | null;
  /** Write it to vote. Reads back what this handler wrote, `true` before any write. */
  allowed: boolean;
}

/** Block-scoped view of one weapon drop. Valid only during the handler. */
export interface WeaponDropView {
  /** The player dropping it, or null when their weapon services matched no live pawn. */
  readonly player: Player | null;
  /** The weapon being dropped, or null when the engine passed none. */
  readonly weapon: Weapon | null;
}

/**
 * Weapon hooks. Register during the load window (module top level or `OnPluginStart`); a call after
 * the plugin settles throws. Subscriptions are owned by the calling plugin and torn down with it.
 *
 * `onCanUse` and `onCanSwitchTo` vote like `chat.onCanHear`: `Continue` leaves it to the engine,
 * `Handled`/`Stop` vote the `allowed` the handler wrote (or `false` when it wrote none), and
 * `Changed` votes only when it wrote `allowed`. When anyone voted, the engine's own check is skipped
 * and any `false` vote wins. `Stop` also keeps later handlers from running.
 */
export interface WeaponsApi {
  /**
   * Whether a player may pick up a weapon (CCSPlayer_WeaponServices::CanUse). The engine asks before
   * a pawn touches a weapon on the ground, so a `false` leaves it there.
   * @example
   * import { weapons, HookResult } from "@s2script/cs2";
   * const unarmed = new Set<number>();
   * // Players in `unarmed` can't pick anything up.
   * weapons.onCanUse(v => {
   *   if (v.player && unarmed.has(v.player.slot)) { v.allowed = false; return HookResult.Handled; }
   * });
   */
  onCanUse(handler: (view: WeaponGateView) => HookResultValue | void): void;
  /** Whether a player may switch to a weapon they hold (CCSPlayer_WeaponServices::CanSwitch). */
  onCanSwitchTo(handler: (view: WeaponGateView) => HookResultValue | void): void;
  /**
   * A weapon drop (CCSPlayer_WeaponServices::DropWeapon): the drop key, death and plugin calls all
   * go through it. `Handled` or `Stop` blocks the drop, so the player keeps the weapon.
   */
  onDrop(handler: (view: WeaponDropView) => HookResultValue | void): void;
}

/** Load-window weapon hooks: `import { weapons } from "@s2script/cs2"`. Throws after settle. */
export declare const weapons: WeaponsApi;
