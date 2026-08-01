//! Self-registration list of every PROCESS-scoped piece of host state that `shutdown()` must reset.
//!
//! The sibling of `owner_stores`, and deliberately DISJOINT from it. `owner_stores` holds state that
//! belongs to a plugin: unloading one plugin must clear its rows and no one else's. The state here
//! belongs to the *process* and is designed to SURVIVE any single plugin's unload — the admin and ban
//! caches, the schema-offset cache, the cookie cache. Registering those owner-scoped would wipe
//! shared admin state on an unrelated plugin's reload, which is why this is a second registry rather
//! than a fourth verb on the first one.
//!
//! Both exist for the same reason: `shutdown()` used to be a hand-written clear-one-line-per-thing
//! cascade that had to be extended for every capability slice, and silently kept stale state on the
//! ones where that was forgotten. Three shipped fixes were exactly that (`98cf483`, `e40492d`,
//! `7e62119`) — and all three were in THIS bucket, the one with no registration at all.
use std::cell::RefCell;

/// WHEN a singleton's reset must run, relative to dropping the V8 isolate in `shutdown()`.
///
/// This is the ordering rule that used to live only in repeated prose comments ("clear X BEFORE
/// dropping the isolate — it holds `Global`s"). Making it a required argument means a new
/// registration cannot omit the decision, and getting it wrong is visible at the registration site
/// instead of 80 lines away in teardown.
///
/// Named `ResetPhase` rather than `Phase` on purpose: `multiplexer::Phase` (Pre/Post) and
/// `plugin::Phase` (Loading/Active) already exist and mean unrelated things.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResetPhase {
    /// Runs while the isolate is still alive. REQUIRED for anything holding a `v8::Global`: the
    /// handle must be released before its isolate goes away.
    BeforeIsolateDrop,
    /// Runs after the isolate is gone. For pure-Rust state with no V8 handles.
    AfterIsolateDrop,
}

pub struct ProcessSingleton {
    pub name: &'static str,
    pub phase: ResetPhase,
    pub reset: Box<dyn Fn()>,
}

thread_local! {
    static SINGLETONS: RefCell<Vec<ProcessSingleton>> = const { RefCell::new(Vec::new()) };
}

pub fn register(name: &'static str, phase: ResetPhase, reset: Box<dyn Fn()>) {
    SINGLETONS.with(|s| s.borrow_mut().push(ProcessSingleton { name, phase, reset }));
}

/// Idempotent re-registration guard for re-init paths (Metamod reload): clears the LIST of
/// registrations without running any of them. Mirrors `owner_stores::reset`.
pub fn reset() {
    SINGLETONS.with(|s| s.borrow_mut().clear());
}

/// Run the `reset` of every singleton registered for `phase`, in registration order.
///
/// Registration order is the historical cascade order, preserved 1:1 — same contract as
/// `owner_stores::sweep_owner`. Reset closures never re-enter `register`/`reset_all` (they only
/// touch their own thread-local), so holding the borrow across the call is sound.
pub fn reset_all(phase: ResetPhase) {
    SINGLETONS.with(|s| {
        let list = s.borrow();
        for st in list.iter().filter(|st| st.phase == phase) {
            (st.reset)();
        }
    });
}

/// Every registered name, in registration order. Diagnostics + the coverage test in `v8host`.
pub fn registered_names() -> Vec<(&'static str, ResetPhase)> {
    SINGLETONS.with(|s| s.borrow().iter().map(|st| (st.name, st.phase)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    thread_local! { static HITS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }

    fn probe(name: &'static str, phase: ResetPhase) {
        register(name, phase, Box::new(move || HITS.with(|h| h.borrow_mut().push(name.to_string()))));
    }

    #[test]
    fn reset_all_runs_only_the_requested_phase_in_registration_order() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        probe("before_a", ResetPhase::BeforeIsolateDrop);
        probe("after_a", ResetPhase::AfterIsolateDrop);
        probe("before_b", ResetPhase::BeforeIsolateDrop);

        reset_all(ResetPhase::BeforeIsolateDrop);
        HITS.with(|h| {
            assert_eq!(
                *h.borrow(),
                vec!["before_a".to_string(), "before_b".to_string()],
                "only the Before phase, and in registration order"
            )
        });

        reset_all(ResetPhase::AfterIsolateDrop);
        HITS.with(|h| {
            assert_eq!(
                *h.borrow(),
                vec!["before_a".to_string(), "before_b".to_string(), "after_a".to_string()]
            )
        });
    }

    /// `reset()` drops the REGISTRATIONS; `reset_all()` runs them. Adjacent names, opposite jobs —
    /// pinned so a refactor cannot collapse them and leave teardown iterating an empty list.
    #[test]
    fn reset_clears_registrations_without_running_them() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        probe("x", ResetPhase::BeforeIsolateDrop);
        reset();
        HITS.with(|h| assert!(h.borrow().is_empty(), "reset() must not RUN anything"));
        reset_all(ResetPhase::BeforeIsolateDrop);
        HITS.with(|h| assert!(h.borrow().is_empty(), "reset() must have dropped the registration"));
    }
}
