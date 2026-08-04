//! Engine-generic host-global client-cookie cache: steamid -> { name -> (value, dirty) } plus a
//! per-client `cached` flag. Mirrors the admin/ban caches (cross-context-visible per-client string
//! KV, read/written via natives). Knows nothing about any game; holds no V8 handles.
use std::cell::RefCell;
use std::collections::HashMap;

struct Entry { value: String, dirty: bool, updated: i64 }
#[derive(Default)]
struct ClientCookies { cached: bool, entries: HashMap<String, Entry> }

thread_local! {
    static CACHE: RefCell<HashMap<String, ClientCookies>> = RefCell::new(HashMap::new());
    /// Offline writes (`setAuthId`) queued for the plugin to drain into the DB each frame —
    /// (steamid, name, value, updated). Distinct from the dirty-flag disconnect flush: an offline
    /// SteamID may never connect, so it needs its own persistence path.
    static OFFLINE: RefCell<Vec<(String, String, String, i64)>> = RefCell::new(Vec::new());
}

/// Cache value, or `None` if the client/name is absent (a true miss — distinct from a stored `""`).
pub fn get(steamid: &str, name: &str) -> Option<String> {
    CACHE.with(|c| c.borrow().get(steamid)
        .and_then(|cc| cc.entries.get(name))
        .map(|e| e.value.clone()))
}

/// Write via the API — marks the entry dirty (flushed on disconnect).
pub fn set(steamid: &str, name: &str, value: &str, updated: i64) {
    CACHE.with(|c| {
        let mut m = c.borrow_mut();
        let cc = m.entry(steamid.to_string()).or_default();
        cc.entries.insert(name.to_string(), Entry { value: value.to_string(), dirty: true, updated });
    });
}

/// Write from the DB load — NOT dirty (a loaded value is not a change).
pub fn load(steamid: &str, name: &str, value: &str, updated: i64) {
    CACHE.with(|c| {
        let mut m = c.borrow_mut();
        let cc = m.entry(steamid.to_string()).or_default();
        cc.entries.insert(name.to_string(), Entry { value: value.to_string(), dirty: false, updated });
    });
}

/// The stored `updated` timestamp for a client's cookie, or 0 if absent.
pub fn get_time(steamid: &str, name: &str) -> i64 {
    CACHE.with(|c| c.borrow().get(steamid)
        .and_then(|cc| cc.entries.get(name))
        .map(|e| e.updated)
        .unwrap_or(0))
}

/// The dirty (name, value) pairs for a client — the disconnect flush set.
pub fn get_dirty(steamid: &str) -> Vec<(String, String)> {
    CACHE.with(|c| {
        let m = c.borrow();
        match m.get(steamid) {
            Some(cc) => cc.entries.iter()
                .filter(|(_, e)| e.dirty)
                .map(|(n, e)| (n.clone(), e.value.clone()))
                .collect(),
            None => Vec::new(),
        }
    })
}

/// Write a cookie for a SteamID that may not currently be connected (`SetAuthIdCookie` parity) —
/// updates the cache (so an online client's value is immediately correct) AND queues the write for
/// the plugin to persist directly (an offline SteamID never fires the disconnect flush).
pub fn set_authid(steamid: &str, name: &str, value: &str, updated: i64) {
    set(steamid, name, value, updated);
    OFFLINE.with(|q| q.borrow_mut().push((steamid.to_string(), name.to_string(), value.to_string(), updated)));
}

/// Drain + clear the queued offline writes (called once per frame by the clientprefs plugin).
pub fn take_offline_writes() -> Vec<(String, String, String, i64)> {
    OFFLINE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// Drop a client's entries (on disconnect, after the flush captures the dirty set).
pub fn clear(steamid: &str) {
    CACHE.with(|c| { c.borrow_mut().remove(steamid); });
}

/// Mark a client's cookies loaded (a zero-cookie client is still "cached").
pub fn mark_cached(steamid: &str) {
    CACHE.with(|c| { c.borrow_mut().entry(steamid.to_string()).or_default().cached = true; });
}

pub fn is_cached(steamid: &str) -> bool {
    CACHE.with(|c| c.borrow().get(steamid).map(|cc| cc.cached).unwrap_or(false))
}

/// Drop ALL clients' cookies. Called from `shutdown()` on a core re-init (a same-thread
/// `shutdown()`→`init()` cycle, e.g. a Metamod reload) so stale entries + stale `cached` flags
/// don't survive — mirrors the admin/ban caches, which reset the same way.
pub fn reset() {
    CACHE.with(|c| c.borrow_mut().clear());
    OFFLINE.with(|q| q.borrow_mut().clear());
}


// ---------------------------------------------------------------------------
// The V8 surface: the `__s2_cookie_*` natives over the cache above, the `Cookies.onCached` mux, its
// post-frame dispatch, and this feature's teardown registrations.
//
// The store half of this module is pure Rust and predates the split; the natives arrived here from
// `v8host.rs` under the core-stabilization program, so the feature is now whole in one file — see
// `crate::usermsg` for the shape.
// ---------------------------------------------------------------------------

use crate::v8host::{fan_out, set_native, subscribe_into, Instrument};

thread_local! {
    /// `Cookies.onCached` subscriber mux, keyed by the constant "" (no name dimension — a single
    /// un-keyed list, like `CHAT_MSG_SUBS`). Fanned out post-frame by `dispatch_pending_cached`
    /// (called from `ffi.rs` AFTER `frame_async_drain()` returns, so HOST is free — no re-entrancy
    /// risk from the plugin's own async cookie-load work). `remove_by_owner` on unload; reset on
    /// shutdown.
    static COOKIE_CACHED_MUX: RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = RefCell::new(crate::channels::Channels::new());
    /// Slots queued by `__s2_cookie_dispatch_cached` (called from inside the plugin's `loadCookies`
    /// async continuation, i.e. possibly mid-async-drain) for the NEXT `dispatch_pending_cached()`
    /// post-drain fan-out. Draining + clearing happens with HOST free.
    static COOKIE_CACHED_PENDING: RefCell<Vec<i32>> = RefCell::new(Vec::new());
}


/// `__s2_cookie_get(steamid, name) -> string | undefined` — `undefined` on a true miss (distinct
/// from a stored `""`); the module layer decides the default fallback.
fn s2_cookie_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        match crate::cookies::get(&sid, &name) {
            Some(v) => { if let Some(s) = v8::String::new(scope, &v) { rv.set(s.into()); } }
            None => { rv.set(v8::undefined(scope).into()); }
        }
    }));
}

/// `__s2_cookie_set(steamid, name, value, updated)` — write via the API; marks the entry dirty.
fn s2_cookie_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        crate::cookies::set(&sid, &name, &val, updated);
    }));
}

/// `__s2_cookie_load(steamid, name, value, updated)` — write from the DB load; NOT dirty.
fn s2_cookie_load(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        crate::cookies::load(&sid, &name, &val, updated);
    }));
}

/// `__s2_cookie_get_time(steamid, name) -> number` — the stored `updated` timestamp, or 0 if absent.
fn s2_cookie_get_time(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let t = crate::cookies::get_time(&sid, &name);
        rv.set(v8::Number::new(scope, t as f64).into());
    }));
}

/// `__s2_cookie_get_dirty(steamid) -> { [name]: value }` — the dirty (disconnect flush) set as a JS object.
fn s2_cookie_get_dirty(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let pairs = crate::cookies::get_dirty(&sid);
        let obj = v8::Object::new(scope);
        for (name, value) in pairs.iter() {
            let k = v8::String::new(scope, name).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let v = v8::String::new(scope, value).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            obj.set(scope, k.into(), v.into());
        }
        rv.set(obj.into());
    }));
}

/// `__s2_cookie_clear(steamid)` — drop a client's entries (on disconnect, after the flush captures the dirty set).
fn s2_cookie_clear(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        crate::cookies::clear(&sid);
    }));
}

/// `__s2_cookie_mark_cached(steamid)` — mark a client's cookies loaded (a zero-cookie client is still "cached").
fn s2_cookie_mark_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        crate::cookies::mark_cached(&sid);
    }));
}

/// `__s2_cookie_is_cached(steamid) -> boolean`.
fn s2_cookie_is_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        rv.set(v8::Boolean::new(scope, crate::cookies::is_cached(&sid)).into());
    }));
}

/// `__s2_cookie_set_authid(steamid, name, value, updated)` — `SetAuthIdCookie` parity: write for a
/// SteamID that may not currently be connected (cache write + queue for offline persistence).
fn s2_cookie_set_authid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        crate::cookies::set_authid(&sid, &name, &val, updated);
    }));
}

/// `__s2_cookie_take_offline_writes() -> Array<[steamid, name, value, updated]>` — drain + clear the
/// queued offline writes for the plugin to persist directly (an offline SteamID never fires the
/// disconnect flush).
fn s2_cookie_take_offline_writes(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let writes = crate::cookies::take_offline_writes();
        let out = v8::Array::new(scope, writes.len() as i32);
        for (i, (sid, name, val, updated)) in writes.iter().enumerate() {
            let row = v8::Array::new(scope, 4);
            let sid_s = v8::String::new(scope, sid).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let name_s = v8::String::new(scope, name).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let val_s = v8::String::new(scope, val).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let updated_n = v8::Number::new(scope, *updated as f64);
            row.set_index(scope, 0, sid_s.into());
            row.set_index(scope, 1, name_s.into());
            row.set_index(scope, 2, val_s.into());
            row.set_index(scope, 3, updated_n.into());
            out.set_index(scope, i as u32, row.into());
        }
        rv.set(out.into());
    }));
}
/// Owner-tracked (mirrors `__s2_client_subscribe`); fixed mux key "" (cookies-cached has no name
/// dimension, like `Chat.onMessage`). The handler receives the raw `slot` at dispatch; the
/// `@s2script/cookies` prelude wraps it into a `Client` via `Clients.fromSlot`.
fn s2_cookie_on_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Some((sub_id, _first)) = subscribe_into(scope, &args, &COOKIE_CACHED_MUX, "", 0) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_cookie_dispatch_cached(slot)` — enqueue `slot` for the next post-frame
/// `dispatch_pending_cookie_cached()` fan-out (clientprefs Task 4). No HOST access here (safe to call
/// from inside the plugin's own async `loadCookies` continuation, which may run mid-async-drain); the
/// actual `onCached` handler invocation happens later, once HOST is free.
fn s2_cookie_dispatch_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push(slot));
    }));
}
/// Drain `COOKIE_CACHED_PENDING` and fan each queued slot out to the `Cookies.onCached` subscribers.
/// Called from `ffi.rs`'s Post-frame branch AFTER `frame_async_drain()` returns (HOST is free — no
/// re-entrancy risk from the plugin's own async cookie-load work). Notify-only — each handler is
/// called with the single Integer `slot` and its return is ignored.
///
/// Uses the shared `fan_out` helper rather than the hand-rolled snapshot/borrow/HandleScope/TryCatch
/// loop this used to carry: six sibling dispatchers were converted in A4 and this one was missed.
/// The nesting is unchanged (HOST borrow per slot, subscribers inside), and `fan_out` formats its
/// WARN as `"WARN: {label}: handler '{owner}': {msg}"` — so passing the same label keeps the log
/// output byte-identical too. Keeping the hand-rolled copy would have meant exposing `HOST`,
/// `PLUGINS` and `REGISTRY` out of `v8host` to move this feature, which is a far worse trade.
pub(crate) fn dispatch_pending_cached() {
    let slots: Vec<i32> = COOKIE_CACHED_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if slots.is_empty() { return; }

    // Snapshot once, with the mux borrow released before any JS runs. Fixed key "".
    let snap = COOKIE_CACHED_MUX.with(|m| m.borrow().snapshot(""));
    if snap.is_empty() { return; }

    for slot in slots {
        let _ = fan_out(&snap, "dispatch_pending_cookie_cached", Instrument::none(), |tc| {
            Some(vec![v8::Integer::new(tc, slot).into()])
        });
    }
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_cookie_get", s2_cookie_get);
    set_native(scope, global_obj, "__s2_cookie_set", s2_cookie_set);
    set_native(scope, global_obj, "__s2_cookie_load", s2_cookie_load);
    set_native(scope, global_obj, "__s2_cookie_get_time", s2_cookie_get_time);
    set_native(scope, global_obj, "__s2_cookie_get_dirty", s2_cookie_get_dirty);
    set_native(scope, global_obj, "__s2_cookie_clear", s2_cookie_clear);
    set_native(scope, global_obj, "__s2_cookie_mark_cached", s2_cookie_mark_cached);
    set_native(scope, global_obj, "__s2_cookie_is_cached", s2_cookie_is_cached);
    set_native(scope, global_obj, "__s2_cookie_set_authid", s2_cookie_set_authid);
    set_native(scope, global_obj, "__s2_cookie_take_offline_writes", s2_cookie_take_offline_writes);
    set_native(scope, global_obj, "__s2_cookie_on_cached", s2_cookie_on_cached);
    set_native(scope, global_obj, "__s2_cookie_dispatch_cached", s2_cookie_dispatch_cached);
}

/// The owner-scoped store: pure post-frame JS dispatch — no engine hook to remove.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "COOKIE_CACHED_MUX",
        Box::new(|owner| { COOKIE_CACHED_MUX.with(|m| { m.borrow_mut().remove_by_owner(owner); }); }),
        Box::new(|ids| { COOKIE_CACHED_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| { COOKIE_CACHED_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new()); }),
    );
}

/// Reset on a core re-init. `AfterIsolateDrop`: the pending queue holds plain i32s and the cache is
/// plain Rust — neither holds a V8 handle. (`COOKIE_CACHED_MUX` is an owner-scoped store, above.)
pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    crate::process_singletons::register("COOKIE_CACHED_PENDING", AfterIsolateDrop,
        Box::new(|| COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().clear())));
    crate::process_singletons::register("COOKIES", AfterIsolateDrop, Box::new(reset));
}

#[cfg(test)]
mod tests {
    use super::*;
    // NOTE: CACHE is thread-local + tests run serial (RUST_TEST_THREADS=1); use a unique steamid per
    // test so they don't observe each other's entries.
    #[test]
    fn set_get_and_dirty() {
        set("A1", "color", "red", 0);
        assert_eq!(get("A1", "color"), Some("red".to_string()));
        let d = get_dirty("A1");
        assert_eq!(d, vec![("color".to_string(), "red".to_string())]);
        assert_eq!(get("A1", "missing"), None);
    }
    #[test]
    fn load_is_not_dirty() {
        load("A2", "k", "v", 0);
        assert_eq!(get("A2", "k"), Some("v".to_string()));
        assert!(get_dirty("A2").is_empty(), "a loaded value is not dirty");
        set("A2", "k2", "v2", 0);   // a later set IS dirty
        assert_eq!(get_dirty("A2"), vec![("k2".to_string(), "v2".to_string())]);
    }
    #[test]
    fn clear_removes_client() {
        set("A3", "k", "v", 0);
        clear("A3");
        assert_eq!(get("A3", "k"), None);
        assert!(get_dirty("A3").is_empty());
    }
    #[test]
    fn cached_flag_tracks() {
        assert!(!is_cached("A4"));
        mark_cached("A4");        // a zero-cookie client can still be cached
        assert!(is_cached("A4"));
    }
    #[test]
    fn reset_clears_all() {
        set("A5", "k", "v", 0);
        mark_cached("A5");
        reset();
        assert_eq!(get("A5", "k"), None);
        assert!(!is_cached("A5"));   // stale cached flag gone
    }
    /// Task 2: a stored `""` is a HIT (`Some("")`), distinct from a true miss (`None`) — the
    /// module-layer empty-string-vs-default bug this task fixes.
    #[test]
    fn empty_string_is_a_hit_not_a_miss() {
        set("A6", "k", "", 0);
        assert_eq!(get("A6", "k"), Some("".to_string()));
        assert_eq!(get("A6", "missing"), None);
    }
    /// Task 2: `get_time` returns the stored `updated` for both `set` and `load`, and 0 when absent.
    #[test]
    fn get_time_reads_back_updated() {
        assert_eq!(get_time("A7", "k"), 0);   // absent
        set("A7", "k", "v", 1_700_000_000);
        assert_eq!(get_time("A7", "k"), 1_700_000_000);
        load("A7", "k2", "v2", 1_600_000_000);
        assert_eq!(get_time("A7", "k2"), 1_600_000_000);
    }
    /// Task 3: `set_authid` writes the cache (an online client immediately sees the value) AND
    /// queues the write for offline persistence; `take_offline_writes` drains + clears (a second
    /// take is empty).
    #[test]
    fn set_authid_writes_cache_and_queues_offline_write() {
        set_authid("A8", "k", "v", 1_234_567_890);
        assert_eq!(get("A8", "k"), Some("v".to_string()));   // cache write visible immediately
        let writes = take_offline_writes();
        assert_eq!(writes, vec![("A8".to_string(), "k".to_string(), "v".to_string(), 1_234_567_890)]);
        assert!(take_offline_writes().is_empty(), "a second take drains nothing new");
    }
}

// The V8-surface tests, over the SHARED in-isolate harness (`v8host::frame_tests`). Kept separate
// from the pure-Rust store tests above, which need no isolate — see `crate::usermsg` for the shape.
#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, load_body, logger,
        read_global_string, read_i32_global_in, LOG};
    use crate::v8host::{create_plugin_context, eval_in_context, init, shutdown, unload_plugin};
    /// clientprefs Task 2: `__s2_cookie_*` natives round-trip through `crate::cookies` — a loaded
    /// value is NOT dirty, a set value IS, `get_dirty` returns only the dirty entries, and
    /// `is_cached` reflects `mark_cached`.
    #[test]
    fn cookie_natives_round_trip() {
        let _ = init(dummy_logger());
        load_body("ck", r#"
            __s2_cookie_load("S1", "a", "1", 111);    // loaded, not dirty
            __s2_cookie_set("S1", "b", "2", 222);     // set, dirty
            __s2_cookie_mark_cached("S1");
            var dirty = __s2_cookie_get_dirty("S1");
            globalThis.__out = __s2_cookie_get("S1","a") + "," + __s2_cookie_get("S1","b")
                + "," + __s2_cookie_is_cached("S1") + "," + Object.keys(dirty).join("|") + "=" + dirty.b;
        "#, "{}");
        assert_eq!(read_global_string("ck", "__out"), "1,2,true,b=2"); // only b is dirty
        shutdown();
    }

    /// clientprefs Task 2: `__s2_cookie_get` returns `undefined` (not `""`) on a true miss, so a
    /// stored `""` reads back as a real hit distinct from an absent name; `__s2_cookie_get_time`
    /// reads back the `updated` passed to `set`/`load`, and is 0 when absent.
    #[test]
    fn cookie_natives_empty_string_and_get_time() {
        let _ = init(dummy_logger());
        load_body("ck2", r#"
            __s2_cookie_set("S2", "empty", "", 12345);
            var missing = __s2_cookie_get("S2", "nope");
            var empty = __s2_cookie_get("S2", "empty");
            globalThis.__out = (missing === undefined) + "," + (empty === "") + ","
                + __s2_cookie_get_time("S2", "empty") + "," + __s2_cookie_get_time("S2", "nope");
        "#, "{}");
        assert_eq!(read_global_string("ck2", "__out"), "true,true,12345,0");
        shutdown();
    }

    /// clientprefs Task 3: the `@s2script/cookies` module — `Cookies.register` is idempotent,
    /// `get`/`set` route through the cache with a default fallback, and bots (`steamId === "0"`)
    /// are skipped entirely by both `get` (returns the default) and `set` (a no-op — the raw
    /// native cache stays empty for that steamid).
    #[test]
    fn clientprefs_module_get_set_default_and_bot_skip() {
        let _ = init(dummy_logger());
        load_body("cp", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("hud", { default: "white" });
            var real = { steamId: "S9" };
            var bot  = { steamId: "0" };
            globalThis.__out = Cookies.get(real, c)                 // default (empty cache) -> "white"
                + "," + (function(){ Cookies.set(real, c, "red"); return Cookies.get(real, c); })()  // "red"
                + "," + Cookies.get(bot, c)                          // bot -> default "white"
                + "," + (function(){ Cookies.set(bot, c, "x"); return __s2_cookie_get("0","hud"); })(); // bot set is a no-op -> undefined
        "#, "{}");
        assert_eq!(read_global_string("cp", "__out"), "white,red,white,undefined");
        shutdown();
    }

    /// clientprefs Task 2 (module layer): a `Cookies.set(client, cookie, "")` followed by
    /// `Cookies.get` returns `""` — NOT the cookie's default — the empty-string-vs-miss fix; and
    /// `Cookies.getTime` reads back a nonzero timestamp after a set, 0 before any set, and 0 for a bot.
    #[test]
    fn clientprefs_module_empty_string_and_get_time() {
        let _ = init(dummy_logger());
        load_body("cp2", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("nickname", { default: "Anonymous" });
            var real = { steamId: "S10" };
            var bot  = { steamId: "0" };
            var beforeSetTime = Cookies.getTime(real, c);      // 0 — never set
            Cookies.set(real, c, "");
            var afterEmptySet = Cookies.get(real, c);          // "" not "Anonymous"
            var afterSetTime = Cookies.getTime(real, c);       // nonzero now
            var botTime = Cookies.getTime(bot, c);             // 0 — bots skipped
            globalThis.__out = beforeSetTime + "," + (afterEmptySet === "") + "," + (afterSetTime > 0) + "," + botTime;
        "#, "{}");
        assert_eq!(read_global_string("cp2", "__out"), "0,true,true,0");
        shutdown();
    }

    /// clientprefs Task 3: `__s2_cookie_set_authid` writes the cache (a subsequent `__s2_cookie_get`
    /// sees it immediately) AND queues the write, drained via `__s2_cookie_take_offline_writes` as a
    /// `[steamid,name,value,updated]` row; a second take is empty.
    #[test]
    fn cookie_set_authid_native_writes_cache_and_queues_offline_write() {
        let _ = init(dummy_logger());
        load_body("ck3", r#"
            __s2_cookie_set_authid("S11", "k", "v", 999);
            var cached = __s2_cookie_get("S11", "k");
            var writes = __s2_cookie_take_offline_writes();
            var again = __s2_cookie_take_offline_writes();
            globalThis.__out = cached + "," + writes.length + "," + writes[0].join("|") + "," + again.length;
        "#, "{}");
        assert_eq!(read_global_string("ck3", "__out"), "v,1,S11|k|v|999,0");
        shutdown();
    }

    /// clientprefs Task 3 (module layer): `Cookies.setAuthId` writes for a SteamID not passed as a
    /// `Client` at all (offline parity) — a subsequent `Cookies.get` on that steamid sees the value,
    /// and it is a no-op for "0" (bot/unset).
    #[test]
    fn clientprefs_module_set_authid_offline_and_bot_skip() {
        let _ = init(dummy_logger());
        load_body("cp3", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("hud", { default: "white" });
            Cookies.setAuthId("S12", c, "blue");
            var real = { steamId: "S12" };
            var seenByClient = Cookies.get(real, c);           // "blue" — the offline write is visible
            Cookies.setAuthId("0", c, "x");                    // bot steamid — no-op
            var botRaw = __s2_cookie_get("0", "hud");
            globalThis.__out = seenByClient + "," + botRaw;
        "#, "{}");
        assert_eq!(read_global_string("cp3", "__out"), "blue,undefined");
        shutdown();
    }

    /// clientprefs Task 4: `Cookies.onCached` (post-drain fan-out). Subscribing via the raw native
    /// `__s2_cookie_on_cached` and enqueuing a slot via `__s2_cookie_dispatch_cached` does NOT run the
    /// handler immediately (only `dispatch_pending_cached()` — the ffi.rs post-`frame_async_drain`
    /// call site — does); calling it fans the queued slot out to the handler exactly once, and a second
    /// call (now-empty queue) does not re-run it. After `unload_plugin` (remove_by_owner teardown), a
    /// further enqueue+dispatch is a safe no-op.
    #[test]
    fn cookie_cached_dispatch_fans_out_queued_slots() {
        let _ = init(dummy_logger());
        load_body("ck4", r#"
            __s2_cookie_on_cached(function (slot) {
                globalThis.__ck_ran = (globalThis.__ck_ran || 0) + 1;
                globalThis.__ck_slot = slot;
            });
            __s2_cookie_dispatch_cached(5);
        "#, "{}");

        // Enqueuing alone must not have run the handler yet.
        assert_eq!(read_i32_global_in("ck4", "__ck_ran"), 0, "enqueue must not itself dispatch");

        dispatch_pending_cached();
        assert_eq!(read_i32_global_in("ck4", "__ck_ran"), 1, "handler must run exactly once");
        assert_eq!(read_i32_global_in("ck4", "__ck_slot"), 5, "handler must receive the queued slot");

        // An empty queue: a further dispatch is a no-op (does not re-run the handler).
        dispatch_pending_cached();
        assert_eq!(read_i32_global_in("ck4", "__ck_ran"), 1, "an empty queue must not re-run the handler");

        // Teardown: unload removes ck4's subscription; a later enqueue+dispatch is a safe no-op
        // (must not crash even though the context is disposed).
        unload_plugin("ck4");
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push(9));
        dispatch_pending_cached();
        shutdown();
    }
}
