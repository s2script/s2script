# Slice: item / weapon manipulation (give / strip / drop / remove / enumerate)

**Date:** 2026-07-09
**Status:** design approved (full scope incl. enumeration) — proceeding to plan
**Reference:** CounterStrikeSharp's item surface (`CCSPlayer_ItemServices`, `VirtualFunctions.cs`, `CsItem`).
Builds on the entity-creation slice (`EntityRef`, `.remove()`), the nav chains (`weaponServices`), and the
`CCSWeaponBase` schema codegen.

## Goal

Give plugins the CS2 item/weapon surface: **give** a player a named weapon/item, **strip** all weapons,
**drop** the active weapon, **remove** a specific weapon, and **enumerate** a player's weapons — plus a
`CsItem` name-constant map. Introduces a reusable engine-generic **`CUtlVector<CHandle>` read primitive**
(the first `CUtlVector` support — unlocks every handle-vector field, `m_hMyWeapons` being the first consumer).

## What we're building

### CS2 API (game layer — `games/cs2/js/pawn.js` + `packages/cs2`)
- **`pawn.giveNamedItem(name: string) → EntityRef | null`** — spawns + equips the item, returns the created
  weapon as a serial-gated `EntityRef` (so callers can then set its ammo/fields). `null` on failure.
- **`pawn.weapons → EntityRef[]`** — the player's weapons (via `m_hMyWeapons`).
- **`pawn.activeWeapon`** — already present (weaponServices nav); documented alongside.
- **`pawn.stripWeapons(): boolean`** — remove all weapons (`RemoveWeapons`, no-arg → `subobj_vcall` with `argIdx=-1`).
- **`pawn.dropActiveWeapon(): boolean`** — drop the currently-held weapon (`DropActivePlayerWeapon` → `subobj_vcall`
  passing the resolved `activeWeapon` ref as the arg; safe whether the fn is no-arg or `(this, weapon)`. If the
  spike shows drop needs >1 arg, drop is deferred to a fast-follow rather than guessed).
- **`pawn.removeWeapon(weapon: EntityRef): boolean`** — remove one specific weapon (proper unequip).
- **`CsItem`** — a `{ AK47: "weapon_ak47", ... }` constant map + `const` type, extracted from CSSharp's
  `CsItem` enum (a re-runnable extraction, like the event catalog). `giveNamedItem` accepts either a raw
  string or a `CsItem` value (both are strings).
- Weapon ammo/clip (`weapon.clip1`, `weapon.reserveAmmo`, …) come free from the existing `CCSWeaponBase`
  schema codegen on the returned/enumerated refs.

### Engine-generic primitives (core + shim)
Four new `S2EngineOps` ops, **ABI-appended after the current last op (`entity_remove`)**, generic-signature
(entity ref + offsets/index/string — no CS2 schema names in core; same pattern as `pawn_commit_suicide`):

| op | signature | returns |
|----|-----------|---------|
| `give_named_item` | `(int idx, int serial, int subObjOffset, const char* className)` | packed `CEntityHandle` of the created weapon (0 = fail) |
| `entity_subobj_vcall` | `(int idx, int serial, int subObjOffset, int vtableIndex, int argIdx, int argSerial)` | bool — call `vtable[index](subObj, argEntityPtr\|null)` on the entity's sub-object; `argIdx < 0` → null (no-arg). Covers strip (no-arg) and drop (one-entity-arg) with ONE call shape (an extra register arg to a no-arg fn is ignored on SysV) |
| `remove_player_item` | `(int pawnIdx, int pawnSerial, int weaponIdx, int weaponSerial)` | bool |
| `entity_read_handle_vector` | `(int idx, int serial, const int* ptrOffs, int ptrCount, int vectorOff, int maxCount)` | `EntityRef[]` — follow a pointer chain, then read a `CUtlVector<CHandle>`, decoded serial-gated |

### The RE (all validated offline against the pinned `libserver.so`, 2026-07-09)
- **`GiveNamedItem`** — sig `55 48 89 E5 41 57 41 56 41 55 41 54 53 48 81 EC ...` **UNIQUE @0x152c560**,
  `resolve:"direct"`. ABI (from CSSharp `VirtualFunctions.cs`): `void* GiveNamedItem(void* itemServices,
  const char* name, void* iSubType, void* pScriptItem, void* a5, void* a6)` → call with
  `(itemServices, name, 0, nullptr, 0, nullptr)`; returns the weapon `CBaseEntity*` (→ `GetRefEHandle().ToInt()`,
  never a raw ptr to JS). The `this` is the **ItemServices** sub-object, reached via `m_pItemServices`.
- **`RemovePlayerItem`** — `CBasePlayerPawn::RemovePlayerItem` sig `55 48 89 E5 41 54 49 89 FC 53 48 89 F3 E8
  ? ? ? ? 48 39 C3` **UNIQUE @0x1580110**, `resolve:"direct"`. ABI `(pawn, weapon) → bool`.
- **`RemoveWeapons`** = ItemServices **vtable index 25**, **`DropActivePlayerWeapon`** = **vtable index 24**
  (CSSharp offsets; borrowed indices → **`.text`-validated before the call** via the existing
  `IsAddressInServerText` — the CommitSuicide-index-burn discipline). Recorded in gamedata `offsets`.
- **`m_pItemServices` @ 3304** (`CBasePlayerPawn`) — in our live schema catalog; resolved via
  `__s2_schema_offset` in `pawn.js` (CS2 name stays in the game layer).
- **`m_hMyWeapons`** — a `CNetworkUtlVectorBase<CHandle<CBasePlayerWeapon>>` on `CCSPlayer_WeaponServices`
  (reached via `m_pWeaponServices`). **`CUtlVector` layout to confirm in the spike** (standard Source 2:
  `int m_Size` @ +0, `T* m_pElements` @ +8; element = `CHandle` = a 4-byte packed uint). The spike verifies
  the size/element-ptr offsets against the binary before committing.

## CUtlVector read primitive (the reusable core piece)

`entity_read_handle_vector`: serial-gate the root `(idx, serial)`; follow `ptrOffs` (each hop
`read_ptr`, null-checked — the intermediate pointers **never cross to JS**); at the final struct read the
`CUtlVector` header (`count` @ `vectorOff`, `elements ptr` @ `vectorOff + 8`); **bound `count` to
`[0, maxCount]`** (a sane cap, e.g. 64 — a corrupt/huge count degrades to an empty/capped read, never a
runaway); read `count` 4-byte handles from `elements`, decode each (`decode_handle`), **serial-validate each
via `entity_resolve_ptr`** (a dead slot → skip), and build an `EntityRef` array. Mirrors
`readFloatsChain` (5C.4) for the chain + `s2_trace`'s handle→EntityRef for each element. `EntityRef` gains
`readHandleVector(ptrOffs, vectorOff, maxCount) → EntityRef[]`. Engine-generic (`CUtlVector` is Source 2).

## Boundary (both gates stay green)

The 4 ops take `(idx, serial, offset(s)/index/string)` — **zero** CS2 schema class/field names in core. The
CS2 facts (`m_pItemServices` / `m_pWeaponServices` / `m_hMyWeapons` offsets, weapon-name strings, `CsItem`,
the `pawn.*` methods) live only in `pawn.js` + `packages/cs2`. The sigs (`GiveNamedItem`, `RemovePlayerItem`)
+ vtable indices (`RemoveWeapons`=25, `DropActivePlayerWeapon`=24) are gamedata. `check-core-boundary.sh` +
`test-boundary-nameleak.sh` stay green.

## Testing

- **In-isolate (core):** each op degrades with no engine — `give_named_item → 0/null`,
  `subobj_vcall/remove_player_item → false`, `read_handle_vector → []`. Assert the `CUtlVector` count-cap
  (a huge count → capped/empty, never a crash) and the ptr-chain null-guard as pure units where possible.
- **Live gate — BOT-PROVABLE (the big advantage over the beam slice):** with `bot_quota 2`, via rcon:
  - `sm_give <bot> weapon_ak47` → the returned ref is valid; read back `pawn.activeWeapon` / the weapon's
    schema to confirm the AK is equipped.
  - `sm_weapons <bot>` → `pawn.weapons` lists the bot's weapons (bots spawn with a rifle + pistol + knife) —
    a non-empty `EntityRef[]` with valid refs, proving the `CUtlVector` read.
  - `sm_strip <bot>` → `pawn.weapons` empties.
  - `sm_drop <bot>` → the active weapon drops (weapon count decreases / active changes).
  - `GAMEDATA VALIDATION` grows by the new sigs; `RestartCount=0`, server ticking, no crash.
  No human client needed — items are fully bot-verifiable.

## Non-goals (do NOT build)

- Cosmetics: skins / paint kits / wear / StatTrak / knives / gloves (`CEconItemView` — a deep niche).
- `GetCSWeaponDataFromKey` weapon-stat lookups; `CanAcquire` (its sig isn't unique — would need refinement).
- Switch/force active weapon; generic non-handle `CUtlVector` element types (float/entity-embedded vectors)
  — only `CUtlVector<CHandle>` this slice.
- A generic plugin-facing "call arbitrary vtable index" API — `entity_subobj_vcall` is core-internal, driven
  only by fixed gamedata indices in `pawn.js`.

## Sequencing (spike-first, one slice)

0. **Spike** — confirm against the pinned binary: (a) the `CUtlVector` header layout (size @ +0, elements-ptr
   @ +8) by disassembling the `m_hMyWeapons` access; (b) `GiveNamedItem`'s 6-arg call; (c) **`DropActivePlayerWeapon`
   (vtable 24)'s arg count** — no-arg vs `(this, weapon)` vs more (if >1 non-`this` arg, defer drop); (d)
   `RemoveWeapons` (vtable 25) is no-arg. Record the gamedata (2 sigs + 2 vtable offsets).
1. **Core ops + `CUtlVector`/item natives + degrade tests** (the 4 ops, ABI-appended; `EntityRef.readHandleVector`).
2. **Shim** — resolve the 2 sigs + 2 vtable indices; implement the 4 op functions (ItemServices deref +
   GiveNamedItem call + `.text`-validated vcalls + RemovePlayerItem + the CUtlVector walk); sniper build.
3. **`CsItem` extraction** — `scripts/extract-csitem.mjs` (pinned CSSharp ref) → a `CsItem` map + `.d.ts`.
4. **CS2 `pawn.*` methods + types** — `giveNamedItem`/`weapons`/`stripWeapons`/`dropActiveWeapon`/`removeWeapon`.
5. **Demo + bot-provable live gate** — `sm_give`/`sm_weapons`/`sm_strip`/`sm_drop`.

Needs one sniper rebuild (the 4 ops + shim). Related: [[re-gamedata-strategy]], [[entity-creation-and-beam]],
[[cs2-schema-entity-access]].
