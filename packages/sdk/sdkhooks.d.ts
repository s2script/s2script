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
type ThisCallback = (entity: EntityRef) => HookResultValue | void;
type ThisVoidCallback = (entity: EntityRef) => void;
type UseCallback = (
  entity: EntityRef,
  activator: EntityRef | null,
  caller: EntityRef | null,
  type: UseTypeValue,
  value: number,
) => HookResultValue | void;
type UsePostCallback = (
  entity: EntityRef,
  activator: EntityRef | null,
  caller: EntityRef | null,
  type: UseTypeValue,
  value: number,
) => void;

/**
 * `CBaseEntity::Use` use-type (`USE_OFF` / `USE_ON` / `USE_SET` / `USE_TOGGLE`).
 */
export type UseTypeValue = 0 | 1 | 2 | 3;

/**
 * `CBaseEntity::Use` use-type constants (wiki `USE_*`).
 */
export declare const UseType: {
  /** `USE_OFF` — turn off. */
  readonly Off: 0;
  /** `USE_ON` — turn on. */
  readonly On: 1;
  /** `USE_SET` — set to `value`. */
  readonly Set: 2;
  /** `USE_TOGGLE` — toggle. */
  readonly Toggle: 3;
};

/**
 * Shipped SDKHook types. Wiki names without the `SDKHook_` prefix. A wiki name whose engine
 * backing failed or is absent returns `false` from {@link SDKHook} (does not throw). A string
 * that is not a member here throws.
 */
export declare const SDKHookType: {
  /** Pre-apply damage (`DispatchTraceAttack`). Mutate {@link DamageInfo.damage} in place. */
  readonly OnTakeDamage: "OnTakeDamage";
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
  /** `CBaseEntity::Spawn` pre. `Handled` / `Stop` skip the original virtual. */
  readonly Spawn: "Spawn";
  /** `Spawn` post. Return is ignored. */
  readonly SpawnPost: "SpawnPost";
  /** `CBaseEntity::Think` pre. `Handled` / `Stop` skip the original virtual. */
  readonly Think: "Think";
  /** `Think` post. Return is ignored. */
  readonly ThinkPost: "ThinkPost";
  /** Player `PreThink`. Return is ignored. */
  readonly PreThink: "PreThink";
  /** `PreThink` post. Return is ignored. */
  readonly PreThinkPost: "PreThinkPost";
  /** Player `PostThink`. Return is ignored. */
  readonly PostThink: "PostThink";
  /** `PostThink` post. Return is ignored. */
  readonly PostThinkPost: "PostThinkPost";
  /** `CBaseEntity::Use` pre. `Handled` / `Stop` skip the original virtual. */
  readonly Use: "Use";
  /** `Use` post. Return is ignored. */
  readonly UsePost: "UsePost";
  /** `GetMaxHealth`. Mutate `info.maxHealth`. `Handled` / `Stop` SUPERCEDE the original. */
  readonly GetMaxHealth: "GetMaxHealth";
  /** `ShouldCollide`. Return a boolean (not {@link HookResultValue}); last defined wins. */
  readonly ShouldCollide: "ShouldCollide";
  /** `VPhysicsUpdate`. Return is ignored. */
  readonly VPhysicsUpdate: "VPhysicsUpdate";
  /** `VPhysicsUpdate` post. Return is ignored. */
  readonly VPhysicsUpdatePost: "VPhysicsUpdatePost";
  /** `GroundEntChanged` post. Return is ignored. */
  readonly GroundEntChangedPost: "GroundEntChangedPost";
  /** `CanBeAutobalanced`. Return a boolean (not {@link HookResultValue}); last defined wins. */
  readonly CanBeAutobalanced: "CanBeAutobalanced";
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
 * Spawn / Think pre-hook. Callback is `(entity)`. Omit return = Continue.
 * `Handled` / `Stop` SUPERCEDE the original virtual (`Stop` also skips later callbacks).
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "Spawn" | "Think",
  callback: ThisCallback,
): boolean;
/**
 * Void this-only hooks (Posts, PreThink/PostThink, VPhysicsUpdate, GroundEntChangedPost).
 * Return is ignored.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type:
    | "SpawnPost"
    | "ThinkPost"
    | "PreThink"
    | "PreThinkPost"
    | "PostThink"
    | "PostThinkPost"
    | "VPhysicsUpdate"
    | "VPhysicsUpdatePost"
    | "GroundEntChangedPost",
  callback: ThisVoidCallback,
): boolean;
/**
 * `Use` pre-hook. `Handled` / `Stop` SUPERCEDE the original virtual.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "Use",
  callback: UseCallback,
): boolean;
/**
 * `Use` post-hook. Return is ignored.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "UsePost",
  callback: UsePostCallback,
): boolean;
/**
 * `GetMaxHealth`. Mutate `info.maxHealth` in place. `Handled` / `Stop` SUPERCEDE with the new value.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "GetMaxHealth",
  callback: (info: { maxHealth: number }) => HookResultValue | void,
): boolean;
/**
 * `ShouldCollide`. Return a boolean (not HookResult); last defined return wins, default original.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "ShouldCollide",
  callback: (
    entity: EntityRef,
    collisionGroup: number,
    contentsMask: number,
    originalResult: boolean,
  ) => boolean,
): boolean;
/**
 * `CanBeAutobalanced`. Return a boolean (not HookResult); last defined return wins, default original.
 * Callback is skipped when the hooked entity has no {@link Client} — never a raw slot `0`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "CanBeAutobalanced",
  callback: (client: Client, origRet: boolean) => boolean,
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
/** Remove a Spawn / Think pre-hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "Spawn" | "Think",
  callback: ThisCallback,
): boolean;
/** Remove a void this-only hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type:
    | "SpawnPost"
    | "ThinkPost"
    | "PreThink"
    | "PreThinkPost"
    | "PostThink"
    | "PostThinkPost"
    | "VPhysicsUpdate"
    | "VPhysicsUpdatePost"
    | "GroundEntChangedPost",
  callback: ThisVoidCallback,
): boolean;
/** Remove a `Use` pre-hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "Use",
  callback: UseCallback,
): boolean;
/** Remove a `Use` post-hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "UsePost",
  callback: UsePostCallback,
): boolean;
/** Remove a `GetMaxHealth` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "GetMaxHealth",
  callback: (info: { maxHealth: number }) => HookResultValue | void,
): boolean;
/** Remove a `ShouldCollide` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "ShouldCollide",
  callback: (
    entity: EntityRef,
    collisionGroup: number,
    contentsMask: number,
    originalResult: boolean,
  ) => boolean,
): boolean;
/** Remove a `CanBeAutobalanced` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "CanBeAutobalanced",
  callback: (client: Client, origRet: boolean) => boolean,
): boolean;
