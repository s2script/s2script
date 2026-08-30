/**
 * @s2script/sdkhooks — per-entity SDKHooks (SourceMod `SDKHook` / `SDKUnhook`).
 * NO runtime code: the engine injects the implementation at load time.
 */
import type { EntityRef } from "./entity";
import type { DamageInfo } from "./damage";
import type { HookResultValue } from "./events";

/**
 * Shipped SDKHook types. New members land with engine backing in the same PR — this object is not
 * a catalog of names that throw.
 */
export declare const SDKHookType: {
  /** Pre-apply damage (`DispatchTraceAttack`). Mutate {@link DamageInfo.damage} in place. */
  readonly OnTakeDamage: "OnTakeDamage";
};

/**
 * Hook `entity` for `type`. Auto-unhooked on entity destroy and plugin unload.
 *
 * Not load-window-only — call from `OnEntityCreated`, `OnPluginStart` (ents already live), or any
 * time you hold a live {@link EntityRef}.
 *
 * `OnTakeDamage`: mutate `info.damage` in place. No return needed. Return `HookResult.Handled` to
 * zero the hit; `HookResult.Stop` to also skip later hooks.
 *
 * @returns `true` if the hook was recorded. `false` on `null`/stale `entity` (does not throw).
 * @example
 * import { SDKHook, SDKHookType, Entity } from "@s2script/sdk";
 * import type { EntityRef, DamageInfo } from "@s2script/sdk";
 *
 * export function OnPluginStart(): void {
 *   for (const pawn of Entity.findByClass("player")) {
 *     SDKHook(pawn, SDKHookType.OnTakeDamage, onTakeDamage);
 *   }
 * }
 * export function OnEntityCreated(entity: EntityRef | null, className: string): void {
 *   if (!entity || className !== "player") return;
 *   SDKHook(entity, SDKHookType.OnTakeDamage, onTakeDamage);
 * }
 * function onTakeDamage(info: DamageInfo) {
 *   info.damage /= 2;
 * }
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "OnTakeDamage",
  callback: (info: DamageInfo) => HookResultValue | void,
): boolean;

/**
 * Remove one matching `(entity, type, callback)` hook. Callback identity is the function reference.
 * @returns `true` if an entry was removed.
 */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "OnTakeDamage",
  callback: (info: DamageInfo) => HookResultValue | void,
): boolean;
