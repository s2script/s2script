# Slice 5C.3 — Vector value type + direct Vector/QAngle field codegen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce an engine-generic `Vector`/`QAngle` value type (`@s2script/math`) + a serial-gated float-triple read primitive, and extend the codegen so direct atomic `Vector`/`QAngle` fields (`pawn.eyeAngles`, `pawn.absVelocity`) become generated accessors returning copied `{x,y,z}` snapshots.

**Architecture:** The `Vector`/`QAngle` value classes live in the core prelude (`INJECTED_STD_PRELUDE`) as `__s2pkg_math` — engine-generic, alongside `entity`/`frame`/…. A new `__s2_ent_ref_read_floats(idx,serial,off,count)` native (over the existing pure `entity::read_f32`) gives `EntityRef.readFloats(off,count) → number[]|null` in one serial-gated lookup. The codegen adds `vector`/`qangle` kinds; the **generated getter** (game layer) constructs the value type via its own `__s2require("@s2script/math")` — so `@s2script/entity` stays independent of `@s2script/math`. Touches core → one sniper rebuild.

**Tech Stack:** Rust `cdylib` core (rusty_v8 149.4.0), the injected JS prelude, the TypeScript/esbuild codegen (`packages/cli`), `node:test`, the Docker CS2 live gate.

## Global Constraints

Every task's requirements implicitly include these (spec §9):

- **Core stays engine-generic.** The `Vector`/`QAngle` value types + `readFloats` native + `__s2pkg_math` are engine-generic (Source 2 math types); CS2 field names appear ONLY in the regenerated `games/cs2`/`packages/cs2` files. NO CS2 identifiers in `core/src`. Both gates green: `bash scripts/check-core-boundary.sh`, `bash scripts/test-boundary-nameleak.sh`.
- **Never expose a raw pointer / copy values.** A vector read returns a fresh `{x,y,z}` value object COPIED from the floats — the pointer never crosses to JS. Every read serial-gated via `entity_resolve_ptr` → `T | null`.
- **Layout is data (known-shape exception).** Field offsets resolve live; the vector *shape* (3 floats) is a fixed known layout hardcoded in the `VEC` table (the catalog records the type name but no byte layout — same posture as the `ATOMIC` scalar table).
- **Deterministic codegen + freshness gate.** Same catalog+list → byte-identical generated files; `bash scripts/check-schema-generated.sh` green.
- **Module dependencies point one way.** `@s2script/entity` must NOT depend on `@s2script/math`; the `Vector`/`QAngle` construction lives in the generated game-layer getter, which requires `@s2script/math` itself.
- **cdylib:** core in-isolate tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase types (`Vector`, `QAngle`), camelCase methods/props (`readFloats`, `eyeAngles`, `absVelocity`, `length`).
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5c3-vector-value-type`; do NOT push.

**Deferred — do NOT build:** `origin` / any field behind `CGameSceneNode`/a pointer/embedded class (the embedded-ptr follow); Vector **writes**; `Vector2D`/`Vector4D`/`Color`/`Quaternion` codegen + value types; Vector arithmetic (`add`/`dot`/`cross`); the quantized wrappers; `enum` codegen; the engine-identity follow; the `tsc` gate; the registry (5.5); the base suite (6).

**Test runners:** core = `cargo test -p s2script-core -- --test-threads=1`; CLI = `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs` (scoped glob — NOT bare `--test`).

---

## Task 1: `@s2script/math` — the `Vector`/`QAngle` value types (cargo-in-isolate)

**Files:**
- Modify: `core/src/v8host.rs` (`INJECTED_STD_PRELUDE` — add the value types + `__s2pkg_math`; a new in-isolate test in `frame_tests`)
- Create: `packages/math/package.json`, `packages/math/index.d.ts`

**Interfaces:**
- Produces (for Task 3's generated code + Task 4): `__s2require("@s2script/math")` → `{ Vector, QAngle }`; `new Vector(x,y,z)` with `.x/.y/.z` + `.length()` + `.toString()`; `new QAngle(x,y,z)` with `.x/.y/.z` + `.toString()`.

- [ ] **Step 1: Write the failing in-isolate test** (add to `#[cfg(test)] mod frame_tests` in `v8host.rs`; mirror the existing `eval_in_context_string`-based tests):

```rust
    #[test]
    fn math_module_provides_vector_and_qangle() {
        let _ = init(dummy_logger());
        create_plugin_context("p");
        // the module resolves + constructs:
        assert_eq!(eval_in_context_string("p", r#"typeof __s2require("@s2script/math").Vector"#), "function");
        assert_eq!(eval_in_context_string("p", r#"typeof __s2require("@s2script/math").QAngle"#), "function");
        // Vector data + length():
        assert_eq!(eval_in_context_string("p", r#"var V=__s2require("@s2script/math").Vector; var v=new V(3,4,0); v.x+","+v.y+","+v.z"#), "3,4,0");
        assert_eq!(eval_in_context_string("p", r#"var V=__s2require("@s2script/math").Vector; String(new V(3,4,0).length())"#), "5");
        // QAngle data:
        assert_eq!(eval_in_context_string("p", r#"var Q=__s2require("@s2script/math").QAngle; var q=new Q(10,20,30); q.x+","+q.y+","+q.z"#), "10,20,30");
        shutdown();
    }
```
(If `eval_in_context_string`/`create_plugin_context` differ, read the neighboring `math`-free tests like `read_string_and_64bit_natives_degrade_without_ops` and mirror their exact harness.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p s2script-core frame_tests::math_module_provides_vector_and_qangle -- --test-threads=1` → FAIL (`__s2pkg_math` undefined → `__s2require` returns null → TypeError).

- [ ] **Step 3: Implement — add the value types to the prelude.** In `INJECTED_STD_PRELUDE` (`v8host.rs`), define the value types and register the module next to the existing `globalThis.__s2pkg_*` assignments. Add the definitions just BEFORE the `globalThis.__s2pkg_entity = …` block, and the assignment WITH that block:

```js
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  Vector.prototype.length = function () { return Math.sqrt(this.x * this.x + this.y * this.y + this.z * this.z); };
  Vector.prototype.toString = function () { return "Vector(" + this.x + ", " + this.y + ", " + this.z + ")"; };
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  QAngle.prototype.toString = function () { return "QAngle(" + this.x + ", " + this.y + ", " + this.z + ")"; };
```
and, alongside the other package registrations:
```js
  globalThis.__s2pkg_math       = { Vector: Vector, QAngle: QAngle };
```

- [ ] **Step 4: Create the types-only package** `packages/math/package.json`:

```json
{
  "name": "@s2script/math",
  "version": "0.1.0",
  "types": "index.d.ts",
  "description": "Type stubs for the @s2script/math injected value types (Vector, QAngle). No runtime code."
}
```
and `packages/math/index.d.ts`:

```ts
/**
 * @s2script/math — author-time type stubs for the injected math value types.
 * NO runtime code: the engine injects the implementation (core prelude) at load time.
 */

/** A 3-component vector value (a copied snapshot; never a live pointer). */
export declare class Vector {
  x: number;
  y: number;
  z: number;
  constructor(x: number, y: number, z: number);
  /** Euclidean magnitude — e.g. speed from a velocity vector. */
  length(): number;
  toString(): string;
}

/** A Source 2 Euler angle value (x=pitch, y=yaw, z=roll), a copied snapshot. */
export declare class QAngle {
  x: number;
  y: number;
  z: number;
  constructor(x: number, y: number, z: number);
  toString(): string;
}
```

- [ ] **Step 5: Run to verify pass** — `cargo test -p s2script-core frame_tests::math_module_provides_vector_and_qangle -- --test-threads=1` → PASS.

- [ ] **Step 6: Full suite + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/v8host.rs packages/math/package.json packages/math/index.d.ts
git commit -m "feat(slice5c3): @s2script/math — Vector/QAngle value types in the core prelude

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: `readFloats` native + `EntityRef.readFloats` + `.d.ts` (cargo-in-isolate)

**Files:**
- Modify: `core/src/v8host.rs` (a new native `s2_ent_ref_read_floats` + install + the `EntityRef` prelude method; a new in-isolate test), `packages/entity/index.d.ts`

**Interfaces:**
- Consumes: `crate::entity::read_f32` (existing), `entity_resolve_ptr`, `set_native`.
- Produces (for Task 3): `__s2_ent_ref_read_floats(index, serial, offset, count) → number[] | null`; `EntityRef.readFloats(off, count) → number[] | null`.

- [ ] **Step 1: Write the failing test** (add to `frame_tests`):

```rust
    #[test]
    fn read_floats_native_and_method_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the native degrades to null (no engine ops → entity_resolve_ptr null):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats(1,7,8,3))"), "null");
        // the EntityRef method degrades to null:
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readFloats(8,3))"#), "null");
        shutdown();
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p s2script-core frame_tests::read_floats_native_and_method_degrade_without_ops -- --test-threads=1` → FAIL (`__s2_ent_ref_read_floats` undefined → throws).

- [ ] **Step 3: Add the native** (near `s2_ent_ref_read_string` in `v8host.rs`):

```rust
/// Native `__s2_ent_ref_read_floats(index, serial, offset, count) -> number[] | null`. Serial-gated;
/// reads `count` (1..=4) contiguous f32s into a JS array (a COPY; the pointer never crosses to JS).
/// null on a stale/invalid ref or an out-of-range count.
fn s2_ent_ref_read_floats(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let count = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        if count <= 0 || count > 4 { return; }          // only small fixed vectors (Vector..Vector4D)
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                     // stale/invalid → null (already set)
        let p = ent as *const u8;
        let arr = v8::Array::new(scope, count);
        for i in 0..count {
            let v = crate::entity::read_f32(p, off + i * 4) as f64;
            let num = v8::Number::new(scope, v);
            arr.set_index(scope, i as u32, num.into());
        }
        rv.set(arr.into());
    }));
}
```

- [ ] **Step 4: Register the native.** In `install_natives`, next to `__s2_ent_ref_read_string`:
`set_native(scope, global_obj, "__s2_ent_ref_read_floats", s2_ent_ref_read_floats);`

- [ ] **Step 5: Add the `EntityRef` prelude method.** In `INJECTED_STD_PRELUDE`, on the `EntityRef.prototype` object (next to `readString`):

```js
    readFloats: function (o, count) { return __s2_ent_ref_read_floats(this.index, this.serial, o, count); },
```

- [ ] **Step 6: Update `packages/entity/index.d.ts`** — add to the `EntityRef` class:

```ts
  /** Read `count` (1..4) contiguous float32s at `offset` into a number[], or null if the ref is stale. */
  readFloats(offset: number, count: number): number[] | null;
```

- [ ] **Step 7: Run + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/v8host.rs packages/entity/index.d.ts
git commit -m "feat(slice5c3): __s2_ent_ref_read_floats native + EntityRef.readFloats (serial-gated float triple)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: Codegen — `vector`/`qangle` kinds + emit + regenerate (node:test)

**Files:**
- Modify: `packages/cli/src/schemagen/model.ts`, `packages/cli/src/schemagen/emit-js.ts`, `packages/cli/src/schemagen/emit-dts.ts`, `packages/cli/test/schemagen-model.test.mjs`, `packages/cli/test/schemagen-emit.test.mjs`, `packages/cli/test/schema-runtime.test.mjs`
- Regenerate (committed): `games/cs2/js/schema.generated.js`, `packages/cs2/schema.generated.d.ts`

**Interfaces:**
- Consumes: Task-1 `Vector`/`QAngle` (via `__s2require("@s2script/math")` in the generated JS), Task-2 `EntityRef.readFloats`.
- Produces: generated accessors `pawn.eyeAngles` (`QAngle | null`) + `pawn.absVelocity` (`Vector | null`).

- [ ] **Step 1: Write the failing model tests** (append to `packages/cli/test/schemagen-model.test.mjs`):

```js
test("classifyField maps Vector/QAngle atomics to vector/qangle kinds", () => {
  assert.deepEqual(classifyField({ kind: "atomic", name: "Vector" }), { accessorKind: "vector", writable: false });
  assert.deepEqual(classifyField({ kind: "atomic", name: "QAngle" }), { accessorKind: "qangle", writable: false });
  // an unmapped vector-ish atomic still skips (Vector2D/Color/Quaternion deferred):
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Vector2D" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Color" }));
});

test("buildModel emits a vector/qangle field with the right kind + TS type", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_vecAbsVelocity", offset: 8, type: { kind: "atomic", name: "Vector" } },
    { name: "m_angEyeAngles", offset: 24, type: { kind: "atomic", name: "QAngle" } },
  ] } };
  const m = buildModel(catalog, ["Base"]);
  const vel = m.classes[0].ownFields.find(x => x.rawName === "m_vecAbsVelocity");
  assert.equal(vel.propName, "absVelocity");     // vec ∈ tags stripped
  assert.equal(vel.accessorKind, "vector");
  assert.equal(TSTYPE.vector, "Vector | null");
  const ang = m.classes[0].ownFields.find(x => x.rawName === "m_angEyeAngles");
  assert.equal(ang.propName, "eyeAngles");        // ang ∈ tags stripped
  assert.equal(ang.accessorKind, "qangle");
  assert.equal(TSTYPE.qangle, "QAngle | null");
});
```
(Ensure `TSTYPE` is imported in this test file — it's exported from `../src/schemagen/model.ts`.)

- [ ] **Step 2: Run to verify failure** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schemagen-model.test.mjs` → FAIL.

- [ ] **Step 3: Implement the model changes** (`packages/cli/src/schemagen/model.ts`):
  - `AccessorKind`: append `| "vector" | "qangle"`.
  - `READ`: add `vector: "readFloats", qangle: "readFloats",`.
  - `TSTYPE`: add `vector: "Vector | null", qangle: "QAngle | null",`.
  - Add the two VEC maps (below `ATOMIC`):

```ts
// atomic vector-type name → kind (only the fixed-3-float types this slice; 2D/4D/Color/Quaternion deferred).
const VEC: Record<string, AccessorKind> = { Vector: "vector", QAngle: "qangle" };
// kind → value-class + float count, for the emitters + import detection.
export const VEC_INFO: Partial<Record<AccessorKind, { cls: string; count: number }>> = {
  vector: { cls: "Vector", count: 3 },
  qangle: { cls: "QAngle", count: 3 },
};
```
  - `classifyField`: inside the `if (type.kind === "atomic")` block, BEFORE the `const m = ATOMIC[...]` lookup, add:

```ts
    const vk = VEC[type.name ?? ""];
    if (vk) return { accessorKind: vk, writable: false };
```

- [ ] **Step 4: Run the model tests** — same command → PASS.

- [ ] **Step 5: emit-js — the vector getter + conditional imports.** In `packages/cli/src/schemagen/emit-js.ts`:
  - Import `VEC_INFO`: change the import to `import { flattenedFields, READ, WRITE, VEC_INFO } from "./model.ts";`.
  - Detect the used value-classes + emit the requires. After `const out: string[] = [HEADER, "(function () {", "  var off = __s2_schema_offset;", "  var A = {};"];`, insert:

```ts
  const vecClasses = new Set<string>();
  for (const c of model.classes) for (const f of flattenedFields(model, c.className)) {
    const vi = VEC_INFO[f.accessorKind]; if (vi) vecClasses.add(vi.cls);
  }
  for (const cls of [...vecClasses].sort()) {
    out.push(`  var ${cls} = __s2require("@s2script/math").${cls};`);
  }
```
  - Add the vector getter branch. In the getter `if/else if/else` chain, add a branch (e.g. after the `u64|i64` branch, before the final `else`):

```ts
      } else if (VEC_INFO[f.accessorKind]) {
        const vi = VEC_INFO[f.accessorKind]!;
        entry = `get: function () { var a = this.ref.readFloats(${resolve}, ${vi.count}); return a === null ? null : new ${vi.cls}(a[0], a[1], a[2]); }`;
```
  (vector/qangle are `writable:false` → the existing `if (f.writable)` guard emits no setter.)

- [ ] **Step 6: emit-dts — the conditional import.** In `packages/cli/src/schemagen/emit-dts.ts`:
  - Import `VEC_INFO`: `import { TSTYPE, VEC_INFO } from "./model.ts";`.
  - After building `out` with the `EntityRef` import, compute + insert the math import. Replace the `out` initialization + add detection:

```ts
  const out: string[] = [HEADER, 'import type { EntityRef } from "@s2script/entity";'];
  const vecClasses = new Set<string>();
  for (const c of model.classes) for (const f of c.ownFields) {
    const vi = VEC_INFO[f.accessorKind]; if (vi) vecClasses.add(vi.cls);
  }
  if (vecClasses.size) out.push(`import type { ${[...vecClasses].sort().join(", ")} } from "@s2script/math";`);
  out.push("");
```
  (`TSTYPE.vector`/`.qangle` already produce `Vector | null` / `QAngle | null` for the interface fields.)

- [ ] **Step 7: emit tests** (append to `packages/cli/test/schemagen-emit.test.mjs`):

```js
test("emitJs: a Vector field emits readFloats(off,3)+new Vector; the @s2script/math require appears", () => {
  const CATALOG = { Base: { parent: null, fields: [
    { name: "m_angEyeAngles", offset: 8, type: { kind: "atomic", name: "QAngle" } },
  ] } };
  const js = emitJs(buildModel(CATALOG, ["Base"]));
  assert.match(js, /var QAngle = __s2require\("@s2script\/math"\)\.QAngle;/);
  assert.match(js, /var a = this\.ref\.readFloats\(off\("Base","m_angEyeAngles"\), 3\); return a === null \? null : new QAngle\(a\[0\], a\[1\], a\[2\]\);/);
});

test("emitDts: a Vector/QAngle field adds the @s2script/math import + the field type", () => {
  const CATALOG = { Base: { parent: null, fields: [
    { name: "m_vecAbsVelocity", offset: 8, type: { kind: "atomic", name: "Vector" } },
  ] } };
  const dts = emitDts(buildModel(CATALOG, ["Base"]));
  assert.match(dts, /import type \{ Vector \} from "@s2script\/math";/);
  assert.match(dts, /absVelocity: Vector \| null;/);
});
```

- [ ] **Step 8: vm-compose test + fix the existing stubs.** ⚠️ FIRST: after Step 9's regen, `schema.generated.js` will have `var Vector = __s2require("@s2script/math").Vector;` at the top, evaluated when `vm.runInContext(genJs …)` runs. **Every existing `__s2require` stub in `packages/cli/test/schema-runtime.test.mjs` returns `null` for `@s2script/math` → `null.Vector` throws and breaks those tests.** So update EACH existing `__s2require: (n) => (n === "@s2script/entity" ? … : null)` stub to also return a value-type stub for math, e.g.:

```js
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef }
      : n === "@s2script/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                                   QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } }
      : null),
```
(Apply to all ~4 existing tests' stubs — the genJs eval needs it regardless of whether that test touches a vector field.) THEN add the new vector case (which stubs `__s2require("@s2script/math")` + a `readFloats` on the stub EntityRef):

```js
test("generated Vector/QAngle accessor: reads a value object, degrades to null (offline vm)", () => {
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  let floatsRet = [1, 2, 3];
  EntityRef.prototype.readFloats = function () { return floatsRet; };   // toggled to null below
  // minimal stub value types (match the real shape):
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  const math = { Vector, QAngle };
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : n === "@s2script/math" ? math : null),
    __s2_schema_offset: () => 8, __s2_ent_current_serial: () => 7, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Pawn } = ctx.__s2pkg_cs2;
  const p = new Pawn(new EntityRef(5, 9));
  const ang = p.eyeAngles;                          // generated QAngle accessor
  assert.ok(ang instanceof QAngle, "eyeAngles is a QAngle");
  assert.deepEqual([ang.x, ang.y, ang.z], [1, 2, 3]);
  floatsRet = null;                                 // stale ref → readFloats null
  assert.equal(p.eyeAngles, null, "a null readFloats → the accessor returns null");
});
```
(If `eyeAngles`/`absVelocity` aren't on `CCSPlayerPawn`'s generated proto after regen — Step 9 — adjust the property name to whichever direct Vector/QAngle field the regen actually produces on the pawn; confirm from the Step-9 `.d.ts` diff.)

- [ ] **Step 9: Regenerate + freshness gate**

```bash
cd /home/gkh/projects/s2script/packages/cli && node build.mjs
cd /home/gkh/projects/s2script && node packages/cli/dist/cli.js gen-schema
bash scripts/check-schema-generated.sh          # PASS (regenerated)
```
Inspect the `.d.ts` diff: `CCSPlayerPawn` gains `readonly eyeAngles: QAngle | null;`, `CBaseEntity` gains `readonly absVelocity: Vector | null;`, and the header gains `import type { QAngle, Vector } from "@s2script/math";`. Confirm the generated `schema.generated.js` gained the `var Vector/QAngle = __s2require("@s2script/math")…` lines. Confirm no scalar/handle/string accessor regressed (only vector/qangle fields added).

- [ ] **Step 10: Full CLI suite + gates + commit** (generated files are committed artifacts):

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add packages/cli/src/schemagen/model.ts packages/cli/src/schemagen/emit-js.ts packages/cli/src/schemagen/emit-dts.ts \
        packages/cli/test/schemagen-model.test.mjs packages/cli/test/schemagen-emit.test.mjs packages/cli/test/schema-runtime.test.mjs \
        games/cs2/js/schema.generated.js packages/cs2/schema.generated.d.ts
git commit -m "feat(slice5c3): codegen for direct Vector/QAngle fields; regenerate schema (pawn.eyeAngles/absVelocity)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: Live gate + README/CLAUDE (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: the Task-3 generated `pawn.eyeAngles` + `pawn.absVelocity`.

**Needs ONE sniper rebuild** (Tasks 1–2 changed the prelude + added a native).

- [ ] **Step 1: Sniper build + package.**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```
Confirm GLIBC ≤ 2.31 + `dist/addons/s2script/js/pawn.js` now contains the `var Vector = __s2require("@s2script/math")…` lines.

- [ ] **Step 2: Rewrite the demo** (`examples/demo-plugin/src/plugin.ts`) to read the generated Vector/QAngle accessors:

```ts
import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

// Slice 5C.3 — Vector/QAngle value types. Every ~256 frames, read each in-game player's pawn view
// angles (QAngle) + velocity (Vector) via the generated accessors — copied {x,y,z} snapshots.
let ticks = 0;
export function onLoad(): void {
  console.log("[demo] onLoad (Vector/QAngle)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      const body = p.pawn;
      if (!body) { console.log("  slot=" + p.slot + " (no pawn)"); continue; }
      const ang = body.eyeAngles;                         // generated QAngle | null
      const vel = body.absVelocity;                       // generated Vector | null
      console.log("  slot=" + p.slot
        + " eyeAngles=" + (ang ? ang.toString() : "null")
        + " absVelocity=" + (vel ? vel.toString() : "null")
        + " speed=" + (vel ? vel.length().toFixed(1) : "null"));
    }
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```
Build the `.s2sp` (`node packages/cli/dist/cli.js build examples/demo-plugin`), deploy to the plugins watch dir (`mkdir -p dist/addons/s2script/plugins && rm -f dist/addons/s2script/plugins/*.s2sp && cp examples/demo-plugin/dist/*.s2sp dist/addons/s2script/plugins/`), restart the container (`docker restart s2script-cs2`), wait past the boot window, `bot_quota 2` via `python3 scripts/rcon.py`, read `docker logs s2script-cs2 | grep '[demo]'`. **Expect:** each bot logs an `eyeAngles=QAngle(pitch, yaw, roll)` + `absVelocity=Vector(x, y, z)` (velocity ~0 for a standing bot; nonzero if moving) + a `speed=`. On `bot_kick` → `players=0`, server ticking, no crash. If a value is garbage or the read crashes, HALT and diagnose (the `readFloats` offset/count or the value-type wiring). If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done and report BLOCKED with commands/errors.

- [ ] **Step 3: README + CLAUDE.**
  - `README.md`: add a `## Vector value type (Slice 5C.3)` section — the `@s2script/math` module (`Vector`/`QAngle` copied `{x,y,z}` value types + `Vector.length()`), `EntityRef.readFloats`, that direct atomic Vector/QAngle fields now generate accessors (`pawn.eyeAngles`, `pawn.absVelocity`), the wire-clean property (a plain object crosses the inter-plugin wire), and the captured live log. Note `origin` (behind `CGameSceneNode` — an embedded/ptr follow), Vector writes, and `Vector2D`/`Vector4D`/`Color`/`Quaternion` are deferred.
  - `CLAUDE.md` "## Current state": Slice 5C.3 done (the first `@s2script/std`-breadth module `@s2script/math` — `Vector`/`QAngle` value types in the core prelude; `readFloats` native; codegen for direct atomic Vector/QAngle fields → `pawn.eyeAngles`/`absVelocity`; copied `{x,y,z}` snapshots, wire-clean). "Current focus" → next (origin/embedded-ptr follow, or the engine-identity follow, or more std breadth). Do NOT alter the standing conventions.

- [ ] **Step 4: Final verification + commit** (no build artifacts):

```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md
git commit -m "feat(slice5c3): live gate PASSED — pawn.eyeAngles + pawn.absVelocity (generated Vector/QAngle); README + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §7)

1. `cargo test -p s2script-core` green (new in-isolate tests); the CLI `node:test` suite green (model + emit + vm-compose + determinism); both boundary gates + `check-schema-generated.sh` green; sniper build clean.
2. `s2script gen-schema` regenerates the committed schema files deterministically with `eyeAngles` (`QAngle | null`) + `absVelocity` (`Vector | null`) + the `@s2script/math` imports.
3. Live gate: `pawn.eyeAngles` (a `QAngle {x,y,z}`) + `pawn.absVelocity` (a `Vector {x,y,z}`, `.length()` = speed) read via generated accessors; `null` on disconnect; no crash.
4. README + CLAUDE updated.
