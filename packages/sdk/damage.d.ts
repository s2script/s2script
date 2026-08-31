/**
 * @s2script/damage — the block-scoped {@link DamageInfo} view of a live `CTakeDamageInfo`.
 * NO runtime code (injected at load). Engine-generic: `CTakeDamageInfo` is a Source 2 type.
 *
 * Subscribe with `SDKHook` (`SDKHookType.OnTakeDamage` / `OnTakeDamagePost`), not a global mux.
 */
import type { EntityRef } from "./entity";

/**
 * A block-scoped view of the current damage event (valid only inside an `OnTakeDamage` or
 * `OnTakeDamagePost` SDKHook).
 * @example
 * import type { DamageInfo } from "@s2script/sdk";
 * function onTakeDamage(info: DamageInfo) { info.damage = info.damage / 2; }
 */
export interface DamageInfo {
  /**
   * The damage amount (`m_flDamage`). On the pre-hook, assigning MODIFIES the live damage; set to
   * 0 to block. On `OnTakeDamagePost`, assignment is ignored (the original already ran).
   */
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
