# Slice 5B.2 — Typed Field Access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand entity field read/write beyond `i32` to `float32`/`bool`/the integer widths/`CHandle`→`EntityRef`, via two kind-dispatched natives + typed `EntityRef` methods, so the 5B.3 codegen can emit typed accessors.

**Architecture:** Two generic natives `__s2_ent_ref_read(idx, serial, off, kind)` / `__s2_ent_ref_write(idx, serial, off, kind, value)` dispatch on a small integer `kind` code to pure `entity.rs` typed readers, all behind the 5A serial-gate (`entity_resolve_ptr`). The `@s2script/std` `EntityRef` exposes typed methods (`readFloat32`, `readBool`, `readHandle`→`EntityRef|null`, …) that pass their `kind`. `readInt32`/`writeInt32` are re-expressed over the generic native and the 5A per-type i32 natives removed.

**Tech Stack:** Rust `cdylib` core (rusty_v8), the `@s2script/std` injected prelude, Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-07-01-slice-5b2-typed-field-access-design.md`.

## Global Constraints

Every task's requirements implicitly include these (from spec §11):

- **Core stays engine-generic.** No CS2 identifiers (incl. string literals in core tests), no `include_str!`/`games/` in `core/src`. `EntityRef` + the natives are engine-generic; CS2 class/field names live only in the demo plugin. Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Never expose a raw pointer across time.** JS holds only `{index, serial}`; every deref is serial-gated via `entity_resolve_ptr`; the typed reads compose it, never surfacing a pointer.
- **`T | null` / degrade-never-crash.** Every native `catch_unwind`-wrapped; invalid `(index,serial)` or unknown `kind` → read `null` / write `false`; no stale deref; no panic across FFI.
- **Layout is data.** Offsets resolved live via `__s2_schema_offset`; none baked into code.
- **`kind` codes are a JS↔core contract** — defined once in the prelude (`K = {...}`) and mirrored as named core consts (`KIND_*`), documented in both, kept in lockstep.
- **Naming:** PascalCase types (`EntityRef`), camelCase fns/props. **cdylib:** unit tests inline `#[cfg(test)] mod`.
- **Commit trailer:** every commit ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5b2-typed-fields`; do not push.

**Deferred — do NOT build:** `i64`/`u64` (JS f64 precision — BigInt later); `write` for narrow widths (i8/i16/u8/u16 — read-only this slice); a `Vector`/`QAngle` value type + strings + embedded-struct accessors (5C / codegen); the 5B.3 codegen itself; the `@s2script/std` module split + breadth (5C); the `tsc` gate; config/permissions; the registry (5.5); the base-plugin suite (6).

**The kind codes (shared contract — use these EXACT values in both the prelude `K` and core `KIND_*`):**
`I32=1, F32=2, BOOL=3, I8=4, I16=5, U8=6, U16=7, U32=8`.

---

## File Structure

- **Modify `core/src/entity.rs`** — add pure typed helpers (`read_f32`/`write_f32`, `read_bool`/`write_bool`, `read_i8`/`read_i16`, `read_u8`/`read_u16`) + tests.
- **Modify `core/src/v8host.rs`** — the `KIND_*` consts; the two generic natives; install them; DELETE `s2_ent_ref_read_i32`/`write_i32` + their installs; re-express the `EntityRef` prelude `readInt32`/`writeInt32` over the generic native + add the new typed methods + `readHandle` + the `K` enum; repoint the one 5A test that called `__s2_ent_ref_read_i32` directly.
- **Modify `packages/std/index.d.ts`** — the new `EntityRef` method signatures.
- **Modify** a demo (`examples/demo-plugin` or a small demo) to read a float/bool/handle field; `README.md`, `CLAUDE.md`.

---

## Task 1: `entity.rs` typed pure helpers (PURE / cargo-unit)

**Files:**
- Modify: `core/src/entity.rs` (add below `read_u32`/`read_ptr`; tests into the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing new (mirrors the existing `read_i32` guard pattern).
- Produces (used by v8host in Task 2): `read_f32(base,off)->f32`, `write_f32(base,off,v)`, `read_bool(base,off)->bool`, `write_bool(base,off,v)`, `read_i8(base,off)->i32` (sign-extended), `read_i16(base,off)->i32` (sign-extended), `read_u8(base,off)->u32` (zero-extended), `read_u16(base,off)->u32` (zero-extended).

- [ ] **Step 1: Write the failing tests** (append to `#[cfg(test)] mod tests` in `entity.rs`):

```rust
    #[test]
    fn read_write_f32_roundtrips() {
        #[repr(C)]
        struct Fake { pad: [u8; 4], f: f32 }
        let mut x = Fake { pad: [0; 4], f: 0.0 };
        let base = &mut x as *mut Fake as *mut u8;
        write_f32(base, 4, 12.5);
        assert_eq!(read_f32(base as *const u8, 4), 12.5);
    }

    #[test]
    fn read_write_bool_roundtrips_and_reads_nonzero_as_true() {
        #[repr(C)]
        struct Fake { pad: [u8; 4], b: u8 }
        let mut x = Fake { pad: [0; 4], b: 0 };
        let base = &mut x as *mut Fake as *mut u8;
        assert_eq!(read_bool(base as *const u8, 4), false);
        write_bool(base, 4, true);
        assert_eq!(read_bool(base as *const u8, 4), true);
        assert_eq!(x.b, 1);
        // any non-zero byte reads as true:
        x.b = 0x7F;
        assert_eq!(read_bool(base as *const u8, 4), true);
    }

    #[test]
    fn read_i8_i16_sign_extend() {
        #[repr(C)]
        struct Fake { i8v: i8, pad: u8, i16v: i16 }
        let x = Fake { i8v: -1, pad: 0, i16v: -1000 };
        let base = &x as *const Fake as *const u8;
        assert_eq!(read_i8(base, 0), -1);       // 0xFF -> -1 (sign-extended to i32)
        assert_eq!(read_i16(base, 2), -1000);   // sign-extended
    }

    #[test]
    fn read_u8_u16_zero_extend() {
        #[repr(C)]
        struct Fake { u8v: u8, pad: u8, u16v: u16 }
        let x = Fake { u8v: 0xFF, pad: 0, u16v: 0xFFFF };
        let base = &x as *const Fake as *const u8;
        assert_eq!(read_u8(base, 0), 255);      // zero-extended, not -1
        assert_eq!(read_u16(base, 2), 65535);
    }

    #[test]
    fn typed_reads_guard_null_and_negative_offset() {
        assert_eq!(read_f32(std::ptr::null(), 4), 0.0);
        assert_eq!(read_f32(std::ptr::null(), -4), 0.0);
        assert_eq!(read_bool(std::ptr::null(), 4), false);
        assert_eq!(read_i8(std::ptr::null(), 0), 0);
        assert_eq!(read_u16(std::ptr::null(), 2), 0);
        // writes to null / negative offset must not crash + must be a no-op:
        write_f32(std::ptr::null_mut(), 4, 1.0);
        write_bool(std::ptr::null_mut(), 4, true);
        let mut v: f32 = 5.0;
        write_f32(&mut v as *mut f32 as *mut u8, -4, 9.0);
        assert_eq!(v, 5.0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core entity:: -- --test-threads=1`
Expected: FAIL — `read_f32`/`read_bool`/… not found.

- [ ] **Step 3: Implement** (add below `read_ptr` in `entity.rs`; mirror the existing null/negative-offset guard):

```rust
/// Read an f32 at `base + offset`. 0.0 on null base / negative offset (degrade-safe).
pub fn read_f32(base: *const u8, offset: i32) -> f32 {
    if base.is_null() || offset < 0 { return 0.0; }
    unsafe { *(base.add(offset as usize) as *const f32) }
}
/// Write an f32 at `base + offset`. No-op on null base / negative offset.
pub fn write_f32(base: *mut u8, offset: i32, value: f32) {
    if base.is_null() || offset < 0 { return; }
    unsafe { *(base.add(offset as usize) as *mut f32) = value; }
}
/// Read a bool (a single byte; any non-zero is true). false on null / negative offset.
pub fn read_bool(base: *const u8, offset: i32) -> bool {
    if base.is_null() || offset < 0 { return false; }
    unsafe { *base.add(offset as usize) != 0 }
}
/// Write a bool as a single byte (1/0). No-op on null / negative offset.
pub fn write_bool(base: *mut u8, offset: i32, value: bool) {
    if base.is_null() || offset < 0 { return; }
    unsafe { *base.add(offset as usize) = if value { 1 } else { 0 }; }
}
/// Read an i8, sign-extended to i32. 0 on null / negative offset.
pub fn read_i8(base: *const u8, offset: i32) -> i32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const i8) as i32 }
}
/// Read an i16, sign-extended to i32. 0 on null / negative offset.
pub fn read_i16(base: *const u8, offset: i32) -> i32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const i16) as i32 }
}
/// Read a u8, zero-extended to u32. 0 on null / negative offset.
pub fn read_u8(base: *const u8, offset: i32) -> u32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *base.add(offset as usize) as u32 }
}
/// Read a u16, zero-extended to u32. 0 on null / negative offset.
pub fn read_u16(base: *const u8, offset: i32) -> u32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const u16) as u32 }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p s2script-core entity:: -- --test-threads=1`
Expected: PASS (existing + 5 new).

- [ ] **Step 5: Full suite + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/entity.rs
git commit -m "feat(slice5b2): entity.rs typed pure helpers (f32/bool/i8/i16/u8/u16)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: kind-dispatched natives + `EntityRef` typed methods + i32 cutover (in-isolate cargo)

**Files:**
- Modify: `core/src/v8host.rs` (`KIND_*` consts; `s2_ent_ref_read`/`s2_ent_ref_write` natives; installs; delete `s2_ent_ref_read_i32`/`write_i32` + installs; the `INJECTED_STD_PRELUDE` `EntityRef` methods + `K` enum; repoint the 5A native-direct test), `packages/std/index.d.ts`.

**Interfaces:**
- Consumes: Task 1's `entity::{read_f32, write_f32, read_bool, write_bool, read_i8, read_i16, read_u8, read_u16, read_i32, write_i32, read_u32}`; the existing `entity_resolve_ptr(index, serial) -> *mut u8`, `__s2_handle_decode`, `set_native`, `frame_tests` helpers.
- Produces: `__s2_ent_ref_read`/`__s2_ent_ref_write`; the `EntityRef` typed methods.

- [ ] **Step 1: Write the failing tests** (in `#[cfg(test)] mod frame_tests`). Add new degrade tests AND repoint the existing 5A native-direct test:

```rust
    #[test]
    fn generic_typed_reads_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);          // no ops → entity_resolve_ptr null → read null / write false
        create_plugin_context("p");
        // each kind degrades to null (read) — I32=1,F32=2,BOOL=3,I8=4,I16=5,U8=6,U16=7,U32=8
        for k in ["1","2","3","4","5","6","7","8"] {
            assert_eq!(eval_in_context_string("p", &format!("String(__s2_ent_ref_read(1,7,8,{}))", k)), "null");
        }
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,999))"), "null"); // unknown kind
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write(1,7,8,2,1.5))"), "false");
        // EntityRef typed methods degrade (proving they're wired + route a kind):
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2pkg_std; String(new EntityRef(1,7).readFloat32(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2pkg_std; String(new EntityRef(1,7).readBool(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2pkg_std; String(new EntityRef(1,7).readHandle(8))"#), "null");
        shutdown();
    }
```

Repoint the existing `ent_ref_natives_degrade_without_engine_ops` test: its line
`assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_i32(1, 7, 8))"), "null");`
becomes `assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1, 7, 8, 1))"), "null");`
and any `__s2_ent_ref_write_i32(...)` line becomes `__s2_ent_ref_write(1,7,8,1,5)`. (The `handle_decode`/`current_serial`/`valid` assertions in that test are unchanged — those natives stay.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core frame_tests::generic_typed_reads_degrade_without_ops -- --test-threads=1`
Expected: FAIL — `__s2_ent_ref_read` not defined; the repointed test also fails until the native exists.

- [ ] **Step 3: Add the `KIND_*` consts + the two generic natives** (`core/src/v8host.rs`, near the other entity natives). `KIND_*` are `const` so they work as match patterns:

```rust
// Field-type kind codes — a JS<->core contract, mirrored in INJECTED_STD_PRELUDE's `K`. Keep in lockstep.
const KIND_I32: i64 = 1;
const KIND_F32: i64 = 2;
const KIND_BOOL: i64 = 3;
const KIND_I8: i64 = 4;
const KIND_I16: i64 = 5;
const KIND_U8: i64 = 6;
const KIND_U16: i64 = 7;
const KIND_U32: i64 = 8;

/// Native `__s2_ent_ref_read(index, serial, offset, kind) -> number|boolean|null`. Serial-gated typed read.
fn s2_ent_ref_read(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(3).integer_value(scope).unwrap_or(0);
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                   // invalid → null (already set)
        let p = ent as *const u8;
        match kind {
            KIND_I32 => rv.set_int32(crate::entity::read_i32(p, off)),
            KIND_F32 => rv.set_double(crate::entity::read_f32(p, off) as f64),
            KIND_BOOL => rv.set_bool(crate::entity::read_bool(p, off)),
            KIND_I8 => rv.set_int32(crate::entity::read_i8(p, off)),
            KIND_I16 => rv.set_int32(crate::entity::read_i16(p, off)),
            KIND_U8 => rv.set_double(crate::entity::read_u8(p, off) as f64),
            KIND_U16 => rv.set_double(crate::entity::read_u16(p, off) as f64),
            KIND_U32 => rv.set_double(crate::entity::read_u32(p, off) as f64),
            _ => { /* unknown kind → leave null */ }
        }
    }));
}

/// Native `__s2_ent_ref_write(index, serial, offset, kind, value) -> boolean`. Serial-gated typed write
/// (I32/F32/BOOL only this slice; narrow-width writes deferred → false).
fn s2_ent_ref_write(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(3).integer_value(scope).unwrap_or(0);
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                   // invalid → false (already set)
        match kind {
            KIND_I32 => crate::entity::write_i32(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_F32 => crate::entity::write_f32(ent, off, args.get(4).number_value(scope).unwrap_or(0.0) as f32),
            KIND_BOOL => crate::entity::write_bool(ent, off, args.get(4).boolean_value(scope)),
            _ => return,                               // unknown / deferred write kind → false
        }
        rv.set_bool(true);
    }));
}
```

- [ ] **Step 4: Install the two natives + delete the per-type i32 natives.** In `install_natives`, replace the two lines
`set_native(scope, global_obj, "__s2_ent_ref_read_i32", s2_ent_ref_read_i32);`
`set_native(scope, global_obj, "__s2_ent_ref_write_i32", s2_ent_ref_write_i32);`
with
```rust
    set_native(scope, global_obj, "__s2_ent_ref_read", s2_ent_ref_read);
    set_native(scope, global_obj, "__s2_ent_ref_write", s2_ent_ref_write);
```
and DELETE the now-unused `fn s2_ent_ref_read_i32` and `fn s2_ent_ref_write_i32` definitions. (`entity::read_i32`/`write_i32` stay — the generic native uses them.)

- [ ] **Step 5: Update the `EntityRef` prelude** (`INJECTED_STD_PRELUDE`). Add the `K` enum before the `EntityRef` definition, and replace the `EntityRef.prototype` object with the typed methods (re-expressing `readInt32`/`writeInt32` over the generic native + the new methods + `readHandle`):

```js
  var K = { I32: 1, F32: 2, BOOL: 3, I8: 4, I16: 5, U8: 6, U16: 7, U32: 8 }; // mirrors core KIND_*
  ...
  EntityRef.prototype = {
    isValid: function () { return __s2_ent_ref_valid(this.index, this.serial); },
    readInt32:   function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.I32); },
    writeInt32:  function (o, v) { return __s2_ent_ref_write(this.index, this.serial, o, K.I32, v); },
    readFloat32: function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.F32); },
    writeFloat32:function (o, v) { return __s2_ent_ref_write(this.index, this.serial, o, K.F32, v); },
    readBool:    function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.BOOL); },
    writeBool:   function (o, v) { return __s2_ent_ref_write(this.index, this.serial, o, K.BOOL, v); },
    readInt8:    function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.I8); },
    readInt16:   function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.I16); },
    readUInt8:   function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.U8); },
    readUInt16:  function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.U16); },
    readUInt32:  function (o)    { return __s2_ent_ref_read(this.index, this.serial, o, K.U32); },
    readHandle:  function (o) {
      var h = __s2_ent_ref_read(this.index, this.serial, o, K.U32);
      if (h === null) return null;
      var d = __s2_handle_decode(h >>> 0);
      var ref = new EntityRef(d[0], d[1]);
      return ref.isValid() ? ref : null;
    },
    notifyStateChanged: function (offset) { __s2_ent_ref_state_changed(this.index, this.serial, offset); },
  };
```

- [ ] **Step 6: Update `packages/std/index.d.ts`.** Add to the `EntityRef` class the new method signatures:
```ts
  readFloat32(offset: number): number | null;
  writeFloat32(offset: number, value: number): boolean;
  readBool(offset: number): boolean | null;
  writeBool(offset: number, value: boolean): boolean;
  readInt8(offset: number): number | null;
  readInt16(offset: number): number | null;
  readUInt8(offset: number): number | null;
  readUInt16(offset: number): number | null;
  readUInt32(offset: number): number | null;
  readHandle(offset: number): EntityRef | null;
```

- [ ] **Step 7: Run the tests + full suite + gates**

Run: `cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`
Expected: green — the new `generic_typed_reads_degrade_without_ops`, the repointed `ent_ref_natives_degrade_without_engine_ops`, the 5A `entity_ref_degrades_without_ops` (still uses `readInt32`/`readFloat32`… which now route through the generic native), and all prior tests. Grep confirms no caller of `__s2_ent_ref_read_i32`/`__s2_ent_ref_write_i32` remains anywhere.

- [ ] **Step 8: Commit**

```bash
git add core/src/v8host.rs packages/std/index.d.ts
git commit -m "feat(slice5b2): kind-dispatched read/write natives + typed EntityRef methods; retire per-type i32 natives

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: Demo + typed-field LIVE gate + README/CLAUDE (LIVE-ONLY)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts` (read a float/bool/handle field), `README.md`, `CLAUDE.md`.

**Interfaces:**
- Consumes: the Task-2 `EntityRef` typed methods; `Pawn`/`@s2script/cs2`; `__s2_schema_offset` (Slice 3, on the global); the committed `games/cs2/gamedata/schema-catalog.json` (to pick real field names/types).

- [ ] **Step 1: Pick real fields from the committed catalog.** From `games/cs2/gamedata/schema-catalog.json`, choose — on `CCSPlayerPawn` or a base class it inherits — one **float32** field, one **bool** field, and one **handle** (`{kind:"handle"}`) field that read sensibly on a live bot. Verify each name+kind against the catalog. Likely candidates (confirm before using): a float such as `m_flFriction`/`m_flGravityScale`; a bool such as `m_bClientSideRagdoll`; a handle such as `m_hOwnerEntity`/`m_hGroundEntity`. Record the three chosen names in the demo. (Reading the handle *on the pawn itself* keeps the demo simple — no controller ref to construct.)

- [ ] **Step 2: Extend the demo** (`examples/demo-plugin/src/plugin.ts`) to read the three typed fields via `p.ref` + demonstrate `readHandle` yielding a live `EntityRef`. Sketch (substitute the Step-1 field names):

```ts
import { OnGameFrame, EntityRef } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

// dev-facing: schema offsets resolved live (Slice 3). __s2_schema_offset is on the global.
declare const __s2_schema_offset: (cls: string, field: string) => number;

let ticks = 0;
export function onLoad(): void {
  console.log("[demo] onLoad (typed fields)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const p = Pawn.forSlot(0);              // Pawn | null (EntityRef-backed)
    if (!p) { console.log("[demo] no pawn"); return; }
    const FLOAT  = __s2_schema_offset("CCSPlayerPawn", "<CHOSEN_FLOAT_FIELD>");
    const BOOLF  = __s2_schema_offset("CCSPlayerPawn", "<CHOSEN_BOOL_FIELD>");
    const HANDLE = __s2_schema_offset("CCSPlayerPawn", "<CHOSEN_HANDLE_FIELD>");
    const f = FLOAT  >= 0 ? p.ref.readFloat32(FLOAT) : null;   // number  | null
    const b = BOOLF  >= 0 ? p.ref.readBool(BOOLF)    : null;   // boolean | null
    // handle field -> a live, serial-gated EntityRef (or null):
    const owner: EntityRef | null = HANDLE >= 0 ? p.ref.readHandle(HANDLE) : null;
    const ownerInfo = owner ? ("idx=" + owner.index + " valid=" + owner.isValid()) : "null";
    console.log("[demo] tick " + ticks + " health=" + p.health
      + " float=" + f + " bool=" + b + " handle=" + ownerInfo);
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```
**Note for the implementer:** the point of the handle read is proving `readHandle` returns a *live* `EntityRef` (not a raw pointer or a stale number). If the chosen handle field is itself a pawn/entity that exposes a readable field, chain one read through `owner` to make that vivid; otherwise logging `owner.index`/`owner.isValid()` suffices. Keep all CS2 field names in the demo plugin only (never in `core/src`).

- [ ] **Step 3: Build the demo `.s2sp` + sniper.**

```bash
cd /home/gkh/projects/s2script
node packages/cli/build.mjs
npx s2script build examples/demo-plugin
bash scripts/build-sniper.sh   # fresh s2script.so; must post-date the Task-2 commit
```
If a CS2 update reset `gameinfo.gi`, run `bash docker/patch-gameinfo.sh` first.

- [ ] **Step 4: Run the typed-field LIVE gate on Docker CS2.** Drop the demo; get the map ticking (`bot_quota 1`, `sv_hibernate_when_empty 0`; wait past the boot window) via `scripts/rcon.py` + logs:
  1. `[demo] tick … health=100 float=<sensible> bool=<true|false>` — the float + bool read sensible values, and the handle chain resolves a live pawn `EntityRef` (log its health via the chained ref).
  2. Kill the pawn (`bot_kick` / lethal damage — NOT `mp_restartgame`) → the reads go `null` (or `Pawn.forSlot` returns null); server keeps ticking, no crash.
  Capture the log excerpts. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done (demo, `.s2sp` + sniper, README/CLAUDE drafted) and report BLOCKED with the exact commands/errors so the controller can drive the gate.

- [ ] **Step 5: README + CLAUDE.md.** Add a `## Typed field access (Slice 5B.2)` section to `README.md` (the `EntityRef` typed-method surface — `readFloat32`/`readBool`/`readHandle`/… — + the captured live log). Update `CLAUDE.md` "## Current state": 5B.2 done (typed field access — scalars + handle over the kind-dispatched natives); "Current focus: Slice 5B.3 next" (the codegen). Do NOT alter the standing conventions above it.

- [ ] **Step 6: Final verification + commit** (do NOT commit build artifacts):

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md
git commit -m "feat(slice5b2): typed-field live gate — read float/bool/handle via EntityRef; README/CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §8)

1. `cargo test -p s2script-core` green (existing + the `entity.rs` typed-helper tests + the generic-native degrade tests + the repointed 5A test); both boundary gates green; sniper build OK.
2. `s2script build` produces the demo `.s2sp`.
3. The live gate passes: a float + bool read correct; `readHandle` yields a live `EntityRef`; all `null` on pawn death, no crash.
4. README documents the typed-field surface; CLAUDE.md "Current state" updated (5B.2 done, focus → 5B.3).
