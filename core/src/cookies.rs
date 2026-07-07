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
