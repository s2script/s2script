//! Shared AND-fold rule table: `owner -> (key -> V)`.
//!
//! Transmit visibility and voice hearability are two instances of the same policy
//! store. The fold is most-restrictive-wins (AND): no owner can WIDEN what another
//! owner restricted. Serial-gating (transmit) and slot-clear (voice) stay on the
//! adapters — they are parameters, not a second table.
//!
//! `None` after a fold means no owner has a rule (the engine decides). `Some(zero)`
//! is a rule: hidden from everyone / audible to nobody.

use std::collections::HashMap;
use std::hash::Hash;

/// How two owners' rules at the same key combine. For a `u64` mask this is `&`.
/// A richer value (transmit's `{serial, mask}`) ANDs the mask and keeps a serial.
pub trait AndFold: Copy {
    fn and_fold(self, other: Self) -> Self;
}

impl AndFold for u64 {
    fn and_fold(self, other: Self) -> Self {
        self & other
    }
}

/// Per-owner rules, folded by AND at a key.
pub struct FoldTable<K, V> {
    owners: HashMap<String, HashMap<K, V>>,
}

impl<K: Eq + Hash + Copy, V: Copy> FoldTable<K, V> {
    pub fn new() -> Self {
        Self { owners: HashMap::new() }
    }

    pub fn insert(&mut self, owner: impl Into<String>, key: K, value: V) {
        self.owners.entry(owner.into()).or_default().insert(key, value);
    }

    pub fn get(&self, owner: &str, key: &K) -> Option<V> {
        self.owners.get(owner).and_then(|r| r.get(key)).copied()
    }

    pub fn remove(&mut self, owner: &str, key: &K) -> Option<V> {
        let v = self.owners.get_mut(owner)?.remove(key);
        if self.owners.get(owner).is_some_and(|m| m.is_empty()) {
            self.owners.remove(owner);
        }
        v
    }

    /// Drop every rule `owner` holds. Returns the keys it touched (so the adapter
    /// can re-push each one).
    pub fn remove_owner(&mut self, owner: &str) -> Vec<K> {
        match self.owners.remove(owner) {
            Some(rules) => rules.into_keys().collect(),
            None => Vec::new(),
        }
    }

    /// Drop EVERY owner's rule for `key`. Returns whether anything was removed.
    /// Voice uses this on disconnect so a recycled slot does not inherit a rule.
    pub fn clear_key(&mut self, key: &K) -> bool {
        let mut any = false;
        for rules in self.owners.values_mut() {
            if rules.remove(key).is_some() {
                any = true;
            }
        }
        any
    }

    pub fn clear(&mut self) {
        self.owners.clear();
    }

    /// AND-fold every owner's value at `key`. `None` if no owner has a rule.
    pub fn merged(&self, key: &K) -> Option<V>
    where
        V: AndFold,
    {
        let mut acc: Option<V> = None;
        for rules in self.owners.values() {
            if let Some(&v) = rules.get(key) {
                acc = Some(match acc {
                    None => v,
                    Some(a) => a.and_fold(v),
                });
            }
        }
        acc
    }

    /// AND `seed` with every OTHER owner's value at `key` that `include` accepts.
    /// Transmit passes "same serial"; voice accepts every other owner.
    pub fn fold_except(&self, owner: &str, key: &K, seed: V, include: impl Fn(&V) -> bool) -> V
    where
        V: AndFold,
    {
        let mut acc = seed;
        for (o, rules) in &self.owners {
            if o == owner {
                continue;
            }
            if let Some(v) = rules.get(key) {
                if include(v) {
                    acc = acc.and_fold(*v);
                }
            }
        }
        acc
    }

    /// Drop values at `key` for which `stale` is true. Transmit evicts a different
    /// serial after the op has validated the live one.
    pub fn evict_at(&mut self, key: &K, stale: impl Fn(&V) -> bool) {
        for rules in self.owners.values_mut() {
            if rules.get(key).is_some_and(|v| stale(v)) {
                rules.remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn and_fold_is_intersection_and_none_is_not_zero() {
        let mut t: FoldTable<i32, u64> = FoldTable::new();
        t.insert("a", 3, 0b0111);
        t.insert("b", 3, 0b0110);
        assert_eq!(t.merged(&3), Some(0b0110));
        assert_eq!(t.merged(&9), None, "no rule is not a zero-mask rule");
        t.insert("c", 5, 0);
        assert_eq!(t.merged(&5), Some(0), "a zero mask is a rule: audible to nobody");
    }

    #[test]
    fn remove_owner_returns_touched_keys_and_leaves_the_survivor() {
        let mut t: FoldTable<i32, u64> = FoldTable::new();
        t.insert("a", 3, 0b0111);
        t.insert("b", 3, 0b0110);
        let touched = t.remove_owner("b");
        assert_eq!(touched, vec![3]);
        assert_eq!(t.merged(&3), Some(0b0111));
        assert!(t.remove_owner("a") == vec![3]);
        assert_eq!(t.merged(&3), None);
    }

    #[test]
    fn clear_key_drops_every_owner_at_that_key() {
        let mut t: FoldTable<i32, u64> = FoldTable::new();
        t.insert("a", 3, 0b1);
        t.insert("b", 3, 0b1);
        t.insert("a", 7, 0b1);
        assert!(t.clear_key(&3));
        assert_eq!(t.merged(&3), None);
        assert_eq!(t.merged(&7), Some(0b1), "other keys are untouched");
        assert!(!t.clear_key(&3), "second clear is a no-op");
    }

    #[test]
    fn fold_except_skips_the_caller_and_honors_include() {
        let mut t: FoldTable<i32, u64> = FoldTable::new();
        t.insert("a", 1, 0b1111);
        t.insert("b", 1, 0b0110);
        // Candidate mask 0b1111 from a, AND b -> 0b0110. a's stored value is skipped.
        assert_eq!(t.fold_except("a", &1, 0b1111, |_| true), 0b0110);
        // include rejects b -> seed unchanged.
        assert_eq!(t.fold_except("a", &1, 0b1111, |_| false), 0b1111);
    }

    #[test]
    fn evict_at_drops_only_the_stale_values() {
        #[derive(Clone, Copy, PartialEq, Debug)]
        struct Rule {
            serial: i32,
            mask: u64,
        }
        impl AndFold for Rule {
            fn and_fold(self, other: Self) -> Self {
                Self { serial: other.serial, mask: self.mask & other.mask }
            }
        }
        let mut t: FoldTable<i32, Rule> = FoldTable::new();
        t.insert("old", 4, Rule { serial: 1, mask: 0b1 });
        t.insert("live", 4, Rule { serial: 2, mask: 0b11 });
        t.evict_at(&4, |r| r.serial != 2);
        assert_eq!(t.get("old", &4), None);
        assert_eq!(t.get("live", &4), Some(Rule { serial: 2, mask: 0b11 }));
    }
}
