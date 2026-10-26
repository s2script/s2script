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

// Independent connection books; clearing does not reset the generation allocator.
thread_local! {
    static CONNECTIONS: std::cell::RefCell<crate::liveness::LiveTable<i32, ()>> =
        std::cell::RefCell::new(crate::liveness::LiveTable::new(1));
}

pub(crate) fn generation(slot: i32) -> u64 {
    CONNECTIONS.with(|b| b.borrow().get(&slot).map_or(0, |(id, _)| id))
}
pub(crate) fn matches(slot: i32, token: u64) -> bool {
    token != 0 && CONNECTIONS.with(|b| b.borrow().is_live(&slot, token))
}
pub(crate) fn begin(slot: i32) -> u64 {
    if !(0..64).contains(&slot) { return 0; }
    end(slot, generation(slot));
    CONNECTIONS.with(|b| b.borrow_mut().insert(slot, ()))
}
pub(crate) fn ensure(slot: i32) -> u64 {
    let token = generation(slot);
    if token != 0 { token } else { begin(slot) }
}
pub(crate) fn end(slot: i32, token: u64) {
    if !matches(slot, token) { return; }
    CONNECTIONS.with(|b| { b.borrow_mut().remove(&slot); });
    cancel_convar_queries(slot);
    voice_clear_slot(slot);
    crate::surface_leases::clear_client(slot, token);
    crate::shared_entity_switch::clear_slot(slot);
    crate::cookies::retire(slot, token);
}

#[repr(C)]
pub struct ClientIdentity {
    pub user_id: i32,
    pub signon: i32,
    pub steam_id: *const std::os::raw::c_char,
    pub name: *const std::os::raw::c_char,
    pub address: *const std::os::raw::c_char,
}
impl ClientIdentity {
    pub(crate) fn owned(&self) -> serde_json::Value {
        fn copy(p: *const std::os::raw::c_char, fallback: &str) -> String {
            if p.is_null() { fallback.into() } else { unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned() }
        }
        serde_json::json!({"userId": self.user_id, "signonState": self.signon,
            "steamId": copy(self.steam_id, "0"), "name": copy(self.name, ""), "address": copy(self.address, "")})
    }
}
fn snapshot(slot: i32) -> serde_json::Value {
    let ops = engine_ops();
    // Engine strings can share scratch storage. Copy each result before the next engine call.
    fn copy(ptr: *const std::os::raw::c_char, fallback: &str) -> String {
        if ptr.is_null() { fallback.into() } else { unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned() }
    }
    let steam_id = copy(ops.and_then(|o| o.client_steamid).map_or(std::ptr::null(), |f| f(slot)), "0");
    let name = copy(ops.and_then(|o| o.client_name).map_or(std::ptr::null(), |f| f(slot)), "");
    let address = copy(ops.and_then(|o| o.client_address).map_or(std::ptr::null(), |f| f(slot)), "");
    serde_json::json!({"steamId": steam_id, "name": name, "address": address,
        "userId": ops.and_then(|o| o.client_userid).map_or(-1, |f| f(slot)),
        "signonState": ops.and_then(|o| o.client_signon).map_or(-1, |f| f(slot))})
}

/// Legacy immediate delivery can still identify the connection at the call boundary.
pub(crate) fn dispatch_client_event(event: &str, slot: i32) -> Delivery {
    let token = match event {
        "connect" => begin(slot),
        "putinserver" | "active" | "fullyconnect" => ensure(slot),
        _ => generation(slot),
    };
    let identity = (event == "disconnect").then(|| snapshot(slot));
    dispatch_client_event_v2(event, slot, token, identity.as_ref())
}
pub(crate) fn dispatch_client_event_v2(event: &str, slot: i32, token: u64, identity: Option<&serde_json::Value>) -> Delivery {
    let s = crate::crash::breadcrumb::snapshot();
    match event {
        "putinserver" => crate::crash::breadcrumb::set_players(s.players + 1),
        "disconnect" => { crate::crash::breadcrumb::set_players(s.players - 1); end(slot, token); }
        _ => {}
    }
    replay_client_event_v2(event, slot, token, identity)
}
/// Slot-only replay cannot prove which occupant produced it. Fail closed.
pub(crate) fn replay_client_event(_event: &str, _slot: i32) -> Delivery {
    crate::v8host::log_warn("WARN: client lifecycle: dropped legacy slot-only replay (shim/core mismatch)");
    Delivery::Delivered
}
pub(crate) fn replay_client_event_v2(event: &str, slot: i32, token: u64, identity: Option<&serde_json::Value>) -> Delivery {
    let snap = CLIENT_MUX.with(|m| m.borrow().snapshot(event));
    fan_out(&snap, &format!("dispatch_client('{}')", event), Instrument::none(), |tc| {
        if token == 0 || (event != "disconnect" && !matches(slot, token)) { return None; }
        let token = v8::String::new(tc, &token.to_string())?;
        let data = v8::String::new(tc, &identity.map_or("null".into(), |s| s.to_string()))?;
        Some(vec![v8::Integer::new(tc, slot).into(), token.into(), data.into()])
    })
}

/// Optional final token preserves raw slot natives, while Client calls are connection-gated.
pub(crate) fn guarded(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, slot: i32, index: i32) -> bool {
    args.length() <= index || matches(slot, args.get(index).to_rust_string_lossy(scope).parse().unwrap_or(0))
}
fn s2_client_generation(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let slot = args.get(0).int32_value(scope).unwrap_or(-1);
    let token = generation(slot).to_string();
    if let Some(s) = v8::String::new(scope, &token) { rv.set(s.into()); }
}
fn s2_client_matches(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let slot = args.get(0).int32_value(scope).unwrap_or(-1);
    let token = args.get(1).to_rust_string_lossy(scope).parse().unwrap_or(0);
    rv.set_bool(matches(slot, token));
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
        if !guarded(scope, &args, slot, 1) { return; }
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
        if !guarded(scope, &args, slot, 1) { return; }
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
        if !guarded(scope, &args, slot, 1) { return; }
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
        if !guarded(scope, &args, slot, 1) { return; }
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
        if !guarded(scope, &args, slot, 1) { return; }
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
        if !guarded(scope, &args, slot, 2) { return; }
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
            if !guarded(scope, &args, slot, 1) { return None; }
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
        if !guarded(scope, &args, slot, 2) { return; }
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
        if !guarded(scope, &args, slot, 2) { return; }
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
            if !guarded(scope, &args, slot, 1) { return None; }
            let ops = engine_ops()?;
            let f = ops.client_address?;
            let ptr = f(slot);
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })().unwrap_or_default();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Values `client_net_stats` writes, in this order. Flow pairs are (outgoing, incoming), the
/// engine's FLOW_OUTGOING = 0 / FLOW_INCOMING = 1. The prelude's `Client.netStats` reads them by
/// these indices, so the two lists change together.
pub(crate) const NET_STATS_LEN: usize = 18;

/// `__s2_client_net_stats(slot, token) -> number[] | null` — one snapshot of the client's
/// INetChannelInfo counters ([`NET_STATS_LEN`] numbers). `null` without the op, for a stale handle,
/// for a bot (no net channel), or when the shim's net-channel layout check failed.
fn s2_client_net_stats(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        if !(0..64).contains(&slot) { return; }
        if !guarded(scope, &args, slot, 1) { return; }
        let Some(ops) = engine_ops() else { return };
        let Some(f) = ops.client_net_stats else { return };
        let mut out = [0f64; NET_STATS_LEN];
        if f(slot, out.as_mut_ptr(), NET_STATS_LEN as i32) != NET_STATS_LEN as i32 { return; }
        let elems: Vec<v8::Local<v8::Value>> = out.iter().map(|v| v8::Number::new(scope, *v).into()).collect();
        rv.set(v8::Array::new_with_elements(scope, &elems).into());
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
        if !guarded(scope, &args, slot, 2) { return; }
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
        if !guarded(scope, &args, slot, 2) { return; }
        let Some(ops) = engine_ops() else { return };
        let Some(f) = ops.client_fake_command else { return };
        let Ok(ccmd) = CString::new(cmd) else { return };
        rv.set_bool(crate::nest::with_outbound(&args, || f(slot, ccmd.as_ptr())) != 0);
    }));
}

// ---------------------------------------------------------------------------
// Client convar queries. `queryConVar` sends CSVCMsg_GetCvarValue with a host-minted cookie; the
// client's answer comes back through the shim's cvar-response hook, queued and drained at the top of
// the next frame. Each pending query is one subscription on CONVAR_QUERIES, keyed by its cookie, so
// owner teardown (plugin unload, scope dispose) drops it like any other subscription.
//
// A reply is delivered only when the slot that answered is the slot the query went to, that slot is
// still the same connection, and the echoed name matches. Cookies are unpredictable, so another
// client cannot answer someone else's query either. A disconnect resolves its pending queries with
// null.
// ---------------------------------------------------------------------------
/// What a query settles with. Kept on the entry when the isolate was busy, so the next frame's sweep
/// retries the delivery instead of losing it.
#[derive(Clone)]
enum ConvarOutcome { Cancelled, Answered { status: i32, value: String } }

struct ConvarQuery {
    slot: i32, token: u64, name: String, owner: String, sub_id: u64,
    deadline: std::time::Instant,
    outcome: Option<ConvarOutcome>,
}

thread_local! {
    static CONVAR_QUERIES: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    static CONVAR_PENDING: std::cell::RefCell<std::collections::HashMap<i32, ConvarQuery>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static COOKIE_SEED: (std::collections::hash_map::RandomState, std::cell::Cell<u64>)
        = (std::collections::hash_map::RandomState::new(), std::cell::Cell::new(0));
}

/// Pending queries across every plugin. A client always answers, so this only fills when queries
/// are issued far faster than clients reply.
const MAX_PENDING_CONVAR_QUERIES: usize = 1024;
/// Longer than any real cvar name; the shim sends the name verbatim.
const MAX_CONVAR_NAME: usize = 255;
/// A query with no reply by then resolves null: the reply was lost (the shim drops a flooding
/// client's replies) or the client never sent one.
const CONVAR_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn mint_cookie() -> i32 {
    use std::hash::BuildHasher;
    COOKIE_SEED.with(|(state, counter)| loop {
        let n = counter.get().wrapping_add(1);
        counter.set(n);
        // Positive and nonzero: the shim treats cookie 0 as "not one of ours".
        let cookie = (state.hash_one(n) & 0x7fff_ffff) as i32;
        if cookie != 0 && !CONVAR_PENDING.with(|p| p.borrow().contains_key(&cookie)) { return cookie; }
    })
}

pub(crate) fn convar_status_name(status: i32) -> &'static str {
    match status {
        0 => "ok",
        1 => "notFound",
        2 => "notValid",
        3 => "protected",
        _ => "unknown",
    }
}

/// Pending queries keep the frame detour requested, so the shim drains replies and the sweep runs.
pub(crate) fn convar_queries_pending() -> usize {
    CONVAR_PENDING.with(|p| p.borrow().len())
}

/// Run the query's callback with `outcome`. The entry and its subscription go only once the callback
/// actually ran; a busy isolate leaves both, with the outcome recorded for the sweep to retry.
fn settle_convar_query(cookie: i32, outcome: ConvarOutcome) {
    let key = cookie.to_string();
    let snap = CONVAR_QUERIES.with(|m| m.borrow().snapshot(&key));
    let delivery = fan_out(&snap, "convar_query_settle", Instrument::none(), |tc| match &outcome {
        ConvarOutcome::Cancelled => Some(vec![v8::null(tc).into(), v8::null(tc).into()]),
        ConvarOutcome::Answered { status, value } => {
            let label = v8::String::new(tc, convar_status_name(*status))?;
            let value = v8::String::new(tc, if *status == 0 { value } else { "" })?;
            Some(vec![label.into(), value.into()])
        }
    });
    CONVAR_PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if delivery == Delivery::Deferred {
            if let Some(q) = p.get_mut(&cookie) { q.outcome = Some(outcome); }
            return;
        }
        if let Some(q) = p.remove(&cookie) {
            CONVAR_QUERIES.with(|m| { m.borrow_mut().remove_by_ids(&[q.sub_id]); });
        }
    });
}

/// Resolve every pending query for `slot` with null (the client left, or the slot was reused).
fn cancel_convar_queries(slot: i32) {
    let cookies: Vec<i32> = CONVAR_PENDING.with(|p| p.borrow().iter()
        .filter(|(_, q)| q.slot == slot && q.outcome.is_none()).map(|(c, _)| *c).collect());
    for cookie in cookies { settle_convar_query(cookie, ConvarOutcome::Cancelled); }
}

/// Per frame, before any JS runs: retry deliveries a busy isolate deferred, and resolve null any query
/// that outlived its connection or the timeout.
pub(crate) fn sweep_convar_queries() {
    let now = std::time::Instant::now();
    let due: Vec<(i32, ConvarOutcome)> = CONVAR_PENDING.with(|p| p.borrow().iter().filter_map(|(c, q)| {
        match &q.outcome {
            Some(o) => Some((*c, o.clone())),
            None if now >= q.deadline || !matches(q.slot, q.token) => Some((*c, ConvarOutcome::Cancelled)),
            None => None,
        }
    }).collect());
    for (cookie, outcome) in due { settle_convar_query(cookie, outcome); }
}

/// Deliver a client's convar answer to the one query that minted `cookie`.
pub(crate) fn dispatch_convar_response(slot: i32, cookie: i32, status: i32, name: &str, value: &str) {
    let Some(token) = CONVAR_PENDING.with(|p| p.borrow().get(&cookie).and_then(|q|
        (q.outcome.is_none() && q.slot == slot && q.name.eq_ignore_ascii_case(name)).then_some(q.token)))
    else { return };   // a stale, forged or misrouted answer leaves the real query pending
    let outcome = if matches(slot, token) {
        ConvarOutcome::Answered { status, value: value.to_string() }
    } else {
        ConvarOutcome::Cancelled   // the connection the query went to is gone
    };
    settle_convar_query(cookie, outcome);
}

/// Native `__s2_client_query_convar(slot, name, token, callback) -> number` — ask the client in
/// `slot` for its value of `name`. Returns the query's cookie, or 0 when nothing was sent (bad slot,
/// stale connection, bot, empty or oversized name, too many pending queries, or the reply hook is
/// unavailable on this build). `callback(status, value)` runs once, on the frame after the reply;
/// `(null, null)` when the client disconnects first or no reply arrives within 15 seconds.
fn s2_client_query_convar(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        if args.length() < 4 || !args.get(3).is_function() { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        if !(0..64).contains(&slot) { return; }
        let name = args.get(1).to_rust_string_lossy(scope);
        if name.is_empty() || name.len() > MAX_CONVAR_NAME || name.bytes().any(|b| b.is_ascii_whitespace()) { return; }
        let token: u64 = args.get(2).to_rust_string_lossy(scope).parse().unwrap_or(0);
        if !matches(slot, token) { return; }
        if CONVAR_PENDING.with(|p| p.borrow().len()) >= MAX_PENDING_CONVAR_QUERIES {
            crate::v8host::log_warn("WARN: queryConVar: too many pending queries; refused");
            return;
        }
        let Some(f) = engine_ops().and_then(|o| o.client_query_convar) else { return };
        let Ok(cname) = CString::new(name.clone()) else { return };
        let cookie = mint_cookie();
        if crate::nest::with_outbound(&args, || f(slot, cname.as_ptr(), cookie)) == 0 { return; }
        let Some((sub_id, _)) = subscribe_into(scope, &args, &CONVAR_QUERIES, &cookie.to_string(), 3) else { return };
        let owner = crate::v8host::current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let deadline = std::time::Instant::now() + CONVAR_QUERY_TIMEOUT;
        CONVAR_PENDING.with(|p| p.borrow_mut().insert(cookie,
            ConvarQuery { slot, token, name, owner, sub_id, deadline, outcome: None }));
        crate::v8host::refresh_detour();
        rv.set_int32(cookie);
    }));
}

/// Native `__s2_client_replicate_convar(slot, name, value, token) -> boolean` — send the client in
/// `slot` a new value for its copy of a replicated cvar (CNETMsg_SetConVar). The server's own value
/// is untouched. False for a bad slot, stale connection, bot, or empty name.
fn s2_client_replicate_convar(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 4 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        if !(0..64).contains(&slot) { return; }
        let name = args.get(1).to_rust_string_lossy(scope);
        if name.is_empty() || name.len() > MAX_CONVAR_NAME || name.bytes().any(|b| b.is_ascii_whitespace()) { return; }
        let value = args.get(2).to_rust_string_lossy(scope);
        let token: u64 = args.get(3).to_rust_string_lossy(scope).parse().unwrap_or(0);
        if !matches(slot, token) { return; }
        let Some(f) = engine_ops().and_then(|o| o.client_replicate_convar) else { return };
        let (Ok(cname), Ok(cvalue)) = (CString::new(name), CString::new(value)) else { return };
        rv.set_bool(crate::nest::with_outbound(&args, || f(slot, cname.as_ptr(), cvalue.as_ptr())) != 0);
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_client_generation", s2_client_generation);
    set_native(scope, global_obj, "__s2_client_matches", s2_client_matches);
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
    set_native(scope, global_obj, "__s2_client_query_convar", s2_client_query_convar);
    set_native(scope, global_obj, "__s2_client_replicate_convar", s2_client_replicate_convar);
    set_native(scope, global_obj, "__s2_client_net_stats", s2_client_net_stats);
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
    crate::owner_stores::register(
        "CONVAR_QUERIES",
        Box::new(|owner| {
            CONVAR_QUERIES.with(|m| { m.borrow_mut().remove_by_owner(owner); });
            CONVAR_PENDING.with(|p| p.borrow_mut().retain(|_, q| q.owner != owner));
        }),
        Box::new(|ids| {
            CONVAR_QUERIES.with(|m| { m.borrow_mut().remove_by_ids(ids); });
            CONVAR_PENDING.with(|p| p.borrow_mut().retain(|_, q| !ids.contains(&q.sub_id)));
        }),
        Box::new(|| {
            CONVAR_QUERIES.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
            CONVAR_PENDING.with(|p| p.borrow_mut().clear());
        }),
    );
}

pub(crate) fn register_singletons() {
    crate::process_singletons::register("CLIENT_CONNECTIONS", crate::process_singletons::ResetPhase::AfterIsolateDrop,
        Box::new(|| CONNECTIONS.with(|b| b.borrow_mut().clear())));
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
    use std::os::raw::{c_char, c_int};
    thread_local! { static EFFECTS: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new()); }
    extern "C" fn effect(slot: i32, value: *const c_char) {
        let value = unsafe { std::ffi::CStr::from_ptr(value) }.to_string_lossy().into_owned();
        EFFECTS.with(|e| e.borrow_mut().push(value.clone()));
        if value == "replace" { begin(slot); }
    }
    extern "C" fn effect_command(slot: i32, value: *const c_char) -> i32 { effect(slot, value); 1 }
    extern "C" fn effect_voice(_: i32, _: i32) -> i32 { EFFECTS.with(|e| e.borrow_mut().push("voice".into())); 1 }
    fn effect_ops() -> S2EngineOps {
        S2EngineOps { client_print: Some(effect), client_console_print: Some(effect), client_kick: Some(effect),
            client_command: Some(effect_command), client_fake_command: Some(effect_command), voice_set_muted: Some(effect_voice), ..S2EngineOps::none() }
    }
    #[test]
    fn shutdown_clears_connections_without_reusing_generations() {
        init(dummy_logger()).unwrap();
        let before = begin(3);
        shutdown();
        assert_eq!(generation(3), 0);
        init(dummy_logger()).unwrap();
        assert_eq!(generation(3), 0);
        assert!(begin(3) > before);
        shutdown();
    }
    #[test]
    fn map_reconciliation_end_retires_only_departed_connection_state() {
        init(dummy_logger()).unwrap();
        extern "C" fn audible_set(_: i32, _: u64) -> i32 { 1 }
        extern "C" fn audible_clear(slot: i32) -> i32 {
            EFFECTS.with(|e| e.borrow_mut().push(format!("clear:{slot}")));
            1
        }
        set_engine_ops(Some(S2EngineOps { voice_audible_set: Some(audible_set),
            voice_audible_clear: Some(audible_clear), ..S2EngineOps::none() }));
        let departed = begin(10);
        let survivor = begin(1);
        load_body("map-retirement", r#"
            globalThis.departed = new __s2pkg_clients.Client(10);
            globalThis.survivor = new __s2pkg_clients.Client(1);
            globalThis.cached = [];
            __s2_cookie_on_cached(function(slot) { cached.push(slot); });
            [10, 1].forEach(function(slot) {
                __s2_cookie_session(slot, __s2_client_generation(slot), 'set', JSON.stringify({
                    steamId: 'account-' + slot, name: 'choice', value: 'value-' + slot, updated: 1
                }));
                __s2_cookie_dispatch_cached(slot, __s2_client_generation(slot));
                __s2_voice_audible_set(slot, []);
            });
        "#, "{}");
        EFFECTS.with(|e| e.borrow_mut().clear());
        // The shim's POST StartupServer scan found slot 10 absent (unsigned userid 65535),
        // but slot 1 occupied. Exercise the exact core entry points used by that scan.
        crate::ffi::s2script_core_client_end(10, departed);
        assert_eq!(crate::ffi::s2script_core_client_ensure(1), survivor);
        crate::ffi::s2script_core_dispatch_map_start(c"de_test".as_ptr());
        assert!(!matches(10, departed));
        assert!(matches(1, survivor));
        assert_eq!(EFFECTS.with(|e| e.borrow().clone()), vec!["clear:10"]);
        crate::cookies::dispatch_pending_cached();
        assert_eq!(eval_in_context_string("map-retirement",
            "[departed.isValid(), survivor.isValid(), cached.join(',')].join('|')"), "false|true|1");
        // Writes became host-owned at admission, for both departed and surviving accounts.
        // Map retirement only evicts the departed session; it does not create another write.
        assert_eq!(eval_in_context_string("map-retirement", r#"
            globalThis.pending = JSON.parse(__s2_cookie_lease(4, 262144));
            JSON.stringify(pending.map(function(row) {
                return [row.steamId, row.name, row.value, row.updated];
            }))
        "#), r#"[["account-10","choice","value-10",1],["account-1","choice","value-1",1]]"#);
        crate::ffi::s2script_core_client_end(10, departed);
        assert_eq!(eval_in_context_string("map-retirement",
            "__s2_cookie_lease(4, 262144)"), "[]", "repeated end cannot duplicate leased writes");
        assert_eq!(eval_in_context_string("map-retirement", r#"
            JSON.stringify(pending.map(function(row) {
                return __s2_cookie_ack(row.leaseId, row.revision, true);
            }))
        "#), "[true,true]", "each admitted write remains ACKable after map reconciliation");
        // Stale retirement cannot duplicate an ACKed write or remove a new occupant's state.
        let replacement = begin(10);
        crate::ffi::s2script_core_client_end(10, departed);
        assert!(matches(10, replacement));
        assert_eq!(eval_in_context_string("map-retirement", r#"
            __s2_cookie_session(10, __s2_client_generation(10), 'get',
                JSON.stringify({steamId: 'account-10', name: 'choice'}))
        "#), "null");
        assert_eq!(eval_in_context_string("map-retirement",
            "__s2_cookie_lease(4, 262144)"), "[]", "stale end cannot replay ACKed writes");
        crate::cookies::dispatch_pending_cached();
        assert_eq!(eval_in_context_string("map-retirement", "cached.join(',')"), "1");
        assert_eq!(eval_in_context_string("map-retirement", r#"
            __s2_cookie_session(1, __s2_client_generation(1), 'get',
                JSON.stringify({steamId: 'account-1', name: 'choice'}))
        "#), r#""value-1""#);
        assert_eq!(eval_in_context_string("map-retirement",
            "[__s2_voice_audible_clear(10), __s2_voice_audible_clear(1)].join('|')"), "false|true");
        shutdown();
    }
    #[test]
    fn current_connection_actions_and_identity_remain_available() {
        init(dummy_logger()).unwrap();
        extern "C" fn name(_: i32) -> *const c_char { c"Current".as_ptr() }
        extern "C" fn sid(_: i32) -> *const c_char { c"76561198000000001".as_ptr() }
        extern "C" fn address(_: i32) -> *const c_char { c"1.2.3.4:123".as_ptr() }
        extern "C" fn userid(_: i32) -> i32 { 42 }
        extern "C" fn signon(_: i32) -> i32 { 6 }
        extern "C" fn muted(_: i32) -> i32 { 1 }
        set_engine_ops(Some(S2EngineOps { client_name: Some(name), client_steamid: Some(sid), client_address: Some(address),
            client_userid: Some(userid), client_signon: Some(signon), voice_get_muted: Some(muted), ..effect_ops() }));
        begin(3);
        create_plugin_context("control");
        EFFECTS.with(|e| e.borrow_mut().clear());
        assert_eq!(eval_in_context_string("control", r#"
            var c = __s2pkg_clients.Clients.fromSlot(3);
            c.chat('chat'); c.print('console'); c.kick('kick'); c.voiceMuted = true;
            [c.isValid(), c.name, c.steamId, c.userId, c.signonState, c.ip, c.isBot,
             c.voiceMuted, c.command('command'), c.fakeCommand('fake')].join('|')
        "#), "true|Current|76561198000000001|42|6|1.2.3.4|false|true|true|true");
        assert_eq!(EFFECTS.with(|e| e.borrow().clone()), vec!["chat", "console\n", "kick", "voice", "command", "fake"]);
        begin(3);
        assert_eq!(eval_in_context_string("control", "[c.isValid(),c.name,c.steamId,c.userId,c.signonState,c.ip,c.isBot,c.voiceMuted].join('|')"), "false||0|-1|-1||false|false");
        shutdown();
    }

    #[test]
    fn legacy_snapshot_copies_each_engine_string_before_the_next_call() {
        init(dummy_logger()).unwrap();
        thread_local! { static SCRATCH: std::cell::RefCell<[u8; 32]> = std::cell::RefCell::new([0; 32]); }
        fn scratch(value: &str) -> *const c_char {
            SCRATCH.with(|s| { let mut s = s.borrow_mut(); s.fill(0); s[..value.len()].copy_from_slice(value.as_bytes()); s.as_ptr().cast() })
        }
        extern "C" fn sid(_: i32) -> *const c_char { scratch("account-A") }
        extern "C" fn name(_: i32) -> *const c_char { scratch("name-A") }
        extern "C" fn address(_: i32) -> *const c_char { scratch("address-A") }
        set_engine_ops(Some(S2EngineOps { client_steamid: Some(sid), client_name: Some(name), client_address: Some(address), ..S2EngineOps::none() }));
        let identity = snapshot(3);
        assert_eq!(identity["steamId"], "account-A");
        assert_eq!(identity["name"], "name-A");
        assert_eq!(identity["address"], "address-A");
        shutdown();
    }
    #[test]
    fn stale_actions_delayed_kick_and_reentrant_coercion_cannot_target_replacement() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(effect_ops()));
        begin(3);
        create_plugin_context("actions");
        EFFECTS.with(|e| e.borrow_mut().clear());
        eval_in_context_string("actions", r#"
            var c = new __s2pkg_clients.Client(3);
            c.chat({ toString: function () { c.command('replace'); return 'bad-chat'; } });
            c.kick('bad-kick'); c.print('bad-console'); c.voiceMuted = true;
            c.command('bad-command'); c.fakeCommand('bad-fake'); c.kickWithReason('bad-delay');
            'ok'
        "#);
        assert_eq!(EFFECTS.with(|e| e.borrow().clone()), vec!["replace"]);
        // A pending connect-time kick must not be adopted by B's active callback.
        eval_in_context_string("actions", "var a = new __s2pkg_clients.Client(3); a.kickWithReason('old-pending'); 'ok'");
        begin(3);
        let _ = dispatch_client_event("active", 3);
        assert_eq!(EFFECTS.with(|e| e.borrow().clone()), vec!["replace"]);
        // Exercise the timer continuation with a real captured connection handle.
        eval_in_context_string("actions", r#"
            var ticks = []; __s2pkg_timers.delay = function () { return { then: function (f) { ticks.push(f); } }; };
            var b = new __s2pkg_clients.Client(3);
            Object.defineProperty(b, 'signonState', { value: 6 });
            b.kickWithReason('current', 1); 'ok'
        "#);
        begin(3);
        eval_in_context_string("actions", "ticks[0](); 'ok'");
        assert_eq!(EFFECTS.with(|e| e.borrow().clone()), vec!["replace", "current", "current\n"]);
        // Coercion may itself establish B's pending kick. A must not overwrite that record.
        eval_in_context_string("actions", r#"
            var pendingA = new __s2pkg_clients.Client(3);
            pendingA.kickWithReason({ toString: function () {
                pendingA.command('replace');
                new __s2pkg_clients.Client(3).kickWithReason('replacement-pending', 1);
                return 'wrong-pending';
            } }); 'ok'
        "#);
        let _ = dispatch_client_event("active", 3);
        assert_eq!(EFFECTS.with(|e| e.borrow().clone()), vec!["replace", "current", "current\n", "replace", "replacement-pending", "replacement-pending\n"]);
        shutdown();
    }
    #[test]
    fn deferred_disconnect_identity_is_scoped_and_cannot_touch_new_occupant() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(effect_ops()));
        let a = begin(4);
        let identity = serde_json::json!({"name":"Departing A", "steamId":"76561198000000001", "userId":12, "signonState":6, "address":"1.2.3.4:123"});
        end(4, a);
        let b = begin(4);
        load_body("snapshot", r#"
            __s2pkg_clients.Clients.onDisconnect(function(c) {
                globalThis.saved = c;
                globalThis.identity = [c.name, c.steamId, c.userId, c.ip, c.isValid()].join('|');
                c.chat('bad'); c.kick('bad'); c.voiceMuted = true;
            });
        "#, "{}");
        EFFECTS.with(|e| e.borrow_mut().clear());
        let _ = replay_client_event_v2("disconnect", 4, a, Some(&identity));
        assert_eq!(eval_in_context_string("snapshot", "identity"), "Departing A|76561198000000001|12|1.2.3.4|false");
        assert_eq!(eval_in_context_string("snapshot", "[saved.name,saved.steamId,saved.userId,saved.isBot].join('|')"), "|0|-1|false");
        assert!(EFFECTS.with(|e| e.borrow().is_empty()));
        assert!(matches(4, b));
        shutdown();
    }
    #[test]
    fn lifecycle_replay_never_adopts_replacement_and_rechecks_each_subscriber() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(effect_ops()));
        let a = begin(2);
        load_body("events", r#"
            globalThis.n = 0;
            __s2pkg_clients.Clients.onActive(function(c) { n++; c.command('replace'); });
            __s2pkg_clients.Clients.onActive(function() { n += 100; });
        "#, "{}");
        let _ = replay_client_event_v2("active", 2, a, None);
        assert_eq!(eval_in_context_string("events", "String(n)"), "1");
        let _ = replay_client_event_v2("active", 2, a, None);
        let _ = replay_client_event("active", 2);
        assert_eq!(eval_in_context_string("events", "String(n)"), "1");
        let before = generation(2);
        crate::ffi::s2script_core_dispatch_map_start(c"de_test".as_ptr());
        assert_eq!(generation(2), before);
        let _ = dispatch_client_event("voice", 9);
        let _ = dispatch_client_event("settingschanged", 9);
        assert_eq!(generation(9), 0);
        assert_eq!(ensure(2), before);
        assert!(begin(2) > before);
        shutdown();
    }

    #[test]
    fn saved_client_cannot_act_on_reused_slot_even_for_same_account() {
        init(dummy_logger()).unwrap();
        CLIENT_CMD_CALLS.lock().unwrap().clear();
        FAKE_CMD_CALLS.lock().unwrap().clear();
        extern "C" fn valid(_: i32) -> i32 { 1 }
        extern "C" fn sid(_: i32) -> *const c_char { c"76561198000000001".as_ptr() }
        set_engine_ops(Some(S2EngineOps { client_valid: Some(valid), client_steamid: Some(sid), ..client_cmd_test_ops() }));
        let _ = dispatch_client_event("connect", 3);
        create_plugin_context("reuse");
        assert_eq!(eval_in_context_string("reuse", "var old = new __s2pkg_clients.Client(3); String(old.command('before'))"), "true");
        let _ = dispatch_client_event("disconnect", 3);
        let _ = dispatch_client_event("connect", 3);
        assert_eq!(eval_in_context_string("reuse", "String(old.isValid())"), "false");
        assert_eq!(eval_in_context_string("reuse", "String(old.command('wrong') || old.fakeCommand('wrong'))"), "false");
        assert_eq!(eval_in_context_string("reuse", "old.steamId"), "0");
        assert_eq!(eval_in_context_string("reuse", "String(new __s2pkg_clients.Client(3).command('after'))"), "true");
        assert_eq!(CLIENT_CMD_CALLS.lock().unwrap().as_slice(), &[(3, "before".into()), (3, "after".into())]);
        assert!(FAKE_CMD_CALLS.lock().unwrap().is_empty());
        shutdown();
    }

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
    /// the connection books are empty → `fromSlot(0)` is null and `all()` is empty; an invalid
    /// `Client` is not treated as a bot just because its stale SteamID defaults to "0".
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
        // The captured slot remains readable, but a stale connection is not a bot.
        assert_eq!(eval_in_context_string("pcl", "String(new __s2pkg_clients.Client(3).slot)"), "3");
        assert_eq!(eval_in_context_string("pcl", "String(new __s2pkg_clients.Client(3).isBot)"), "false");
        shutdown();
    }

    thread_local! { static QUERIES: std::cell::RefCell<Vec<(i32, String, i32)>> = std::cell::RefCell::new(Vec::new()); }
    extern "C" fn mock_query(slot: i32, name: *const c_char, cookie: i32) -> i32 {
        let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
        QUERIES.with(|q| q.borrow_mut().push((slot, name, cookie)));
        1
    }
    extern "C" fn mock_replicate(slot: i32, name: *const c_char, value: *const c_char) -> i32 {
        let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
        let value = unsafe { std::ffi::CStr::from_ptr(value) }.to_string_lossy().into_owned();
        EFFECTS.with(|e| e.borrow_mut().push(format!("{slot}:{name}={value}")));
        1
    }
    fn convar_ops() -> S2EngineOps {
        S2EngineOps { client_query_convar: Some(mock_query), client_replicate_convar: Some(mock_replicate), ..S2EngineOps::none() }
    }
    /// Promises settle at the frame's explicit microtask checkpoint; run one before reading.
    fn js(plugin: &str, expr: &str) -> String {
        crate::v8host::with_host_isolate(|isolate| isolate.perform_microtask_checkpoint()).unwrap();
        eval_in_context_string(plugin, expr)
    }
    fn last_cookie() -> i32 { QUERIES.with(|q| q.borrow().last().map(|q| q.2).unwrap_or(0)) }

    /// A reply reaches the query only from the queried slot, with the queried name, on the same
    /// connection; anything else leaves it pending.
    #[test]
    fn query_convar_delivers_only_the_matching_reply() {
        init(dummy_logger()).unwrap();
        QUERIES.with(|q| q.borrow_mut().clear());
        set_engine_ops(Some(convar_ops()));
        begin(4);
        create_plugin_context("cq1");
        eval_in_context_string("cq1", r#"globalThis.r = "pending";
            new __s2pkg_clients.Client(4).queryConVar("cl_interp").then(function (v) { globalThis.r = JSON.stringify(v); }); """#);
        let cookie = last_cookie();
        assert!(cookie > 0);
        assert_eq!(QUERIES.with(|q| q.borrow()[0].clone()), (4, "cl_interp".to_string(), cookie));
        dispatch_convar_response(5, cookie, 0, "cl_interp", "1");          // wrong slot
        dispatch_convar_response(4, cookie, 0, "cl_cmdrate", "1");         // wrong name
        dispatch_convar_response(4, cookie ^ 1, 0, "cl_interp", "1");      // wrong cookie
        assert_eq!(js("cq1", "r"), "pending");
        dispatch_convar_response(4, cookie, 0, "CL_INTERP", "0.03");
        assert_eq!(js("cq1", "r"), r#"{"status":"ok","value":"0.03"}"#);
        dispatch_convar_response(4, cookie, 0, "cl_interp", "again");      // answered once only
        assert_eq!(js("cq1", "r"), r#"{"status":"ok","value":"0.03"}"#);

        eval_in_context_string("cq1", r#"new __s2pkg_clients.Client(4).queryConVar("bind").then(function (v) { globalThis.r = JSON.stringify(v); }); """#);
        dispatch_convar_response(4, last_cookie(), 2, "bind", "ignored");
        assert_eq!(js("cq1", "r"), r#"{"status":"notValid","value":""}"#);
        shutdown();
    }

    /// Nothing is sent for a stale handle, a bot-like refusal, or a bad name, and the promise
    /// resolves null; a disconnect resolves a pending query null; unload drops it silently.
    #[test]
    fn query_convar_refuses_and_cancels() {
        init(dummy_logger()).unwrap();
        QUERIES.with(|q| q.borrow_mut().clear());
        set_engine_ops(Some(convar_ops()));
        let token = begin(6);
        create_plugin_context("cq2");
        eval_in_context_string("cq2", r#"globalThis.out = [];
            var c = new __s2pkg_clients.Client(6);
            ["", "has space", "x".repeat(300)].forEach(function (n) { c.queryConVar(n).then(function (v) { out.push(String(v)); }); }); """#);
        assert_eq!(js("cq2", "out.join()"), "null,null,null");
        assert!(QUERIES.with(|q| q.borrow().is_empty()));
        eval_in_context_string("cq2", r#"globalThis.c6 = new __s2pkg_clients.Client(6);
            c6.queryConVar("cl_interp").then(function (v) { out.push(String(v)); }); """#);
        let cookie = last_cookie();
        end(6, token);
        assert_eq!(js("cq2", "out.join()"), "null,null,null,null");
        dispatch_convar_response(6, cookie, 0, "cl_interp", "1");          // late reply: nobody left
        assert_eq!(js("cq2", "out.join()"), "null,null,null,null");
        begin(6);
        eval_in_context_string("cq2", r#"c6.queryConVar("cl_interp").then(function (v) { out.push(String(v)); }); """#);
        assert_eq!(js("cq2", "out.join()"), "null,null,null,null,null", "stale handle");

        create_plugin_context("cq3");
        eval_in_context_string("cq3", r#"globalThis.r = "pending";
            new __s2pkg_clients.Client(6).queryConVar("cl_interp").then(function (v) { globalThis.r = String(v); }); """#);
        assert_eq!(CONVAR_PENDING.with(|p| p.borrow().len()), 1);
        unload_plugin("cq3");
        assert_eq!(CONVAR_PENDING.with(|p| p.borrow().len()), 0, "unload drops the owner's queries");
        shutdown();
    }

    /// A reply that arrives while the isolate is busy is kept and delivered by the next frame's
    /// sweep; a query nobody answers resolves null at the timeout; a token mismatch resolves null.
    #[test]
    fn query_convar_retries_deferred_and_times_out() {
        init(dummy_logger()).unwrap();
        QUERIES.with(|q| q.borrow_mut().clear());
        set_engine_ops(Some(convar_ops()));
        begin(7);
        create_plugin_context("cq4");
        eval_in_context_string("cq4", r#"globalThis.out = []; globalThis.c7 = new __s2pkg_clients.Client(7);
            c7.queryConVar("cl_interp").then(function (v) { out.push(JSON.stringify(v)); }); """#);
        let cookie = last_cookie();
        // A reply that lands while the host is borrowed (and not inside an outbound call) defers.
        crate::v8host::with_host_isolate(|_| dispatch_convar_response(7, cookie, 0, "cl_interp", "late")).unwrap();
        assert_eq!(js("cq4", "out.join()"), "", "nothing ran inside the busy isolate");
        assert_eq!(CONVAR_PENDING.with(|p| p.borrow().len()), 1, "the reply is kept");
        dispatch_convar_response(7, cookie, 0, "cl_interp", "dup");          // already settled
        sweep_convar_queries();
        assert_eq!(js("cq4", "out.join()"), r#"{"status":"ok","value":"late"}"#);
        assert_eq!(convar_queries_pending(), 0);

        eval_in_context_string("cq4", r#"c7.queryConVar("cl_interp").then(function (v) { out.push(String(v)); }); """#);
        sweep_convar_queries();
        assert_eq!(js("cq4", "out.length"), "1", "not yet due");
        CONVAR_PENDING.with(|p| p.borrow_mut().values_mut().for_each(|q| q.deadline = std::time::Instant::now()));
        sweep_convar_queries();
        assert_eq!(js("cq4", "out[1]"), "null", "timed out");
        assert_eq!(convar_queries_pending(), 0);

        eval_in_context_string("cq4", r#"c7.queryConVar("cl_interp").then(function (v) { out.push(String(v)); }); """#);
        let cookie = last_cookie();
        CONVAR_PENDING.with(|p| p.borrow_mut().get_mut(&cookie).unwrap().token ^= 1);
        dispatch_convar_response(7, cookie, 0, "cl_interp", "1");
        assert_eq!(js("cq4", "out[2]"), "null", "the queried connection is gone");
        assert_eq!(convar_queries_pending(), 0);
        shutdown();
    }

    #[test]
    fn replicate_convar_sends_text_and_refuses_stale() {
        init(dummy_logger()).unwrap();
        EFFECTS.with(|e| e.borrow_mut().clear());
        set_engine_ops(Some(convar_ops()));
        begin(2);
        create_plugin_context("cr1");
        let out = eval_in_context_string("cr1", r#"var c = new __s2pkg_clients.Client(2);
            [c.replicateConVar("sv_cheats", true), c.replicateConVar("fov_desired", 90), c.replicateConVar("", "x")].join()"#);
        assert_eq!(out, "true,true,false");
        assert_eq!(EFFECTS.with(|e| e.borrow().clone()), vec!["2:sv_cheats=1".to_string(), "2:fov_desired=90".to_string()]);
        begin(2);
        assert_eq!(eval_in_context_string("cr1", r#"String(c.replicateConVar("sv_cheats", 0))"#), "false");
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
        begin(5);
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
        begin(3);
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

    extern "C" fn mock_client_net_stats(slot: c_int, out: *mut f64, n: c_int) -> c_int {
        if slot != 3 || n < NET_STATS_LEN as c_int { return 0; }   // slot 3 is the only human
        let out = unsafe { std::slice::from_raw_parts_mut(out, NET_STATS_LEN) };
        for (i, v) in out.iter_mut().enumerate() { *v = i as f64 + 0.5; }
        out[16] = 1.0;   // isTimingOut
        NET_STATS_LEN as c_int
    }

    /// `netStats()` maps the op's values by index into the documented shape, flow pairs as
    /// (outgoing, incoming), and is null for a slot the op has no net channel for.
    #[test]
    fn net_stats_maps_values_and_is_null_for_a_bot() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps { client_net_stats: Some(mock_client_net_stats), ..mock_event_ops() }));
        begin(3);
        begin(4);
        create_plugin_context("ns1");
        let out = eval_in_context_string("ns1", r#"var s = new __s2pkg_clients.Client(3).netStats();
            JSON.stringify([s.avgLatency, s.engineLatency, s.avgLoss, s.avgChoke.incoming, s.avgData.outgoing,
                s.avgPackets.incoming, s.totalData, s.totalPackets.outgoing, s.timeConnected,
                s.timeSinceLastReceived, s.isTimingOut, s.dataRate])"#);
        assert_eq!(out, r#"[0.5,1.5,{"outgoing":2.5,"incoming":3.5},5.5,6.5,9.5,{"outgoing":10.5,"incoming":11.5},12.5,14.5,15.5,true,17.5]"#);
        assert_eq!(eval_in_context_string("ns1", "String(new __s2pkg_clients.Client(4).netStats())"), "null");
        assert_eq!(eval_in_context_string("ns1", "String(new __s2pkg_clients.Client(64).netStats())"), "null");
        shutdown();
    }

    /// Without the op (an older shim) `netStats()` is null rather than a zeroed snapshot.
    #[test]
    fn net_stats_is_null_without_the_op() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(mock_event_ops()));
        begin(3);
        create_plugin_context("ns2");
        assert_eq!(eval_in_context_string("ns2", "String(new __s2pkg_clients.Client(3).netStats())"), "null");
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
