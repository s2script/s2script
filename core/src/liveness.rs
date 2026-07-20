//! The shared liveness primitive: host-minted monotonic ids + keyed `is_live` tables.
//! Doctrine (north-star §1): liveness is decided by the HOST'S BOOKS — populated by
//! notifications, cleared by transitions — never by reading the resource's own memory.
//! Two instances share this module and stay SEPARATE tables: plugins (`plugin::Registry`)
//! and entities (`entity_live`). A plugin reload must not invalidate entities; a map
//! change must not invalidate plugins.

use std::collections::HashMap;
use std::hash::Hash;

/// A host-owned liveness table: `key → (host-minted id, meta)`. The id allocator is
/// monotonic for the table's lifetime and NEVER resets (not on remove, not on clear) —
/// that monotonicity IS the anti-aliasing guarantee.
pub struct LiveTable<K: Eq + Hash, M> {
    entries: HashMap<K, (u64, M)>,
    next_id: u64,
}

impl<K: Eq + Hash, M> LiveTable<K, M> {
    /// `first_id`: plugins use 0 (exact `Registry` behavior today); entities use 1 so
    /// id 0 is a never-live sentinel on the JS wire.
    pub fn new(first_id: u64) -> Self {
        Self { entries: HashMap::new(), next_id: first_id }
    }
    /// Mint a fresh id for `key`, replacing any existing entry. Replacement IS the
    /// invalidation of the previous holder — its captured id can never match again.
    pub fn insert(&mut self, key: K, meta: M) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(key, (id, meta));
        id
    }
    pub fn remove(&mut self, key: &K) -> Option<(u64, M)> {
        self.entries.remove(key)
    }
    pub fn is_live(&self, key: &K, id: u64) -> bool {
        self.entries.get(key).map_or(false, |(cur, _)| *cur == id)
    }
    pub fn get(&self, key: &K) -> Option<(u64, &M)> {
        self.entries.get(key).map(|(id, m)| (*id, m))
    }
    pub fn get_mut(&mut self, key: &K) -> Option<(u64, &mut M)> {
        self.entries.get_mut(key).map(|(id, m)| (*id, m))
    }
    pub fn keys(&self) -> Vec<K>
    where
        K: Clone,
    {
        self.entries.keys().cloned().collect()
    }
    /// Drop every entry. The allocator NEVER resets.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_monotonic_and_replace_invalidates_the_old_id() {
        let mut t: LiveTable<i32, &str> = LiveTable::new(1);
        let a = t.insert(7, "first");
        assert_eq!(a, 1, "first_id honored");
        assert!(t.is_live(&7, a));
        let b = t.insert(7, "second"); // same key: replace = invalidation
        assert!(b > a, "ids are strictly monotonic");
        assert!(!t.is_live(&7, a), "the replaced holder's id can never match again");
        assert!(t.is_live(&7, b));
        assert_eq!(t.get(&7), Some((b, &"second")));
    }

    #[test]
    fn clear_drops_entries_but_never_resets_the_allocator() {
        let mut t: LiveTable<i32, ()> = LiveTable::new(1);
        let a = t.insert(1, ());
        t.clear();
        assert!(t.is_empty());
        assert!(!t.is_live(&1, a), "cleared entry is dead");
        let b = t.insert(1, ());
        assert!(b > a, "an id from before a clear can NEVER alias one minted after");
    }

    #[test]
    fn remove_get_mut_keys_len() {
        let mut t: LiveTable<String, i32> = LiveTable::new(0);
        let g0 = t.insert("a".into(), 10);
        assert_eq!(g0, 0, "plugin-compat: first_id 0 mints 0");
        t.insert("b".into(), 20);
        assert_eq!(t.len(), 2);
        if let Some((_, m)) = t.get_mut(&"a".to_string()) {
            *m = 11;
        }
        assert_eq!(t.get(&"a".to_string()).map(|(_, m)| *m), Some(11));
        let mut ks = t.keys();
        ks.sort();
        assert_eq!(ks, vec!["a".to_string(), "b".to_string()]);
        let (gid, meta) = t.remove(&"a".to_string()).unwrap();
        assert_eq!((gid, meta), (g0, 11));
        assert!(t.remove(&"a".to_string()).is_none());
    }
}
