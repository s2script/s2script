# Entity lifecycle listeners Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let plugins observe the engine creating/spawning/deleting entities via `Entity.onCreate` / `onSpawn` / `onDelete(className, handler)`, delivering each entity to JS as a serial-gated `EntityRef`.

**Architecture:** The 4th notify-mux. A static `IEntityListener` on `CGameEntitySystem` (registered via a signature-scanned `AddListenerEntity`) → the shim's callback packs the entity handle + reads its class → a core `ENTITY_MUX` (keyed `"<kind>\0<className>"`, like `OUTPUT_MUX`) → JS subscribers, mirroring `dispatch_client_event` (notify-only, `try_borrow_mut` re-entrancy-guarded). Lazy install on the first subscribe (one new tail op) + a `StartupServer` per-map re-assert.

**Tech Stack:** Rust core (`core/src/{v8host,ffi}.rs`), C++ shim (`shim/src/{s2script_mm,entity_listener}.cpp`, `shim/include/s2script_core.h`), gamedata (`gamedata/core.gamedata.jsonc`), the injected core-prelude JS (a string in `v8host.rs`), `packages/entity/index.d.ts`, an `examples/entity-listeners-demo` plugin. Signatures borrowed as HINTS from ModSharp git118 (`sharp/gamedata/server.games.jsonc`), re-resolved + load-validated against our pinned `libserver.so`.

## Global Constraints

- **Boundary — both gates stay green.** `IEntityListener`/`CGameEntitySystem`/`CEntityInstance` are Source2 types → shim-only. Core (`ENTITY_MUX` + `dispatch_entity_event`) speaks only `(kind, className, handle)` strings/ints. **NO CS2 identifiers in `core/src`.** Gates: `bash scripts/check-core-boundary.sh` (no games/* crate dep) + `bash scripts/test-boundary-nameleak.sh` (no CS2 name leak).
- **RE doctrine ([[re-gamedata-strategy]]):** every engine fact self-resolved + load-validated. The two new signatures are HINTS from ModSharp — re-resolved + validated UNIQUE + `.text`-range-guarded by the existing `ResolveSigValidated` → the GAMEDATA VALIDATION gate at boot (loud on drift). Never a bare borrowed constant.
- **Serial gate:** the entity crosses only as a packed `CEntityHandle` (`GetRefEHandle().ToInt()`); core rebuilds a serial-gated `EntityRef` (`null` if the slot is stale). No raw pointer / cross-time ref ever crosses to JS.
- **Degrade-never-crash:** every native `catch_unwind`s; a missing/stale signature → the install op is a no-op (subscribe silently delivers nothing), never a crash; a null/garbage handle → `null` ref.
- **ABI-append discipline:** the one new op (`entity_listener_install`) is appended at the CURRENT struct tail (after `entity_set_model`, the zones-real-trigger slice's last op) in ALL FIVE places: the C typedef + struct member (`s2script_core.h`), the Rust type alias + struct field (`v8host.rs`), and BOTH Rust test op-structs (the `set_engine_ops(Some(S2EngineOps{…}))` at ~L9737 and `mock_event_ops()` at ~L10250). Order MUST match.
- **Sniper build (GLIBC ≤ 2.31):** `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`. Both `libs2script_core.so` (core changed) + `s2script.so` (shim changed) must rebuild.
- **Core tests run single-threaded** (`.cargo/config.toml` `RUST_TEST_THREADS=1`). Run: `cargo test -p s2script-core`.
- **Handler shape:** `handler(entity: EntityRef | null, className: string)`. `className` is passed explicitly (it already crosses for the mux key) because a `"*"` subscriber needs the class and the `entity` may be null (barely-constructed `onCreate` / dying `onDelete`).

## File Structure

- `shim/include/s2script_core.h` — +1 op typedef + struct member (tail); +1 `s2script_core_dispatch_entity_event` extern decl. *(Task 1 — the ABI contract.)*
- `core/src/v8host.rs` — `EntityListenerInstallFn` alias + struct field + both test op-structs; `ENTITY_MUX`; `dispatch_entity_event`; `s2_entity_listener_on`/`off` natives + `install_natives` registration; the prelude `Entity.onCreate/onSpawn/onDelete`; teardown + shutdown; core tests. *(Task 1.)*
- `core/src/ffi.rs` — `s2script_core_dispatch_entity_event` export. *(Task 1.)*
- `packages/entity/index.d.ts` — the three method signatures. *(Task 1.)*
- `gamedata/core.gamedata.jsonc` — +2 signatures (`AddListenerEntity`, `RemoveListenerEntity`). *(Task 2.)*
- `shim/src/entity_listener.cpp` (new, isolated SDK-including TU, mirrors `ekv.cpp`) — the `IEntityListener` subclass + `S2_GetEntityListener()`. *(Task 2.)*
- `shim/CMakeLists.txt` — add the new TU. *(Task 2.)*
- `shim/src/s2script_mm.cpp` — sig resolution, `EnsureEntityListenerRegistered()`, the `entity_listener_install` op + wiring, the `StartupServer` re-assert, `RemoveListenerEntity` on Unload. *(Task 2.)*
- `examples/entity-listeners-demo/{package.json,tsconfig.json,src/plugin.ts}` (new). *(Task 3.)*
- `.changeset/entity-lifecycle-listeners.md` (new) — minor `@s2script/entity`. *(Task 3.)*

---

## Task 1: Core notify-mux + ABI contract + prelude + types + tests

**Files:**
- Modify: `shim/include/s2script_core.h` (op typedef+member + dispatch extern decl)
- Modify: `core/src/v8host.rs` (alias/struct/test-structs, `ENTITY_MUX`, `dispatch_entity_event`, natives, prelude, teardown/shutdown, tests)
- Modify: `core/src/ffi.rs` (dispatch export)
- Modify: `packages/entity/index.d.ts` (types)

**Interfaces:**
- Produces (consumed by Task 2):
  - C op `int (*entity_listener_install)(void)` at the struct tail (shim fills it).
  - C extern `void s2script_core_dispatch_entity_event(const char* kind, const char* className, int handle);` (shim calls it).
- Produces (JS-facing): `Entity.onCreate/onSpawn/onDelete(className, handler)`; `handler(entity: EntityRef|null, className: string)`.

### Steps

- [ ] **Step 1: C ABI — op typedef + struct member + dispatch extern decl.**

In `shim/include/s2script_core.h`, add the typedef next to the other op typedefs (after `s2_entity_set_model_fn` at ~L218):

```c
/* Entity lifecycle listeners slice — APPENDED after entity_set_model; order is the ABI.
 * entity_listener_install: lazily register the IEntityListener on CGameEntitySystem on the
 * first-ever JS entity-lifecycle subscribe. Idempotent (AddListenerEntity guards Find) + re-asserted
 * each map by the StartupServer POST hook. Returns 1 if installed/queued, 0 if the AddListenerEntity
 * signature is unresolved (degrade — subscribe delivers nothing). */
typedef int (*s2_entity_listener_install_fn)(void);
```

Add the struct member at the tail (after `s2_entity_set_model_fn entity_set_model;`, ~L316):

```c
    /* Entity lifecycle listeners slice — APPENDED after entity_set_model; order is the ABI. */
    s2_entity_listener_install_fn entity_listener_install;
```

Add the dispatch extern decl next to the other `s2script_core_dispatch_*` decls (after `s2script_core_dispatch_map_start` at ~L348):

```c
/* Shim -> core: an IEntityListener callback (create/spawn/delete) reports an entity by its packed
 * CEntityHandle (ToInt()) + class name. Notify-only; core builds a serial-gated EntityRef. */
void s2script_core_dispatch_entity_event(const char* kind, const char* className, int handle);
```

- [ ] **Step 2: Rust ABI mirror — alias + struct field + both test op-structs.**

In `core/src/v8host.rs`, add the type alias next to `EntitySetModelFn` (~L210):

```rust
type EntityListenerInstallFn = extern "C" fn() -> c_int;
```

Add the struct field at the tail of `pub struct S2EngineOps` (after `pub entity_set_model: Option<EntitySetModelFn>,` ~L312):

```rust
    pub entity_listener_install: Option<EntityListenerInstallFn>,
```

Add `entity_listener_install: None,` after `entity_set_model: None,` in BOTH test op-structs: the `set_engine_ops(Some(S2EngineOps { … }))` block (~L9737, ends ~L9791) and `mock_event_ops()` (~L10250, ends ~L10309). (Verify by `grep -n "entity_set_model: None," core/src/v8host.rs` — add the line after each.)

- [ ] **Step 3: `ENTITY_MUX` static + teardown + shutdown.**

In `core/src/v8host.rs`, add the mux static right after `OUTPUT_MUX` (~L560, inside the same `thread_local!`):

```rust
    /// Entity lifecycle listeners slice: `Entity.onCreate/onSpawn/onDelete(className, handler)` mux,
    /// keyed `"<kind>\0<className>"` (kind = "create"/"spawn"/"delete"; className "*" = all). Notify-only,
    /// dispatched SYNCHRONOUSLY from the shim's IEntityListener callback (it fires from the engine's entity
    /// path, not under our own borrow; the try_borrow_mut guard covers a handler that synchronously
    /// creates/removes an entity). `remove_by_owner` on unload; reset on shutdown so a re-init starts empty.
    static ENTITY_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
```

Add the shutdown reset right after the `OUTPUT_MUX` reset (~L7876):

```rust
    // Reset the entity-lifecycle mux (entity-listeners slice) so a re-init starts clean.
    ENTITY_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
```

Add the teardown `remove_by_owner` right after the `OUTPUT_MUX` one (~L8214):

```rust
    // Drop the plugin's entity-lifecycle subscriptions (entity-listeners slice). The IEntityListener
    // stays registered for the process lifetime (removed in the shim's Unload), so no per-plugin
    // hook-removal is needed.
    ENTITY_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
```

- [ ] **Step 4: `dispatch_entity_event` in `v8host.rs`.**

Add next to `dispatch_client_event` / `dispatch_map_start` (anywhere among the dispatch fns, e.g. after `dispatch_map_start` ~L3715):

```rust
/// Deliver an entity lifecycle event to the `Entity.on{Create,Spawn,Delete}` subscribers. Called from
/// ffi.rs's `s2script_core_dispatch_entity_event` (the shim's IEntityListener callback). `kind` is
/// "create"/"spawn"/"delete"; the mux is keyed `"<kind>\0<className>"` with a `"<kind>\0*"` wildcard.
/// Notify-only. Mirrors `dispatch_client_event`: snapshot (release the mux borrow), `try_borrow_mut`
/// re-entrancy guard, per-sub `is_live` + context clone + HandleScope/ContextScope/TryCatch + WARN-on-throw.
/// The entity crosses as a packed handle → a serial-gated EntityRef (null if stale/free — the exact-(-1)
/// + resolve-null discipline of `dispatch_output`); className is passed as a 2nd arg (always valid).
pub(crate) fn dispatch_entity_event(kind: &str, class_name: &str, handle: i32) {
    // Phase 1: snapshot the exact-class key + the "<kind>\0*" wildcard (skip the wild when class == "*",
    // else the same key would be snapshotted twice).
    let exact = format!("{}\0{}", kind, class_name);
    let mut snap = ENTITY_MUX.with(|m| m.borrow().snapshot(&exact));
    if class_name != "*" {
        let wild = format!("{}\0*", kind);
        snap.extend(ENTITY_MUX.with(|m| m.borrow().snapshot(&wild)));
    }
    if snap.is_empty() { return; }

    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };
        for (owner, generation, handler_g) in &snap {
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let entity_val: v8::Local<v8::Value> = if handle == -1 {
                v8::null(tc).into()
            } else {
                let (idx, ser) = crate::entity::decode_handle(handle as u32);
                if entity_resolve_ptr(idx, ser).is_null() { v8::null(tc).into() } else { build_entity_ref(tc, idx, ser) }
            };
            let class_val: v8::Local<v8::Value> = match v8::String::new(tc, class_name) {
                Some(s) => s.into(),
                None => v8::undefined(tc).into(),
            };
            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            if func.call(tc, recv, &[entity_val, class_val]).is_none() {
                let msg = tc.exception().map(|e| e.to_rust_string_lossy(&*tc)).unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_entity_event('{}','{}'): handler '{}': {}", kind, class_name, owner, msg));
            }
        }
    });
}
```

- [ ] **Step 5: `ffi.rs` export.**

In `core/src/ffi.rs`, add after `s2script_core_dispatch_map_start` (~L115):

```rust
/// Shim → core: an IEntityListener callback (create/spawn/delete) with the entity's packed
/// CEntityHandle (ToInt()) + class name. Notify-only: dispatches to the `Entity.on{Create,Spawn,Delete}`
/// JS subscribers. `catch_unwind`-wrapped; null/invalid-UTF-8 degrade to a no-op.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_entity_event(kind: *const c_char, class_name: *const c_char, handle: c_int) {
    let _ = catch_unwind(|| {
        if kind.is_null() || class_name.is_null() { return; }
        let Ok(kind_str) = (unsafe { CStr::from_ptr(kind) }).to_str() else { return };
        let Ok(class_str) = (unsafe { CStr::from_ptr(class_name) }).to_str() else { return };
        v8host::dispatch_entity_event(kind_str, class_str, handle as i32);
    });
}
```

- [ ] **Step 6: The `s2_entity_listener_on`/`off` natives + registration.**

In `core/src/v8host.rs`, add after `s2_output_unsubscribe` (~L5209):

```rust
/// Native `__s2_entity_listener_on(kind, className, handler)`. Subscribes a JS fn to the entity
/// lifecycle mux (entity-listeners slice), keyed `"<kind>\0<className>"`. On the FIRST-EVER subscribe
/// (the mux was empty), calls the `entity_listener_install` engine op so the shim lazily registers its
/// IEntityListener (zero cost when no plugin subscribes). Degrade-never-crash: no op → the subscribe
/// still records, the engine just never delivers.
fn s2_entity_listener_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let kind = args.get(0).to_rust_string_lossy(scope);
        let class_name = args.get(1).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(2)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        let key = format!("{}\0{}", kind, class_name);
        let first_ever = ENTITY_MUX.with(|m| m.borrow().is_empty());
        ENTITY_MUX.with(|m| { m.borrow_mut().subscribe(&key, owner, generation, handler_g); });
        if first_ever {
            if let Some(func) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.entity_listener_install) {
                let _ = func();
            }
        }
    }));
}

/// Native `__s2_entity_listener_off(kind, className)`. Drops the CURRENT plugin's subs for the exact
/// `"<kind>\0<className>"` key (best-effort, mirrors `s2_output_unsubscribe`). The IEntityListener stays
/// installed (unload/reload cleanup runs via `remove_by_owner`); this is available as a primitive.
fn s2_entity_listener_off(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let kind = args.get(0).to_rust_string_lossy(scope);
        let class_name = args.get(1).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let key = format!("{}\0{}", kind, class_name);
        ENTITY_MUX.with(|m| { m.borrow_mut().remove_by_owner_on(&key, &owner); });
    }));
}
```

Register both in `install_natives`, after the `__s2_output_unsubscribe` line (~L6165):

```rust
    // Entity lifecycle listeners slice: Entity.onCreate/onSpawn/onDelete subscribe/unsubscribe (the
    // IEntityListener is lazily installed shim-side on the first subscribe via entity_listener_install).
    set_native(scope, global_obj, "__s2_entity_listener_on", s2_entity_listener_on);
    set_native(scope, global_obj, "__s2_entity_listener_off", s2_entity_listener_off);
```

- [ ] **Step 7: Prelude `Entity.onCreate/onSpawn/onDelete`.**

In `core/src/v8host.rs`, extend the prelude `Entity` object (the JS string at ~L884). Replace:

```js
  var Entity = {
    onOutput: function (classname, output, handler) { __s2_output_subscribe(String(classname), String(output), handler); },
    // Find every entity whose designer-name (class) exactly matches className. Returns serial-gated
    // EntityRefs (empty array on no-op/degrade). Broadly reusable (gamerules proxy, props, triggers...).
    findByClass: function (className) {
      return __s2_entity_find_by_class(String(className));
    },
  };
```

with:

```js
  var Entity = {
    onOutput: function (classname, output, handler) { __s2_output_subscribe(String(classname), String(output), handler); },
    // Entity lifecycle listeners: fire when the engine creates/spawns/deletes an entity of `className`
    // ("*" = all). The handler gets (entity, className): `entity` is a serial-gated EntityRef (may be
    // null for a barely-constructed onCreate / a dying onDelete); `className` is always valid.
    onCreate: function (className, handler) { __s2_entity_listener_on("create", String(className), handler); },
    onSpawn:  function (className, handler) { __s2_entity_listener_on("spawn",  String(className), handler); },
    onDelete: function (className, handler) { __s2_entity_listener_on("delete", String(className), handler); },
    // Find every entity whose designer-name (class) exactly matches className. Returns serial-gated
    // EntityRefs (empty array on no-op/degrade). Broadly reusable (gamerules proxy, props, triggers...).
    findByClass: function (className) {
      return __s2_entity_find_by_class(String(className));
    },
  };
```

- [ ] **Step 8: Types in `packages/entity/index.d.ts`.**

Replace the `export declare const Entity: { … }` block (~L146) with:

```ts
export declare const Entity: {
  onOutput(classname: string, output: string, handler: (ev: OutputEvent) => HookResultValue | void): void;
  /**
   * Fire when the engine CREATES an entity of `className` (`"*"` = all) — earliest hook; the entity is
   * barely constructed, schema fields may be zero/default. The handler receives the serial-gated
   * `entity` (may be `null`) plus its `className`. Prefer `onSpawn` to read fields.
   */
  onCreate(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /**
   * Fire after the engine SPAWNS an entity of `className` (`"*"` = all) — `Spawn()` has run, so schema
   * fields/keyvalues are populated. The useful hook for reading state.
   */
  onSpawn(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /**
   * Fire as the engine DELETES an entity of `className` (`"*"` = all). The entity is still readable
   * during the synchronous handler; a stashed ref reads `null` once the slot is freed (serial gate),
   * never garbage.
   */
  onDelete(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** Find every entity whose designer-name (class) exactly matches `className`. Returns serial-gated refs. */
  findByClass(className: string): EntityRef[];
};
```

- [ ] **Step 9: Write the core in-isolate tests.**

In `core/src/v8host.rs`, inside the `#[cfg(test)] mod tests` block (place near `map_start_dispatch_delivers_map_name`, ~L9491). First add the install-capture helpers at the top of the test module (next to the other test helpers like `mock_event_ops`):

```rust
    thread_local! { static ENTITY_INSTALL_CALLS: std::cell::Cell<i32> = std::cell::Cell::new(0); }
    extern "C" fn capture_entity_install() -> c_int { ENTITY_INSTALL_CALLS.with(|c| c.set(c.get() + 1)); 1 }
```

Then the three tests:

```rust
    /// dispatch_entity_event delivers to the matching kind+class subscriber AND the "*" wildcard, with
    /// (entity, className). With no engine ops entity_resolve_ptr degrades to null, so the entity arg is
    /// null (also forced by handle=-1) and we assert on the className arg. Mirrors map_start_dispatch.
    #[test]
    fn entity_event_dispatch_delivers_class_to_matching_subscriber() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pel");
        eval_in_context_string("pel", r#"
            globalThis.__hits = [];
            var E = __s2pkg_entity.Entity;
            E.onSpawn("weapon_ak47", function (e, cls) { globalThis.__hits.push("exact:" + cls + ":" + (e === null)); });
            E.onSpawn("*",           function (e, cls) { globalThis.__hits.push("star:"  + cls + ":" + (e === null)); });
            E.onCreate("weapon_ak47", function (e, cls) { globalThis.__hits.push("create:" + cls); });
            "ok"
        "#);
        dispatch_entity_event("spawn", "weapon_ak47", -1);   // hits the exact + the "*" spawn subs, NOT the create sub
        assert_eq!(eval_in_context_string("pel", "globalThis.__hits.slice().sort().join('|')"),
                   "exact:weapon_ak47:true|star:weapon_ak47:true");
        shutdown();
    }

    /// kind separation: a "spawn" subscriber does NOT fire on a "delete"/"create" dispatch.
    #[test]
    fn entity_event_dispatch_respects_kind() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pel2");
        eval_in_context_string("pel2", r#"
            globalThis.__n = 0;
            __s2pkg_entity.Entity.onSpawn("*", function () { globalThis.__n++; });
            "ok"
        "#);
        dispatch_entity_event("delete", "prop_physics", -1);
        dispatch_entity_event("create", "prop_physics", -1);
        assert_eq!(eval_in_context_string("pel2", "String(globalThis.__n)"), "0", "spawn sub must not fire on delete/create");
        dispatch_entity_event("spawn", "prop_physics", -1);
        assert_eq!(eval_in_context_string("pel2", "String(globalThis.__n)"), "1");
        shutdown();
    }

    /// First-ever subscribe calls entity_listener_install exactly once; a second subscribe does not.
    #[test]
    fn entity_listener_install_called_once_on_first_subscribe() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps { entity_listener_install: Some(capture_entity_install), ..mock_event_ops() }));
        ENTITY_INSTALL_CALLS.with(|c| c.set(0));
        create_plugin_context("pel3");
        eval_in_context_string("pel3", r#"__s2pkg_entity.Entity.onSpawn("a", function(){}); "ok""#);
        assert_eq!(ENTITY_INSTALL_CALLS.with(|c| c.get()), 1, "install on first subscribe");
        eval_in_context_string("pel3", r#"__s2pkg_entity.Entity.onDelete("b", function(){}); "ok""#);
        assert_eq!(ENTITY_INSTALL_CALLS.with(|c| c.get()), 1, "no second install");
        shutdown();
    }
```

- [ ] **Step 10: Run the tests.**

Run: `cargo test -p s2script-core entity_event_ entity_listener_install_`
Expected: the three new tests PASS. Then run the full suite: `cargo test -p s2script-core` — expected: ALL pass (the CLAUDE.md baseline is ~252; this adds 3 → ~255, 0 failures).

- [ ] **Step 11: Boundary + typecheck gates.**

Run:
```bash
bash scripts/check-core-boundary.sh        # no games/* crate dep — expected: PASS
bash scripts/test-boundary-nameleak.sh     # no CS2 name leak in core — expected: PASS
( cd packages/cli && node build.mjs ) && bash scripts/check-plugins-typecheck.sh   # the new .d.ts compiles — expected: PASS
```

- [ ] **Step 12: Commit.**

```bash
git add shim/include/s2script_core.h core/src/v8host.rs core/src/ffi.rs packages/entity/index.d.ts
git commit -m "feat(entity-listeners): core ENTITY_MUX + Entity.onCreate/onSpawn/onDelete + ABI op

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: Shim — signature-scanned IEntityListener registration

**Files:**
- Modify: `gamedata/core.gamedata.jsonc` (+2 signatures)
- Create: `shim/src/entity_listener.cpp` (isolated SDK-including TU)
- Modify: `shim/CMakeLists.txt` (add the TU)
- Modify: `shim/src/s2script_mm.cpp` (sig resolution + register/re-assert/unload + op wiring)

**Interfaces:**
- Consumes (from Task 1): the C op `entity_listener_install` (fills it) + the extern `s2script_core_dispatch_entity_event`.
- Produces: `extern "C" void* S2_GetEntityListener()` (the isolated TU → `s2script_mm.cpp`); the resolved `AddListenerEntity`/`RemoveListenerEntity` fn ptrs; `EnsureEntityListenerRegistered()`.

### Steps

- [ ] **Step 1: Add the two gamedata signatures.**

In `gamedata/core.gamedata.jsonc`, inside `"signatures": { … }`, add (linux patterns borrowed from ModSharp git118 `sharp/gamedata/server.games.jsonc` as HINTS — re-resolved + validated UNIQUE at load):

```jsonc
    // Entity lifecycle listeners slice: CGameEntitySystem::AddListenerEntity / RemoveListenerEntity —
    // register/unregister an IEntityListener (the CSSharp/ModSharp mechanism for OnEntityCreated/Spawned/
    // Deleted). DIRECT prologue signatures borrowed as a HINT from the ModSharp git118 gamedata, per the
    // RE doctrine re-resolved + validated UNIQUE against our pinned libserver.so (loud GAMEDATA VALIDATION
    // on drift). Unresolved -> entity_listener_install no-ops (subscribe delivers nothing) / Unload skips
    // RemoveListenerEntity — degrade-never-crash. If the borrowed RemoveListenerEntity pattern is AMBIGUOUS
    // on our build (it is short), re-derive a tighter unique pattern from our libserver.so.
    "AddListenerEntity": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 41 55 49 89 FD 41 54 53 48 89 F3 48 83 EC ? 8B 8F ? ? ? ? 4C 63 E1 85 C9 7E ? 48 8B 97 ? ? ? ? 4C 63 E1 31 C0 EB ? 66 90 48 83 C0 ? 4C 39 E0 74 ? 48 39 1C C2 75 ? 48 83 C4 ? 5B 41 5C 41 5D 5D C3 66 0F 1F 44 00 ? 41 3B 8D ? ? ? ? 74 ? 49 8B 85 ? ? ? ? 83 C1 ? 41 89 8D ? ? ? ? 4A 89 1C E0 48 83 C4",
        "resolve": "direct"
      }
    },
    "RemoveListenerEntity": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 53 48 89 FB 48 83 EC 08 8B BF D0 20 00 00",
        "resolve": "direct"
      }
    },
```

- [ ] **Step 2: Create the isolated listener TU `shim/src/entity_listener.cpp`.**

This is the ONLY new TU that includes `entity2/entitysystem.h` (for `IEntityListener`) — mirrors `ekv.cpp`'s isolation so the heavy header stays out of the normal TUs. It does NOT instantiate any `CUtlVector` template (registration is a sig-call in `s2script_mm.cpp`), so no `g_pMemAlloc`/tier0 cascade.

```cpp
// Entity lifecycle listeners slice — the ONLY TU that includes entity2/entitysystem.h (for
// IEntityListener). Mirrors ekv.cpp's isolation discipline: everything else in the shim treats the
// listener as an opaque void* (via S2_GetEntityListener()). We DERIVE the real IEntityListener so the
// vtable layout matches the SDK exactly (create/spawn/delete/parentChanged in that order); we do NOT
// call m_entityListeners.AddToTail here — registration is a sig-resolved AddListenerEntity call in
// s2script_mm.cpp, so this TU instantiates no heavy CUtlVector template.
#include <entity2/entitysystem.h>       // IEntityListener, CEntityInstance
#include "s2script_core.h"              // s2script_core_dispatch_entity_event (Task 1 core export)

namespace {
class S2EntityListener : public IEntityListener {
public:
    void OnEntityCreated(CEntityInstance* pEntity) override { fire("create", pEntity); }
    void OnEntitySpawned(CEntityInstance* pEntity) override { fire("spawn",  pEntity); }
    void OnEntityDeleted(CEntityInstance* pEntity) override { fire("delete", pEntity); }
    void OnEntityParentChanged(CEntityInstance*, CEntityInstance*) override {}  // not exposed to JS (YAGNI)
private:
    static void fire(const char* kind, CEntityInstance* pEntity) {
        if (!pEntity) return;
        const char* cls = pEntity->GetClassname();          // designer name; valid at create/spawn/delete
        int handle = pEntity->GetRefEHandle().ToInt();       // packed CEntityHandle — the shim's handle idiom
        s2script_core_dispatch_entity_event(kind, cls ? cls : "", handle);
    }
};
S2EntityListener g_entityListener;   // static; lives for the process — its address is a stable IEntityListener*
}  // namespace

// Opaque accessor for s2script_mm.cpp (which never includes entitysystem.h): the IEntityListener* to
// pass to the sig-resolved AddListenerEntity/RemoveListenerEntity.
extern "C" void* S2_GetEntityListener() { return &g_entityListener; }
```

*(Note for the implementer: `CEntityInstance::GetClassname()` and `GetRefEHandle()` are already used elsewhere in the shim — confirm the exact include path via `grep -rn "GetClassname\|GetRefEHandle" shim/src/s2script_mm.cpp` and mirror whatever header brings them; `entitysystem.h` transitively includes the entity-instance headers on the vendored SDK, but add the explicit include if the build complains.)*

- [ ] **Step 3: Add the TU to `shim/CMakeLists.txt`.**

Add `${CMAKE_CURRENT_SOURCE_DIR}/src/entity_listener.cpp` (or `src/entity_listener.cpp`, matching the sibling entries) to the shim target's source list — next to `src/s2script_mm.cpp`/`src/ekv.cpp`. (Confirm the exact list syntax by `grep -n "s2script_mm.cpp\|ekv.cpp" shim/CMakeLists.txt` and add a peer line. It is a normal shim TU — NOT the vendored `${HL2SDK}/entity2/*.cpp` list.)

- [ ] **Step 4: `s2script_mm.cpp` — declare the accessor + fn-ptr statics + flag.**

Near the top of `s2script_mm.cpp` (with the other `extern "C"` / file-scope statics, e.g. by the `S2_EntitySystemBridge` / `GetEntitySystem` area ~L145–155), add:

```cpp
// Entity lifecycle listeners slice: the isolated entity_listener.cpp TU owns the IEntityListener; we
// register/unregister it here via the sig-resolved (this, IEntityListener*) member fns. void* keeps
// entitysystem.h out of this TU.
extern "C" void* S2_GetEntityListener();
using AddRemoveListenerFn = void (*)(void* gameEntitySystem, void* listener);
static AddRemoveListenerFn s_pAddListenerEntity    = nullptr;   // sig-resolved in Load
static AddRemoveListenerFn s_pRemoveListenerEntity = nullptr;   // sig-resolved in Load (best-effort)
static bool               s_wantEntityListener     = false;     // set true by the install op

// Idempotent register: AddListenerEntity guards Find, so re-asserting each map (StartupServer) is safe
// whether the entity system persists across a changelevel or is recreated with a fresh listener list.
static void EnsureEntityListenerRegistered() {
    if (!s_wantEntityListener || !s_pAddListenerEntity) return;
    CGameEntitySystem* es = GetEntitySystem();     // fresh; null before the first map
    if (es) s_pAddListenerEntity(es, S2_GetEntityListener());
}
```

- [ ] **Step 5: `s2script_mm.cpp` — resolve the two signatures in `Load()`.**

In the sig-resolution block of `Load()` (next to `CommitSuicide`/`LegacyGameEventListener`, ~L2340–2370), add (mirrors `ResolveSigValidated` + `FindModuleText`; `resolve=="direct"` → the unique match IS the function start):

```cpp
            // Entity lifecycle listeners slice: resolve CGameEntitySystem::AddListenerEntity (register an
            // IEntityListener) + RemoveListenerEntity (best-effort Unload cleanup). Both validated UNIQUE +
            // .text via ResolveSigValidated. Unresolved -> entity_listener_install no-ops / Unload skips remove.
            auto aleit = sigs.find("AddListenerEntity");
            if (aleit == sigs.end()) {
                GamedataResult("AddListenerEntity", false, "signature absent from gamedata");
            } else {
                int64_t aleOff = ResolveSigValidated("AddListenerEntity", aleit->second);
                ModText alemt = FindModuleText(aleit->second.module.c_str());
                if (aleOff != s2sig::kFail && alemt.text) {
                    s_pAddListenerEntity = reinterpret_cast<AddRemoveListenerFn>(const_cast<uint8_t*>(alemt.text) + aleOff);
                    META_CONPRINTF("[s2script] AddListenerEntity resolved @%p (entity lifecycle listeners)\n",
                                   reinterpret_cast<void*>(s_pAddListenerEntity));
                }
            }
            auto rleit = sigs.find("RemoveListenerEntity");
            if (rleit == sigs.end()) {
                GamedataResult("RemoveListenerEntity", false, "signature absent from gamedata");
            } else {
                int64_t rleOff = ResolveSigValidated("RemoveListenerEntity", rleit->second);
                ModText rlemt = FindModuleText(rleit->second.module.c_str());
                if (rleOff != s2sig::kFail && rlemt.text) {
                    s_pRemoveListenerEntity = reinterpret_cast<AddRemoveListenerFn>(const_cast<uint8_t*>(rlemt.text) + rleOff);
                    META_CONPRINTF("[s2script] RemoveListenerEntity resolved @%p (entity lifecycle listeners)\n",
                                   reinterpret_cast<void*>(s_pRemoveListenerEntity));
                }
            }
```

- [ ] **Step 6: `s2script_mm.cpp` — the `entity_listener_install` op + wiring.**

Add the op impl (near the other `Shim_*` op impls, e.g. by `Shim_EntitySetModel` ~L1758):

```cpp
// entity_listener_install: called by core on the first-ever JS entity-lifecycle subscribe. Set the
// want-flag + register now (if the entity system exists); the StartupServer POST hook re-asserts each
// map. Returns 1 if the AddListenerEntity signature resolved, else 0 (degrade — subscribe delivers nothing).
static int Shim_EntityListenerInstall() {
    s_wantEntityListener = true;
    EnsureEntityListenerRegistered();
    return s_pAddListenerEntity ? 1 : 0;
}
```

Wire it in the ops-table init (next to `ops.entity_set_model = &Shim_EntitySetModel;` ~L2722):

```cpp
    // Entity lifecycle listeners slice — APPENDED after entity_set_model; order MUST match S2EngineOps.
    ops.entity_listener_install = &Shim_EntityListenerInstall;
```

- [ ] **Step 7: `s2script_mm.cpp` — StartupServer per-map re-assert + Unload cleanup.**

In `Hook_StartupServer` (~L2969), add the re-assert after the existing `s2script_core_dispatch_map_start(...)` line, before `RETURN_META`:

```cpp
    EnsureEntityListenerRegistered();   // re-assert the IEntityListener each map (idempotent Find-guard)
```

In the plugin `Unload()` (near the `SH_REMOVE_HOOK(... StartupServer ...)` at ~L2823), add:

```cpp
    // Entity lifecycle listeners slice: unregister the IEntityListener so a dangling vtable call can't
    // happen if s2script is unloaded while the entity system lives. Best-effort (unresolved sig -> skip).
    if (s_wantEntityListener && s_pRemoveListenerEntity) {
        CGameEntitySystem* es = GetEntitySystem();
        if (es) s_pRemoveListenerEntity(es, S2_GetEntityListener());
    }
```

- [ ] **Step 8: Sniper build.**

Run:
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: BOTH `libs2script_core.so` (core changed in Task 1) and `s2script.so` (shim changed) rebuild; the script's GLIBC checks print `<= 2.31` for core and `<= 2.14` for the shim; no compile error. If `entity_listener.cpp` fails to find `GetClassname`/`GetRefEHandle`, add the explicit entity-instance include per Step 2's note and rebuild.

- [ ] **Step 9: Boundary gates.**

Run:
```bash
bash scripts/check-core-boundary.sh        # expected: PASS
bash scripts/test-boundary-nameleak.sh     # expected: PASS (entity_listener.cpp is shim, not core)
```

- [ ] **Step 10: Commit.**

```bash
git add gamedata/core.gamedata.jsonc shim/src/entity_listener.cpp shim/CMakeLists.txt shim/src/s2script_mm.cpp
git commit -m "feat(entity-listeners): shim IEntityListener via sig-scanned AddListenerEntity

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: Demo plugin + live gate + changeset + PR

**Files:**
- Create: `examples/entity-listeners-demo/{package.json,tsconfig.json,src/plugin.ts}`
- Create: `.changeset/entity-lifecycle-listeners.md`

**Interfaces:**
- Consumes: `Entity.onCreate/onSpawn/onDelete` (`@s2script/entity`), `createEntity` (`@s2script/entity`), `Commands.register` (`@s2script/commands`), `pawn.giveNamedItem` (`@s2script/cs2`) for the weapon-spawn observation.

### Steps

- [ ] **Step 1: Scaffold the demo plugin.**

`examples/entity-listeners-demo/package.json`:
```json
{
  "name": "@demo/entity-listeners-demo",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x" }
}
```

`examples/entity-listeners-demo/tsconfig.json` (copy verbatim from `examples/entityio-demo/tsconfig.json`):
```bash
cp examples/entityio-demo/tsconfig.json examples/entity-listeners-demo/tsconfig.json
```

`examples/entity-listeners-demo/src/plugin.ts`:
```ts
// Live-gate demo for entity lifecycle listeners. Self-contained + bot-provable: a spawned/removed
// logic_relay exercises onCreate/onSpawn/onDelete; a "*" spawn logger shows the create/spawn burst of
// any entity (incl. weapons given to a bot). No human client needed. The delivered `entity` is a
// serial-gated EntityRef (may be null); `className` is always valid.
import { Commands } from "@s2script/commands";
import { createEntity, Entity } from "@s2script/entity";

Entity.onSpawn("logic_relay", (e, cls) => {
  const valid = !!(e && e.isValid());
  console.log("[entlisten] onSpawn " + cls + " entity=" + (e ? ("EntityRef(valid=" + valid + ")") : "null"));
});
Entity.onCreate("logic_relay", (e, cls) => {
  console.log("[entlisten] onCreate " + cls + " entity=" + (e ? "EntityRef" : "null"));
});
Entity.onDelete("logic_relay", (e, cls) => {
  console.log("[entlisten] onDelete " + cls + " entity=" + (e ? "EntityRef" : "null"));
});

// A global "*" spawn logger — proves the wildcard + shows real engine spawns (weapons, projectiles).
let starCount = 0;
Entity.onSpawn("*", (_e, cls) => {
  starCount++;
  if (starCount <= 40) console.log("[entlisten] * onSpawn: " + cls);   // cap the map-load burst log
});

Commands.register("sm_entlisten", (ctx) => {
  const relay = createEntity("logic_relay");
  if (!relay) { ctx.reply("[entlisten] createEntity failed"); return; }
  relay.spawn();   // -> onCreate then onSpawn fire (watch the log)
  relay.remove();  // -> onDelete fires (next tick)
  ctx.reply("[entlisten] spawned+removed a logic_relay; watch the log for onCreate/onSpawn/onDelete");
});

export function onLoad(): void {
  console.log("[entity-listeners-demo] onLoad — sm_entlisten registered; * onSpawn logging armed");
}
export function onUnload(): void {}
```

- [ ] **Step 2: Build the demo (typecheck gate).**

Run:
```bash
( cd packages/cli && node build.mjs ) && node packages/cli/dist/cli.js build examples/entity-listeners-demo
```
Expected: produces `examples/entity-listeners-demo/dist/*.s2sp`; the full-strict typecheck passes (validates the new `@s2script/entity` `.d.ts` surface). Then `bash scripts/check-plugins-typecheck.sh` — expected PASS.

- [ ] **Step 3: Add the changeset.**

`.changeset/entity-lifecycle-listeners.md`:
```md
---
"@s2script/entity": minor
---

Entity lifecycle listeners: `Entity.onCreate` / `onSpawn` / `onDelete(className, handler)` fire when the
engine creates/spawns/deletes an entity of `className` (`"*"` = all), delivering a serial-gated
`EntityRef` (may be null) plus the `className`. Class-keyed, notify-only. Backed by a signature-scanned
`CGameEntitySystem::AddListenerEntity` (the CSSharp/ModSharp `IEntityListener` mechanism).
```

- [ ] **Step 4: Deploy to the live server.**

Run (the sniper `.so`s from Task 2 Step 8 must already exist; `package-addon.sh` wipes `dist/addons/s2script`, so recreate a writable `configs/` if any plugin needs it — the demo does not):
```bash
bash scripts/package-addon.sh
cp examples/entity-listeners-demo/dist/*.s2sp dist/addons/s2script/plugins/
cd docker && docker compose restart cs2   # NOT --force-recreate; re-run `docker exec s2script-cs2 /patch-gameinfo.sh` first if a game update intervened
```

- [ ] **Step 5: Live gate — verify.**

- Boot log (`docker logs s2script-cs2 --tail 200 | grep -i "GAMEDATA\|AddListenerEntity\|RemoveListenerEntity\|entity-listeners-demo"`): the `=== GAMEDATA VALIDATION: N ok, 0 FAILED ===` count is **+2** vs the pre-change baseline (both `AddListenerEntity` + `RemoveListenerEntity` resolve UNIQUE); `AddListenerEntity resolved @0x…`; `[entity-listeners-demo] onLoad`.
  - **If `RemoveListenerEntity` reports AMBIGUOUS/NOT FOUND:** its borrowed pattern is short — re-derive a unique pattern from our `libserver.so` (disassemble around the borrowed bytes; anchor on the `8B BF D0 20 00 00` m_entityListeners load) and update the gamedata; the count target is then still +2. (A degraded `RemoveListenerEntity` alone does NOT block the slice — only Unload cleanup is lost.)
- `python3 scripts/rcon.py "sm_entlisten"` → the log shows `[entlisten] onCreate logic_relay …`, `[entlisten] onSpawn logic_relay entity=EntityRef(valid=true)`, then next tick `[entlisten] onDelete logic_relay …`. The `* onSpawn` logger also printed a burst of real classes at map load.
- `docker logs s2script-cs2 | grep -i "RestartCount"` → `RestartCount=0`; server ticking; no panic/abort.

Record the exact GAMEDATA baseline→+2 and the observed classes in the PR body. Note the human-client deferral (real bullet/grenade gameplay spawns) if relevant per [[deferred-live-tests]] — the mechanism is proven by the self-contained relay + the map-load `*` burst.

- [ ] **Step 6: Commit + PR.**

```bash
git add examples/entity-listeners-demo .changeset/entity-lifecycle-listeners.md
git commit -m "test(entity-listeners): demo plugin + changeset; live-gate proof

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
git push -u origin feat/entity-lifecycle-listeners
gh pr create --title "feat(entity): entity lifecycle listeners — Entity.onCreate/onSpawn/onDelete" --body "$(cat <<'EOF'
The 4th notify-mux: an IEntityListener on CGameEntitySystem → core ENTITY_MUX → JS subscribers,
class-keyed, notify-only. Registration via a signature-scanned AddListenerEntity (ModSharp hint,
load-validated) — the CSSharp/ModSharp mechanism. Serial-gated EntityRef crosses to JS.

Closes the CSSharp `Listeners.OnEntitySpawned`/`OnEntityCreated`/`OnEntityDeleted` parity gap.

Live-gate: <fill in — GAMEDATA baseline→+2, sm_entlisten onCreate/onSpawn/onDelete, the * burst, RestartCount=0>.

https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
)"
```

---

## Self-Review

**1. Spec coverage.** Spec §Architecture (shim listener / core mux / prelude) → Tasks 1–2. §Registration decision (sig-scan AddListenerEntity) → Task 2 Steps 1,5. §Lazy install + map re-assert → Task 1 (op + `first_ever`) + Task 2 (Shim_EntityListenerInstall + StartupServer re-assert). §API (`onCreate/onSpawn/onDelete`, `"*"`, notify-only) → Task 1 Steps 7–8. §Safety (serial gate, re-entrancy, degrade) → Task 1 Step 4 (`-1` sentinel, `try_borrow_mut`, `entity_resolve_ptr` null) + Global Constraints. §Boundary → Global Constraints + Task 1 Step 11 / Task 2 Step 9. §Testing (core in-isolate + live gate) → Task 1 Step 9 / Task 3 Step 5. §Cost (one op, two sigs, one sniper) → matches. `OnEntityParentChanged` deferred → Task 2 Step 2 (no-op override). All covered.

**2. Placeholder scan.** No TBD/TODO; every code step has complete code. The only `<fill in>` is the PR body's live-gate numbers (correct — captured at run time). The two `~L####` anchors are guidance; each step also gives a `grep` to locate the exact site.

**3. Type consistency.** `entity_listener_install` op: C `s2_entity_listener_install_fn` = `int(*)(void)` ↔ Rust `EntityListenerInstallFn = extern "C" fn() -> c_int` ↔ shim `Shim_EntityListenerInstall() -> int`. Consistent. `dispatch_entity_event(kind, className, handle)`: ffi export `(*const c_char, *const c_char, c_int)` → `dispatch_entity_event(&str, &str, i32)` → shim call `(const char*, const char*, int)`. Consistent. Natives `__s2_entity_listener_on/off` ↔ prelude `Entity.on{Create,Spawn,Delete}` → `__s2_entity_listener_on("create"|"spawn"|"delete", className, handler)` ↔ mux key `"<kind>\0<className>"` ↔ `dispatch_entity_event` builds the same keys. Handler shape `(entity, className)` consistent across dispatch (2 args), prelude, `.d.ts`, demo, tests. `S2_GetEntityListener()` produced by Task 2 Step 2, consumed by Steps 4/7. Consistent.
