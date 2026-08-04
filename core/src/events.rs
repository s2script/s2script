//! Game events: the `Events.on` / `Events.onPre` muxes, the field get/set natives over the shim's
//! current IGameEvent, event creation + firing (including per-client), and this feature's teardown.
//!
//! Two muxes, deliberately separate. POST (`EVENT_MUX`) is notify-only and lazily subscribed
//! per-name via the `event_subscribe` engine-op, so an emptied name calls `event_unsubscribe`. PRE
//! (`EVENT_MUX_PRE`) is a collapsing hook chain behind ONE global FireEvent hook, so its teardown
//! reconciles whole-mux-empty rather than per-name.
//!
//! Engine-generic: event names and field keys are the caller's strings, and every engine fact
//! arrives through an `S2EngineOps` function pointer. Nothing here knows a specific game — the
//! 272-event typed catalog lives in the game package.
//!
//! Extracted from `v8host.rs` under the core-stabilization program — see `crate::usermsg` for the
//! shape (a feature owns its state, natives, dispatch and teardown together).

use crate::multiplexer::HookResult;
use crate::v8host::{
    current_plugin, engine_ops, fan_out, fan_out_collapsing, hook_request, set_native,
    subscribe_into, Delivery, Instrument, StopAt,
};
use std::ffi::{CStr, CString};
use std::os::raw::c_int;

thread_local! {
    /// POST-hook subscribers: event name → per-plugin subscribers. Notify-only (a handler's return is
    /// ignored). Lazily subscribed at the engine level on a name's first subscriber via the
    /// `event_subscribe` op; an emptied name calls `event_unsubscribe`. `remove_by_owner` on unload;
    /// reset on shutdown so a re-init starts empty.
    static EVENT_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    /// PRE-hook subscribers, same keying. Collapsing dispatch (max-by-precedence) — a collapsed result
    /// >= Handled(2) tells the shim to suppress the event. These run behind ONE global FireEvent hook
    /// rather than a per-name engine subscription, so teardown reconciles on WHOLE-MUX-empty
    /// (`reconcile_pre_hook`), not per emptied name.
    static EVENT_MUX_PRE: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    /// One-shot recipient mask for the NEXT `Events.fireToClient`, set by `__s2_event_set_recipients`
    /// and consumed by `take_event_recipients` (which replaces it with `None`). Transient by design:
    /// it is written and read within a single fire.
    static EVENT_RECIPIENTS: std::cell::Cell<Option<u64>> = std::cell::Cell::new(None);
}

/// Take (and clear) the recipient allow-mask set by `Events.setRecipients` during the current
/// game-event pre-dispatch. `None` when the plugin set nothing.
pub(crate) fn take_event_recipients() -> Option<u64> {
    EVENT_RECIPIENTS.with(|c| c.replace(None))
}

/// Native `__s2_event_subscribe(name, handler)` — subscribe a JS function to a named game event.
/// On first subscriber for a name, calls the `event_subscribe` engine-op (null-degrade).
/// The subscription is tagged with the calling plugin's (id, generation) for liveness-gated dispatch.
fn s2_event_subscribe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Some((sub_id, first)) = subscribe_into(scope, &args, &EVENT_MUX, &name, 1) else { return };
        if first {
            if let Some(ops) = engine_ops() {
                if let Some(func) = ops.event_subscribe {
                    if let Ok(cn) = CString::new(name.as_str()) {
                        func(cn.as_ptr());
                    }
                }
            }
        }
        // T3: return the sub id so ctx/scope's `viaId` can track it for Scope disposal.
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// Native `__s2_event_unsubscribe(name, handler)` — unsubscribe the calling plugin from a named event.
/// Removes ALL of the calling plugin's subs for `name` (handler identity match not required —
/// mirrors iface_off's best-effort approach; callers rarely double-subscribe).
/// If the name's subscriber list is now empty, calls `event_unsubscribe` engine-op (null-degrade).
fn s2_event_unsubscribe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());

        let emptied = EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner_on(&name, &owner));
        if emptied {
            if let Some(ops) = engine_ops() {
                if let Some(func) = ops.event_unsubscribe {
                    if let Ok(cn) = CString::new(name.as_str()) {
                        func(cn.as_ptr());
                    }
                }
            }
        }
    }));
}

/// Native `__s2_event_get_int(key) -> i32`. Calls the `event_get_int` engine-op; degrades to 0.
fn s2_event_get_int(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_get_int else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_int32(func(ck.as_ptr()));
    }));
}

/// Native `__s2_event_get_float(key) -> f64`. Calls the `event_get_float` engine-op; degrades to 0.0.
fn s2_event_get_float(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_get_float else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_double(func(ck.as_ptr()) as f64);
    }));
}

/// `__s2_event_set_recipients(slots)` — restrict the event currently being pre-dispatched to `slots`.
///
/// An empty array means "no client sees it". Slots outside [0, 64) are ignored rather than throwing:
/// this runs inside an event pre-hook, where a throw would lose the whole dispatch.
fn s2_event_set_recipients(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(0)) else { return };
        let mut mask: u64 = 0;
        for i in 0..arr.length() {
            if let Some(v) = arr.get_index(scope, i) {
                let n = v.integer_value(scope).unwrap_or(-1);
                if (0..64).contains(&n) { mask |= 1u64 << (n as u32); }
            }
        }
        EVENT_RECIPIENTS.with(|c| c.set(Some(mask)));
    }));
}

/// Native `__s2_event_get_bool(key) -> boolean`. Calls the `event_get_bool` engine-op; degrades to false.
fn s2_event_get_bool(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_get_bool else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_bool(func(ck.as_ptr()) != 0);
    }));
}

/// Native `__s2_event_get_string(key) -> string`. Calls the `event_get_string` engine-op; degrades to "".
/// The returned C string pointer is ONLY valid during the shim's event dispatch — copy it NOW.
fn s2_event_get_string(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(empty) = v8::String::new(scope, "") { rv.set(empty.into()); }
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_get_string else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        let ptr = func(ck.as_ptr());
        if ptr.is_null() { return; }
        // Copy the C string immediately — the pointer is only valid during the dispatch call.
        let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_event_get_uint64(key) -> string`. Calls `event_get_uint64`; returns a DECIMAL STRING.
/// uint64 → decimal string so JS can handle it without BigInt precision loss.
fn s2_event_get_uint64(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(zero) = v8::String::new(scope, "0") { rv.set(zero.into()); }
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_get_uint64 else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        let val = func(ck.as_ptr());
        let s = format!("{}", val);
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_event_get_player_slot(key) -> i32`. Calls `event_get_player_slot`; degrades to -1.
fn s2_event_get_player_slot(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_get_player_slot else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_int32(func(ck.as_ptr()));
    }));
}

/// Native `__s2_event_subscribe_pre(name, handler)` — register a pre-hook (can block/modify).
fn s2_event_subscribe_pre(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        // WHOLE-STORE emptiness, not per-channel: the global FireEvent hook covers every event name,
        // so it installs on the first pre-sub across ALL names. Sampled BEFORE subscribing.
        let was_empty = EVENT_MUX_PRE.with(|m| m.borrow().is_empty());
        let Some((sub_id, _)) = subscribe_into(scope, &args, &EVENT_MUX_PRE, &name, 1) else { return };
        if was_empty {
            if let Some(req) = hook_request() {
                if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 1); }
            }
        }
        rv.set(v8::Number::new(scope, sub_id as f64).into());
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
            if let Some(req) = hook_request() {
                if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 0); }
            }
        }
    }));
}

/// Native `__s2_event_set_int(key, value)` — write the current event's int field (pre-hook / fire builder).
fn s2_event_set_int(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).int32_value(scope).unwrap_or(0);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_set_int else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_set_float(key, value)` — write the current event's float field.
fn s2_event_set_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).number_value(scope).unwrap_or(0.0) as f32;
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_set_float else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_set_bool(key, value)` — write the current event's bool field (0/1).
fn s2_event_set_bool(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).boolean_value(scope) as c_int;
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_set_bool else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_set_string(key, value)` — write the current event's string field.
fn s2_event_set_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let val_str = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_set_string else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        let Ok(cv) = CString::new(val_str.as_str()) else { return };
        func(ck.as_ptr(), cv.as_ptr());
    }));
}

/// Native `__s2_event_set_uint64(key, value)` — write the current event's uint64 field.
/// `value` is a DECIMAL STRING at the JS boundary (consistent with getUint64/readUInt64).
fn s2_event_set_uint64(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let val_str = args.get(1).to_rust_string_lossy(scope);
        let value = val_str.parse::<u64>().unwrap_or(0);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_set_uint64 else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_create(name) -> boolean` — create a to-be-fired event; retargets set* writes to it.
fn s2_event_create(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
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
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_fire else { return };
        rv.set_bool(func(dont as c_int) != 0);
    }));
}

/// Native `__s2_event_fire_to_client(slot) -> boolean` — fire the created event to ONE client's
/// per-client listener (serialized to that netchannel; does NOT pass through IGameEventManager2::FireEvent,
/// so no pre-hook / dispatch re-entrancy). Returns false on any miss.
fn s2_event_fire_to_client(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.event_fire_to_client else { return };
        rv.set_bool(func(slot) != 0);
    }));
}

/// Dispatch a game event by name to all LIVE JS subscribers.
///
/// Called from `ffi.rs`'s `s2script_core_dispatch_game_event` (C-ABI export), which the shim's
/// `IGameEventListener2` callback invokes when an event fires (the shim has already stashed the
/// live `IGameEvent*` for the accessor engine-ops).
///
/// **Re-entrancy discipline:** snapshot before invoke — release `EVENT_MUX` borrow, then enter
/// each subscriber's PLUGIN context in its own HandleScope+ContextScope+TryCatch.  `EVENT_MUX` and
/// `PLUGINS`/`REGISTRY` are NOT held across any JS call.
///
/// This path carries NO bookkeeping, so `dispatch` and `replay` are the same work; both names exist
/// so the shim's queue has one uniform `replay_*` vocabulary (contract §6.1).
pub(crate) fn dispatch_game_event(name: &str) -> Delivery {
    replay_game_event(name)
}

/// The JS half of `dispatch_game_event` — the entry the shim's deferred queue replays.
///
/// This is the ONE deferrable entry whose payload is not scalars: the handlers read the event's
/// fields through the shim's `s_currentEvent` accessor ops, and the engine's `IGameEvent` is valid
/// only for the duration of the original call. The shim therefore `DuplicateEvent`s it, points
/// `s_currentEvent` at the copy for this call, and `FreeEvent`s it after. Nothing about an
/// `IGameEvent` is ever represented in Rust — no raw pointer is queued on this side.
pub(crate) fn replay_game_event(name: &str) -> Delivery {
    // Snapshot — releases the EVENT_MUX borrow before any JS runs, so `Events.fire()` from inside a
    // handler cannot mutate the list being walked (a re-entrant dispatch reports `Deferred` and is
    // replayed by the shim next frame; the engine-side fire has already happened).
    let snap = EVENT_MUX.with(|m| m.borrow().snapshot(name));
    let tag = format!("event:{}", name);
    fan_out(&snap, &format!("dispatch_game_event('{}')", name), Instrument::full(&tag), |tc| {
        // Construct new GameEvent(name) from globalThis.__s2pkg_events.GameEvent. A failure here
        // yields `undefined` rather than skipping the subscriber, matching the pre-fan_out behaviour.
        let event_val: Option<v8::Local<v8::Value>> = (|| {
            let global = tc.get_current_context().global(tc);
            let pkg_key = v8::String::new(tc, "__s2pkg_events")?;
            let pkg = global.get(tc, pkg_key.into())?;
            let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
            let ctor_key = v8::String::new(tc, "GameEvent")?;
            let ctor_val = pkg.get(tc, ctor_key.into())?;
            let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
            let name_str = v8::String::new(tc, name)?;
            ctor.new_instance(tc, &[name_str.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
        })();
        Some(vec![event_val.unwrap_or_else(|| v8::undefined(tc).into())])
    })
}

/// Pre-dispatch for the FireEvent hook (Slice 5D.3). Runs the PRE subscribers for `name`, collapses
/// their HookResults via `run_chain`, and returns 1 to suppress client broadcast (collapsed result
/// >= Handled) or 0 to allow. The shim has set `s_currentEvent` (mutable) before calling this.
pub(crate) fn dispatch_game_event_pre(name: &str) -> i32 {
    let snap = EVENT_MUX_PRE.with(|m| m.borrow().snapshot(name));
    let result = fan_out_collapsing(&snap, &format!("dispatch_game_event_pre('{}')", name), Instrument::none(), StopAt::Stop, |tc| {
        // Construct new GameEvent(name) from globalThis.__s2pkg_events.GameEvent (as in dispatch_game_event).
        let event_arg: Option<v8::Local<v8::Value>> = (|| {
            let global = tc.get_current_context().global(tc);
            let pkg_key = v8::String::new(tc, "__s2pkg_events")?;
            let pkg = global.get(tc, pkg_key.into())?;
            let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
            let ctor_key = v8::String::new(tc, "GameEvent")?;
            let ctor = v8::Local::<v8::Function>::try_from(pkg.get(tc, ctor_key.into())?).ok()?;
            let name_str = v8::String::new(tc, name)?;
            ctor.new_instance(tc, &[name_str.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
        })();
        let event_val: v8::Local<v8::Value> = event_arg.unwrap_or_else(|| v8::undefined(tc).into());
        Some(vec![event_val])
    });
    if result >= HookResult::Handled { 1 } else { 0 }
}

/// For each name that became empty in an EVENT_MUX teardown, call the engine-op event_unsubscribe so
/// the shim can deregister at the engine level.
fn unsubscribe_emptied_events(emptied: &[String]) {
    for evname in emptied {
        if let Some(ops) = engine_ops() {
            if let Some(func) = ops.event_unsubscribe {
                if let Ok(cn) = CString::new(evname.as_str()) { func(cn.as_ptr()); }
            }
        }
    }
}

/// If the PRE game-event mux is now entirely empty (no more pre-hooks), request removal of the global
/// FireEvent hook.
fn reconcile_pre_hook() {
    if EVENT_MUX_PRE.with(|m| m.borrow().is_empty()) {
        if let Some(req) = hook_request() {
            if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 0); }
        }
    }
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_event_subscribe", s2_event_subscribe);
    set_native(scope, global_obj, "__s2_event_unsubscribe", s2_event_unsubscribe);
    set_native(scope, global_obj, "__s2_event_get_int", s2_event_get_int);
    set_native(scope, global_obj, "__s2_event_get_float", s2_event_get_float);
    set_native(scope, global_obj, "__s2_event_get_bool", s2_event_get_bool);
    set_native(scope, global_obj, "__s2_event_get_string", s2_event_get_string);
    set_native(scope, global_obj, "__s2_event_get_uint64", s2_event_get_uint64);
    set_native(scope, global_obj, "__s2_event_get_player_slot", s2_event_get_player_slot);
    set_native(scope, global_obj, "__s2_event_subscribe_pre", s2_event_subscribe_pre);
    set_native(scope, global_obj, "__s2_event_unsubscribe_pre", s2_event_unsubscribe_pre);
    set_native(scope, global_obj, "__s2_event_set_int", s2_event_set_int);
    set_native(scope, global_obj, "__s2_event_set_float", s2_event_set_float);
    set_native(scope, global_obj, "__s2_event_set_bool", s2_event_set_bool);
    set_native(scope, global_obj, "__s2_event_set_string", s2_event_set_string);
    set_native(scope, global_obj, "__s2_event_set_uint64", s2_event_set_uint64);
    set_native(scope, global_obj, "__s2_event_create", s2_event_create);
    set_native(scope, global_obj, "__s2_event_fire", s2_event_fire);
    set_native(scope, global_obj, "__s2_event_fire_to_client", s2_event_fire_to_client);
    set_native(scope, global_obj, "__s2_event_set_recipients", s2_event_set_recipients);
}

/// The two owner-scoped stores. They tear down DIFFERENTLY and that asymmetry is the point: POST is
/// per-name engine subscription, PRE is one global hook.
pub(crate) fn register_stores() {
    // EVENT_MUX: emptied names → engine-op event_unsubscribe.
    crate::owner_stores::register(
        "EVENT_MUX",
        Box::new(|owner| {
            let emptied = EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(owner));
            unsubscribe_emptied_events(&emptied);
        }),
        Box::new(|ids| {
            let emptied = EVENT_MUX.with(|m| m.borrow_mut().remove_by_ids(ids));
            unsubscribe_emptied_events(&emptied);
        }),
        Box::new(|| { EVENT_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new()); }),
    );
    // EVENT_MUX_PRE: whole-mux-empty → remove the global GameEvent hook.
    crate::owner_stores::register(
        "EVENT_MUX_PRE",
        Box::new(|owner| {
            EVENT_MUX_PRE.with(|m| { m.borrow_mut().remove_by_owner(owner); });
            reconcile_pre_hook();
        }),
        Box::new(|ids| {
            EVENT_MUX_PRE.with(|m| { m.borrow_mut().remove_by_ids(ids); });
            reconcile_pre_hook();
        }),
        Box::new(|| { EVENT_MUX_PRE.with(|m| *m.borrow_mut() = crate::channels::Channels::new()); }),
    );
}

/// Reset on a core re-init. `AfterIsolateDrop`: a plain `Cell<Option<u64>>`, no V8 handles.
///
/// This slot had NO teardown registration before this module existed. It is transient — written by
/// `__s2_event_set_recipients` and consumed by the very next `fireToClient` — so the window is
/// narrow, but a mask set and never fired would have survived into the next init and applied to a
/// fire that never asked for it. Registering it is the same rule every other slot follows.
pub(crate) fn register_singletons() {
    crate::process_singletons::register(
        "EVENT_RECIPIENTS", crate::process_singletons::ResetPhase::AfterIsolateDrop,
        Box::new(|| EVENT_RECIPIENTS.with(|c| c.set(None))),
    );
}

/// How many POST subscribers a name currently has. The LIFECYCLE tests use this as their observable
/// for "did the buffered registration arm?", so it stays a supported read rather than those tests
/// reaching into the mux.
#[cfg(test)]
pub(crate) fn subscriber_count(name: &str) -> usize {
    EVENT_MUX.with(|m| m.borrow().snapshot(name).len())
}

// Per-feature tests over the SHARED in-isolate harness (`v8host::frame_tests`) — see `crate::usermsg`.
//
// The DEFERRED-QUEUE tests and the L1 LIFECYCLE tests stay in `v8host.rs`: they drive game-event
// dispatch as their vehicle but assert on the defer queue and the load state machine respectively.
// The lifecycle ones read `subscriber_count` above rather than reaching into the mux.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, load_body,
        mock_event_ops, read_bool_global_in, read_global_string, read_i32_global_in, with_host_borrowed,
        EV_SUBSCRIBED, EV_UNSUBSCRIBED};
    use crate::v8host::{create_plugin_context, eval_in_context, init, set_engine_ops, shutdown,
        unload_plugin};
    /// Slice 5D.1 Task 1 (core): game-event subscribe/dispatch/accessor/teardown integration.
    ///
    /// - A plugin subscribes to "player_death" via the raw `__s2_event_subscribe` native.
    /// - The first subscriber triggers the `event_subscribe` engine-op.
    /// - `dispatch_game_event("player_death")` delivers a `new GameEvent("player_death")` to the
    ///   handler; the handler reads all six accessor methods and stores the results.
    /// - Dispatching an unsubscribed name does NOT call the handler.
    /// - `unload_plugin` calls `remove_by_owner` on the mux and triggers `event_unsubscribe`.
    /// - After unload, further dispatches are no-ops (no crash, no delivery).
    #[test]
    fn game_event_dispatch_subscribe_accessor_and_teardown() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        // Plugin subscribes to player_death using the raw __s2_event_subscribe native.
        load_body("pev", r#"
            __s2_event_subscribe("player_death", function(ev) {
                globalThis.__ev_ran    = (globalThis.__ev_ran || 0) + 1;
                globalThis.__ev_name   = ev.name;
                globalThis.__ev_int    = ev.getInt("damage");
                globalThis.__ev_str    = ev.getString("victim");
                globalThis.__ev_slot   = ev.getPlayerSlot("attacker");
                globalThis.__ev_uint64 = ev.getUint64("steamid");
                globalThis.__ev_bool   = ev.getBool("headshot");
            });
        "#, "{}");

        // First subscribe must trigger the engine-op.
        assert!(
            EV_SUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_death"),
            "event_subscribe engine-op must fire on the first subscriber"
        );

        // Dispatch player_death → handler runs and reads mocked accessor values.
        let _ = dispatch_game_event("player_death");
        assert_eq!(read_i32_global_in("pev", "__ev_ran"),  1,             "handler must run exactly once");
        assert_eq!(read_global_string("pev", "__ev_name"), "player_death","GameEvent.name must be set");
        assert_eq!(read_i32_global_in("pev", "__ev_int"),  42,            "getInt() returns mock 42");
        assert_eq!(read_global_string("pev", "__ev_str"),  "mocked_string","getString() returns mock");
        assert_eq!(read_i32_global_in("pev", "__ev_slot"), 7,             "getPlayerSlot() returns mock 7");
        assert_eq!(read_global_string("pev", "__ev_uint64"), "999000000000","getUint64() returns decimal string");
        assert_eq!(read_bool_global_in("pev", "__ev_bool"), true,         "getBool() returns mock true");

        // Dispatching a different name must NOT call the handler.
        let _ = dispatch_game_event("round_start");
        assert_eq!(read_i32_global_in("pev", "__ev_ran"), 1,
                   "handler must not run for unsubscribed event name");

        // Teardown: unload_plugin removes all of "pev"'s game-event subs; the name empties →
        // event_unsubscribe engine-op fires.
        unload_plugin("pev");
        assert!(
            EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_death"),
            "event_unsubscribe engine-op must fire when the last subscriber unloads"
        );

        // After unload, dispatching must be a safe no-op (no crash, no delivery).
        let _ = dispatch_game_event("player_death");
        // (No count assertion — the context is disposed; just confirm no panic.)

        shutdown();
    }

    /// Slice 5D.1: a second plugin subscribes to the same event → event_subscribe engine-op fires
    /// only ONCE (on the first subscriber); event_unsubscribe fires only when the LAST subscriber
    /// unloads.  Both handlers receive the event; after one unloads only the other still runs.
    #[test]
    fn game_event_two_subscribers_first_last_semantics() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        load_body("p1", r#"
            __s2_event_subscribe("player_hurt", function(ev) {
                globalThis.__p1_ran = (globalThis.__p1_ran || 0) + 1;
            });
        "#, "{}");
        // First subscriber → engine-op called once.
        assert_eq!(EV_SUBSCRIBED.lock().unwrap().iter().filter(|n| *n == "player_hurt").count(), 1);

        load_body("p2", r#"
            __s2_event_subscribe("player_hurt", function(ev) {
                globalThis.__p2_ran = (globalThis.__p2_ran || 0) + 1;
            });
        "#, "{}");
        // Second subscriber → engine-op NOT called again (not first).
        assert_eq!(EV_SUBSCRIBED.lock().unwrap().iter().filter(|n| *n == "player_hurt").count(), 1,
                   "event_subscribe must only fire on the FIRST subscriber");

        let _ = dispatch_game_event("player_hurt");
        assert_eq!(read_i32_global_in("p1", "__p1_ran"), 1);
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 1);

        // Unload p1: p2 still subscribed → event_unsubscribe must NOT fire yet.
        unload_plugin("p1");
        assert!(!EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_hurt"),
                "event_unsubscribe must NOT fire while p2 is still subscribed");

        // Dispatch again → only p2 runs.
        let _ = dispatch_game_event("player_hurt");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 2);

        // Unload p2: last subscriber → event_unsubscribe fires.
        unload_plugin("p2");
        assert!(EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_hurt"),
                "event_unsubscribe must fire when the last subscriber unloads");

        shutdown();
    }

    /// Slice 5D.1 Task 1: explicit `__s2_event_unsubscribe` path — single subscriber.
    ///
    /// After subscribing and then explicitly calling `__s2_event_unsubscribe("test_event")`,
    /// the handler must NOT be called on the next dispatch, AND the `event_unsubscribe`
    /// engine-op must fire (the name became empty).
    #[test]
    fn game_event_explicit_unsubscribe_removes_handler_and_fires_engine_op() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        // Plugin subscribes then immediately unsubscribes from the same module body.
        load_body("pev", r#"
            globalThis.__ev_ran = 0;
            __s2_event_subscribe("test_event", function(ev) {
                globalThis.__ev_ran = (globalThis.__ev_ran || 0) + 1;
            });
            __s2_event_unsubscribe("test_event");
        "#, "{}");

        // event_subscribe engine-op must have fired (on the first/only subscribe).
        assert!(
            EV_SUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_subscribe engine-op must fire on subscribe"
        );

        // event_unsubscribe engine-op must have fired (name is now empty after unsubscribe).
        assert!(
            EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_unsubscribe engine-op must fire when last subscriber explicitly unsubscribes"
        );

        // Dispatch must NOT invoke the removed handler.
        let _ = dispatch_game_event("test_event");
        assert_eq!(
            read_i32_global_in("pev", "__ev_ran"), 0,
            "handler must not run after explicit unsubscribe"
        );

        shutdown();
    }

    /// Slice 5D.1 Task 1: explicit `__s2_event_unsubscribe` with two plugins.
    ///
    /// Unsubscribing the first plugin removes only its subs; the second plugin still receives
    /// the event.  The `event_unsubscribe` engine-op fires only when the LAST subscriber
    /// explicitly unsubscribes (the name is then empty).
    #[test]
    fn game_event_explicit_unsubscribe_two_plugins_last_fires_op() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        load_body("p1", r#"
            globalThis.__p1_ran = 0;
            __s2_event_subscribe("test_event", function(ev) {
                globalThis.__p1_ran = (globalThis.__p1_ran || 0) + 1;
            });
        "#, "{}");
        load_body("p2", r#"
            globalThis.__p2_ran = 0;
            __s2_event_subscribe("test_event", function(ev) {
                globalThis.__p2_ran = (globalThis.__p2_ran || 0) + 1;
            });
        "#, "{}");

        // Confirm both handlers run on dispatch before any explicit unsubscribe.
        let _ = dispatch_game_event("test_event");
        assert_eq!(read_i32_global_in("p1", "__p1_ran"), 1, "p1 handler must run before unsubscribe");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 1, "p2 handler must run before unsubscribe");

        // p1 explicitly unsubscribes: p2 still subscribed → event_unsubscribe must NOT fire yet.
        eval_in_context("p1", r#"__s2_event_unsubscribe("test_event");"#).unwrap();
        assert!(
            !EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_unsubscribe must NOT fire while p2 is still subscribed"
        );

        // Dispatch: only p2 runs; p1's handler must be gone.
        let _ = dispatch_game_event("test_event");
        assert_eq!(read_i32_global_in("p1", "__p1_ran"), 1, "p1 must not run after explicit unsubscribe");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 2, "p2 must still receive the event");

        // p2 explicitly unsubscribes: last subscriber → event_unsubscribe engine-op must now fire.
        eval_in_context("p2", r#"__s2_event_unsubscribe("test_event");"#).unwrap();
        assert!(
            EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_unsubscribe must fire when the last subscriber explicitly unsubscribes"
        );

        // After both unsubscribed, dispatch is a safe no-op.
        let _ = dispatch_game_event("test_event");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 2, "p2 must not run after explicit unsubscribe");

        shutdown();
    }

    /// Slice 5D.1 Task 2: `@s2script/events` `Events.on/off` — end-to-end in-isolate test.
    ///
    /// - Subscribe via `require("@s2script/events").Events.on` (the prelude module object, NOT the raw native).
    /// - `dispatch_game_event("player_death")` delivers a `GameEvent`; the handler reads `ev.name`
    ///   + `ev.getInt("attacker")` (mock returns 42) and stores `"player_death:42"` in `__saw`.
    /// - `Events.off("player_death", handler)` removes all of the plugin's subs for the name;
    ///   subsequent dispatch must NOT change `__saw`.
    #[test]
    fn events_module_events_on_off_drives_dispatch_via_pkg() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        load_body("ep", r#"
            var evMod = require("@s2script/events");
            var handler = function(ev) {
                globalThis.__saw = ev.name + ":" + ev.getInt("attacker");
            };
            evMod.Events.on("player_death", handler);
        "#, "{}");

        // Events.on must delegate to __s2_event_subscribe → engine-op must have fired.
        assert!(
            EV_SUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_death"),
            "Events.on must trigger the event_subscribe engine-op"
        );

        // Dispatch → handler runs; ev.name="player_death", ev.getInt → mock 42.
        let _ = dispatch_game_event("player_death");
        assert_eq!(
            read_global_string("ep", "__saw"),
            "player_death:42",
            "Events.on handler must receive a GameEvent with correct name and getInt"
        );

        // Events.off removes all of the plugin's subs for the name; handler identity not checked.
        // Use globalThis.__s2_require directly — eval_in_context has no CJS `require` alias.
        eval_in_context("ep", r#"
            globalThis.__s2_require("@s2script/events").Events.off("player_death", null);
        "#).unwrap();

        // Dispatch again → __saw must remain unchanged (handler must NOT run).
        let saw_before = read_global_string("ep", "__saw");
        let _ = dispatch_game_event("player_death");
        assert_eq!(
            read_global_string("ep", "__saw"),
            saw_before,
            "After Events.off, dispatch must not invoke the handler"
        );

        shutdown();
    }

    /// Slice 5D.3 Task 1: `dispatch_game_event_pre` with no subscribers returns 0 (allow).
    #[test]
    fn dispatch_game_event_pre_no_subs_allows() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        assert_eq!(dispatch_game_event_pre("player_hurt"), 0);   // no pre-subs → allow
        shutdown();
    }

    /// Slice 5D.3 Task 2: pre-hook collapse + setter/create/fire degrade with no engine ops.
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

    /// Slice 5D.3: Events.fire() from inside a handler re-enters the dispatch while the isolate is
    /// already borrowed. The re-entrancy guard must SKIP the nested JS dispatch gracefully (no
    /// "RefCell already borrowed" panic), allowing the event through (pre → 0/allow; notify → no-op).
    #[test]
    fn reentrant_dispatch_skips_gracefully_no_panic() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // A pre-hook that WOULD suppress, and a notify sub — both must be skipped under re-entrancy.
        eval_in_context("p", r#"__s2_event_subscribe_pre("player_hurt", function(ev){ return HookResult.Handled; });"#).unwrap();
        eval_in_context("p", r#"__s2_event_subscribe("player_hurt", function(ev){ globalThis.__notified = true; });"#).unwrap();
        // Hold the HOST borrow (simulating an outer dispatch), then re-enter:
        let pre = with_host_borrowed(|| dispatch_game_event_pre("player_hurt"));  // 0 (allow), not a panic
        assert_eq!(pre, 0, "re-entrant pre-dispatch allows instead of double-borrow panic");
        with_host_borrowed(|| { let _ = dispatch_game_event("player_hurt"); });   // nested notify skipped
        shutdown();
    }
}
