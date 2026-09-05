//! The client (player-slot) surface: the lifecycle-event mux and its dispatch, the slot accessors
//! (`valid`/`userid`/`signon`/`name`/`language`/`steamid`/`address`), and the slot actions
//! (`print`/`consolePrint`/`kick`/`command`/`fakeCommand`).
//!
//! Engine-generic: a client is a SLOT, and every engine fact about it arrives through an
//! `S2EngineOps` function pointer. Nothing here knows a specific game.
//!
//! Extracted from `v8host.rs` under the core-stabilization program — see `crate::usermsg` for the
//! shape (a feature owns its state, natives, dispatch and teardown together).
//!
//! # The boundary, which is NOT the `__s2_client_*` prefix
//! `CLIENT_CMD_SUBS`, `__s2_client_command_listen` and `dispatch_command_listeners` live in
//! `crate::commands` despite the shared prefix. They implement `Commands.onClientCommand` — a
//! listener over registered ConCommands whose semantics are defined by contrast with `CONCOMMANDS`
//! ("a matching ConCommand supersedes; a listener observes"). Prefix is a naming accident; the
//! mux it feeds is what decides the owner.

use crate::dispatch::{fan_out, Delivery, Instrument};
use crate::v8host::{
    engine_ops, set_native, subscribe_into, voice_clear_slot,
};
use std::ffi::CString;

thread_local! {
    /// Client-lifecycle subscriber mux: name → per-plugin subscribers, keyed by the lifecycle event
    /// name ("connect"/"putinserver"/"active"/"fullyconnect"/"disconnect"/"settingschanged"). Same
    /// EventMux shape/discipline as `EVENT_MUX`; notify-only (a handler's return is ignored — no
    /// HookResult collapse). The shim's six lifecycle hooks are installed unconditionally at Load, so
    /// there is no per-subscribe engine-op and no engine-op on empty teardown. `remove_by_owner` on
    /// unload; reset on shutdown so a re-init starts empty.
    static CLIENT_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
}

/// `dispatch_client_event` = **bookkeeping** (unconditional, never replayed) + the JS fan-out.
///
/// The split is the deferred-dispatch contract's §6.1 invariant: the breadcrumb player count and the
/// voice slot-reuse hygiene are NOT idempotent, so a replayed dispatch must run the fan-out ONLY.
/// The shim queues `replay_client_event`, never this entry.
pub(crate) fn dispatch_client_event(event: &str, slot: i32) -> Delivery {
    {
        let s = crate::crash::breadcrumb::snapshot();
        match event {
            "putinserver" => crate::crash::breadcrumb::set_players(s.players + 1),
            "disconnect" => crate::crash::breadcrumb::set_players(s.players - 1),
            _ => {}
        }
    }
    // Slot-reuse hygiene for voice hearability, run UNCONDITIONALLY and BEFORE the no-subscriber
    // early return below — a rule must be dropped whether or not any plugin happens to subscribe to
    // "disconnect". The shim clears its own copy in Hook_ClientDisconnect; this drops the policy
    // source of truth so a later recompute (triggered by an unrelated owner) cannot re-push a rule
    // authored about a player who has left.
    if event == "disconnect" {
        voice_clear_slot(slot);
        crate::shared_entity_switch::clear_slot(slot);
    }
    replay_client_event(event, slot)
}

/// The JS half of `dispatch_client_event`, and NOTHING else — no bookkeeping, so it is safe to run
/// a frame late. This is what the shim's deferred queue replays.
///
/// A deferred `"disconnect"` therefore arrives AFTER the shim has cleared the slot: the handler sees
/// `Clients.isValid(slot) === false`, and in principle the slot could already be reused. Accepted —
/// strictly more than today's total drop — and documented in the clients `.d.ts`.
pub(crate) fn replay_client_event(event: &str, slot: i32) -> Delivery {
    // Snapshot — releases the CLIENT_MUX borrow before any JS runs (see `fan_out` §1).
    let snap = CLIENT_MUX.with(|m| m.borrow().snapshot(event));
    fan_out(&snap, &format!("dispatch_client('{}')", event), Instrument::none(), |tc| {
        Some(vec![v8::Integer::new(tc, slot).into()])
    })
}

/// `__s2_client_subscribe(event, handler)` — subscribe a JS fn to a client-lifecycle event name
/// (Clients sub-project). Owner-tracked (mirror `__s2_event_subscribe`); the shim's six lifecycle hooks
/// are installed unconditionally at Load, so there is no per-name engine-op — the "first subscriber"
/// signal is ignored. The handler receives the raw `slot` at dispatch; the `@s2script/clients` JS
/// wrapper builds a `Client` from it. Notify-only (the return is ignored).
fn s2_client_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let event = args.get(0).to_rust_string_lossy(scope);
        // The six lifecycle hooks are installed for the process lifetime — no first-subscriber op.
        let Some((sub_id, _)) = subscribe_into(scope, &args, &CLIENT_MUX, &event, 1) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// Native `__s2_client_valid(slot) -> boolean`. Calls `client_valid`; degrades to false.
fn s2_client_valid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.client_valid else { return };
        rv.set_bool(func(slot) != 0);
    }));
}

/// Native `__s2_client_userid(slot) -> i32`. Calls `client_userid`; degrades to -1.
fn s2_client_userid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.client_userid else { return };
        rv.set_int32(func(slot));
    }));
}

/// Native `__s2_client_signon(slot) -> i32`. Calls `client_signon`; degrades to -1.
fn s2_client_signon(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.client_signon else { return };
        rv.set_int32(func(slot));
    }));
}

/// Native `__s2_client_find_by_userid(userid) -> i32`. Calls `client_find_by_userid`; degrades to -1.
fn s2_client_find_by_userid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let id = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.client_find_by_userid else { return };
        rv.set_int32(func(id));
    }));
}

/// Native `__s2_client_name(slot) -> string | null`. Calls `client_name`; copies the C string now.
fn s2_client_name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.client_name else { return };
        let ptr = func(slot);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_client_language(slot) -> string | null`. Mirrors `s2_client_name` exactly, calling
/// `client_language` (the client's `cl_language` cvar) instead.
fn s2_client_language(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.client_language else { return };
        let ptr = func(slot);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_client_print(slot, msg)` — print `msg` to the chat of the client in `slot`.
/// Degrade: no ops / no op fn → no-op (server console has no chat).
fn s2_client_print(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let msg = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(f) = ops.client_print else { return };
        if let Ok(cmsg) = CString::new(msg) { f(slot, cmsg.as_ptr()); }
    }));
}

/// `__s2_client_steamid(slot) -> string` — the client's SteamID64 as a decimal string; "0" if no op / bot / invalid.
fn s2_client_steamid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let s: String = (|| {
            let ops = engine_ops()?;
            let f = ops.client_steamid?;
            let ptr = f(slot);
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })().unwrap_or_else(|| "0".to_string());
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// `__s2_client_kick(slot, reason)` — disconnect the client in `slot`. No-op without the op / for a bad slot.
fn s2_client_kick(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let reason = if args.length() >= 2 { args.get(1).to_rust_string_lossy(scope) } else { "Kicked by admin".to_string() };
        let Some(ops) = engine_ops() else { return };
        let Some(f) = ops.client_kick else { return };
        if let Ok(creason) = CString::new(reason) { f(slot, creason.as_ptr()); }
    }));
}

/// `__s2_client_console_print(slot, msg)` — print one line to the client's developer console.
/// No-op without the op / for a bad slot / for a bot (shim skips a null-netchannel fake client).
fn s2_client_console_print(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let msg = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = engine_ops() else { return };
        let Some(f) = ops.client_console_print else { return };
        if let Ok(cmsg) = CString::new(msg) { f(slot, cmsg.as_ptr()); }
    }));
}

/// `__s2_client_address(slot) -> string` — the client's IP address ("IP:port"). `""` without the op / for a bot / on null.
fn s2_client_address(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let s: String = (|| {
            let ops = engine_ops()?;
            let f = ops.client_address?;
            let ptr = f(slot);
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })().unwrap_or_default();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// `__s2_server_command(cmd)` — run `cmd` at the server console. No-op without the op / null.
/// Native `__s2_client_command(slot, cmd) -> boolean` — tell the CLIENT to run `cmd`.
/// False when the op is unassigned, the slot is out of range, or `cmd` is empty; the shim also
/// guards, so this is belt-and-braces plus an honest return value.
fn s2_client_command(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 2 { return; }
        let slot = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        if !(0..64).contains(&slot) { return; }
        let cmd = args.get(1).to_rust_string_lossy(scope);
        if cmd.is_empty() { return; }
        let Some(ops) = engine_ops() else { return };
        let Some(f) = ops.client_command else { return };
        let Ok(ccmd) = CString::new(cmd) else { return };
        rv.set_bool(crate::nest::with_outbound(&args, || f(slot, ccmd.as_ptr())) != 0);
    }));
}

/// Native `__s2_client_fake_command(slot, cmd) -> boolean` — have the SERVER process `cmd` as if
/// this client sent it. False when the op is unassigned, the slot is out of range, `cmd` is empty,
/// or tokenization rejected it.
fn s2_client_fake_command(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 2 { return; }
        let slot = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        if !(0..64).contains(&slot) { return; }
        let cmd = args.get(1).to_rust_string_lossy(scope);
        if cmd.is_empty() { return; }
        let Some(ops) = engine_ops() else { return };
        let Some(f) = ops.client_fake_command else { return };
        let Ok(ccmd) = CString::new(cmd) else { return };
        rv.set_bool(crate::nest::with_outbound(&args, || f(slot, ccmd.as_ptr())) != 0);
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_client_valid", s2_client_valid);
    set_native(scope, global_obj, "__s2_client_userid", s2_client_userid);
    set_native(scope, global_obj, "__s2_client_signon", s2_client_signon);
    set_native(scope, global_obj, "__s2_client_name", s2_client_name);
    set_native(scope, global_obj, "__s2_client_find_by_userid", s2_client_find_by_userid);
    set_native(scope, global_obj, "__s2_client_language", s2_client_language);
    set_native(scope, global_obj, "__s2_client_print", s2_client_print);
    set_native(scope, global_obj, "__s2_client_subscribe", s2_client_subscribe);
    set_native(scope, global_obj, "__s2_client_steamid", s2_client_steamid);
    set_native(scope, global_obj, "__s2_client_kick", s2_client_kick);
    set_native(scope, global_obj, "__s2_client_console_print", s2_client_console_print);
    set_native(scope, global_obj, "__s2_client_command", s2_client_command);
    set_native(scope, global_obj, "__s2_client_fake_command", s2_client_fake_command);
    set_native(scope, global_obj, "__s2_client_address", s2_client_address);
}

/// The owner-scoped store: the six lifecycle hooks stay installed for the process lifetime — no
/// engine-op follow-up on an emptied name.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "CLIENT_MUX",
        Box::new(|owner| { CLIENT_MUX.with(|m| { m.borrow_mut().remove_by_owner(owner); }); }),
        Box::new(|ids| { CLIENT_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| { CLIENT_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new()); }),
    );
}

// Per-feature tests over the SHARED in-isolate harness (`v8host::frame_tests`) — see `crate::usermsg`.
//
// `client_print_and_chat_degrade_without_ops` stays: it is a `@s2script/chat` prelude test that
// happens to touch `__s2_client_print`. Command/listener tests live in `crate::commands`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, load_body, logger,
        mock_client_command, mock_client_fake_command, mock_event_ops, read_i32_global_in, CLIENT_CMD_CALLS,
        FAKE_CMD_CALLS, LOG};
    use crate::v8host::{create_plugin_context, eval_in_context, init, set_engine_ops, shutdown,
        unload_plugin, S2EngineOps};
    use std::os::raw::c_char;
    /// Slice 5D.2: the five engine-identity client natives degrade safely with no engine-ops table
    /// (no crash — false/-1/null as documented).
    #[test]
    fn client_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops table → every client op is a safe miss
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_valid(0))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_userid(0))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_signon(0))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_name(0))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_find_by_userid(5))"), "-1");
        shutdown();
    }

    /// Clients sub-project Task 1: the `@s2script/clients` prelude exposes the `Client` class + the
    /// `Clients` namespace (6 lifecycle `on*` + `fromSlot`/`all`).  With no engine-ops table wired,
    /// `__s2_client_valid` degrades false → `fromSlot(0)` is null and `all()` is empty; `Client.isBot`
    /// derives from `steamId === "0"` (the no-ops steamid degrade), so a bare `new Client(0)` is a bot.
    #[test]
    fn clients_prelude_exposes_client_and_clients_namespace() {
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops → __s2_client_valid degrades false, steamid "0"
        create_plugin_context("pcl");
        assert_eq!(eval_in_context_string("pcl", "typeof globalThis.__s2pkg_clients"), "object");
        assert_eq!(eval_in_context_string("pcl", "typeof __s2pkg_clients.Client"), "function");
        // All 6 lifecycle subscribers + the two enumerators are present as functions.
        for m in ["onConnect", "onPutInServer", "onActive", "onFullyConnect", "onDisconnect",
                  "onSettingsChanged", "fromSlot", "all"] {
            assert_eq!(
                eval_in_context_string("pcl", &format!("typeof __s2pkg_clients.Clients.{}", m)),
                "function", "Clients.{} must be a function", m);
        }
        // No engine → an empty slot: fromSlot(0) is null, all() is [].
        assert_eq!(eval_in_context_string("pcl", "String(__s2pkg_clients.Clients.fromSlot(0))"), "null");
        assert_eq!(eval_in_context_string("pcl", "String(__s2pkg_clients.Clients.all().length)"), "0");
        // A Client is slot-backed; isBot derives from steamId === "0" (no-ops steamid → "0" → bot).
        assert_eq!(eval_in_context_string("pcl", "String(new __s2pkg_clients.Client(3).slot)"), "3");
        assert_eq!(eval_in_context_string("pcl", "String(new __s2pkg_clients.Client(3).isBot)"), "true");
        shutdown();
    }

    /// Clients sub-project Task 1: a subscribed `onConnect` handler receives a `Client` whose `.slot`
    /// equals the dispatched slot (the `CLIENT_MUX` reuse + the JS wrapper's `new Client(slot)`);
    /// a different event name (`"active"`) is independent (does NOT run the connect handler); and after
    /// `unload_plugin` (remove_by_owner teardown) further dispatches are a safe no-op.
    #[test]
    fn client_dispatch_delivers_client_with_slot() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_body("pcl", r#"
            __s2pkg_clients.Clients.onConnect(function (c) {
                globalThis.__cl_ran  = (globalThis.__cl_ran || 0) + 1;
                globalThis.__cl_slot = c.slot;
                globalThis.__cl_ctor = (c instanceof __s2pkg_clients.Client) ? 1 : 0;
            });
            __s2pkg_clients.Clients.onActive(function (c) {
                globalThis.__cl_active_slot = c.slot;
            });
        "#, "{}");

        // Dispatch "connect" slot 3 → the connect handler runs once and receives a Client(3).
        let _ = dispatch_client_event("connect", 3);
        assert_eq!(read_i32_global_in("pcl", "__cl_ran"), 1, "connect handler must run exactly once");
        assert_eq!(read_i32_global_in("pcl", "__cl_slot"), 3, "handler must receive the dispatched slot");
        assert_eq!(read_i32_global_in("pcl", "__cl_ctor"), 1, "the argument must be a Client instance");

        // Independence: dispatching "active" must not re-run the connect handler.
        let _ = dispatch_client_event("active", 5);
        assert_eq!(read_i32_global_in("pcl", "__cl_ran"), 1, "connect handler must not run for 'active'");
        assert_eq!(read_i32_global_in("pcl", "__cl_active_slot"), 5, "the active handler receives its own slot");

        // Teardown: unload removes all of pcl's client subs; a later dispatch is a safe no-op.
        unload_plugin("pcl");
        let _ = dispatch_client_event("connect", 9);   // must not crash / must not deliver (context disposed)
        shutdown();
    }

    fn client_cmd_test_ops() -> S2EngineOps {
        S2EngineOps {
            client_command: Some(mock_client_command),
            client_fake_command: Some(mock_client_fake_command),
            ..mock_event_ops()
        }
    }

    /// fakeCommand reaches its own op with the slot and text verbatim — and is a DIFFERENT op from
    /// command(), which is the whole point (server-side vs client-side).
    #[test]
    fn fake_command_passes_slot_and_text_verbatim() {
        let _ = init(dummy_logger());
        FAKE_CMD_CALLS.lock().unwrap().clear();
        CLIENT_CMD_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(client_cmd_test_ops()));
        create_plugin_context("fc1");
        let out = eval_in_context_string("fc1",
            r#"var c = new __s2pkg_clients.Client(5); String(c.fakeCommand('sm_ban \"some guy\" 60'))"#);
        assert_eq!(out, "true");
        assert_eq!(FAKE_CMD_CALLS.lock().unwrap().as_slice(),
            &[(5, "sm_ban \"some guy\" 60".to_string())]);
        assert!(CLIENT_CMD_CALLS.lock().unwrap().is_empty(), "must NOT route through client_command");
        shutdown();
    }

    /// Same degrade contract as command(): refuse rather than silently no-op.
    #[test]
    fn fake_command_refuses_bad_slot_and_empty_text() {
        let _ = init(dummy_logger());
        FAKE_CMD_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(client_cmd_test_ops()));
        create_plugin_context("fc2");
        for expr in [r#"new __s2pkg_clients.Client(64).fakeCommand("x")"#,
                     r#"new __s2pkg_clients.Client(-1).fakeCommand("x")"#,
                     r#"new __s2pkg_clients.Client(0).fakeCommand("")"#] {
            assert_eq!(eval_in_context_string("fc2", &format!("String({expr})")), "false", "{expr}");
        }
        assert!(FAKE_CMD_CALLS.lock().unwrap().is_empty());
        shutdown();
    }

    /// The slot and the command text reach the op verbatim — including a '%', which must survive as
    /// literal text (the shim passes it as a "%s" argument, never as the format string).
    #[test]
    fn client_command_passes_slot_and_text_verbatim() {
        let _ = init(dummy_logger());
        CLIENT_CMD_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(client_cmd_test_ops()));
        create_plugin_context("cc1");
        // `new Client(slot)` rather than Clients.fromSlot: the test isolate has no engine client
        // state, so fromSlot resolves to null. Same construction the voice-mute test uses.
        let out = eval_in_context_string("cc1",
            r#"var c = new __s2pkg_clients.Client(3); String(c.command("say 100% done"))"#);
        assert_eq!(out, "true");
        assert_eq!(CLIENT_CMD_CALLS.lock().unwrap().as_slice(),
            &[(3, "say 100% done".to_string())]);
        shutdown();
    }

    /// An empty command is refused before the op — dispatching "" would be a no-op the caller
    /// could not distinguish from success.
    #[test]
    fn client_command_refuses_empty_text() {
        let _ = init(dummy_logger());
        CLIENT_CMD_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(client_cmd_test_ops()));
        create_plugin_context("cc3");
        let out = eval_in_context_string("cc3",
            r#"var c = new __s2pkg_clients.Client(0); String(c.command(""))"#);
        assert_eq!(out, "false");
        assert!(CLIENT_CMD_CALLS.lock().unwrap().is_empty());
        shutdown();
    }

    /// A slot outside [0,64) never reaches the op. The engine indexes a fixed 64-slot array, so a
    /// bad slot is an out-of-bounds read on the far side of the FFI — refuse before the call.
    #[test]
    fn client_command_refuses_out_of_range_slot() {
        let _ = init(dummy_logger());
        CLIENT_CMD_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(client_cmd_test_ops()));
        create_plugin_context("cc2");
        for slot in ["-1", "64", "9999"] {
            let out = eval_in_context_string("cc2",
                &format!(r#"var c = new __s2pkg_clients.Client({slot}); String(c.command("sm_help"))"#));
            assert_eq!(out, "false", "slot {slot} should have been refused");
        }
        assert!(CLIENT_CMD_CALLS.lock().unwrap().is_empty(), "no out-of-range slot may reach the op");
        shutdown();
    }

    /// With no op wired (old shim) both report false rather than pretending they dispatched.
    #[test]
    fn client_command_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(mock_event_ops()));   // client_command stays None
        create_plugin_context("cc4");
        assert_eq!(eval_in_context_string("cc4",
            r#"var c = new __s2pkg_clients.Client(0); String(c.command("sm_help"))"#), "false");
        shutdown();
    }

    /// Ban-reason sub-project 2: the console-print + client-address natives degrade cleanly
    /// with no engine ops wired — `__s2_client_console_print` is a no-op (never throws) and
    /// `__s2_client_address` returns "" (an empty string, never null).
    #[test]
    fn client_console_print_and_address_degrade_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // console-print no-ops (returns undefined, never throws) without the op.
        assert_eq!(eval_in_context_string("p", "String(__s2_client_console_print(0, 'x'))"), "undefined");
        // address returns "" (empty string, NOT null) without the op.
        assert_eq!(eval_in_context_string("p", "__s2_client_address(0)"), "");
        assert_eq!(eval_in_context_string("p", "typeof __s2_client_address(0)"), "string");
        shutdown();
    }

    /// Ban-reason sub-project 2: the `@s2script/clients` prelude exposes `Client.prototype.print`,
    /// the `ip` getter, and `Client.prototype.kickWithReason` on the module surface.  With no engine
    /// ops, `print` is a no-op (returns undefined), `ip` returns "" (address degrade → ""), and
    /// `kickWithReason` is a callable function.  Also verifies the ":port" strip logic via a faked
    /// `__s2_client_address` in-isolate.
    #[test]
    fn clients_prelude_exposes_print_ip_and_kick_with_reason() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        set_engine_ops(None);   // no ops → __s2_client_address returns ""
        create_plugin_context("pcl2");
        // print is a function on the prototype.
        assert_eq!(eval_in_context_string("pcl2", "typeof __s2pkg_clients.Client.prototype.print"), "function");
        // kickWithReason is a function on the prototype.
        assert_eq!(eval_in_context_string("pcl2", "typeof __s2pkg_clients.Client.prototype.kickWithReason"), "function");
        // ip getter: no engine → address "" → ip "".
        assert_eq!(eval_in_context_string("pcl2", "new __s2pkg_clients.Client(0).ip"), "");
        // print is a no-op without the op (returns undefined, never throws).
        assert_eq!(eval_in_context_string("pcl2", "String(new __s2pkg_clients.Client(0).print('hello'))"), "undefined");
        // ":port" strip logic: fake __s2_client_address then check the getter strips correctly.
        assert_eq!(
            eval_in_context_string("pcl2",
                "(function () { \
                    var orig = globalThis.__s2_client_address; \
                    globalThis.__s2_client_address = function () { return \"1.2.3.4:27005\"; }; \
                    var ip = new __s2pkg_clients.Client(0).ip; \
                    globalThis.__s2_client_address = orig; \
                    return ip; \
                }())"),
            "1.2.3.4");
        // address with no colon returns the value unchanged.
        assert_eq!(
            eval_in_context_string("pcl2",
                "(function () { \
                    var orig = globalThis.__s2_client_address; \
                    globalThis.__s2_client_address = function () { return \"1.2.3.4\"; }; \
                    var ip = new __s2pkg_clients.Client(0).ip; \
                    globalThis.__s2_client_address = orig; \
                    return ip; \
                }())"),
            "1.2.3.4");
        shutdown();
    }
}
