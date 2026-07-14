# CS2 `Weapon` Entity Object + Pawn Fire Control — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a typed, serial-gated `Weapon` object over `CCSWeaponBase` in `@s2script/cs2`, the Pawn↔Weapon relationship (`disarm` folding over `Weapon.remove`), and player-level fire control (`pawn.blockFiring`), powered by one new engine-generic write-chain native.

**Architecture:** A weapon is entity-backed (`CCSWeaponBase ← CBaseEntity`), so `Weapon` is an `EntityRef`-backed prototype mounting the already-generated `CCSWeaponBase` field accessors + a few methods — the same shape as `Pawn`/`Player`. The one new engine primitive is `__s2_ent_ref_write_chain` (the write mirror of the existing `__s2_ent_ref_read_chain`), which unlocks writing pointer-reached sub-object fields — used here for `m_flNextAttack` (the fire gate on the `CCSPlayer_WeaponServices` sub-object).

**Tech Stack:** Rust (core V8 host, `rusty_v8`), injected prelude JS (games/cs2/js/*.js concatenated into the shipped `pawn.js`), TypeScript `.d.ts` type stubs, esbuild-built `.s2sp` plugins, CS2 + Metamod live gate via Docker.

## Global Constraints

- **CS2 identifiers live ONLY in the game-package/CLI layer** (`games/cs2/`, `packages/cs2/`) — NEVER in `core/src`. The new native takes offsets/kinds (engine-generic); weapon field names stay in `weapon.js`/`pawn.js`. `scripts/check-core-boundary.sh` enforces this.
- **Never expose a raw pointer to JS.** The write-chain native follows pointers entirely in-core; only offsets + the final scalar cross the boundary. Every object is `EntityRef`-backed and serial-gated (`T | null`/`false` on a stale ref).
- **Degrade-never-crash.** Every native is `catch_unwind`-wrapped; a stale ref / unresolved offset / bad kind is a bounded `false`/`null`, never a panic.
- **Offsets resolved live, never baked** — via `__s2_schema_offset(class, field)` at call time (self-healing across CS2 updates).
- **Core tests run single-threaded** (`.cargo/config.toml` sets `RUST_TEST_THREADS = "1"`); run with `cd core && cargo test`.
- **Pure ESM plugins** (`import {...} from "@s2script/..."`); the typecheck gate is `scripts/check-plugins-typecheck.sh` (full strict).
- **Home package:** `@s2script/cs2`. Runtime split into `games/cs2/js/weapon.js` (concatenated before `pawn.js`); types in `packages/cs2/weapon.d.ts` re-exported from `index.d.ts`.

---

## Task 1: The `write_chain` engine primitive (`EntityRef.write*Via`)

Add `__s2_ent_ref_write_chain` — the write mirror of `__s2_ent_ref_read_chain` — plus the `EntityRef.writeFloat32Via`/`writeInt32Via`/`writeBoolVia` prelude wrappers and their `.d.ts` types. This is the only engine change; it needs a sniper rebuild for the live gate (Task 4), but its unit tests run under `cargo test` with no rebuild.

**Files:**
- Modify: `core/src/v8host.rs` (add `s2_ent_ref_write_chain` fn near the existing `s2_ent_ref_read_chain` ~line 2917; register it near line 5539; add three `write*Via` methods to the `EntityRef` prelude object ~line 745; add one test in the tests module near the existing `__s2_ent_ref_write` degrade test ~line 9017)
- Modify: `packages/entity/index.d.ts` (add three `write*Via` signatures after `readHandleVia` ~line 79)

**Interfaces:**
- Produces (native): `__s2_ent_ref_write_chain(index, serial, pathOffs: number[], finalOff: number, kind: number, value) -> boolean` — serial-gates the root, derefs each i32 offset in `pathOffs` (null-checked hops, raw pointers never cross to JS), writes a scalar of `kind` at `finalOff`. `false` on stale ref / unresolved hop / bad `finalOff` / unknown or 64-bit `kind`. Does NOT call `notifyStateChanged`.
- Produces (prelude): `EntityRef.writeInt32Via(pathOffs, finalOff, value) -> boolean`, `EntityRef.writeFloat32Via(pathOffs, finalOff, value) -> boolean`, `EntityRef.writeBoolVia(pathOffs, finalOff, value) -> boolean`.

- [ ] **Step 1: Write the failing test**

Add to the `core/src/v8host.rs` tests module (alongside the existing `__s2_ent_ref_write` degrade assertions ~line 9017). Uses `eval_in_context_string` exactly like the neighboring native-degrade tests (root index 1 / serial 7 resolves to null in the test isolate → every call must return `false`, never panic):

```rust
    /// The write-chain native degrades safely on every bad input (no live entity in the test isolate).
    #[test]
    fn ent_ref_write_chain_degrades_safely() {
        // stale/absent root ref (index 1, serial 7) → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, [0], 8, 2, 1.5))"), "false");
        // finalOff < 0 → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, [0], -1, 2, 1.5))"), "false");
        // non-array path arg → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, 5, 8, 2, 1.5))"), "false");
        // unknown kind (99) → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, [0], 8, 99, 1.5))"), "false");
        // the prelude wrapper forwards + degrades to false on a stale ref
        assert_eq!(eval_in_context_string("p", "String(new EntityRef(1, 7).writeFloat32Via([0], 8, 1.5))"), "false");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd core && cargo test ent_ref_write_chain_degrades_safely`
Expected: FAIL — `__s2_ent_ref_write_chain` is not defined (ReferenceError from the eval), so the assertions don't return `"false"`.

- [ ] **Step 3: Add the native function**

Insert immediately after the `s2_ent_ref_read_chain` function (after its closing `}` ~line 2952) in `core/src/v8host.rs`:

```rust
/// Native `__s2_ent_ref_write_chain(index, serial, pathOffs, finalOff, kind, value) -> boolean`.
/// Write mirror of `s2_ent_ref_read_chain`: serial-gates the root, derefs each i32 offset in `pathOffs`
/// (each hop null-checked; raw intermediate pointers never cross to JS), then writes a SCALAR of `kind`
/// at `finalOff`. Returns false on a stale ref, an unresolved hop, a bad `finalOff`, or an unknown/64-bit
/// kind. Does NOT call notifyStateChanged (the caller decides). The final ptr is only ever written in-core.
fn s2_ent_ref_write_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let final_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(4).integer_value(scope).unwrap_or(0);
        if final_off < 0 { return; }
        let Ok(path) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return; };
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }
        let mut p = ent as *const u8;
        for i in 0..path.length() {
            let off = path.get_index(scope, i).and_then(|v| v.integer_value(scope)).unwrap_or(-1) as i32;
            if off < 0 { return; }
            p = crate::entity::read_ptr(p, off);
            if p.is_null() { return; }
        }
        let dst = p as *mut u8;
        let off = final_off;
        match kind {
            KIND_I32  => crate::entity::write_i32(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_F32  => crate::entity::write_f32(dst, off, args.get(5).number_value(scope).unwrap_or(0.0) as f32),
            KIND_BOOL => crate::entity::write_bool(dst, off, args.get(5).boolean_value(scope)),
            KIND_I8   => crate::entity::write_i8(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_I16  => crate::entity::write_i16(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_U8   => crate::entity::write_u8(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_U16  => crate::entity::write_u16(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_U32  => crate::entity::write_u32(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as u32),
            _ => return,   // unknown / 64-bit kind → false (already set)
        }
        rv.set_bool(true);
    }));
}
```

- [ ] **Step 4: Register the native**

In `core/src/v8host.rs`, find the line registering the read-chain native (~line 5539):

```rust
    set_native(scope, global_obj, "__s2_ent_ref_read_chain", s2_ent_ref_read_chain);
```

Add immediately after it:

```rust
    set_native(scope, global_obj, "__s2_ent_ref_write_chain", s2_ent_ref_write_chain);
```

- [ ] **Step 5: Add the prelude wrapper methods**

In `core/src/v8host.rs`, the `EntityRef` prelude object literal has the `read*Via` methods (~lines 735-745). Immediately after the `readInt64Via` line and before `readHandleVia`, add:

```javascript
    writeInt32Via:  function (c, o, v) { return __s2_ent_ref_write_chain(this.index, this.serial, c, o, K.I32, v); },
    writeFloat32Via:function (c, o, v) { return __s2_ent_ref_write_chain(this.index, this.serial, c, o, K.F32, v); },
    writeBoolVia:   function (c, o, v) { return __s2_ent_ref_write_chain(this.index, this.serial, c, o, K.BOOL, v); },
```

- [ ] **Step 6: Add the `.d.ts` types**

In `packages/entity/index.d.ts`, after the `readHandleVia(...)` line (~line 79), add:

```typescript
  /** Write a scalar through a pointer chain (write mirror of `read*Via`). Serial-gated at the root;
   *  returns false on a stale ref, an unresolved hop, or a bad offset/kind. Does NOT notifyStateChanged —
   *  the caller decides (many sub-object fields, e.g. the fire gate, are server-authoritative). */
  writeInt32Via(pathOffs: number[], finalOff: number, value: number): boolean;
  writeFloat32Via(pathOffs: number[], finalOff: number, value: number): boolean;
  writeBoolVia(pathOffs: number[], finalOff: number, value: boolean): boolean;
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cd core && cargo test ent_ref_write_chain_degrades_safely`
Expected: PASS (1 passed).

- [ ] **Step 8: Run the full core suite**

Run: `cd core && cargo test`
Expected: all tests pass (the total climbs by one vs. the prior count). No panics.

- [ ] **Step 9: Commit**

```bash
git add core/src/v8host.rs packages/entity/index.d.ts
git commit -m "feat(entity): __s2_ent_ref_write_chain + EntityRef.write*Via (nav-wrapper write primitive)"
```

---

## Task 2: The `Weapon` entity object

Create `Weapon` over `CCSWeaponBase` in a new `weapon.js`, register it on the shared `__s2pkg_cs2` namespace, add its types, and concatenate it before `pawn.js`. This task does NOT touch `pawn.js` — so all existing plugins keep compiling unchanged (Pawn's `giveNamedItem`/`weapons` still return `EntityRef` until Task 3).

**Files:**
- Create: `games/cs2/js/weapon.js`
- Create: `packages/cs2/weapon.d.ts`
- Modify: `packages/cs2/index.d.ts` (re-export `Weapon` — add near the other re-exports at the top)
- Modify: `scripts/package-addon.sh` (add `weapon.js` to the concatenation before `pawn.js`, ~line 40-45)

**Interfaces:**
- Consumes: `globalThis.__s2pkg_cs2_schema.applyAccessors` (from `schema.generated.js`); `__s2_remove_player_item`, `__s2require`, `globalThis.__s2pkg_cs2.Pawn` (resolved lazily at call time); the generated `CCSWeaponBase` accessors `ownerEntity` (→ `EntityRef | null`), `clip1`, `fallbackPaintKit`; `EntityRef.isValid()`/`remove()`; `Entity.findByClass(name) -> EntityRef[]` from `@s2script/entity`.
- Produces (runtime): `globalThis.__s2pkg_cs2.Weapon` — constructor `Weapon(ref)`; instance `isValid()`, `paintKit` get/set, `owner` (→ Pawn|null), `setAmmo(clip, reserve?)`, `remove()`; statics `Weapon.fromEntity(ref)`, `Weapon.findAll(className)`.
- Produces (types): `interface Weapon extends CCSWeaponBase` + `declare const Weapon` in `packages/cs2/weapon.d.ts`.

- [ ] **Step 1: Create `games/cs2/js/weapon.js`**

```javascript
// @s2script/cs2 — the Weapon entity object (CCSWeaponBase). CS2 identifiers live ONLY in the CS2 game
// package (never in core). Concatenated by scripts/package-addon.sh AFTER schema.generated.js (which sets
// globalThis.__s2pkg_cs2_schema) and BEFORE pawn.js (whose acquisition getters reference Weapon).
// A weapon IS entity-backed (CCSWeaponBase <- CBaseEntity), so Weapon is EntityRef-backed + serial-gated,
// exactly like Pawn/Player. Cross-refs (weapon.owner -> Pawn) resolve LAZILY via globalThis.__s2pkg_cs2 at
// call time, since pawn.js loads after this file. Offsets are live-resolved by the generated accessors.
(function () {
  var schema = globalThis.__s2pkg_cs2_schema;   // set by schema.generated.js (loaded before this file)

  function Weapon(ref) { this.ref = ref; }
  if (schema) schema.applyAccessors(Weapon.prototype, "CCSWeaponBase");   // clip1, clip2, fallbackPaintKit, ownerEntity, ...

  // weapon.isValid() — serial-gated liveness (delegates to the backing EntityRef).
  Weapon.prototype.isValid = function () { return this.ref.isValid(); };

  // weapon.paintKit — ergonomic alias for the generated fallbackPaintKit (the weapon skin id).
  Object.defineProperty(Weapon.prototype, "paintKit", {
    get: function () { return this.fallbackPaintKit; },
    set: function (v) { this.fallbackPaintKit = v; },
    enumerable: true, configurable: true,
  });

  // weapon.owner — the holding Pawn (m_hOwnerEntity -> a serial-gated EntityRef, wrapped in Pawn). null if
  // unowned (on the ground) / stale. Pawn resolved lazily (pawn.js loads after this file).
  Object.defineProperty(Weapon.prototype, "owner", {
    get: function () {
      var h = this.ownerEntity;   // generated CBaseEntity accessor -> EntityRef | null
      if (!h) return null;
      var Pawn = globalThis.__s2pkg_cs2.Pawn;
      return Pawn ? new Pawn(h) : null;
    },
    enumerable: true, configurable: true,
  });

  // weapon.setAmmo(clip, reserve?) — set the magazine (clip1) via the generated setter. `reserve` is
  // deferred (m_pReserveAmmo layout unverified) — accepted but ignored. Returns false on a stale ref.
  Weapon.prototype.setAmmo = function (clip, reserve) {
    if (!this.ref.isValid()) return false;
    if (typeof clip === "number") this.clip1 = clip;
    return true;
  };

  // weapon.remove() — the complete "take this weapon away" atom: unequip from the owner (RemovePlayerItem)
  // then destroy the entity (UTIL_Remove via EntityRef.remove). Unowned -> just destroy. Serial-gated: a
  // stale weapon is a no-op false. Returns true iff the entity was removed.
  Weapon.prototype.remove = function () {
    if (!this.ref.isValid()) return false;
    var owner = this.owner;
    if (owner) __s2_remove_player_item(owner.ref.index, owner.ref.serial, this.ref.index, this.ref.serial);
    return this.ref.remove();
  };

  // Weapon.fromEntity(ref) — wrap a raw weapon EntityRef; null if ref is null.
  Weapon.fromEntity = function (ref) { return ref ? new Weapon(ref) : null; };

  // Weapon.findAll(className) — every live entity of `className` as a Weapon (e.g. "weapon_ak47").
  Weapon.findAll = function (className) {
    var entity = __s2require("@s2script/entity");
    var refs = (entity && entity.Entity) ? entity.Entity.findByClass(String(className)) : [];
    var out = [];
    for (var i = 0; i < refs.length; i++) out.push(new Weapon(refs[i]));
    return out;
  };

  globalThis.__s2pkg_cs2 = Object.assign({}, globalThis.__s2pkg_cs2, { Weapon: Weapon });
})();
```

- [ ] **Step 2: Create `packages/cs2/weapon.d.ts`**

```typescript
/**
 * @s2script/cs2 — the Weapon entity object (CCSWeaponBase), hand-written on top of the generated field
 * accessors. EntityRef-backed + serial-gated exactly like Pawn/Player. Re-exported from ./index.
 */
import type { EntityRef } from "@s2script/entity";
import type { CCSWeaponBase } from "./schema.generated";
import type { Pawn } from "./index";

/**
 * A CS2 weapon entity (CCSWeaponBase). All generated field accessors (clip1, clip2, fallbackPaintKit,
 * itemDefinitionIndex, ownerEntity, inherited health/teamNum/...) are read+write; a stale weapon reads null.
 */
export interface Weapon extends CCSWeaponBase {
  /** The backing entity ref (escape hatch to the raw EntityRef surface). */
  readonly ref: EntityRef;
  /** Serial-gated liveness. */
  isValid(): boolean;
  /** The weapon skin id (alias of the generated `fallbackPaintKit`). */
  paintKit: number | null;
  /** The holding Pawn (m_hOwnerEntity), or null if unowned (on the ground) / stale. */
  readonly owner: Pawn | null;
  /** Set the magazine (clip1). `reserve` is accepted but deferred (m_pReserveAmmo layout). false if stale. */
  setAmmo(clip: number, reserve?: number): boolean;
  /** Unequip from the owner (RemovePlayerItem) + destroy the entity (UTIL_Remove). true iff removed. */
  remove(): boolean;
}

export declare const Weapon: {
  /** Wrap a raw weapon EntityRef; null if ref is null. */
  fromEntity(ref: EntityRef | null): Weapon | null;
  /** Every live entity of `className` as a Weapon (e.g. "weapon_ak47"). */
  findAll(className: string): Weapon[];
};
```

- [ ] **Step 3: Re-export `Weapon` from `packages/cs2/index.d.ts`**

In `packages/cs2/index.d.ts`, after the existing generated re-exports near the top (after the `export { CsItem } from "./csitem.generated";` line ~line 16), add:

```typescript
export type { Weapon } from "./weapon";
export { Weapon } from "./weapon";
```

- [ ] **Step 4: Add `weapon.js` to the addon concatenation**

In `scripts/package-addon.sh`, find the concatenation (~line 45):

```bash
    cat games/cs2/js/schema.generated.js games/cs2/js/nav.generated.js games/cs2/js/activity.js games/cs2/js/csitem.generated.js games/cs2/js/pawn.js > "$DIST/s2script/js/pawn.js"
```

Change it to insert `weapon.js` immediately before `pawn.js`:

```bash
    cat games/cs2/js/schema.generated.js games/cs2/js/nav.generated.js games/cs2/js/activity.js games/cs2/js/csitem.generated.js games/cs2/js/weapon.js games/cs2/js/pawn.js > "$DIST/s2script/js/pawn.js"
```

Also update the comment block just above it (~lines 40-44) to note `weapon.js` runs after `schema.generated.js` (needs `__s2pkg_cs2_schema`) and before `pawn.js` (which references `Weapon`).

- [ ] **Step 5: Verify the addon assembles**

Run: `bash scripts/package-addon.sh`
Expected: exits 0; `dist/addons/s2script/js/pawn.js` now contains the `Weapon` IIFE (verify: `grep -c "function Weapon(ref)" dist/addons/s2script/js/pawn.js` → `1`).

- [ ] **Step 6: Verify existing plugins/examples still typecheck**

Run: `bash scripts/check-plugins-typecheck.sh`
Expected: PASS — no existing plugin references `Weapon` yet, and Pawn's types are unchanged, so nothing regresses. (Also confirms `weapon.d.ts` has no standalone type errors via the re-export.)

- [ ] **Step 7: Verify the core boundary gate**

Run: `bash scripts/check-core-boundary.sh`
Expected: PASS — no CS2 identifier leaked into `core/src`.

- [ ] **Step 8: Commit**

```bash
git add games/cs2/js/weapon.js packages/cs2/weapon.d.ts packages/cs2/index.d.ts scripts/package-addon.sh
git commit -m "feat(cs2): Weapon entity object (CCSWeaponBase) + concat before pawn.js"
```

---

## Task 3: Pawn ↔ Weapon integration + fire control

Wire `Weapon` into `Pawn`'s acquisition points, refactor `stripWeapons`/`disarm` to fold over `Weapon.remove()`, and add `pawn.blockFiring`/`allowFiring`/`nextAttack` using the Task-1 `writeFloat32Via`. Update the Pawn `.d.ts`.

**Files:**
- Modify: `games/cs2/js/pawn.js` (grab `Weapon` from the shared namespace; rewrite `giveNamedItem`, `weapons`, `removeWeapon`, `stripWeapons`; add `disarm`, `activeWeapon`, `nextAttack`, `blockFiring`, `allowFiring` — the weapon block is ~lines 190-240)
- Modify: `packages/cs2/index.d.ts` (Pawn interface: change `giveNamedItem`/`weapons`/`removeWeapon` types, add `activeWeapon`/`disarm`/`nextAttack`/`blockFiring`/`allowFiring`; import `Weapon`)

**Interfaces:**
- Consumes: `globalThis.__s2pkg_cs2.Weapon` (from Task 2, loaded before pawn.js); `EntityRef.writeFloat32Via`/`readFloat32Via` (Task 1); `__s2_schema_offset`, `__s2_give_named_item`, `EntityRef.readHandleVector`, `__s2require("@s2script/server").Server.gameTime` (existing); the nav getter `pawn.weaponServices.activeWeapon` (→ `EntityRef | null`).
- Produces (Pawn): `activeWeapon: Weapon | null`, `weapons: Weapon[]`, `giveNamedItem(name): Weapon | null`, `removeWeapon(weapon: Weapon): boolean`, `stripWeapons(): boolean`, `disarm(): boolean`, `nextAttack: number | null` (readonly), `blockFiring(seconds?): boolean`, `allowFiring(): boolean`.

- [ ] **Step 1: Bind `Weapon` at the top of the pawn.js IIFE**

In `games/cs2/js/pawn.js`, just after the existing `var schema = globalThis.__s2pkg_cs2_schema;` line (~line 10), add:

```javascript
  var Weapon = globalThis.__s2pkg_cs2.Weapon;   // set by weapon.js (concatenated before this file)
```

- [ ] **Step 2: Rewrite the weapon acquisition + removal methods**

In `games/cs2/js/pawn.js`, replace the existing `giveNamedItem`, `weapons`, `removeWeapon`, and `stripWeapons` definitions (~lines 193-232) with the versions below. Leave the `dropActiveWeapon` stub (~line 240) and its comment unchanged.

```javascript
  // pawn.giveNamedItem(name) — give this pawn a weapon/item by classname (CsItem.AK47 or a raw "weapon_*"
  // string). Returns the created Weapon, or null if the ItemServices ptr is unresolved / failed / stale.
  Pawn.prototype.giveNamedItem = function (name) {
    var off = __s2_schema_offset("CBasePlayerPawn", "m_pItemServices");
    if (off < 0) return null;
    var ref = __s2_give_named_item(this.ref.index, this.ref.serial, off, String(name));
    return ref ? new Weapon(ref) : null;
  };

  // pawn.activeWeapon — the currently-deployed weapon (m_hActiveWeapon on WeaponServices), as a Weapon.
  // null if unresolved / none / stale.
  Object.defineProperty(Pawn.prototype, "activeWeapon", {
    get: function () {
      var ws = this.weaponServices;               // nav wrapper (may be null)
      var h = ws ? ws.activeWeapon : null;        // -> EntityRef | null
      return h ? new Weapon(h) : null;
    },
    enumerable: true, configurable: true,
  });

  // pawn.weapons — this pawn's held weapons (m_hMyWeapons, a CUtlVector<CHandle> on the WeaponServices
  // sub-object), each decoded + serial-gated into a live Weapon. [] if offsets/chain unresolved / stale.
  Object.defineProperty(Pawn.prototype, "weapons", {
    get: function () {
      var wsOff = __s2_schema_offset("CBasePlayerPawn", "m_pWeaponServices");
      var vecOff = __s2_schema_offset("CCSPlayer_WeaponServices", "m_hMyWeapons");
      if (wsOff < 0 || vecOff < 0) return [];
      var refs = this.ref.readHandleVector([wsOff], vecOff, 64);
      var out = [];
      for (var i = 0; i < refs.length; i++) out.push(new Weapon(refs[i]));
      return out;
    },
    enumerable: true, configurable: true,
  });

  // pawn.removeWeapon(weapon) — remove ONE Weapon (delegates to the Weapon.remove atom: unequip via
  // RemovePlayerItem + destroy via UTIL_Remove). false if the weapon is absent/stale.
  Pawn.prototype.removeWeapon = function (weapon) {
    return weapon ? weapon.remove() : false;
  };

  // pawn.stripWeapons() / pawn.disarm() — remove ALL held weapons by folding over Weapon.remove(). `ws` is
  // a snapshot (each Weapon is independent + serial-gated), so mutating m_hMyWeapons mid-loop is safe.
  // Returns true iff every weapon removed.
  Pawn.prototype.stripWeapons = function () {
    var ws = this.weapons;
    var ok = true;
    for (var i = 0; i < ws.length; i++) { if (!ws[i].remove()) ok = false; }
    return ok;
  };
  Pawn.prototype.disarm = function () { return this.stripWeapons(); };   // destroy-all alias
```

- [ ] **Step 3: Add fire control (`nextAttack` / `blockFiring` / `allowFiring`)**

In `games/cs2/js/pawn.js`, immediately after the `dropActiveWeapon` stub (~line 240, before the `moveType` block), add:

```javascript
  // --- Player fire control: the effective "can't fire" gate is m_flNextAttack (a GameTime_t, seconds) on
  // the CCSPlayer_WeaponServices SUB-OBJECT, reached via the m_pWeaponServices pointer. Written through the
  // write-chain primitive (writeFloat32Via). The fire check is server-authoritative, so the raw write blocks
  // the shot — no notifyStateChanged needed. It's a time gate the engine advances past: a durable block is a
  // large `seconds` or a per-OnGameFrame refresh (the caller's policy).
  function fireGateOffsets() {
    var wsOff = __s2_schema_offset("CBasePlayerPawn", "m_pWeaponServices");
    var naOff = __s2_schema_offset("CCSPlayer_WeaponServices", "m_flNextAttack");
    return (wsOff < 0 || naOff < 0) ? null : { ws: wsOff, na: naOff };
  }
  function nowGameTime() {
    var Server = __s2require("@s2script/server").Server;
    var t = Server ? Server.gameTime : 0;
    return (typeof t === "number") ? t : 0;
  }

  // pawn.nextAttack — the current m_flNextAttack (seconds), or null if unresolved/stale. Read companion to
  // blockFiring (verifies the write landed).
  Object.defineProperty(Pawn.prototype, "nextAttack", {
    get: function () {
      var o = fireGateOffsets();
      return o ? this.ref.readFloat32Via([o.ws], o.na) : null;
    },
    enumerable: true, configurable: true,
  });

  // pawn.blockFiring(seconds?) — block ALL weapon fire for `seconds` (default ~effectively-indefinite).
  // Writes m_flNextAttack = gameTime + seconds. Returns false if unresolved/stale.
  Pawn.prototype.blockFiring = function (seconds) {
    var o = fireGateOffsets();
    if (!o) return false;
    var dur = (typeof seconds === "number" && isFinite(seconds)) ? seconds : 1e9;
    return this.ref.writeFloat32Via([o.ws], o.na, nowGameTime() + dur);
  };

  // pawn.allowFiring() — clear the block (m_flNextAttack = now). Returns false if unresolved/stale.
  Pawn.prototype.allowFiring = function () {
    var o = fireGateOffsets();
    if (!o) return false;
    return this.ref.writeFloat32Via([o.ws], o.na, nowGameTime());
  };
```

- [ ] **Step 4: Update the Pawn `.d.ts`**

In `packages/cs2/index.d.ts`: first add a `Weapon` import near the top imports (after `import type { EntityRef } from "@s2script/entity";` ~line 8):

```typescript
import type { Weapon } from "./weapon";
```

Then, inside the `Pawn` interface (~lines 50-68), replace the `giveNamedItem`, `weapons`, `stripWeapons`, and `removeWeapon` member declarations and add the new members, so the block reads:

```typescript
  /** Give this pawn a named item/weapon (e.g. CsItem.AK47 or a raw "weapon_*" string). Returns the created
   *  Weapon, or null if unresolved/failed/stale. */
  giveNamedItem(name: string): Weapon | null;
  /** The currently-deployed weapon (m_hActiveWeapon), or null if none/stale. */
  readonly activeWeapon: Weapon | null;
  /** This pawn's held weapons (m_hMyWeapons, a CUtlVector<CHandle>). Empty if stale/unresolved/none. */
  readonly weapons: Weapon[];
  /** Remove ONE weapon (unequip via RemovePlayerItem + destroy via UTIL_Remove). false if absent/stale. */
  removeWeapon(weapon: Weapon): boolean;
  /** Remove ALL held weapons (folds over Weapon.remove). true iff every one removed. */
  stripWeapons(): boolean;
  /** Alias of stripWeapons — destroy all held weapons. */
  disarm(): boolean;
  /** DEFERRED (always false): a true drop spawns a world pickup, not composable from remove(); needs the
   *  DropActivePlayerWeapon signature-resolve. */
  dropActiveWeapon(): boolean;
  /** The current fire gate (m_flNextAttack, seconds), or null if unresolved/stale. Read companion to blockFiring. */
  readonly nextAttack: number | null;
  /** Block ALL weapon fire for `seconds` (default effectively-indefinite) by writing m_flNextAttack. The
   *  gate is server-authoritative and time-based: a durable block needs a large value or a per-frame refresh.
   *  Returns false if unresolved/stale. */
  blockFiring(seconds?: number): boolean;
  /** Clear a fire block (m_flNextAttack = now). Returns false if unresolved/stale. */
  allowFiring(): boolean;
```

(The existing `dropActiveWeapon` declaration is folded into the block above; delete the old standalone one to avoid a duplicate member.)

- [ ] **Step 5: Verify typecheck (existing plugins adapt to the new Weapon types)**

Run: `bash scripts/check-plugins-typecheck.sh`
Expected: PASS. `examples/items-demo` uses `pawn.giveNamedItem(...).isValid()` (now `Weapon.isValid()` ✓), `pawn.weapons.length` (now `Weapon[].length` ✓), `pawn.stripWeapons()` (✓), `pawn.dropActiveWeapon()` (✓) — all still valid.

- [ ] **Step 6: Verify the addon still assembles + boundary**

Run: `bash scripts/package-addon.sh && bash scripts/check-core-boundary.sh`
Expected: both exit 0.

- [ ] **Step 7: Commit**

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts
git commit -m "feat(cs2): Pawn<->Weapon (activeWeapon/weapons/disarm) + pawn.blockFiring fire control"
```

---

## Task 4: `weapon-demo` plugin + live gate

Add a bot-provable demo exercising the surface, then rebuild core (the new native), redeploy, and verify on the live CS2 server.

**Files:**
- Create: `examples/weapon-demo/package.json`
- Create: `examples/weapon-demo/tsconfig.json`
- Create: `examples/weapon-demo/src/plugin.ts`

**Interfaces:**
- Consumes: `@s2script/commands` (`Commands.register`), `@s2script/cs2` (`Pawn`, `Weapon`), `@s2script/server` (`Server.gameTime`). (Note: `Player.target`/`allConnected` return 0 on this CS2 build — stale client-list offsets — so, like `items-demo`, the demo acts on live pawns via `Pawn.forSlot`.)

- [ ] **Step 1: Create `examples/weapon-demo/package.json`**

```json
{ "name": "@demo/weapon-demo", "version": "0.1.0", "main": "src/plugin.ts", "s2script": { "apiVersion": "1.x" } }
```

- [ ] **Step 2: Create `examples/weapon-demo/tsconfig.json`**

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/globals/globals.d.ts"]
}
```

- [ ] **Step 3: Create `examples/weapon-demo/src/plugin.ts`**

```typescript
// Live-gate demo for the Weapon entity object + pawn fire control (CS2 2000870). Like items-demo, this
// acts on every live pawn (Pawn.forSlot) rather than SM target resolution, since the client-list offsets
// are stale on this build.
import { Commands } from "@s2script/commands";
import { Pawn } from "@s2script/cs2";
import { Server } from "@s2script/server";

const MAX_SLOTS = 12;

function livePawns(): Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> {
  const out: Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> = [];
  for (let slot = 0; slot < MAX_SLOTS; slot++) {
    const pawn = Pawn.forSlot(slot);
    if (pawn) out.push({ slot, pawn });
  }
  return out;
}

Commands.register("sm_wpn", (ctx) => {
  for (const { slot, pawn } of livePawns()) {
    const w = pawn.activeWeapon;
    const active = w ? "ref#" + w.ref.index + " clip1=" + w.clip1 + "/" + w.clip2 + " paint=" + w.paintKit : "none";
    ctx.reply("[wpn] slot=" + slot + " active=" + active + " count=" + pawn.weapons.length);
  }
});

Commands.register("sm_refill", (ctx) => {
  let n = 0;
  for (const { pawn } of livePawns()) {
    const w = pawn.activeWeapon;
    if (w && w.setAmmo(90)) n++;
  }
  ctx.reply("[wpn] refilled clip1=90 on " + n + " active weapon(s)");
});

Commands.register("sm_disarm", (ctx) => {
  let n = 0;
  for (const { pawn } of livePawns()) { if (pawn.disarm()) n++; }
  ctx.reply("[wpn] disarmed " + n + " player(s)");
});

Commands.register("sm_nofire", (ctx) => {
  const secs = ctx.args[0] ? Number(ctx.args[0]) : 5;
  const now = Server.gameTime;
  for (const { slot, pawn } of livePawns()) {
    const ok = pawn.blockFiring(secs);
    ctx.reply("[wpn] slot=" + slot + " blockFiring(" + secs + ")=" + ok + " nextAttack=" + pawn.nextAttack + " gameTime=" + now);
  }
});
```

- [ ] **Step 4: Typecheck the demo**

Run: `bash scripts/check-plugins-typecheck.sh`
Expected: PASS (the new `weapon-demo` compiles under full strict against the updated `@s2script/cs2` types).

- [ ] **Step 5: Build core (sniper) — the new native needs the rebuilt core `.so`**

Run: `bash scripts/build-sniper.sh`
Expected: builds `libs2script_core.so` (GLIBC floors met) with no errors. (No shim change — the write-chain is pure core.)

- [ ] **Step 6: Assemble the addon + build the demo `.s2sp`**

Run:
```bash
bash scripts/package-addon.sh
node packages/cli/dist/cli.js build examples/weapon-demo
```
Expected: `package-addon.sh` exits 0; the CLI emits `examples/weapon-demo/dist/*.s2sp` (typecheck-gated). Copy the built `.s2sp` into the deployed plugins dir (same location the other demos deploy to, e.g. `dist/addons/s2script/plugins/`).

- [ ] **Step 7: Deploy + restart the CS2 container**

Run: `docker compose restart cs2` (NOT `--force-recreate` — that resets `gameinfo.gi`; a plain restart re-binds the addon and preserves the Metamod patch). If the boot shows a stale gameinfo, re-run `docker exec s2script-cs2 /patch-gameinfo.sh` then restart.

- [ ] **Step 8: Verify boot is clean**

Run: `docker logs s2script-cs2 --since 60s 2>&1 | grep -E "GAMEDATA VALIDATION|weapon-demo|RestartCount"`
Expected: `=== GAMEDATA VALIDATION: N ok, 0 FAILED ===` (unchanged signature count — no new sigs), the demo's commands register, `RestartCount=0`, no panic/abort.

- [ ] **Step 9: Live-gate the surface via rcon (bots present, e.g. `bot_quota 2`)**

Run each and read the reply/log:
```bash
python3 scripts/rcon.py "bot_quota 2"
python3 scripts/rcon.py "sm_wpn"      # active weapon reads: def/clip1/paint + weapons count > 0
python3 scripts/rcon.py "sm_refill"   # clip1 <- 90
python3 scripts/rcon.py "sm_wpn"      # read-back: clip1=90 STICKS (proves the write landed on a live pawn)
python3 scripts/rcon.py "sm_nofire 5" # blockFiring=true AND nextAttack ~= gameTime+5 (proves write_chain round-trips)
python3 scripts/rcon.py "sm_disarm"   # disarm returns per-player
python3 scripts/rcon.py "sm_wpn"      # weapons count -> 0 (disarm worked)
```
Expected: `sm_wpn` shows a valid weapon + count > 0; after `sm_refill` the follow-up `sm_wpn` shows `clip1=90`; `sm_nofire 5` shows `blockFiring(5)=true nextAttack≈gameTime+5` (the write-chain primitive proven end-to-end on a live pawn); after `sm_disarm` the count is `0`. Server keeps ticking, `RestartCount=0`, no crash.

**Deferred (human-client only — document in the reply/notes, do not block):** *visually* confirming a reskin renders and that a real client genuinely cannot fire while blocked — same ceiling as SayText2/damage (bots don't render/shoot reliably). The read-backs above prove the mechanism.

- [ ] **Step 10: Commit**

```bash
git add examples/weapon-demo
git commit -m "test(cs2): weapon-demo live gate (activeWeapon/refill/disarm/blockFiring)"
```

---

## Self-Review

**Spec coverage:**
- `Weapon` object (EntityRef-backed, generated accessors, `.ref`, `owner`, `remove`, `setAmmo`, `paintKit`, `fromEntity`, `findAll`) → Task 2. ✓
- `isValid()` on Weapon (needed by `items-demo`'s `giveNamedItem(...).isValid()`) → Task 2 Step 1. ✓
- Pawn ↔ Weapon (acquisition wrapping, `disarm` folds over `Weapon.remove`) → Task 3. ✓
- Fire control (`pawn.blockFiring`/`allowFiring`) + the write-chain primitive (`__s2_ent_ref_write_chain` + `write*Via`) → Task 1 (primitive) + Task 3 (Pawn API). ✓ Added `pawn.nextAttack` read companion (not in spec) so the write is bot-verifiable — a minor, in-spirit addition.
- Packaging (home `@s2script/cs2`, `weapon.js` concatenated before `pawn.js`, `weapon.d.ts`) → Task 2. ✓
- Safety (serial-gated, no raw pointer, degrade-never-crash) → Task 1 native guards + Task 2/3 `isValid` gating. ✓
- Testing (in-isolate core degrade + bot-provable live gate + human-client deferral) → Task 1 tests + Task 4. ✓
- Deferred items (dropActiveWeapon, onFire detour, getWeapon/className, reserveAmmo, nametag, navgen setter codegen) → left as-is / documented; `dropActiveWeapon` stub untouched. ✓

**Placeholder scan:** No TBD/TODO; every code step shows the full code; commands have expected output. ✓

**Type consistency:** `Weapon` (interface + `declare const`) consistent across `weapon.d.ts` (Task 2) and its use in the Pawn interface (Task 3). `writeFloat32Via(pathOffs, finalOff, value)` signature identical in the native wrapper (Task 1 Step 5), the `.d.ts` (Task 1 Step 6), and the call site (`this.ref.writeFloat32Via([o.ws], o.na, ...)`, Task 3 Step 3). `Weapon.remove()`/`isValid()`/`setAmmo(clip, reserve?)` names match between `weapon.js`, `weapon.d.ts`, and the demo. `pawn.disarm()`/`blockFiring(seconds?)`/`nextAttack` match between `pawn.js`, the Pawn `.d.ts`, and the demo. ✓

**Deviation noted:** the spec suggested a separate `weapon.d.ts` re-exported (followed) — but the runtime `Weapon`↔`Pawn` cycle is resolved via the established lazy `globalThis.__s2pkg_cs2` lookup (idiomatic here), not a new mechanism.
