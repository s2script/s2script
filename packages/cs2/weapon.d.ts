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
  /** Set the magazine (clip1). `reserve` is accepted but deferred (m_pReserveAmmo layout). false if stale. */
  setAmmo(clip: number, reserve?: number): boolean;
  /** Unequip from the owner (RemovePlayerItem) + destroy the entity (UTIL_Remove). true iff removed. */
  remove(): boolean;
}

export declare const Weapon: {
  /** Wrap a raw weapon EntityRef; null if ref is null. */
  fromEntity(ref: EntityRef | null): Weapon | null;
  /** Every live entity of `className` as a Weapon (e.g. "weapon_ak47"). */
  findAll(className: string): Weapon[];
};
