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
    /// Whole-process teardown: restore this store to its empty initial state.
    ///
    /// CONTENTS ONLY — no engine-op follow-up. `remove_by_owner` calls `event_unsubscribe` (etc.)
    /// because the engine must stop delivering to a plugin that is going away while the server keeps
    /// running; `shutdown()` is tearing the whole host down, and the hand-written cascade this
    /// replaces never issued those calls either. Firing per-name engine ops during teardown would be
    /// new traffic into the engine at the worst possible moment.
    pub reset: Box<dyn Fn()>,
}

thread_local! {
    static STORES: RefCell<Vec<OwnerScopedStore>> = const { RefCell::new(Vec::new()) };
}

pub fn register(
    name: &'static str,
    remove_by_owner: Box<dyn Fn(&str)>,
    remove_by_ids: Box<dyn Fn(&[u64])>,
    reset: Box<dyn Fn()>,
) {
    STORES.with(|s| {
        s.borrow_mut()
            .push(OwnerScopedStore { name, remove_by_owner, remove_by_ids, reset })
    });
}

/// Idempotent re-registration guard for re-init paths (Metamod reload): clears the LIST of
/// registrations without running any of them. Not to be confused with `sweep_reset`, which leaves
/// the registrations in place and clears each store's CONTENTS.
pub fn reset() {
    STORES.with(|s| s.borrow_mut().clear());
}

/// Run every store's `reset` in registration order — the whole-process teardown verb.
///
/// This replaces the hand-maintained clear-one-line-per-store cascade in `shutdown()`, which had to
/// be extended by hand for every new capability slice and silently kept stale state on the slices
/// where that was forgotten. A store registered here is now torn down because it is registered, not
/// because someone remembered.
///
/// Store closures never re-enter `register`/`sweep_*` (documented invariant), so holding the borrow
/// across the call is sound — same reasoning as `sweep_owner`.
pub fn sweep_reset() {
    STORES.with(|s| {
        let stores = s.borrow();
        for st in stores.iter() {
            (st.reset)();
        }
    });
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
            Box::new(|| {}),
        );
        register(
            "b",
            Box::new(|o| HITS.with(|h| h.borrow_mut().push(format!("b:{o}")))),
            Box::new(|_| {}),
            Box::new(|| {}),
        );
        sweep_owner("p1");
        HITS.with(|h| assert_eq!(*h.borrow(), vec!["a:p1".to_string(), "b:p1".to_string()]));
    }

    /// The third verb. `sweep_owner`/`sweep_ids` are per-plugin; `sweep_reset` is the whole-process
    /// teardown verb that `shutdown()` needs, and it must run in registration order for the same
    /// reason the other two do — the order IS the historical hand-written cascade order.
    #[test]
    fn sweep_reset_runs_every_store_in_registration_order() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        register(
            "a",
            Box::new(|_| {}),
            Box::new(|_| {}),
            Box::new(|| HITS.with(|h| h.borrow_mut().push("a".to_string()))),
        );
        register(
            "b",
            Box::new(|_| {}),
            Box::new(|_| {}),
            Box::new(|| HITS.with(|h| h.borrow_mut().push("b".to_string()))),
        );
        sweep_reset();
        HITS.with(|h| assert_eq!(*h.borrow(), vec!["a".to_string(), "b".to_string()]));
    }

    /// `reset()` clears the REGISTRATION list; `sweep_reset()` clears each store's CONTENTS. Two
    /// different jobs with confusingly adjacent names — assert they stay distinct, so a future
    /// refactor cannot quietly collapse them and leave teardown running against an empty registry.
    #[test]
    fn reset_clears_registrations_without_running_them() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        register(
            "a",
            Box::new(|_| {}),
            Box::new(|_| {}),
            Box::new(|| HITS.with(|h| h.borrow_mut().push("a".to_string()))),
        );
        reset();
        HITS.with(|h| assert!(h.borrow().is_empty(), "reset() must not RUN the reset closures"));
        sweep_reset();
        HITS.with(|h| assert!(h.borrow().is_empty(), "reset() must have dropped the registration"));
    }

    #[test]
    fn sweep_ids_skips_empty_and_hits_all_stores() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        register(
            "a",
            Box::new(|_| {}),
            Box::new(|ids| HITS.with(|h| h.borrow_mut().push(format!("a:{}", ids.len())))),
            Box::new(|| {}),
        );
        sweep_ids(&[]);
        HITS.with(|h| assert!(h.borrow().is_empty(), "empty ids = no-op"));
        sweep_ids(&[1, 2]);
        HITS.with(|h| assert_eq!(*h.borrow(), vec!["a:2".to_string()]));
    }
}
