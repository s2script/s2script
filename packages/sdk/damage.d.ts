/**
 * @s2script/damage — the borrowed {@link DamageInfo} view of the current damage event.
 * Type-only: the view is supplied by the selected game package (its damage function and damage-info
 * layout are game facts; CS2 maps `CTakeDamageInfo`), through the engine-generic `SDKHook`.
 *
 * Subscribe with `SDKHook` (`SDKHookType.OnTakeDamage` / `OnTakeDamagePost`), not a global mux.
 */
import type { EntityRef } from "./entity";

/**
 * A borrowed view of the current damage event, valid only during the synchronous `OnTakeDamage` or
 * `OnTakeDamagePost` SDKHook callback: reading or writing it afterwards (including after an `await`)
 * throws "expired borrowed view". Returning `HookResult.Handled`/`Stop` from `OnTakeDamage` blocks the
 * hit by setting damage to 0 after every handler ran; the engine's damage function still runs.
 * @example
 * import type { DamageInfo } from "@s2script/sdk";
 * function onTakeDamage(info: DamageInfo) { info.damage = info.damage / 2; }
 */
export interface DamageInfo {
  /**
   * The damage amount (`m_flDamage`). On the pre-hook, assigning MODIFIES the damage the engine
   * applies (later handlers see it); set to 0 to block. A non-finite value is refused (the damage
   * keeps its prior value). On `OnTakeDamagePost`, assignment is ignored (the original already ran).
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
