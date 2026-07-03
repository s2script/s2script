# Slice 5C.5 — Curated pointer-chain field navigation (codegen) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generalize 5C.4's hand-written `pawn.origin` into codegen over a curated `nav-targets.json`, so `pawn.sceneNode`/`weaponServices`/`movementServices`/`aimPunchServices` are typed pointer-backed wrappers whose fields read through the pointer chain, serial-gated at the root entity.

**Architecture:** One new `__s2_ent_ref_read_chain(idx, serial, pathOffs[], finalOff, kind)` native (the `KIND_*` scalar dispatch + a pointer-chain deref) + `EntityRef.*Via` methods; vectors reuse 5C.4's `readFloatsChain`; handles via `readUInt32Via` + `__s2_handle_decode`. A `navgen` codegen (reusing the schemagen `classifyField`/`TSTYPE`) emits pointer-backed wrappers + nav accessors. Wrappers hold `(rootEntityRef, pathOffsets)` and re-resolve per access — never a cached pointer. Touches core → one sniper rebuild.

**Tech Stack:** Rust `cdylib` core (rusty_v8 149.4.0), the injected JS prelude, the TS codegen (`packages/cli`), `node:test`, the Docker CS2 live gate.

## Global Constraints

Every task's requirements implicitly include these (spec §11):

- **Core stays engine-generic.** The `__s2_ent_ref_read_chain` native + `EntityRef.*Via` are engine-generic; the nav-targets config + wrappers + CS2 field names live ONLY in `games/cs2` + `packages/cs2`. NO CS2 identifiers in `core/src`. Both gates green: `bash scripts/check-core-boundary.sh`, `bash scripts/test-boundary-nameleak.sh`.
- **Never expose a raw pointer across time.** The chain is followed + read within ONE synchronous native; wrappers hold a `(rootEntityRef, pathOffsets)` VALUE, not a pointer; every access re-resolves the chain serial-gated at the root → `T | null`.
- **Layout is data.** Every offset (path + field) resolves live via `__s2_schema_offset`; nothing baked.
- **Deterministic codegen + freshness gate.** Same config+catalog → byte-identical `nav.generated.{js,d.ts}`.
- **cdylib:** core in-isolate tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase wrappers/types (`SceneNode`, `WeaponServices`), camelCase props/methods (`sceneNode`, `activeWeapon`, `readHandleVia`).
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5c5-ptr-nav-codegen`; do NOT push.

**Deferred — do NOT build:** graph auto-traversal / non-curated targets; deeper-than-curated or cyclic chains; `CUtlVector`/array fields (`m_hMyWeapons`); pointer-chain **writes**; string-via (no curated target needs a `char[N]` through a chain); recursive wrapper-to-wrapper nav (one hop of wrappers only); the engine-RE bundle (next slice); the `tsc` gate; the registry (5.5); the base suite (6).

**Test runners:** core = `cargo test -p s2script-core -- --test-threads=1`; CLI = `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs` (scoped glob).

---

## Task 1: `__s2_ent_ref_read_chain` native + `EntityRef.*Via` methods (cargo-in-isolate)

**Files:**
- Modify: `core/src/v8host.rs` (the native + install + the `EntityRef` prelude methods + an in-isolate test), `packages/entity/index.d.ts`

**Interfaces:**
- Consumes: `entity_resolve_ptr`, `crate::entity::read_ptr`/`read_i32`/`read_f32`/etc., the `KIND_*` consts (I32=1…F64=11), `__s2_handle_decode`, `v8::Local::<v8::Array>::try_from`.
- Produces (for T3's generated code): `__s2_ent_ref_read_chain(index, serial, pathOffs, finalOff, kind) → value | null`; `EntityRef.readInt32Via`/`readInt8Via`/`readInt16Via`/`readUInt8Via`/`readUInt16Via`/`readUInt32Via`/`readFloat32Via`/`readBoolVia` (→ number/boolean); `readUInt64Via`/`readInt64Via` (→ bigint); `readHandleVia` (→ `EntityRef | null`).

- [ ] **Step 1: Write the failing test** (add to `#[cfg(test)] mod frame_tests`):

```rust
    #[test]
    fn read_chain_native_and_via_methods_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the native degrades to null (no ops → entity_resolve_ptr null):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,[48],200,1))"), "null");   // KIND_I32
        // guards (fire before the resolve): non-array path, negative finalOff, bad kind:
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,42,200,1))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,[48],-1,1))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,[48],200,999))"), "null");
        // the EntityRef via-methods degrade:
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readInt32Via([48],200))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readHandleVia([48],200))"#), "null");
        shutdown();
    }
```
(Mirror the neighboring `read_floats_chain_degrades_without_ops` harness if `eval_in_context_string` differs.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p s2script-core frame_tests::read_chain_native_and_via_methods_degrade_without_ops -- --test-threads=1` → FAIL.

- [ ] **Step 3: Add the native** (near `s2_ent_ref_read_floats_chain` in `v8host.rs`). It is that native's deref loop + `s2_ent_ref_read`'s `match kind` for the final read:

```rust
/// Native `__s2_ent_ref_read_chain(index, serial, pathOffs, finalOff, kind) -> value | null`. Follows a chain
/// of pointer derefs (each i32 offset in `pathOffs`), then reads a SCALAR of `kind` at `finalOff`. Serial-gated
/// at the root; each hop null-checked; the raw intermediate pointers never cross to JS. Vectors use
/// __s2_ent_ref_read_floats_chain; handles = read KIND_U32 here then __s2_handle_decode in JS.
fn s2_ent_ref_read_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
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
        let off = final_off;
        match kind {
            KIND_I32  => rv.set_int32(crate::entity::read_i32(p, off)),
            KIND_F32  => rv.set_double(crate::entity::read_f32(p, off) as f64),
            KIND_BOOL => rv.set_bool(crate::entity::read_bool(p, off)),
            KIND_I8   => rv.set_int32(crate::entity::read_i8(p, off)),
            KIND_I16  => rv.set_int32(crate::entity::read_i16(p, off)),
            KIND_U8   => rv.set_double(crate::entity::read_u8(p, off) as f64),
            KIND_U16  => rv.set_double(crate::entity::read_u16(p, off) as f64),
            KIND_U32  => rv.set_double(crate::entity::read_u32(p, off) as f64),
            KIND_U64  => { let bi = v8::BigInt::new_from_u64(scope, crate::entity::read_u64(p, off)); rv.set(bi.into()); }
            KIND_I64  => { let bi = v8::BigInt::new_from_i64(scope, crate::entity::read_i64(p, off)); rv.set(bi.into()); }
            KIND_F64  => rv.set_double(crate::entity::read_f64(p, off)),
            _ => { }   // unknown kind → leave null
        }
    }));
}
```
(If `read_i16`/`read_u16` etc. differ, use whatever `s2_ent_ref_read`'s arms call — copy that native's arms verbatim.)

- [ ] **Step 4: Register the native.** In `install_natives`, next to `__s2_ent_ref_read_floats_chain`:
`set_native(scope, global_obj, "__s2_ent_ref_read_chain", s2_ent_ref_read_chain);`

- [ ] **Step 5: Add the `EntityRef` prelude methods.** In `INJECTED_STD_PRELUDE`, on `EntityRef.prototype` (next to `readFloatsChain`). Reuse the `K` kind map already in the prelude:

```js
    readInt32Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I32); },
    readInt8Via:   function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I8); },
    readInt16Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I16); },
    readUInt8Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U8); },
    readUInt16Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U16); },
    readUInt32Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U32); },
    readFloat32Via:function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.F32); },
    readBoolVia:   function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.BOOL); },
    readUInt64Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U64); },
    readInt64Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I64); },
    // a handle field: read the u32 handle through the chain, then decode to a serial-gated EntityRef.
    readHandleVia: function (c, o) { var h = __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U32);
      if (h === null) return null; var d = __s2_handle_decode(h >>> 0); return new EntityRef(d[0], d[1]); },
```
(Confirm `K` has `I32/I8/I16/U8/U16/U32/F32/BOOL/U64/I64` — it was defined in 5B.2/5B.4. Confirm `__s2_handle_decode` returns `[index, serial]` as the 5A/5C.2 `readHandle` uses it.)

- [ ] **Step 6: Update `packages/entity/index.d.ts`** — add to the `EntityRef` class:

```ts
  /** Follow a pointer chain (each an offset), then read a scalar at `finalOff`. null if the root is stale or any
   *  hop is null. `readHandleVia` decodes a handle field → a serial-gated EntityRef; vectors use readFloatsChain. */
  readInt32Via(pathOffs: number[], finalOff: number): number | null;
  readInt8Via(pathOffs: number[], finalOff: number): number | null;
  readInt16Via(pathOffs: number[], finalOff: number): number | null;
  readUInt8Via(pathOffs: number[], finalOff: number): number | null;
  readUInt16Via(pathOffs: number[], finalOff: number): number | null;
  readUInt32Via(pathOffs: number[], finalOff: number): number | null;
  readFloat32Via(pathOffs: number[], finalOff: number): number | null;
  readBoolVia(pathOffs: number[], finalOff: number): boolean | null;
  readUInt64Via(pathOffs: number[], finalOff: number): bigint | null;
  readInt64Via(pathOffs: number[], finalOff: number): bigint | null;
  readHandleVia(pathOffs: number[], finalOff: number): EntityRef | null;
```

- [ ] **Step 7: Run + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/v8host.rs packages/entity/index.d.ts
git commit -m "feat(slice5c5): __s2_ent_ref_read_chain native + EntityRef.*Via (scalar/handle read through a pointer chain)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: `VectorWS`→VEC + the `navgen` codegen (node:test)

**Files:**
- Modify: `packages/cli/src/schemagen/model.ts` (add `VectorWS` to `VEC`); regenerate `games/cs2/js/schema.generated.js` + `packages/cs2/schema.generated.d.ts` if it changes
- Create: `packages/cli/src/navgen/{model.ts,emit-js.ts,emit-dts.ts,gen.ts}`, `packages/cli/test/navgen.test.mjs`

**Interfaces:**
- Consumes: schemagen's `classifyField`, `idiomaticName`, `TSTYPE`, the catalog `Catalog` type; T1's `EntityRef.*Via`.
- Produces (for T3): `buildNavModel(config, catalog)`, `emitNavJs(model)`, `emitNavDts(model)`, `runGenNav({check})`.

- [ ] **Step 1: `VectorWS`→VEC + regenerate.** In `packages/cli/src/schemagen/model.ts`, change
`const VEC: Record<string, AccessorKind> = { Vector: "vector", QAngle: "qangle" };` to add `VectorWS: "vector",`. Then:

```bash
cd packages/cli && node build.mjs && cd .. && node packages/cli/dist/cli.js gen-schema
bash scripts/check-schema-generated.sh   # regenerate committed schema.generated.* (VectorWS side-effect)
```
Inspect the `.d.ts` diff (likely small/empty — few direct `VectorWS` fields on curated schema classes; that's fine). Commit later with T2.

- [ ] **Step 2: Write the failing navgen tests** (`packages/cli/test/navgen.test.mjs`). The config schema: `{ prop, wrapper, target, path: [{cls, field}, …] }`.

```js
import { test } from "node:test";
import assert from "node:assert";
import { buildNavModel } from "../src/navgen/model.ts";
import { emitNavJs } from "../src/navgen/emit-js.ts";
import { emitNavDts } from "../src/navgen/emit-dts.ts";

const CAT = {
  CCSPlayerPawn: { parent: "CBaseEntity", fields: [] },
  CBaseEntity: { parent: null, fields: [{ name: "m_CBodyComponent", offset: 48, type: { kind: "ptr", inner: "CBodyComponent" } }] },
  CBodyComponent: { parent: null, fields: [{ name: "m_pSceneNode", offset: 8, type: { kind: "ptr", inner: "CGameSceneNode" } }] },
  CGameSceneNode: { parent: null, fields: [
    { name: "m_flScale", offset: 160, type: { kind: "atomic", name: "float32" } },
    { name: "m_vecAbsOrigin", offset: 200, type: { kind: "atomic", name: "VectorWS" } },
    { name: "m_bDormant", offset: 228, type: { kind: "atomic", name: "bool" } },
    { name: "m_pParent", offset: 56, type: { kind: "ptr", inner: "CGameSceneNode" } },   // skipped (ptr)
  ] },
};
const CONFIG = [{ prop: "sceneNode", wrapper: "SceneNode", target: "CGameSceneNode", source: "CCSPlayerPawn",
  path: [{ cls: "CBaseEntity", field: "m_CBodyComponent" }, { cls: "CBodyComponent", field: "m_pSceneNode" }] }];

test("buildNavModel builds a wrapper's readable fields (scalars+vector; skips ptr)", () => {
  const m = buildNavModel(CONFIG, CAT);
  const w = m.wrappers.find(x => x.wrapper === "SceneNode");
  const props = w.fields.map(f => f.propName).sort();
  assert.deepEqual(props, ["dormant", "origin", "scale"]);   // m_pParent (ptr) skipped
  assert.equal(w.fields.find(f => f.propName === "scale").accessorKind, "f32");
  assert.equal(w.fields.find(f => f.propName === "origin").accessorKind, "vector");
});

test("emitNavJs: wrapper getters read via the chain; nav accessor resolves the path", () => {
  const js = emitNavJs(buildNavModel(CONFIG, CAT));
  assert.match(js, /function SceneNode\(root, path\)/);
  assert.match(js, /this\.root\.readFloat32Via\(this\.path, off\("CGameSceneNode","m_flScale"\)\)/);
  assert.match(js, /var a = this\.root\.readFloatsChain\(this\.path, off\("CGameSceneNode","m_vecAbsOrigin"\), 3\); return a === null \? null : new Vector/);
  // the nav accessor + its per-hop path resolution:
  assert.match(js, /off\("CBaseEntity","m_CBodyComponent"\)/);
  assert.match(js, /off\("CBodyComponent","m_pSceneNode"\)/);
  assert.match(js, /globalThis\.__s2pkg_cs2_nav = \{ applyNav/);
});

test("emitNavDts: a wrapper interface + the nav prop type", () => {
  const dts = emitNavDts(buildNavModel(CONFIG, CAT));
  assert.match(dts, /export interface SceneNode \{/);
  assert.match(dts, /readonly scale: number \| null;/);
  assert.match(dts, /readonly origin: Vector \| null;/);
  // (the nav prop `sceneNode: SceneNode | null` is declared on Pawn in index.d.ts by T3, not here.)
});
```

- [ ] **Step 3: Implement `navgen/model.ts`** (pure). `buildNavModel(config, catalog)` → `{ wrappers: [{ wrapper, target, source, prop, path: [{cls,field}], fields: FieldDescriptor[] }] }`. **Reuse schemagen's `buildModel(catalog, targetClasses)` + `flattenedFields(model, target)`** to get each target class's readable fields WITH the inheritance walk + the propName-collision→raw handling already implemented there — do NOT reimplement the walk. (Call `buildModel(catalog, [all distinct target classes])` once, then `flattenedFields(model, entry.target)` per config entry.) The `FieldDescriptor.accessorKind` from that already reflects `classifyField` (so `VectorWS`/`Vector`→`vector`, `QAngle`→`qangle`, handle→`handle`, scalars→their kind; ptr/embedded/CUtl/str/enum are skipped-with-reason and absent from `flattenedFields`). Sort wrappers by `wrapper` for determinism (fields already come sorted from `flattenedFields`).

- [ ] **Step 4: Implement `navgen/emit-js.ts`** (pure). Header + `var off = __s2_schema_offset;` + `var Vector = __s2require("@s2script/math").Vector;` + `var QAngle = __s2require("@s2script/math").QAngle;`. Per wrapper: `function <Wrapper>(root, path) { this.root = root; this.path = path; }` + `Object.defineProperties(<Wrapper>.prototype, { … })` where each field's getter is:
  - scalar (`f32`/`bool`/`i8`/`i16`/`i32`/`u8`/`u16`/`u32`): `get: function () { return this.root.<READ_VIA[kind]>(this.path, off("<target>","<raw>")); }`
  - `u64`/`i64`: `get: function () { var v = this.root.<readUInt64Via|readInt64Via>(this.path, off(...)); return v === null ? null : v.toString(); }` (string, the 5B.4 rule)
  - `vector`/`qangle`: `get: function () { var a = this.root.readFloatsChain(this.path, off(...), 3); return a === null ? null : new <Vector|QAngle>(a[0],a[1],a[2]); }`
  - `handle`: `get: function () { return this.root.readHandleVia(this.path, off(...)); }`
  Use a `READ_VIA` map `{ f32:"readFloat32Via", bool:"readBoolVia", i8:"readInt8Via", i16:"readInt16Via", i32:"readInt32Via", u8:"readUInt8Via", u16:"readUInt16Via", u32:"readUInt32Via" }`. Then a `NAV` table keyed by source class → `[{ prop, path:[{cls,field}], wrapper }]`, and an `applyNav(proto, className)` that, per nav entry, defines a getter resolving each hop's offset (`var o = off(hop.cls, hop.field); if (o < 0) return null; path.push(o);`) then `return new <Wrapper>(this.ref, path);`. Expose `globalThis.__s2pkg_cs2_nav = { applyNav: applyNav };`.

- [ ] **Step 5: Implement `navgen/emit-dts.ts`** (pure). `import type { Vector, QAngle } from "@s2script/math"; import type { EntityRef } from "@s2script/entity";` (only those actually used). Per wrapper: `export interface <Wrapper> { <readonly propName: TSTYPE[kind]>; … }` (reuse `TSTYPE`; `vector`→`Vector | null`, `qangle`→`QAngle | null`, `handle`→`EntityRef | null`, `u64`/`i64`→`string | null`).

- [ ] **Step 6: Implement `navgen/gen.ts`** (mirror `schemagen/gen.ts`): `runGenNav({check})` reads `games/cs2/nav-targets.json` + `games/cs2/gamedata/schema-catalog.json` → `emitNavJs`/`emitNavDts` → writes `games/cs2/js/nav.generated.js` + `packages/cs2/nav.generated.d.ts`; `--check` regenerates + compares. (Wire the `gen-nav` CLI command in T3.)

- [ ] **Step 7: Run the navgen tests** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/navgen.test.mjs` → PASS.

- [ ] **Step 8: Build + commit**

```bash
cd /home/gkh/projects/s2script/packages/cli && node build.mjs
cd /home/gkh/projects/s2script && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add packages/cli/src/schemagen/model.ts packages/cli/src/navgen packages/cli/test/navgen.test.mjs \
        games/cs2/js/schema.generated.js packages/cs2/schema.generated.d.ts
git commit -m "feat(slice5c5): VectorWS->VEC + the navgen codegen (pointer-backed wrappers + nav accessors)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: `nav-targets.json` + generate + wire + freshness gate (node:test)

**Files:**
- Create: `games/cs2/nav-targets.json`, `games/cs2/js/nav.generated.js` (generated), `packages/cs2/nav.generated.d.ts` (generated), `scripts/check-nav-generated.sh`
- Modify: `packages/cli/src/cli.ts` (a `gen-nav` command), `games/cs2/js/pawn.js` (applyNav + compat aliases), `packages/cs2/index.d.ts` (the nav props on `Pawn`), `scripts/package-addon.sh` (concatenate `nav.generated.js`), `packages/cli/test/schema-runtime.test.mjs` (a vm-compose test)

**Interfaces:**
- Consumes: T2's `runGenNav`; T1's `EntityRef.*Via`; the 5C.3 `@s2script/math`.

- [ ] **Step 1: `nav-targets.json`** — the 4 curated targets (spec §6). Paths are `{cls, field}` per hop (each hop's class = the type you read the field from):

```json
[
  { "prop": "sceneNode", "wrapper": "SceneNode", "source": "CCSPlayerPawn", "target": "CGameSceneNode",
    "path": [ {"cls":"CBaseEntity","field":"m_CBodyComponent"}, {"cls":"CBodyComponent","field":"m_pSceneNode"} ] },
  { "prop": "weaponServices", "wrapper": "WeaponServices", "source": "CCSPlayerPawn", "target": "CCSPlayer_WeaponServices",
    "path": [ {"cls":"CBasePlayerPawn","field":"m_pWeaponServices"} ] },
  { "prop": "movementServices", "wrapper": "MovementServices", "source": "CCSPlayerPawn", "target": "CCSPlayer_MovementServices",
    "path": [ {"cls":"CBasePlayerPawn","field":"m_pMovementServices"} ] },
  { "prop": "aimPunchServices", "wrapper": "AimPunchServices", "source": "CCSPlayerPawn", "target": "CCSPlayer_AimPunchServices",
    "path": [ {"cls":"CCSPlayerPawn","field":"m_pAimPunchServices"} ] }
]
```

- [ ] **Step 2: Wire `gen-nav` + generate.** Add `gen-nav` to `packages/cli/src/cli.ts` (mirror `gen-schema`/`gen-events`; call `runGenNav`). Then:

```bash
cd packages/cli && node build.mjs && cd .. && node packages/cli/dist/cli.js gen-nav
```
Inspect `packages/cs2/nav.generated.d.ts`: `SceneNode` (origin/angles/scale/dormant/…), `WeaponServices` (activeWeapon: `EntityRef | null`, …), `MovementServices` (ducked/…), `AimPunchServices` (the QAngles). Confirm `nav.generated.js` has the wrapper ctors + `applyNav` + the `__s2require("@s2script/math")` requires.

- [ ] **Step 3: `pawn.js` — applyNav + compat aliases.** In `games/cs2/js/pawn.js`, after `schema.applyAccessors(Pawn.prototype, "CCSPlayerPawn")`, add:

```js
  var nav = globalThis.__s2pkg_cs2_nav;   // set by nav.generated.js (concatenated ahead of pawn.js)
  if (nav) nav.applyNav(Pawn.prototype, "CCSPlayerPawn");   // sceneNode, weaponServices, movementServices, aimPunchServices
```
Replace the 5C.4 hand-written `origin`/`angles` defineProperty blocks with thin compat aliases (they now delegate to the generated `sceneNode` wrapper):

```js
  Object.defineProperty(Pawn.prototype, "origin", {
    get: function () { var s = this.sceneNode; return s ? s.origin : null; }, enumerable: true, configurable: true,
  });
  Object.defineProperty(Pawn.prototype, "angles", {
    get: function () { var s = this.sceneNode; return s ? s.angles : null; }, enumerable: true, configurable: true,
  });
```

- [ ] **Step 4: `package-addon.sh` — concatenate `nav.generated.js`.** The addon `pawn.js` is `schema.generated.js` + `pawn.js`; insert `nav.generated.js` too (it must precede `pawn.js`, since `pawn.js` reads `__s2pkg_cs2_nav`). Change the concatenation line to `cat games/cs2/js/schema.generated.js games/cs2/js/nav.generated.js games/cs2/js/pawn.js > "$DIST/s2script/js/pawn.js"`.

- [ ] **Step 5: `packages/cs2/index.d.ts` — the nav props on `Pawn`.** Add `export type { SceneNode, WeaponServices, MovementServices, AimPunchServices } from "./nav.generated";` + import them, and add to the `Pawn` interface:

```ts
  /** The pawn's scene node (world transform) — origin/angles/scale, via the CBodyComponent->CGameSceneNode chain. */
  readonly sceneNode: SceneNode | null;
  /** The pawn's weapon services (active weapon, …). */ readonly weaponServices: WeaponServices | null;
  /** The pawn's movement services (duck/ladder/…). */ readonly movementServices: MovementServices | null;
  /** The pawn's aim-punch services (recoil angles). */ readonly aimPunchServices: AimPunchServices | null;
```
(`origin`/`angles` stay declared on `Pawn` as before — the aliases keep them working.)

- [ ] **Step 6: The vm-compose test** (append to `packages/cli/test/schema-runtime.test.mjs`). Extend the stub `EntityRef` with the `*Via` + `readFloatsChain` methods + `__s2require("@s2script/math")`; eval `schema.generated.js` + `nav.generated.js` + `pawn.js`; assert `pawn.sceneNode` is a `SceneNode` with `scale`/`origin`; `pawn.weaponServices.activeWeapon` is an `EntityRef`; a stub returning null for a hop → `pawn.sceneNode === null`. (Mirror the existing vm tests; the stub `readFloatsChain`/`readHandleVia`/`readFloat32Via` return fixed values; `__s2_schema_offset` returns ≥0 for the path + fields.)

- [ ] **Step 7: The freshness gate.** Create `scripts/check-nav-generated.sh` (mirror `check-schema-generated.sh`: build the CLI, `gen-nav`, `git diff --exit-code` on `games/cs2/js/nav.generated.js` + `packages/cs2/nav.generated.d.ts`). Run it → PASS.

- [ ] **Step 8: Full CLI suite + gates + commit**

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-nav-generated.sh && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add games/cs2/nav-targets.json games/cs2/js/nav.generated.js packages/cs2/nav.generated.d.ts \
        packages/cli/src/cli.ts games/cs2/js/pawn.js packages/cs2/index.d.ts scripts/package-addon.sh \
        scripts/check-nav-generated.sh packages/cli/test/schema-runtime.test.mjs
git commit -m "feat(slice5c5): nav-targets.json (4 curated targets) + generate + wire (applyNav, compat aliases, freshness gate)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: Sniper build + live gate + README/CLAUDE (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`; **create** a dated spike-findings doc.

**Needs ONE sniper rebuild** (Task 1 added a core native).

- [ ] **Step 1: Sniper build.** `docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh` (GLIBC ≤ 2.31). Then `bash scripts/package-addon.sh` (picks up the concatenated `nav.generated.js` + `pawn.js`), and **re-copy the demo `.s2sp` into `dist/addons/s2script/plugins/` AFTER `package-addon.sh`** (it `rm -rf`s the addon dir — a known gotcha).

- [ ] **Step 2: The demo** (`examples/demo-plugin/src/plugin.ts`):

```ts
import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

let ticks = 0;
export function onLoad(): void {
  console.log("[demo] onLoad (ptr-nav wrappers)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    for (const p of Player.all()) {
      const body = p.pawn; if (!body) continue;
      const sn = body.sceneNode;
      const ws = body.weaponServices;
      const mv = body.movementServices;
      console.log("  slot=" + p.slot
        + " origin=" + (sn && sn.origin ? sn.origin.toString() : "null")
        + " scale=" + (sn ? sn.scale : "null")
        + " activeWeapon=" + (ws && ws.activeWeapon ? ("ref#" + ws.activeWeapon.index) : "null")
        + " ducked=" + (mv ? mv.ducked : "null"));
    }
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```
Build the `.s2sp`, deploy, restart, wait past the boot window, `bot_quota 2`, read `docker logs s2script-cs2 | grep '[demo]'`. **Expect:** `origin` a plausible de_inferno coord (matches 5C.4), `scale`≈1, `activeWeapon` a valid `ref#<index>` (bots hold a weapon), `ducked=false`. On `bot_kick` → the players drop, server ticking, no crash. If a wrapper reads garbage/null unexpectedly, HALT + diagnose (the path `{cls,field}` hops or the `*Via` native). Record findings in `docs/superpowers/specs/2026-07-03-slice-5c5-spike-findings.md`.

- [ ] **Step 3: README + CLAUDE.**
  - `README.md`: a `## Pointer-chain wrappers (Slice 5C.5)` section — the curated `nav-targets.json`, pointer-backed wrappers (`pawn.sceneNode`/`weaponServices`/…), the `readChain`/`*Via` primitive, the re-resolve-per-access safety, the compat aliases (`pawn.origin` → `pawn.sceneNode.origin`), and the captured live log. Note auto-traversal/cyclic chains/CUtlVector/writes deferred.
  - `CLAUDE.md` "## Current state": Slice 5C.5 done (curated ptr-nav codegen — `__s2_ent_ref_read_chain` + `EntityRef.*Via`; `navgen` over `nav-targets.json` → pointer-backed wrappers `pawn.sceneNode`/`weaponServices`/`movementServices`/`aimPunchServices`, re-resolve-per-access serial-gated; 5C.4 `origin`/`angles` now aliases; `VectorWS`→vector). "Current focus" → the engine-RE bundle (5D.1b event-manager signature + engine-identity — feasibility-risky). Do NOT alter the standing conventions.

- [ ] **Step 4: Final verification + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-nav-generated.sh && bash scripts/check-schema-generated.sh && bash scripts/check-events-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md docs/superpowers/specs/2026-07-03-slice-5c5-spike-findings.md
git commit -m "feat(slice5c5): live gate PASSED — pawn.sceneNode/weaponServices/movementServices wrappers; spike + README + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §9)

1. `cargo test -p s2script-core` green (the chain native); the CLI `node:test` suite green (navgen + vm-compose); both boundary gates + all freshness gates (schema/events/nav) green; sniper build clean.
2. `s2script gen-nav` deterministically generates the wrappers + nav accessors from `nav-targets.json`; freshness-gated.
3. Live gate: `pawn.sceneNode.origin`/`scale`, `pawn.weaponServices.activeWeapon` (`EntityRef`), `pawn.movementServices.ducked` read through the chains; `null` on disconnect; server ticking, no crash.
4. README + CLAUDE updated.
