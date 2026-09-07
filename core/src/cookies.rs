//! Engine-generic host-global client-cookie cache: steamid -> { name -> (value, dirty) } plus a
//! per-client `cached` flag. Mirrors the admin/ban caches (cross-context-visible per-client string
//! KV, read/written via natives). Knows nothing about any game; holds no V8 handles.
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone)]
struct Entry { value: String, dirty: bool, updated: i64 }
#[derive(Default, Clone)]
struct ClientCookies { cached: bool, entries: HashMap<String, Entry> }

thread_local! {
    static CACHE: RefCell<HashMap<String, ClientCookies>> = RefCell::new(HashMap::new());

}

/// Cache value, or `None` if the client/name is absent (a true miss — distinct from a stored `""`).
pub fn get(steamid: &str, name: &str) -> Option<String> {
    CACHE.with(|c| c.borrow().get(steamid)
        .and_then(|cc| cc.entries.get(name))
        .map(|e| e.value.clone()))
}

/// Reserve persistence ownership before changing the account cache.
pub fn set(steamid: &str, name: &str, value: &str, updated: i64) -> bool {
    if !outbox().accept(steamid, name, value, updated) { return false; }
    cache_set(steamid, name, value, updated); true
}
fn cache_set(steamid: &str, name: &str, value: &str, updated: i64) {
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

/// Dirty cache entries (inspection only; persistence belongs exclusively to the outbox).
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
/// reserves the same outbox as online writes, then updates matching live session caches.
pub fn set_authid(steamid: &str, name: &str, value: &str, updated: i64) -> bool {
    if !set(steamid, name, value, updated) { return false; }
    SESSIONS.with(|s| {
        for session in s.borrow_mut().values_mut().filter(|s| s.steam_id == steamid) {
            session.cookies.entries.insert(name.into(), Entry { value: value.into(), dirty: true, updated });
        }
    });
    true
}

/// Drop account cache entries without altering persistence ownership.
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
/// don't survive. Accepted outbox payloads, accounting and IDs survive; only leases are reclaimed.
pub fn reset() {
    CACHE.with(|c| c.borrow_mut().clear());
    SESSIONS.with(|s| s.borrow_mut().clear());
    outbox().reclaim(None);
}


// Online caches belong to connections. Offline account writes remain a separate API.
#[derive(Default)]
struct Session { steam_id: String, cookies: ClientCookies }
thread_local! {
    static SESSIONS: RefCell<HashMap<(i32, u64), Session>> = RefCell::new(HashMap::new());
}
pub(crate) fn retire(slot: i32, token: u64) {
    SESSIONS.with(|s| { s.borrow_mut().remove(&(slot, token)); });
}

fn session_op(slot: i32, token: u64, op: &str, data: serde_json::Value) -> serde_json::Value {
    use serde_json::{json, Value};
    if !crate::client::matches(slot, token) {
        return Value::Null;
    }
    let sid = data["steamId"].as_str().unwrap_or("0");
    if sid.is_empty() || sid == "0" {
        return Value::Null;
    }
    // Admission precedes even creation of a session; rejection cannot change cache state.
    if op == "set" {
        let name = data["name"].as_str().unwrap_or("");
        let value = data["value"].as_str().unwrap_or("");
        let updated = data["updated"].as_i64().unwrap_or(0);
        if !outbox().accept(sid, name, value, updated) {
            return json!(false);
        }
        // A prior offline write may seed a later connection from CACHE. Refresh that existing
        // snapshot without retaining a second account cache for every online-only connection.
        CACHE.with(|c| {
            if let Some(e) = c
                .borrow_mut()
                .get_mut(sid)
                .and_then(|cc| cc.entries.get_mut(name))
            {
                *e = Entry {
                    value: value.into(),
                    dirty: true,
                    updated,
                };
            }
        });
    }
    SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        let session = sessions.entry((slot, token)).or_insert_with(|| Session {
            steam_id: sid.into(),
            cookies: CACHE.with(|c| c.borrow().get(sid).cloned().unwrap_or_default()),
        });
        let cache = &mut session.cookies;
        let name = data["name"].as_str().unwrap_or("");
        match op {
            "get" => cache
                .entries
                .get(name)
                .map_or(Value::Null, |e| json!(e.value)),
            "time" => json!(cache.entries.get(name).map_or(0, |e| e.updated)),
            "cached" => json!(cache.cached),
            "set" => {
                cache.entries.insert(
                    name.into(),
                    Entry {
                        value: data["value"].as_str().unwrap_or("").into(),
                        dirty: true,
                        updated: data["updated"].as_i64().unwrap_or(0),
                    },
                );
                json!(true)
            }
            "load" => {
                if let Some(rows) = data["rows"].as_array() {
                    for row in rows {
                        let name = row["name"].as_str().unwrap_or("");
                        if cache.entries.get(name).is_some_and(|e| e.dirty) {
                            continue;
                        }
                        cache.entries.insert(
                            name.into(),
                            Entry {
                                value: row["value"].as_str().unwrap_or("").into(),
                                dirty: false,
                                updated: row["updated"].as_i64().unwrap_or(0),
                            },
                        );
                    }
                }
                cache.cached = true;
                queue_cached(slot, token);
                json!(true)
            }
            _ => Value::Null,
        }
    })
}
fn s2_cookie_session(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let token = args.get(1).to_rust_string_lossy(scope).parse().unwrap_or(0);
        let op = args.get(2).to_rust_string_lossy(scope);
        let data = serde_json::from_str(&args.get(3).to_rust_string_lossy(scope)).unwrap_or(serde_json::Value::Null);
        let value = session_op(slot, token, &op, data).to_string();
        if let Some(s) = v8::String::new(scope, &value) { rv.set(s.into()); }
    }));
}
// One bounded persistence owner for online, retired and offline changes. Payloads survive
// same-process shutdown/init. Nothing here contains V8 handles or engine pointers.
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
type CookieKey = (String, String);
type WriterOwner = (String, u64);
#[derive(Clone)]
pub(crate) struct OutboxPolicy {
    pub versions: usize, pub bytes: usize, pub write_bytes: usize,
    pub batch_items: usize, pub batch_bytes: usize, pub concurrent: usize,
}
impl Default for OutboxPolicy {
    fn default() -> Self {
        let p = crate::async_limits::policy();
        Self {
            versions: p.cookie_versions,
            bytes: p.cookie_bytes,
            write_bytes: p.cookie_write_bytes,
            batch_items: 4,
            batch_bytes: 256 * 1024,
            concurrent: 4,
        }
    }
}
#[derive(Clone)]
struct Payload { steam_id: String, name: String, value: String, updated: i64, revision: i64, covers_from: i64 }
impl Payload { fn bytes(&self) -> usize { 128 + self.steam_id.len() + self.name.len() + self.value.len() } }
#[derive(Clone)]
struct Attempt { id: u64, owner: WriterOwner, payload: Arc<Payload> }
#[derive(Default)]
struct Pending { ready: Option<Arc<Payload>>, attempt: Option<Attempt>, failures: u32, next_attempt: u64 }
struct Outbox {
    entries: HashMap<CookieKey, Pending>, order: VecDeque<CookieKey>, policy: OutboxPolicy,
    revision: i64, lease_id: u64, epoch: String, clock: Instant,
    accepted: u64, coalesced: u64, rejected: u64, retries: u64, stale_acks: u64,
}
impl Default for Outbox {
    fn default() -> Self { Self { entries: HashMap::new(), order: VecDeque::new(), policy: OutboxPolicy::default(), revision: 0, lease_id: 0,
        epoch: format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()), clock: Instant::now(), accepted: 0, coalesced: 0, rejected: 0, retries: 0, stale_acks: 0 } }
}
static OUTBOX: OnceLock<Mutex<Outbox>> = OnceLock::new();
fn outbox() -> std::sync::MutexGuard<'static, Outbox> { OUTBOX.get_or_init(|| Mutex::new(Outbox::default())).lock().unwrap_or_else(|e| e.into_inner()) }
impl Outbox {
    fn now(&self) -> u64 { self.clock.elapsed().as_millis().min(u64::MAX as u128) as u64 }
    fn usage(&self) -> (usize, usize) {
        self.entries.values().fold((0,0), |(mut n,mut b), p| { for v in p.ready.iter().chain(p.attempt.iter().map(|a| &a.payload)) { n+=1; b+=v.bytes(); } (n,b) })
    }
    fn accept(&mut self, sid: &str, name: &str, value: &str, updated: i64) -> bool {
        let key = (sid.to_owned(), name.to_owned());
        let old = self.entries.get(&key).and_then(|p| p.ready.as_ref());
        let charge = 128usize.saturating_add(sid.len()).saturating_add(name.len()).saturating_add(value.len());
        let (n,b) = self.usage();
        if sid.is_empty() || sid == "0" || charge > self.policy.write_bytes || n + usize::from(old.is_none()) > self.policy.versions
            || b.saturating_sub(old.map_or(0, |v| v.bytes())).saturating_add(charge) > self.policy.bytes || self.revision == i64::MAX {
            self.rejected = self.rejected.saturating_add(1); return false;
        }
        self.revision += 1;
        let covers_from = old.map_or(self.revision, |v| v.covers_from);
        if old.is_some() { self.coalesced = self.coalesced.saturating_add(1); }
        if !self.entries.contains_key(&key) { self.order.push_back(key.clone()); }
        self.entries.entry(key).or_default().ready = Some(Arc::new(Payload { steam_id: sid.into(), name: name.into(), value: value.into(), updated, revision: self.revision, covers_from }));
        self.accepted = self.accepted.saturating_add(1); true
    }
    fn fence(&self, _sid: &str) -> i64 { self.revision }
    fn done(&self, sid: &str, fence: i64) -> bool {
        self.entries.iter().filter(|((s,_),_)| s == sid).all(|(_,p)| p.ready.iter().chain(p.attempt.iter().map(|a| &a.payload)).all(|v| v.covers_from > fence))
    }
    fn lease(&mut self, owner: WriterOwner, max_items: usize, max_bytes: usize, now: u64) -> Vec<Attempt> {
        let mut busy: HashSet<String> = self.entries.iter().filter(|(_,p)| p.attempt.is_some()).map(|((sid,_),_)| sid.clone()).collect();
        let limit = max_items.min(self.policy.batch_items).min(self.policy.concurrent.saturating_sub(busy.len()));
        let mut oldest: HashMap<String, i64> = HashMap::new();
        for ((sid,_), p) in &self.entries { for v in p.ready.iter().chain(p.attempt.iter().map(|a| &a.payload)) { oldest.entry(sid.clone()).and_modify(|r| *r = (*r).min(v.covers_from)).or_insert(v.covers_from); } }
        let mut bytes = 0; let mut result = Vec::new();
        for _ in 0..self.order.len() {
            let key = self.order.pop_front().unwrap();
            let p = self.entries.get_mut(&key).unwrap();
            if result.len() < limit && !busy.contains(&key.0) && p.next_attempt <= now && p.attempt.is_none() {
                if let Some(payload) = p.ready.as_ref() {
                    if oldest.get(&key.0) == Some(&payload.covers_from) && bytes + payload.bytes() <= max_bytes.min(self.policy.batch_bytes) && self.lease_id < u64::MAX {
                        self.lease_id += 1; bytes += payload.bytes();
                        let a = Attempt { id: self.lease_id, owner: owner.clone(), payload: p.ready.take().unwrap() };
                        p.attempt = Some(a.clone()); busy.insert(key.0.clone()); result.push(a);
                    }
                }
            }
            self.order.push_back(key);
        }
        // Move admitted keys behind skipped keys so continuously-written accounts cannot starve others.
        for a in &result { let key = (a.payload.steam_id.clone(), a.payload.name.clone()); self.order.retain(|k| k != &key); self.order.push_back(key); }
        result
    }
    fn ack(&mut self, owner: &WriterOwner, id: u64, revision: i64, success: bool, now: u64) -> bool {
        let key = self
            .entries
            .iter()
            .find(|(_, p)| {
                p.attempt
                    .as_ref()
                    .is_some_and(|a| a.id == id && &a.owner == owner && a.payload.revision == revision)
            })
            .map(|(k, _)| k.clone());
        let Some(key) = key else {
            self.stale_acks = self.stale_acks.saturating_add(1);
            return false;
        };
        let p = self.entries.get_mut(&key).unwrap();
        let a = p.attempt.take().unwrap();
        if success {
            p.failures = 0;
            p.next_attempt = 0;
        } else {
            Self::restore(p, a.payload);
            p.next_attempt = now.saturating_add((100u64 << p.failures.min(6)).min(5000));
            p.failures = p.failures.saturating_add(1);
            self.retries = self.retries.saturating_add(1);
        }
        if p.ready.is_none() {
            self.entries.remove(&key);
            self.order.retain(|k| k != &key);
        }
        if success && !self.entries.contains_key(&key) {
            // Evict this acknowledged key even if another key for the account is still pending:
            // retaining whole-account history behind one slow key would grow without limit.
            // Active connection snapshots are separate; a newer same-key obligation prevents eviction.
            CACHE.with(|c| {
                let mut c = c.borrow_mut();
                if let Some(cc) = c.get_mut(&key.0) {
                    cc.entries.remove(&key.1);
                    if cc.entries.is_empty() {
                        c.remove(&key.0);
                    }
                }
            });
        }
        true
    }
    fn restore(p: &mut Pending, payload: Arc<Payload>) {
        if let Some(v) = p.ready.as_mut() { Arc::make_mut(v).covers_from = v.covers_from.min(payload.covers_from); }
        else { p.ready = Some(payload); }
    }
    fn reclaim(&mut self, owner: Option<&str>) {
        for p in self.entries.values_mut() {
            if p.attempt.as_ref().is_some_and(|a| owner.is_none_or(|o| a.owner.0 == o)) {
                let a = p.attempt.take().unwrap(); Self::restore(p, a.payload); p.next_attempt = 0;
            }
        }
    }
    fn stats(&self) -> serde_json::Value {
        let (versions,bytes) = self.usage(); let leased = self.entries.values().filter(|p| p.attempt.is_some()).count();
        serde_json::json!({"ready":versions-leased,"leased":leased,"bytes":bytes,"accepted":self.accepted,"coalesced":self.coalesced,"rejected":self.rejected,"retries":self.retries,"staleAcks":self.stale_acks})
    }
}

// ---------------------------------------------------------------------------
// The V8 surface: the `__s2_cookie_*` natives over the cache above, the `Cookies.onCached` mux, its
// post-frame dispatch, and this feature's teardown registrations.
//
// The store half of this module is pure Rust and predates the split; the natives arrived here from
// `v8host.rs` under the core-stabilization program, so the feature is now whole in one file — see
// `crate::usermsg` for the shape.
// ---------------------------------------------------------------------------

use crate::dispatch::{fan_out, Instrument};
use crate::v8host::{set_native, subscribe_into};

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
    static COOKIE_CACHED_PENDING: RefCell<Vec<(i32, u64)>> = RefCell::new(Vec::new());
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
fn s2_cookie_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        rv.set(v8::Boolean::new(scope, crate::cookies::set(&sid, &name, &val, updated)).into());
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
fn s2_cookie_set_authid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        rv.set(v8::Boolean::new(scope, crate::cookies::set_authid(&sid, &name, &val, updated)).into());
    }));
}

fn s2_cookie_lease(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(owner) = crate::v8host::jobs_owner_tag(scope) else { return; };
    let items = args.get(0).uint32_value(scope).unwrap_or(0) as usize;
    let bytes = args.get(1).uint32_value(scope).unwrap_or(0) as usize;
    let mut q = outbox(); let now = q.now();
    let rows: Vec<_> = q.lease(owner, items, bytes, now).iter().map(|a| serde_json::json!({
        "leaseId":a.id.to_string(), "revision":a.payload.revision.to_string(), "writerEpoch":q.epoch,
        "steamId":a.payload.steam_id,"name":a.payload.name,"value":a.payload.value,"updated":a.payload.updated
    })).collect();
    if let Some(s) = v8::String::new(scope, &serde_json::to_string(&rows).unwrap()) { rv.set(s.into()); }
}
fn s2_cookie_ack(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(owner) = crate::v8host::jobs_owner_tag(scope) else { return; };
    let id = args.get(0).to_rust_string_lossy(scope).parse().unwrap_or(0);
    let rev = args.get(1).to_rust_string_lossy(scope).parse().unwrap_or(0);
    let success = args.get(2).is_true(); let mut q = outbox(); let now = q.now();
    rv.set(v8::Boolean::new(scope, q.ack(&owner, id, rev, success, now)).into());
}
fn s2_cookie_account_fence(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let sid = args.get(0).to_rust_string_lossy(scope);
    if let Some(s) = v8::String::new(scope, &outbox().fence(&sid).to_string()) { rv.set(s.into()); }
}
fn s2_cookie_fence_done(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let sid = args.get(0).to_rust_string_lossy(scope);
    let fence = args.get(1).to_rust_string_lossy(scope).parse().unwrap_or(i64::MAX);
    rv.set(v8::Boolean::new(scope, outbox().done(&sid, fence)).into());
}
fn s2_cookie_outbox_stats(scope: &mut v8::PinScope, _: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    if let Some(s) = v8::String::new(scope, &outbox().stats().to_string()) { rv.set(s.into()); }
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
fn s2_cookie_dispatch_cached(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let token = args.get(1).to_rust_string_lossy(scope).parse().unwrap_or(0);
        if crate::client::matches(slot, token) {
            queue_cached(slot, token);
        }
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
fn queue_cached(slot: i32, token: u64) {
    COOKIE_CACHED_PENDING.with(|q| {
        let mut q = q.borrow_mut();
        q.retain(|(s, t)| crate::client::matches(*s, *t));
        if !q.contains(&(slot, token)) {
            q.push((slot, token));
        }
    });
    crate::v8host::refresh_detour();
}
pub(crate) fn pending_cached() -> bool {
    COOKIE_CACHED_PENDING.with(|q| !q.borrow().is_empty())
}
pub(crate) fn dispatch_pending_cached() {
    for _ in 0..crate::async_limits::policy().frame_items {
        if !dispatch_one_cached() {
            break;
        }
    }
}
pub(crate) fn dispatch_one_cached() -> bool {
    if !pending_cached() || !crate::async_limits::can_deliver(32, false) {
        return false;
    }
    let (slot, token) = COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().remove(0));
    crate::async_limits::deliver(32);
    let snap = COOKIE_CACHED_MUX.with(|m| m.borrow().snapshot(""));
    if !snap.is_empty() {
        let _ = fan_out(
            &snap,
            "dispatch_pending_cookie_cached",
            Instrument::none(),
            |tc| {
                if !crate::client::matches(slot, token) {
                    return None;
                }
                let token = v8::String::new(tc, &token.to_string())?;
                Some(vec![v8::Integer::new(tc, slot).into(), token.into()])
            },
        );
    }
    true
}


/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_cookie_lease", s2_cookie_lease);
    set_native(scope, global_obj, "__s2_cookie_ack", s2_cookie_ack);
    set_native(scope, global_obj, "__s2_cookie_account_fence", s2_cookie_account_fence);
    set_native(scope, global_obj, "__s2_cookie_fence_done", s2_cookie_fence_done);
    set_native(scope, global_obj, "__s2_cookie_outbox_stats", s2_cookie_outbox_stats);
    set_native(scope, global_obj, "__s2_cookie_session", s2_cookie_session);
    set_native(scope, global_obj, "__s2_cookie_get", s2_cookie_get);
    set_native(scope, global_obj, "__s2_cookie_set", s2_cookie_set);
    set_native(scope, global_obj, "__s2_cookie_load", s2_cookie_load);
    set_native(scope, global_obj, "__s2_cookie_get_time", s2_cookie_get_time);
    set_native(scope, global_obj, "__s2_cookie_get_dirty", s2_cookie_get_dirty);
    set_native(scope, global_obj, "__s2_cookie_clear", s2_cookie_clear);
    set_native(scope, global_obj, "__s2_cookie_mark_cached", s2_cookie_mark_cached);
    set_native(scope, global_obj, "__s2_cookie_is_cached", s2_cookie_is_cached);
    set_native(scope, global_obj, "__s2_cookie_set_authid", s2_cookie_set_authid);
    set_native(scope, global_obj, "__s2_cookie_on_cached", s2_cookie_on_cached);
    set_native(scope, global_obj, "__s2_cookie_dispatch_cached", s2_cookie_dispatch_cached);
}

/// The owner-scoped store: pure post-frame JS dispatch — no engine hook to remove.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "COOKIE_CACHED_MUX",
        Box::new(|owner| { outbox().reclaim(Some(owner)); COOKIE_CACHED_MUX.with(|m| { m.borrow_mut().remove_by_owner(owner); }); }),
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
    #[test]
    fn online_change_supersedes_offline_cache_when_same_account_reconnects() {
        reset(); assert!(set_authid("reconnect", "k", "offline-old", 1));
        let a=crate::client::begin(4);
        session_op(4,a,"set",serde_json::json!({"steamId":"reconnect","name":"k","value":"online-new"}));
        crate::client::end(4,a); let b=crate::client::begin(4);
        session_op(4,b,"load",serde_json::json!({"steamId":"reconnect","rows":[{"name":"k","value":"online-new"}]}));
        assert_eq!(session_op(4,b,"get",serde_json::json!({"steamId":"reconnect","name":"k"})),"online-new");
        crate::client::end(4,b); reset();
    }
    #[test]
    fn online_set_reports_admission() {
        reset();
        let token = crate::client::begin(4);
        assert_eq!(session_op(4, token, "set", serde_json::json!({"steamId":"admission", "name":"k", "value":"v"})), serde_json::json!(true));
        crate::client::end(4, token);
    }
    #[test]
    fn same_account_reconnect_fences_load_and_detaches_dirty_state() {
        reset();
        let a = crate::client::begin(4);
        session_op(4, a, "set", serde_json::json!({"steamId":"same", "name":"color", "value":"red", "updated":7}));
        crate::client::end(4, a);
        let b = crate::client::begin(4);
        session_op(4, b, "set", serde_json::json!({"steamId":"same", "name":"color", "value":"blue", "updated":8}));
        assert_eq!(session_op(4, a, "load", serde_json::json!({"steamId":"same", "rows":[{"name":"color","value":"stale","updated":1}]})), serde_json::Value::Null);
        retire(4, a); // delayed A disconnect cannot clear B
        assert_eq!(session_op(4, b, "get", serde_json::json!({"steamId":"same", "name":"color"})), "blue");
        { let q = outbox(); assert!(!q.done("same", q.revision)); }
        // A DB snapshot cannot overwrite a value authored while the query was pending.
        session_op(4, b, "load", serde_json::json!({"steamId":"same", "rows":[{"name":"color","value":"db-old","updated":2}]}));
        assert_eq!(session_op(4, b, "get", serde_json::json!({"steamId":"same", "name":"color"})), "blue");
        crate::client::end(4, b);
        reset();
    }
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

}

// The V8-surface tests, over the SHARED in-isolate harness (`v8host::frame_tests`). Kept separate
// from the pure-Rust store tests above, which need no isolate — see `crate::usermsg` for the shape.
#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, load_body, logger,
        read_global_string, read_i32_global_in, LOG};
    use crate::v8host::{create_plugin_context, eval_in_context, init, shutdown, unload_plugin};
    #[test]
    fn admission_rejection_preserves_online_offline_cache_and_disconnect() {
        init(dummy_logger()).unwrap();
        connect_cookie_client(c"pressure");
        load_body("pressure-writer", r#"
            var {Cookies}=require('@s2script/cookies'); var c=Cookies.register('k');
            var client=new __s2pkg_clients.Client(3);
            globalThis.accepted=Cookies.set(client,c,'old');
            globalThis.probe=()=>JSON.stringify([accepted,Cookies.set(client,c,'new'),Cookies.setAuthId('pressure',c,'offline'),Cookies.get(client,c),Cookies.set({steamId:'0'},c,'bot')]);
        "#, "{}");
        let old_policy=outbox().policy.clone(); outbox().policy.write_bytes=1;
        let result=eval_in_context_string("pressure-writer", "probe()");
        outbox().policy=old_policy;
        assert_eq!(result, "[true,false,false,\"old\",false]");
        let token=crate::client::generation(3); crate::client::end(3,token);
        { let q=outbox(); assert!(!q.done("pressure",q.revision)); }
        assert!(!SESSIONS.with(|s| s.borrow().contains_key(&(3,token))));
        shutdown();
    }
    #[test]
    fn owner_reload_and_core_reinit_keep_epoch_payloads_and_reject_late_sql_actor() {
        // Isolate this scenario's queue, without resetting its process-stable allocators/epoch.
        { let mut q=outbox(); q.entries.clear(); q.order.clear(); }
        init(dummy_logger()).unwrap();
        load_body("writer", r#"
            __s2_cookie_set_authid('reload-account','k','old',1);
            globalThis.old=JSON.parse(__s2_cookie_lease(4,262144))[0];
        "#, "{}");
        let old: serde_json::Value=serde_json::from_str(&eval_in_context_string("writer","JSON.stringify(old)")).unwrap();
        let sql=include_str!("../../plugins/clientprefs/src/plugin.ts").split("const UPSERT = \"").nth(1).unwrap().split('"').next().unwrap().to_owned();
        let path=std::env::temp_dir().join(format!("s2-cookie-reload-{}-{}.sqlite",std::process::id(),old["leaseId"].as_str().unwrap()));
        let db=rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE cookies(steamid TEXT,name TEXT,value TEXT,updated INTEGER,writer_epoch TEXT,revision INTEGER,PRIMARY KEY(steamid,name))").unwrap();
        let (release,wait)=std::sync::mpsc::channel();
        let worker_path=path.clone(); let worker_sql=sql.clone(); let worker_old=old.clone();
        let worker=std::thread::spawn(move || { let conn=rusqlite::Connection::open(worker_path).unwrap(); wait.recv().unwrap();
            conn.execute(&worker_sql,rusqlite::params!["reload-account","k","old",1,worker_old["writerEpoch"].as_str().unwrap(),worker_old["revision"].as_str().unwrap()]).unwrap(); });
        assert!(set_authid("reload-account","k","new",1));
        unload_plugin("writer");
        shutdown(); init(dummy_logger()).unwrap();
        load_body("writer", "globalThis.current=JSON.parse(__s2_cookie_lease(4,262144))[0];", "{}");
        let current:serde_json::Value=serde_json::from_str(&eval_in_context_string("writer","JSON.stringify(current)")).unwrap();
        assert_eq!(current["value"],"new"); assert_eq!(current["writerEpoch"],old["writerEpoch"]);
        assert!(current["revision"].as_str().unwrap().parse::<i64>().unwrap()>old["revision"].as_str().unwrap().parse::<i64>().unwrap());
        db.execute(&sql,rusqlite::params!["reload-account","k","new",1,current["writerEpoch"].as_str().unwrap(),current["revision"].as_str().unwrap()]).unwrap();
        release.send(()).unwrap(); worker.join().unwrap();
        assert_eq!(db.query_row("SELECT value FROM cookies",[],|r| r.get::<_,String>(0)).unwrap(),"new");
        assert_eq!(eval_in_context_string("writer",&format!("String(__s2_cookie_ack('{}','{}',true))",old["leaseId"].as_str().unwrap(),old["revision"].as_str().unwrap())),"false");
        assert_eq!(eval_in_context_string("writer","String(__s2_cookie_ack(current.leaseId,current.revision,true))"),"true");
        assert_eq!(outbox().usage(),(0,0)); shutdown(); drop(db); std::fs::remove_file(path).unwrap();
    }
    fn connect_cookie_client(sid: &'static std::ffi::CStr) {
        thread_local! { static SID: std::cell::Cell<*const std::os::raw::c_char> = std::cell::Cell::new(std::ptr::null()); }
        extern "C" fn steam_id(_: i32) -> *const std::os::raw::c_char { SID.with(|s| s.get()) }
        SID.with(|s| s.set(sid.as_ptr()));
        crate::client::begin(3);
        crate::v8host::set_engine_ops(Some(crate::v8host::S2EngineOps { client_steamid: Some(steam_id), ..crate::v8host::S2EngineOps::none() }));
    }
    #[test]
    fn queued_cached_event_is_dropped_after_reuse() {
        init(dummy_logger()).unwrap();
        let a = crate::client::begin(5);
        load_body("race", r#"
            globalThis.n = 0; __s2_cookie_on_cached(function() { n++; });
        "#, "{}");
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push((5, a)));
        crate::client::begin(5);
        dispatch_pending_cached();
        assert_eq!(eval_in_context_string("race", "String(n)"), "0");
        let b = crate::client::generation(5);
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push((5, b)));
        dispatch_pending_cached();
        assert_eq!(eval_in_context_string("race", "String(n)"), "1");
        shutdown();
    }
    #[test]
    fn cached_fanout_rechecks_connection_after_each_subscriber() {
        init(dummy_logger()).unwrap();
        extern "C" fn replace(slot: i32, _: *const std::os::raw::c_char) -> i32 { crate::client::begin(slot); 1 }
        crate::v8host::set_engine_ops(Some(crate::v8host::S2EngineOps { client_command: Some(replace), ..crate::v8host::S2EngineOps::none() }));
        let token = crate::client::begin(5);
        load_body("cached-reentrant", r#"
            globalThis.n = 0;
            __s2pkg_cookies.Cookies.onCached(function(c) { n++; c.command('replace'); });
            __s2pkg_cookies.Cookies.onCached(function() { n += 100; });
        "#, "{}");
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push((5, token)));
        dispatch_pending_cached();
        assert_eq!(eval_in_context_string("cached-reentrant", "String(n)"), "1");
        shutdown();
    }
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
        connect_cookie_client(c"S9");
        load_body("cp", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("hud", { default: "white" });
            var real = new __s2pkg_clients.Client(3);
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
        connect_cookie_client(c"S10");
        load_body("cp2", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("nickname", { default: "Anonymous" });
            var real = new __s2pkg_clients.Client(3);
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

    /// clientprefs Task 3 (module layer): `Cookies.setAuthId` writes for a SteamID not passed as a
    /// `Client` at all (offline parity) — a subsequent `Cookies.get` on that steamid sees the value,
    /// and it is a no-op for "0" (bot/unset).
    #[test]
    fn clientprefs_module_set_authid_offline_and_bot_skip() {
        let _ = init(dummy_logger());
        connect_cookie_client(c"S12");
        load_body("cp3", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("hud", { default: "white" });
            Cookies.setAuthId("S12", c, "blue");
            var real = new __s2pkg_clients.Client(3);
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
        crate::client::begin(5);
        load_body("ck4", r#"
            __s2_cookie_on_cached(function (slot) {
                globalThis.__ck_ran = (globalThis.__ck_ran || 0) + 1;
                globalThis.__ck_slot = slot;
            });
            __s2_cookie_dispatch_cached(5, __s2_client_generation(5));
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
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push((9, 0)));
        dispatch_pending_cached();
        shutdown();
    }
}
#[cfg(test)]
mod outbox_tests {
    use super::*;
    fn box_with(versions: usize, bytes: usize) -> Outbox {
        Outbox { policy: OutboxPolicy { versions, bytes, write_bytes: bytes, ..Default::default() }, ..Default::default() }
    }
    #[test]
    fn pressure_coalescing_fence_and_immutable_attempt() {
        let mut q = box_with(2, 1024);
        assert!(q.accept("A", "k", "one", 1));
        let fence = q.fence("A");
        assert!(q.accept("A", "k", "two", 1));
        assert!(!q.done("A", fence));
        let a = q.lease(("p".into(), 1), 4, 1024, 0).pop().unwrap();
        assert!(q.accept("A", "k", "three", 1));
        assert!(q.accept("A", "k", "four", 1));
        assert!(!q.accept("B", "k", "full", 1));
        assert_eq!(a.payload.value, "two");
        assert!(q.ack(&a.owner, a.id, a.payload.revision, false, 0));
        assert!(!q.done("A", fence));
        assert!(q.lease(a.owner.clone(), 4, 1024, 99).is_empty());
        let b = q.lease(a.owner.clone(), 4, 1024, 100).pop().unwrap();
        assert_eq!(b.payload.value, "four");
        assert!(!q.ack(&a.owner, a.id, a.payload.revision, true, 100));
        assert!(q.ack(&b.owner, b.id, b.payload.revision, true, 100));
        assert!(q.done("A", fence));
        assert_eq!(q.usage(), (0, 0));
    }
    #[test]
    fn exact_bytes_retry_cap_fifo_and_reclaim() {
        let mut q = box_with(8, 128 + 1 + 1 + 2);
        assert!(q.accept("A", "k", "é", 0));
        assert!(!q.accept("A", "k", "éx", 0));
        let first = q.lease(("old".into(), 1), 4, 1024, 0).pop().unwrap();
        q.reclaim(Some("old"));
        let mut now = 0;
        for delay in [100,200,400,800,1600,3200,5000,5000] {
            let a = q.lease(("new".into(), 2), 4, 1024, now).pop().unwrap();
            assert!(!q.ack(&first.owner, first.id, first.payload.revision, true, now));
            assert!(q.ack(&a.owner, a.id, a.payload.revision, false, now));
            assert!(q.lease(a.owner.clone(), 4, 1024, now+delay-1).is_empty());
            now += delay;
        }
        let a = q.lease(("new".into(), 2), 4, 1024, now).pop().unwrap();
        assert!(q.ack(&a.owner, a.id, a.payload.revision, true, now));
        q.policy.bytes = 4096;
        for (sid,name) in [("A","1"),("A","2"),("B","1"),("C","1"),("D","1"),("E","1")] { assert!(q.accept(sid,name,"v",0)); }
        let batch = q.lease(("p".into(), 3), 99, 99999, now);
        assert_eq!(batch.len(), 4);
        assert_eq!(batch.iter().map(|a| a.payload.steam_id.as_str()).collect::<Vec<_>>(), ["A","B","C","D"]);
        assert!(q.lease(("p".into(), 3), 4, 99999, now).is_empty());
    }
    #[test]
    fn success_of_old_attempt_never_acknowledges_new_ready_value() {
        let mut q = box_with(2, 4096);
        assert!(q.accept("A","k","old",7));
        let old = q.lease(("p".into(),1),4,4096,0).pop().unwrap();
        assert!(q.accept("A","k","middle",7)); assert!(q.accept("A","k","new",7));
        let fence = q.fence("A");
        assert!(!q.ack(&("other".into(),1),old.id,old.payload.revision,true,0));
        assert!(!q.ack(&old.owner,old.id,old.payload.revision+1,true,0));
        assert!(q.ack(&old.owner,old.id,old.payload.revision,true,0));
        assert!(!q.done("A",fence));
        let new = q.lease(old.owner.clone(),4,4096,0).pop().unwrap();
        assert_eq!(new.payload.value,"new"); assert!(new.payload.revision > old.payload.revision);
        assert!(q.ack(&new.owner,new.id,new.payload.revision,true,0));
        assert!(q.done("A",fence));
    }
    #[test]
    fn saturated_accounts_progress_fairly_and_bytes_are_exact() {
        let mut q = box_with(100, 65536);
        for n in 0..20 { assert!(q.accept(&format!("{n:02}"),"k","v",0)); }
        let (count,bytes) = q.usage(); assert_eq!(count,20); assert_eq!(bytes,20*(128+2+1+1));
        assert!(q.accept("00","k","longer",0)); assert_eq!(q.usage(),(20,bytes+5));
        let mut seen = HashSet::new();
        for _ in 0..5 {
            let batch = q.lease(("p".into(),1),99,65536,0); assert_eq!(batch.len(),4);
            for a in batch { seen.insert(a.payload.steam_id.clone()); assert!(q.ack(&a.owner,a.id,a.payload.revision,true,0)); }
            assert!(q.accept("00","k","again",0));
        }
        assert_eq!(seen.len(),20);
    }
    #[test]
    fn account_fifo_blocks_later_keys_while_earliest_retries() {
        let mut q = box_with(8,4096);
        assert!(q.accept("A","1","first",0)); assert!(q.accept("A","2","second",0));
        let a=q.lease(("p".into(),1),4,4096,0).pop().unwrap();
        assert!(q.ack(&a.owner,a.id,a.payload.revision,false,0));
        assert!(q.lease(a.owner.clone(),4,4096,99).is_empty());
        let a=q.lease(a.owner,4,4096,100).pop().unwrap(); assert_eq!(a.payload.name,"1");
        assert!(q.ack(&a.owner,a.id,a.payload.revision,true,100));
        let b=q.lease(a.owner,4,4096,100).pop().unwrap(); assert_eq!(b.payload.name,"2");
    }
    #[test]
    fn allocator_exhaustion_rejects_without_wrap() {
        let mut q=box_with(2,1024); q.revision=i64::MAX;
        assert!(!q.accept("A","k","v",0)); assert_eq!(q.usage(),(0,0));
        q.revision=i64::MAX-1; assert!(q.accept("A","k","v",0));
        q.lease_id=u64::MAX; assert!(q.lease(("p".into(),1),4,4096,0).is_empty());
        assert_eq!(q.revision,i64::MAX); assert_eq!(q.usage().0,1);
    }
    #[test]
    fn acknowledged_offline_account_history_plateaus_without_losing_newer_pending() {
        let baseline = cache_stats()["accounts"].as_u64().unwrap();
        let mut q = box_with(2, 1024);
        let owner = ("writer".into(), 1);
        for n in 0..1000 {
            let sid = format!("task6-account-{n}");
            assert!(q.accept(&sid, "k", "v", 1));
            cache_set(&sid, "k", "v", 1);
            let a = q.lease(owner.clone(), 1, 1024, 0).pop().unwrap();
            assert!(q.ack(&owner, a.id, a.payload.revision, true, 0));
            assert_eq!(cache_stats()["accounts"].as_u64().unwrap(), baseline);
            assert_eq!(q.usage(), (0, 0));
        }
        assert!(q.accept("task6-newer", "k", "old", 1));
        cache_set("task6-newer", "k", "old", 1);
        let a = q.lease(owner.clone(), 1, 1024, 0).pop().unwrap();
        assert!(q.accept("task6-newer", "k", "new", 1));
        cache_set("task6-newer", "k", "new", 1);
        assert!(q.ack(&owner, a.id, a.payload.revision, true, 0));
        assert_eq!(get("task6-newer", "k").as_deref(), Some("new"));
        let a = q.lease(owner.clone(), 1, 1024, 0).pop().unwrap();
        assert!(q.ack(&owner, a.id, a.payload.revision, true, 0));
        assert_eq!(get("task6-newer", "k"), None);
    }

}

pub(crate) fn pending_count() -> usize {
    COOKIE_CACHED_PENDING.with(|q| q.borrow().len())
}
pub(crate) fn cache_stats() -> serde_json::Value {
    CACHE.with(|c| {
        let c = c.borrow();
        let mut entries = 0;
        let mut bytes = 0usize;
        for (sid, cc) in c.iter() {
            bytes += sid.len() + 64;
            for (name, e) in &cc.entries {
                entries += 1;
                bytes += name.len() + e.value.len() + 64;
            }
        }
        serde_json::json!({"accounts":c.len(),"entries":entries,"bytes":bytes})
    })
}

#[cfg(test)] pub(crate) fn test_queue_notification() {COOKIE_CACHED_PENDING.with(|q|{let mut q=q.borrow_mut();if q.is_empty(){q.push((-1,0));}});}
