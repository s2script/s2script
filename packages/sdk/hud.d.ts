/**
 * @s2script/sdk/hud — hide parts of a player's native HUD. NO runtime code: the engine injects the
 * implementation at load (`__s2pkg_hud`).
 *
 * Backed by the pawn's `m_iHideHUD` bit field, but owned by the host so plugins never stomp each
 * other: every plugin holds its own bits per player, and the pawn carries the OR of all of them on
 * top of whatever the game itself set. The mask follows the PLAYER, not the pawn: a respawn, a new
 * pawn or the switch to spectating picks it up within a frame, and it is re-applied if the game
 * resets the field. Unload drops only that plugin's bits; a disconnect clears the slot for everyone.
 *
 * What each bit hides is game-specific. CS2's are exported by `@s2script/cs2` as `HideHud`.
 *
 * @example
 * import { Hud } from "@s2script/sdk/hud";
 * import { HideHud } from "@s2script/cs2";
 * // A gamemode with its own radar and round timer.
 * Hud.hide(slot, HideHud.Radar | HideHud.RoundTimer);
 * // ...and give the radar back later. Other plugins' bits are untouched.
 * Hud.show(slot, HideHud.Radar);
 */

/** A player slot (0-based), or anything carrying one (`Client`, `Player`). */
export type HudTarget = number | { readonly slot: number };

export declare const Hud: {
  /** Add `bits` to this plugin's mask for the player. Returns this plugin's bits afterwards.
   *  @throws `RangeError` on a slot outside `[0, 64)`; `TypeError` if `bits` is not a uint32. */
  hide(who: HudTarget, bits: number): number;
  /** Remove `bits` from this plugin's mask. Bits another plugin hides stay hidden. Returns this
   *  plugin's bits afterwards. */
  show(who: HudTarget, bits: number): number;
  /** Replace this plugin's mask for the player (0 = this plugin hides nothing). Returns `bits`. */
  set(who: HudTarget, bits: number): number;
  /** Drop this plugin's mask for the player. */
  reset(who: HudTarget): void;
  /** Drop this plugin's mask for every player. Unload does this automatically. */
  resetAll(): void;
  /** This plugin's bits for the player. */
  hidden(who: HudTarget): number;
  /** Every plugin's bits for the player, OR'd — what s2script puts on the pawn. Bits the game set
   *  itself are not included. */
  hiddenByAll(who: HudTarget): number;
  /** True when every bit in `bits` is hidden by some plugin. False for `bits === 0`. */
  isHidden(who: HudTarget, bits: number): boolean;
};
