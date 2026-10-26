/**
 * @s2script/cs2 — the Weapon entity object (CCSWeaponBase), hand-written on top of the generated field
 * accessors. EntityRef-backed + liveness-gated exactly like Pawn/Player. Re-exported from ./index.
 */
import type { EntityRef } from "@s2script/sdk/entity";
import type { CCSWeaponBase } from "./schema.generated";
import type { Pawn } from "./index";

/**
 * A CS2 weapon entity (CCSWeaponBase). All generated field accessors (clip1, clip2, fallbackPaintKit,
 * ownerEntity, inherited health/teamNum/...) are read+write; a stale weapon reads null.
 */
export interface Weapon extends CCSWeaponBase {
  /** The backing entity ref (escape hatch to the raw EntityRef surface). */
  readonly ref: EntityRef;
  /** Serial-gated liveness. */
  isValid(): boolean;
  /** The weapon skin id (alias of the generated `fallbackPaintKit`). */
  paintKit: number | null;
  /** The holding Pawn (m_hOwnerEntity), or null if unowned (on the ground) / stale. */
  readonly owner: Pawn | null;
  /** The weapon's static VData (max clip, reserve max, price, damage, ...), shared by every weapon of
   *  this class. Null on a stale weapon or when this build's entity layout fails the VData pointer check
   *  (see {@link Weapon.dataUnavailableReason}). Read-only. */
  readonly data: WeaponData | null;
  /** Set the magazine (clip1). `reserve` is accepted but deferred (m_pReserveAmmo layout). false if stale. */
  setAmmo(clip: number, reserve?: number): boolean;
  /** Unequip and schedule destruction through the owner's weapon services; directly remove an unowned
   *  weapon. False if stale or an owned weapon's native service is unavailable. */
  remove(): boolean;
  /**
   * Set or add an econ attribute on this weapon by its definition name, the way skin plugins set
   * paint kit, wear, seed, StatTrak and stickers. The value goes on the item's networked dynamic
   * attribute list (`CAttributeList::SetOrAddAttributeValueByName`).
   *
   * Values are floats. Integer-valued attributes such as `"kill eater"` store the integer's bit
   * pattern in some cases; pass what the attribute definition expects.
   *
   * Returns `false` for a stale weapon, an empty name, a non-finite value, or a server build where
   * the function did not resolve. `true` means the engine was called: it ignores a name it has no
   * definition for. Clients usually see skin changes only after the weapon is re-given or redrawn.
   *
   * @example
   * weapon.setAttribute("set item texture prefab", 44);   // paint kit
   * weapon.setAttribute("set item texture wear", 0.01);
   * weapon.setAttribute("set item texture seed", 661);
   */
  setAttribute(name: string, value: number): boolean;
}

/**
 * A weapon's static VData (CCSWeaponBaseVData), read live through the entity's VData pointer. Every
 * field reads null when the weapon goes stale or the field is missing from this build's schema.
 * "Alt" values are the weapon's second firing mode (scoped, silencer on, or burst).
 * @example
 * const data = pawn.activeWeapon?.data;
 * if (data) chat.reply(`${data.maxClip1} rounds, $${data.price}`);
 */
export interface WeaponData {
  readonly maxClip1: number | null;
  readonly maxClip2: number | null;
  readonly defaultClip1: number | null;
  /** Inventory slot (m_iSlot). */
  readonly slot: number | null;
  /** CSWeaponType: 0 knife, 1 pistol, 2 SMG, 3 rifle, 4 shotgun, 5 sniper, 6 machine gun, 7 C4, 8 taser, 9 grenade, ... */
  readonly weaponType: number | null;
  /** gear_slot_t: 0 rifle, 1 pistol, 2 knife, 3 grenades, 4 C4, ... */
  readonly gearSlot: number | null;
  readonly price: number | null;
  readonly killAward: number | null;
  readonly primaryReserveAmmoMax: number | null;
  readonly secondaryReserveAmmoMax: number | null;
  readonly isFullAuto: boolean | null;
  readonly isMelee: boolean | null;
  /** Pellets per shot (shotguns fire more than one). */
  readonly numBullets: number | null;
  /** Seconds between shots in the primary firing mode. */
  readonly cycleTime: number | null;
  readonly cycleTimeAlt: number | null;
  /** Player move speed while holding the weapon, primary mode. */
  readonly maxSpeed: number | null;
  readonly maxSpeedAlt: number | null;
  readonly damage: number | null;
  readonly headshotMultiplier: number | null;
  readonly armorRatio: number | null;
  readonly range: number | null;
}

/**
 * Entry point for the {@link Weapon} entity object — wrap a raw {@link EntityRef} or enumerate weapons
 * by class name. Prefer {@link Pawn}'s `activeWeapon`/`weapons`/`giveNamedItem` for owned weapons.
 * @example
 * import { Weapon } from "@s2script/cs2";
 * // `ent` is an EntityRef, e.g. from Entity.onSpawn("weapon_*", …)
 * const weapon = Weapon.fromEntity(ent);
 * if (weapon) weapon.setAmmo(90);
 */
export declare const Weapon: {
  /** Wrap a raw weapon EntityRef; null if ref is null. */
  fromEntity(ref: EntityRef | null): Weapon | null;
  /** Every live entity of `className` as a Weapon (e.g. "weapon_ak47"). */
  findAll(className: string): Weapon[];
  /** Why {@link Weapon.data} is null on this build (the entity's VData pointer could not be located from
   *  the schema), or null when it is available. */
  readonly dataUnavailableReason: string | null;
};
