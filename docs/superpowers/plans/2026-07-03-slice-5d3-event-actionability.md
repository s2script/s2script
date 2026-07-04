# Slice 5D.3 — event actionability (pre-hooks: block + modify, + firing) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the write direction to game events — pre-hooks that can **block** (suppress client broadcast) or **modify** an event, plus **firing** events — by hooking `IGameEventManager2::FireEvent` (the sig-scanned manager from 5D.2) with SourceHook, reusing the existing `HookResult` collapse machinery.

**Architecture:** One SourceHook on `FireEvent`; a Pre hook runs JS `Events.onPre` subscribers through `core/src/multiplexer.rs::run_chain` (the same collapse `OnGameFrame` uses) and on `Handled`/`Stop` re-calls the original with `bDontBroadcast=true` via `SH_CALL` + `MRES_SUPERCEDE`. Event setters + `Events.fire` share one write target (`s_currentEvent`, save/restore on create/fire). Mechanism is engine-generic (core/shim); only the typed `onPre<K>`/`fire<K>` overlay is CS2.

**Tech Stack:** Rust `cdylib` core (rusty_v8), C++ Metamod shim (SourceHook), the injected JS prelude, the `@s2script/events`/`@s2script/cs2` `.d.ts`, the `eventgen` codegen, Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-07-03-slice-5d3-event-actionability-design.md`.

## Global Constraints

- **Core stays engine-generic.** No CS2 identifier in `core/src` (`IGameEventManager2`/`IGameEvent` are Source2 ENGINE types → shim only). Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **`Events.on` (notify/post) is UNCHANGED** — this slice is purely additive (the pre path + firing).
- **ABI append-only.** New ops APPENDED to `S2EngineOps` after the 5D.2 `client_find_by_userid` field — identical order in `shim/include/s2script_core.h` AND the Rust `#[repr(C)]` mirror in `core/src/v8host.rs`. Never reorder/insert above existing fields.
- **Degrade-never-crash.** Null manager → the `FireEvent` hook never installs, `onPre` never fires, `set*`/`create`/`fire` no-op (`false`/undefined). `set*` outside a pre-hook / created event → no-op. A throwing pre-hook drops that sub from the collapse (never wrongly suppresses).
- **HookResult values (must match the existing `globalThis.HookResult` at v8host.rs:310):** `{ Continue:0, Changed:1, Handled:2, Stop:3 }`; the Rust `HookResult` enum (`multiplexer.rs:5`) derives `Ord` in that order. Suppress-broadcast iff the collapsed result `>= Handled`.
- **Suppress semantic (user-confirmed): SM-parity.** `Handled`/`Stop` → `SH_CALL(FireEvent)(ev, /*bDontBroadcast=*/true)` + `MRES_SUPERCEDE` — the event still fires server-side (post-subs still see it), only the client broadcast is suppressed.
- **64-bit parity (from 5B.4):** `getUint64`/`setUint64` use a DECIMAL STRING at the JS boundary (wire-safe, SM-parity).
- **Commit trailer:** every commit message ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`.
- **Test runners:** core = `cargo test -p s2script-core -- --test-threads=1`; CLI/JS = `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs`.
- **Build/live is controller-driven** (Task 5): ONE sniper build; redeploy via `docker compose restart cs2` (re-binds the addon mount + keeps the gameinfo patch — NOT `--force-recreate`).

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `core/src/event_mux.rs` | Modify | `EventMux::is_empty()` (global pre-hook install trigger) |
| `core/src/v8host.rs` | Modify | `EVENT_MUX_PRE` static; `dispatch_game_event_pre`; set/create/fire + subscribe_pre natives; `S2EngineOps` Rust mirror append; prelude (GameEvent setters, `Events.onPre`/`fire`, `HookResult` export); register + teardown |
| `core/src/ffi.rs` | Modify | `s2script_core_dispatch_game_event_pre` C export |
| `shim/include/s2script_core.h` | Modify | 7 event-write/fire op typedefs + struct append; `dispatch_game_event_pre` decl |
| `shim/src/s2script_mm.cpp` | Modify | `SH_DECL_HOOK2` FireEvent; `Hook_FireEventPre`; `"GameEvent"` request-hook branch; set/create/fire op impls (`s_currentEvent` write target + save/restore); wire into `S2EngineOps` |
| `packages/events/index.d.ts` | Modify | `GameEvent` setters; `Events.onPre`/`fire`; `HookResult` |
| `packages/cli/src/eventgen/emit-dts.ts` | Modify | emit `Events.onPre<K>` + `Events.fire<K>` |
| `packages/cs2/events.generated.d.ts` | Modify (regen) | the regenerated typed overlay |
| `scripts/check-events-generated.sh` | (unchanged gate) | freshness of the regen |
| `packages/cli/test/*.mjs` | Create/Modify | eventgen emit + a vm test for onPre/fire |
| `examples/demo-plugin/src/plugin.ts` | Modify | live-gate demo |
| `README.md` / `CLAUDE.md` | Modify | document the slice |

---

## Task 1: Core pre-multiplexer + `dispatch_game_event_pre`

**Files:**
- Modify: `core/src/event_mux.rs`, `core/src/v8host.rs`, `core/src/ffi.rs`, `shim/include/s2script_core.h`

**Interfaces:**
- Consumes: `multiplexer::{run_chain, HookResult, Priority, SubId}`; the existing `EVENT_MUX` pattern; `dispatch_game_event`'s GameEvent-construction + liveness pattern.
- Produces: a second pre-subscription mux `EVENT_MUX_PRE` (reuses `EventMux<v8::Global<v8::Function>>`); `pub(crate) fn dispatch_game_event_pre(name: &str) -> i32` (1 = suppress broadcast, 0 = allow); the C ABI `s2script_core_dispatch_game_event_pre(name) -> c_int`; `EventMux::is_empty()`.

- [ ] **Step 1: Add `EventMux::is_empty()` + a failing test in `core/src/event_mux.rs`**

Add the method (in the `impl<H: Clone> EventMux<H>` block):

```rust
    /// True iff no name has any subscriber (the trigger to install/remove a GLOBAL hook,
    /// as opposed to the per-name `subscribe` "first for this name" signal).
    pub fn is_empty(&self) -> bool {
        self.by_name.values().all(|v| v.is_empty())
    }
```

Add a test in the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn is_empty_tracks_any_subscriber() {
        let mut m: EventMux<&str> = EventMux::new();
        assert!(m.is_empty());
        m.subscribe("player_hurt", "p".into(), 1, "h");
        assert!(!m.is_empty());
        m.remove_by_owner("p");
        assert!(m.is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core event_mux::tests::is_empty -- --test-threads=1`
Expected: FAIL — `no method named 'is_empty'`.

- [ ] **Step 3: (method added in Step 1) Run to verify it passes**

Run: `cargo test -p s2script-core event_mux::tests::is_empty -- --test-threads=1`
Expected: PASS.

- [ ] **Step 4: Add `EVENT_MUX_PRE` + `dispatch_game_event_pre` in `core/src/v8host.rs`**

Next to the `EVENT_MUX` thread-local (around v8host.rs:242), add a sibling:

```rust
    static EVENT_MUX_PRE: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
```

Add the pre-dispatch — a hybrid of `dispatch_game_event` (constructs `new GameEvent(name)`, liveness) and `dispatch_onframe` (`run_chain` collapse + return→HookResult). Place it next to `dispatch_game_event`:

```rust
/// Pre-dispatch for the FireEvent hook (Slice 5D.3). Runs the PRE subscribers for `name`, collapses
/// their HookResults via `run_chain`, and returns 1 to suppress client broadcast (collapsed result
/// >= Handled) or 0 to allow. The shim has set `s_currentEvent` (mutable) before calling this.
pub(crate) fn dispatch_game_event_pre(name: &str) -> i32 {
    use crate::multiplexer::{run_chain, HookResult, Priority, SubId};
    // Phase 1: snapshot — release EVENT_MUX_PRE borrow before entering any context.
    let snap0 = EVENT_MUX_PRE.with(|m| m.borrow().snapshot(name));   // Vec<(owner, gen, Global<Function>)>
    if snap0.is_empty() { return 0; }
    // run_chain wants (SubId, Priority, H); all pre-hooks are Priority::Normal this slice (order =
    // subscription order; a priority param is deferred). SubId = enumerate index (only used for the
    // errored list). Carry (owner, gen, handler) so the invoke closure can route + liveness-check.
    let snap: Vec<(SubId, Priority, (String, u64, v8::Global<v8::Function>))> = snap0
        .into_iter().enumerate()
        .map(|(i, (owner, gen, h))| (i as SubId, Priority::Normal, (owner, gen, h)))
        .collect();

    let outcome = HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow.as_mut().expect("dispatch_game_event_pre before init");
        run_chain(&snap, |(owner, gen, handler_g): &(String, u64, v8::Global<v8::Function>)| {
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *gen)) { return Ok(HookResult::Continue); }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone()))
                else { return Ok(HookResult::Continue); };
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            // Construct new GameEvent(name) from globalThis.__s2pkg_events.GameEvent (as in dispatch_game_event).
            let event_arg: Option<v8::Local<v8::Value>> = (|| {
                let global = ctx_local.global(tc);
                let pkg_key = v8::String::new(tc, "__s2pkg_events")?;
                let pkg = global.get(tc, pkg_key.into())?;
                let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
                let ctor_key = v8::String::new(tc, "GameEvent")?;
                let ctor = v8::Local::<v8::Function>::try_from(pkg.get(tc, ctor_key.into())?).ok()?;
                let name_str = v8::String::new(tc, name)?;
                ctor.new_instance(tc, &[name_str.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
            })();
            let event_val: v8::Local<v8::Value> = event_arg.unwrap_or_else(|| v8::undefined(tc).into());
            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            match func.call(tc, recv, &[event_val]) {
                None => Err(()),                                   // threw → drop this sub
                Some(ret) if ret.is_undefined() => Ok(HookResult::Continue),
                Some(ret) => Ok(match ret.uint32_value(tc).unwrap_or(0) {
                    0 => HookResult::Continue, 1 => HookResult::Changed,
                    2 => HookResult::Handled, 3 => HookResult::Stop,
                    _ => HookResult::Continue,                     // out-of-range → Continue
                }),
            }
        })
    });
    if outcome.result >= HookResult::Handled { 1 } else { 0 }
}
```

- [ ] **Step 5: Add the C ABI export in `core/src/ffi.rs`**

Mirror `s2script_core_dispatch_game_event` (ffi.rs:78):

```rust
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_game_event_pre(name: *const c_char) -> c_int {
    if name.is_null() { return 0; }
    let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0; };
    std::panic::catch_unwind(|| v8host::dispatch_game_event_pre(name_str)).unwrap_or(0)
}
```

- [ ] **Step 6: Declare it in `shim/include/s2script_core.h`**

After the `s2script_core_dispatch_game_event` declaration:

```c
/* Shim -> core: called by the FireEvent Pre hook (Slice 5D.3). Runs the PRE subscribers for `name`
 * (s_currentEvent is set + mutable during the call). Returns 1 to suppress the client broadcast
 * (a pre-hook returned Handled/Stop), else 0. */
int s2script_core_dispatch_game_event_pre(const char* name);
```

- [ ] **Step 7: Write the in-isolate collapse test (`core/src/v8host.rs`)**

In `#[cfg(test)] mod tests`, mirroring the event/entity in-isolate tests (`init(dummy_logger())` / `create_plugin_context`). Register a pre-sub in JS that returns a HookResult and assert the collapsed suppress decision:

```rust
    #[test]
    fn dispatch_game_event_pre_collapses_handled_to_suppress() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // No pre-subs → allow (0).
        assert_eq!(dispatch_game_event_pre("player_hurt"), 0);
        // A pre-sub returning HookResult.Handled → suppress (1).
        eval_in_context("p", r#"__s2_event_subscribe_pre("player_hurt", function(ev){ return HookResult.Handled; });"#).unwrap();
        assert_eq!(dispatch_game_event_pre("player_hurt"), 1);
        // A different name with a Continue sub → allow (0).
        eval_in_context("p", r#"__s2_event_subscribe_pre("round_start", function(ev){ return HookResult.Continue; });"#).unwrap();
        assert_eq!(dispatch_game_event_pre("round_start"), 0);
        shutdown();
    }
```

Note to implementer: this test depends on the `__s2_event_subscribe_pre` native + the `HookResult` global — both land in Task 2. So this test is written HERE but will only pass once Task 2 is done. To keep Task 1 self-testing, split: assert the no-subs path (`dispatch_game_event_pre("x") == 0`) in Task 1 (passes immediately), and add the Handled/Continue assertions in Task 2's test step. **Task 1 Step 7 = only the no-subs assertion**; the full collapse assertions move to Task 2.

Task-1 Step 7 test (self-contained):

```rust
    #[test]
    fn dispatch_game_event_pre_no_subs_allows() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        assert_eq!(dispatch_game_event_pre("player_hurt"), 0);   // no pre-subs → allow
        shutdown();
    }
```

- [ ] **Step 8: Run the core suite**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: all pass (existing + `is_empty_tracks_any_subscriber` + `dispatch_game_event_pre_no_subs_allows`).

- [ ] **Step 9: Commit**

```bash
git add core/src/event_mux.rs core/src/v8host.rs core/src/ffi.rs shim/include/s2script_core.h
git commit -m "$(printf 'feat(slice5d3): core pre-multiplexer + dispatch_game_event_pre\n\nEVENT_MUX_PRE (reuses EventMux) + dispatch_game_event_pre: constructs GameEvent(name) per-sub (like\ndispatch_game_event), collapses pre-hook HookResults via multiplexer run_chain (like dispatch_onframe),\nreturns 1 (suppress broadcast) iff collapsed >= Handled. EventMux::is_empty() for the global-hook\ninstall trigger; the s2script_core_dispatch_game_event_pre C ABI + header decl. Core-only; no-subs\npath tested (full collapse asserted in Task 2 once the pre-subscribe native + HookResult land).\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 2: Event write/fire ops (contract + Rust natives) + JS API

**Files:**
- Modify: `shim/include/s2script_core.h`, `core/src/v8host.rs`

**Interfaces:**
- Consumes: Task 1's `EVENT_MUX_PRE`, `dispatch_game_event_pre`, `EventMux::is_empty()`; the `HOOK_REQUEST` mechanism (`set_hook_request`/`request_hook`); the event-native pattern (`s2_event_subscribe`, `s2_event_get_int`, `s2_event_get_string`).
- Produces: 7 new `S2EngineOps` fields (C header + Rust mirror); the natives `__s2_event_subscribe_pre`/`__s2_event_unsubscribe_pre`, `__s2_event_set_int/float/bool/string/uint64`, `__s2_event_create`/`__s2_event_fire`; the prelude `GameEvent` setters + `Events.onPre`/`fire` + `HookResult` in `__s2pkg_events`.

- [ ] **Step 1: Append the 7 op typedefs + struct fields in `shim/include/s2script_core.h`**

After the `s2_client_find_by_userid_fn` typedef:

```c
/* Event write/fire ops (Slice 5D.3). Write the shim's current write target (the pre-hook's live
 * IGameEvent, OR a just-created to-be-fired event). All no-op if the target/manager is null. */
typedef void (*s2_event_set_int_fn)(const char* key, int value);
typedef void (*s2_event_set_float_fn)(const char* key, float value);
typedef void (*s2_event_set_bool_fn)(const char* key, int value);       /* 0/1 */
typedef void (*s2_event_set_string_fn)(const char* key, const char* value);
typedef void (*s2_event_set_uint64_fn)(const char* key, uint64_t value);
typedef int  (*s2_event_create_fn)(const char* name);                   /* 1 = created (retargets writes); 0 = null mgr / unknown name */
typedef int  (*s2_event_fire_fn)(int dontBroadcast);                    /* returns FireEvent result; 0 if no created event */
```

In `S2EngineOps`, APPEND after `client_find_by_userid`:

```c
    /* Event write/fire ops (Slice 5D.3) — APPENDED after the client ops; order is the ABI. */
    s2_event_set_int_fn    event_set_int;
    s2_event_set_float_fn  event_set_float;
    s2_event_set_bool_fn   event_set_bool;
    s2_event_set_string_fn event_set_string;
    s2_event_set_uint64_fn event_set_uint64;
    s2_event_create_fn     event_create;
    s2_event_fire_fn       event_fire;
```

- [ ] **Step 2: Append the Rust mirror aliases + fields in `core/src/v8host.rs`**

After the `ClientFindByUseridFn` alias:

```rust
// --- Slice 5D.3: event write/fire ops (C-ABI; the C header must match exactly) ---
pub type EventSetIntFn    = extern "C" fn(key: *const c_char, value: i32);
pub type EventSetFloatFn  = extern "C" fn(key: *const c_char, value: f32);
pub type EventSetBoolFn   = extern "C" fn(key: *const c_char, value: c_int);
pub type EventSetStringFn = extern "C" fn(key: *const c_char, value: *const c_char);
pub type EventSetUint64Fn = extern "C" fn(key: *const c_char, value: u64);
pub type EventCreateFn    = extern "C" fn(name: *const c_char) -> c_int;
pub type EventFireFn      = extern "C" fn(dont_broadcast: c_int) -> c_int;
```

APPEND to `struct S2EngineOps` after `client_find_by_userid`:

```rust
    // --- Slice 5D.3: event write/fire ops (APPENDED — order is the ABI; do not reorder above) ---
    pub event_set_int:    Option<EventSetIntFn>,
    pub event_set_float:  Option<EventSetFloatFn>,
    pub event_set_bool:   Option<EventSetBoolFn>,
    pub event_set_string: Option<EventSetStringFn>,
    pub event_set_uint64: Option<EventSetUint64Fn>,
    pub event_create:     Option<EventCreateFn>,
    pub event_fire:       Option<EventFireFn>,
```

- [ ] **Step 3: Add the natives in `core/src/v8host.rs`**

**Pre-subscribe / unsubscribe** — mirror `s2_event_subscribe`/`s2_event_unsubscribe` (v8host.rs:1636+) but target `EVENT_MUX_PRE` and gate the GLOBAL `"GameEvent"` hook on total emptiness (not per-name):

```rust
/// Native `__s2_event_subscribe_pre(name, handler)` — register a pre-hook (can block/modify).
fn s2_event_subscribe_pre(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(1)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        let was_empty = EVENT_MUX_PRE.with(|m| m.borrow().is_empty());
        EVENT_MUX_PRE.with(|m| { m.borrow_mut().subscribe(&name, owner, generation, handler_g); });
        if was_empty {   // first pre-sub across ALL names → install the global FireEvent hook
            if let Some(req) = HOOK_REQUEST.with(|r| r.get()) {
                if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 1); }
            }
        }
    }));
}

/// Native `__s2_event_unsubscribe_pre(name, handler)` — remove this plugin's pre-hooks for `name`.
fn s2_event_unsubscribe_pre(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        EVENT_MUX_PRE.with(|m| { m.borrow_mut().remove_by_owner_on(&name, &owner); });
        if EVENT_MUX_PRE.with(|m| m.borrow().is_empty()) {   // last pre-sub gone → remove the hook
            if let Some(req) = HOOK_REQUEST.with(|r| r.get()) {
                if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 0); }
            }
        }
    }));
}
```

Note to implementer: use the EXACT `HOOK_REQUEST` accessor the code uses (grep `HOOK_REQUEST`/`set_hook_request` in v8host.rs; it mirrors `ENGINE_OPS`/`LOGGER` `.with(|x| x.get())`). If the request fn is stored differently, match it.

**Setters** — mirror the arg-parsing of `s2_event_get_int`/`get_string` but call the write op (void return). Example (`set_int`; do the analogous 5):

```rust
/// Native `__s2_event_set_int(key, value)` — write the current event's int field (pre-hook / fire builder).
fn s2_event_set_int(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).int32_value(scope).unwrap_or(0);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_set_int else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}
```

- `set_float`: `args.get(1).number_value(scope).unwrap_or(0.0) as f32` → `event_set_float`.
- `set_bool`: `args.get(1).boolean_value(scope) as c_int` → `event_set_bool`.
- `set_string`: second arg `to_rust_string_lossy` → `CString` → `event_set_string`.
- `set_uint64`: second arg is a DECIMAL STRING; `key`+`val_str.parse::<u64>().unwrap_or(0)` → `event_set_uint64`.

**Create / Fire:**

```rust
/// Native `__s2_event_create(name) -> boolean` — create a to-be-fired event; retargets set* writes to it.
fn s2_event_create(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_create else { return };
        let Ok(cn) = CString::new(name.as_str()) else { return };
        rv.set_bool(func(cn.as_ptr()) != 0);
    }));
}

/// Native `__s2_event_fire(dontBroadcast) -> boolean` — fire the created event; restores the write target.
fn s2_event_fire(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let dont = if args.length() >= 1 { args.get(0).boolean_value(scope) } else { false };
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_fire else { return };
        rv.set_bool(func(dont as c_int) != 0);
    }));
}
```

Register all 9 where the event natives register (near `set_native(... "__s2_event_subscribe" ...)`):

```rust
    set_native(scope, global_obj, "__s2_event_subscribe_pre", s2_event_subscribe_pre);
    set_native(scope, global_obj, "__s2_event_unsubscribe_pre", s2_event_unsubscribe_pre);
    set_native(scope, global_obj, "__s2_event_set_int", s2_event_set_int);
    set_native(scope, global_obj, "__s2_event_set_float", s2_event_set_float);
    set_native(scope, global_obj, "__s2_event_set_bool", s2_event_set_bool);
    set_native(scope, global_obj, "__s2_event_set_string", s2_event_set_string);
    set_native(scope, global_obj, "__s2_event_set_uint64", s2_event_set_uint64);
    set_native(scope, global_obj, "__s2_event_create", s2_event_create);
    set_native(scope, global_obj, "__s2_event_fire", s2_event_fire);
```

- [ ] **Step 4: Extend the prelude in `core/src/v8host.rs` (GameEvent setters + Events.onPre/fire + HookResult export)**

After the `GameEvent.prototype.getPlayerSlot` line, add setters:

```javascript
  GameEvent.prototype.setInt    = function (k, v) { __s2_event_set_int(k, v | 0); };
  GameEvent.prototype.setFloat  = function (k, v) { __s2_event_set_float(k, v); };
  GameEvent.prototype.setBool   = function (k, v) { __s2_event_set_bool(k, !!v); };
  GameEvent.prototype.setString = function (k, v) { __s2_event_set_string(k, String(v)); };
  GameEvent.prototype.setUint64 = function (k, v) { __s2_event_set_uint64(k, String(v)); };   // decimal string
```

Extend the `Events` object (keep `on`/`off`):

```javascript
  var Events = {
    on:    function (name, handler) { __s2_event_subscribe(name, handler); },
    off:   function (name, handler) { __s2_event_unsubscribe(name, handler); },
    onPre: function (name, handler) { __s2_event_subscribe_pre(name, handler); },
    // Fire a game event. fields: { key: value }. Runtime type-infer: bool→setBool, string→setString,
    // bigint→setUint64, integer number→setInt, other number→setFloat. Returns the FireEvent result.
    fire:  function (name, fields, dontBroadcast) {
      if (!__s2_event_create(name)) return false;
      if (fields) {
        for (var k in fields) {
          if (!Object.prototype.hasOwnProperty.call(fields, k)) continue;
          var v = fields[k];
          var t = typeof v;
          if (t === "boolean") __s2_event_set_bool(k, v);
          else if (t === "string") __s2_event_set_string(k, v);
          else if (t === "bigint") __s2_event_set_uint64(k, v.toString());
          else if (t === "number") { if (Number.isInteger(v)) __s2_event_set_int(k, v); else __s2_event_set_float(k, v); }
        }
      }
      return __s2_event_fire(!!dontBroadcast);
    },
  };
```

Add `HookResult` to the events package object (it already exists as `globalThis.HookResult`):

```javascript
  globalThis.__s2pkg_events     = { GameEvent: GameEvent, Events: Events, HookResult: globalThis.HookResult };
```

- [ ] **Step 5: Teardown — clear `EVENT_MUX_PRE` on shutdown + remove-by-owner on unload**

Where `EVENT_MUX` is reset on shutdown (v8host.rs:2586) add `EVENT_MUX_PRE.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());`. Where a plugin's `EVENT_MUX` subs are removed on unload (v8host.rs:2718, `remove_by_owner(id)`), add the same for `EVENT_MUX_PRE` and, if it becomes empty, request the `"GameEvent"` hook removal (mirror the unsubscribe_pre emptiness check). Note to implementer: match the exact teardown site + the `emptied_events` handling.

- [ ] **Step 6: Move the full collapse test here (from Task 1 Step 7)**

Add to `#[cfg(test)] mod tests` (now the pre-subscribe native + HookResult exist):

```rust
    #[test]
    fn pre_hooks_collapse_and_degrade() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // Handled → suppress (1).
        eval_in_context("p", r#"__s2_event_subscribe_pre("player_hurt", function(ev){ return HookResult.Handled; });"#).unwrap();
        assert_eq!(dispatch_game_event_pre("player_hurt"), 1);
        // Continue on another name → allow (0).
        eval_in_context("p", r#"__s2_event_subscribe_pre("round_start", function(ev){ return HookResult.Continue; });"#).unwrap();
        assert_eq!(dispatch_game_event_pre("round_start"), 0);
        // set*/create/fire degrade with no ops (no crash).
        assert_eq!(eval_in_context_string("p", r#"var {Events}=__s2pkg_events; String(Events.fire("x",{a:1}))"#), "false");
        eval_in_context("p", r#"var e = new (__s2pkg_events.GameEvent)("t"); e.setInt("k", 5);"#).unwrap();  // no-op, no throw
        shutdown();
    }
```

- [ ] **Step 7: Run core + boundary gates**

Run:
```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
Expected: all core pass (incl. `pre_hooks_collapse_and_degrade`); both gates green (no CS2 name in core).

- [ ] **Step 8: Commit**

```bash
git add shim/include/s2script_core.h core/src/v8host.rs
git commit -m "$(printf 'feat(slice5d3): event write/fire ops + Events.onPre/fire + GameEvent setters\n\n7 ops appended to S2EngineOps (C header + Rust mirror, ABI order): event_set_int/float/bool/string/uint64\n+ event_create + event_fire. Natives: __s2_event_subscribe_pre/unsubscribe_pre (global FireEvent hook\ninstall gated on EVENT_MUX_PRE emptiness), the 5 setters, create/fire. Prelude: GameEvent setters,\nEvents.onPre/fire (runtime type-infer), HookResult into __s2pkg_events. EVENT_MUX_PRE teardown. Collapse\n+ degrade tested in-isolate; boundary green.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 3: Shim — the FireEvent SourceHook + op impls

**Files:**
- Modify: `shim/src/s2script_mm.cpp`

**Interfaces:**
- Consumes: `s_pGameEventManager` (5D.2); `s_currentEvent` + the save/restore discipline (5D.1); `s2script_core_dispatch_game_event_pre` (Task 1); the 7 op typedefs (Task 2); the `s2_request_hook` pattern + the `Hook_GameFramePre` model.
- Produces: the installed `FireEvent` Pre hook + the 7 op impls wired into `S2EngineOps`.

- [ ] **Step 1: Declare the FireEvent hook**

Near the `SH_DECL_HOOK3_void(ISource2Server, GameFrame, ...)` line (s2script_mm.cpp:45):

```cpp
// FireEvent(IGameEvent*, bool bDontBroadcast) -> bool (Slice 5D.3). Pre hook only.
SH_DECL_HOOK2(IGameEventManager2, FireEvent, SH_NOATTRIB, 0, bool, IGameEvent*, bool);
```

- [ ] **Step 2: Add the write-target statics + the pending-fire save + the hook member/impl**

Near `s_currentEvent` (s2script_mm.cpp:220), add a pending-fire save slot + an installed flag (mirror `m_frameHookInstalled`):

```cpp
// Slice 5D.3: Events.fire creates an event and retargets s_currentEvent to it (save/restore on
// create/fire) so the same set* ops serve both pre-hook modify and fire-building. Nests correctly.
static IGameEvent* s_pendingFireEvent = nullptr;
static IGameEvent* s_savedCurrentEvent = nullptr;   // s_currentEvent saved by event_create, restored by event_fire
```

Add the hook method to `S2ScriptPlugin` (mirror `Hook_GameFramePre`; declare in the class + define):

```cpp
// FireEvent Pre hook: run pre-subscribers (they may getX/setX + return a HookResult); if they collapse
// to "suppress broadcast", re-call the original with bDontBroadcast=true and SUPERCEDE.
bool S2ScriptPlugin::Hook_FireEventPre(IGameEvent* ev, bool bDontBroadcast) {
    if (!ev) RETURN_META_VALUE(MRES_IGNORED, true);
    IGameEvent* prev = s_currentEvent;
    s_currentEvent = ev;                                       // mutable during the pre-dispatch
    int suppress = s2script_core_dispatch_game_event_pre(ev->GetName());
    s_currentEvent = prev;                                     // restore (re-entrancy)
    if (suppress) {
        bool ret = SH_CALL(s_pGameEventManager, &IGameEventManager2::FireEvent)(ev, true);
        RETURN_META_VALUE(MRES_SUPERCEDE, ret);                // we fired it ourselves with broadcast off
    }
    RETURN_META_VALUE(MRES_IGNORED, true);                     // original runs; any set* mods already applied
}
```

Note to implementer: add `bool Hook_FireEventPre(IGameEvent*, bool);` to the `S2ScriptPlugin` class declaration next to `Hook_GameFramePre`.

- [ ] **Step 3: Add the `"GameEvent"` branch to `s2_request_hook`**

Extend `s2_request_hook` (s2script_mm.cpp:583) — currently `if (strcmp(descriptor,"OnGameFrame")!=0) return;`. Restructure to handle both:

```cpp
static void s2_request_hook(const char* descriptor, int enable) {
    if (strcmp(descriptor, "OnGameFrame") == 0) {
        // ... existing OnGameFrame block unchanged ...
        return;
    }
    if (strcmp(descriptor, "GameEvent") == 0) {
        if (enable && !g_S2ScriptPlugin.m_eventHookInstalled && s_pGameEventManager) {
            SH_ADD_HOOK(IGameEventManager2, FireEvent, s_pGameEventManager,
                        SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_FireEventPre), false);
            g_S2ScriptPlugin.m_eventHookInstalled = true;
        } else if (!enable && g_S2ScriptPlugin.m_eventHookInstalled) {
            SH_REMOVE_HOOK(IGameEventManager2, FireEvent, s_pGameEventManager,
                           SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_FireEventPre), false);
            g_S2ScriptPlugin.m_eventHookInstalled = false;
        }
        return;
    }
}
```

Add `bool m_eventHookInstalled = false;` to `S2ScriptPlugin` (next to `m_frameHookInstalled`). Also `SH_REMOVE` it in the plugin `Unload()` teardown alongside the GameFrame removal (guard on `m_eventHookInstalled`).

- [ ] **Step 4: Implement the 7 write/fire op impls + wire them**

With the event op impls (near `s2_event_get_int`). The setters write `s_currentEvent`; `CKV3MemberName` keys the field (as the getters do):

```cpp
static void s2_event_set_int(const char* k, int v)          { if (s_currentEvent && k) s_currentEvent->SetInt(CKV3MemberName(k), v); }
static void s2_event_set_float(const char* k, float v)      { if (s_currentEvent && k) s_currentEvent->SetFloat(CKV3MemberName(k), v); }
static void s2_event_set_bool(const char* k, int v)         { if (s_currentEvent && k) s_currentEvent->SetBool(CKV3MemberName(k), v != 0); }
static void s2_event_set_string(const char* k, const char* v){ if (s_currentEvent && k) s_currentEvent->SetString(CKV3MemberName(k), v ? v : ""); }
static void s2_event_set_uint64(const char* k, uint64_t v)  { if (s_currentEvent && k) s_currentEvent->SetUint64(CKV3MemberName(k), v); }

static int s2_event_create(const char* name) {
    if (!s_pGameEventManager || !name) return 0;
    IGameEvent* e = s_pGameEventManager->CreateEvent(name, /*bForce=*/true);
    if (!e) return 0;
    s_savedCurrentEvent = s_currentEvent;   // save (nest: a fire inside a pre-hook)
    s_pendingFireEvent  = e;
    s_currentEvent      = e;                 // retarget set* to the created event
    return 1;
}
static int s2_event_fire(int dontBroadcast) {
    if (!s_pGameEventManager || !s_pendingFireEvent) return 0;
    IGameEvent* e = s_pendingFireEvent;
    s_pendingFireEvent = nullptr;
    s_currentEvent = s_savedCurrentEvent;    // restore the write target
    s_savedCurrentEvent = nullptr;
    // FireEvent flows through our own Hook_FireEventPre (SM parity: fired events are hookable).
    return s_pGameEventManager->FireEvent(e, dontBroadcast != 0) ? 1 : 0;
}
```

Wire into the `S2EngineOps ops = {}` table (after `ops.client_find_by_userid = ...`), in ABI order:

```cpp
    ops.event_set_int    = &s2_event_set_int;
    ops.event_set_float  = &s2_event_set_float;
    ops.event_set_bool   = &s2_event_set_bool;
    ops.event_set_string = &s2_event_set_string;
    ops.event_set_uint64 = &s2_event_set_uint64;
    ops.event_create     = &s2_event_create;
    ops.event_fire       = &s2_event_fire;
```

- [ ] **Step 5: Verify (shim compile deferred to Task 5)**

The shim compiles only in the sniper container (Task 5) — do NOT attempt a local shim build. Verify by inspection + the core side:
```bash
cargo test -p s2script-core -- --test-threads=1        # unaffected; still green
grep -c "Hook_FireEventPre" shim/src/s2script_mm.cpp   # expect >= 3 (decl, def, add/remove)
grep -c "ops.event_set_int\|ops.event_fire" shim/src/s2script_mm.cpp   # expect >= 2
```
Expected: core green; greps non-zero.

- [ ] **Step 6: Commit**

```bash
git add shim/src/s2script_mm.cpp
git commit -m "$(printf 'feat(slice5d3): shim FireEvent SourceHook + write/fire op impls\n\nSH_DECL_HOOK2 IGameEventManager2::FireEvent + Hook_FireEventPre: sets s_currentEvent (mutable),\ndispatch_game_event_pre, and on suppress SH_CALLs the original with bDontBroadcast=true + MRES_SUPERCEDE\n(SM parity). The GameEvent request-hook branch installs/removes on s_pGameEventManager (gated on\nEVENT_MUX_PRE emptiness). set*/create/fire op impls write s_currentEvent; create saves+retargets, fire\nrestores (nests). Wired into S2EngineOps in ABI order. Compiles at the Task-5 sniper build.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 4: Typed overlay (`@s2script/events` + eventgen `onPre<K>`/`fire<K>`)

**Files:**
- Modify: `packages/events/index.d.ts`, `packages/cli/src/eventgen/emit-dts.ts`, `packages/cs2/events.generated.d.ts` (regen)
- Create/Modify: a `packages/cli/test/*.mjs` vm/emit test

**Interfaces:**
- Consumes: the Task-2 runtime (`Events.onPre`/`fire`, `GameEvent` setters, `HookResult`); the existing `GameEvents` map + `Events.on<K>` overload the eventgen already emits.
- Produces: author-time types for the new surface.

- [ ] **Step 1: Extend `packages/events/index.d.ts` (engine-generic, untyped-key forms + HookResult)**

Add to `GameEvent` (setters), and to `Events` (`onPre`/`fire`), and export `HookResult`:

```typescript
export declare class GameEvent {
  // ... existing getters ...
  setInt(key: string, value: number): void;
  setFloat(key: string, value: number): void;
  setBool(key: string, value: boolean): void;
  setString(key: string, value: string): void;
  /** Set a 64-bit field from a decimal string (SM-parity, wire-safe). */
  setUint64(key: string, value: string): void;
}

/** Collapsed pre-hook result. Return from an `onPre` handler; `Handled`/`Stop` suppress the client
 *  broadcast (server still processes). Returning nothing = `Continue`. */
export declare const HookResult: { readonly Continue: 0; readonly Changed: 1; readonly Handled: 2; readonly Stop: 3 };
export type HookResultValue = 0 | 1 | 2 | 3;

export declare const Events: {
  on(name: string, handler: (ev: GameEvent) => void): void;
  off(name: string, handler: (ev: GameEvent) => void): void;
  /** Pre-hook: runs before the event broadcasts; may read+modify `ev` and return a HookResult to block. */
  onPre(name: string, handler: (ev: GameEvent) => HookResultValue | void): void;
  /** Fire a game event. Returns the engine FireEvent result. */
  fire(name: string, fields?: Record<string, number | string | boolean | bigint>, dontBroadcast?: boolean): boolean;
};
```

- [ ] **Step 2: Write the failing eventgen emit test**

In the eventgen test file (`packages/cli/test/eventgen.test.mjs` — match its existing structure), assert the generated `.d.ts` now contains a typed `onPre<K>` overload and a typed `fire<K>`:

```javascript
test("eventgen emits typed onPre<K> + fire<K>", () => {
  const out = emitDts(MODEL);   // use the same emit entrypoint the existing tests call
  assert.match(out, /onPre<K extends keyof GameEvents>/);
  assert.match(out, /fire<K extends keyof GameEvents>/);
});
```

Note to implementer: read `packages/cli/test/eventgen.test.mjs` + `packages/cli/src/eventgen/emit-dts.ts` first; reuse the exact model fixture + emit function name the existing `Events.on<K>` test uses. Run it to see it FAIL (`onPre` not emitted).

- [ ] **Step 3: Emit `onPre<K>` + `fire<K>` in `packages/cli/src/eventgen/emit-dts.ts`**

Where the generator emits the `Events.on<K extends keyof GameEvents>` overload, add the parallel `onPre` (same key-typing, handler returns `HookResultValue | void`) and `fire` (fields typed to the event's fields). The exact typed-field shape mirrors the existing `on<K>`/`GameEvents[K]` machinery — extend it; do not invent a new type model. Emit into the same `@s2script/cs2` `Events` augmentation block.

- [ ] **Step 4: Regenerate + verify the freshness gate**

Run the eventgen (the same command `check-events-generated.sh` runs — read the script) to regenerate `packages/cs2/events.generated.d.ts`, then:
```bash
bash scripts/check-events-generated.sh
```
Expected: PASS (regenerated output committed; `git diff --exit-code` clean).

- [ ] **Step 5: Run the CLI suite + a vm sanity test**

Add a vm test (or extend an existing events vm test) that loads the prelude and asserts `Events.onPre` is a function, `HookResult.Handled === 2`, and `Events.fire` returns `false` with no ops (degrade). Then:
```bash
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
```
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add packages/events/index.d.ts packages/cli/src/eventgen/emit-dts.ts packages/cs2/events.generated.d.ts packages/cli/test/
git commit -m "$(printf 'feat(slice5d3): typed Events.onPre<K> / fire<K> overlay + GameEvent setters + HookResult\n\n@s2script/events gains the setters, onPre, fire, and HookResult; eventgen emit-dts emits typed\nonPre<K>/fire<K> (same key-typing as on<K>) into the @s2script/cs2 overlay; events.generated.d.ts\nregenerated (freshness gate green). vm sanity + emit tests.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 5: Demo + sniper build + live gate + docs

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`

Controller-driven (the sniper build + Docker server are controller ops).

- [ ] **Step 1: Rewrite the demo to exercise block + modify + fire**

```typescript
import { Events, HookResult, Player } from "@s2script/cs2";

export function onLoad(): void {
  console.log("[demo] onLoad (5D.3 event actionability)");

  // POST (notify) — proves the event still fires server-side even when a pre-hook suppresses broadcast.
  Events.on("player_hurt", (ev) => {
    console.log("[demo] POST player_hurt dmg=" + ev.getInt("dmg_health") + " slot=" + ev.getPlayerSlot("userid"));
  });

  // PRE — block (suppress broadcast) + modify: halve the reported damage, then suppress the client broadcast.
  Events.onPre("player_hurt", (ev) => {
    const dmg = ev.getInt("dmg_health");
    ev.setInt("dmg_health", (dmg / 2) | 0);        // modify (observed by the POST handler)
    return HookResult.Handled;                      // suppress client broadcast (SM parity)
  });

  // FIRE — synthesize a custom-ish event on round_start and confirm a POST subscriber receives it.
  Events.on("round_start", () => {
    const ok = Events.fire("player_say", { userid: 0, text: "s2script fired this" });
    console.log("[demo] fired player_say ok=" + ok);
  });
  Events.on("player_say", (ev) => {
    console.log("[demo] POST player_say text=" + ev.getString("text"));
  });
}

export function onUnload(): void { console.log("[demo] onUnload"); }
```

Note to implementer: confirm `player_hurt`/`player_say` field names against `games/cs2/gamedata/event-catalog.json`; if `player_say`/`text` isn't catalogued, use a catalogued event with a settable string/int field. Build with `npx s2script build .` from `examples/demo-plugin`; do NOT hand-edit the `.s2sp`.

- [ ] **Step 2: Controller — one sniper build**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: core + shim build clean (first compile of the Task-3 shim `FireEvent` hook + op impls). Fix inline + rebuild if the shim fails.

- [ ] **Step 3: Controller — redeploy (the 5D.2 recipe) + live gate**

```bash
cp examples/demo-plugin/dist/*.s2sp dist/addons/s2script/plugins/
docker compose -f docker/docker-compose.yml restart cs2       # re-binds mount + keeps gameinfo
# wait past the boot window (poll for "[s2script] interface OK: GameEventManager (sig-scan")
python3 scripts/rcon.py "bot_quota 2" "bot_add" "mp_restartgame 1"
# fire damage between bots (or sv_cheats hurt) to trigger player_hurt; observe the log
```
Expected in the server log: `[demo] POST player_hurt dmg=<halved>` (modify works, and the POST handler still fires → the event fired server-side while the client broadcast was suppressed); `[demo] fired player_say ok=true` + `[demo] POST player_say text=s2script fired this` (fire + re-delivery). Confirm no crash + server ticking; `bot_kick` degrades cleanly.

- [ ] **Step 4: Write the live-gate results + docs**

- Append a "Slice 5D.3 live-gate" section to a spec-findings doc under `docs/superpowers/specs/` with the exact log lines (block/modify observed, fire delivered) + whether the `FireEvent` vtable index proved correct (the one flagged risk).
- README: a "Event actionability (Slice 5D.3)" section (onPre/HookResult/fire, the suppress-broadcast semantic).
- CLAUDE.md `## Current state`: append a 5D.3 paragraph + update `Current focus`.

- [ ] **Step 5: Full verification sweep + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs && cd -
for g in check-nav-generated check-schema-generated check-events-generated check-core-boundary test-boundary-nameleak; do bash scripts/$g.sh >/dev/null 2>&1 && echo "$g PASS" || echo "$g FAIL"; done
```
Expected: core green; CLI green; all 5 gates PASS.

```bash
git add examples/demo-plugin README.md CLAUDE.md docs/superpowers/specs/
git commit -m "$(printf 'feat(slice5d3): live gate PASSED — event block + modify + fire\n\n<fill with the exact live evidence>\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Self-Review notes (author checklist — completed)

- **Spec coverage:** §2.1 block → T1 (pre-mux/collapse) + T2 (onPre native) + T3 (hook/SUPERCEDE); §2.2 modify → T2 (setter natives + prelude) + T3 (set op impls); §2.3 fire → T2 (create/fire natives + prelude) + T3 (create/fire impls, unified `s_currentEvent`); §3 boundary → T1/T2 core-generic + T4 CS2 overlay; §5 degrade → every native/op + tests; §6 risk → T5 live gate; §7 tests → per-task + T5 sweep; §8 tasks → T1–T5.
- **Type consistency:** native names identical across T2 (register) + T3 (ops) + prelude callers: `__s2_event_set_int/float/bool/string/uint64`, `__s2_event_create`, `__s2_event_fire`, `__s2_event_subscribe_pre`/`unsubscribe_pre`. Op field order identical in C header (T2 Step 1) + Rust mirror (T2 Step 2) + shim wiring (T3 Step 4). `HookResult` values `{0,1,2,3}` match the existing global + the Rust enum ordering. Suppress iff `>= Handled` used consistently (T1 dispatch, T3 hook).
- **No placeholders:** every code step carries complete code; the "match the neighbours" notes (HOOK_REQUEST accessor in T2; the teardown site in T2 Step 5; the eventgen emit shape in T4 Step 3; the demo field names in T5) point at concrete existing code the implementer reads.
