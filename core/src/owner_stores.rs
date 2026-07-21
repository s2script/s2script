//! Self-registration list of every owner-scoped subscription store (design spec §6).
//! `unload_plugin` sweeps THIS registry instead of a hand-maintained cascade; a new
//! capability slice registers its store next to the store's definition.
//!
//! Invariant enforced by convention (not the type system): a store closure NEVER calls
//! `register`/`sweep_*`. Every builtin closure only touches its own mux/table thread-locals
//! plus its engine-op follow-up, so `sweep_*` can hold the `STORES` borrow across the call.
use std::cell::RefCell;

pub struct OwnerScopedStore {
    pub name: &'static str,
    pub remove_by_owner: Box<dyn Fn(&str)>,
    pub remove_by_ids: Box<dyn Fn(&[u64])>,
}

thread_local! {
    static STORES: RefCell<Vec<OwnerScopedStore>> = const { RefCell::new(Vec::new()) };
}

pub fn register(
    name: &'static str,
    remove_by_owner: Box<dyn Fn(&str)>,
    remove_by_ids: Box<dyn Fn(&[u64])>,
) {
    STORES.with(|s| {
        s.borrow_mut()
            .push(OwnerScopedStore { name, remove_by_owner, remove_by_ids })
    });
}

/// Idempotent re-registration guard for re-init paths (Metamod reload): clears the list.
pub fn reset() {
    STORES.with(|s| s.borrow_mut().clear());
}

/// Run every store's `remove_by_owner` in registration order. Store closures never re-enter
/// `register`/`sweep_*` (documented invariant), so holding the borrow across the call is sound.
pub fn sweep_owner(owner: &str) {
    STORES.with(|s| {
        let stores = s.borrow();
        for st in stores.iter() {
            (st.remove_by_owner)(owner);
        }
    });
}

/// Run every store's `remove_by_ids` in registration order. A no-op for an empty id list.
pub fn sweep_ids(ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    STORES.with(|s| {
        let stores = s.borrow();
        for st in stores.iter() {
            (st.remove_by_ids)(ids);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    thread_local! { static HITS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }

    #[test]
    fn sweep_owner_runs_every_store_in_registration_order() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        register(
            "a",
            Box::new(|o| HITS.with(|h| h.borrow_mut().push(format!("a:{o}")))),
            Box::new(|_| {}),
        );
        register(
            "b",
            Box::new(|o| HITS.with(|h| h.borrow_mut().push(format!("b:{o}")))),
            Box::new(|_| {}),
        );
        sweep_owner("p1");
        HITS.with(|h| assert_eq!(*h.borrow(), vec!["a:p1".to_string(), "b:p1".to_string()]));
    }

    #[test]
    fn sweep_ids_skips_empty_and_hits_all_stores() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        register(
            "a",
            Box::new(|_| {}),
            Box::new(|ids| HITS.with(|h| h.borrow_mut().push(format!("a:{}", ids.len())))),
        );
        sweep_ids(&[]);
        HITS.with(|h| assert!(h.borrow().is_empty(), "empty ids = no-op"));
        sweep_ids(&[1, 2]);
        HITS.with(|h| assert_eq!(*h.borrow(), vec!["a:2".to_string()]));
    }
}
