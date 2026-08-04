//! UserMessage interception (usermsg-hook slice) — the `UserMessages.onPre` mux natives, the
//! block-scoped view read natives, the dispatch entry, and this feature's own teardown registrations.
//!
//! Subscribe resolves + validates + lazily-installs shim-side (`usermsg_hook_sub`); the read natives
//! target the shim's current intercepted message (null-guarded, valid only during a dispatch). All
//! engine-generic: message NAMES and dotted field PATHS are the caller's strings, so nothing here
//! knows a specific game.
//!
//! # Why this is its own module
//! This is the first feature extracted from `v8host.rs` under the core-stabilization program, and it
//! sets the shape the rest follow. The rule is that a feature owns FOUR things and they live together:
//!
//!   1. its thread-local state (the mux + any sidecar caches),
//!   2. its natives, and the `install_natives` call that publishes them,
//!   3. its dispatch entry (called from `ffi.rs` via `v8host`),
//!   4. its teardown — the `owner_stores` and `process_singletons` registrations.
//!
//! Previously those four sat 3,000–5,000 lines apart in one file, ordered by the day each was
//! written. Co-locating them is the whole point: the teardown that clears the shim's bitmap bit is
//! twenty lines from the subscribe that sets it, so the pairing is reviewable in one screen.
//!
//! # What deliberately did NOT move
//! `S2EngineOps` and its `UsermsgHook*Fn` type aliases stay in `v8host.rs`. They are not this
//! feature's code — they are the flat C-ABI contract with the shim, whose FIELD ORDER is the ABI,
//! checked positionally against `shim/include/s2script_core.h` by `scripts/check-engine-ops-order.sh`
//! (which parses both the struct and the aliases out of `v8host.rs`). Splitting that contract across
//! per-feature modules would fragment the one declaration the gate exists to keep whole.

use crate::v8host::{
    current_plugin, engine_ops, fan_out_collapsing, set_native, subscribe_into, Instrument, StopAt,
};
use std::os::raw::c_int;

thread_local! {
    /// `UserMessages.onPre(name, handler)` pre-hook subscribers keyed by the CANONICAL unscoped
    /// message name (the shim's `usermsg_hook_sub` canonicalizes the caller's partial name).
    /// Collapsing dispatch (max-by-precedence, multiplexer.rs rule) — a collapsed result >=
    /// Handled(2) tells the shim to MRES_SUPERCEDE the outbound send. The shim's PostEventAbstract
    /// SourceHook installs LAZILY on the first-ever subscribe (via `usermsg_hook_sub`, which also
    /// sets the id's bitmap bit); on a canonical name's last-empty transition (off, or unload's
    /// `remove_by_owner`) the shim's bitmap bit is cleared via `usermsg_hook_unsub`.
    /// `remove_by_owner` on unload; reset on shutdown so a re-init starts empty.
    static USERMSG_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    /// Canonical name -> engine id (from `usermsg_hook_sub`) so teardown (a name that became empty
    /// on unload) can clear the shim's bitmap bit via `usermsg_hook_unsub`.
    static USERMSG_IDS: std::cell::RefCell<std::collections::HashMap<String, i32>>
        = std::cell::RefCell::new(std::collections::HashMap::new());

    /// The (possibly partial) name a plugin passed to `onPre` -> its canonical unscoped name (and
    /// canonical -> itself). Populated in `s2_usermsg_on` so `s2_usermsg_off` resolves the mux key
    /// WITHOUT calling the installing `usermsg_hook_sub` op — which would lazily install the
    /// PostEventAbstract hook + set a bitmap bit for a never-subscribed name (a permanent hot-path
    /// leak on a no-op `off()`). A name we never subscribed simply misses this map and `off` no-ops.
    static USERMSG_RESOLVE: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// Native `__s2_usermsg_on(name, wrappedHandler) -> canonicalName | null`. Resolves + validates the
/// message name via the `usermsg_hook_sub` op (which lazily installs the shim's PostEventAbstract hook
/// and sets the id's bitmap bit on the first-ever subscribe), records the canonical name → engine id in
/// `USERMSG_IDS`, and subscribes the wrapped handler in `USERMSG_MUX` keyed by the CANONICAL name.
/// Returns the canonical name on success; `null` on a missing op, an unresolvable name, or a degraded
/// intercept descriptor (the op returned -1) — the prelude then throws at subscribe time.
fn s2_usermsg_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        // Validate the handler BEFORE the engine op, so a bad argument costs no usermsg_hook_sub
        // call. `subscribe_into` re-checks it; this guard exists purely to preserve that ordering.
        if v8::Local::<v8::Function>::try_from(args.get(1)).is_err() { return; }
        let nc = match std::ffi::CString::new(name.clone()) { Ok(c) => c, Err(_) => return };
        let Some(sub) = engine_ops().and_then(|o| o.usermsg_hook_sub) else { return };
        let mut buf = [0i8; 256];
        let id = sub(nc.as_ptr(), buf.as_mut_ptr(), buf.len() as c_int);
        if id < 0 { return; }   // unresolvable name / null NetMessageInfo — prelude throws
        let canonical = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
        USERMSG_IDS.with(|m| { m.borrow_mut().insert(canonical.clone(), id); });
        USERMSG_RESOLVE.with(|m| { let mut b = m.borrow_mut();
            b.insert(name, canonical.clone()); b.insert(canonical.clone(), canonical.clone()); });
        // Subscribes under the CANONICAL name — the alias the caller used is only a lookup key.
        if subscribe_into(scope, &args, &USERMSG_MUX, &canonical, 1).is_none() { return; }
        if let Some(js) = v8::String::new(scope, &canonical) { rv.set(js.into()); }
    }));
}

/// Native `__s2_usermsg_off(name)`. Drops the CURRENT plugin's `UserMessages.onPre` subs for `name`
/// (best-effort, mirrors `s2_output_unsubscribe` — V8 Globals can't be compared by identity, so all of
/// the caller's subs for that canonical name go). Resolves the caller's (possibly partial) name to its
/// canonical via the subscribe-time `USERMSG_RESOLVE` cache — NOT the `usermsg_hook_sub` op, whose
/// install/set-bit side effects would permanently arm the hook for a never-subscribed name. A name we
/// never subscribed misses the cache and this is a clean no-op. If the name is now empty across ALL owners
/// the shim's bitmap bit is cleared via `usermsg_hook_unsub`.
fn s2_usermsg_off(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Some(canonical) = USERMSG_RESOLVE.with(|m| m.borrow().get(&name).cloned()) else { return };
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let now_empty = USERMSG_MUX.with(|m| m.borrow_mut().remove_by_owner_on(&canonical, &owner));
        if now_empty {
            if let Some(hook_id) = USERMSG_IDS.with(|m| m.borrow_mut().remove(&canonical)) {
                if let Some(unsub) = engine_ops().and_then(|o| o.usermsg_hook_unsub) {
                    let _ = unsub(hook_id);
                }
            }
        }
    }));
}

/// Native `__s2_usermsg_read_int(path) -> number | null`. Over the `usermsg_hook_read_int` op (int32/
/// uint32/fixed32/enum/bool via reflection; dotted nested paths). rc 0 → `null` (no field / repeated /
/// wrong type / no current message / a 64-bit field — those are refused shim-side per the decimal-string
/// doctrine, spec §10). Every value it returns fits f64 exactly (int32/uint32/fixed32 ≤ 2^32).
fn s2_usermsg_read_int(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let path = args.get(0).to_rust_string_lossy(scope);
        let pc = match std::ffi::CString::new(path) { Ok(c) => c, Err(_) => return };
        let Some(f) = engine_ops().and_then(|o| o.usermsg_hook_read_int) else { return };
        let mut out: i64 = 0;
        if f(pc.as_ptr(), &mut out as *mut i64) != 0 {
            rv.set(v8::Number::new(scope, out as f64).into());
        }
    }));
}

/// Native `__s2_usermsg_read_float(path) -> number | null`. Over `usermsg_hook_read_float` (float/double,
/// dotted paths). rc 0 → `null`.
fn s2_usermsg_read_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let path = args.get(0).to_rust_string_lossy(scope);
        let pc = match std::ffi::CString::new(path) { Ok(c) => c, Err(_) => return };
        let Some(f) = engine_ops().and_then(|o| o.usermsg_hook_read_float) else { return };
        let mut out: f64 = 0.0;
        if f(pc.as_ptr(), &mut out as *mut f64) != 0 {
            rv.set(v8::Number::new(scope, out).into());
        }
    }));
}

/// Native `__s2_usermsg_read_string(path) -> string | null`. Over `usermsg_hook_read_string` (a 4096-byte
/// buffer; the op returns the byte length or -1). -1 → `null`.
fn s2_usermsg_read_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let path = args.get(0).to_rust_string_lossy(scope);
        let pc = match std::ffi::CString::new(path) { Ok(c) => c, Err(_) => return };
        let Some(f) = engine_ops().and_then(|o| o.usermsg_hook_read_string) else { return };
        let mut buf = [0u8; 4096];
        let n = f(pc.as_ptr(), buf.as_mut_ptr() as *mut i8, buf.len() as c_int);
        if n < 0 { return; }
        let len = (n as usize).min(buf.len());
        let s = String::from_utf8_lossy(&buf[..len]).into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_usermsg_has_field(path) -> i32` (1 present / 0 absent-or-invalid / -1 no current
/// message). Over `usermsg_hook_has_field`. Degrades to -1 with no op.
fn s2_usermsg_has_field(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let path = args.get(0).to_rust_string_lossy(scope);
        let pc = match std::ffi::CString::new(path) { Ok(c) => c, Err(_) => return };
        if let Some(f) = engine_ops().and_then(|o| o.usermsg_hook_has_field) {
            rv.set_int32(f(pc.as_ptr()));
        }
    }));
}

/// Native `__s2_usermsg_recipients() -> number[]`. Over `usermsg_hook_recipients` (the recipient mask,
/// broadcast pre-expanded shim-side to all valid slots). Decodes the u64 mask's set bits to a JS array
/// of 0-based slots. rc 0 (no current message) or no op → an empty array (never null).
fn s2_usermsg_recipients(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut mask: u64 = 0;
        let ok = engine_ops().and_then(|o| o.usermsg_hook_recipients)
            .map_or(false, |f| f(&mut mask as *mut u64) != 0);
        let mut slots: Vec<u32> = Vec::new();
        if ok {
            for s in 0..64u32 { if mask & (1u64 << s) != 0 { slots.push(s); } }
        }
        let arr = v8::Array::new(scope, slots.len() as i32);
        for (i, s) in slots.iter().enumerate() {
            let num = v8::Number::new(scope, *s as f64);
            arr.set_index(scope, i as u32, num.into());
        }
        rv.set(arr.into());
    }));
}

/// Native `__s2_usermsg_debug() -> string`. Over `usermsg_hook_debug` (protobuf TextFormat dump into an
/// 8192-byte buffer; the op returns the byte length or -1). -1 / no op → "".
fn s2_usermsg_debug(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(js) = v8::String::new(scope, "") { rv.set(js.into()); }
        let Some(f) = engine_ops().and_then(|o| o.usermsg_hook_debug) else { return };
        let mut buf = [0u8; 8192];
        let n = f(buf.as_mut_ptr() as *mut i8, buf.len() as c_int);
        if n < 0 { return; }
        let len = (n as usize).min(buf.len());
        let s = String::from_utf8_lossy(&buf[..len]).into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Publish this feature's natives onto a fresh context's global object. Called from `v8host`'s
/// `install_natives`, which owns the ordering of every feature's block.
///
/// The `onPre` mux + the block-scoped view read natives (which route through the `usermsg_hook_*` ops;
/// the shim's PostEventAbstract hook installs lazily on the first subscribe).
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_usermsg_on", s2_usermsg_on);
    set_native(scope, global_obj, "__s2_usermsg_off", s2_usermsg_off);
    set_native(scope, global_obj, "__s2_usermsg_read_int", s2_usermsg_read_int);
    set_native(scope, global_obj, "__s2_usermsg_read_float", s2_usermsg_read_float);
    set_native(scope, global_obj, "__s2_usermsg_read_string", s2_usermsg_read_string);
    set_native(scope, global_obj, "__s2_usermsg_has_field", s2_usermsg_has_field);
    set_native(scope, global_obj, "__s2_usermsg_recipients", s2_usermsg_recipients);
    set_native(scope, global_obj, "__s2_usermsg_debug", s2_usermsg_debug);
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch an intercepted user message to the `UserMessages.onPre` subscribers (which read fields via
/// the `usermsg_hook_*` ops from inside each handler). A verbatim copy of `dispatch_output`'s shape
/// (snapshot with the mux borrow released → `try_borrow_mut` graceful-skip → `run_chain` collapse)
/// except the handler receives `(nameString, idNumber)` — the prelude wraps those into a
/// `UserMessageView`. Returns the collapsed `HookResult` (0 Continue .. 3 Stop); the shim
/// MRES_SUPERCEDEs the send when it is >= Handled(2). The `try_borrow_mut` skip guards re-entrancy (a
/// handler that itself sends a user message re-enters PostEventAbstract while the isolate is borrowed —
/// the documented v1 limitation, mirrored by the shim's own `s_inUserMsgDispatch` flag).
pub(crate) fn dispatch(name: &str, id: i32) -> i32 {
    let snap = USERMSG_MUX.with(|m| m.borrow().snapshot(name));
    let result = fan_out_collapsing(&snap, &format!("dispatch_usermsg('{}')", name), Instrument::none(), StopAt::Stop, |tc| {
        // The handler is the prelude wrapper `function (n, id) { return handler(__s2_umView(n, id)); }`
        // — pass the canonical name + id; the view's field reads route through the usermsg_hook_* ops.
        let name_val: v8::Local<v8::Value> = match v8::String::new(tc, name) {
            Some(s) => s.into(), None => v8::null(tc).into(),
        };
        let id_val: v8::Local<v8::Value> = v8::Number::new(tc, id as f64).into();
        Some(vec![name_val, id_val])
    });
    result as i32
}

// ---------------------------------------------------------------------------
// Teardown
// ---------------------------------------------------------------------------

/// For each canonical usermessage name that became empty, remove its `USERMSG_IDS` entry and clear the
/// shim's bitmap bit via `usermsg_hook_unsub`.
fn unhook_emptied(emptied: &[String]) {
    for canonical in emptied {
        if let Some(hook_id) = USERMSG_IDS.with(|m| m.borrow_mut().remove(canonical)) {
            if let Some(func) = engine_ops().and_then(|o| o.usermsg_hook_unsub) {
                let _ = func(hook_id);
            }
        }
    }
}

/// The owner-scoped store: emptied canonical names → clear the shim bitmap bit via `usermsg_hook_unsub`.
/// Called from `v8host::register_builtin_stores`.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "USERMSG_MUX",
        Box::new(|owner| {
            let emptied = USERMSG_MUX.with(|m| m.borrow_mut().remove_by_owner(owner));
            unhook_emptied(&emptied);
        }),
        Box::new(|ids| {
            let emptied = USERMSG_MUX.with(|m| m.borrow_mut().remove_by_ids(ids));
            unhook_emptied(&emptied);
        }),
        Box::new(|| {
            USERMSG_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );
}

/// The name→id resolution caches (the MUX itself is an owner-scoped store, registered above).
/// `AfterIsolateDrop`: plain Rust maps, no V8 handles. Called from
/// `v8host::register_process_singletons`.
pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    crate::process_singletons::register(
        "USERMSG_IDS", AfterIsolateDrop,
        Box::new(|| USERMSG_IDS.with(|m| m.borrow_mut().clear())),
    );
    crate::process_singletons::register(
        "USERMSG_RESOLVE", AfterIsolateDrop,
        Box::new(|| USERMSG_RESOLVE.with(|m| m.borrow_mut().clear())),
    );
}

// ---------------------------------------------------------------------------
// Tests
//
// These drive the SHARED in-isolate harness (`v8host::frame_tests`) — one isolate, one thread, which
// is why `.cargo/config.toml` forces `--test-threads=1`. The harness stays in `v8host`; only the
// per-feature assertions live here. That split is the point of the extraction: a feature's tests
// should move with the feature.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{
        dummy_logger, eval_in_context_string, mock_event_ops, read_global_string, read_i32_global_in,
    };
    use crate::v8host::{
        create_plugin_context, eval_in_context, init, set_engine_ops, shutdown, unload_plugin,
        S2EngineOps,
    };
    use std::os::raw::c_char;

    thread_local! {
        static USERMSG_SUB_CAPTURE:   std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        static USERMSG_UNSUB_CAPTURE: std::cell::RefCell<Vec<i32>>    = std::cell::RefCell::new(Vec::new());
    }
    /// Mock resolver: canonicalizes any name containing "FireBullets" -> "TE_FireBullets_Canonical" id 452
    /// (partial-match shape); "SubFail" (and anything else) -> -1 (degraded/unknown). Records every sub call.
    extern "C" fn mock_usermsg_sub(name: *const c_char, out: *mut c_char, len: c_int) -> c_int {
        let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().to_string();
        USERMSG_SUB_CAPTURE.with(|v| v.borrow_mut().push(n.clone()));
        if n == "SubFail" { return -1; }
        let canonical = if n.contains("FireBullets") { "TE_FireBullets_Canonical" } else { return -1 };
        let bytes = canonical.as_bytes();
        assert!((bytes.len() as c_int) < len);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, bytes.len());
            *out.add(bytes.len()) = 0;
        }
        452
    }
    extern "C" fn mock_usermsg_unsub(id: c_int) -> c_int {
        USERMSG_UNSUB_CAPTURE.with(|v| v.borrow_mut().push(id)); 1
    }
    extern "C" fn mock_usermsg_read_int(path: *const c_char, out: *mut i64) -> c_int {
        let p = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().to_string();
        let v = match p.as_str() { "item_def_index" => 9, "player" => 6390016, "origin.x" => 0, _ => return 0 };
        unsafe { *out = v; } 1
    }
    extern "C" fn mock_usermsg_read_float(path: *const c_char, out: *mut f64) -> c_int {
        let p = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().to_string();
        if p == "origin.x" { unsafe { *out = 128.5; } 1 } else { 0 }
    }
    extern "C" fn mock_usermsg_recipients(out: *mut u64) -> c_int { unsafe { *out = 0b1010; } 1 }

    fn usermsg_test_ops() -> S2EngineOps {
        S2EngineOps {
            usermsg_hook_sub:        Some(mock_usermsg_sub),
            usermsg_hook_unsub:      Some(mock_usermsg_unsub),
            usermsg_hook_read_int:   Some(mock_usermsg_read_int),
            usermsg_hook_read_float: Some(mock_usermsg_read_float),
            usermsg_hook_recipients: Some(mock_usermsg_recipients),
            ..mock_event_ops()
        }
    }

    /// onPre resolves the name through usermsg_hook_sub (canonicalized), and a dispatched message
    /// delivers a view with the canonical name + id; a non-subscribed name does not cross-fire.
    #[test]
    fn usermsg_on_pre_subscribes_and_dispatch_delivers_view() {
        USERMSG_SUB_CAPTURE.with(|v| v.borrow_mut().clear());
        let _ = init(dummy_logger());
        set_engine_ops(Some(usermsg_test_ops()));
        create_plugin_context("pum");
        eval_in_context("pum", r#"
            __s2pkg_usermessages.UserMessages.onPre("FireBullets", function (m) {
                globalThis.__m_ran = (globalThis.__m_ran || 0) + 1;
                globalThis.__m_name = m.name;
                globalThis.__m_id = m.id;
                globalThis.__m_idx = m.readInt("item_def_index");
            });
        "#).unwrap();

        assert!(USERMSG_SUB_CAPTURE.with(|v| v.borrow().iter().any(|s| s == "FireBullets")),
            "onPre must resolve through usermsg_hook_sub with the caller's name");

        assert_eq!(dispatch("TE_FireBullets_Canonical", 452), 0, "no-suppress handler -> Continue");
        assert_eq!(read_i32_global_in("pum", "__m_ran"), 1);
        assert_eq!(read_global_string("pum", "__m_name"), "TE_FireBullets_Canonical", "view carries canonical name");
        assert_eq!(read_i32_global_in("pum", "__m_id"), 452);
        assert_eq!(read_i32_global_in("pum", "__m_idx"), 9, "readInt routes through the read op");

        // A non-subscribed canonical name must not cross-fire.
        assert_eq!(dispatch("SomethingElse", 7), 0);
        assert_eq!(read_i32_global_in("pum", "__m_ran"), 1, "unsubscribed name must not run the handler");
        shutdown();
    }

    /// The HookResult collapse: Handled(2) from any handler suppresses (dispatch returns 2); a
    /// Continue-only chain returns 0 (max-by-precedence across handlers).
    #[test]
    fn usermsg_dispatch_collapses_hook_results() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(usermsg_test_ops()));
        create_plugin_context("pa");
        create_plugin_context("pb");
        eval_in_context("pa", r#"
            __s2pkg_usermessages.UserMessages.onPre("FireBullets", function (m) { return HookResult.Handled; });
        "#).unwrap();
        eval_in_context("pb", r#"
            __s2pkg_usermessages.UserMessages.onPre("FireBullets", function (m) { return HookResult.Continue; });
        "#).unwrap();

        assert_eq!(dispatch("TE_FireBullets_Canonical", 452), 2,
            "one Handled + one Continue collapses to Handled(2) -> suppress");

        // With the Handled subscriber's plugin unloaded, only the Continue subscriber remains -> 0.
        unload_plugin("pa");
        assert_eq!(dispatch("TE_FireBullets_Canonical", 452), 0,
            "Continue-only chain collapses to Continue(0)");
        shutdown();
    }

    /// Degrade: usermsg_hook_sub returning -1 makes onPre THROW at subscribe (plugin-load-loud);
    /// with NO engine ops at all it also throws (never a silent dead subscription).
    #[test]
    fn usermsg_on_pre_throws_on_unresolvable_or_degraded() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(usermsg_test_ops()));
        create_plugin_context("pt");
        let degraded = eval_in_context_string("pt", r#"
            try { __s2pkg_usermessages.UserMessages.onPre('SubFail', function () {}); 'no-throw' }
            catch (e) { 'threw' }
        "#);
        assert_eq!(degraded, "threw", "an unresolvable (-1) name must throw at subscribe");

        set_engine_ops(None);
        create_plugin_context("pt2");
        let no_op = eval_in_context_string("pt2", r#"
            try { __s2pkg_usermessages.UserMessages.onPre('FireBullets', function () {}); 'no-throw' }
            catch (e) { 'threw' }
        "#);
        assert_eq!(no_op, "threw", "no engine op at all must throw (never a silent dead subscription)");
        shutdown();
    }

    /// View reads route through the read ops; a missing field reads null (not 0/undefined-crash);
    /// readFloat("origin.x") proves dotted paths cross the ABI verbatim; recipients decode the mask.
    #[test]
    fn usermsg_view_reads_route_through_ops_and_null_on_miss() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(usermsg_test_ops()));
        create_plugin_context("pv");
        eval_in_context("pv", r#"
            __s2pkg_usermessages.UserMessages.onPre("FireBullets", function (m) {
                globalThis.__m_result =
                    String(m.readFloat("origin.x")) + "|" +
                    String(m.readInt("nope")) + "|" +
                    JSON.stringify(m.recipients) + "|" +
                    String(m.readInt("player"));
            });
        "#).unwrap();
        assert_eq!(dispatch("TE_FireBullets_Canonical", 452), 0);
        assert_eq!(read_global_string("pv", "__m_result"), "128.5|null|[1,3]|6390016",
            "dotted read + null-on-miss + recipients-mask-decode + top-level fixed32 read");
        shutdown();
    }

    /// Teardown is the ledger's job: unloading the only subscriber plugin calls usermsg_hook_unsub with
    /// the id from sub time; while a second subscriber remains, the bit stays (no unsub until last-empty).
    #[test]
    fn usermsg_unload_unsubscribes_on_last_empty() {
        USERMSG_UNSUB_CAPTURE.with(|v| v.borrow_mut().clear());
        let _ = init(dummy_logger());
        set_engine_ops(Some(usermsg_test_ops()));
        create_plugin_context("pu_a");
        create_plugin_context("pu_b");
        eval_in_context("pu_a", r#"__s2pkg_usermessages.UserMessages.onPre("FireBullets", function () {});"#).unwrap();
        eval_in_context("pu_b", r#"__s2pkg_usermessages.UserMessages.onPre("FireBullets", function () {});"#).unwrap();

        unload_plugin("pu_a");
        assert!(USERMSG_UNSUB_CAPTURE.with(|v| v.borrow().is_empty()),
            "one subscriber still hooks the name -> no unsub");

        unload_plugin("pu_b");
        assert_eq!(USERMSG_UNSUB_CAPTURE.with(|v| v.borrow().clone()), vec![452],
            "last subscriber gone -> usermsg_hook_unsub(452)");
        shutdown();
    }
}
