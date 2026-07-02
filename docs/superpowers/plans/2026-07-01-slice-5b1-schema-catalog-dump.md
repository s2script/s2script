# Slice 5B.1 — Schema Catalog Dump Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enumerate the live `SchemaSystem` and dump the full class/field/type catalog to a committed, regenerable `games/cs2/gamedata/schema-catalog.json` — the source of truth the codegen (5B.3) will consume.

**Architecture:** The shim (which already links the hl2sdk `<schemasystem/schemasystem.h>` and uses its typed APIs for `schema_offset`) enumerates the server type scope's declared classes via the SDK and streams each class/field to core through C-ABI callbacks (`emit_class`/`emit_field`). Core assembles them into an in-memory catalog and serializes JSON — a pure, unit-testable path. Offsets are recorded for reference only; the runtime always resolves live.

**Tech Stack:** Rust `cdylib` core (rusty_v8, serde_json), the C++ Metamod shim (hl2sdk SchemaSystem), Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-07-01-slice-5b1-schema-catalog-dump-design.md`.

## Global Constraints

Every task's requirements implicitly include these (from spec §12):

- **Core stays engine-generic.** No CS2 identifiers, no `include_str!`/`games/` in `core/src`. The enumeration reads class/field NAMES as DATA streamed from the engine via callbacks — never hardcoded. The SDK-typed walk lives in the SHIM (which legitimately knows Source 2 schema types). Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Layout is data.** No raw schema-struct offsets in `core/src`; any shim-side raw member offset the spike requires is a named constant with `// TODO(gamedata)`. The runtime resolves field offsets live (never bakes them); the catalog's offsets are reference/diff only.
- **Degrade-per-descriptor, never crash globally.** `catch_unwind` on the native AND on every C-ABI callback body (a callback invoked from C++ must never unwind across the FFI boundary). A broken class/field → skip with a named WARN; an unmapped `CSchemaType` category → `{kind:"unknown", ...}` (recorded, not dropped); schema-not-ready / null ops / write failure → `false`, no file.
- **cdylib test constraint:** unit tests inline `#[cfg(test)] mod` in the source file — never `core/tests/`.
- **Naming:** PascalCase types, camelCase fns/props.
- **Commit trailer:** every commit ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5b1-schema-dump`; do not push.

**Deferred — do NOT build:** typed field read/write natives beyond `i32` (5B.2); the codegen / `.d.ts` / generated accessors (5B.3); the runtime EVER consuming the catalog (it stays live-resolve); hl2sdk-header offline parsing; the `@s2script/std` module split + breadth (5C); the `tsc` gate; config/permissions; the registry (5.5); the base-plugin suite (6).

---

## File Structure

- **Create `core/src/schema_catalog.rs`** — the pure catalog builder (`Catalog` + `add_class`/`add_field`/`to_json`) + serde types; unit-tested from synthetic calls. `mod schema_catalog;` in `core/src/lib.rs`.
- **Modify `core/src/v8host.rs`** — the `S2EngineOps.schema_enumerate` field + callback typedefs; the `cb_emit_class`/`cb_emit_field` C-ABI callbacks; the `__s2_schema_dump` native + install.
- **Modify `shim/include/s2script_core.h`** — the `s2_schema_enumerate_fn` + emit-callback typedefs in `S2EngineOps`.
- **Modify `shim/src/s2script_mm.cpp`** — implement `schema_enumerate` (SDK walk) + wire it into the ops table.
- **Create `examples/schema-dump/{package.json, src/plugin.ts}`** — the dev dump plugin.
- **Create** the spike-findings doc; **commit** `games/cs2/gamedata/schema-catalog.json` (the dumped catalog).
- **Modify `README.md`, `CLAUDE.md`.**

---

## Task 1: Spike — SDK schema enumeration (RECON, throwaway, LIVE)

**Files:**
- Create: `docs/superpowers/specs/2026-07-01-slice-5b1-spike-findings.md`
- Scratch (temporary): a throwaway enumeration in `shim/src/s2script_mm.cpp` (logging), removed at the end.

**Interfaces:**
- Consumes: the shim's `s_pSchemaSystem` (`ISchemaSystem*`), its `FindTypeScopeForModule("libserver.so")` / `GlobalTypeScope()` path, the hl2sdk `<schemasystem/schemasystem.h>` types; `scripts/build-sniper.sh`, Docker CS2, `scripts/rcon.py`, `docker/patch-gameinfo.sh`.
- Produces: the confirmed SDK enumeration recipe + the `CSchemaType` category → `kind` mapping. No production code.

This is LIVE C++/SDK reverse-engineering. Escalation: if the live infra won't cooperate after reasonable attempts, report BLOCKED with the exact commands/errors so the controller can drive it.

- [ ] **Step 1: Find how to iterate a type scope's declared classes.** In the shim (scratch), from the server `CSchemaSystemTypeScope*`, inspect the hl2sdk header for the declared-classes container (commonly `m_DeclaredClasses`, a `CUtlTSHash<CSchemaClassBinding*>` — iterate via its element/handle API) or a `GetClassBindings()`-style accessor. If the header exposes NO iteration (only `FindDeclaredClass` by name), record that and fall back to a raw walk of the container member using the SDK type as the base + the member offset (spike-confirmed) — still in C++. Log the class count + a few class names.

- [ ] **Step 2: Confirm class + field + base access.** For a `CSchemaClassInfo`/`CSchemaClassBinding`: its name, its base class (the SDK's `m_pBaseClasses`/`base.m_pClass` used by the existing `schema_find_field`), and its fields (`m_pFields` + `m_nFieldCount`, or the SDK's field accessor). For a `CSchemaClassFieldData`: `m_pszName`, `m_nSingleInheritanceOffset`, `m_pSchemaType`. Log a few `(class, field, offset)` triples.

- [ ] **Step 3: Map the `CSchemaType` category → `kind`.** Read `CSchemaType`'s category enum (`m_eTypeCategory` / `GetTypeCategory()`) + `m_pszName`. Determine the enum values for atomic (builtin like int32/float32/bool), class/struct, pointer, enum, and how a `CHandle<T>` (an atomic/template) surfaces its inner class name. Record the enum-value → `kind` string mapping (`atomic`/`handle`/`class`/`ptr`/`enum`/`unknown`).

- [ ] **Step 4: Validate.** Enumerate `CCSPlayerPawn`: confirm `m_iHealth` = `int32` at the same offset `s2_schema_offset("CCSPlayerPawn","m_iHealth")` returns, and a `CHandle<>` field → `kind="handle"` with the inner class name. Confirm the total class count is large (full catalog feasible).

- [ ] **Step 5: Write findings.** Fill `docs/superpowers/specs/2026-07-01-slice-5b1-spike-findings.md` with: the class-iteration recipe (SDK API or the fallback member offset), the field/base access, the category→`kind` mapping table, the validation evidence, and a **GO/NO-GO**. If NO-GO (SDK can't enumerate even via a shim raw walk), state the blocker and stop.

- [ ] **Step 6: Remove scratch, commit the findings doc only.**

```bash
bash scripts/build-sniper.sh   # confirm the shim still builds after removing scratch
git add docs/superpowers/specs/2026-07-01-slice-5b1-spike-findings.md
git commit -m "docs(slice5b1): spike findings — SDK schema enumeration recipe + category->kind map

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: `schema_catalog.rs` — the pure catalog builder (PURE / cargo-unit)

**Files:**
- Create: `core/src/schema_catalog.rs`
- Modify: `core/src/lib.rs` (add `mod schema_catalog;` near `mod schema;`)

**Interfaces:**
- Consumes: `serde` + `serde_json` (already dependencies — `loader.rs` uses them).
- Produces (used by v8host in Task 3): `Catalog::new()`, `Catalog::add_class(&mut self, name: &str, parent: Option<&str>)`, `Catalog::add_field(&mut self, class: &str, name: &str, offset: i32, kind: &str, type_name: Option<&str>, inner: Option<&str>)`, `Catalog::to_json(&self) -> String`, `Catalog::class_count(&self) -> usize`.

- [ ] **Step 1: Write the failing tests** (`core/src/schema_catalog.rs`, test module first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn built() -> Catalog {
        let mut c = Catalog::new();
        c.add_class("CCSPlayerPawn", Some("CBaseEntity"));
        c.add_field("CCSPlayerPawn", "m_iHealth", 844, "atomic", Some("int32"), None);
        c.add_field("CCSPlayerPawn", "m_hController", 812, "handle", None, Some("CCSPlayerController"));
        c.add_class("CBaseEntity", None); // root: no parent
        c.add_field("CBaseEntity", "m_vecOrigin", 300, "class", Some("Vector"), None);
        c
    }

    #[test]
    fn serializes_classes_fields_and_types() {
        let v: Value = serde_json::from_str(&built().to_json()).unwrap();
        assert_eq!(v["CCSPlayerPawn"]["parent"], "CBaseEntity");
        let f0 = &v["CCSPlayerPawn"]["fields"][0];
        assert_eq!(f0["name"], "m_iHealth");
        assert_eq!(f0["offset"], 844);
        assert_eq!(f0["type"]["kind"], "atomic");
        assert_eq!(f0["type"]["name"], "int32");
        assert!(f0["type"].get("inner").is_none(), "atomic has no inner");
        let f1 = &v["CCSPlayerPawn"]["fields"][1];
        assert_eq!(f1["type"]["kind"], "handle");
        assert_eq!(f1["type"]["inner"], "CCSPlayerController");
        assert!(f1["type"].get("name").is_none(), "handle has no name");
    }

    #[test]
    fn root_class_omits_parent() {
        let v: Value = serde_json::from_str(&built().to_json()).unwrap();
        assert!(v["CBaseEntity"].get("parent").is_none(), "root class has no parent key");
    }

    #[test]
    fn output_is_deterministic_across_identical_builds() {
        // classes sorted (BTreeMap); fields in insertion order — a stable committed file.
        assert_eq!(built().to_json(), built().to_json());
    }

    #[test]
    fn add_field_to_unknown_class_is_defensive_no_panic() {
        let mut c = Catalog::new();
        c.add_field("CNeverAdded", "x", 0, "atomic", Some("int32"), None); // must not panic
        // The field is dropped (no class) — degrade, not crash.
        assert_eq!(c.class_count(), 0);
    }

    #[test]
    fn unknown_kind_round_trips() {
        let mut c = Catalog::new();
        c.add_class("C", None);
        c.add_field("C", "weird", 4, "unknown", Some("SomeExoticType"), None);
        let v: Value = serde_json::from_str(&c.to_json()).unwrap();
        assert_eq!(v["C"]["fields"][0]["type"]["kind"], "unknown");
        assert_eq!(v["C"]["fields"][0]["type"]["name"], "SomeExoticType");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core schema_catalog:: -- --test-threads=1`
Expected: FAIL to compile (module/types not found).

- [ ] **Step 3: Implement** (above the test module):

```rust
//! Pure, engine-generic schema catalog builder (V8-free, no CS2 identifiers). The live SDK walk lives
//! in the shim; it streams classes/fields here via v8host's C-ABI callbacks. This module only
//! assembles + serializes — so it is fully unit-testable without an engine.

use serde::Serialize;
use std::collections::BTreeMap;

/// A field's type. `kind` ∈ atomic | handle | class | ptr | enum | unknown (the shim maps the
/// CSchemaType category → this string). `name` = the type name for atomic/class/enum; `inner` = the
/// referenced class for handle/ptr. Absent fields are omitted from JSON.
#[derive(Serialize)]
pub struct FieldType {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner: Option<String>,
}

#[derive(Serialize)]
pub struct Field {
    pub name: String,
    pub offset: i32,
    #[serde(rename = "type")]
    pub ty: FieldType,
}

#[derive(Serialize)]
pub struct Class {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub fields: Vec<Field>,
}

/// The catalog. Classes are keyed in a BTreeMap for deterministic (sorted) output; fields keep
/// insertion order (the shim emits them in schema order, which is stable per binary).
pub struct Catalog {
    classes: BTreeMap<String, Class>,
}

impl Catalog {
    pub fn new() -> Self {
        Self { classes: BTreeMap::new() }
    }

    /// Record a class (idempotent: a repeat keeps the first, so a duplicate emit is harmless).
    pub fn add_class(&mut self, name: &str, parent: Option<&str>) {
        self.classes.entry(name.to_string()).or_insert_with(|| Class {
            parent: parent.map(|p| p.to_string()),
            fields: Vec::new(),
        });
    }

    /// Append a field to its class. If the class was never added, the field is dropped (degrade,
    /// never panic) — the shim always emits the class before its fields.
    pub fn add_field(&mut self, class: &str, name: &str, offset: i32, kind: &str, type_name: Option<&str>, inner: Option<&str>) {
        if let Some(c) = self.classes.get_mut(class) {
            c.fields.push(Field {
                name: name.to_string(),
                offset,
                ty: FieldType {
                    kind: kind.to_string(),
                    name: type_name.map(|s| s.to_string()),
                    inner: inner.map(|s| s.to_string()),
                },
            });
        }
    }

    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// Serialize to pretty JSON (stable order). Returns "{}" on the (impossible) serialization error.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.classes).unwrap_or_else(|_| "{}".to_string())
    }
}

impl Default for Catalog {
    fn default() -> Self { Self::new() }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p s2script-core schema_catalog:: -- --test-threads=1`
Expected: PASS (5 tests).

- [ ] **Step 5: Full suite + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/schema_catalog.rs core/src/lib.rs
git commit -m "feat(slice5b1): pure schema catalog builder (classes/fields/types -> stable JSON)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

Expected: green; both gates pass (no CS2 ids — the module is pure).

---

## Task 3: `schema_enumerate` op + `__s2_schema_dump` native (core cargo-unit + shim live)

**Files:**
- Modify: `shim/include/s2script_core.h` (the enumerate + emit typedefs in `S2EngineOps`), `shim/src/s2script_mm.cpp` (implement `schema_enumerate` + wire it into the ops table), `core/src/v8host.rs` (the `S2EngineOps` field + callbacks + native + install).

**Interfaces:**
- Consumes: Task 2's `Catalog`; the existing `ENGINE_OPS`, `set_native`, native/`catch_unwind` idioms; the shim's `s_pSchemaSystem` + type scope; the spike's enumeration recipe + category→`kind` map.
- Produces: the native `__s2_schema_dump(path: string) -> boolean`; the ops `schema_enumerate` contract.

- [ ] **Step 1: Add the C typedefs + `S2EngineOps` field (shim header).** In `shim/include/s2script_core.h`, next to the existing `S2EngineOps` typedefs:

```c
/* Schema enumeration (5B.1). The shim walks the SchemaSystem and streams each class/field to core
 * via these callbacks (core provides them + an opaque ctx). kind ∈ atomic|handle|class|ptr|enum|unknown.
 * A null parent/name/inner is an absent value. */
typedef void (*s2_emit_class_fn)(void* ctx, const char* name, const char* parent);
typedef void (*s2_emit_field_fn)(void* ctx, const char* cls, const char* name, int offset,
                                 const char* kind, const char* type_name, const char* inner);
typedef int  (*s2_schema_enumerate_fn)(void* ctx, s2_emit_class_fn emit_class, s2_emit_field_fn emit_field);
```
Add `s2_schema_enumerate_fn schema_enumerate;` to the `S2EngineOps` struct (append — keep existing field order).

- [ ] **Step 2: Add the Rust `#[repr(C)]` mirror + callback typedefs (`core/src/v8host.rs`).** Next to the existing `SchemaOffsetFn` etc.:

```rust
pub type EmitClassFn = extern "C" fn(ctx: *mut c_void, name: *const c_char, parent: *const c_char);
pub type EmitFieldFn = extern "C" fn(
    ctx: *mut c_void, cls: *const c_char, name: *const c_char, offset: c_int,
    kind: *const c_char, type_name: *const c_char, inner: *const c_char,
);
pub type SchemaEnumerateFn = extern "C" fn(ctx: *mut c_void, emit_class: EmitClassFn, emit_field: EmitFieldFn) -> c_int;
```
Add `pub schema_enumerate: Option<SchemaEnumerateFn>,` to the `S2EngineOps` struct (append, matching the C order). (Returns `c_int` — NOT `bool` — to match the C `int` return; core treats `!= 0` as success.)

- [ ] **Step 3: Write the failing tests** (append to `#[cfg(test)] mod frame_tests`). A STUB `schema_enumerate` (a test-only `extern "C"` fn) drives the full core path with NO real shim — proving the callbacks + catalog + serialize + file-write end-to-end:

```rust
    // A stub shim-side enumerate: emits one class + two fields via the core callbacks.
    extern "C" fn stub_enumerate(ctx: *mut c_void, ec: EmitClassFn, ef: EmitFieldFn) -> c_int {
        ec(ctx, b"CTest\0".as_ptr() as *const c_char, b"CBase\0".as_ptr() as *const c_char);
        ef(ctx, b"CTest\0".as_ptr() as *const c_char, b"m_x\0".as_ptr() as *const c_char, 8,
           b"atomic\0".as_ptr() as *const c_char, b"int32\0".as_ptr() as *const c_char, std::ptr::null());
        ef(ctx, b"CTest\0".as_ptr() as *const c_char, b"m_h\0".as_ptr() as *const c_char, 12,
           b"handle\0".as_ptr() as *const c_char, std::ptr::null(), b"CThing\0".as_ptr() as *const c_char);
        1
    }

    #[test]
    fn schema_dump_writes_catalog_via_stub_enumerate() {
        let _ = init(dummy_logger());
        // Wire an ops table whose schema_enumerate is the stub (other fields None).
        set_engine_ops(Some(S2EngineOps {
            schema_offset: None, ent_by_index: None, deref_handle: None,
            ent_state_changed: None, concommand_register: None,
            schema_enumerate: Some(stub_enumerate),
        }));
        create_plugin_context("p");
        let path = std::env::temp_dir().join("s2_schema_test.json");
        let path_s = path.to_string_lossy().replace('\\', "\\\\");
        let ok = eval_in_context_string("p", &format!("String(__s2_schema_dump(\"{}\"))", path_s));
        assert_eq!(ok, "true");
        let written = std::fs::read_to_string(&path).expect("catalog file written");
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["CTest"]["parent"], "CBase");
        assert_eq!(v["CTest"]["fields"][0]["name"], "m_x");
        assert_eq!(v["CTest"]["fields"][0]["type"]["kind"], "atomic");
        assert_eq!(v["CTest"]["fields"][1]["type"]["inner"], "CThing");
        let _ = std::fs::remove_file(&path);
        shutdown();
    }

    #[test]
    fn schema_dump_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);              // no ops table → no schema_enumerate → false, no file
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_schema_dump(\"/tmp/should_not_exist.json\"))"), "false");
        shutdown();
    }
```

Note: this test constructs an `S2EngineOps` literal — if the struct has more/fewer fields, set the new one (`schema_enumerate`) and the rest to `None`. If `set_engine_ops`/`eval_in_context_string`/`create_plugin_context` need a `super::` prefix from `frame_tests`, add it.

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p s2script-core frame_tests::schema_dump -- --test-threads=1`
Expected: FAIL — `__s2_schema_dump` / `schema_enumerate` not defined.

- [ ] **Step 5: Add the core callbacks + the native (`core/src/v8host.rs`).**

```rust
/// C-ABI callback: the shim calls this per class. Body under catch_unwind — it is invoked FROM C++
/// and must never unwind across the FFI boundary.
extern "C" fn cb_emit_class(ctx: *mut c_void, name: *const c_char, parent: *const c_char) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() || name.is_null() { return; }
        let catalog = unsafe { &mut *(ctx as *mut crate::schema_catalog::Catalog) };
        let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        let parent = if parent.is_null() { None } else { Some(unsafe { CStr::from_ptr(parent) }.to_string_lossy().into_owned()) };
        catalog.add_class(&name, parent.as_deref());
    }));
}

/// C-ABI callback: the shim calls this per field. Same catch_unwind discipline.
extern "C" fn cb_emit_field(
    ctx: *mut c_void, cls: *const c_char, name: *const c_char, offset: c_int,
    kind: *const c_char, type_name: *const c_char, inner: *const c_char,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() || cls.is_null() || name.is_null() || kind.is_null() { return; }
        let catalog = unsafe { &mut *(ctx as *mut crate::schema_catalog::Catalog) };
        let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        let opt = |p: *const c_char| if p.is_null() { None } else { Some(s(p)) };
        catalog.add_field(&s(cls), &s(name), offset as i32, &s(kind), opt(type_name).as_deref(), opt(inner).as_deref());
    }));
}

/// Native `__s2_schema_dump(path: string) -> boolean`. Drives the shim's schema_enumerate with the
/// core callbacks into a Catalog, then serializes + writes the file. Degrade-never-crash.
fn s2_schema_dump(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let path = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else {
            log_warn("WARN: __s2_schema_dump: no engine ops table");
            return;
        };
        let Some(enumerate) = ops.schema_enumerate else {
            log_warn("WARN: __s2_schema_dump: schema_enumerate not wired in ops");
            return;
        };
        let mut catalog = crate::schema_catalog::Catalog::new();
        let ok = enumerate(&mut catalog as *mut _ as *mut c_void, cb_emit_class, cb_emit_field);
        if ok == 0 || catalog.class_count() == 0 {
            log_warn("WARN: __s2_schema_dump: schema not ready (no classes) — try again once a map is live");
            return;
        }
        match std::fs::write(&path, catalog.to_json()) {
            Ok(()) => rv.set_bool(true),
            Err(e) => log_warn(&format!("WARN: __s2_schema_dump: write '{}' failed: {}", path, e)),
        }
    }));
}
```
Install in `install_natives`: `set_native(scope, global_obj, "__s2_schema_dump", s2_schema_dump);`

- [ ] **Step 6: Implement `schema_enumerate` in the shim (`shim/src/s2script_mm.cpp`).** Using the spike's recipe — iterate the server type scope's declared classes; for each, call `emit_class(ctx, name, parentName)`; for each field, map its `CSchemaType` category → the `kind` string and call `emit_field(...)`. Skeleton (fill the iteration + category switch from the spike findings):

```cpp
// Schema enumeration engine-op (5B.1). Walks the server type scope's declared classes via the SDK
// and streams each class/field to core. Degrade-never-crash: null system/scope -> return false.
static int schema_enumerate(void* ctx, s2_emit_class_fn emit_class, s2_emit_field_fn emit_field) {
    if (!s_pSchemaSystem) return 0;
    CSchemaSystemTypeScope* scope = s_pSchemaSystem->FindTypeScopeForModule("libserver.so");
    if (!scope) scope = s_pSchemaSystem->GlobalTypeScope();
    if (!scope) return 0;

    // --- iterate declared classes (SDK recipe from the spike; e.g. m_DeclaredClasses CUtlTSHash) ---
    // for each CSchemaClassInfo* info in scope's declared classes:
    //     const char* parent = info->m_nBaseClassCount ? info->m_pBaseClasses[0].m_pClass->m_pszName : nullptr;
    //     emit_class(ctx, info->m_pszName, parent);
    //     for (int i = 0; i < info->m_nFieldCount; ++i) {
    //         const CSchemaClassFieldData& f = info->m_pFields[i];
    //         const char* kind; const char* type_name = nullptr; const char* inner = nullptr;
    //         schema_type_to_kind(f.m_pSchemaType, &kind, &type_name, &inner);   // category switch from spike
    //         emit_field(ctx, info->m_pszName, f.m_pszName, f.m_nSingleInheritanceOffset, kind, type_name, inner);
    //     }
    return 1;
}
```
Wire `schema_enumerate` into the `S2EngineOps` the shim passes to `s2script_core_init` (set the new field). Add a `schema_type_to_kind` helper (the category→kind switch from the spike). Keep the exact SDK iteration/offsets from Task 1's findings; any raw member offset gets a named `// TODO(gamedata)` constant.

- [ ] **Step 7: Run the core tests + full suite + gates**

Run: `cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`
Expected: green (the two new `schema_dump` tests + all prior; gates pass — no CS2 ids, names are streamed data). The shim's `schema_enumerate` is exercised live in Task 4 (it needs a real server).

- [ ] **Step 8: Commit**

```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp core/src/v8host.rs
git commit -m "feat(slice5b1): schema_enumerate op (shim SDK walk) + __s2_schema_dump native

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: Dev dump plugin + LIVE dump + committed catalog + README/CLAUDE (LIVE-ONLY)

**Files:**
- Create: `examples/schema-dump/{package.json, src/plugin.ts}`; commit `games/cs2/gamedata/schema-catalog.json`.
- Modify: `README.md`, `CLAUDE.md`.

**Interfaces:**
- Consumes: Task 3's `__s2_schema_dump` native + the shim `schema_enumerate`.
- Produces: the committed catalog + the treadmill runbook.

- [ ] **Step 1: Create the dev dump plugin `examples/schema-dump`.**

`package.json`:
```json
{
  "name": "@demo/schema-dump",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x", "pluginDependencies": { "@s2script/std": "^1.0.0" } }
}
```
`src/plugin.ts` — dump once the schema is ready (a few frames into a live map):
```ts
import { OnGameFrame } from "@s2script/std";

// __s2_schema_dump is a dev/treadmill native (not part of the typed @s2script/std surface).
declare const __s2_schema_dump: (path: string) => boolean;

let done = false;
let ticks = 0;
export function onLoad(): void {
  console.log("[schema-dump] onLoad — will dump once the schema is live");
  OnGameFrame.subscribe(() => {
    if (done) return;
    if (ticks++ < 128) return;                 // let a map load + the schema populate
    const ok = __s2_schema_dump("/tmp/schema-catalog.json");
    console.log("[schema-dump] dump " + (ok ? "OK -> /tmp/schema-catalog.json" : "not ready, retrying"));
    if (ok) done = true;
  });
}
export function onUnload(): void { console.log("[schema-dump] onUnload"); }
```

- [ ] **Step 2: Build the plugin `.s2sp` + the sniper runtime.**

```bash
cd /home/gkh/projects/s2script
node packages/cli/build.mjs
npx s2script build examples/schema-dump
bash scripts/build-sniper.sh   # fresh s2script.so + shim (carries schema_enumerate); must post-date Task 3
```
If a CS2 update reset `gameinfo.gi` (addon loads 0 plugins), run `bash docker/patch-gameinfo.sh` and restart.

- [ ] **Step 3: Run the LIVE dump on Docker CS2.** Bring up the server, drop the dump plugin, get the map ticking (`bot_quota 1`, `sv_hibernate_when_empty 0`; wait past the boot window so the schema is populated) via `scripts/rcon.py` + container logs:
  1. `[schema-dump] onLoad`, then `[schema-dump] dump OK -> /tmp/schema-catalog.json` (retries logged until the schema is ready).
  2. Copy the file out of the container to `games/cs2/gamedata/schema-catalog.json` (e.g. `docker cp`).
  If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done (plugin, `.s2sp` + sniper built, README/CLAUDE drafted) and report BLOCKED with the exact commands/errors so the controller can drive the dump.

- [ ] **Step 4: Spot-check the committed catalog.**

```bash
cd /home/gkh/projects/s2script
python3 -c "import json;d=json.load(open('games/cs2/gamedata/schema-catalog.json'));\
print('classes',len(d));\
p=d['CCSPlayerPawn'];print('parent',p['parent']);\
h=[f for f in p['fields'] if f['name']=='m_iHealth'][0];print('health',h['type'],h['offset']);\
print('has_handle',any(f['type']['kind']=='handle' for c in d.values() for f in c['fields']));\
print('has_class',any(f['type']['kind']=='class' for c in d.values() for f in c['fields']))"
```
Expected: many classes; `CCSPlayerPawn` present with a `parent`; `m_iHealth` = `{kind:atomic,name:int32}` at the offset `__s2_schema_offset("CCSPlayerPawn","m_iHealth")` also returns (cross-check against the Slice-3 resolve if handy); at least one `handle` field and one `class` field present.

- [ ] **Step 5: README + CLAUDE.md.** Add a `## Schema catalog dump (Slice 5B.1 — treadmill)` section to `README.md`: the runbook (build → run server on a map → drop the dump plugin → `docker cp` the catalog → commit) + the spot-check + a note that this is regenerated after each CS2 update. Update `CLAUDE.md` "## Current state": 5B.1 done (the schema catalog dump — `games/cs2/gamedata/schema-catalog.json`, regenerable); "Current focus: Slice 5B.2 next" (typed field access). Do NOT alter the standing conventions above it.

- [ ] **Step 6: Final verification + commit** (commit the catalog JSON — it's the regenerable gamedata artifact; do NOT commit `.s2sp`/`dist`/`.so`):

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/schema-dump games/cs2/gamedata/schema-catalog.json README.md CLAUDE.md
git commit -m "feat(slice5b1): dev dump plugin + committed schema-catalog.json (live dump); README/CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §9)

1. `cargo test -p s2script-core` green (existing + the `schema_catalog` unit tests + the two `schema_dump` core tests via the stub); both boundary gates green; sniper build OK.
2. `s2script build` produces a loadable `schema-dump` `.s2sp`.
3. The live dump writes a valid `schema-catalog.json` passing the spot-checks (many classes; `CCSPlayerPawn` + `parent`; `m_iHealth`=int32 at the resolved offset; ≥1 handle + ≥1 class field).
4. README documents the dump/treadmill runbook; CLAUDE.md "Current state" updated (5B.1 done, focus → 5B.2).
