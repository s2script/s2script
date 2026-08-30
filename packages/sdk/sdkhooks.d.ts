/**
 * @s2script/sdkhooks — per-entity SDKHooks (SourceMod `SDKHook` / `SDKUnhook`).
 * NO runtime code: the engine injects the implementation at load time.
 */
import type { EntityRef } from "./entity";
import type { DamageInfo } from "./damage";
import type { HookResultValue } from "./events";
import type { Client } from "./clients";

type TouchCallback = (entity: EntityRef, other: EntityRef | null) => HookResultValue | void;
type TouchPostCallback = (entity: EntityRef, other: EntityRef | null) => void;

/**
 * Shipped SDKHook types. Wiki names without the `SDKHook_` prefix. A wiki name whose engine
 * backing failed or is absent returns `false` from {@link SDKHook} (does not throw). A string
 * that is not a member here throws.
 */
export declare const SDKHookType: {
  /** Pre-apply damage (`DispatchTraceAttack`). Mutate {@link DamageInfo.damage} in place. */
  readonly OnTakeDamage: "OnTakeDamage";
  /**
   * CheckTransmit mux (wiki `SDKHook_SetTransmit`). `Handled` / `Stop` hide this entity from this
   * viewer. AND-merge with `Transmit.setVisibleTo`: either API can hide; SetTransmit cannot
   * un-hide a native mask clear. There is no `SetTransmitPost`.
   */
  readonly SetTransmit: "SetTransmit";
  /** `CBaseEntity::StartTouch` pre. `Handled` / `Stop` skip the original virtual. */
  readonly StartTouch: "StartTouch";
  /** `CBaseEntity::Touch` pre. `Handled` / `Stop` skip the original virtual. */
  readonly Touch: "Touch";
  /** `CBaseEntity::EndTouch` pre. `Handled` / `Stop` skip the original virtual. */
  readonly EndTouch: "EndTouch";
  /** `CBaseEntity::Blocked` pre. `Handled` / `Stop` skip the original virtual. */
  readonly Blocked: "Blocked";
  /** `StartTouch` post. Return is ignored. */
  readonly StartTouchPost: "StartTouchPost";
  /** `Touch` post. Return is ignored. */
  readonly TouchPost: "TouchPost";
  /** `EndTouch` post. Return is ignored. */
  readonly EndTouchPost: "EndTouchPost";
  /** `Blocked` post. Return is ignored. */
  readonly BlockedPost: "BlockedPost";
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
 * Touch-family pre-hook. Callback is `(entity, other)`. Omit return = Continue.
 * `Handled` / `Stop` SUPERCEDE the original virtual (`Stop` also skips later callbacks).
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "StartTouch" | "Touch" | "EndTouch" | "Blocked",
  callback: TouchCallback,
): boolean;
/**
 * Touch-family post-hook. Return is ignored; the original virtual already ran.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "StartTouchPost" | "TouchPost" | "EndTouchPost" | "BlockedPost",
  callback: TouchPostCallback,
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
/** Remove a Touch-family pre-hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "StartTouch" | "Touch" | "EndTouch" | "Blocked",
  callback: TouchCallback,
): boolean;
/** Remove a Touch-family post-hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "StartTouchPost" | "TouchPost" | "EndTouchPost" | "BlockedPost",
  callback: TouchPostCallback,
): boolean;

/**
 * SetTransmit. Callback is `(entity, client)`. Omit return = Continue.
 * `Handled` / `Stop` hide this entity from this viewer (AND-merge with `Transmit.setVisibleTo`;
 * cannot un-hide a native mask clear). `Stop` also skips later SetTransmit callbacks on that pair.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "SetTransmit",
  callback: (entity: EntityRef, client: Client) => HookResultValue | void,
): boolean;
/** Remove a SetTransmit hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "SetTransmit",
  callback: (entity: EntityRef, client: Client) => HookResultValue | void,
): boolean;
