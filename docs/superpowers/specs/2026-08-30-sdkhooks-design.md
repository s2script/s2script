# First-party SDKHooks — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/hook-only-events-a8c9`.
**Date:** 2026-08-30.
**Scope:** replace the fake `OnTakeDamage` named public with SourceMod-shaped `SDKHook` / `SDKUnhook`. First type: `OnTakeDamage`. 0.x **minor**.

## Why

`export function OnTakeDamage` is not a SourceMod public. `sourcemod.inc` has no such forward. SDKHooks is `SDKHook(entity, SDKHook_OnTakeDamage, callback)` per entity; the function name is conventional. We wrap a process-wide `DispatchTraceAttack` mux and auto-subscribe every hit. That is the hodgepodge this slice deletes.

`HookResult` already **is** SourceMod’s `Action` (`Continue=0`, `Changed=1`, `Handled=2`, `Stop=3`, same collapse). There is no second `Action` type, and no `export { HookResult as Action }` alias.

## Shape

```ts
import { SDKHook, SDKHookType, HookResult, Entity } from "@s2script/sdk";
import type { EntityRef, DamageInfo, HookResultValue } from "@s2script/sdk";

export function OnPluginStart(): void {
  for (const pawn of Entity.findByClass("player")) {
    SDKHook(pawn, SDKHookType.OnTakeDamage, onTakeDamage);
  }
}

export function OnEntityCreated(entity: EntityRef | null, className: string): void {
  if (!entity || className !== "player") return;
  SDKHook(entity, SDKHookType.OnTakeDamage, onTakeDamage);
}

function onTakeDamage(info: DamageInfo): HookResultValue | void {
  info.damage /= 2;
  return HookResult.Changed;
}
```

Handlers return `HookResultValue | void`, same as `hook.onPre` and command handlers. Omit the annotation, or write that union. Do **not** annotate returns as `typeof HookResult.Changed` (or `typeof HookResult.Handled | …`).

`SDKHook` is **not** load-window-only. You call it from `OnEntityCreated`, `OnClientPutInServer`, `OnPluginStart` (ents already live), or any other time you hold a live `EntityRef`. That is the SDKHooks exception to “register at load.” `hook.on` / `command` stay load-window.

## API

New capability `@s2script/sdk/sdkhooks`, re-exported from the barrel. Prelude injects `SDKHook`, `SDKUnhook`, `SDKHookType` onto `__s2pkg_sdkhooks` (and therefore the barrel).

```ts
export declare const SDKHookType: {
  readonly OnTakeDamage: "OnTakeDamage";
};

export declare function SDKHook(
  entity: EntityRef | null,
  type: "OnTakeDamage",
  callback: (info: DamageInfo) => HookResultValue | void,
): boolean;

export declare function SDKUnhook(
  entity: EntityRef | null,
  type: "OnTakeDamage",
  callback: (info: DamageInfo) => HookResultValue | void,
): boolean;
```

Overloads key on the string literal (`"OnTakeDamage"`), not `typeof SDKHookType.OnTakeDamage`. Call sites still write `SDKHookType.OnTakeDamage`.

- **`SDKHook` returns `boolean`.** `true` if the hook was recorded. `false` on `null`/stale `EntityRef` (does not throw). SourceMod’s `SDKHook` / `SDKHookEx` split is the dual-API we do not copy; one function with the Ex return is enough.
- **`SDKUnhook` returns `boolean`.** `true` if that `(entity, type, callback)` entry was removed. Callback identity is the function reference.
- Multiple hooks on the same entity+type all run (subscribe order). Same as SourceMod.
- Unsupported `type` is a programmer error: throw with a named reason. First slice’s `SDKHookType` contains only shipped members. New members land with their engine backing, overload, dispatch, and tests in the same PR — not a 44-name catalog that all throw.
- Ledgered to the plugin. Entity destroy (books delete) unhooks that identity. Plugin unload unhooks the rest. Authors do not `SDKUnhook` in `OnEntityDestroyed` unless they want to drop a hook early.

## What is deleted

- `export function OnTakeDamage` — the host stops auto-subscribing it.
- `ctx.entities.onDamage` / `Scope.entities.onDamage` — authoring dual of the same global mux.
- Public `Damage.onPre`. `DamageInfo` stays (`@s2script/sdk/damage`). The native subscribe under `SDKHook` is not a plugin-facing API.

## What stays (not SDKHooks)

| Surface | What it is |
|---------|------------|
| `OnEntityCreated` / `OnEntityDestroyed` | SDKHooks **does** ship these as global forwards. Keep as named publics. |
| `OnEntitySpawned` | SM 1.11 `entity.inc` convenience over the entity listener. Keep. Per-entity spawn is `SDKHookType.Spawn` when that type is backed. |
| `hook.on` / `hook.onPre` | Game-event catalog. |
| `onOutput` | SourceMod `HookEntityOutput`. |
| `gameRules.onTerminateRound` (and other gamedata `hooks`) | Declarative inbound detours. |
| `Engine.hook` (`@s2script/sdk/unsafe`) | Plugin-declared inbound detours. |

## Host

Per-plugin table: `(entity books-id, type) → callbacks`. Identity is books-gated (`EntityRef` `{index, id}`), never a raw pointer.

`dispatch_damage` resolves the victim the same way `DamageInfo.victim` does. It fans **only** callbacks whose hooked identity matches that victim, then the existing collapse:

- `Handled` zeroes live `m_flDamage` and does **not** skip other callbacks.
- `Stop` short-circuits.
- `void` = `Continue`.

The process-wide `DispatchTraceAttack` detour stays the engine touchpoint for `OnTakeDamage`. This slice is per-entity **fan-out**, not `SH_ADD_MANUALVPHOOK` on the victim. Think / Touch / SetTransmit wait for real per-entity VP hooks in later slices.

No hook on the victim → JS does not run. Isolate tests that call `dispatch_damage` with no victim op (victim `null`) therefore do not invoke handlers; they seed `entity_live`, `SDKHook` that ref, and a fake `damage_victim` that decodes to it. `S2_DAMAGE_SELFTEST` still proves detour→core (`META_CONPRINTF`); plugin handlers fire only when that synthetic victim is in the plugin’s hook table.

`SDKHook` after settle must succeed (isolate test). `hook.on` after settle still throws.

## In-tree migrations

`OnTakeDamage` exports become `SDKHook` in `OnEntityCreated` plus `Entity.findByClass("player")` in `OnPluginStart` for ents already live:

- `plugins/basecommands`
- `examples/cookbook` (plugin.ts keeps one `onTakeDamage` fold over recipes; that function is the SDKHook callback, not a named public)
- `tools/a4gate`, `tools/detourgate`
- SDK fixtures that exported `OnTakeDamage` (`authoring-command`, `authoring-publics`, `authoring-barrel`)

Migrated handlers use `HookResultValue | void` (cookbook already does). Do not introduce new `typeof HookResult.*` return annotations. Repo-wide cleanup of existing `typeof HookResult` (antiflood, leftover fixtures) is **not** this slice.

Demos hook `"player"` (the pawn that takes bullet damage). Hooking every classname is `OnEntityCreated` with no filter; that is allowed, not required.

## Out of scope

- An `Action` type or `HookResult as Action` alias
- The rest of the SDKHooks catalog (`OnTakeDamagePost`, `Spawn`, Think, Touch, SetTransmit, weapon hooks, …)
- Per-entity vtable / manual VP hooks
- `SDKHookEx` as a second name
- Typed catalog event-name generics
- Engine SUPERCEDE-on-Continue
- Priority on `SDKHook` (subscribe order only)
