//! Host-side HUD component pool claims (`__s2_ui_pool_claim` / `__s2_ui_pool_release`).
//!
//! The cs2 prelude's hudkit hands out pooled panel trees (`s2_m0..`, badge corners) from ONE
//! shared layout entity — ui.js finds that entity by targetname, identically from every plugin's
//! context. The claim bookkeeping used to live in the prelude on `globalThis.__s2ui_pool`, under
//! a comment calling it "HOST-GLOBAL". It was not: the prelude is evaluated once PER PLUGIN
//! CONTEXT (`run_prelude` in v8host), so every plugin had its own private pool. Every plugin's
//! menuhud claimed `s2_m0` in its own books, two plugins' second sheets both got `s2_m1`, and
//! both painted the same entity panels over each other. Exhaustion accounting was fiction.
//!
//! Claims are a cross-plugin resource, so they live here, on the host, in one table — and the
//! owner is `current_plugin(scope)` read at claim time, never a JS-supplied tag (the JS side's
//! `ownerTag` was the literal string "plugin" for everyone). Each claim is ledgered against its
//! owner via the `owner_stores` registry, so `unload_plugin` frees a departed plugin's slots by
//! walking the ledger — never by trusting the plugin's own cleanup code to have run (the repo's
//! teardown doctrine).
//!
//! Deliberately NOT map-scoped: the pooled panel ids are properties of the layout resource, not
//! of a map, and a plugin's modal object survives map changes the way its other handles do.

use crate::v8host::{current_plugin, log_warn, set_native};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// kind ("modal" / "badge" / future families) → slot index → owning plugin id.
    /// `None` = free. The vec grows lazily to the capacity the claimer passes: capacity is an
    /// argument, not a constant here, because the slot counts (MODALS/BADGES) are facts about
    /// `s2script_lib.xml` and belong to the game package — the host only arbitrates ownership.
    static UI_POOL_CLAIMS: RefCell<HashMap<String, Vec<Option<String>>>>
        = RefCell::new(HashMap::new());
}

/// Claim the lowest free slot in `kind`'s pool, scanning `[0, capacity)`. Returns the claimed
/// index, or -1 when every slot below `capacity` is held. Lowest-first is load-bearing: clients
/// on an older addon only have the low panel trees, so the common case must keep landing there.
pub(crate) fn claim(kind: &str, capacity: usize, owner: &str) -> i32 {
    UI_POOL_CLAIMS.with(|p| {
        let mut pools = p.borrow_mut();
        let pool = pools.entry(kind.to_string()).or_default();
        if pool.len() < capacity {
            pool.resize(capacity, None);
        }
        for (i, slot) in pool.iter_mut().enumerate().take(capacity) {
            if slot.is_none() {
                *slot = Some(owner.to_string());
                return i as i32;
            }
        }
        -1
    })
}

/// Release `kind`[`index`] — but only if `owner` actually holds it. A plugin must not be able to
/// free another plugin's slot (that would re-mint the exact aliasing this module exists to end),
/// so a mismatch is a logged refusal, not a best-effort clear.
pub(crate) fn release(kind: &str, index: usize, owner: &str) -> bool {
    UI_POOL_CLAIMS.with(|p| {
        let mut pools = p.borrow_mut();
        let Some(pool) = pools.get_mut(kind) else {
            log_warn(&format!("WARN: ui_pool release: no '{}' pool exists", kind));
            return false;
        };
        match pool.get_mut(index) {
            Some(slot @ Some(_)) if slot.as_deref() == Some(owner) => {
                *slot = None;
                true
            }
            Some(Some(holder)) => {
                log_warn(&format!(
                    "WARN: ui_pool release: '{}' slot {} is held by '{}', not caller '{}' — refused",
                    kind, index, holder, owner
                ));
                false
            }
            _ => {
                log_warn(&format!(
                    "WARN: ui_pool release: '{}' slot {} is not claimed",
                    kind, index
                ));
                false
            }
        }
    })
}

/// Free every slot `owner` holds, across all kinds. The `owner_stores` teardown path.
fn remove_owner(owner: &str) {
    UI_POOL_CLAIMS.with(|p| {
        for pool in p.borrow_mut().values_mut() {
            for slot in pool.iter_mut() {
                if slot.as_deref() == Some(owner) {
                    *slot = None;
                }
            }
        }
    });
}

/// `__s2_ui_pool_claim(kind, capacity) -> index | -1`. The owner is the CALLING context's plugin
/// id — read host-side, never accepted from JS, so a claim is attributed correctly no matter what
/// the prelude passes or omits.
fn s2_ui_pool_claim(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 2 {
            return;
        }
        let kind = args.get(0).to_rust_string_lossy(scope);
        let capacity = args.get(1).integer_value(scope).unwrap_or(0);
        if capacity <= 0 {
            return;
        }
        // Clamped defensively: capacity comes from the game package's constants (single digits);
        // a buggy caller must not make the host allocate a giant vec.
        let capacity = capacity.min(1024) as usize;
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        rv.set_int32(claim(&kind, capacity, &owner));
    }));
}

/// `__s2_ui_pool_release(kind, index) -> bool`. Owner-checked against the calling context.
fn s2_ui_pool_release(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 2 {
            return;
        }
        let kind = args.get(0).to_rust_string_lossy(scope);
        let Some(index) = args.get(1).integer_value(scope).filter(|i| *i >= 0) else {
            return;
        };
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        rv.set_bool(release(&kind, index as usize, &owner));
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_ui_pool_claim", s2_ui_pool_claim);
    set_native(scope, global_obj, "__s2_ui_pool_release", s2_ui_pool_release);
}

/// The owner-scoped store: pure bookkeeping — no engine hook to remove, not a scope surface
/// (ids no-op). This registration IS the ledger entry that makes teardown automatic.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "UI_POOL_CLAIMS",
        Box::new(|owner| remove_owner(owner)),
        Box::new(|_ids| {}),
        Box::new(|| UI_POOL_CLAIMS.with(|p| p.borrow_mut().clear())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, load_body};
    use crate::v8host::{init, shutdown, unload_plugin};

    fn clear_pool() {
        UI_POOL_CLAIMS.with(|p| p.borrow_mut().clear());
    }

    // ── pure-store tests ────────────────────────────────────────────────────────────────────

    #[test]
    fn claim_hands_out_lowest_free_across_owners() {
        clear_pool();
        assert_eq!(claim("modal", 3, "a"), 0);
        assert_eq!(claim("modal", 3, "b"), 1, "a second PLUGIN must not also get 0");
        assert_eq!(claim("modal", 3, "a"), 2);
        assert_eq!(claim("modal", 3, "b"), -1, "exhaustion must be real, not per-plugin fiction");
        clear_pool();
    }

    #[test]
    fn kinds_are_independent_pools() {
        clear_pool();
        assert_eq!(claim("modal", 2, "a"), 0);
        assert_eq!(claim("badge", 2, "a"), 0, "a modal claim must not spend a badge slot");
        clear_pool();
    }

    #[test]
    fn release_is_owner_checked() {
        clear_pool();
        assert_eq!(claim("modal", 2, "a"), 0);
        assert!(!release("modal", 0, "b"), "a plugin must not free another plugin's slot");
        assert_eq!(claim("modal", 2, "b"), 1, "the refused release must have left 0 held");
        assert!(release("modal", 0, "a"));
        assert_eq!(claim("modal", 2, "b"), 0, "a released slot is reclaimable, lowest-first");
        assert!(!release("badge", 0, "a"), "releasing in a pool that was never claimed is refused");
        clear_pool();
    }

    #[test]
    fn remove_owner_frees_only_that_owners_slots() {
        clear_pool();
        assert_eq!(claim("modal", 3, "a"), 0);
        assert_eq!(claim("modal", 3, "b"), 1);
        assert_eq!(claim("badge", 2, "a"), 0);
        remove_owner("a");
        assert_eq!(claim("modal", 3, "c"), 0, "a's modal slot must be free again");
        assert_eq!(claim("modal", 3, "c"), 2, "b's slot must have survived a's teardown");
        assert_eq!(claim("badge", 2, "c"), 0, "a's badge slot must be free again");
        clear_pool();
    }

    // ── in-isolate tests: the natives + the unload ledger ──────────────────────────────────

    /// The defect this module fixes, asserted end-to-end: two plugin CONTEXTS claiming through
    /// the native get DISTINCT slots, because the table is host-side — not one `s2_m0` each.
    #[test]
    fn native_claims_are_global_across_plugin_contexts() {
        init(dummy_logger()).unwrap();
        load_body("pool_a", r#" globalThis.__idx = __s2_ui_pool_claim("modal", 6); "#, "{}");
        load_body("pool_b", r#" globalThis.__idx = __s2_ui_pool_claim("modal", 6); "#, "{}");
        assert_eq!(eval_in_context_string("pool_a", "String(globalThis.__idx)"), "0");
        assert_eq!(
            eval_in_context_string("pool_b", "String(globalThis.__idx)"), "1",
            "the second plugin's first claim must see the first plugin's claim"
        );
        shutdown();
    }

    /// Teardown walks the ledger: unloading a plugin frees its claims with NO release call in
    /// the plugin — the next claimer gets the freed low slot back.
    #[test]
    fn native_unload_frees_the_departed_plugins_claims() {
        init(dummy_logger()).unwrap();
        load_body("pool_c", r#" __s2_ui_pool_claim("modal", 6); __s2_ui_pool_claim("badge", 4); "#, "{}");
        load_body("pool_d", r#" globalThis.__idx = __s2_ui_pool_claim("modal", 6); "#, "{}");
        assert_eq!(eval_in_context_string("pool_d", "String(globalThis.__idx)"), "1");
        unload_plugin("pool_c");
        assert_eq!(
            eval_in_context_string("pool_d", r#" String(__s2_ui_pool_claim("modal", 6)) "#), "0",
            "pool_c's modal slot must be free after its unload — the ledger, not plugin cleanup"
        );
        assert_eq!(
            eval_in_context_string("pool_d", r#" String(__s2_ui_pool_claim("badge", 4)) "#), "0",
            "pool_c's badge slot must be free after its unload"
        );
        shutdown();
    }

    /// The native release path is owner-checked exactly like the store: a different plugin's
    /// release of a held slot is refused, the owner's succeeds.
    #[test]
    fn native_release_is_owner_checked() {
        init(dummy_logger()).unwrap();
        load_body("pool_e", r#" globalThis.__idx = __s2_ui_pool_claim("modal", 6); "#, "{}");
        load_body("pool_f", r#" "#, "{}");
        assert_eq!(
            eval_in_context_string("pool_f", r#" String(__s2_ui_pool_release("modal", 0)) "#),
            "false", "a non-owner's release must be refused"
        );
        assert_eq!(
            eval_in_context_string("pool_e", r#" String(__s2_ui_pool_release("modal", 0)) "#),
            "true"
        );
        assert_eq!(
            eval_in_context_string("pool_f", r#" String(__s2_ui_pool_claim("modal", 6)) "#), "0",
            "the owner's release must actually free the slot"
        );
        shutdown();
    }
}
