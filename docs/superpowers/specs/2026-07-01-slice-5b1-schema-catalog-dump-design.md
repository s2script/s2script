# Slice 5B.1 — Schema Catalog Dump (the schema codegen's source of truth)

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5b1-schema-dump` (off `main`, which has Slices 0–5A + the EntityRef-wire fast-follow merged).
**Parent:** Slice 5B (schema codegen), decomposed into 5B.1 (this — the catalog dump), 5B.2 (typed field access), 5B.3 (the codegen). 5B.1 first (build-by-risk: the enumeration is the novel engine capability everything else consumes).

---

## 1. Goal — the closing thread

Enumerate the live in-process `SchemaSystem` and dump the full class/field/type catalog to a
committed, regenerable JSON file. This is the source of truth the codegen (5B.3) consumes to generate
`@s2script/cs2`'s typed accessors + `.d.ts`, and the "regenerable schema file" the maintenance
treadmill regenerates after every CS2 update. Acceptance: on a live CS2 server, dump a
`schema-catalog.json` that spot-checks correct — `CCSPlayerPawn` present, `m_iHealth` recorded as
`int32` at the offset `__s2_schema_offset` also returns, a `CHandle<T>` field encoded as a `handle`.

## 2. What we build on (merged)

- **Slice 3 schema** (`core/src/v8host.rs` `__s2_schema_offset`; `core/src/schema.rs` `OffsetCache`): the
  shim's `SchemaOffsetFn = fn(class, field) -> c_int` is **lookup-only** — it resolves ONE known
  `(class, field)` to an offset, with no enumeration and no field TYPE. `OffsetCache` caches hits +
  a miss-once-WARN policy. The runtime resolves offsets LIVE from the in-process SchemaSystem.
- **Slice 5A `EntityRef`** + `core/src/entity.rs` (read helpers) — consumed by 5B.2/5B.3, not 5B.1.
- **`S2EngineOps`** C-ABI fn-pointer table — the shim provides engine access; core does the logic
  (the 5A pattern: shim hands core a pointer, core walks the layout via offset constants).

**Today's gap:** there is no way to enumerate the schema (all classes → all fields → name/type/offset).
5B.1 adds that.

## 3. Decisions locked during brainstorming

1. **Scope = full catalog.** Enumerate ALL declared classes + fields + types (the walk is the same
   effort as a filtered one; filtering would be extra inheritance logic; the codegen filters what it
   emits). The committed JSON is large but is regenerable data (like SourceMod's gamedata files).
2. **Approach A — live SchemaSystem dump** (not hl2sdk-header parsing): authoritative (matches the
   exact running binary), treadmill-aligned (correct the moment CS2 updates). hl2sdk lags Valve, so
   header-parsing is at best a future offline cross-check, not the source of truth.
3. **Runtime stays live-resolve.** The catalog records offsets for reference/diffing, but the runtime
   (and generated accessors, 5B.3) NEVER read the catalog's offsets — they resolve live via
   `__s2_schema_offset` ("a field-offset change must never require a code change").
4. **Trigger = a native + a dev dump plugin.** CS2 doesn't export `ConCommand::Create` (Slice-3 memory),
   so no console command. A native `__s2_schema_dump(path) -> bool` writes the catalog; a tiny dev
   dump plugin calls it on `OnGameFrame` once the schema is ready.

## 4. Architecture — shim hands the type-scope pointer, core walks the layout

Mirrors the 5A split: the shim provides a pointer, core reads the struct layout via offset constants,
staying **engine-generic** (it reads a *schema* layout + builds JSON — no CS2 class name lives in core;
class/field names are DATA read out of the schema at runtime).

- **New shim op** `SchemaTypeScopeFn = fn() -> *mut c_void` — returns the global `CSchemaSystemTypeScope`
  pointer (the shim already reaches the SchemaSystem for `schema_offset`).
- **Core enumeration** walks: type scope → its class-bindings container → each `CSchemaClassBinding`
  `{name, base-class ptr, field count, fields ptr}` → each `CSchemaClassFieldData`
  `{name, offset, type ptr}` → `CSchemaType` `{category, name}`. Offsets/container layout are named
  engine-generic Source 2 constants (`// TODO(gamedata)` migration). Core builds the catalog + writes
  JSON. No plugin V8 context is required for the walk (it's a pure engine read + serialize).

## 5. Catalog format

Committed to `games/cs2/gamedata/schema-catalog.json` (regenerable data):
```json
{
  "CCSPlayerPawn": {
    "parent": "CCSPlayerPawnBase",
    "fields": [
      { "name": "m_iHealth",     "offset": 844, "type": { "kind": "atomic", "name": "int32" } },
      { "name": "m_hController",  "offset": 812, "type": { "kind": "handle", "inner": "CCSPlayerController" } },
      { "name": "m_vecOrigin",    "offset": 300, "type": { "kind": "class",  "name": "Vector" } }
    ]
  },
  "CBaseEntity": { "parent": "CEntityInstance", "fields": [ ... ] }
}
```
- `parent`: the base class name (or `null`) — feeds codegen inheritance (5B.3).
- `type.kind` ∈ `atomic` (name = `int32`/`float32`/`bool`/`CUtlString`/…), `handle` (`CHandle<T>` →
  `inner` = T's class name), `class` (embedded struct, `name` = class), `ptr` (`inner` = pointee),
  `enum` (`name`, `underlying`), plus a fallback `unknown` (`raw` = the category + type-name string)
  for categories not yet mapped — degrade-per-descriptor, never drop the field silently.
- `offset`: recorded for reference/diffing; the runtime resolves live and never reads it.

## 6. Trigger + data flow

`__s2_schema_dump(path) -> boolean` (native): if the schema isn't ready (type scope null / no classes)
→ return `false`, write nothing. Else enumerate → build the catalog → `std::fs::write(path, json)` →
return `true`. The dev dump plugin (`examples/schema-dump`) subscribes `OnGameFrame` and calls it once
after the schema is populated (a few frames into a live map), logging the outcome. Treadmill flow: on
update day, run the server on a map, drop the dump plugin, commit the regenerated
`schema-catalog.json`.

## 7. Front-loaded spike (the engine-touchpoint unknown)

Confirm the enumeration walk on a live server; findings → a dated spike doc:
1. The global `CSchemaSystemTypeScope` pointer (the shim's `schema_offset` path already reaches it —
   expose it via the new op).
2. The class-bindings container layout (array vs `CUtlTSHash`/hashtable — count + element access) and
   `CSchemaClassBinding` `{name, base, field count, fields ptr}` offsets.
3. `CSchemaClassFieldData` `{name, offset, type ptr}` offsets + `CSchemaType` `{category, name}` and the
   category enum values → the `type.kind` mapping.
4. Validate: enumerate `CCSPlayerPawn`, confirm `m_iHealth` = int32 at the offset `__s2_schema_offset`
   returns, and a `CHandle<>` field maps to `{kind:"handle"}`.
If the enumeration proves unworkable, revise before the load-bearing work. Escalation path for the live
RE (like the 5A spike).

## 8. Error handling — degrade-per-descriptor, never crash globally

Every native `catch_unwind`-wrapped (no panic across FFI). The walk guards each pointer hop (null →
skip that class/field with a WARN, continue); an unmapped `CSchemaType` category → `{kind:"unknown",
raw:...}` (recorded, not dropped, not fatal). Schema-not-ready → `false`, no file written (retry).
`std::fs::write` failure → WARN + `false`.

## 9. Testing & acceptance

**Cargo-unit-testable** (pure, in `core/src/schema.rs` or a new `core/src/schema_catalog.rs`, inline
`#[cfg(test)] mod`): the catalog-building + JSON serialization + the type-category → `type.kind`
mapping, driven from a SYNTHETIC in-memory schema representation (a small Rust fixture of
classes/fields/types → assert the emitted JSON structure, incl. the `handle`/`class`/`unknown` cases).
No engine needed.

**Live-only (the acceptance thread):** dump on a live CS2 server → a committed `schema-catalog.json`;
spot-check it: `CCSPlayerPawn` present with a `parent`; `m_iHealth` = `{kind:"atomic",name:"int32"}` at
the offset `__s2_schema_offset("CCSPlayerPawn","m_iHealth")` returns; at least one `handle` field and
one `class`/embedded field present; the file parses as valid JSON with many classes (full catalog).

**Acceptance criteria:**
1. `cargo test -p s2script-core` green (existing + new catalog-build unit tests); both boundary gates
   green; sniper build OK.
2. `s2script build` produces a loadable `schema-dump` `.s2sp`.
3. The live dump writes a valid `schema-catalog.json` that passes the spot-checks above.
4. README documents the dump/treadmill runbook; CLAUDE.md "Current state" notes 5B.1 done (catalog
   dump) + focus → 5B.2.

## 10. File structure

- **Modify** `core/src/v8host.rs` (`__s2_schema_dump` native + install; the enumeration walk, or call
  into schema_catalog.rs), `core/src/schema.rs` OR **create** `core/src/schema_catalog.rs` (the pure
  catalog-build + serialization + type-mapping, unit-tested), `core/src/lib.rs` (if a new module),
  the `S2EngineOps` `SchemaTypeScopeFn` field.
- **Modify** the shim (`shim/include/s2script_core.h` + `shim/src/s2script_mm.cpp`) — the new
  type-scope-pointer op.
- **Create** `examples/schema-dump/{package.json, src/plugin.ts}` (the dev dump plugin); the
  spike-findings doc; commit `games/cs2/gamedata/schema-catalog.json` (the dumped catalog).
- **Modify** `README.md`, `CLAUDE.md`.

## 11. Scope & deferrals

**Scope:** 5B.1 = the enumeration + the committed catalog only.

**Deferred (do NOT build):** typed field read/write natives beyond `i32` (5B.2); the codegen —
`.d.ts` + generated accessors (5B.3); consuming the catalog at runtime (never — runtime stays
live-resolve); hl2sdk-header parsing / offline cross-check; migrating the schema-struct offsets into a
formal gamedata pipeline (named constants + `TODO(gamedata)` for now); the `@s2script/std` module
split + std breadth (5C); the tsc gate; config/permissions; the registry (5.5); the base-plugin suite
(6).

## 12. Global constraints (bind every task)

- **Core stays engine-generic.** No CS2 identifiers, no `include_str!`/`games/` in `core/src`. The
  enumeration reads a *schema layout* + emits class/field names as DATA (read from the engine at
  runtime), never a hardcoded CS2 name. Both boundary gates green.
- **Layout is data, semantics are code.** Schema-struct offsets are named constants with
  `// TODO(gamedata)`; the catalog is regenerable data; the runtime resolves offsets live (never bakes
  them).
- **Degrade-per-descriptor, never crash globally.** A broken class/field/type → skip with a named
  WARN (or `{kind:"unknown"}`), never fatal; `catch_unwind` on the native; no panic across FFI.
- **cdylib test constraint:** unit tests inline `#[cfg(test)] mod` in the source file.
- **Naming:** PascalCase types, camelCase fns/props.
- The dump native + the dev dump plugin are dev/treadmill tooling; a permission model over
  file-writing natives is deferred (permissions are a later slice).
