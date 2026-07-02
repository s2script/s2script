# Slice 5B.2 — Typed Field Access

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5b2-typed-fields` (off `main`, which has Slices 0–5A + entref-wire + 5B.1 merged).
**Parent:** Slice 5B (schema codegen), sub-project 2 of 3: 5B.1 (catalog dump, done), 5B.2 (this — typed field access), 5B.3 (the codegen).

---

## 1. Goal — the closing thread

Expand entity field read/write beyond today's `i32`-only to the scalar types the schema catalog
surfaces — `float32`, `bool`, the integer widths (`i8`/`i16`/`u8`/`u16`/`u32`), enum-as-integer, and
`CHandle`→`EntityRef` — so the 5B.3 codegen can emit typed accessors and plugins can read/write floats,
bools, and entity-handle fields. Every access stays serial-gated (`T | null`); no raw pointer crosses
to JS. Acceptance: on a live CS2 server, read a real float, bool, and handle field of the pawn through
`EntityRef` (the handle yields a live `EntityRef`), all going `null` when the pawn dies.

## 2. What we build on (merged)

- **Slice 5A** — `EntityRef` (`{index, serial}`, engine-generic, in the `@s2script/std` prelude) with
  `isValid()`/`readInt32(off)`/`writeInt32(off,v)`/`notifyStateChanged(off)`. `entity_resolve_ptr(index,
  serial)` serial-gates the deref (validates against the engine `CEntityIdentity`, returns the live
  pointer or null; no raw ptr to JS). `core/src/entity.rs` holds the PURE pointer-arithmetic helpers
  `read_i32`/`write_i32`/`read_u32`/`read_ptr` + `decode_handle`/`resolve` (unit-tested).
  `__s2_handle_decode(handle uint32) -> [index, serial]` exists.
- **Slice 5B.1** — the schema catalog records each field's `type: {kind, name, inner}`
  (`atomic`/`class`/`enum`/`handle`/`ptr`/`unknown`). 5B.2 provides the runtime read/write primitives
  the 5B.3 codegen will emit calls to for `atomic`/`enum`/`handle` fields.

## 3. Decisions locked during brainstorming

1. **Scope = scalars + handle.** `float32`, `bool`, integer widths `i8`/`i16`/`u8`/`u16`/`u32` (+ the
   existing `i32`), enum read as its underlying integer width, and `CHandle`→`EntityRef`. This is one
   cohesive slice (extends the 5A pattern per type — no decomposition).
2. **Kind-dispatched natives** (not one native per type). Two generic natives dispatch on a small
   integer `kind` code — keeps the core native surface small and extensible as more types land.
3. **Vector/QAngle via composition** — because `float32` reads exist, a vector field is readable as
   three `readFloat32` calls (at `off`/`+4`/`+8`); the 5B.3 codegen emits that. A pretty `Vector`
   value type is `@s2script/std`-breadth (5C), not this slice.
4. **Deferred:** `i64`/`u64` (JS numbers are f64 — precise only ≤ 2^53; a `BigInt` path comes later);
   strings (`CUtlString`/`char[]` — representation-specific); embedded-struct nested accessors (codegen
   territory); the `Vector` value type; `write` for the narrow widths if the codegen doesn't need it
   yet (read-complete, write for the common types).

## 4. Architecture — kind-dispatched read/write over the 5A serial-gate

Two generic natives replace the "one native per type" approach; the author-facing `EntityRef` still
exposes **typed methods** (so authors and the codegen emit typed calls), each a thin wrapper that
passes its `kind` code to the generic native.

- **`core/src/entity.rs` (pure)** — add `read_f32`/`write_f32`, `read_bool`/`write_bool`,
  `read_i8`/`read_i16` (sign-extended to `i32`), `read_u8`/`read_u16` (zero-extended); `read_i32`/
  `write_i32`/`read_u32` exist. Same null/negative-offset guards. Unit-tested with `#[repr(C)]`
  fixtures.
- **`core/src/v8host.rs`** — two natives + a `kind`-code enum:
  - `__s2_ent_ref_read(index, serial, offset, kind) -> number | boolean | null`
  - `__s2_ent_ref_write(index, serial, offset, kind, value) -> boolean`
  Each: `catch_unwind`; `entity_resolve_ptr(index, serial)` (invalid → read `null` / write `false`, no
  raw ptr to JS); `match kind` → the `entity.rs` typed read/write; set the right JS value (`set_int32`
  for i8/i16/i32, `set_double` for u32/f32, `set_bool` for bool). An unknown `kind` → `null`/`false`.
  The generic native handles ALL scalar kinds **including `I32`**: `EntityRef.readInt32`/`writeInt32`
  are re-expressed over it, and the Slice-5A per-type `__s2_ent_ref_read_i32`/`write_i32` natives are
  REMOVED in the cutover — no behavior change (all method callers, incl. `pawn.js` via `Pawn.health`,
  use the `EntityRef` METHOD, not the native). The one Slice-5A test that called
  `__s2_ent_ref_read_i32` directly is repointed to the generic native. End state: exactly one read
  native + one write native for all scalar kinds.
- **`kind` codes** — a small integer enum, defined once in the `@s2script/std` prelude and mirrored as
  named consts in core (a tiny contract, documented in both like the ledger `Resource` variants):
  `F32`, `BOOL`, `I8`, `I16`, `I32`, `U8`, `U16`, `U32`.

## 5. `EntityRef` method surface (`@s2script/std`)

Typed wrappers, each passing its `kind` to `__s2_ent_ref_read`/`write`. All reads → `T | null`; writes
→ `boolean`:
- `readFloat32(off) → number|null` / `writeFloat32(off, v) → boolean`
- `readBool(off) → boolean|null` / `writeBool(off, v) → boolean`
- `readInt8(off)` / `readInt16(off)` / `readUInt8(off)` / `readUInt16(off)` / `readUInt32(off)` → `number|null`
  (write for the narrow widths deferred unless the codegen needs it)
- existing `readInt32`/`writeInt32` (may be re-expressed over the generic native, unchanged behavior)
- **`readHandle(off) → EntityRef | null`** — reads the `CHandle` uint32 at `off`, `__s2_handle_decode`s
  it to `[index, serial]`, and returns `new EntityRef(index, serial)` if the decoded handle is valid
  (not the invalid sentinel and `isValid()`), else `null`. So a *handle field* yields a live,
  serial-gated `EntityRef`.

Enum fields: the codegen reads the underlying integer width (e.g. `readInt32`); no distinct native.

## 6. Data flow

`pawn.readFloat32(off)` → `__s2_ent_ref_read(index, serial, off, F32)` → core `entity_resolve_ptr`
serial-checks → live ptr or null → `entity::read_f32(ptr, off)` → `rv.set_double` → `number`; a dead
entity → `null`. `readHandle(off)` → `__s2_ent_ref_read(..., U32)` gets the handle uint32 → JS
`__s2_handle_decode` → `[i, s]` → `new EntityRef(i, s)` (valid?) → a live ref the caller reads through.

## 7. Error handling — degrade-never-crash / `T | null`

Every native `catch_unwind`-wrapped; no panic across FFI. Invalid `(index, serial)` → read `null`,
write `false`, no deref. Negative/OOB offset guarded in `entity.rs` (returns 0 / no-op, and the read
native still returns via the serial gate). An unknown `kind` code → `null`/`false` (degrade, not a
crash). `readHandle` returns `null` on an invalid/zero handle. Writes never partially apply.

## 8. Testing & acceptance

**Cargo-unit-testable:**
- `entity.rs` (pure): `read_f32`/`write_f32`, `read_bool`/`write_bool`, `read_i8`/`i16`/`u8`/`u16`
  round-trip + sign/zero-extension + the null/negative-offset guards, via `#[repr(C)]` fixtures.
- In-isolate (`frame_tests`): with `set_engine_ops(None)`, `__s2_ent_ref_read`/`write` degrade for each
  `kind` (read → `null`, write → `false`), and an unknown `kind` → `null`/`false`; `EntityRef.readFloat32`/
  `readBool`/`readHandle` over null ops degrade (`null`), proving the kind routing + the `readHandle`
  wrapper are wired (a plain object couldn't do this).

**Live-only (the acceptance thread):** on Docker CS2, read a real **float** field, a **bool** field,
and a **handle** field of the pawn (offsets via `__s2_schema_offset`, class/field names in the demo
plugin only) through `EntityRef`: the float/bool read correct values; `readHandle` yields a live
`EntityRef` (chain to read a field of the referenced entity, e.g. the controller); then the pawn dies
→ all reads go `null`, no crash, server keeps ticking.

**Acceptance criteria:**
1. `cargo test -p s2script-core` green (existing + the new `entity.rs` + in-isolate tests); both
   boundary gates green; sniper build OK.
2. `s2script build` produces the demo `.s2sp`.
3. The live gate passes: float/bool read correct; `readHandle` → a live `EntityRef`; all `null` on
   pawn death, no crash.
4. README documents the typed-field usage; CLAUDE.md "Current state" updated (5B.2 done, focus → 5B.3).

## 9. File structure

- **Modify** `core/src/entity.rs` (the typed pure helpers + tests), `core/src/v8host.rs` (the two
  generic natives + `kind` consts + install; the `@s2script/std` prelude `EntityRef` methods + `kind`
  enum), `packages/std/index.d.ts` (the new `EntityRef` method signatures).
- **Modify** a demo (extend `examples/demo-plugin` or a small demo) to read a float/bool/handle field
  for the live gate; `README.md`, `CLAUDE.md`.

No new C-ABI/shim change, no game-package change beyond the demo — this is a core + `@s2script/std`
expansion.

## 10. Scope & deferrals

**Scope:** the scalar + handle read/write primitives + the `EntityRef` typed methods.

**Deferred (do NOT build):** `i64`/`u64` (BigInt path later); a `Vector`/`QAngle` value type + strings
+ embedded-struct accessors (5C / codegen); `write` for narrow widths if unused; the 5B.3 codegen
itself (this only adds the primitives it will call); the `@s2script/std` module split + breadth (5C);
the `tsc` gate; config/permissions; the registry (5.5); the base-plugin suite (6).

## 11. Global constraints (bind every task)

- **Core stays engine-generic.** No CS2 identifiers (incl. string literals in core tests), no
  `include_str!`/`games/` in `core/src`. `EntityRef` + the natives are engine-generic; CS2 class/field
  names live only in the demo plugin / `games/cs2/js`. Both boundary gates green.
- **Never expose a raw pointer across time.** JS holds only `{index, serial}`; every deref is
  serial-gated via `entity_resolve_ptr`; the typed reads compose it, never surfacing a pointer.
- **`T | null` / degrade-never-crash.** Every native `catch_unwind`; invalid/unknown-kind → `null`/
  `false`; no stale deref; no panic across FFI.
- **Layout is data.** Offsets are resolved live (`__s2_schema_offset`); no offsets baked into code.
- **Naming:** PascalCase types (`EntityRef`), camelCase fns/props (`readFloat32`, `readHandle`).
- **cdylib test constraint:** unit tests inline `#[cfg(test)] mod` in the source file.
- **`kind` codes are a JS↔core contract** — defined once in the prelude, mirrored as named core
  consts, documented in both; keep them in lockstep.
