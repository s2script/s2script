//! Engine-generic host-global client-cookie cache: steamid -> { name -> (value, dirty) } plus a
//! per-client `cached` flag. Mirrors the admin/ban caches (cross-context-visible per-client string
//! KV, read/written via natives). Knows nothing about any game; holds no V8 handles.
use std::cell::RefCell;
use std::collections::HashMap;

struct Entry { value: String, dirty: bool }
#[derive(Default)]
struct ClientCookies { cached: bool, entries: HashMap<String, Entry> }

thread_local! {
    static CACHE: RefCell<HashMap<String, ClientCookies>> = RefCell::new(HashMap::new());
}

/// Cache value, or "" if the client/name is absent.
pub fn get(steamid: &str, name: &str) -> String {
    CACHE.with(|c| c.borrow().get(steamid)
        .and_then(|cc| cc.entries.get(name))
        .map(|e| e.value.clone())
        .unwrap_or_default())
}

/// Write via the API — marks the entry dirty (flushed on disconnect).
pub fn set(steamid: &str, name: &str, value: &str) {
    CACHE.with(|c| {
        let mut m = c.borrow_mut();
        let cc = m.entry(steamid.to_string()).or_default();
        cc.entries.insert(name.to_string(), Entry { value: value.to_string(), dirty: true });
    });
}

/// Write from the DB load — NOT dirty (a loaded value is not a change).
pub fn load(steamid: &str, name: &str, value: &str) {
    CACHE.with(|c| {
        let mut m = c.borrow_mut();
        let cc = m.entry(steamid.to_string()).or_default();
        cc.entries.insert(name.to_string(), Entry { value: value.to_string(), dirty: false });
    });
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
}

#[cfg(test)]
mod tests {
    use super::*;
    // NOTE: CACHE is thread-local + tests run serial (RUST_TEST_THREADS=1); use a unique steamid per
    // test so they don't observe each other's entries.
    #[test]
    fn set_get_and_dirty() {
        set("A1", "color", "red");
        assert_eq!(get("A1", "color"), "red");
        let d = get_dirty("A1");
        assert_eq!(d, vec![("color".to_string(), "red".to_string())]);
        assert_eq!(get("A1", "missing"), "");
    }
    #[test]
    fn load_is_not_dirty() {
        load("A2", "k", "v");
        assert_eq!(get("A2", "k"), "v");
        assert!(get_dirty("A2").is_empty(), "a loaded value is not dirty");
        set("A2", "k2", "v2");   // a later set IS dirty
        assert_eq!(get_dirty("A2"), vec![("k2".to_string(), "v2".to_string())]);
    }
    #[test]
    fn clear_removes_client() {
        set("A3", "k", "v");
        clear("A3");
        assert_eq!(get("A3", "k"), "");
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
        set("A5", "k", "v");
        mark_cached("A5");
        reset();
        assert_eq!(get("A5", "k"), "");
        assert!(!is_cached("A5"));   // stale cached flag gone
    }
}
