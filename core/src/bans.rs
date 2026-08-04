//! The host-global ban cache — SteamID64 → (until_unix, reason) — plus the `__s2_ban_*` natives over
//! it, the synchronous `ban_check` primitive, and its teardown registrations.
//!
//! `until == 0` = permanent; otherwise the unix-second expiry. Host-global in core (not plugin-local
//! JS) so it is visible across all plugin contexts, like the admin cache. Populated by JS via the
//! natives (loaded from bans.json through the config bridge). Enforcement is JS-driven: a ban
//! plugin's `Clients.onConnect` handler reads it via `__s2_ban_get` and shows-then-kicks a banned
//! player. Engine-generic; holds no V8 handles.
//!
//! Extracted from `v8host.rs` under the core-stabilization program — see `crate::usermsg` for the
//! shape (a feature owns its state, natives, dispatch and teardown together).

use crate::v8host::set_native;

thread_local! {
    static BAN_CACHE: std::cell::RefCell<std::collections::HashMap<String, (i64, String)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// One-shot guard so bans.json loads once (mirrors `admin::ADMIN_FILE_LOADED`).
    static BAN_LOADED: std::cell::Cell<bool> = std::cell::Cell::new(false);
}


/// `__s2_ban_set(steamid, until, reason)` — insert/overwrite a ban. `until == 0` = permanent, else unix-sec expiry.
fn s2_ban_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let until = args.get(1).number_value(scope).unwrap_or(0.0) as i64;
        let reason = args.get(2).to_rust_string_lossy(scope);
        BAN_CACHE.with(|m| { m.borrow_mut().insert(sid, (until, reason)); });
    }));
}

/// `__s2_ban_get(steamid) -> string | null` — JSON `{"until":N,"reason":"..."}` if present, else null.
fn s2_ban_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let entry = BAN_CACHE.with(|m| m.borrow().get(&sid).cloned());
        match entry {
            Some((until, reason)) => {
                let json = serde_json::json!({ "until": until, "reason": reason }).to_string();
                if let Some(js) = v8::String::new(scope, &json) { rv.set(js.into()); }
            }
            None => rv.set_null(),
        }
    }));
}

/// `__s2_ban_remove(steamid) -> boolean` — remove; returns whether the key was present.
fn s2_ban_remove(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let removed = BAN_CACHE.with(|m| m.borrow_mut().remove(&sid).is_some());
        rv.set_bool(removed);
    }));
}

/// `__s2_ban_clear()` — wipe the cache (Bans.reload re-parses the file into it).
fn s2_ban_clear(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        BAN_CACHE.with(|m| m.borrow_mut().clear());
    }));
}

/// `__s2_ban_list() -> string` — JSON array `[{"steamid":"..","until":N,"reason":".."}]`.
fn s2_ban_list(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let items: Vec<serde_json::Value> = BAN_CACHE.with(|m| {
            m.borrow().iter()
                .map(|(sid, (until, reason))| serde_json::json!({
                    "steamid": sid, "until": until, "reason": reason,
                }))
                .collect()
        });
        let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
        if let Some(js) = v8::String::new(scope, &json) { rv.set(js.into()); }
    }));
}

/// `__s2_ban_mark_loaded() -> boolean` — returns the PRIOR loaded state, then sets it true (one-shot guard).
fn s2_ban_mark_loaded(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let prev = BAN_LOADED.with(|c| { let p = c.get(); c.set(true); p });
        rv.set_bool(prev);
    }));
}

/// Returns `Some(reason)` if `xuid` is currently banned (perm or unexpired), else `None`.
/// Retained as an available synchronous ban-check primitive (via the `s2script_core_ban_check` ffi
/// export); no longer called by the shim since sub-project 3 moved enforcement to the JS onConnect path.
pub fn ban_check(xuid: u64, now: i64) -> Option<String> {
    let key = xuid.to_string();
    BAN_CACHE.with(|m| {
        m.borrow().get(&key).and_then(|(until, reason)| {
            if *until == 0 || *until > now { Some(reason.clone()) } else { None }
        })
    })
}
/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_ban_set", s2_ban_set);
    set_native(scope, global_obj, "__s2_ban_get", s2_ban_get);
    set_native(scope, global_obj, "__s2_ban_remove", s2_ban_remove);
    set_native(scope, global_obj, "__s2_ban_clear", s2_ban_clear);
    set_native(scope, global_obj, "__s2_ban_list", s2_ban_list);
    set_native(scope, global_obj, "__s2_ban_mark_loaded", s2_ban_mark_loaded);
}

/// Reset on a core re-init. `AfterIsolateDrop`: plain Rust state, no V8 handles.
pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    crate::process_singletons::register("BAN_CACHE", AfterIsolateDrop,
        Box::new(|| BAN_CACHE.with(|m| m.borrow_mut().clear())));
    crate::process_singletons::register("BAN_LOADED", AfterIsolateDrop,
        Box::new(|| BAN_LOADED.with(|c| c.set(false))));
}

// Per-feature tests over the SHARED in-isolate harness (`v8host::frame_tests`) — see `crate::usermsg`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{eval_in_context_string, logger, LOG};
    use crate::v8host::{create_plugin_context, eval_in_context, init, shutdown};
    /// Slice 6.18 Task 1: `__s2_ban_*` natives round-trip through `BAN_CACHE`; the `@s2script/bans`
    /// prelude parses a `{steamid:{until,reason}}` blob (skipping `_help`), degrades on malformed JSON,
    /// and exposes `Bans.add/remove/get/list/reload`.
    #[test]
    fn bans_natives_and_prelude() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // set two SteamIDs (one perm, one timed) → get returns the right JSON, list has 2 entries.
        eval_in_context("p", "__s2_ban_set('111', 0, 'grief'); __s2_ban_set('222', 5000000000, 'cheat');").unwrap();
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_get('111')).until)"), "0");
        assert_eq!(eval_in_context_string("p", "JSON.parse(__s2_ban_get('111')).reason"), "grief");
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_get('222')).until)"), "5000000000");
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_list()).length)"), "2");
        // absent → get is null.
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_get('999'))"), "null");
        // remove returns true (was present); a second remove is false; the get is then null.
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_remove('111'))"), "true");
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_remove('111'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_get('111'))"), "null");
        // clear empties the list.
        eval_in_context("p", "__s2_ban_clear();").unwrap();
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_list()).length)"), "0");
        // mark_loaded: the prelude already called it in create_plugin_context, so it now returns true.
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_mark_loaded())"), "true");
        // the @s2script/bans module is wired.
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_bans.Bans.add"), "function");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_bans.Bans.reload"), "function");
        // prelude parseFile: a {steamid:{until,reason}} blob populates via the natives (skips _help).
        eval_in_context("p", r#"__s2_ban_parseFile('{"_help":"ignore me","333":{"until":0,"reason":"x"}}');"#).unwrap();
        assert_eq!(eval_in_context_string("p", "JSON.parse(__s2_ban_get('333')).reason"), "x");
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_get('_help'))"), "null");
        // malformed JSON degrades without throwing.
        eval_in_context("p", "__s2_ban_parseFile('not json');").unwrap();
        shutdown();
    }

    /// Slice 6.18 Task 1: `ban_check` — banned iff present AND (`until == 0` perm OR `until > now`).
    /// An expired entry and an absent SteamID both read as not-banned.
    #[test]
    fn ban_check_expiry_semantics() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        let now: i64 = 1_000_000;
        // perm (until=0) → banned; the reason is returned.
        eval_in_context("p", "__s2_ban_set('111', 0, 'perm-reason');").unwrap();
        assert_eq!(ban_check(111, now), Some("perm-reason".to_string()));
        // future expiry → banned.
        eval_in_context("p", "__s2_ban_set('222', 1000100, 'timed');").unwrap();
        assert_eq!(ban_check(222, now), Some("timed".to_string()));
        // past expiry → not banned.
        eval_in_context("p", "__s2_ban_set('333', 999900, 'expired');").unwrap();
        assert_eq!(ban_check(333, now), None);
        // absent → not banned.
        assert_eq!(ban_check(444, now), None);
        shutdown();
    }
}
