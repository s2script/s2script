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
type WeaponCallback = (entity: EntityRef, weapon: EntityRef | null) => HookResultValue | void;
type WeaponPostCallback = (entity: EntityRef, weapon: EntityRef | null) => void;
type ThisCallback = (entity: EntityRef) => HookResultValue | void;
type ThisVoidCallback = (entity: EntityRef) => void;
type FireBulletsPostCallback = (entity: EntityRef, shots: number, weaponName: string) => void;
type ReloadPostCallback = (weapon: EntityRef, successful: boolean) => void;
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
  /**
   * Post-apply damage (`DispatchTraceAttack` after the original). {@link DamageInfo} is read-only
   * for effect; mutations are not written back. Return is ignored.
   */
  readonly OnTakeDamagePost: "OnTakeDamagePost";
  /**
   * `OnTakeDamageAlive` pre. CS2 has no distinct function yet — {@link SDKHook} returns `false`.
   */
  readonly OnTakeDamageAlive: "OnTakeDamageAlive";
  /**
   * `OnTakeDamageAlive` post. CS2 has no distinct function yet — {@link SDKHook} returns `false`.
   */
  readonly OnTakeDamageAlivePost: "OnTakeDamageAlivePost";
  /**
   * `TraceAttack` pre ({@link TraceAttackInfo}). No distinct CS2 virtual yet — {@link SDKHook}
   * returns `false` (OnTakeDamage remains the DTA mux).
   */
  readonly TraceAttack: "TraceAttack";
  /**
   * `TraceAttack` post. No distinct CS2 virtual yet — {@link SDKHook} returns `false`.
   */
  readonly TraceAttackPost: "TraceAttackPost";
  /**
   * `FireBulletsPost`. Wiki already documents this as often absent. {@link SDKHook} returns `false`
   * until a live path is found.
   */
  readonly FireBulletsPost: "FireBulletsPost";
  /** Weapon reload pre. `Handled` / `Stop` skip the original virtual. Missing VP → `false`. */
  readonly Reload: "Reload";
  /** Weapon reload post. Callback is `(weapon, successful)`. Missing VP → `false`. */
  readonly ReloadPost: "ReloadPost";
  /** `WeaponCanUse` pre. Callback is `(entity, weapon)`. Missing ItemServices VP → `false`. */
  readonly WeaponCanUse: "WeaponCanUse";
  /** `WeaponCanUse` post. Return is ignored. Missing VP → `false`. */
  readonly WeaponCanUsePost: "WeaponCanUsePost";
  /** `WeaponCanSwitchTo` pre. Callback is `(entity, weapon)`. Missing VP → `false`. */
  readonly WeaponCanSwitchTo: "WeaponCanSwitchTo";
  /** `WeaponCanSwitchTo` post. Return is ignored. Missing VP → `false`. */
  readonly WeaponCanSwitchToPost: "WeaponCanSwitchToPost";
  /** `WeaponDrop` pre. Callback is `(entity, weapon)`. Missing VP → `false`. */
  readonly WeaponDrop: "WeaponDrop";
  /** `WeaponDrop` post. Return is ignored. Missing VP → `false`. */
  readonly WeaponDropPost: "WeaponDropPost";
  /** `WeaponEquip` pre. Callback is `(entity, weapon)`. Missing VP → `false`. */
  readonly WeaponEquip: "WeaponEquip";
  /** `WeaponEquip` post. Return is ignored. Missing VP → `false`. */
  readonly WeaponEquipPost: "WeaponEquipPost";
  /** `WeaponSwitch` pre. Callback is `(entity, weapon)`. Missing VP → `false`. */
  readonly WeaponSwitch: "WeaponSwitch";
  /** `WeaponSwitch` post. Return is ignored. Missing VP → `false`. */
  readonly WeaponSwitchPost: "WeaponSwitchPost";
};

/**
 * Block-scoped view of a `TraceAttack` callback. Mutate {@link TraceAttackInfo.damage} on the
 * pre-hook. Valid only during the handler.
 */
export interface TraceAttackInfo {
  /** Incoming damage. Mutate in place on the pre-hook. */
  damage: number;
  /** Damage type bits. */
  readonly damageType: number;
  /** Ammo type index. */
  readonly ammoType: number;
  /** Hitbox index. */
  readonly hitbox: number;
  /** Hitgroup index. */
  readonly hitgroup: number;
  /** Attacking entity, books-gated. */
  readonly attacker: EntityRef | null;
  /** Inflicting entity, books-gated. */
  readonly inflictor: EntityRef | null;
  /** Victim entity, books-gated. */
  readonly victim: EntityRef | null;
}

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
 * Post-apply damage. {@link DamageInfo} is a read of the live info after the original ran.
 * Return is ignored; Handled does not zero the hit (that is the pre-hook).
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "OnTakeDamagePost",
  callback: (info: DamageInfo) => void,
): boolean;
/**
 * `OnTakeDamageAlive` pre. No CS2 backing yet — always returns `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "OnTakeDamageAlive",
  callback: (info: DamageInfo) => HookResultValue | void,
): boolean;
/**
 * `OnTakeDamageAlive` post. No CS2 backing yet — always returns `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "OnTakeDamageAlivePost",
  callback: (info: DamageInfo) => void,
): boolean;
/**
 * `TraceAttack` pre. No distinct CS2 virtual yet — always returns `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "TraceAttack",
  callback: (info: TraceAttackInfo) => HookResultValue | void,
): boolean;
/**
 * `TraceAttack` post. No distinct CS2 virtual yet — always returns `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "TraceAttackPost",
  callback: (info: TraceAttackInfo) => void,
): boolean;
/**
 * `FireBulletsPost`. No CS2 backing yet — always returns `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "FireBulletsPost",
  callback: FireBulletsPostCallback,
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
  type: "Spawn" | "Think" | "Reload",
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
 * SetTransmit. Callback is `(entity, client)`. Omit return = Continue.
 * `Handled` / `Stop` hide this entity from this viewer (AND-merge with `Transmit.setVisibleTo`;
 * cannot un-hide a native mask clear). `Stop` also skips later SetTransmit callbacks on that pair.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "SetTransmit",
  callback: (entity: EntityRef, client: Client) => HookResultValue | void,
): boolean;
/**
 * Weapon-family pre-hook. Callback is `(entity, weapon)`. Omit return = Continue.
 * `Handled` / `Stop` SUPERCEDE the original virtual. Missing ItemServices VP → `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "WeaponCanUse" | "WeaponCanSwitchTo" | "WeaponDrop" | "WeaponEquip" | "WeaponSwitch",
  callback: WeaponCallback,
): boolean;
/**
 * Weapon-family post-hook. Return is ignored. Missing VP → `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type:
    | "WeaponCanUsePost"
    | "WeaponCanSwitchToPost"
    | "WeaponDropPost"
    | "WeaponEquipPost"
    | "WeaponSwitchPost",
  callback: WeaponPostCallback,
): boolean;
/**
 * `ReloadPost`. Callback is `(weapon, successful)`. Missing VP → `false`.
 */
export declare function SDKHook(
  entity: EntityRef | null,
  type: "ReloadPost",
  callback: ReloadPostCallback,
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
/** Remove an `OnTakeDamagePost` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "OnTakeDamagePost",
  callback: (info: DamageInfo) => void,
): boolean;
/** Remove an `OnTakeDamageAlive` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "OnTakeDamageAlive",
  callback: (info: DamageInfo) => HookResultValue | void,
): boolean;
/** Remove an `OnTakeDamageAlivePost` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "OnTakeDamageAlivePost",
  callback: (info: DamageInfo) => void,
): boolean;
/** Remove a `TraceAttack` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "TraceAttack",
  callback: (info: TraceAttackInfo) => HookResultValue | void,
): boolean;
/** Remove a `TraceAttackPost` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "TraceAttackPost",
  callback: (info: TraceAttackInfo) => void,
): boolean;
/** Remove a `FireBulletsPost` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "FireBulletsPost",
  callback: FireBulletsPostCallback,
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
  type: "Spawn" | "Think" | "Reload",
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
/** Remove a SetTransmit hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "SetTransmit",
  callback: (entity: EntityRef, client: Client) => HookResultValue | void,
): boolean;
/** Remove a Weapon-family pre-hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "WeaponCanUse" | "WeaponCanSwitchTo" | "WeaponDrop" | "WeaponEquip" | "WeaponSwitch",
  callback: WeaponCallback,
): boolean;
/** Remove a Weapon-family post-hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type:
    | "WeaponCanUsePost"
    | "WeaponCanSwitchToPost"
    | "WeaponDropPost"
    | "WeaponEquipPost"
    | "WeaponSwitchPost",
  callback: WeaponPostCallback,
): boolean;
/** Remove a `ReloadPost` hook. */
export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "ReloadPost",
  callback: ReloadPostCallback,
): boolean;
