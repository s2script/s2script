//! `@s2script/sdk/steam` — the server's own Steam game-server session and Steam Workshop downloads
//! (ISteamGameServer + ISteamUGC).
//!
//! Engine-generic: every Source 2 dedicated server runs the Steamworks game-server API. The shim
//! borrows the engine's session (`shim/src/steam.cpp`); every engine fact arrives through an
//! `S2EngineOps` function pointer, and every Steam callback through
//! `s2script_core_dispatch_steam_event`.
//!
//! Workshop ids are 64-bit, so they cross into JS as decimal strings (a `BigInt` cannot cross the
//! plugin boundary). Natives accept a decimal string, a safe integer or a `BigInt`.
//!
//! Callbacks are notify-only. Each is queued here and delivered at once; when the isolate is busy
//! the event stays queued, keeps the frame detour requested, and the next frame's sweep delivers it.
//! `Workshop.download` waiters are one-shot subscriptions on the same store, keyed by item, so
//! plugin unload drops them like any other subscription.

use crate::dispatch::{fan_out, Delivery, Instrument};
use crate::v8host::{engine_ops, set_native, subscribe_into};
use std::collections::VecDeque;

/// One Steam callback, as the shim reports it. Kinds mirror `shim/src/steam.h` `S2SteamEventKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SteamEvent {
    Connected,
    Disconnected { result: i32 },
    ConnectFailure { result: i32, still_retrying: bool },
    DownloadResult { id: u64, result: i32, app_id: u32 },
    ItemInstalled { id: u64, app_id: u32 },
}

impl SteamEvent {
    fn from_raw(kind: i32, id: u64, result: i32, flag: i32) -> Option<Self> {
        Some(match kind {
            0 => SteamEvent::Connected,
            1 => SteamEvent::Disconnected { result },
            2 => SteamEvent::ConnectFailure { result, still_retrying: flag != 0 },
            3 => SteamEvent::DownloadResult { id, result, app_id: flag as u32 },
            4 => SteamEvent::ItemInstalled { id, app_id: flag as u32 },
            _ => return None,
        })
    }

    /// The listener channel this event fans out on.
    fn channel(&self) -> &'static str {
        match self {
            SteamEvent::Connected => "connected",
            SteamEvent::Disconnected { .. } => "disconnected",
            SteamEvent::ConnectFailure { .. } => "connectfailure",
            SteamEvent::DownloadResult { .. } => "download",
            SteamEvent::ItemInstalled { .. } => "installed",
        }
    }
}

/// The channels `__s2_steam_subscribe` accepts.
const CHANNELS: [&str; 5] = ["connected", "disconnected", "connectfailure", "download", "installed"];

/// Queued, undelivered callbacks. Bounded so a stuck isolate cannot grow it without limit; the
/// oldest event is dropped first, with a warning.
const MAX_QUEUED_EVENTS: usize = 4096;
/// Outstanding `Workshop.download` waiters across every plugin.
const MAX_DOWNLOAD_WAITERS: usize = 4096;
/// Steam's install folder paths are short; GetItemInstallInfo truncates to the buffer.
const INSTALL_FOLDER_CAP: usize = 1024;

thread_local! {
    /// Listener and one-shot waiter store. Listener keys are `CHANNELS`; a download waiter is keyed
    /// `"wait:<id>"` and removed once it has run.
    static STEAM_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    /// Undelivered events. `listeners_done` marks a download result whose listeners already ran
    /// and whose waiters were deferred, so a retry never runs a listener twice.
    static QUEUE: std::cell::RefCell<VecDeque<Queued>> = const { std::cell::RefCell::new(VecDeque::new()) };
    /// Outstanding download waiters: sub id → (owning plugin, waiter key), for the bound and teardown.
    static WAITERS: std::cell::RefCell<std::collections::HashMap<u64, (String, String)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Per item, the epoch a new waiter joins. A download result takes the current epoch when it is
    /// queued and moves the item to the next one, so a waiter added after the result arrived (a
    /// listener retrying the same item, say) waits for the next result instead of this one.
    static EPOCHS: std::cell::RefCell<std::collections::HashMap<u64, u64>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
}

#[derive(Clone, Copy, Debug)]
struct Queued { event: SteamEvent, listeners_done: bool, epoch: u64 }

fn waiter_key(id: u64, epoch: u64) -> String { format!("wait:{id}:{epoch}") }

fn current_epoch(id: u64) -> u64 { EPOCHS.with(|e| e.borrow().get(&id).copied().unwrap_or(0)) }

/// The epoch a result for `id` settles, moving later waiters on to the next one.
fn take_epoch(id: u64) -> u64 {
    EPOCHS.with(|e| { let mut e = e.borrow_mut(); let n = e.entry(id).or_insert(0); let cur = *n; *n += 1; cur })
}

/// Queued events keep the frame detour requested, so the sweep that delivers them runs.
pub(crate) fn pending() -> usize {
    QUEUE.with(|q| q.borrow().len())
}

/// Shim → core entry (via `ffi.rs`). Queue the event, then try to deliver everything queued.
pub(crate) fn dispatch_event(kind: i32, id: u64, result: i32, flag: i32) {
    let Some(event) = SteamEvent::from_raw(kind, id, result, flag) else { return };
    let dropped = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let dropped = if q.len() >= MAX_QUEUED_EVENTS { q.pop_front() } else { None };
        let epoch = if let SteamEvent::DownloadResult { id, .. } = event { take_epoch(id) } else { 0 };
        q.push_back(Queued { event, listeners_done: false, epoch });
        dropped
    });
    if let Some(d) = dropped {
        crate::v8host::log_warn(&format!("WARN: steam: event queue full; dropped {:?}", d.event));
    }
    deliver_queued();
    crate::v8host::refresh_detour();
}

/// Per frame, before any JS runs: deliver events a busy isolate deferred.
pub(crate) fn sweep() {
    if pending() == 0 { return; }
    deliver_queued();
    crate::v8host::refresh_detour();
}

thread_local! {
    /// True while `deliver_queued` runs. A listener can make Steam report an event on this thread
    /// (an already-current item's DownloadItemResult inside `Workshop.download`); that nested
    /// dispatch only queues, and the running drain reaches it after the current head.
    static DRAINING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn deliver_queued() {
    if DRAINING.with(|d| d.replace(true)) { return; }
    struct Reset;
    impl Drop for Reset { fn drop(&mut self) { DRAINING.with(|d| d.set(false)); } }
    let _reset = Reset;
    loop {
        let Some(head) = QUEUE.with(|q| q.borrow().front().copied()) else { return };
        match deliver(head) {
            Some(retry) => {
                QUEUE.with(|q| { if let Some(f) = q.borrow_mut().front_mut() { *f = retry; } });
                return;
            }
            None => { QUEUE.with(|q| { q.borrow_mut().pop_front(); }); }
        }
    }
}

/// Run every listener for the event, then (for a download result) every waiter on that item.
/// `Some(entry)` = the isolate was busy; `entry` is what to retry next frame.
fn deliver(entry: Queued) -> Option<Queued> {
    let event = entry.event;
    if !entry.listeners_done {
        let snap = STEAM_MUX.with(|m| m.borrow().snapshot(event.channel()));
        let label = format!("steam:{}", event.channel());
        if fan_out(&snap, &label, Instrument::breadcrumb(&label), |tc| build_args(tc, event)) == Delivery::Deferred {
            return Some(entry);
        }
    }
    let SteamEvent::DownloadResult { id, result, .. } = event else { return None };
    let key = waiter_key(id, entry.epoch);
    let waiters = STEAM_MUX.with(|m| m.borrow().snapshot(&key));
    if waiters.is_empty() { return None; }
    let delivery = fan_out(&waiters, "steam:download-wait", Instrument::none(), |tc| {
        Some(vec![v8::Integer::new(tc, result).into()])
    });
    if delivery == Delivery::Deferred {
        return Some(Queued { listeners_done: true, ..entry });
    }
    STEAM_MUX.with(|m| m.borrow_mut().remove_by_name(&key));
    WAITERS.with(|w| w.borrow_mut().retain(|_, (_, k)| *k != key));
    None
}

fn build_args<'s>(tc: &mut v8::PinScope<'s, '_>, event: SteamEvent) -> Option<Vec<v8::Local<'s, v8::Value>>> {
    Some(match event {
        SteamEvent::Connected => vec![],
        SteamEvent::Disconnected { result } => vec![v8::Integer::new(tc, result).into()],
        SteamEvent::ConnectFailure { result, still_retrying } => {
            vec![v8::Integer::new(tc, result).into(), v8::Boolean::new(tc, still_retrying).into()]
        }
        SteamEvent::DownloadResult { id, result, app_id } => vec![
            v8::String::new(tc, &id.to_string())?.into(),
            v8::Integer::new(tc, result).into(),
            v8::Integer::new_from_unsigned(tc, app_id).into(),
        ],
        SteamEvent::ItemInstalled { id, app_id } => vec![
            v8::String::new(tc, &id.to_string())?.into(),
            v8::Integer::new_from_unsigned(tc, app_id).into(),
        ],
    })
}

// ---------------------------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------------------------

/// A workshop item id from JS: a decimal string, a positive safe integer, or a `BigInt`. `None` for
/// anything else, zero, or out of range.
fn item_id(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<u64> {
    let id = if v.is_string() {
        let s = v.to_rust_string_lossy(scope);
        if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
        s.parse::<u64>().ok()?
    } else if v.is_big_int() {
        let (n, lossless) = v8::Local::<v8::BigInt>::try_from(v).ok()?.u64_value();
        if !lossless { return None; }
        n
    } else if v.is_number() {
        let n = v.number_value(scope)?;
        if !(n.is_finite() && n.fract() == 0.0 && n > 0.0 && n <= 9_007_199_254_740_991.0) { return None; }
        n as u64
    } else {
        return None;
    };
    (id != 0).then_some(id)
}

/// Native `__s2_steam_status() -> number`. Bit 0: the Steam interfaces resolved. Bit 1: the server
/// is logged on. Bit 2: VAC secure. 0 with no op.
fn s2_steam_status(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(engine_ops().and_then(|o| o.steam_status).map(|f| f()).unwrap_or(0));
    }));
}

/// Native `__s2_steam_server_id() -> string | null`. The server's SteamID64, null before log-on.
fn s2_steam_server_id(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let id = engine_ops().and_then(|o| o.steam_server_id).map(|f| f()).unwrap_or(0);
        if id == 0 { return; }
        if let Some(s) = v8::String::new(scope, &id.to_string()) { rv.set(s.into()); }
    }));
}

/// Native `__s2_steam_public_ip() -> string | null`. The public IPv4 Steam reports, dotted.
fn s2_steam_public_ip(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let ip = engine_ops().and_then(|o| o.steam_public_ip).map(|f| f()).unwrap_or(0);
        if ip == 0 { return; }
        let text = std::net::Ipv4Addr::from(ip).to_string();
        if let Some(s) = v8::String::new(scope, &text) { rv.set(s.into()); }
    }));
}

/// Native `__s2_steam_subscribe(channel, handler) -> number`. A plugin-lifetime listener on one of
/// `CHANNELS`; 0 for an unknown channel or a non-function.
fn s2_steam_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let channel = args.get(0).to_rust_string_lossy(scope);
        if !CHANNELS.contains(&channel.as_str()) { return; }
        let Some((sub_id, _)) = subscribe_into(scope, &args, &STEAM_MUX, &channel, 1) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// Native `__s2_steam_download(id, highPriority, callback?) -> boolean`. ISteamUGC::DownloadItem:
/// download (or update) a workshop item. True when Steam accepted the request; then `callback(result)`
/// runs once with the EResult of the item's DownloadItemResult. False for a bad id, Steam
/// unavailable or not logged on, or too many outstanding waiters (nothing is requested then).
fn s2_steam_download(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some(id) = item_id(scope, args.get(0)) else { return };
        let high = args.get(1).boolean_value(scope);
        let wants_callback = args.get(2).is_function();
        if wants_callback && WAITERS.with(|w| w.borrow().len()) >= MAX_DOWNLOAD_WAITERS {
            crate::v8host::log_warn("WARN: Workshop.download: too many outstanding downloads; refused");
            return;
        }
        let Some(f) = engine_ops().and_then(|o| o.steam_ugc_download) else { return };
        // The waiter is in place BEFORE the outbound call: an item that is already current can have
        // its DownloadItemResult dispatched on this thread during DownloadItem itself, and that
        // result takes the epoch the waiter must be keyed on.
        let mut waiter = None;
        if wants_callback {
            let key = waiter_key(id, current_epoch(id));
            let Some((sub_id, _)) = subscribe_into(scope, &args, &STEAM_MUX, &key, 2) else { return };
            let owner = crate::v8host::current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
            WAITERS.with(|w| w.borrow_mut().insert(sub_id, (owner, key)));
            waiter = Some(sub_id);
        }
        if crate::nest::with_outbound(&args, || f(id, high as i32)) == 0 {
            if let Some(sub_id) = waiter {
                STEAM_MUX.with(|m| m.borrow_mut().remove_by_ids(&[sub_id]));
                WAITERS.with(|w| w.borrow_mut().remove(&sub_id));
            }
            return;
        }
        rv.set_bool(true);
    }));
}

/// Native `__s2_steam_item_state(id) -> number`. ISteamUGC::GetItemState flags; -1 when Steam is
/// unavailable or the id is bad.
fn s2_steam_item_state(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let Some(id) = item_id(scope, args.get(0)) else { return };
        let Some(f) = engine_ops().and_then(|o| o.steam_ugc_state) else { return };
        let state = f(id);
        if (0..=i32::MAX as i64).contains(&state) { rv.set_int32(state as i32); }
    }));
}

/// Native `__s2_steam_download_info(id) -> [downloaded, total] | null`. ISteamUGC::GetItemDownloadInfo.
fn s2_steam_download_info(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some(id) = item_id(scope, args.get(0)) else { return };
        let Some(f) = engine_ops().and_then(|o| o.steam_ugc_download_info) else { return };
        let (mut done, mut total) = (0u64, 0u64);
        if f(id, &mut done, &mut total) == 0 { return; }
        let d = v8::Number::new(scope, done as f64).into();
        let t = v8::Number::new(scope, total as f64).into();
        rv.set(v8::Array::new_with_elements(scope, &[d, t]).into());
    }));
}

/// Native `__s2_steam_install_info(id) -> [folder, sizeOnDisk, timestamp] | null`.
/// ISteamUGC::GetItemInstallInfo: null when the item is not installed.
fn s2_steam_install_info(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some(id) = item_id(scope, args.get(0)) else { return };
        let Some(f) = engine_ops().and_then(|o| o.steam_ugc_install_info) else { return };
        let mut buf = vec![0u8; INSTALL_FOLDER_CAP];
        let (mut size, mut ts) = (0u64, 0u32);
        if f(id, &mut size, buf.as_mut_ptr().cast(), buf.len() as i32, &mut ts) == 0 { return; }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let folder = String::from_utf8_lossy(&buf[..len]).into_owned();
        let Some(folder) = v8::String::new(scope, &folder) else { return };
        let size = v8::Number::new(scope, size as f64).into();
        let ts = v8::Number::new(scope, ts as f64).into();
        rv.set(v8::Array::new_with_elements(scope, &[folder.into(), size, ts]).into());
    }));
}

/// Native `__s2_steam_suspend(suspend) -> boolean`. ISteamUGC::SuspendDownloads; false when Steam is
/// unavailable.
fn s2_steam_suspend(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let suspend = args.get(0).boolean_value(scope);
        let Some(f) = engine_ops().and_then(|o| o.steam_ugc_suspend) else { return };
        rv.set_bool(f(suspend as i32) != 0);
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_steam_status", s2_steam_status);
    set_native(scope, global_obj, "__s2_steam_server_id", s2_steam_server_id);
    set_native(scope, global_obj, "__s2_steam_public_ip", s2_steam_public_ip);
    set_native(scope, global_obj, "__s2_steam_subscribe", s2_steam_subscribe);
    set_native(scope, global_obj, "__s2_steam_download", s2_steam_download);
    set_native(scope, global_obj, "__s2_steam_item_state", s2_steam_item_state);
    set_native(scope, global_obj, "__s2_steam_download_info", s2_steam_download_info);
    set_native(scope, global_obj, "__s2_steam_install_info", s2_steam_install_info);
    set_native(scope, global_obj, "__s2_steam_suspend", s2_steam_suspend);
}

/// The owner-scoped store: listeners and download waiters go with their plugin. The shim's Steam
/// callbacks stay registered for the process lifetime, so there is no engine-op follow-up.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "STEAM_MUX",
        Box::new(|owner| {
            STEAM_MUX.with(|m| { m.borrow_mut().remove_by_owner(owner); });
            WAITERS.with(|w| w.borrow_mut().retain(|_, (o, _)| o != owner));
        }),
        Box::new(|ids| {
            STEAM_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); });
            WAITERS.with(|w| { let mut w = w.borrow_mut(); for i in ids { w.remove(i); } });
        }),
        Box::new(|| {
            STEAM_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
            WAITERS.with(|w| w.borrow_mut().clear());
            EPOCHS.with(|e| e.borrow_mut().clear());
            QUEUE.with(|q| q.borrow_mut().clear());
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string};
    use crate::v8host::{create_plugin_context, init, set_engine_ops, shutdown, unload_plugin, S2EngineOps};
    use std::os::raw::{c_char, c_int};

    thread_local! {
        static REQUESTS: std::cell::RefCell<Vec<(u64, c_int)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    extern "C" fn status() -> c_int { 3 }
    extern "C" fn server_id() -> u64 { 90_071_992_547_409_920 }
    extern "C" fn public_ip() -> u32 { 0xCB00_7107 }
    extern "C" fn download(id: u64, high: c_int) -> c_int {
        REQUESTS.with(|r| r.borrow_mut().push((id, high)));
        // 77 is already current: Steam reports its result on this thread, inside DownloadItem.
        if id == 77 { dispatch_event(DOWNLOAD, 77, 1, 730); }
        (id != 404) as c_int
    }
    extern "C" fn state(id: u64) -> i64 { if id == 7 { 4 } else { 8 | 32 } }
    extern "C" fn download_info(id: u64, d: *mut u64, t: *mut u64) -> c_int {
        if id != 7 { return 0; }
        unsafe { *d = 512; *t = 2048; }
        1
    }
    extern "C" fn install_info(id: u64, size: *mut u64, folder: *mut c_char, cap: c_int, ts: *mut u32) -> c_int {
        if id != 7 || cap < 16 { return 0; }
        let path = b"/ws/content/7\0";
        unsafe {
            std::ptr::copy_nonoverlapping(path.as_ptr().cast(), folder, path.len());
            *size = 4096;
            *ts = 1_700_000_000;
        }
        1
    }
    fn ops() -> S2EngineOps {
        S2EngineOps {
            steam_status: Some(status), steam_server_id: Some(server_id), steam_public_ip: Some(public_ip),
            steam_ugc_download: Some(download), steam_ugc_state: Some(state),
            steam_ugc_download_info: Some(download_info), steam_ugc_install_info: Some(install_info),
            ..S2EngineOps::none()
        }
    }
    fn js(plugin: &str, expr: &str) -> String {
        crate::v8host::with_host_isolate(|isolate| isolate.perform_microtask_checkpoint()).unwrap();
        eval_in_context_string(plugin, expr)
    }
    const DOWNLOAD: i32 = 3;

    #[test]
    fn session_queries_read_the_ops_and_degrade_without_them() {
        init(dummy_logger()).unwrap();
        create_plugin_context("st1");
        assert_eq!(eval_in_context_string("st1", r#"var S = __s2pkg_steam.Steam, W = __s2pkg_steam.Workshop;
            JSON.stringify([S.isAvailable(), S.isLoggedOn(), S.steamId(), S.publicIp(), W.state("7"),
                W.installInfo(7), W.suspendDownloads()])"#),
            "[false,false,null,null,null,null,false]");
        set_engine_ops(Some(ops()));
        assert_eq!(eval_in_context_string("st1", r#"JSON.stringify([S.isAvailable(), S.isLoggedOn(), S.isSecure(),
            S.steamId(), S.publicIp(), W.state(7n), W.isInstalled("7"), W.isInstalled("8"),
            W.downloadProgress("7"), W.installInfo("7"), W.downloadProgress("8")])"#),
            r#"[true,true,false,"90071992547409920","203.0.113.7",4,true,false,{"id":"7","bytesDownloaded":512,"bytesTotal":2048},{"id":"7","folder":"/ws/content/7","sizeOnDisk":4096,"timestamp":1700000000},null]"#);
        // Ids that are not a positive decimal u64 never reach Steam.
        assert_eq!(eval_in_context_string("st1", r#"JSON.stringify(["", "0", "-1", "1e3", "x", 0, 1.5, -2, 2 ** 60,
            "18446744073709551616", null, {}].map(function (id) { return W.state(id); }))"#),
            "[null,null,null,null,null,null,null,null,null,null,null,null]");
        shutdown();
    }

    #[test]
    fn download_resolves_from_its_item_result_and_listeners_see_every_result() {
        init(dummy_logger()).unwrap();
        REQUESTS.with(|r| r.borrow_mut().clear());
        set_engine_ops(Some(ops()));
        create_plugin_context("st2");
        eval_in_context_string("st2", r#"globalThis.out = []; globalThis.seen = [];
            var W = __s2pkg_steam.Workshop;
            W.onDownloadResult(function (e) { seen.push(e.id + ":" + e.result + ":" + e.appId); });
            W.download("18446744073709551615", { highPriority: true }).then(function (r) { out.push(JSON.stringify(r)); });
            W.download(404).then(function (r) { out.push(JSON.stringify(r)); });
            W.download("x").then(function (r) { out.push(JSON.stringify(r)); }); "" "#);
        assert_eq!(REQUESTS.with(|r| r.borrow().clone()), vec![(u64::MAX, 1), (404, 0)], "a bad id is never requested");
        assert_eq!(js("st2", "out.join('|')"),
            r#"{"id":"404","result":0,"ok":false}|{"id":"x","result":0,"ok":false}"#);
        dispatch_event(DOWNLOAD, 5, 1, 730);                       // another item
        assert_eq!(js("st2", "out.length"), "2");
        dispatch_event(DOWNLOAD, u64::MAX, 1, 730);
        assert_eq!(js("st2", "out[2]"), r#"{"id":"18446744073709551615","result":1,"ok":true}"#);
        dispatch_event(DOWNLOAD, u64::MAX, 2, 730);                // the waiter was one-shot
        assert_eq!(js("st2", "out.length + '|' + seen.join()"),
            "3|5:1:730,18446744073709551615:1:730,18446744073709551615:2:730");
        assert!(WAITERS.with(|w| w.borrow().is_empty()));
        shutdown();
    }

    /// A listener that retries the same item gets a waiter for the NEXT result, not the one it is
    /// being told about.
    #[test]
    fn a_retry_from_a_listener_waits_for_the_next_result() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(ops()));
        create_plugin_context("st5");
        eval_in_context_string("st5", r#"globalThis.out = []; var W = __s2pkg_steam.Workshop;
            W.onDownloadResult(function (e) {
                if (!e.ok && out.length === 0) W.download(e.id).then(function (r) { out.push("retry:" + r.result); });
            });
            W.download(12).then(function (r) { out.push("first:" + r.result); }); "" "#);
        dispatch_event(DOWNLOAD, 12, 2, 730);
        assert_eq!(js("st5", "out.join()"), "first:2", "the retry is still pending");
        dispatch_event(DOWNLOAD, 12, 1, 730);
        assert_eq!(js("st5", "out.join()"), "first:2,retry:1");
        assert!(WAITERS.with(|w| w.borrow().is_empty()));
        shutdown();
    }

    /// A result Steam dispatches during DownloadItem itself still settles that call's waiter.
    #[test]
    fn a_result_reported_inside_download_item_settles_it() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(ops()));
        create_plugin_context("st6");
        eval_in_context_string("st6", r#"globalThis.out = [];
            __s2pkg_steam.Workshop.download(77).then(function (r) { out.push(JSON.stringify(r)); }); "" "#);
        sweep();
        assert_eq!(js("st6", "out.join()"), r#"{"id":"77","result":1,"ok":true}"#);
        assert!(WAITERS.with(|w| w.borrow().is_empty()));
        assert_eq!(pending(), 0);
        shutdown();
    }

    /// A listener whose download is answered inside DownloadItem: the nested result is queued
    /// behind the running one, so no listener sees an event twice and nothing is dropped.
    #[test]
    fn a_result_reported_from_inside_a_listener_is_queued_behind_it() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(ops()));
        create_plugin_context("st7");
        eval_in_context_string("st7", r#"globalThis.seen = []; globalThis.out = [];
            var W = __s2pkg_steam.Workshop;
            W.onDownloadResult(function (e) {
                seen.push(e.id + ":" + e.result);
                if (e.id === "5") W.download(77).then(function (r) { out.push(r.id + ":" + r.result); });
            }); "" "#);
        dispatch_event(DOWNLOAD, 5, 2, 730);
        dispatch_event(DOWNLOAD, 6, 1, 730);
        sweep();
        assert_eq!(js("st7", "seen.join()"), "5:2,77:1,6:1");
        assert_eq!(js("st7", "out.join()"), "77:1");
        assert_eq!(pending(), 0);
        assert!(WAITERS.with(|w| w.borrow().is_empty()));
        shutdown();
    }

    #[test]
    fn busy_isolate_defers_events_without_repeating_listeners() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(ops()));
        create_plugin_context("st3");
        eval_in_context_string("st3", r#"globalThis.log = [];
            var S = __s2pkg_steam.Steam, W = __s2pkg_steam.Workshop;
            S.onConnected(function () { log.push("up"); });
            S.onConnectFailure(function (r, again) { log.push("fail:" + r + ":" + again); });
            W.onItemInstalled(function (e) { log.push("inst:" + e.id); });
            W.download(9).then(function (r) { log.push("done:" + r.ok); }); "" "#);
        crate::v8host::with_host_isolate(|_| {
            dispatch_event(0, 0, 0, 0);
            dispatch_event(2, 0, 3, 1);
            dispatch_event(4, 9, 0, 730);
            dispatch_event(DOWNLOAD, 9, 1, 730);
        }).unwrap();
        assert_eq!(js("st3", "log.join()"), "", "nothing ran inside the busy isolate");
        assert_eq!(pending(), 4);
        sweep();
        assert_eq!(js("st3", "log.join()"), "up,fail:3:true,inst:9,done:true", "delivered in order");
        assert_eq!(pending(), 0);
        shutdown();
    }

    #[test]
    fn unload_drops_listeners_and_waiters() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(ops()));
        create_plugin_context("st4");
        eval_in_context_string("st4", r#"__s2pkg_steam.Workshop.download(11);
            __s2pkg_steam.Steam.onConnected(function () {}); "" "#);
        assert_eq!(WAITERS.with(|w| w.borrow().len()), 1);
        assert!(STEAM_MUX.with(|m| m.borrow().has_subscribers("connected")));
        unload_plugin("st4");
        assert!(WAITERS.with(|w| w.borrow().is_empty()));
        assert!(!STEAM_MUX.with(|m| m.borrow().has_subscribers("connected")));
        assert!(!STEAM_MUX.with(|m| m.borrow().has_subscribers(&waiter_key(11, 0))));
        dispatch_event(DOWNLOAD, 11, 1, 730);   // nobody left to deliver to
        assert_eq!(pending(), 0);
        shutdown();
    }
}
