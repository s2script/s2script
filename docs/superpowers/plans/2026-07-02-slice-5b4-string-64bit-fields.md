# Slice 5B.4 — String + 64-bit Field Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the typed field reads + codegen to `char[N]` inline strings and 64-bit numbers (`uint64`/`int64`/`float64`), so `player.playerName`/`player.steamID` (and every string/64-bit field) become generated accessors — completing the player model's identity with no engine natives.

**Architecture:** 64-bit reuses the 5B.2 kind-dispatch (`__s2_ent_ref_read` gains `KIND_U64`/`I64`→`BigInt`, `F64`→double); strings get a new `__s2_ent_ref_read_string(…, maxLen)` native returning a **copied** JS string. Pure `entity.rs` helpers behind both. The 5B.3 codegen gains the new kinds + a `char[N]`→`str` classifier + a name-transform fix; the committed schema files regenerate. Reads only.

**Tech Stack:** Rust `cdylib` core (rusty_v8 149.4.0, `v8::BigInt`), the injected `@s2script/entity` prelude, the TypeScript/esbuild codegen (`packages/cli`), `node:test`, the Docker CS2 live gate.

## Global Constraints

Every task's requirements implicitly include these (spec §10):

- **Core stays engine-generic.** New read primitives + natives + `KIND_*` codes are engine-generic; CS2 names appear ONLY in the regenerated `games/cs2`/`packages/cs2` files. NO CS2 identifiers in `core/src`. Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Never expose a raw pointer / copy strings.** `read_string` → a `v8::String` **copied** from the bytes; the pointer never crosses to JS. Every read serial-gated (`entity_resolve_ptr`) → `T | null`.
- **Layout is data.** Offsets resolve live; the `char[N]` length is parsed from the catalog type name; no offsets baked. Regeneration absorbs layout changes.
- **Deterministic codegen + freshness gate.** Same catalog+list → byte-identical generated files; `bash scripts/check-schema-generated.sh` green.
- **BigInt correctness.** `readUInt64` → `v8::BigInt::new_from_u64` (unsigned); `readInt64` → `new_from_i64` (signed). Never route a `u64 > i64::MAX` through the signed path.
- **cdylib:** core tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase types, camelCase methods (`readUInt64`, `readString`, `playerName`).
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5b4-string-64bit-fields`; do NOT push.

**Deferred — do NOT build:** `CUtlSymbolLarge`/`CUtlString` (pointer/table derefs — spike/follow); string + 64-bit **writes**; `BigInt` across the inter-plugin wire (deferred, graceful — NOT devalue); `enum` codegen; the `Vector` value type; embedded/`ptr`; the `userId` engine op; the base suite (6); the registry (5.5); the `tsc` gate.

**The kind codes (JS↔core contract — use these EXACT values in both the prelude `K` and core `KIND_*`):** existing `I32=1,F32=2,BOOL=3,I8=4,I16=5,U8=6,U16=7,U32=8`; NEW `U64=9, I64=10, F64=11`.

---

## Task 1: `entity.rs` — `read_u64`/`read_i64`/`read_f64`/`read_string` (PURE / cargo-unit)

**Files:**
- Modify: `core/src/entity.rs` (add below the existing readers; tests into the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing new (mirrors the existing `read_i32` null/negative-offset guard style).
- Produces (used by Task 2): `read_u64(base,off)->u64`, `read_i64(base,off)->i64`, `read_f64(base,off)->f64`, `read_string(base,off,max_len)->String`.

- [ ] **Step 1: Write the failing tests** (append to `#[cfg(test)] mod tests` in `entity.rs`):

```rust
    #[test]
    fn read_u64_i64_f64_roundtrip() {
        #[repr(C)]
        struct Fake { pad: [u8; 8], u: u64, i: i64, f: f64 }
        let x = Fake { pad: [0; 8], u: 76561198000000000, i: -9000000000, f: 6.5 }; // u > 2^53
        let base = &x as *const Fake as *const u8;
        assert_eq!(read_u64(base, 8), 76561198000000000);
        assert_eq!(read_i64(base, 16), -9000000000);
        assert_eq!(read_f64(base, 24), 6.5);
    }

    #[test]
    fn read_string_nul_terminated_and_bounded() {
        // "hi\0" then junk within a char[8] buffer.
        let buf: [u8; 8] = [b'h', b'i', 0, b'X', b'Y', 0, 0, 0];
        let base = buf.as_ptr();
        assert_eq!(read_string(base, 0, 8), "hi");            // stops at the first NUL
        assert_eq!(read_string(base, 3, 8), "XY");            // reads from an offset, stops at NUL
        // max_len bounds the scan even without a NUL:
        let full: [u8; 4] = [b'a', b'b', b'c', b'd'];         // no NUL
        assert_eq!(read_string(full.as_ptr(), 0, 4), "abcd");
        assert_eq!(read_string(full.as_ptr(), 0, 2), "ab");   // bounded by max_len
    }

    #[test]
    fn sixtyfour_bit_and_string_guard_null_and_negative_offset() {
        assert_eq!(read_u64(std::ptr::null(), 8), 0);
        assert_eq!(read_i64(std::ptr::null(), -8), 0);
        assert_eq!(read_f64(std::ptr::null(), 8), 0.0);
        assert_eq!(read_string(std::ptr::null(), 0, 8), "");
        let b: [u8; 2] = [b'x', 0];
        assert_eq!(read_string(b.as_ptr(), -1, 8), "");       // negative offset
        assert_eq!(read_string(b.as_ptr(), 0, 0), "");        // non-positive max_len
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p s2script-core entity:: -- --test-threads=1` → FAIL (`read_u64`/`read_string`/… not found).

- [ ] **Step 3: Implement** (add below `read_ptr` in `entity.rs`; match the existing aligned-deref style used by `read_i32`):

```rust
/// Read a u64 at `base + offset`. 0 on null base / negative offset.
pub fn read_u64(base: *const u8, offset: i32) -> u64 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const u64) }
}
/// Read an i64 at `base + offset`. 0 on null base / negative offset.
pub fn read_i64(base: *const u8, offset: i32) -> i64 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const i64) }
}
/// Read an f64 at `base + offset`. 0.0 on null base / negative offset.
pub fn read_f64(base: *const u8, offset: i32) -> f64 {
    if base.is_null() || offset < 0 { return 0.0; }
    unsafe { *(base.add(offset as usize) as *const f64) }
}
/// Read a NUL-terminated string of at most `max_len` bytes at `base + offset` (an inline `char[N]`
/// buffer), UTF-8-lossy → an owned `String` (a COPY; the pointer never leaves core). Empty on null
/// base / negative offset / non-positive `max_len`.
pub fn read_string(base: *const u8, offset: i32, max_len: i32) -> String {
    if base.is_null() || offset < 0 || max_len <= 0 { return String::new(); }
    let start = unsafe { base.add(offset as usize) };
    let max = max_len as usize;
    let mut len = 0usize;
    unsafe {
        while len < max && *start.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(start, len)).into_owned()
    }
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p s2script-core entity:: -- --test-threads=1` → PASS.

- [ ] **Step 5: Full suite + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/entity.rs
git commit -m "feat(slice5b4): entity.rs read_u64/i64/f64/string pure helpers

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: Natives + `EntityRef` prelude methods + `.d.ts` (cargo-in-isolate)

**Files:**
- Modify: `core/src/v8host.rs` (the `KIND_*` consts; the `s2_ent_ref_read` match; a new `s2_ent_ref_read_string` native + install; the `INJECTED_STD_PRELUDE` `K` + `EntityRef` methods), `packages/entity/index.d.ts`

**Interfaces:**
- Consumes: Task 1's `entity::{read_u64,read_i64,read_f64,read_string}`; `entity_resolve_ptr`, `set_native`, the `frame_tests` helpers.
- Produces: `__s2_ent_ref_read` handling `KIND_U64/I64/F64`; `__s2_ent_ref_read_string`; `EntityRef.readUInt64`/`readInt64` (`bigint|null`)/`readFloat64`/`readString`.

- [ ] **Step 1: Write the failing tests** (add to `#[cfg(test)] mod frame_tests`):

```rust
    #[test]
    fn read_string_and_64bit_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the generic read degrades for the new kinds (U64=9, I64=10, F64=11):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,9))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,10))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,11))"), "null");
        // the string native degrades:
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_string(1,7,8,128))"), "null");
        // EntityRef methods degrade (proving they're wired) — use `__s2require` (the native, available in a
        // create_plugin_context raw scope, as `eval_std` uses), NOT the CJS `require` (only in load_plugin_js):
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readUInt64(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readString(8,128))"#), "null");
        shutdown();
    }
```
(If in doubt about the raw-context mechanism, read the existing 5B.2 `generic_typed_reads_degrade_without_ops` test first and copy its proven pattern for the `EntityRef`-method assertions.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p s2script-core frame_tests::read_string_and_64bit_natives_degrade_without_ops -- --test-threads=1` → FAIL (kinds 9–11 leave `null` only by luck today; `__s2_ent_ref_read_string` undefined → throws).

- [ ] **Step 3: Add the `KIND_*` consts + extend the read match.** After `const KIND_U32: i64 = 8;` add:

```rust
const KIND_U64: i64 = 9;
const KIND_I64: i64 = 10;
const KIND_F64: i64 = 11;
```

In `fn s2_ent_ref_read`'s `match kind`, add BEFORE the `_ =>` arm:

```rust
            KIND_U64  => { let bi = v8::BigInt::new_from_u64(scope, crate::entity::read_u64(p, off)); rv.set(bi.into()); }
            KIND_I64  => { let bi = v8::BigInt::new_from_i64(scope, crate::entity::read_i64(p, off)); rv.set(bi.into()); }
            KIND_F64  => rv.set_double(crate::entity::read_f64(p, off)),
```

- [ ] **Step 4: Add the string native + install.** Add the native (near `s2_ent_ref_read`):

```rust
/// Native `__s2_ent_ref_read_string(index, serial, offset, maxLen) -> string|null`. Serial-gated;
/// returns a COPIED string (the pointer never crosses to JS). null on a stale ref.
fn s2_ent_ref_read_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let max_len = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                 // invalid → null (already set)
        let s = crate::entity::read_string(ent as *const u8, off, max_len);
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}
```

In `install_natives`, next to `__s2_ent_ref_read`/`write`, add:
`set_native(scope, global_obj, "__s2_ent_ref_read_string", s2_ent_ref_read_string);`

- [ ] **Step 5: Extend the `EntityRef` prelude** (`INJECTED_STD_PRELUDE`). Extend `K`:
`var K = { I32: 1, F32: 2, BOOL: 3, I8: 4, I16: 5, U8: 6, U16: 7, U32: 8, U64: 9, I64: 10, F64: 11 };`
Add to the `EntityRef.prototype` object (alongside the existing read methods):

```js
    readUInt64: function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.U64); },
    readInt64:  function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.I64); },
    readFloat64:function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.F64); },
    readString: function (o, maxLen) { return __s2_ent_ref_read_string(this.index, this.serial, o, maxLen); },
```

- [ ] **Step 6: Update `packages/entity/index.d.ts`.** Add to the `EntityRef` class:

```ts
  /** Read a u64 at `offset` as a BigInt, or null if the ref is stale. */
  readUInt64(offset: number): bigint | null;
  /** Read an i64 at `offset` as a BigInt, or null if the ref is stale. */
  readInt64(offset: number): bigint | null;
  /** Read an f64 at `offset`, or null if the ref is stale. */
  readFloat64(offset: number): number | null;
  /** Read a NUL-terminated string (up to `maxLen` bytes) at `offset`, or null if the ref is stale. */
  readString(offset: number, maxLen: number): string | null;
```

- [ ] **Step 7: Run + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/v8host.rs packages/entity/index.d.ts
git commit -m "feat(slice5b4): 64-bit (BigInt) + string read natives + EntityRef.readUInt64/readInt64/readFloat64/readString

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: Codegen — new kinds + `char[N]` classifier + name-transform fix + regenerate (node:test)

**Files:**
- Modify: `packages/cli/src/schemagen/model.ts`, `packages/cli/src/schemagen/emit-js.ts`, `packages/cli/test/schemagen-model.test.mjs`, `packages/cli/test/schemagen-emit.test.mjs`
- Regenerate (committed): `games/cs2/js/schema.generated.js`, `packages/cs2/schema.generated.d.ts`

**Interfaces:**
- Consumes: the existing `buildModel`/`emit*` structure.
- Produces: `AccessorKind` gains `u64`/`i64`/`f64`/`str`; `FieldDescriptor.strLen?`; `classifyField` handles `uint64`/`int64`/`float64` + `char[N]`; the fixed `idiomaticName`.

- [ ] **Step 1: Write the failing model tests** (append to `packages/cli/test/schemagen-model.test.mjs`):

```js
test("idiomaticName strips only KNOWN Hungarian tags (steamID/bombSite fixed)", () => {
  assert.equal(idiomaticName("m_iHealth"), "health");         // i ∈ tags
  assert.equal(idiomaticName("m_flFriction"), "friction");    // fl ∈ tags
  assert.equal(idiomaticName("m_hController"), "controller");  // h ∈ tags
  assert.equal(idiomaticName("m_iszPlayerName"), "playerName");// isz ∈ tags
  assert.equal(idiomaticName("m_steamID"), "steamID");        // "steam" ∉ tags → kept (was "iD")
  assert.equal(idiomaticName("m_bombSite"), "bombSite");      // "bomb" ∉ tags → kept (was "site")
  assert.equal(idiomaticName("m_flags"), "flags");            // no uppercase core → unchanged
});

test("classifyField maps 64-bit + char[N], skips other unknowns", () => {
  assert.deepEqual(classifyField({ kind: "atomic", name: "uint64" }), { accessorKind: "u64", writable: false });
  assert.deepEqual(classifyField({ kind: "atomic", name: "int64" }), { accessorKind: "i64", writable: false });
  assert.deepEqual(classifyField({ kind: "atomic", name: "float64" }), { accessorKind: "f64", writable: false });
  assert.deepEqual(classifyField({ kind: "unknown", name: "char[128]" }), { accessorKind: "str", writable: false, strLen: 128 });
  assert.ok("skip" in classifyField({ kind: "unknown", name: "CUtlSomething" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "CUtlSymbolLarge" }));
});

test("buildModel threads strLen onto a char[N] field descriptor", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_iszName", offset: 8, type: { kind: "unknown", name: "char[64]" } },
    { name: "m_steamID", offset: 16, type: { kind: "atomic", name: "uint64" } },
  ] } };
  const m = buildModel(catalog, ["Base"]);
  const f = m.classes[0].ownFields.find(x => x.rawName === "m_iszName");
  assert.equal(f.propName, "name");           // isz stripped
  assert.equal(f.accessorKind, "str");
  assert.equal(f.strLen, 64);
  const sid = m.classes[0].ownFields.find(x => x.rawName === "m_steamID");
  assert.equal(sid.propName, "steamID");
  assert.equal(sid.accessorKind, "u64");
});
```

- [ ] **Step 2: Run to verify failure** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schemagen-model.test.mjs` → FAIL.

- [ ] **Step 3: Implement the model changes** (`packages/cli/src/schemagen/model.ts`):
  - `AccessorKind`: `… | "u64" | "i64" | "f64" | "str"`.
  - `FieldDescriptor`: add `strLen?: number;`.
  - `ATOMIC`: add `uint64: { k: "u64", w: false }, int64: { k: "i64", w: false }, float64: { k: "f64", w: false },`.
  - `READ`: add `u64: "readUInt64", i64: "readInt64", f64: "readFloat64", str: "readString",` (the primitive method names).
  - `TSTYPE`: add `u64: "string | null", i64: "string | null", f64: "number | null", str: "string | null",` — **generated 64-bit ints are decimal strings** (SM-parity, wire-safe; the `readUInt64`/`readInt64` primitives stay `bigint`, and `emit-js` `.toString()`s them). `f64` stays `number`.
  - `idiomaticName` — the known-tags fix:

```ts
const KNOWN_TAGS = new Set(["i","n","b","h","fl","f","u","e","p","a","v","vec","ang","q","sz","isz","ch","clr","un"]);
export function idiomaticName(raw: string): string {
  const s = raw.replace(/^m_/, "");
  const m = s.match(/^([a-z]+)([A-Z].*)$/);         // leading lowercase run, then an Uppercase-led core
  const core = (m && KNOWN_TAGS.has(m[1])) ? m[2] : s;
  return core.charAt(0).toLowerCase() + core.slice(1);
}
```

  - `classifyField` — its return type gains `strLen?: number`; add a `char[N]` branch:

```ts
export function classifyField(type: CatalogField["type"]): { accessorKind: AccessorKind; writable: boolean; strLen?: number } | { skip: string } {
  if (type.kind === "handle") return { accessorKind: "handle", writable: false };
  if (type.kind === "atomic") {
    const m = ATOMIC[type.name ?? ""];
    if (m) return { accessorKind: m.k, writable: m.w };
    return { skip: `atomic '${type.name}' is not a scalar (string/vector/compound)` };
  }
  if (type.kind === "unknown") {
    const cm = (type.name ?? "").match(/^char\[(\d+)\]$/);
    if (cm) return { accessorKind: "str", writable: false, strLen: parseInt(cm[1], 10) };
    return { skip: `unmapped 'unknown' type '${type.name ?? ""}'` };
  }
  if (type.kind === "enum") return { skip: "enum byte-width absent from catalog (deferred)" };
  if (type.kind === "class") return { skip: `embedded class '${type.name ?? ""}' deferred` };
  if (type.kind === "ptr") return { skip: "raw pointer" };
  return { skip: `unmapped kind '${type.kind}'` };
}
```

  - `buildModel` — where it builds a `FieldDescriptor` from `classifyField`'s result, thread `strLen`: add `strLen: c.strLen` to the pushed descriptor (present only for `str`).

- [ ] **Step 4: Run the model tests** — same command → PASS.

- [ ] **Step 5: The emit-js getter shapes + test.** In `emit-js.ts`, where the getter is built, produce THREE shapes — `str` (length arg), `u64`/`i64` (null-safe `.toString()` → decimal string, SM-parity + wire-safe), and the default scalar/handle/f64 (a simple return):

```ts
const resolve = `off(${S(f.declaringClass)}, ${S(f.rawName)})`;
let entry;
if (f.accessorKind === "str") {
  entry = `get: function () { return this.ref.readString(${resolve}, ${f.strLen}); }`;
} else if (f.accessorKind === "u64" || f.accessorKind === "i64") {
  // 64-bit ints -> decimal string (SM-parity, wire-safe); readUInt64/readInt64 are the bigint primitives.
  entry = `get: function () { var v = this.ref.${READ[f.accessorKind]}(${resolve}); return v === null ? null : v.toString(); }`;
} else {
  entry = `get: function () { return this.ref.${READ[f.accessorKind]}(${resolve}); }`;
}
```
(Then the existing `if (f.writable)` block appends the setter as before — `u64`/`i64`/`f64`/`str` are all `writable:false`, so no setter for them. Adjust the exact assembly to match how the current emit-js concatenates the getter + optional setter into the `defineProperty` descriptor; the point is the three getter bodies above.) Append to `packages/cli/test/schemagen-emit.test.mjs`:

```js
test("emitJs: char[N] emits readString(len); 64-bit int emits a null-safe readUInt64().toString()", () => {
  const CATALOG = { Base: { parent: null, fields: [
    { name: "m_iszName", offset: 8, type: { kind: "unknown", name: "char[64]" } },
    { name: "m_steamID", offset: 16, type: { kind: "atomic", name: "uint64" } },
  ] } };
  const model = buildModel(CATALOG, ["Base"]);
  const js = emitJs(model);
  assert.match(js, /readString\(off\("Base","m_iszName"\), 64\)/);
  // 64-bit int -> decimal string: reads the bigint primitive, null-guards, stringifies.
  assert.match(js, /var v = this\.ref\.readUInt64\(off\("Base","m_steamID"\)\); return v === null \? null : v\.toString\(\);/);
  // and the .d.ts types the 64-bit int as string (SM-parity), NOT bigint (import emitDts if not already):
  const dts = emitDts(model);
  assert.match(dts, /steamID[^;\n]*: string \| null/);
  assert.doesNotMatch(dts, /steamID[^;\n]*: bigint/);
});
```
(If `emitDts` isn't already imported at the top of `schemagen-emit.test.mjs`, add it to the import from `../src/schemagen/emit-dts.ts` alongside `emitJs`.)

- [ ] **Step 6: Run the emit test** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schemagen-emit.test.mjs` → PASS.

- [ ] **Step 7: Regenerate the committed schema files + freshness gate**

```bash
cd /home/gkh/projects/s2script/packages/cli && node build.mjs
cd /home/gkh/projects/s2script && node packages/cli/dist/cli.js gen-schema
bash scripts/check-schema-generated.sh          # PASS (regenerated)
```
Inspect the diff of `packages/cs2/schema.generated.d.ts`: `CBasePlayerController` gains `readonly playerName: string | null;` + `readonly steamID: string | null;` (both strings — the steamid64 is a decimal string; inherited by `CCSPlayerController` → `Player`). Sanity-check the name-transform re-idiomaticized any previously over-stripped names as intended (no regressions to `health`/`teamNum`/`friction`/`pawn`/`controller`). Confirm the vm tests (`schema-runtime.test.mjs`, `schemagen-determinism.test.mjs`) still pass after regen.

- [ ] **Step 8: Full CLI suite + gates + commit** (the generated files are committed artifacts):

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add packages/cli/src/schemagen/model.ts packages/cli/src/schemagen/emit-js.ts \
        packages/cli/test/schemagen-model.test.mjs packages/cli/test/schemagen-emit.test.mjs \
        games/cs2/js/schema.generated.js packages/cs2/schema.generated.d.ts
git commit -m "feat(slice5b4): codegen for char[N]/64-bit kinds + known-tags idiomaticName fix; regenerate schema

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: Spike + live gate + README/CLAUDE (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`; **create** a dated spike-findings doc.

**Interfaces:**
- Consumes: the Task-2 natives (`readString`/`readUInt64`), the Task-3 generated `player.playerName`/`player.steamID`.

**Needs ONE sniper rebuild** (Task-2 natives are new core). The spike + gate share it.

- [ ] **Step 1: Sniper build + package.**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```
Confirm GLIBC ≤ 2.31.

- [ ] **Step 2: SPIKE — live-verify the raw primitives** (before trusting the generated accessors). Write a throwaway diagnostic in `examples/demo-plugin/src/plugin.ts` that, for the bot's controller (slot 0 → entity index 1), resolves `m_iszPlayerName` (on `CBasePlayerController`) + `m_steamID` offsets via `__s2_schema_offset` and reads them RAW via `EntityRef.readString(off, 128)` + `EntityRef.readUInt64(off)`:

```ts
import { OnGameFrame } from "@s2script/frame";
import { EntityRef } from "@s2script/entity";
declare const __s2_schema_offset: (cls: string, field: string) => number;
declare const __s2_ent_current_serial: (index: number) => number;
```
Build the controller `new EntityRef(1, __s2_ent_current_serial(1))`, resolve `__s2_schema_offset("CBasePlayerController","m_iszPlayerName")` / `"m_steamID"`, read `readString(nameOff, 128)` and `readUInt64(sidOff)`, log them. Build the `.s2sp` (`node packages/cli/dist/cli.js build examples/demo-plugin`), concatenate the addon JS (`cat games/cs2/js/schema.generated.js games/cs2/js/pawn.js > dist/addons/s2script/js/pawn.js`), drop, restart, arm `bot_quota 1`. **Expect:** the bot's name string (e.g. a bot name) + a uint64 steamid (bots often read `0` — that's fine, it proves the read path; a real player would show a `7656…` value). If the string is garbage or the read crashes, HALT — the `char[N]` inline assumption or the primitive is wrong; fix `entity.rs`/the native and rebuild. Record the observed values in `docs/superpowers/specs/2026-07-02-slice-5b4-spike-findings.md`.

- [ ] **Step 3: The live gate — GENERATED accessors.** Rewrite the demo to use the generated `Player` accessors (no `__s2_schema_offset`, no raw reads):

```ts
import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

let ticks = 0;
export function onLoad(): void {
  console.log("[demo] onLoad (player identity)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    for (const p of Player.all()) {
      console.log("  slot=" + p.slot
        + " name=" + p.playerName                    // generated (m_iszPlayerName, char[128]) -> string
        + " steamID=" + p.steamID                    // generated (m_steamID, uint64 -> decimal string)
        + " health=" + (p.pawn ? p.pawn.health : "none"));
    }
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```
Rebuild the `.s2sp` + re-concat the addon JS + restart + arm. **Expect:** `slot=0 name=<bot name> steamID=<decimal string> health=100` (a `typeof p.steamID === "string"`; bots may read `"0"`, a real player a `"7656…"`); on `bot_kick` the player drops (occupancy filter), server ticking, no crash. Capture the log. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done and report BLOCKED with commands/errors.

- [ ] **Step 4: README + CLAUDE.**
  - `README.md`: add a `## String + 64-bit fields (Slice 5B.4)` section — `EntityRef.readString`/`readUInt64`/`readInt64`/`readFloat64` (the `readUInt64`/`readInt64` primitives return `bigint`); that `char[N]`/64-bit fields now generate accessors — `player.playerName` (string), `player.steamID` (**decimal string**, SM-parity: SourcePawn has no 64-bit int, so `GetClientAuthId(AuthId_SteamID64)` is a string; wire-safe); the name-transform fix; and the captured live log. Note `CUtlSymbolLarge`/`CUtlString` + writes are deferred, and that generating 64-bit as strings makes the BigInt-on-the-wire concern moot (a raw `bigint` on the inter-plugin wire still hits the graceful `InterfaceValueNotSerializable`).
  - `CLAUDE.md` "## Current state": Slice 5B.4 done (string + 64-bit field support — `char[N]`→string, `uint64`/`int64` → `bigint` **primitive** but **generated accessors are decimal strings** (SM-parity, wire-safe), `float64`→number; `player.playerName`/`steamID` generated as strings; the known-tags idiomaticName fix); "Current focus" → next (5C.3 std breadth / Vector, or the base-plugin surface). Do NOT alter the standing conventions.

- [ ] **Step 5: Final verification + commit** (no build artifacts):

```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md docs/superpowers/specs/2026-07-02-slice-5b4-spike-findings.md
git commit -m "feat(slice5b4): live gate PASSED — player.playerName + player.steamID (generated); spike + README + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §8)

1. `cargo test -p s2script-core` green (new `entity.rs` + in-isolate tests); the CLI `node:test` suite green (model + emit + determinism); both boundary gates + `check-schema-generated.sh` green; sniper build clean.
2. `s2script gen-schema` regenerates the committed schema files deterministically with `playerName`/`steamID` (+ every string/64-bit field) as accessors; the name-transform fix holds.
3. Spike: raw `readString`/`readUInt64` read a sane name + steamid live. Live gate: `player.playerName` (string) + `player.steamID` (decimal **string**, `typeof === "string"`) read via generated accessors; `null` on disconnect; no crash.
4. README + CLAUDE updated.
