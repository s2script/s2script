# Slice 3 — Schema-Backed Typed Accessor (`pawn.health`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve `CCSPlayerPawn::m_iHealth` live from the in-process SchemaSystem and expose `pawn.health` get/set (state-change folded into the setter) in `@s2script/cs2` (JS) over engine-generic core natives — proving the core/game boundary at the first game-specific accessor.

**Architecture:** Core (Rust, engine-generic) gains a small set of Source-2 natives — schema-offset resolution, entity-by-index / handle-deref, raw i32 field read/write, `NetworkStateChanged`, and a raw ConCommand. `@s2script/cs2` (a JS file under `games/cs2/`, eval'd at boot) holds the CS2 names + the `slot→controller→pawn` walk + the typed `Pawn` wrapper. Two baked demos (auto readback gate targeting a bot pawn + a manual `s2_sethp` ConCommand HUD path) verify it on a live CS2 server.

**Tech Stack:** Rust (`s2script-core` cdylib, `v8` 149.4.0), C++ MM:S shim, hl2sdk `cs2` branch (SchemaSystem/entity2/tier1 headers), JavaScript (`@s2script/cs2`), Docker (`joedwards32/cs2`) + sniper build.

## Global Constraints

- **Core contains ZERO CS2 identifiers.** No `CCSPlayer*`, `m_iHealth`, `m_hPlayerPawn`, `cs2`/game names in `core/`. Litmus: "would this be true on a different Source 2 game?" Enforced by the boundary gate (Task 2).
- **The `m_iHealth` offset is resolved LIVE from SchemaSystem** at runtime — never hardcoded, never in gamedata. Only interface/signature *strings* live in `gamedata/`.
- **Degrade per-descriptor, never crash globally.** A missing class/field/signature disables *that* accessor with a named `WARN` and returns a safe sentinel; the framework keeps running. No Rust panic crosses the FFI boundary (`catch_unwind` on every native, as in Slices 1–2).
- **Raw pointers are block-scoped, never stored across `await`.** `ExternalPointer` (a `v8::External`) is valid only for the current call chain (the durable `EntityRef` is Slice 5).
- **No `HOST` borrow held across a JS invocation** (the Slice 1–2 re-entrancy discipline); natives take their scope from the V8 callback argument.
- **The C++ shim + C ABI MAY change this slice** (unlike Slice 2): to acquire SchemaSystem (currently deferred) and pass its pointer + the resolved cs2-JS path to core.
- **`cargo test -p s2script-core -- --test-threads=1`** is the test command (the V8 platform is process-global). All Slice 0/1/2 tests stay green.
- **Sniper build** (`scripts/build-sniper.sh` in `rust:bullseye`) produces the loadable server binaries (GLIBC ≤ 2.31).

---

### Task 1: Engine-RE reconnaissance — pin the §11 unknowns (findings doc; no production code)

**Purpose:** Resolve the exact hl2sdk `cs2` APIs the Task 3–6 natives will call, so the engine glue is not built on guesses. Output is a committed findings doc + a throwaway compile probe proving the named symbols exist and the headers include cleanly. This is a **reconnaissance task**, not TDD — its "test" is (a) the probe compiles, (b) the doc answers every question below with a concrete symbol/signature.

**Files:**
- Create: `docs/superpowers/specs/2026-06-30-slice-3-recon-findings.md`
- Create (throwaway, deleted at end of task): `/tmp/s2recon/probe.cpp`
- Read: `third_party/hl2sdk/public/schemasystem/schemasystem.h`, `third_party/hl2sdk/public/entity2/{entityinstance.h,entitysystem.h,entityidentity.h,concreteentitylist.h}`, `third_party/hl2sdk/public/tier1/convar.h`, `third_party/hl2sdk/public/networkvar.h`, `shim/src/s2script_mm.cpp` (interface acquisition), `third_party/metamod-source/core/metamod.cpp` (module/factory access).

**Interfaces:**
- Produces: the findings doc that Tasks 3–6 cite for exact call sequences. Each finding names: the type(s), the header, the method/function signature, and whether it is **header-confirmed** or **needs live confirmation** (deferred to the Task 7 live gate).

- [ ] **Step 1: Answer each reconnaissance question in the findings doc.** For every item, quote the relevant header line(s) with `file:line` and give the concrete call:
  1. **SchemaSystem offset resolve.** How to get the type scope for the server module (`ISchemaSystem::FindTypeScopeForModule("server.so"/"libserver.so")` → `CSchemaSystemTypeScope*`), resolve a class (`FindDeclaredClass("CCSPlayerPawn")` → `SchemaClassInfoData_t*`/`CSchemaClassInfo*`), and read a field's offset (iterate `GetFields()`/`m_pFields`, match `m_pszName == "m_iHealth"`, read `m_nSingleInheritanceOffset`/`m_nOffset`). Name the exact getter for the offset.
  2. **SchemaSystem acquisition.** Which module/factory yields `ISchemaSystem` — the interface string is `SchemaSystem_001` (already in gamedata). Is it reached via `ismm->GetServerFactory`, an engine factory, or a dedicated `schemasystem` module lookup (`ismm->...`/`g_SMAPI`/`GetInterfaceFactory` on the schemasystem module)? Give the shim code shape.
  3. **Entity system + entity-by-index.** How to obtain `CGameEntitySystem*` (an exported accessor `GameEntitySystem()`, a global, or a single **signature** to scan). Then `CEntityInstance*` by index (`CGameEntitySystem`/`CEntityIdentity` list, or `CConcreteEntityList`). State whether a gamedata **signature** is required; if so, specify its shape (name + module).
  4. **Handle deref.** Given a `CEntityHandle`/`CBaseHandle` u32 (as read from a schema handle field), resolve to `CEntityInstance*` (via the entity system's `GetEntityInstance`/`GetBaseEntity`), returning null when stale.
  5. **`slot → controller → pawn`.** How a client slot maps to its `CBasePlayerController`/controller entity (entity-index convention `slot+1`, or a player-manager lookup), and the field to read for the pawn handle (`m_hPlayerPawn`, resolved via schema).
  6. **`NetworkStateChanged`.** The exact call to mark `m_iHealth` dirty for clients — `CEntityInstance::NetworkStateChanged(offset, ...)` or the `CNetworkVarBase`/chain call (`networkvar.h`, `entityinstance.h`). Give the argument list.
  7. **ConCommand.** How to register a raw ConCommand in Source 2 (`ConCommand`/`ConCommandRefAbstract` in `tier1/convar.h`, `CommandCallback_t`/`FnCommandCallback_t`), and how the callback surfaces the **calling client slot** (`CCommandContext`/`CPlayerSlot`) + args (`CCommand`).

- [ ] **Step 2: Write the throwaway compile probe.** A `.cpp` that `#include`s each header above and references the key type of each finding (declare a pointer of each type, take the address of each method where possible) — proving the symbols exist and headers include together. Compile it inside the sniper container against the SDK include paths (mirror `shim/CMakeLists.txt` include dirs):
  ```
  docker run --rm -v "$(pwd):/repo" -w /repo rust:bullseye bash -lc \
    'apt-get update >/dev/null && apt-get install -y g++ >/dev/null; \
     g++ -std=c++17 -fsyntax-only -I third_party/hl2sdk/public -I third_party/hl2sdk/public/tier1 \
       -I shim/src/sdk_stubs /tmp/s2recon/probe.cpp && echo PROBE_OK'
  ```
  Expected: `PROBE_OK` (add whatever additional `-I`/stub the compiler demands, and record those include paths in the findings doc for Tasks 3–5 to reuse).

- [ ] **Step 3: Mark live-confirmation risk.** In the doc, list which findings are header-confirmed vs need live confirmation (e.g. the entity-system accessor/signature, the exact offset value, the controller index convention). These are the debugging targets for the Task 7 live gate.

- [ ] **Step 4: Commit** (delete the throwaway probe first):
  ```bash
  rm -rf /tmp/s2recon
  git add docs/superpowers/specs/2026-06-30-slice-3-recon-findings.md
  git commit -m "docs(slice3): engine-RE reconnaissance — SchemaSystem/entity/NetworkStateChanged/ConCommand call sequences"
  ```

---

### Task 2: CS2-name-leak boundary gate (guards every later core change)

**Purpose:** Fail the build if a CS2 identifier leaks into `core/`. Land this *before* the native tasks so the gate guards them.

**Files:**
- Modify: `scripts/check-core-boundary.sh` (append a name-leak grep after the existing crate-dependency check)
- Test: `scripts/test-boundary-nameleak.sh` (Create) — a self-contained test that plants a CS2 name in a temp copy and asserts the gate fails

**Interfaces:**
- Consumes: the existing `check-core-boundary.sh` (crate-dependency check, prints `core boundary OK`).
- Produces: an extended gate that also greps `core/` for CS2 identifiers; exit 1 + a named message on a hit.

- [ ] **Step 1: Write the failing test** — `scripts/test-boundary-nameleak.sh`:
```bash
#!/usr/bin/env bash
# Verifies the name-leak gate FAILS when a CS2 identifier is present in core/ and PASSES when clean.
set -uo pipefail
cd "$(dirname "$0")/.."

# 1. Clean tree must pass.
if ! bash scripts/check-core-boundary.sh >/dev/null 2>&1; then
  echo "FAIL: gate rejected a clean core/"; exit 1
fi

# 2. Plant a CS2 name in a temp core file; gate must fail.
tmp="core/src/__nameleak_probe.rs"
echo '// CCSPlayerPawn m_iHealth' > "$tmp"
if bash scripts/check-core-boundary.sh >/dev/null 2>&1; then
  rm -f "$tmp"; echo "FAIL: gate did not catch a CS2 name in core/"; exit 1
fi
rm -f "$tmp"
echo "PASS: name-leak gate catches CS2 identifiers and passes when clean"
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash scripts/test-boundary-nameleak.sh`
Expected: FAIL — the gate has no name-leak check yet, so it still passes with the probe present (the test prints "gate did not catch a CS2 name").

- [ ] **Step 3: Add the name-leak grep** to the end of `scripts/check-core-boundary.sh`, before its final success echo. Match on identifiers that are unambiguously game-specific (do not match generic words):
```bash
# --- CS2 name-leak gate: core/ must contain no CS2 identifier (engine-generic only). ---
# Patterns are CS2 schema/game identifiers that must live only in games/cs2 (JS) or gamedata.
NAME_LEAK_RE='CCSPlayer|CCSPlayerPawn|CCSPlayerController|m_iHealth|m_hPlayerPawn|\bCS2\b|counterstrike'
if grep -rInE "$NAME_LEAK_RE" core/src 2>/dev/null; then
  echo "BOUNDARY VIOLATION: CS2 identifier found in core/ (must live in games/cs2 or gamedata)" >&2
  exit 1
fi
```
(Note: the existing final `echo "core boundary OK: ..."` line stays last.)

- [ ] **Step 4: Run to verify it passes**

Run: `bash scripts/test-boundary-nameleak.sh`
Expected: PASS — "name-leak gate catches CS2 identifiers and passes when clean".
Also run: `bash scripts/check-core-boundary.sh` → still prints `core boundary OK: ...` on the clean tree.

- [ ] **Step 5: Commit**
```bash
git add scripts/check-core-boundary.sh scripts/test-boundary-nameleak.sh
git commit -m "test(boundary): fail the build if a CS2 identifier leaks into core/"
```

---

### Task 3: SchemaSystem acquisition + `__s2_schema_offset` native (offset resolution, cached)

**Purpose:** Wire SchemaSystem (currently deferred) through the shim → C ABI → core, and expose a cached `__s2_schema_offset(class, field) → i32` native. The pure **cache** logic is unit-tested; the live SchemaSystem query is engine-dependent (verified in Task 7).

**Files:**
- Create: `core/src/schema.rs` — the schema-offset resolver + cache (V8-free core logic)
- Modify: `core/src/lib.rs` (add `mod schema;`), `core/src/v8host.rs` (install `__s2_schema_offset`; store the SchemaSystem pointer), `core/src/ffi.rs` (extend `s2script_core_init` to receive the SchemaSystem pointer), `shim/include/s2script_core.h` (C ABI), `shim/src/s2script_mm.cpp` (acquire SchemaSystem per Task-1 finding #2, pass the pointer)
- Test: `core/src/schema.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1 findings #1 (offset resolve) and #2 (acquisition); the existing native-install region in `v8host.rs` (~line 375, `global_obj.set(scope, key, fn)`) and the `native_callback` HRTB wrapper (~v8host.rs:146).
- Produces:
  - `schema::OffsetCache` with `fn resolve(&mut self, class: &str, field: &str, raw: impl Fn(&str,&str)->i32, log: impl Fn(&str)) -> i32` — caches by `(class, field)`, calls `raw` at most once per key, logs a `WARN` once on a `-1` miss.
  - JS global `__s2_schema_offset(class: string, field: string) -> number` (i32; `-1` on miss).
  - C ABI: `s2script_core_init(logger, request_hook, schema_system_ptr)` (extends the Slice-2 signature with a `void*` SchemaSystem pointer; may be null → schema natives degrade).

- [ ] **Step 1: Write the failing test** (`core/src/schema.rs` `#[cfg(test)]`) — the cache is pure, no engine:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn resolves_once_and_caches_hits() {
        let mut cache = OffsetCache::new();
        let calls = Cell::new(0);
        let raw = |_c: &str, _f: &str| { calls.set(calls.get() + 1); 320 };
        let noop = |_s: &str| {};
        assert_eq!(cache.resolve("CCSPlayerPawn", "m_iHealth", &raw, &noop), 320);
        assert_eq!(cache.resolve("CCSPlayerPawn", "m_iHealth", &raw, &noop), 320);
        assert_eq!(calls.get(), 1, "second lookup must hit the cache, not re-query");
    }

    #[test]
    fn caches_and_warns_once_on_miss() {
        let mut cache = OffsetCache::new();
        let raw = |_c: &str, _f: &str| -1;
        let warns = Cell::new(0);
        let log = |_s: &str| warns.set(warns.get() + 1);
        assert_eq!(cache.resolve("X", "y", &raw, &log), -1);
        assert_eq!(cache.resolve("X", "y", &raw, &log), -1);
        assert_eq!(warns.get(), 1, "a missing field must WARN at most once (cached miss)");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core schema:: -- --test-threads=1`
Expected: FAIL — `OffsetCache` undefined.

- [ ] **Step 3: Implement the cache** (`core/src/schema.rs`):
```rust
//! Engine-generic SchemaSystem offset resolution + cache (V8-free logic).
//! The live SchemaSystem query lives in v8host (needs the raw pointer); this module
//! owns the cache + miss-once-WARN policy so it is unit-testable without an engine.
use std::collections::HashMap;

pub struct OffsetCache {
    map: HashMap<(String, String), i32>,
}

impl OffsetCache {
    pub fn new() -> Self { OffsetCache { map: HashMap::new() } }

    /// Resolve `(class, field)` to a byte offset, caching the result (including a `-1` miss).
    /// `raw` performs the live SchemaSystem lookup (returns `-1` if not found); `log` receives a
    /// one-time WARN message on a miss. `raw`/`log` are called at most once per distinct key.
    pub fn resolve(
        &mut self,
        class: &str,
        field: &str,
        raw: impl Fn(&str, &str) -> i32,
        log: impl Fn(&str),
    ) -> i32 {
        let key = (class.to_string(), field.to_string());
        if let Some(&off) = self.map.get(&key) {
            return off;
        }
        let off = raw(class, field);
        if off < 0 {
            log(&format!(
                "WARN: schema offset not found for {class}::{field}; accessor disabled"
            ));
        }
        self.map.insert(key, off);
        off
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core schema:: -- --test-threads=1`
Expected: PASS (both cache tests).

- [ ] **Step 5: Wire acquisition + the native (engine-dependent — no unit test; verified live in Task 7).**
  - `shim/include/s2script_core.h`: change the init signature to `int s2script_core_init(s2_log_fn logger, s2_hook_request_fn request_hook, void* schema_system);`.
  - `shim/src/s2script_mm.cpp`: acquire `ISchemaSystem*` per Task-1 finding #2 (interface string `SchemaSystem_001` from gamedata; replace the deferred-note at ~line 131), and pass it as the third arg to `s2script_core_init`. Keep the degrade-never-crash posture: if acquisition fails, log a WARN and pass `nullptr`.
  - `core/src/ffi.rs`: extend `s2script_core_init` to accept `schema_system: *mut c_void`; stash it in a `thread_local`/`OnceLock` in `v8host` (e.g. `SCHEMA_SYSTEM: Cell<*mut c_void>`), before the logger guard (mirror how `set_hook_request` is called early in Slice 2).
  - `core/src/v8host.rs`: add a module-level `OffsetCache` (thread-local `RefCell`), and a `native_schema_offset(scope, args, rv)` that: reads `class`/`field` string args; calls `SCHEMA_OFFSETS.with(|c| c.borrow_mut().resolve(class, field, live_raw, live_log))` where `live_raw` calls into a `schema` helper that walks the SchemaSystem pointer per Task-1 finding #1 (returns `-1` if the pointer is null or the class/field isn't found) and `live_log` routes to `LOGGER`; sets `rv` to the i32. Install it on the global exactly like the existing `__s2_delay` native (same `native_callback`/scope/`catch_unwind` wrapper). **No CS2 names appear here** — `class`/`field` are opaque string args from JS.

- [ ] **Step 6: Verify build + boundary + full suite**

Run: `cargo build -p s2script-core && cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh`
Expected: builds; all prior tests + the two new schema tests pass; `core boundary OK` (no CS2 name leaked).

- [ ] **Step 7: Commit**
```bash
git add core/src/schema.rs core/src/lib.rs core/src/v8host.rs core/src/ffi.rs shim/include/s2script_core.h shim/src/s2script_mm.cpp
git commit -m "feat(core): SchemaSystem acquisition + __s2_schema_offset native (live offset resolve, cached)"
```

---

### Task 4: Entity access + memory read/write + state-change natives

**Purpose:** Expose the generic entity/memory natives the cs2 accessor needs. The raw i32 read/write is unit-tested against a fake struct; entity-by-index, handle-deref, and `NetworkStateChanged` are engine-dependent (verified in Task 7).

**Files:**
- Create: `core/src/entity.rs` — the raw read/write helpers + null/bounds guards (V8-free)
- Modify: `core/src/lib.rs` (`mod entity;`), `core/src/v8host.rs` (install `__s2_entity_by_index`, `__s2_deref_handle`, `__s2_ent_read_i32`, `__s2_ent_write_i32`, `__s2_ent_state_changed`)
- Test: `core/src/entity.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1 findings #3 (entity-by-index), #4 (handle deref), #6 (NetworkStateChanged); the `ExternalPointer` (`v8::External`) convention; the native-install region.
- Produces:
  - `entity::read_i32(base: *const u8, offset: i32) -> i32` and `entity::write_i32(base: *mut u8, offset: i32, value: i32)` — null/negative-offset guarded (read returns 0, write is a no-op on a bad arg).
  - JS globals `__s2_entity_by_index(i: number) -> External|null`, `__s2_deref_handle(h: number) -> External|null`, `__s2_ent_read_i32(ent: External, off: number) -> number`, `__s2_ent_write_i32(ent: External, off: number, v: number)`, `__s2_ent_state_changed(ent: External, off: number)`.

- [ ] **Step 1: Write the failing test** (`core/src/entity.rs` `#[cfg(test)]`) — pure, against a fake struct:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct Fake { pad: [u8; 8], health: i32, more: i32 }

    #[test]
    fn write_then_read_roundtrips_at_offset() {
        let mut f = Fake { pad: [0; 8], health: 100, more: 7 };
        let base = &mut f as *mut Fake as *mut u8;
        let off = 8; // offset of `health`
        assert_eq!(read_i32(base as *const u8, off), 100);
        write_i32(base, off, 1234);
        assert_eq!(read_i32(base as *const u8, off), 1234);
        assert_eq!(f.more, 7, "adjacent field untouched");
    }

    #[test]
    fn guards_null_and_negative_offset() {
        assert_eq!(read_i32(std::ptr::null(), 8), 0);
        assert_eq!(read_i32(std::ptr::null(), -4), 0);
        // write to null / negative offset must not crash and must be a no-op:
        write_i32(std::ptr::null_mut(), 8, 1);
        let mut v: i32 = 5;
        write_i32(&mut v as *mut i32 as *mut u8, -4, 9);
        assert_eq!(v, 5);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core entity:: -- --test-threads=1`
Expected: FAIL — `read_i32`/`write_i32` undefined.

- [ ] **Step 3: Implement the guarded read/write** (`core/src/entity.rs`):
```rust
//! Engine-generic raw entity-field access (V8-free helpers + guards). The entity-SYSTEM lookups
//! (by index / handle) and NetworkStateChanged live in v8host (they need engine pointers); this
//! module owns the pointer-arithmetic read/write so it is unit-testable without an engine.

/// Read an i32 at `base + offset`. Returns 0 on a null base or negative offset (degrade-safe).
pub fn read_i32(base: *const u8, offset: i32) -> i32 {
    if base.is_null() || offset < 0 {
        return 0;
    }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *const i32) }
}

/// Write an i32 at `base + offset`. No-op on a null base or negative offset (degrade-safe).
pub fn write_i32(base: *mut u8, offset: i32, value: i32) {
    if base.is_null() || offset < 0 {
        return;
    }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *mut i32) = value; }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core entity:: -- --test-threads=1`
Expected: PASS (both round-trip + guard tests).

- [ ] **Step 5: Install the natives (engine-dependent parts — verified live in Task 7).** In `v8host.rs`, add five `native_*` callbacks installed exactly like `__s2_delay`:
  - `__s2_ent_read_i32(ent, off)`: unwrap the `External` to `*const u8`, call `entity::read_i32`, return the i32. (Fully exercised by the pure test above via the helper.)
  - `__s2_ent_write_i32(ent, off, v)`: unwrap to `*mut u8`, call `entity::write_i32`.
  - `__s2_entity_by_index(i)`: call the entity-system per Task-1 finding #3 → `*const CEntityInstance` (opaque `*mut c_void`) → wrap in `v8::External`, or return `null`. Store the entity-system pointer the same way as SchemaSystem (Task 3) if the shim provides it, else resolve per finding #3.
  - `__s2_deref_handle(h)`: per Task-1 finding #4 → `External|null`.
  - `__s2_ent_state_changed(ent, off)`: per Task-1 finding #6, invoke `NetworkStateChanged`. No return.
  - All wrapped in `catch_unwind`; **no CS2 names** (indices/offsets/handles are opaque numbers).

- [ ] **Step 6: Verify build + boundary + full suite**

Run: `cargo build -p s2script-core && cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh`
Expected: builds; all prior + the two new entity tests pass; `core boundary OK`.

- [ ] **Step 7: Commit**
```bash
git add core/src/entity.rs core/src/lib.rs core/src/v8host.rs
git commit -m "feat(core): entity-by-index/handle-deref + guarded i32 field read/write + NetworkStateChanged natives"
```

---

### Task 5: `__s2_concommand` native (raw ConCommand + calling-slot callback)

**Purpose:** Register a raw Source 2 ConCommand whose callback hands JS the calling client slot + args. Minimal — not the Slice-5 command framework. Engine-dependent; verified live in Task 7, with a bridge-level unit test that the registration + JS-fn storage path is sound.

**Files:**
- Modify: `core/src/v8host.rs` (install `__s2_concommand`; store the JS callback `Global<Function>` keyed by command name; a C-side `CommandCallback` that resolves the slot+args and invokes the stored JS fn under the isolate)
- Test: `core/src/v8host.rs` `#[cfg(test)]` (a bridge test that a registered command's JS callback is stored + invocable with a simulated `(slot, args)`)

**Interfaces:**
- Consumes: Task 1 finding #7 (ConCommand registration + `CCommandContext`/`CCommand`); the `CONCOMMANDS` callback map convention (mirror `RESOLVERS` from Slice 2 for storing `Global<Function>`).
- Produces: JS global `__s2_concommand(name: string, fn: (slot: number, argString: string) => void)`; an internal `dispatch_concommand(name, slot, args)` (crate-visible) that the C callback calls, invoking the stored JS fn under a HandleScope with `(slot, args)`.

- [ ] **Step 1: Write the failing test** (`core/src/v8host.rs` `#[cfg(test)]`) — exercises the store + dispatch path without the engine (call `dispatch_concommand` directly):
```rust
    #[test]
    fn concommand_callback_receives_slot_and_args() {
        init(dummy_logger()).unwrap();
        eval(r#"
            globalThis.__cc = null;
            __s2_concommand("s2_test", function (slot, args) { globalThis.__cc = slot + ":" + args; });
        "#).unwrap();
        // Simulate the engine invoking the command (bypasses ConCommand registration):
        dispatch_concommand("s2_test", 3, "1234");
        assert_eq!(read_string_global("__cc"), "3:1234");
        shutdown();
    }
```
> Provide `read_string_global(name)` (mirror `read_bool_global`, returning the JS string) if not already present.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core concommand_callback_receives_slot_and_args -- --test-threads=1`
Expected: FAIL — `__s2_concommand` / `dispatch_concommand` undefined.

- [ ] **Step 3: Implement the store + dispatch.** In `v8host.rs`:
  - Add `CONCOMMANDS: RefCell<HashMap<String, Global<Function>>>` (thread-local).
  - `native_concommand(scope, args, _rv)`: read `name` (string) + `fn` (Function) args; store `Global::new(scope, fn)` in `CONCOMMANDS[name]`; then register the raw engine ConCommand per Task-1 finding #7 with a C trampoline that calls `dispatch_concommand(name, slot, args)` (engine-dependent; skipped/guarded when no engine, so the unit test path works via direct `dispatch_concommand`).
  - `pub(crate) fn dispatch_concommand(name: &str, slot: i32, args: &str)`: look up the `Global<Function>` (borrow `CONCOMMANDS`, clone the Global, **drop the borrow** — re-entrancy discipline), then under a `HOST` HandleScope+ContextScope call the fn with `[Number(slot), String(args)]`. Must not hold the `CONCOMMANDS` borrow across the JS call.
  - Install `__s2_concommand` like the other natives; `catch_unwind`; no CS2 names.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core concommand_callback_receives_slot_and_args -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Verify build + boundary + full suite**

Run: `cargo build -p s2script-core && cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh`
Expected: builds; all tests pass; `core boundary OK`.

- [ ] **Step 6: Commit**
```bash
git add core/src/v8host.rs
git commit -m "feat(core): __s2_concommand native — raw ConCommand with calling-slot + args to JS"
```

---

### Task 6: `@s2script/cs2` JS package + the cs2-file load path

**Purpose:** Add the first `@s2script/cs2` code (JS): the CS2 names, the `slot→controller→pawn` walk, and the `Pawn` accessor — loaded from disk + eval'd at boot. This is where the CS2 identifiers live; the boundary gate (Task 2) proves they stay out of core.

**Files:**
- Create: `games/cs2/js/pawn.js` — the cs2 accessor (CS2 names + walk + `Pawn` + `pawnForSlot`)
- Modify: `core/src/v8host.rs` (a `load_cs2_file(path)` that reads + evals a JS file), `core/src/ffi.rs` + `shim/include/s2script_core.h` (`s2script_core_load_cs2(path)` C ABI), `shim/src/s2script_mm.cpp` (resolve the cs2 JS path via `dladdr` like `GamedataPath`, call `s2script_core_load_cs2`)
- Test: `core/src/v8host.rs` `#[cfg(test)]` (loading a small JS file evaluates it in the shared context)

**Interfaces:**
- Consumes: `__s2_schema_offset`, `__s2_entity_by_index`, `__s2_deref_handle`, `__s2_ent_read_i32/write_i32`, `__s2_ent_state_changed` (Tasks 3–4); the `eval`/`load` scope construction.
- Produces: `s2script_core_load_cs2(const char* path)` (C ABI); `games/cs2/js/pawn.js` defining `globalThis.cs2 = { Pawn, pawnForSlot }`.

- [ ] **Step 1: Write the failing test** (`core/src/v8host.rs` `#[cfg(test)]`) — a file load evaluates in the shared context:
```rust
    #[test]
    fn load_cs2_file_evaluates_in_context() {
        init(dummy_logger()).unwrap();
        let dir = std::env::temp_dir().join("s2_cs2_load_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("probe.js");
        std::fs::write(&f, "globalThis.__loaded = 41 + 1;").unwrap();
        load_cs2_file(f.to_str().unwrap());
        assert_eq!(read_i32_global("__loaded"), 42);
        shutdown();
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core load_cs2_file_evaluates_in_context -- --test-threads=1`
Expected: FAIL — `load_cs2_file` undefined.

- [ ] **Step 3: Implement `load_cs2_file`** in `v8host.rs`: read the file to a String (on read error, log a named WARN and return — degrade-never-crash), then evaluate it in the HOST context exactly as `eval` does (reuse the `eval` scope construction). Add the C ABI `s2script_core_load_cs2(path: *const c_char)` in `ffi.rs` + `s2script_core.h`, forwarding to `load_cs2_file`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core load_cs2_file_evaluates_in_context -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Write `games/cs2/js/pawn.js`** (the CS2 accessor — this file is allowed to contain CS2 names; core is not):
```js
// @s2script/cs2 — provisional pawn.health accessor (Slice 3). CS2 names live here, never in core.
// Loaded at boot via s2script_core_load_cs2 (real plugin loading is Slice 4).
(function () {
  var HEALTH = __s2_schema_offset("CCSPlayerPawn", "m_iHealth");
  var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");

  function Pawn(ent) { this.ent = ent; }
  Pawn.prototype = {
    get health() { return __s2_ent_read_i32(this.ent, HEALTH); },
    set health(v) {
      __s2_ent_write_i32(this.ent, HEALTH, v);
      __s2_ent_state_changed(this.ent, HEALTH); // fold the network state-change into the setter
    },
  };

  // slot -> controller entity -> m_hPlayerPawn handle -> pawn CEntityInstance.
  // CS2 convention: player controllers occupy entity indices slot+1 (confirmed in the live gate).
  function pawnForSlot(slot) {
    if (HEALTH < 0 || PAWN_HANDLE < 0) return null;
    var controller = __s2_entity_by_index(slot + 1);
    if (!controller) return null;
    var handle = __s2_ent_read_i32(controller, PAWN_HANDLE);
    var pawnEnt = __s2_deref_handle(handle);
    return pawnEnt ? new Pawn(pawnEnt) : null;
  }

  globalThis.cs2 = { Pawn: Pawn, pawnForSlot: pawnForSlot, HEALTH_OFFSET: HEALTH };
})();
```

- [ ] **Step 6: Wire the shim to load it.** In `shim/src/s2script_mm.cpp`, add a `Cs2JsPath()` (mirror `GamedataPath` via `dladdr`, resolving `addons/s2script/gamedata/../js/pawn.js` or the packaged location — align with `scripts/package-addon.sh`/`build-sniper.sh` so `pawn.js` ships in `dist/`). After `s2script_core_init`, call `s2script_core_load_cs2(Cs2JsPath())`. Update `scripts/build-sniper.sh`/packaging to copy `games/cs2/js/pawn.js` into `dist/addons/s2script/js/pawn.js`.

- [ ] **Step 7: Verify build + boundary + full suite**

Run: `cargo build -p s2script-core && cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`
Expected: builds; all tests pass; `core boundary OK` (the CS2 names are in `games/cs2/js/`, not `core/`); name-leak gate green.

- [ ] **Step 8: Commit**
```bash
git add games/cs2/js/pawn.js core/src/v8host.rs core/src/ffi.rs shim/include/s2script_core.h shim/src/s2script_mm.cpp scripts/build-sniper.sh
git commit -m "feat(cs2): @s2script/cs2 pawn.health accessor (JS) + core cs2-file load path"
```

---

### Task 7: Live gate (auto readback + manual HUD) + README

**Purpose:** Prove the accessor on a real CS2 server — the auto readback gate (bot pawn) in logs, and the manual `s2_sethp` HUD path. Controller-driven (Claude drives the container), like Slice 2 Task 6.

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (extend the baked demo: the auto readback + the `s2_sethp` ConCommand registration via cs2), `README.md`
- (cs2-side demo logic may live in `games/cs2/js/pawn.js` or a sibling demo file)

**Interfaces:**
- Consumes: everything from Tasks 3–6.
- Produces: an operator-run demonstration + the README Slice-3 runbook & acceptance table.

- [ ] **Step 1: Add the two demos.** In the baked boot JS (via `pawn.js` or the shim eval), after `cs2` is defined:
  - **Auto readback** — arm after a live-frame threshold (Slice-2 pattern; the server barely ticks during boot). Once armed, scan slots `0..64` for the first non-null `cs2.pawnForSlot(slot)` (a bot); log the resolved `cs2.HEALTH_OFFSET`, the pawn's `health` (get), set `health = 1234`, then read back and log: `"[cs2] slot=N health get=H set=1234 readback=R"`.
  - **Manual HUD** — register `__s2_concommand("s2_sethp", function(slot, args){ var p = cs2.pawnForSlot(slot); if (p) p.health = parseInt(args) || 100; })`.

- [ ] **Step 2: Sniper build + recreate the container**
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
docker compose -f docker/docker-compose.yml up -d --force-recreate cs2
```
Wait for server-up (poll `docker logs` for `GC Connection established`). Then add a bot so a pawn exists:
```bash
python3 scripts/rcon.py "bot_add" ; python3 scripts/rcon.py "bot_add_ct"
```

- [ ] **Step 3: Observe the auto gate + record acceptance**
```bash
docker logs s2script-cs2 2>&1 | grep -E "\[cs2\]|\[s2script\]|\[async\]|\[demo\]" | tail -30
```
**Acceptance to record (spec §8):**
- `[cs2] slot=N health get=H set=1234 readback=1234` — the offset resolved live, get/set/state-change ran, readback confirms the write; no crash.
- (Regression) the Slice-1 `[demo] HIGH`/`low` composition and the Slice-2 `[async]` lines still appear; `interface OK` including SchemaSystem.

- [ ] **Step 4: Manual HUD confirmation (operator — you).** Document the steps and record the result: connect a CS2 client to the LAN server (`connect <host>:27015`), open console, run `s2_sethp 1234`, and confirm the HUD health box shows `1234`. (This proves the setter's `NetworkStateChanged` networks to the client — the part the headless auto gate can't show.)

- [ ] **Step 5: Update the README.** Add a "Schema-backed typed accessor (Slice 3)" section (mirror the Slice 1/2 sections): what it proves (boundary + live offset), the core natives vs the cs2 JS, the auto-gate log excerpt, the manual HUD steps, and a Slice-3 acceptance table covering spec §8 with the live evidence. Note the accessor is provisional (the codegen-backed `@s2script/cs2` API is Slice 5).

- [ ] **Step 6: Commit + stop the container (keep the copy)**
```bash
git add shim/src/s2script_mm.cpp games/cs2/js/pawn.js README.md
git commit -m "docs+demo: Slice 3 live gate (pawn.health readback + s2_sethp HUD) + acceptance"
docker stop s2script-cs2 && docker rm s2script-cs2
```

---

## Self-Review (completed during planning)

- **Spec coverage:** §1 thesis → all tasks. §2.1 JS accessor + core natives → T3–T6. §2.2 live offset → T3. §2.3 two demos → T7. §2.4 name-leak gate → T2. §3 core natives (schema/entity/memory/state-change/concommand) → T3/T4/T5. §4 cs2 package + load path → T6. §5 demos → T7. §6 boundary → T2 + T6. §7 testing (unit: schema cache T3, memory helpers T4; integration: bridge T3–T6; live T7) → covered. §8 acceptance → T3–T7. §9 out-of-scope honored (no codegen, no EntityRef wrapper, no command framework, no loader, one field/i32 only). §10 files → matches. §11 open items → T1 recon resolves them, cited by T3–T6. No spec section unmapped.
- **Placeholder scan:** engine-dependent steps (live SchemaSystem/entity/NetworkStateChanged/ConCommand calls) are deliberately delegated to the T1 recon findings + verified in the T7 live gate — not vague "add error handling." The unit-testable logic (schema cache, memory read/write, concommand dispatch, cs2-file load) has complete code. No "TBD".
- **Type consistency:** `OffsetCache::resolve(class, field, raw, log) -> i32`; `entity::{read_i32(*const u8,i32)->i32, write_i32(*mut u8,i32,i32)}`; natives `__s2_schema_offset`/`__s2_entity_by_index`/`__s2_deref_handle`/`__s2_ent_read_i32`/`__s2_ent_write_i32`/`__s2_ent_state_changed`/`__s2_concommand`; C ABI `s2script_core_init(logger, request_hook, schema_system)` + `s2script_core_load_cs2(path)`; `dispatch_concommand(name, slot, args)`; cs2 `Pawn`/`pawnForSlot(slot)`/`HEALTH_OFFSET`. Consistent across tasks. The `-1` miss sentinel is uniform (schema offset). `External`-wrapped pointers uniform for entities.

---

## Architecture amendment (post-Task-1 recon)

Task 1 confirmed every engine operation (SchemaSystem virtuals, entity-system access,
`NetworkStateChanged` vtable call, `ConCommand` construction) is a **C++** call. Therefore the
engine glue for Tasks 3–5 lives in the **C++ shim** as C-ABI helper functions, passed to core as
**function-pointer callbacks** (the existing `logger`/`request_hook` pattern) so the dependency
direction stays shim→core. Core's Rust V8 natives call through the stored pointers; a null pointer
degrades that native with a named WARN. Pure pointer read/write (`ent_read_i32`/`write_i32`) and the
`OffsetCache` + concommand-dispatch logic stay inline Rust (unit-tested with fakes). The shim gains
`shim/src/sdk_stubs/entitydatainstantiator.h` (one-line forward-declare) to compile
`entity2/entitysystem.h`, and reaches `CGameEntitySystem*` via an `IGameResourceService`-anchored
offset in `gamedata/` (`GameResourceServiceServerV001`; offset value dumped live in Task 7). Helper
set: `s2_schema_offset(cls,field)->int`, `s2_ent_by_index(idx)->void*`, `s2_deref_handle(u32)->void*`,
`s2_ent_state_changed(ent,off)`, `s2_concommand_register(name)`; core exposes
`s2script_core_dispatch_concommand(name,slot,args)` for the ConCommand trampoline.
