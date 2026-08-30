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
…). We do not copy those numbers. A bare index is the `sm_slay` / slot-400 failure class: `.text`-valid
and the wrong function. The **owner split** we do copy.

### Owner: `gamedata/sdkhooks/` — you were right

A5 reserved the extension-tier owner and left it empty “until a capability needs its own namespace.”
This is that capability. The previous draft stuffed the catalog into `gamedata/core/` (even a sibling
`game.cs2.sdkhooks.jsonc`) because `check-gamedata-owners.sh` says: *if shim/core contains the string,
the key is core-owned.* That grep is a **category error** for a first-party extension.

SourceMod 1.5 rolled SDKHooks *into* the sourcemod binary and **still** loads `gamedata/sdkhooks.games/`,
not `core.games`. The C++ calls `GetOffset("Touch")` on the sdkhooks GameConfig. “The engine layer
names the key” did not mean “merge the file into core.” Our gate was written to keep **game-package**
keys (`gamedata/cs2/`, `Respawn`, `GiveNamedItem`) from leaking back into shim after A5b retired those
ops. Applying the same rule to `sdkhooks` forced a workaround (a fake sibling file under core) that
will not scale to SDKTools leftovers, a second game, or an operator who expects `gamedata/sdkhooks/`.

**What stays in the shim (do not move “outside core”):** SourceHook install, the per-entity table,
`HookResult` collapse, ledger, DTA mux, CheckTransmit mux. Those are engine touchpoints. A JS plugin
or a second Metamod binary must not own them — that is the unification product. `@s2script/sdk/sdkhooks`
stays types. Familiar to SM: the extension’s *gamedata* is separate; the extension’s *code* lives in
the runtime.

**Amended owners rule** (gate + `kGamedataOwners` grow a kind):

| Kind | Today | Shim may name keys? |
|------|--------|---------------------|
| `core` | `gamedata/core/` | yes — via `s_gdCore` |
| `game` | `gamedata/cs2/` (future `gamedata/<game>/`) | **no** — descriptors / game-package JS only |
| `extension` | `gamedata/sdkhooks/` (future leftovers, not this stack) | **yes** — via that owner’s `GameConfig`, never `s_gdCore` |

`sdkhooks` keys therefore look like SM (`"Touch"`) and may appear as string literals in
`sdkhooks.rs` / the VP installer. They must not be read out of `s_gdCore`. The first
implementation PR amends `check-gamedata-owners.sh` and adds `{ "sdkhooks", …, Extension }` to
the loader table. An owner directory with no loader row still fails the gate.

Tree:

```
gamedata/sdkhooks/
  master.gamedata.jsonc          # { file: game.cs2.jsonc, game: csgo }
  game.cs2.jsonc                 # signatures for this CS2 build
  custom/                        # operator hot-fix, same as every owner
```

Second Source 2 game: `gamedata/sdkhooks/game.<mod>.jsonc`, same keys, different bytes. Do not put
those bytes in `gamedata/cs2/` — that owner is `@s2script/cs2`’s descriptors, not this catalog.

### Validation stays — it is not why SM skipped it

SM did not load-validate offsets because Source 1 Linux often had **symbols** (`@RoundRespawn`), a
large gamedata army, and a culture of “wrong slot = crash / weird.” We do not have the first two.
We already paid for the third: `sm_slay`’s borrowed 400, ChangeTeam’s unique-but-wrong signature.
`vtable-member` + unique-match is the treadmill, not a workaround for the owner grep. Stripping it
would make 40 virtuals fail the same silent way one slay did.

What we will **not** do: invent extra JSON sections, ship slot numbers as the source of truth, or
treat `.text`-range alone as enough (that is what green-lit slot 400).

What we **will** make easier: gamedata looks like SM’s Signatures block, keyed by the **wiki type
name** (`Touch`, not `CBaseEntity_Touch`). The class for the RTTI gate is `validate.vtable-member`.
The boot banner prints the **derived** slot so an SM developer can grep logs the way they grep
offsets. Mapping type → thunk shape / pre-vs-post stays code (closed shape vocabulary). Pre and
Post twins share one signature.

```jsonc
"signatures": {
  "Touch": {
    "linuxsteamrt64": {
      "module": "libserver.so",
      "pattern": "55 48 89 E5 …",
      "resolve": "direct",
      "validate": { "vtable-member": "CBaseEntity" }
    }
  }
}
```

Load, per wiki type that uses a VP hook:

1. Unique match in `libserver.so` + `.text` (`ResolveSigValidated` on the **sdkhooks** GameConfig).
2. `vtable-member`: that address is a slot of the named class’s RTTI primary vtable.
3. Scan the vtable → derived slot. Cache it. Banner `gamedata OK Touch (slot N)`.
4. Failure disables that type and its Post twin. `SDKHook` returns `false`. Framework keeps running.

SourceHook `SH_ADD_MANUALHOOK` consumes the cached slot, not a number from git. A CSSharp/SM slot in
a comment is a HINT for the offline RE, never the shipped fact.

### What does not get an sdkhooks signature

| Type family | Engine fact | Owner |
|-------------|-------------|--------|
| `OnTakeDamage` / Post / Alive | `DispatchTraceAttack` | **core** (damage mux; existed before this catalog) |
| `SetTransmit` | `CheckTransmit` + `CheckTransmitInfo_clientEntityIndex` | **core** (`Transmit.setVisibleTo` owns this hook) |
| `SDKHooks.takeDamage` | reuse DTA | none new |
| `SDKHooks.dropWeapon` | ItemServices (or equivalent) signature + `vtable-member` | **sdkhooks** |
| `TraceAttack` / `FireBulletsPost` | own signature if RE finds a distinct virtual | **sdkhooks** |

DTA and CheckTransmit stay core because other capabilities already consume them, not because of the
grep. Plugins do not ship SDKHooks gamedata. Operator hot-fix is `gamedata/sdkhooks/custom/*.jsonc`.
A type with no resolvable virtual on this CS2 build has no signature row until an RE slice finds
one; `SDKHookType` may still list it; `SDKHook` returns `false`.

## Implementation stack (atomic PRs on `cursor/sdkhooks-a8c9`)

One wiki-complete product; several merge-safe PRs. Each PR is green `make ci` on its own. Do not ship a
type in `SDKHookType` without the degrade-or-dispatch path in the same PR.

1. **VP-hook primitive + Touch family** (`StartTouch` / `Touch` / `EndTouch` / `Blocked` + Posts).
   Stand up `gamedata/sdkhooks/` + amend the owners gate for extension owners. Proves install/teardown,
   ledger, destroy-unhook, `HookResult` collapse on a virtual.
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
  onto this catalog. A later leftover slice would use `gamedata/sdktools/` as an **extension** owner
  (same kind as `sdkhooks`), not `gamedata/core/`.
- DHooks / plugin-authored detour layouts
- `SDKHookEx` as a second name
- `Action` / `HookResult as Action`
- `SetTransmitPost`
- Moving `SDKHook` under `SDKHooks`
- Wiring `pawn.dropActiveWeapon()`
- Priority on `SDKHook`
- Forcing transmit (`m_pTransmitAlways`)
- Books-delete `v8::Global` leak / a4gate reload re-hook (known nits on #134)
