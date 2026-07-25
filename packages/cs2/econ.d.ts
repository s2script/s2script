/**
 * `@s2script/cs2/econ` — the standard contract for a **weapon skin / econ** plugin (CS2).
 *
 * TYPE-ONLY. The framework ships no implementation. Applying skins means driving CS2's economy
 * item model and, for anything inventory-backed, Valve's services — game content, not an engine
 * touchpoint. A community plugin implements this and publishes it. See
 * `@s2script/sdk/contracts/README.md`.
 *
 * This lives in `@s2script/cs2` rather than the SDK because it is CS2-specific: the core is
 * engine-generic and dependencies point one way, game → core.
 *
 * **What the framework already gives an implementer**, so this contract stays thin: the generated
 * schema exposes `fallbackPaintKit`, `fallbackSeed`, `fallbackWear` and `fallbackStatTrak` on the
 * weapon entity, and `@s2script/entity` gives serial-gated `EntityRef`s and item enumeration. An
 * implementation is mostly policy — per-player loadouts, persistence, re-applying on spawn — over
 * primitives that already exist.
 *
 * @example
 * import type { EconService } from "@s2script/cs2/econ";
 * const econ = ctx.tryUse<EconService>("econ");
 * econ?.applySkin(weaponRef, { paintKit: 44, wear: 0.01, statTrak: 1337 });
 */

import type { EntityRef } from "@s2script/sdk/entity";

/**
 * A skin to apply to one weapon. Only `paintKit` is required; an omitted field means "leave as is",
 * which is what lets a caller change wear without knowing the seed.
 */
export interface WeaponSkin {
  /** Paint-kit ID (`m_nFallbackPaintKit`). `0` is the default/unskinned finish. */
  paintKit: number;
  /** Pattern seed (`m_nFallbackSeed`). */
  seed?: number;
  /** Float wear in `[0, 1]` (`m_flFallbackWear`) — 0 is factory new. */
  wear?: number;
  /** StatTrak kill count (`m_nFallbackStatTrak`); omit or `-1` for no StatTrak. */
  statTrak?: number;
  /** Name-tag text; `""` clears it. */
  nameTag?: string;
}

/**
 * A player's chosen skins, keyed by weapon class name (`"weapon_ak47"`).
 * Keyed by class rather than item-definition index so a loadout is readable and diffable.
 */
export type Loadout = Readonly<Record<string, WeaponSkin>>;

/**
 * The contract an econ plugin publishes.
 *
 * Every method degrades rather than throwing: a stale {@link EntityRef}, an unknown class, or an
 * unresolved schema field returns `false`/`null`. Note that CS2 generally requires a weapon to be
 * re-given or the view model refreshed for a change to be visible — an implementation should
 * document its own refresh behaviour.
 */
export interface EconService {
  /** Apply `skin` to one weapon entity. `false` if the ref is stale or the write failed. */
  applySkin(weapon: EntityRef, skin: WeaponSkin): boolean;

  /** Read a weapon's current skin, or `null` if the ref is stale/unreadable. */
  readSkin(weapon: EntityRef): WeaponSkin | null;

  /** Reset a weapon to its default finish. */
  clearSkin(weapon: EntityRef): boolean;

  /** A player's stored loadout, or `null` if they have none. */
  getLoadout(slot: number): Loadout | null;

  /** Replace a player's stored loadout. `false` if the slot has no live client. */
  setLoadout(slot: number, loadout: Loadout): boolean;

  /**
   * Re-apply the player's stored loadout to everything they are currently carrying — the operation
   * a plugin runs on spawn or after a weapon is given. Resolves the number of weapons changed.
   */
  refresh(slot: number): number;
}
