# CS2 `Weapon` entity object + player fire control

**Date:** 2026-07-13
**Status:** Design
**Slice:** First of the "entity-backed object" program (maximizing typed, safe game objects in `@s2script/cs2`).

## Goal

Give plugin authors ergonomic, safe, typed objects for the game entities they actually manipulate — starting with weapons — so a plugin can do:

```js
import { Pawn, Weapon } from "@s2script/cs2";

const w = pawn.activeWeapon;      // → Weapon | null
w.setAmmo(30, 90);                // refill
w.paintKit = 44;                  // reskin (AK Fire Serpent)
pawn.disarm();                    // take all their guns
pawn.blockFiring(3);              // and they can't shoot for 3s
```

This slice ships the **`Weapon`** object, the **Pawn ↔ Weapon** relationship, and player-level **fire control** (`pawn.blockFiring`). It also establishes the reusable *entity-object recipe* that later slices (Bomb, Projectile, Prop) copy at near-zero cost.

## Background: the two-tier object model

CounterStrikeSharp models every schema class as a raw-pointer wrapper (`new CGlowProperty(ptr)`). s2script cannot — the charter forbids exposing a raw pointer across time. Once raw pointers are off the table, "CC classes" split into two kinds, each with an existing codegen path:

| Kind | Example | Reached by | Our pattern |
| --- | --- | --- | --- |
| **Entity** (has an `EntityRef`) | `CCSPlayerPawn`, **`CCSWeaponBase`**, `env_beam` | index + serial, serial-gated | `EntityRef`-backed prototype (`Pawn`, `Player`) + mounted generated field accessors |
| **Sub-object** (embedded / pointer-reached) | `CCSPlayer_WeaponServices`, `CGlowProperty` | pointer/embedded chain **from** an owning entity | nav-wrapper `(rootRef, path)`, re-resolved + serial-gated at the root (`navgen`) |

A **weapon is entity-backed** (`CCSWeaponBase` ← `CBasePlayerWeapon` ← `CEconEntity` ← `CBaseEntity`), so it's a first-class `Weapon` object. Its field accessors already generate today (`CCSWeaponBase` is in `codegen-classes.json`) — there is simply no object mounting them yet.

"Maximizing entity-backed objects" therefore means: **curate the entity classes worth modeling and mount their generated accessors + add methods/acquisition** — *not* mint a `.js` per schema class. Sub-objects (glow, weapon-services) stay on the nav-wrapper track and are explicitly out of this program.

## The reusable entity-object recipe

Every entity object added by this program follows the same steps (documented here so Bomb/Projectile are turn-key):

1. Confirm the class is a `CBaseEntity` subclass (entity-backed). If not → it's a sub-object, use the nav track instead.
2. Ensure the class is in `games/cs2/codegen-classes.json` (its accessors then generate, flattened with ancestors).
3. In `games/cs2/js/<family>.js`: `function X(ref){ this.ref = ref; }`, `schema.applyAccessors(X.prototype, "ClassName")`, and register the constructor on the shared game namespace: `globalThis.__s2pkg_cs2 = Object.assign(globalThis.__s2pkg_cs2 || {}, { X });`.
4. Add methods (over existing writes / sig-scanned ops).
5. Add acquisition helpers (`X.fromEntity`, `X.findAll`) and nav to/from related objects (`pawn.weapons`, `weapon.owner`).
6. Add types to `packages/cs2` (a `weapon.d.ts` re-exported from `index.d.ts`).
7. Concatenate the file in `scripts/package-addon.sh` **before** `pawn.js`.

Cross-object references (`pawn.weapons` → `Weapon`, `weapon.owner` → `Pawn`) resolve **lazily at call time** via the shared `globalThis.__s2pkg_cs2` namespace, so the two files need not be ordered relative to each other's *definitions* — only both must have registered their constructors before any getter runs (guaranteed: getters run at plugin runtime, long after all concatenated files load).

## Slice scope

### 1. The `Weapon` object (`games/cs2/js/weapon.js`, `packages/cs2/weapon.d.ts`)

`EntityRef`-backed, serial-gated. `schema.applyAccessors(Weapon.prototype, "CCSWeaponBase")` mounts the full flattened field surface (verified present: `clip1`, `clip2`, `fallbackPaintKit`, `fallbackWear`, `fallbackSeed`, `weaponMode`, `inReload`, `itemDefinitionIndex`, `accountId`, plus inherited `health`/`teamNum`/`ownerEntity`/`absVelocity`/…). Read **and** write come free (the generated setters call `writeInt32`/`writeFloat32` + `notifyStateChanged`).

Curated public surface (the `.d.ts` documents these; the raw generated surface remains available):

- **Reads:** `clip1`, `clip2`, `reserveAmmo`\*, `itemDefinitionIndex`, `paintKit` (= `fallbackPaintKit`), `weaponMode`, `inReload`, `accountId`.
- **Writes:** `clip1`, `clip2`, `paintKit` — the marquee refill/infinite-ammo/reskin manipulations.
- **`.ref`** → the underlying `EntityRef` (escape hatch).
- **`weapon.owner`** → the holding `Pawn | null` (via `m_hOwnerEntity` handle, serial-gated; wraps in `Pawn` lazily).
- **Methods:**
  - `weapon.setAmmo(clip, reserve?)` — writes `clip1` (+ reserve when the layout is known; see \*).
  - `weapon.remove()` → the **complete** "take this weapon away" atom: resolve `owner`; if present, `RemovePlayerItem(owner, this)` (existing plumbing) to unequip; then `UTIL_Remove(this)` (existing `EntityRef.remove`) to destroy. Returns `boolean`.
- **Statics:** `Weapon.fromEntity(ref)` → `Weapon | null`; `Weapon.findAll(className)` → `Weapon[]` over `Entity.findByClass` (e.g. `Weapon.findAll("weapon_ak47")`).

\* **`reserveAmmo`** (`m_pReserveAmmo`) is likely an indexed array / network-var rather than a plain scalar. The MVP ships `clip1`/`clip2` (plain int32) with confidence; `reserveAmmo` read/write is verified during implementation and, if the layout is awkward, deferred to a follow-up. `setAmmo`'s `reserve` arg is a no-op until then (documented).

### 2. Pawn ↔ Weapon (acquisition + altitude rule)

The relationship is **containment, navigable both ways**, with a strict altitude rule: *single-instance* ops live on `Weapon`; *collection/intent* ops live on `Pawn` and **fold over** the weapon-level ops (one implementation site each).

Pawn-side changes (`games/cs2/js/pawn.js`):

- `pawn.activeWeapon` → `Weapon | null` (wraps the existing `weaponServices.activeWeapon` `EntityRef`).
- `pawn.weapons` → **`Weapon[]`** (was `EntityRef[]` — wrap each; pre-1.0, and `Weapon.ref` recovers the raw ref).
- `pawn.giveNamedItem(name)` → **`Weapon`** (was `EntityRef`).
- `pawn.stripWeapons()` / **`pawn.disarm()`** (alias) → remove all held weapons, re-implemented as `for (const w of pawn.weapons) w.remove();` — i.e. a fold over `Weapon.remove()`. Returns `boolean`.

`disarm()` here means **destroy all held weapons** (available now). A **drop-to-ground** disarm (spawns a pickup-able world weapon) is *not* composable from `remove()`; it needs the deferred `dropActiveWeapon` signature-resolve (see Deferred).

### 3. Player fire control (`pawn.blockFiring`) + the write-chain primitive

The effective "can't fire" gate in CS2 is **`m_flNextAttack`** (a `GameTime_t`, in seconds) at offset 192 on **`CCSPlayer_WeaponServices`** — the pawn's weapon-services **sub-object**, reached via the `m_pWeaponServices` pointer. Setting it into the future blocks *every* weapon the player holds (this is CSSharp's `NextAttack` and what real anti-fire plugins write).

Because it's a **pointer-reached sub-object field**, writing it needs a capability we don't have: our `EntityRef.*Via` methods are **read-only** (there is no write-chain native). This slice adds the **one new engine-generic primitive**:

- **`__s2_ent_ref_write_chain(idx, serial, pathOffs[], finalOff, kind, value)`** — the write mirror of the existing `__s2_ent_ref_read_chain`: serial-gate the root, deref each `pathOffs` hop (null-checked; raw intermediate pointers never cross to JS), then write the kind-dispatched scalar (`float32`/`int32`/`bool`/…) at `finalOff`. Guards fire before the resolve so they're in-isolate testable. Engine-generic (a pointer chain of offsets is Source2-generic) → lives in `core`; needs one sniper rebuild.
- **`EntityRef.writeFloat32Via` / `writeInt32Via` / `writeBoolVia`** — thin JS wrappers over the native (mirror the read-via methods).

Pawn API (`games/cs2/js/pawn.js`):

- `pawn.blockFiring(seconds?)` → writes `m_flNextAttack = Server.gameTime + seconds` through `m_pWeaponServices` via `writeFloat32Via`. `seconds` defaults to a large value ("effectively indefinite"); returns `boolean` (false if the chain/offset is unresolved or the ref is stale).
- `pawn.allowFiring()` → writes `m_flNextAttack = Server.gameTime` (fire immediately).

**Semantics/caveats (documented in the `.d.ts`):**
- The fire check is **server-authoritative**, so the raw write blocks the actual shot — no netvar `notifyStateChanged` is required for the effect (unlike networked-for-display fields like `health`). A client may still animate a click; the server rejects the shot.
- It's a **time gate the engine advances past**: a brief cooldown is `now + N`; a *durable* block means a large `seconds` or refreshing each `OnGameFrame`. The helper does the write; persistence policy is the caller's.

The **weapon-level** next-attack fields (`m_nNextPrimaryAttackTick`, GameTick_t on `CBasePlayerWeapon`) are *not* in the curated generated surface (the generator currently exposes ratio fields, not the raw ticks) and are unnecessary given the player-level gate — noted, not built.

## Engine work summary

- **New:** one engine-generic core native — `__s2_ent_ref_write_chain` (write mirror of `__s2_ent_ref_read_chain`, registered the same way) + `EntityRef.write*Via` JS wrappers. **One sniper rebuild** (core `.so` only; no shim / no `S2EngineOps` op — the chain is pure in-core pointer math on the already-resolved, serial-gated root, exactly like the read-chain native).
- **Reused, no new work:** generated `CCSWeaponBase` accessors, `writeInt32`/`writeFloat32`/`notifyStateChanged`, `RemovePlayerItem`/`UTIL_Remove` (existing `pawn.removeWeapon`/`EntityRef.remove`), `Entity.findByClass`, `Server.gameTime`.
- **No new signatures** → gamedata validation count unchanged.

## Safety

- `Weapon` is `EntityRef`-backed; every field access re-validates the captured serial (a stale weapon reads `null`/`false`, never garbage). Consistent with `Pawn`/`Player`.
- `weapon.owner` and `Weapon.findAll` hand back serial-gated wrappers; no raw pointer crosses to JS.
- `write_chain` serial-gates at the root and null-checks every hop; a stale ref or unresolved offset is a bounded `false`, never a crash. `catch_unwind` on the native (degrade-never-crash).

## Testing

- **In-isolate (core):** `write_chain` guard/degrade behavior (bad kind, unresolved hop, non-array path, stale serial → false, no panic); `write*Via` wrappers.
- **Live gate (bot-provable):** a `weapon-demo` plugin — `pawn.activeWeapon` reads a valid weapon, `setAmmo`/`clip1=` writes stick (read-back), `paintKit=` writes, `pawn.weapons` lists, `pawn.disarm()` empties the list (weapon count → 0), `pawn.blockFiring()` writes `m_flNextAttack` (read-back confirms `> gameTime`). Server ticks, `RestartCount=0`.
- **Deferred (human-client):** *visually* confirming a reskin renders and that a real client genuinely cannot fire while blocked — same ceiling as SayText2/damage (bots don't render/shoot reliably). Mechanism is proven by the read-backs.

## Packaging / file layout

- Home: **`@s2script/cs2`** (charter-consistent with `Pawn`/`Player`/`GameRules`). Import: `import { Weapon, Pawn } from "@s2script/cs2";`.
- New runtime file `games/cs2/js/weapon.js`, concatenated by `scripts/package-addon.sh` in order: `schema.generated.js → nav.generated.js → activity.js → csitem.generated.js → weapon.js → pawn.js` (`weapon.js` after `schema.generated.js` so `__s2pkg_cs2_schema` exists; before `pawn.js` so `Weapon` is registered when `pawn.js`'s acquisition getters reference it).
- New types file `packages/cs2/weapon.d.ts`, re-exported from `packages/cs2/index.d.ts`; the new `EntityRef.write*Via` methods added to `packages/entity/index.d.ts`.
- `scripts/check-schema-generated.sh` / typecheck / boundary gates stay green (no core→game import).

## Deferred (do NOT build ahead)

- **`pawn.dropActiveWeapon()`** (drop-to-ground, pickup-able) — needs the real `DropWeapon` signature-resolve (the borrowed vtable index is a `GiveNamedItem` thunk; not composable from `remove()`). Its own RE task.
- **`Weapon.onFire` pre-hook** (bulletproof block, immune to weapon-switch/reload) — a detour on the weapon fire function returning `HookResult ≥ Handled`, same machinery as the 6.6 damage detour. Separate RE slice. (Hooking the `weapon_fire` *event* does not work — it fires after the shot.)
- **`pawn.getWeapon(className)` / `hasWeapon(className)`** — needs a `EntityRef.className` (designer-name) read the engine doesn't yet surface to JS.
- **`reserveAmmo`** write (and `setAmmo`'s reserve arg) if `m_pReserveAmmo`'s layout is an indexed array.
- **`customName`/nametag** — `m_szCustomName` is not in the direct generated surface (likely on an `CEconItemView` sub-struct); deferred.
- **Full navgen setter emission** — this slice hand-uses `write*Via` for `blockFiring`; codegen-ing setters across all nav wrappers (the general WeaponServices/MovementServices write surface) is the natural follow-up now that the write-chain primitive exists.
- **`switchWeapon`/`equip`** — needs a game-function call (deploy).

## Follow-up slices (the rest of the entity-object program)

Each is the recipe above at near-zero cost:

- **Bomb** (`CPlantedC4`) — `bombSite`, `defuser` (→ `Player`), `ticking`, `defuseLength`.
- **Projectile** (`CBaseCSGrenadeProjectile`) — `thrower` (→ `Player`), velocity, detonate via `acceptInput`.
- **Prop / Door / Button / Trigger** — mostly already drivable via `Entity.findByClass` + `acceptInput`/`onOutput`; typed objects add field accessors + convenience.
- **Generic `Entity` + `EntityRef.as("ClassName")`** — the long-tail escape hatch for any entity class's field surface without a named object.
