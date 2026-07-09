# Entity-Creation Lifecycle Primitive + Beam Drawing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. (This project executes plans via the `Workflow` tool — one agent per task, adversarial review between tasks — which is the subagent-driven pattern.)

**Goal:** Add an engine-generic entity-creation lifecycle primitive (`createEntity`/`spawn`/`teleport`/`remove`) and a CS2 `Beam` helper on top of it, proven by a hold-E laser-sight demo that draws a visible `env_beam` following the player's aim.

**Architecture:** Four new `S2EngineOps` ops (ABI-appended after `trace_shape`) back the primitive; the shim resolves `UTIL_CreateEntityByName`/`CBaseEntity::DispatchSpawn`/`UTIL_Remove` by self-validated byte signature and `CBaseEntity::Teleport` by a `.text`-validated vtable index. `createEntity` returns a serial-gated `EntityRef` (the raw `CBaseEntity*` is converted to a `CEntityHandle` shim-side and never crosses to JS). The CS2 `Beam` helper (in `pawn.js`) composes `createEntity` + existing raw schema writes (`writeUInt8`/`writeFloat32`/`writeUInt32` + `notifyStateChanged`) + `teleport`/`spawn`.

**Tech Stack:** Rust (core, rusty_v8), C++ (shim, hl2sdk), JavaScript (prelude + game package + plugin), TypeScript (`.d.ts`).

## Global Constraints

- **Boundary (both CI gates must stay green):** core/`@s2script/*` never names a CS2 schema class or field. The 4 ops are `className`-parameterized or take `(index, serial)` — no game names. All `env_beam`/`CBeam`/`CBaseModelEntity` field names live ONLY in `games/cs2/js/pawn.js` + `packages/cs2`. Run `bash scripts/check-core-boundary.sh` and `bash scripts/test-boundary-nameleak.sh`.
- **ABI discipline:** new ops are APPENDED after the current last op (`trace_shape`) — never reordered — and kept identical across the C header (`shim/include/s2script_core.h`), the Rust mirror (`core/src/v8host.rs`), BOTH in-isolate test op-structs, and the shim `ops.` assignment.
- **No raw pointer crosses to JS:** `createEntity` returns a `CEntityHandle.ToInt()` int; core decodes it and builds a serial-gated `EntityRef` (the `DamageInfo.victim` / `s2_trace` pattern via `build_entity_ref`).
- **Degrade-never-crash:** every native is `catch_unwind`-wrapped; an unresolved op or stale serial returns `null`/`false`/no-op. Every shim function is `s_p*`-null-guarded.
- **RE doctrine:** signatures are borrowed from CSSharp gamedata as HINTS, re-scanned + validated UNIQUE in our pinned `libserver.so`; the Teleport vtable index is validated in-`.text` before the first call (never trusted blind — the CommitSuicide-index lesson).
- **Tests run serial:** `.cargo/config.toml` sets `RUST_TEST_THREADS=1`; run core tests with `cargo test` from `core/`.
- **Commits:** end every commit message with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. NEVER put backticks in `git commit -m`; use `git commit -F -` with a heredoc.
- **Offline-validated signature seeds (already scanned UNIQUE @ 2026-07-09 in `docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so`, all `resolve:"direct"`):**
  - `UTIL_CreateEntityByName` `48 8D 05 ? ? ? ? 55 48 89 FA` (@0x16a9140; ABI rdi=className, esi=forceEdictIndex → CBaseEntity*)
  - `CBaseEntity_DispatchSpawn` `48 85 FF 74 ? 55 48 89 E5 41 55 41 54 49 89 FC` (@0x1785b00; ABI rdi=this, rsi=CEntityKeyValues*)
  - `UTIL_Remove` `48 89 FE 48 85 FF 74 ? 48 8D 05 ? ? ? ? 48` (@0x16a9460; ABI rdi=entity)
  - `CBaseEntity::Teleport` — vtable **index 162 (linux)** (CSSharp `CBaseEntity_Teleport` offset); ABI (rdi=this, rsi=Vector* origin, rdx=QAngle* angles, rcx=Vector* velocity), all nullable.

---

## File Structure

- `shim/include/s2script_core.h` — 4 op typedefs + 4 `S2EngineOps` fields (after `trace_shape`).
- `core/src/v8host.rs` — 4 `*Fn` types + 4 `ENGINE_OPS` fields; 4 natives (`s2_entity_create/spawn/teleport/remove`); their `set_native` registration; the `@s2script/entity` prelude additions (`createEntity` + `EntityRef.prototype.spawn/teleport/remove`); both test op-structs; degrade tests.
- `packages/entity/index.d.ts` — `createEntity` + the 3 `EntityRef` methods.
- `gamedata/core.gamedata.jsonc` — 3 signatures + 1 offset (Teleport vtable index).
- `shim/src/s2script_mm.cpp` (+ its signature-load site) — resolve the 3 sigs + Teleport index; implement + wire the 4 op functions.
- `games/cs2/js/pawn.js` — the `Beam` helper + a `pawn.buttons` accessor (CS2 schema names live here).
- `packages/cs2/index.d.ts` — `Beam`/`BeamHandle` types + `pawn.buttons`.
- `plugins/beam-demo/{package.json,tsconfig.json,src/plugin.ts}` — the hold-E laser demo + an rcon `sm_beam` bot-provable check.

---

## Task 1: Core ops + `@s2script/entity` primitive + degrade tests

**Files:**
- Modify: `shim/include/s2script_core.h` (after line 199, `trace_shape`)
- Modify: `core/src/v8host.rs` (ENGINE_OPS struct ~228; natives near `s2_trace` ~3780; `set_native` block ~4511; `@s2script/entity` module ~880; test op-structs; tests)
- Modify: `packages/entity/index.d.ts`

**Interfaces:**
- Produces (C ops): `s2_entity_create_fn = int(const char*)` → packed `CEntityHandle` (0 = fail); `s2_entity_spawn_fn = int(int index,int serial)`; `s2_entity_teleport_fn = int(int index,int serial,const float* origin,const float* angles,const float* velocity)` (nullable ptrs); `s2_entity_remove_fn = int(int index,int serial)`.
- Produces (JS): `__s2pkg_entity.createEntity(className: string): EntityRef | null`; `EntityRef.prototype.spawn(): boolean`; `EntityRef.prototype.teleport(origin: number[3], angles?: number[3]|null, velocity?: number[3]|null): boolean`; `EntityRef.prototype.remove(): boolean`.
- Consumes: `build_entity_ref`, `entity_resolve_ptr`, `crate::entity::decode_handle`, `ENGINE_OPS`, `set_native` (all existing).

- [ ] **Step 1: Add the C op typedefs + struct fields**

In `shim/include/s2script_core.h`, immediately after `s2_trace_shape_fn trace_shape;` (line 199) and before `} S2EngineOps;`:

```c
    /* Entity-creation lifecycle slice — APPENDED after trace_shape; order is the ABI. */
    /* create: className -> packed CEntityHandle (ToInt), 0 = failure. The raw CBaseEntity* is
       converted shim-side and never crosses to JS. */
    typedef_placeholder /* see below: typedefs go with the other s2_*_fn typedefs, near line 131 */
```

Put the typedefs with the other `s2_*_fn` typedefs (near line 131, after `s2_trace_shape_fn`):

```c
typedef int (*s2_entity_create_fn)(const char* className);
typedef int (*s2_entity_spawn_fn)(int index, int serial);
typedef int (*s2_entity_teleport_fn)(int index, int serial, const float* origin, const float* angles, const float* velocity);
typedef int (*s2_entity_remove_fn)(int index, int serial);
```

And the struct fields (after `trace_shape;`):

```c
    /* Entity-creation lifecycle slice — APPENDED after trace_shape; order is the ABI. */
    s2_entity_create_fn   entity_create;
    s2_entity_spawn_fn    entity_spawn;
    s2_entity_teleport_fn entity_teleport;
    s2_entity_remove_fn   entity_remove;
```

- [ ] **Step 2: Add the Rust op mirror**

In `core/src/v8host.rs`, define the fn types near the other `*Fn` type aliases, then add the fields after `pub trace_shape: Option<TraceShapeFn>,` (line 228):

```rust
type EntityCreateFn   = extern "C" fn(*const std::os::raw::c_char) -> c_int;
type EntitySpawnFn    = extern "C" fn(c_int, c_int) -> c_int;
type EntityTeleportFn = extern "C" fn(c_int, c_int, *const f32, *const f32, *const f32) -> c_int;
type EntityRemoveFn   = extern "C" fn(c_int, c_int) -> c_int;
```

```rust
    // --- Entity-creation lifecycle slice (APPENDED after trace_shape; order is the ABI; do not reorder above) ---
    pub entity_create:   Option<EntityCreateFn>,
    pub entity_spawn:    Option<EntitySpawnFn>,
    pub entity_teleport: Option<EntityTeleportFn>,
    pub entity_remove:   Option<EntityRemoveFn>,
```

Add `entity_create: None, entity_spawn: None, entity_teleport: None, entity_remove: None,` to BOTH in-isolate test op-structs (search for the two literals that already list `trace_shape: None,`).

- [ ] **Step 3: Write the failing degrade tests**

Add to the core test module (near the other `*_native_degrades_*` tests):

```rust
#[test]
fn entity_create_native_degrades_to_null_without_op() {
    // No engine ops installed -> createEntity returns null (never a crash).
    let out = eval_in_isolate(r#"
        const { createEntity } = __s2pkg_entity;
        String(createEntity("env_beam"))
    "#);
    assert_eq!(out, "null");
}

#[test]
fn entity_lifecycle_methods_degrade_to_false_without_op() {
    // spawn/teleport/remove on a synthetic ref return false with no ops.
    let out = eval_in_isolate(r#"
        const r = new (__s2pkg_entity.EntityRef)(1, 7);
        [r.spawn(), r.teleport([0,0,0]), r.teleport([0,0,0],null,null), r.remove()].join(",")
    "#);
    assert_eq!(out, "false,false,false,false");
}
```

(Use the existing in-isolate eval harness the other tests use — match its helper name, e.g. `eval_in_isolate` / the pattern in the neighbouring `#[test]`s.)

- [ ] **Step 4: Run the tests — verify they FAIL**

Run: `cd core && cargo test entity_ -- --nocapture`
Expected: FAIL — `createEntity`/`spawn`/`teleport`/`remove` are undefined.

- [ ] **Step 5: Implement the 4 natives**

Add near `s2_trace` (~line 3780). `s2_entity_create` mirrors the `s2_trace` op-call + `build_entity_ref` pattern:

```rust
/// Native `__s2_entity_create(className) -> EntityRef | null`. Over the `entity_create` op. The op
/// returns a packed CEntityHandle (ToInt); the raw CBaseEntity* never crosses to JS. 0 / a stale
/// handle -> null. Degrades to null with no op (every in-isolate test).
fn s2_entity_create(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let name = args.get(0).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_create) {
            let handle = func(cname.as_ptr());
            if handle != 0 {
                let (index, serial) = crate::entity::decode_handle(handle as u32);
                if !entity_resolve_ptr(index, serial).is_null() {
                    rv.set(build_entity_ref(scope, index, serial));
                }
            }
        }
    }));
}

/// Native `__s2_entity_spawn(index, serial) -> boolean`. Serial-gated DispatchSpawn.
fn s2_entity_spawn(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_spawn) { rv.set_bool(func(index, serial) != 0); }
    }));
}

/// Native `__s2_entity_teleport(index, serial, originArr|null, anglesArr|null, velArr|null) -> boolean`.
fn s2_entity_teleport(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let origin = read_vec3_opt(scope, args.get(2));   // Option<[f32;3]>, None if not a 3-array
        let angles = read_vec3_opt(scope, args.get(3));
        let vel    = read_vec3_opt(scope, args.get(4));
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_teleport) {
            let op = origin.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            let ap = angles.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            let vp = vel.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            rv.set_bool(func(index, serial, op, ap, vp) != 0);
        }
    }));
}

/// Native `__s2_entity_remove(index, serial) -> boolean`. Serial-gated UTIL_Remove.
fn s2_entity_remove(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_remove) { rv.set_bool(func(index, serial) != 0); }
    }));
}
```

Add the `read_vec3_opt` helper next to `read_vec3` (returns `None` unless the arg is a 3-element array of numbers):

```rust
/// Like `read_vec3` but returns `None` when the arg isn't a 3-number array (for nullable teleport args).
fn read_vec3_opt(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<[f32; 3]> {
    let arr = v8::Local::<v8::Array>::try_from(v).ok()?;
    if arr.length() != 3 { return None; }
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        out[i as usize] = arr.get_index(scope, i)?.number_value(scope).unwrap_or(0.0) as f32;
    }
    Some(out)
}
```

- [ ] **Step 6: Register the natives**

In the `set_native` block (near line 4511, beside `__s2_trace`):

```rust
    set_native(scope, global_obj, "__s2_entity_create", s2_entity_create);
    set_native(scope, global_obj, "__s2_entity_spawn", s2_entity_spawn);
    set_native(scope, global_obj, "__s2_entity_teleport", s2_entity_teleport);
    set_native(scope, global_obj, "__s2_entity_remove", s2_entity_remove);
```

- [ ] **Step 7: Add the `@s2script/entity` prelude surface**

In the prelude JS, add methods on `EntityRef.prototype` (beside the existing `write*` methods) and a `createEntity` function; extend the module object at line 880.

```js
  EntityRef.prototype.spawn = function () { return __s2_entity_spawn(this.index, this.serial); };
  EntityRef.prototype.teleport = function (origin, angles, velocity) {
    return __s2_entity_teleport(this.index, this.serial,
      origin ? [origin[0], origin[1], origin[2]] : null,
      angles ? [angles[0], angles[1], angles[2]] : null,
      velocity ? [velocity[0], velocity[1], velocity[2]] : null);
  };
  EntityRef.prototype.remove = function () { return __s2_entity_remove(this.index, this.serial); };
  function createEntity(className) { return __s2_entity_create(String(className)); }
```

Change line 880 to:

```js
  globalThis.__s2pkg_entity     = { EntityRef: EntityRef, createEntity: createEntity };
```

(`teleport` takes plain `[x,y,z]` arrays — the CS2 `Beam` caller converts its `Vector` to an array, keeping `@s2script/entity` free of the `math` dependency.)

- [ ] **Step 8: Add the `.d.ts` surface**

In `packages/entity/index.d.ts`, add to the `EntityRef` interface and a module-level export:

```ts
  /** DispatchSpawn this created entity (register/activate it). Returns false if stale/unresolved. */
  spawn(): boolean;
  /** Teleport this entity. origin/angles/velocity are [x,y,z] triples; any may be null. False if stale. */
  teleport(origin: number[] | null, angles?: number[] | null, velocity?: number[] | null): boolean;
  /** Remove (UTIL_Remove) this entity from the world. Returns false if stale/unresolved. */
  remove(): boolean;
```

```ts
/** Create a new entity by class name (e.g. "env_beam"). Returns a serial-gated EntityRef, or null on
 *  failure. Call `.spawn()` after setting fields to register it. The created entity is game-world-owned
 *  (NOT auto-removed on plugin unload) — the plugin owns cleanup via `.remove()`. */
export declare function createEntity(className: string): EntityRef | null;
```

- [ ] **Step 9: Run the tests — verify they PASS**

Run: `cd core && cargo test 2>&1 | tail -3`
Expected: `test result: ok. <N> passed; 0 failed` (the 2 new tests pass; all prior tests still pass).

- [ ] **Step 10: Boundary + commit**

Run: `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` → both pass (no CS2 names added to core).

```bash
git add shim/include/s2script_core.h core/src/v8host.rs packages/entity/index.d.ts
git commit -F - <<'EOF'
feat(entity): createEntity/spawn/teleport/remove ops + @s2script/entity surface

4 S2EngineOps ops (ABI-appended after trace_shape) + serial-gated natives; createEntity
returns an EntityRef (raw CBaseEntity* converted to a CEntityHandle shim-side, never crosses
to JS). Degrade-to-null/false with no op; 2 in-isolate degrade tests. Shim impl in Task 2.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 2: Shim — gamedata signatures + the 4 op implementations

**Files:**
- Modify: `gamedata/core.gamedata.jsonc` (`.signatures` + `.offsets`)
- Modify: `shim/src/s2script_mm.cpp` (signature-load site + op impls + `ops.` assignment)

**Interfaces:**
- Consumes: the C op typedefs from Task 1; the existing serial-gated entity resolver (`s2script_mm.cpp:~234`, `(index,serial) -> CBaseEntity*` via `CEntityIdentity::GetRefEHandle`); the sig-loader (`LoadSignatures`/`FindPattern` in `sigscan.cpp`); the `.text`-range check pattern from `vtable.cpp` (the trace slice).
- Produces: `ops.entity_create/entity_spawn/entity_teleport/entity_remove` populated.

- [ ] **Step 1: Add the gamedata entries**

In `gamedata/core.gamedata.jsonc`, add to `"signatures"` (all `resolve:"direct"`, validated UNIQUE @ 2026-07-09):

```jsonc
    "UtilCreateEntityByName": {
      "linuxsteamrt64": { "module": "libserver.so",
        "pattern": "48 8D 05 ? ? ? ? 55 48 89 FA", "resolve": "direct" }
    },
    "DispatchSpawn": {
      "linuxsteamrt64": { "module": "libserver.so",
        "pattern": "48 85 FF 74 ? 55 48 89 E5 41 55 41 54 49 89 FC", "resolve": "direct" }
    },
    "UtilRemove": {
      "linuxsteamrt64": { "module": "libserver.so",
        "pattern": "48 89 FE 48 85 FF 74 ? 48 8D 05 ? ? ? ? 48", "resolve": "direct" }
    }
```

And to `"offsets"` (a vtable INDEX on the entity's own vtable — validated in-`.text` before the first call, like `CNavPhysicsInterface_TraceShape`):

```jsonc
    "CBaseEntity_Teleport": { "linuxsteamrt64": 162 }
```

- [ ] **Step 2: Resolve the signatures at load**

At the shim's signature-load site (where `GameEventManager`/`HostSay`/`DispatchTraceAttack` are resolved via `LoadSignatures`), resolve the 3 new sigs into typed function pointers and read the Teleport vtable index from the offsets block. Guard each (null if unresolved; log a one-line status, matching the existing `interface OK:` / gamedata lines):

```cpp
using CreateEntityByNameFn = CBaseEntity* (*)(const char* className, int forceEdictIndex);
using DispatchSpawnFn      = void (*)(CBaseEntity* self, void* pEntityKeyValues);
using UtilRemoveFn         = void (*)(CBaseEntity* self);
static CreateEntityByNameFn s_pCreateEntityByName = nullptr;
static DispatchSpawnFn      s_pDispatchSpawn      = nullptr;
static UtilRemoveFn         s_pUtilRemove         = nullptr;
static int                  s_teleportVtblIndex   = -1;   // from gamedata offsets; -1 = unresolved
```

Populate them from the resolved sig addresses / offsets block during init (reuse the exact `FindSignature`/gamedata-read calls the other sigs use).

- [ ] **Step 3: Implement the op functions**

```cpp
// create: className -> packed CEntityHandle (ToInt). The raw ptr NEVER leaves the shim.
static int Shim_EntityCreate(const char* className) {
    if (!s_pCreateEntityByName || !className) return 0;
    CBaseEntity* ent = s_pCreateEntityByName(className, -1);
    if (!ent) return 0;
    return ent->GetRefEHandle().ToInt();
}
static int Shim_EntitySpawn(int index, int serial) {
    if (!s_pDispatchSpawn) return 0;
    CBaseEntity* ent = ResolveEntityBySerial(index, serial);   // the existing serial-gated resolver
    if (!ent) return 0;
    s_pDispatchSpawn(ent, nullptr);
    return 1;
}
static int Shim_EntityTeleport(int index, int serial, const float* o, const float* a, const float* v) {
    if (s_teleportVtblIndex < 0) return 0;
    CBaseEntity* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    void** vtbl = *reinterpret_cast<void***>(ent);
    void* fn = vtbl[s_teleportVtblIndex];
    if (!IsAddressInServerText(fn)) return 0;   // .text-validate the borrowed index (trace-slice pattern)
    using TeleportFn = void (*)(void*, const Vector*, const QAngle*, const Vector*);
    reinterpret_cast<TeleportFn>(fn)(ent,
        reinterpret_cast<const Vector*>(o), reinterpret_cast<const QAngle*>(a), reinterpret_cast<const Vector*>(v));
    return 1;
}
static int Shim_EntityRemove(int index, int serial) {
    if (!s_pUtilRemove) return 0;
    CBaseEntity* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    s_pUtilRemove(ent);
    return 1;
}
```

Use the shim's existing serial-gated resolver (the helper at `s2script_mm.cpp:~234` / the one `trace` uses at `:269` for its ignore entity) as `ResolveEntityBySerial`. For `IsAddressInServerText`, reuse/extend the module-`.text`-range logic from `vtable.cpp` (the same check that validates `s_pTraceShape`).

- [ ] **Step 4: Wire the ops**

Where the shim fills the `S2EngineOps` struct passed to core (beside `ops.trace_shape = ...`):

```cpp
    ops.entity_create   = &Shim_EntityCreate;
    ops.entity_spawn    = &Shim_EntitySpawn;
    ops.entity_teleport = &Shim_EntityTeleport;
    ops.entity_remove   = &Shim_EntityRemove;
```

- [ ] **Step 5: Sniper build**

Run: `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh 2>&1 | tail -20`
Expected: both `libs2script_core.so` and `s2script.so` build with no errors.

- [ ] **Step 6: Commit** (live GAMEDATA VALIDATION is deferred to the Task 4 live gate — the shim has no in-isolate test)

```bash
git add gamedata/core.gamedata.jsonc shim/src/s2script_mm.cpp
git commit -F - <<'EOF'
feat(entity): shim — CreateEntityByName/DispatchSpawn/Teleport/UTIL_Remove

3 self-validated byte sigs (unique @ 2026-07-09 in the pinned libserver.so) + Teleport via
the entity vtable index 162 (linux), .text-validated before the first call. The 4 entity_*
ops back the Task-1 primitive; each s_p*-null / serial-gated / degrade-guarded.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 3: CS2 `Beam` helper + `pawn.buttons` (game layer)

**Files:**
- Modify: `games/cs2/js/pawn.js` (the CS2 IIFE — where `Pawn`/`Player`/`Events`/`Activity` live)
- Modify: `packages/cs2/index.d.ts`

**Interfaces:**
- Consumes: `__s2pkg_entity.createEntity`; `EntityRef.writeUInt8/writeFloat32/writeUInt32/notifyStateChanged` (existing); `EntityRef.teleport/spawn/remove` (Task 1); `__s2_schema_offset(cls, field)`; the movement-services button chain the menu poll already uses (`m_pMovementServices` → `CPlayer_MovementServices.m_nButtons` → `CInButtonState.m_pButtonStates`, `readUInt64Via`).
- Produces (JS): `__s2pkg_cs2.Beam.draw(start, end, opts?) -> BeamHandle|null`; `handle.update(start,end)`; `handle.remove()`; `pawn.buttons -> number` (low 32 bits of the pressed-button mask; `IN_USE = 32`).

- [ ] **Step 1: Add the `Beam` helper to `pawn.js`**

Inside the CS2 IIFE (where `__s2pkg_cs2` is assembled), add:

```js
  // --- Beam: a CEnvBeam point-to-point line. CS2 schema names live HERE (never in core). Composes the
  //     engine-generic createEntity/spawn/teleport/remove primitive + raw schema writes. ---
  var RENDERMODE_TRANSALPHA = 4;   // RenderMode_t::kRenderTransAlpha (verify at the live gate)
  function beamPackRGBA(c) {
    return ((c[0] & 255) | ((c[1] & 255) << 8) | ((c[2] & 255) << 16) | ((c[3] & 255) << 24)) >>> 0;
  }
  function beamWriteEnd(ref, end) {
    var o = __s2_schema_offset("CBeam", "m_vecEndPos");
    ref.writeFloat32(o, end.x); ref.writeFloat32(o + 4, end.y); ref.writeFloat32(o + 8, end.z);
    ref.notifyStateChanged(o);
  }
  var Beam = {
    draw: function (start, end, opts) {
      opts = opts || {};
      var ref = globalThis.__s2pkg_entity.createEntity("env_beam");
      if (!ref) return null;
      ref.writeUInt8(__s2_schema_offset("CBaseModelEntity", "m_nRenderMode"), RENDERMODE_TRANSALPHA);
      ref.writeFloat32(__s2_schema_offset("CBeam", "m_flWidth"), opts.width || 2.0);
      ref.writeUInt32(__s2_schema_offset("CBaseModelEntity", "m_clrRender"), beamPackRGBA(opts.color || [255, 0, 0, 255]));
      beamWriteEnd(ref, end);
      ref.teleport([start.x, start.y, start.z]);   // start = the entity's own origin
      ref.spawn();
      return {
        ref: ref,
        update: function (s, e) { ref.teleport([s.x, s.y, s.z]); beamWriteEnd(ref, e); },
        remove: function () { return ref.remove(); }
      };
    }
  };
```

Re-export it on `__s2pkg_cs2` beside `Events` (match the existing assignment style — e.g. `__s2pkg_cs2.Beam = Beam;`).

- [ ] **Step 2: Add the `pawn.buttons` accessor to `pawn.js`**

Resolve the same movement-services button chain the menu renderer uses; return the low 32 bits as a Number (so bitwise edge-detection works; `IN_USE = 32`):

```js
  Object.defineProperty(Pawn.prototype, "buttons", {
    get: function () {
      var msPtrOff = __s2_schema_offset("CCSPlayerPawnBase", "m_pMovementServices");
      var btnOff = __s2_schema_offset("CPlayer_MovementServices", "m_nButtons");
      var btnStateOff = __s2_schema_offset("CInButtonState", "m_pButtonStates");
      var v = this.ref.readUInt64Via([msPtrOff], btnOff + btnStateOff);   // index 0 of m_pButtonStates[3]
      return v === null ? 0 : Number(BigInt(v) & 0xFFFFFFFFn);
    },
    configurable: true
  });
```

(Confirm the exact class of `m_pMovementServices` against the menu poll in the same file — reuse its identical offset resolution so the two never drift.)

- [ ] **Step 3: Add the `.d.ts` types**

In `packages/cs2/index.d.ts`, add on the `Pawn` interface:

```ts
  /** The currently-pressed button mask (low 32 bits; IN_USE/E = 32). 0 if the mask is unreadable. */
  readonly buttons: number;
```

And near the other CS2 exports:

```ts
/** A live CEnvBeam handle. update() moves both endpoints; remove() destroys it. */
export interface BeamHandle {
  readonly ref: EntityRef;
  update(start: Vector, end: Vector): void;
  remove(): boolean;
}
/** Draw a point-to-point beam (a CEnvBeam) from start to end. Returns a handle, or null if the entity
 *  couldn't be created. The beam is game-world-owned — call handle.remove() to clean up. */
export declare const Beam: {
  draw(start: Vector, end: Vector, opts?: { color?: [number, number, number, number]; width?: number }): BeamHandle | null;
};
```

- [ ] **Step 4: Typecheck + assemble + commit**

Run: `bash scripts/check-plugins-typecheck.sh 2>&1 | tail -3` (all plugins still typecheck) and `bash scripts/test-boundary-nameleak.sh` (CS2 names stayed in the game layer).

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts
git commit -F - <<'EOF'
feat(cs2): Beam helper (env_beam) + pawn.buttons

Beam.draw/update/remove composes createEntity + raw CBeam/CBaseModelEntity schema writes
(width/color/rendermode/endpos, offsets live-resolved) + teleport(start) + spawn — CS2 schema
names stay in pawn.js. pawn.buttons exposes the pressed-button mask (IN_USE=32) for the demo.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 4: Hold-E laser demo + live gate

**Files:**
- Create: `plugins/beam-demo/package.json`, `plugins/beam-demo/tsconfig.json`, `plugins/beam-demo/src/plugin.ts`

**Interfaces:**
- Consumes: `@s2script/frame` (`OnGameFrame`), `@s2script/cs2` (`Pawn`, `Beam`), `@s2script/commands` (`Commands.register` for the bot-provable `sm_beam`), `@s2script/entity` (`createEntity` — for the rcon check).

- [ ] **Step 1: Scaffold the plugin**

`plugins/beam-demo/package.json`:

```json
{
  "name": "@demo/beam-demo",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x" }
}
```

`plugins/beam-demo/tsconfig.json` (copy a sibling plugin's — e.g. `plugins/trace-demo/tsconfig.json` — verbatim).

- [ ] **Step 2: Write the plugin**

`plugins/beam-demo/src/plugin.ts`:

```ts
import { OnGameFrame } from "@s2script/frame";
import { Pawn, Beam, BeamHandle } from "@s2script/cs2";
import { Commands } from "@s2script/commands";
import { createEntity } from "@s2script/entity";

const IN_USE = 32;                       // the E key bit
const state = new Map<number, { held: boolean; beam: BeamHandle | null }>();

function eyeOf(pawn: any): { x: number; y: number; z: number } | null {
  const sn = pawn.sceneNode;
  const o = sn && sn.absOrigin;
  return o ? { x: o.x, y: o.y, z: o.z + 64 } : null;   // standing eye height
}

function clearBeam(slot: number) {
  const s = state.get(slot);
  if (s && s.beam) { s.beam.remove(); s.beam = null; }
}

OnGameFrame(() => {
  for (let slot = 0; slot < 12; slot++) {
    const pawn = Pawn.forSlot(slot);
    if (!pawn) { clearBeam(slot); state.delete(slot); continue; }
    let s = state.get(slot);
    if (!s) { s = { held: false, beam: null }; state.set(slot, s); }
    const held = (pawn.buttons & IN_USE) !== 0;
    if (held) {
      const eye = eyeOf(pawn);
      const hit = pawn.aimTrace();
      if (eye && hit) {
        if (s.beam) s.beam.update(eye as any, hit.endPos as any);
        else s.beam = Beam.draw(eye as any, hit.endPos as any, { color: [255, 0, 0, 255], width: 2 });
      }
    } else if (s.beam) {
      clearBeam(slot);
    }
    s.held = held;
  }
});

// Bot-provable rcon check: create a static beam at fixed coords and report the EntityRef validity.
Commands.register("sm_beam", (ctx) => {
  const start = { x: 0, y: 0, z: 100 }, end = { x: 200, y: 0, z: 100 };
  const h = Beam.draw(start as any, end as any, { color: [0, 255, 0, 255], width: 3 });
  if (!h) { ctx.reply("[beam] createEntity FAILED"); return; }
  ctx.reply("[beam] drawn ref valid=" + h.ref.isValid() + " index=" + (h.ref as any).index);
  // leave it for 3s then remove (prove teleport/remove too)
  (globalThis as any).__s2pkg_timers.delay(3000).then(() => {
    ctx.reply("[beam] remove -> " + h.remove());
  });
});

export function onUnload() {
  for (const slot of state.keys()) clearBeam(slot);
  state.clear();
}
```

(`aimTrace`/`sceneNode`/`buttons` are `any`-cast where the `Vector` vs `{x,y,z}` shapes differ; the runtime shapes match. Verify against the actual `@s2script/cs2` types — tighten casts if the typecheck flags them.)

- [ ] **Step 3: Typecheck**

Run: `bash scripts/check-plugins-typecheck.sh 2>&1 | tail -3`
Expected: PASS (all plugins including `beam-demo`).

- [ ] **Step 4: Build the `.s2sp` + deploy**

Build the demo (`npx s2script build plugins/beam-demo`), assemble the addon (`bash scripts/package-addon.sh`), copy the sniper `.so`s + the demo `.s2sp` into `dist/addons/s2script/{bin,plugins}`, then `docker compose restart cs2` (NOT `--force-recreate`; re-run `docker exec s2script-cs2 /patch-gameinfo.sh` first if a game update intervened).

- [ ] **Step 5: Live gate — bot-provable mechanism**

With `bot_quota 2` on the running server:
- Confirm `=== GAMEDATA VALIDATION: N ok, 0 FAILED ===` (the 3 new sigs resolve — N grows by 3).
- `python3 scripts/rcon.py "sm_beam"` → expect `[beam] drawn ref valid=true index=<n>` then (after 3s) `[beam] remove -> true`. This proves `createEntity` returns a valid serial-gated `EntityRef`, and `spawn`/`teleport`/`remove` all succeed live.
- Confirm `RestartCount=0`, server keeps ticking, no crash.

- [ ] **Step 6: Live gate — human visual (deferred)**

Document as a deferred human-client live test (same ceiling as SayText2/menus/damage): a human joins, holds E, and SEES a red laser from their eye tracking their crosshair; release → gone. Record in memory `[[deferred-live-tests]]`. If the beam doesn't render, the `RENDERMODE_TRANSALPHA` value / a required sprite field is the tunable (a field add, not a redesign).

- [ ] **Step 7: Commit**

```bash
git add plugins/beam-demo
git commit -F - <<'EOF'
feat(demo): hold-E laser sight + sm_beam bot-provable check (beam-demo)

Polls pawn.buttons (IN_USE), aimTrace's the crosshair, and draws/updates/removes a red
env_beam eye->hit each frame; sm_beam (rcon) draws a static beam + reports EntityRef validity
for the bots-only gate. Live-proven mechanism; the human laser visual is a deferred live test.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Self-Review

**Spec coverage:** entity-lifecycle primitive (Task 1) ✓; 4 sig-scanned ops / shim (Task 2) ✓; CS2 `Beam` with live schema writes (Task 3) ✓; hold-E laser demo (Task 4) ✓; boundary held (Global Constraints + Tasks 1/3 gates) ✓; lifecycle policy = plugin-owned cleanup (Task 4 `onUnload` + `clearBeam`) ✓; testing split bot-provable vs human-visual (Task 4 Steps 5-6) ✓; the 4 risks (Teleport index → `.text`-validated Step 2/3 of Task 2; render fields → Task 4 Step 6; treadmill → gate-validated sigs; UAF → serial-gated throughout) ✓.

**Deviation from spec (noted):** `Beam` lives in `pawn.js`, not a new `beam.js` — matches the codebase pattern (all CS2 runtime is in `pawn.js`) and avoids concatenation-ordering fragility. Behaviourally identical.

**Type consistency:** `entity_create/spawn/teleport/remove` names match across the C typedef, Rust mirror, natives, and gamedata. `EntityRef.teleport(origin, angles?, velocity?)` takes `number[]` arrays consistently (Task 1 `.d.ts` ↔ Task 3 `Beam` caller passes `[x,y,z]`). `Beam.draw`/`BeamHandle.update`/`.remove` signatures match between `pawn.js` (Task 3) and `packages/cs2` (Task 3) and the demo (Task 4). `pawn.buttons: number` matches its use (`& IN_USE`).
