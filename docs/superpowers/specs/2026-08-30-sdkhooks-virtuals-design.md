# SDKHooks virtuals + natives — design spec

**Status:** draft for review. Stacks on the shipped `OnTakeDamage` SDKHook
(`docs/superpowers/specs/2026-08-30-sdkhooks-design.md`, PR #134).
**Date:** 2026-08-30.
**Catalog:** [SDKHooks wiki](https://wiki.alliedmods.net/SDKHooks) + `sdkhooks.inc`. Do not skip a wiki
type for taste. 0.x **minor**.

This spec does **not** add DHooks (`DHookAddParam`, plugin-authored register layouts). Inbound detours
already exist (`Engine.hook` / game-package `hooks`). This work is the rest of SDKHooks.

## Why

PR #134 replaced the fake `OnTakeDamage` public with per-entity `SDKHook` / `SDKUnhook`. The engine
touchpoint for that one type is still a process-wide `DispatchTraceAttack` mux. The wiki catalog is
mostly **per-entity virtuals** (SourceMod `SH_ADD_MANUALHOOK`), plus two natives
(`SDKHooks_TakeDamage`, `SDKHooks_DropWeapon`) that are not methods on `Pawn`.

Authors porting SM plugins write the wiki names. A type CS2 does not implement **degrades by name**
(wiki already says this for `FireBulletsPost`). `Transmit.setVisibleTo` is not a substitute for
`SDKHook_SetTransmit`.

## Locked authoring surface

Every wiki **hook type** — including every `Action` callback (`Think`, `Spawn`, `Touch`, `SetTransmit`,
`Use`, weapon hooks, remaining damage types, …) — is still the same three-arg register call as
OnTakeDamage. The callback is the third argument. Wiki `Action` is `HookResult`. Omit the return
(`void` = `Continue`); return `HookResult.Handled` / `Stop` to skip the original virtual (and, for
`Stop`, later callbacks).

```ts
import { SDKHook, SDKHookType, HookResult } from "@s2script/sdk";
import type { EntityRef, DamageInfo } from "@s2script/sdk";

export function OnEntityCreated(entity: EntityRef | null, className: string): void {
  if (!entity) return;
  SDKHook(entity, SDKHookType.Think, onThink);
  SDKHook(entity, SDKHookType.OnTakeDamage, onTakeDamage);
}

function onThink(entity: EntityRef) {
  // wiki: Action (int entity). Same register shape as OnTakeDamage.
  if (/* skip this entity's Think this tick */) return HookResult.Handled;
}

function onTakeDamage(info: DamageInfo) {
  info.damage /= 2;
}
```

Not `SDKHooks.think()`. Not a named public `OnThink`. `SDKHookType.Think` is the wiki name without the
`SDKHook_` prefix.

`SDKHook` / `SDKUnhook` / `SDKHookType` stay as shipped. No `Action` type. No `SDKHookEx` name. Boolean
return is the Ex semantic. Multiple callbacks per `(entity, type)` still run (subscribe order) — we do
not copy SM’s “same hook twice is blocked.”

### Natives are a namespace, not `SDKHooks_*`

`SDKHooks.takeDamage` / `SDKHooks.dropWeapon` are **not** hooks. They are SourceMod’s two underscore
**natives** (`SDKHooks_TakeDamage`, `SDKHooks_DropWeapon`) — functions that apply damage / drop a
weapon. The `.` is only this namespace (`Transmit.setVisibleTo` / `Entity.findByClass` pattern). Hook
registration never moves onto it (`SDKHooks.think` / `SDKHooks.hook` do not exist).

```ts
SDKHooks.takeDamage(victim, inflictor, attacker, 50);
SDKHooks.dropWeapon(client, weapon);
```

Not `SDKHooks_TakeDamage`. Not `takeDamage` as a free function. Not methods on `Pawn`.

`bypassHooks` defaults **true** (SM). When true, that invocation does not re-enter the matching
SDKHook types (TakeDamage family / WeaponDrop). Implementation is a thread-local latch, same idea as
`terminateRound`’s `bypassWith`.

Both natives return `boolean`: `true` if the engine call ran. `false` on `null`/stale refs, missing
gamedata, or (drop) weapon not owned by that client. They do not throw.

```ts
export declare const SDKHooks: {
  takeDamage(
    entity: EntityRef | null,
    inflictor: EntityRef | null,
    attacker: EntityRef | null,
    damage: number,
    damageType?: number,           // default 0 = DMG_GENERIC
    weapon?: EntityRef | null,
    damageForce?: Vector | null,
    damagePosition?: Vector | null,
    bypassHooks?: boolean,         // default true
  ): boolean;

  dropWeapon(
    client: Client | null,
    weapon: EntityRef | null,
    target?: Vector | null,
    velocity?: Vector | null,
    bypassHooks?: boolean,         // default true
  ): boolean;
};
```

`pawn.dropActiveWeapon()` stays the deferred stub in `@s2script/cs2`. It is a no-arg convenience that
needs its own DropWeapon sig-resolve; this native is the SM-shaped `(client, weapon, …)` call and
lives here so that concern stays off `Pawn`.

`sm_slap` still does not go through TakeDamage (wiki). Slap stays a health write.

## Transmit

Wiki transmit hook: **`SDKHook_SetTransmit` only**. There is no `SetTransmitPost`. Do not invent one.

```ts
SDKHook(entity, SDKHookType.SetTransmit, (entity, client) => {
  if (/* this viewer should not see this entity */) return HookResult.Handled;
});
```

Callback: `(entity: EntityRef, client: Client) => HookResultValue | void`. `Handled` / `Stop` = this
viewer does **not** get this entity. `Stop` also skips later SetTransmit callbacks on that pair.
`void` = `Continue`.

**CS2 engine path is `ISource2GameEntities::CheckTransmit`**, already hooked POST for
`@s2script/sdk/transmit`. CS2 does not expose a useful per-entity `CBaseEntity::SetTransmit` virtual
the way Source 1 did. The wiki callback stays; the mux is CheckTransmit, same pattern as
OnTakeDamage + `DispatchTraceAttack`.

**Composition with `Transmit.setVisibleTo`:** AND-merge. An entity reaches viewer *v* only if every
suppressor allows it. `Handled` cannot force-show an entity `Transmit.setVisibleTo` already hid.
`Transmit.setVisibleTo` cannot un-hide a SetTransmit `Handled`. Either API is sufficient to hide;
neither is a substitute for the other.

**Hot path:** CheckTransmit runs per snapshot. JS runs **only** for entities that have at least one
SetTransmit hook, and only for viewers whose bit is still set after the native Transmit pass. Empty
SetTransmit table → zero JS (today’s `Transmit.setVisibleTo` cost). Authors with a static viewer mask
should keep using `Transmit.setVisibleTo`. SetTransmit is for logic that cannot be a stored mask.

## Wiki catalog

`SDKHookType` members are wiki names **without** the `SDKHook_` prefix (`SetTransmit`, not
`SDKHook_SetTransmit`). Overloads key on the string literal. New members land with attempted engine
backing in the same PR as the type; a type we cannot find on CS2 still gets the member so a port
typechecks, and `SDKHook` returns `false` (named degrade in the boot banner / first call). A type
string that is not a wiki name still throws (programmer error). A wiki name whose descriptor failed
at load returns `false` and does not throw.

`OnEntityCreated` / `OnEntityDestroyed` stay named publics. They are not `SDKHookType` members.
`OnEntitySpawned` stays; per-entity spawn is `SDKHookType.Spawn`.

| Type | Callback | Return |
|------|----------|--------|
| `OnTakeDamage` | `(info: DamageInfo)` | already shipped |
| `OnTakeDamagePost` | `(info: DamageInfo)` | `void` (info read-only) |
| `OnTakeDamageAlive` | `(info: DamageInfo)` | `HookResultValue \| void` |
| `OnTakeDamageAlivePost` | `(info: DamageInfo)` | `void` |
| `TraceAttack` | `(info: TraceAttackInfo)` | `HookResultValue \| void` |
| `TraceAttackPost` | `(info: TraceAttackInfo)` | `void` |
| `FireBulletsPost` | `(entity, shots: number, weaponName: string)` | `void` |
| `Spawn` | `(entity)` | `HookResultValue \| void` |
| `SpawnPost` | `(entity)` | `void` |
| `Think` | `(entity)` | `HookResultValue \| void` |
| `ThinkPost` | `(entity)` | `void` |
| `PreThink` | `(entity)` | `void` |
| `PreThinkPost` | `(entity)` | `void` |
| `PostThink` | `(entity)` | `void` |
| `PostThinkPost` | `(entity)` | `void` |
| `StartTouch` / `Touch` / `EndTouch` / `Blocked` | `(entity, other: EntityRef \| null)` | `HookResultValue \| void` |
| `StartTouchPost` / `TouchPost` / `EndTouchPost` / `BlockedPost` | `(entity, other: EntityRef \| null)` | `void` |
| `SetTransmit` | `(entity, client: Client)` | `HookResultValue \| void` |
| `ShouldCollide` | `(entity, collisionGroup: number, contentsMask: number, originalResult: boolean)` | `boolean` |
| `GetMaxHealth` | `(info: { maxHealth: number })` | `HookResultValue \| void` (mutate `maxHealth`) |
| `Use` | `(entity, activator, caller, type: UseTypeValue, value: number)` | `HookResultValue \| void` |
| `UsePost` | same args | `void` |
| `Reload` | `(weapon)` | `HookResultValue \| void` |
| `ReloadPost` | `(weapon, successful: boolean)` | `void` |
| `VPhysicsUpdate` / `VPhysicsUpdatePost` | `(entity)` | `void` |
| `GroundEntChangedPost` | `(entity)` | `void` |
| `WeaponCanUse` / `WeaponCanSwitchTo` / `WeaponDrop` / `WeaponEquip` / `WeaponSwitch` | `(entity, weapon: EntityRef \| null)` | `HookResultValue \| void` |
| matching `*Post` | same args | `void` |
| `CanBeAutobalanced` | `(client: Client, origRet: boolean)` | `boolean` |

`entity` in every callback is the hooked `EntityRef`. By-ref SM cells become a mutated view (`DamageInfo`,
`{ maxHealth }`, `TraceAttackInfo`). Post hooks are `void`. `ShouldCollide` / `CanBeAutobalanced` return
`boolean` (wiki), not `HookResult`.

```ts
export declare const UseType: {
  readonly Off: 0;
  readonly On: 1;
  readonly Set: 2;
  readonly Toggle: 3;
};
```

```ts
export interface TraceAttackInfo {
  damage: number;                    // mutate in place on the pre-hook
  readonly damageType: number;
  readonly ammoType: number;
  readonly hitbox: number;
  readonly hitgroup: number;
  readonly attacker: EntityRef | null;
  readonly inflictor: EntityRef | null;
  readonly victim: EntityRef | null;
}
```

`CanBeAutobalanced` / player-think types fire only when the hooked `EntityRef` resolves to a
`Client`. Otherwise `SDKHook` may still record (the instance has the virtual) but the callback is
skipped when there is no client — never a raw slot `0`.

## Host mechanism

Three install styles, chosen per type — not one global detour for Think/Touch (too hot):

1. **Already shipped.** `OnTakeDamage` → process-wide `DispatchTraceAttack` + per-entity table.
2. **SetTransmit.** Same CheckTransmit POST as `Transmit.setVisibleTo`. Core holds the SetTransmit
   callback table; shim clears bits after the native Transmit pass. No extra SourceHook.
3. **Everything else.** Per-entity SourceHook **manual VP hook** on the instance (`SH_ADD_MANUALHOOK`),
   lazy on the first `SDKHook` of that type on that entity, removed when the last callback for that
   `(entity, type)` drops (destroy, `SDKUnhook`, plugin unload). The shim owns the C++ thunk; core owns
   the JS table already in `sdkhooks.rs`.

Vtable indices are **derived on our `libserver.so` at load** (`docs/re-strategy.md` Rule 1): find the
function by signature, find the class vtable via RTTI, scan for the pointer, **cache the slot in
memory**. The committed gamedata is the signature + `vtable-member` class, never the slot number. A
borrowed SM/CSSharp slot is a HINT for which function to look at offline, never a shipped fact.

A missing or failed signature **disables that `SDKHookType` only**. `SDKHook` returns `false`. The
rest of the catalog keeps running.

Weapon\* / `Reload` / `CanBeAutobalanced` may have moved to item services or disappeared on CS2. If the
virtual is not on the hooked instance’s class, that type degrades by name. Authors still write
`SDKHookType.WeaponDrop`; they get `false` until a later RE slice finds a live path.

PreThink/PostThink/CanBeAutobalanced hook the `EntityRef` the author passed. On CS2 that is usually the
pawn, not the controller. Hooking the wrong class is `false`, not a crash.

## Gamedata

SourceMod ships `gamedata/sdkhooks.games/` as a table of **vtable slot numbers** (`Touch` linux 104,
…). We do not copy that file. A bare index is the `sm_slay` / slot-400 failure class: `.text`-valid
and the wrong function. Layout is data; the data we ship is a **byte signature**, not a slot.

### Owner: `core`, not `gamedata/sdkhooks/`

A5 reserved an extension-tier `gamedata/sdkhooks/` owner and left it empty until a capability
needed its own namespace. That owner is still the wrong home.

`scripts/check-gamedata-owners.sh`: a key in `gamedata/<other>/` must **not** appear as a string
literal in `shim/src` or `core/src`. `core/src/sdkhooks.rs` already names `"OnTakeDamage"` and will
name `"Touch"`, `"Think"`, …. The shim VP installer will name the signature keys
(`"CBaseEntity_Touch"`). Those strings make every fact **core-owned**. Standing up
`gamedata/sdkhooks/` would fail the gate the moment the table lands.

SM’s `sdkhooks` owner existed because SDKHooks was an extension with its own `LoadGameConfigFile`.
Ours is first-party in shim+core, same as Transmit / DTA. The npm path `@s2script/sdk/sdkhooks` is
types; it does not own gamedata.

Target stays CS2: `gamedata/core/game.cs2.jsonc` (core owner, `game: csgo` target). A second Source 2
game is a sibling `game.<mod>.jsonc` with the same key set.

To keep ~40 signatures from drowning Teleport / DTA / CheckTransmit, core’s master lists a **sibling
file** — `gamedata/core/game.cs2.sdkhooks.jsonc`, same `"game": "csgo"` condition. Same owner, same
`custom/` hot-fix channel (`gamedata/core/custom/`). No new loader owner. No new JSON section until
one is forced.

### What a VP type stores

Signatures named after the C++ virtual, plus the existing `vtable-member` validator (Respawn’s
gate). Mapping `SDKHookType` → signature / thunk shape / pre-vs-post is **code** (CLAUDE.md: name
mappings in reviewed code).

```jsonc
"signatures": {
  "CBaseEntity_Touch": {
    "linuxsteamrt64": {
      "module": "libserver.so",
      "pattern": "55 48 89 E5 …",
      "resolve": "direct",
      "validate": { "vtable-member": "CBaseEntity" }
    }
  }
}
```

Load, per signature in the VP table:

1. Unique match in `libserver.so` + `.text` (existing `ResolveSigValidated`).
2. `vtable-member`: that address is a slot of the named class’s RTTI primary vtable
   (`s2vtable::GetVTableByName`). Unique-but-wrong dies here.
3. Scan the vtable for the address → derived slot. Cache it. Banner
   `gamedata OK CBaseEntity_Touch (slot N)`.
4. Any step fails → `GAMEDATA descriptor 'CBaseEntity_Touch' FAILED: <reason>`, that type (and its
   Post twin) disabled, `SDKHook` returns `false`.

SourceHook `SH_ADD_MANUALHOOK` consumes the **cached** slot, not a number from git. Pre and Post
wiki types share one signature (`Touch` + `TouchPost` = one virtual, two SourceHook modes).

Do not add an `sdkhooks` JSON section of type names. That would duplicate the C++ table and put
authoring names in layout data.

### What does not get a new signature

| Type family | Engine fact | Already lives |
|-------------|-------------|----------------|
| `OnTakeDamage` / Post / Alive | `DispatchTraceAttack` | `gamedata/core/game.cs2.jsonc` `signatures` |
| `SetTransmit` | `ISource2GameEntities::CheckTransmit` + `CheckTransmitInfo_clientEntityIndex` | same file, `interfaces` / `offsets` |
| `SDKHooks.takeDamage` | reuse DTA | none new |
| `SDKHooks.dropWeapon` | ItemServices (or equivalent) **signature**, `vtable-member` on the real class | new key in the sdkhooks sibling file; borrowed vtable index 24 stays forbidden |

`TraceAttack` / `FireBulletsPost` get a signature row in the sibling file if RE finds a distinct
virtual; they do not borrow DTA’s slot by default.

Plugins do not ship SDKHooks gamedata. Operator hot-fix is `gamedata/core/custom/*.jsonc`, same as
every other core fact. A type with no resolvable virtual on this CS2 build simply never gets a
signature row until an RE slice finds one — `SDKHookType` may still list it; `SDKHook` returns
`false`.

## Implementation stack (atomic PRs on `cursor/sdkhooks-a8c9`)

One wiki-complete product; several merge-safe PRs. Each PR is green `make ci` on its own. Do not ship a
type in `SDKHookType` without the degrade-or-dispatch path in the same PR.

1. **VP-hook primitive + Touch family** (`StartTouch` / `Touch` / `EndTouch` / `Blocked` + Posts).
   Proves install/teardown, ledger, destroy-unhook, `HookResult` collapse on a virtual.
2. **Entity lifecycle virtuals** — Spawn, Think, Use, GetMaxHealth, ShouldCollide, GroundEntChangedPost,
   VPhysicsUpdate, PreThink/PostThink, CanBeAutobalanced.
3. **SetTransmit** — CheckTransmit mux + AND-merge with `Transmit.setVisibleTo`.
4. **Weapon family + Reload + remaining damage types** (`OnTakeDamagePost`, Alive, TraceAttack,
   FireBulletsPost).
5. **`SDKHooks.takeDamage` + `SDKHooks.dropWeapon`** — latch + DropWeapon sig-resolve. Does not un-stub
   `pawn.dropActiveWeapon`.

## Out of scope

- **SDKTools** ([wiki](https://wiki.alliedmods.net/SDKTools_(SourceMod_Scripting))) — different product,
  not this stack. SDKHooks is inbound per-entity virtuals + two natives. SDKTools is outbound SDK
  calls (BinTools/`SDKCall`), helper natives, sound, and tempents. We already replaced the spine:
  `Engine.call` + gamedata `calls` is `PrepSDKCall`/`SDKCall`; `EntityRef.teleport` / `setModel` /
  `acceptInput` / `createEntity` / `Entity.findByClass` / `pawn.giveNamedItem` / `Sound.emit` /
  `@s2script/sdk/trace` / schema accessors cover the helpers that used to live in
  `sdktools_functions.inc`. Remaining SDKTools-shaped gaps (classic `TE_*` tempents if CS2 still has
  an `ITempEnts` equivalent; `IgniteEntity`) are their own slices, not a `SDKTools` namespace bolted
  onto this catalog. Do not stand up `gamedata/sdktools/` for the same owners-gate reason as
  `gamedata/sdkhooks/`.
- DHooks / plugin-authored detour layouts
- `SDKHookEx` as a second name
- `Action` / `HookResult as Action`
- `SetTransmitPost`
- Moving `SDKHook` under `SDKHooks`
- Wiring `pawn.dropActiveWeapon()`
- Priority on `SDKHook`
- Forcing transmit (`m_pTransmitAlways`)
- Books-delete `v8::Global` leak / a4gate reload re-hook (known nits on #134)
