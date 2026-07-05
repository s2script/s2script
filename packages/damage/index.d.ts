/**
 * @s2script/damage — damage pre-hooks (SDKHooks-equivalent). NO runtime code (injected at load).
 * CTakeDamageInfo is a Source 2 engine type, so this module is engine-generic (lives in core).
 */
import type { EntityRef } from "@s2script/entity";

/** A block-scoped view of the current damage event (valid only inside a Damage.onPre handler). */
export interface DamageInfo {
  /** The damage amount (m_flDamage). Assigning MODIFIES the live damage; set to 0 to block. */
  damage: number;
  /** The damage-type bit flags (m_bitsDamageType). */
  readonly damageType: number;
  /** The attacking entity (m_hAttacker), or null if none/stale. */
  readonly attacker: EntityRef | null;
  /** The inflicting entity (m_hInflictor), or null if none/stale. */
  readonly inflictor: EntityRef | null;
  /** The victim — the entity taking damage (the hooked `this`), or null if stale. */
  readonly victim: EntityRef | null;
}

export declare const Damage: {
  /** Register a pre-damage hook. The handler reads/modifies the DamageInfo in place (damage=0 blocks). */
  onPre(handler: (info: DamageInfo) => void): void;
};
