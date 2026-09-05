//! Host-owned reference counts for a game-declared entity boolean switch per player slot.
//! No game names, entity pointers, JS callbacks, or V8 handles survive an invocation.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use crate::v8host::{current_plugin, engine_ops, set_native};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key { game: String, call: String, index: i32, id: u64, slot: i32 }
type Holders = HashSet<(String, String)>;
thread_local! {
    // An empty holder set means the last disable failed and needs a host-side retry.
    static LEASES: RefCell<HashMap<Key, Holders>> = RefCell::new(HashMap::new());
    static BUSY: RefCell<HashSet<Key>> = RefCell::new(HashSet::new());
    static EPOCH: Cell<u64> = const { Cell::new(0) };
}
fn invalidate_pending() { EPOCH.with(|e| e.set(e.get().wrapping_add(1))); }
fn live(key: &Key) -> bool { crate::entity_live::engine_serial_for(key.index, key.id).is_some() }

fn invoke(key: &Key, on: bool) -> Result<(), String> {
    let serial = crate::entity_live::engine_serial_for(key.index, key.id)
        .ok_or("shared entity switch: stale entity")?;
    // Never use a cached call id after game-package replacement.
    if crate::gamedata_calls::game_package_owner().as_deref() != Some(key.game.as_str()) {
        return Err("shared entity switch: game package changed".into());
    }
    let plan = crate::gamedata_calls::plan(&key.game, &key.call)
        .ok_or_else(|| format!("unavailable: {}: {}", key.call,
            crate::gamedata_calls::status(&key.game, &key.call)))?;
    if plan.receiverless || plan.via.is_some() || plan.ret_code != crate::gamedata_calls::RET_VOID
        || plan.args != ["int", "bool"] {
        return Err(format!("unavailable: {} must be an entity void(int,bool) binding without via", key.call));
    }
    let func = engine_ops().and_then(|o| o.engine_call_invoke)
        .ok_or("unavailable: shared entity switch engine operation")?;
    let gp = [key.slot as u64, u64::from(on)];
    let kinds = [crate::gamedata_calls::GP_SCALAR; 2];
    let mut ret = 0;
    let bypass_ids = crate::gamedata_hooks::bypass_ids_for_call(&key.game, &key.call);
    let ops = engine_ops();
    let bypass = match (ops.and_then(|o| o.hook_arm_bypass), ops.and_then(|o| o.hook_disarm_bypass)) {
        (Some(arm), Some(disarm)) => Some((arm, disarm)),
        _ => None,
    };
    if let Some((arm, _)) = bypass { for id in &bypass_ids { arm(*id); } }
    let ok = func(plan.call_id, key.index, serial, -1, gp.as_ptr(), kinds.as_ptr(), 2,
        std::ptr::null(), 0, std::ptr::null(), std::ptr::null(), plan.ret_code, &mut ret);
    if let Some((_, disarm)) = bypass { for id in &bypass_ids { disarm(*id); } }
    if ok == 0 { Err(format!("unavailable: {} invocation failed", key.call)) } else { Ok(()) }
}

fn change(owner: &str, key: Key, token: Option<String>, on: bool,
    outbound: impl FnOnce(&Key, bool) -> Result<(), String>) -> Result<(), String> {
    if key.slot < 0 { return Err("shared entity switch: needs a player slot".into()); }
    if on && token.is_none() { return Err("shared entity switch: acquire needs a token".into()); }
    if BUSY.with(|b| b.borrow().contains(&key)) {
        return Err("shared entity switch: transition in progress; retry later".into());
    }
    if !live(&key) {
        LEASES.with(|l| { l.borrow_mut().remove(&key); });
        return if on { Err("shared entity switch: stale entity".into()) } else { Ok(()) };
    }
    let previous = LEASES.with(|l| l.borrow().get(&key).cloned());
    let mut next = previous.clone().unwrap_or_default();
    if on { next.insert((owner.to_string(), token.unwrap())); }
    else if let Some(token) = token { next.remove(&(owner.to_string(), token)); }
    else { next.retain(|(who, _)| who != owner); }
    let was_on = previous.is_some();
    let want_on = !next.is_empty();
    if was_on == want_on {
        if want_on { LEASES.with(|l| { l.borrow_mut().insert(key, next); }); }
        return Ok(());
    }
    BUSY.with(|b| { b.borrow_mut().insert(key.clone()); });
    let epoch = EPOCH.with(Cell::get);
    let result = outbound(&key, want_on);
    BUSY.with(|b| { b.borrow_mut().remove(&key); });
    if EPOCH.with(Cell::get) != epoch {
        // A lifecycle event ran during the engine call. It supersedes this attempted claim.
        // BUSY excluded another claimant for this key, so rolling back cannot clear their switch.
        if want_on && result.is_ok() && live(&key) {
            LEASES.with(|l| { l.borrow_mut().entry(key.clone()).or_default(); });
            retry_key(&key, true);
        }
        if !want_on && result.is_ok() { LEASES.with(|l| { l.borrow_mut().remove(&key); }); }
        return Err("shared entity switch: lifecycle changed during transition; retry later".into());
    }
    if let Err(err) = result {
        // Closing presenters may discard their JS view. The host keeps failed last-off as an
        // ownerless pending release, so a transient failure cannot strand capture indefinitely.
        if !want_on { LEASES.with(|l| { l.borrow_mut().insert(key, Holders::new()); }); }
        return Err(err);
    } // Failed first-on never records a lease.
    LEASES.with(|l| {
        if want_on { l.borrow_mut().insert(key, next); }
        else { l.borrow_mut().remove(&key); }
    });
    Ok(())
}

fn retry_key(key: &Key, warn: bool) {
    if !LEASES.with(|l| l.borrow().get(key).is_some_and(HashSet::is_empty)) { return; }
    if BUSY.with(|b| b.borrow().contains(key)) { return; }
    if !live(key) { LEASES.with(|l| { l.borrow_mut().remove(key); }); return; }
    BUSY.with(|b| { b.borrow_mut().insert(key.clone()); });
    let result = crate::dispatch::defer_while(|| invoke(key, false));
    BUSY.with(|b| { b.borrow_mut().remove(key); });
    if result.is_ok() { LEASES.with(|l| { l.borrow_mut().remove(key); }); }
    else if warn { if let Err(err) = result { crate::v8host::log_warn(&err); } }
}

pub(crate) fn retry_pending() {
    let keys: Vec<_> = LEASES.with(|l| l.borrow().iter()
        .filter(|(_, holders)| holders.is_empty()).map(|(k, _)| k.clone()).collect());
    for key in keys { retry_key(&key, false); }
}

fn remove_where(mut matches: impl FnMut(&Key, &(String, String)) -> bool) {
    invalidate_pending();
    let keys: Vec<_> = LEASES.with(|l| {
        let mut leases = l.borrow_mut();
        leases.retain(|key, _| live(key));
        for (key, holders) in leases.iter_mut() { holders.retain(|h| !matches(key, h)); }
        leases.iter().filter(|(_, h)| h.is_empty()).map(|(k, _)| k.clone()).collect()
    });
    for key in keys { retry_key(&key, true); }
}
pub(crate) fn clear_slot(slot: i32) { remove_where(|key, _| key.slot == slot); }
fn remove_owner(owner: &str) { remove_where(|_, (who, _)| who == owner); }
pub(crate) fn reset() {
    invalidate_pending();
    LEASES.with(|l| l.borrow_mut().clear());
}
pub(crate) fn prune_dead() {
    invalidate_pending();
    LEASES.with(|l| l.borrow_mut().retain(|k, _| live(k)));
}
pub(crate) fn register_store() {
    crate::owner_stores::register("SHARED_ENTITY_SWITCH", Box::new(remove_owner),
        Box::new(|_| {}), Box::new(reset));
}

pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    set_native(scope, global, "__s2_shared_entity_switch", native);
}
fn native(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = current_plugin(scope).ok_or("shared entity switch: missing plugin context")?;
        if matches!(crate::v8host::plugin_phase(&owner),
            Some(crate::plugin::Phase::Unloading | crate::plugin::Phase::Failed)) {
            return Err("shared entity switch: plugin is unloading".into());
        }
        let game = crate::gamedata_calls::game_package_owner().ok_or("unavailable: game package")?;
        let key = Key { game, call: args.get(0).to_rust_string_lossy(scope),
            index: args.get(1).int32_value(scope).unwrap_or(-1),
            id: args.get(2).number_value(scope).unwrap_or(0.0) as u64,
            slot: args.get(3).int32_value(scope).unwrap_or(-1) };
        let token = if args.get(4).is_null_or_undefined() { None }
            else { Some(args.get(4).to_rust_string_lossy(scope)) };
        let on = args.get(5).boolean_value(scope);
        change(&owner, key, token, on, |key, on| crate::nest::with_outbound(&args, || invoke(key, on)))
    })).unwrap_or_else(|_| Err("shared entity switch: internal failure".into()));
    match result {
        Ok(()) => rv.set_null(),
        Err(err) => if let Some(s) = v8::String::new(scope, &err) { rv.set(s.into()); },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{c_char, c_int};
    use crate::v8host::{self, frame_tests::{dummy_logger, eval_in_context_string, mock_event_ops}, S2EngineOps};
    thread_local! {
        static CALLS: RefCell<Vec<(i32, i32, i32, bool)>> = RefCell::new(Vec::new());
        static FAIL: Cell<bool> = const { Cell::new(false) };
        static REENTER: Cell<bool> = const { Cell::new(false) };
        static DELIVERY: Cell<Option<crate::dispatch::Delivery>> = const { Cell::new(None) };
    }
    extern "C" fn resolve(_: *const c_char, _: *const c_char, _: *const c_char,
        _: *const c_char, _: *const c_char, _: c_int, _: *const c_char, _: *mut c_char, _: c_int) -> c_int { 7 }
    extern "C" fn invoke_fake(_: c_int, index: c_int, serial: c_int, _: c_int,
        gp: *const u64, _: *const u8, _: c_int, _: *const f64, _: c_int,
        _: *const *const c_char, _: *const f32, _: c_int, _: *mut u64) -> c_int {
        let (slot, on) = unsafe { (*gp as i32, *gp.add(1) != 0) };
        CALLS.with(|c| c.borrow_mut().push((index, serial, slot, on)));
        if REENTER.with(|r| r.replace(false)) {
            DELIVERY.with(|d| d.set(Some(crate::client::replay_client_event("settingschanged", slot))));
        }
        if FAIL.with(Cell::get) { 0 } else { 1 }
    }
    fn setup() -> u64 {
        let _ = v8host::init(dummy_logger());
        CALLS.with(|c| c.borrow_mut().clear()); FAIL.with(|f| f.set(false));
        REENTER.with(|r| r.set(false)); DELIVERY.with(|d| d.set(None));
        v8host::set_engine_ops(Some(S2EngineOps {
            engine_call_resolve: Some(resolve), engine_call_invoke: Some(invoke_fake), ..mock_event_ops()
        }));
        crate::gamedata_calls::register_game_package("@test/game", r#"{
          "signatures":{"Switch":{"linuxsteamrt64":{"module":"server","pattern":"55 48","resolve":"direct"}}},
          "calls":{"toggle":{"receiver":{"kind":"entity"},"target":{"kind":"signature","name":"Switch"},
          "args":["int","bool"],"returns":"void"}}}"#);
        assert_eq!(crate::gamedata_calls::status("game-package:@test/game", "toggle"), "available");
        for owner in ["switch_a", "switch_b"] { v8host::create_plugin_context(owner); }
        crate::entity_live::on_created(10, 123)
    }
    fn run(owner: &str, id: u64, token: Option<&str>, on: bool) -> String {
        let token = token.map(|t| format!("{t:?}")).unwrap_or("null".into());
        eval_in_context_string(owner, &format!("JSON.stringify(__s2_shared_entity_switch('toggle',10,{id},2,{token},{on}))"))
    }
    fn calls() -> Vec<bool> { CALLS.with(|c| c.borrow().iter().map(|c| c.3).collect()) }
    fn done() { v8host::set_engine_ops(None); v8host::shutdown(); }

    #[test]
    fn two_real_contexts_share_first_on_last_off_and_distinct_panel_manual_tokens() {
        let id = setup();
        assert_eq!(run("switch_a", id, Some("panel:a"), true), "null");
        assert_eq!(run("switch_a", id, Some("panel:b"), true), "null");
        assert_eq!(run("switch_a", id, Some("manual:*"), true), "null");
        assert_eq!(run("switch_b", id, Some("panel:a"), true), "null");
        assert_eq!(calls(), [true]);
        assert_eq!(run("switch_a", id, Some("manual:*"), false), "null");
        assert_eq!(run("switch_a", id, None, false), "null");
        assert_eq!(calls(), [true], "owner-wide forget cannot disable another context");
        assert_eq!(run("switch_b", id, Some("panel:a"), false), "null");
        assert_eq!(calls(), [true, false]);
        assert_eq!(run("switch_a", id, None, false), "null");
        assert_eq!(calls(), [true, false], "empty forget is idempotent");
        done();
    }

    #[test]
    fn unload_is_host_owned_and_failed_last_disable_is_retried_without_js() {
        let id = setup();
        run("switch_a", id, Some("a"), true); run("switch_b", id, Some("b"), true);
        v8host::unload_plugin("switch_a");
        assert_eq!(calls(), [true]);
        FAIL.with(|f| f.set(true));
        v8host::unload_plugin("switch_b");
        assert_eq!(calls(), [true, false]);
        assert!(LEASES.with(|l| l.borrow().values().all(HashSet::is_empty)));
        FAIL.with(|f| f.set(false)); retry_pending();
        assert_eq!(calls(), [true, false, false]);
        assert!(LEASES.with(|l| l.borrow().is_empty()));
        done();
    }

    #[test]
    fn failed_enable_never_records_a_lease_and_failed_disable_can_be_retried() {
        let id = setup(); FAIL.with(|f| f.set(true));
        assert!(run("switch_a", id, Some("a"), true).contains("invocation failed"));
        assert!(LEASES.with(|l| l.borrow().is_empty()));
        FAIL.with(|f| f.set(false)); assert_eq!(run("switch_b", id, Some("b"), true), "null");
        FAIL.with(|f| f.set(true)); assert!(run("switch_b", id, Some("b"), false).contains("invocation failed"));
        FAIL.with(|f| f.set(false)); assert_eq!(run("switch_b", id, Some("b"), false), "null");
        assert_eq!(calls(), [true, true, false, false]); done();
    }

    #[test]
    fn failed_close_retries_without_a_js_handle_and_reacquire_cancels_pending_disable() {
        let id = setup(); run("switch_a", id, Some("closed"), true);
        FAIL.with(|f| f.set(true));
        assert!(run("switch_a", id, Some("closed"), false).contains("invocation failed"));
        assert!(LEASES.with(|l| l.borrow().values().all(HashSet::is_empty)));
        FAIL.with(|f| f.set(false));
        v8host::frame_async_drain();
        assert_eq!(calls(), [true, false, false], "host retries even though close dropped its JS handle");
        run("switch_a", id, Some("next"), true);
        FAIL.with(|f| f.set(true)); run("switch_a", id, Some("next"), false);
        FAIL.with(|f| f.set(false));
        let pending = LEASES.with(|l| l.borrow().keys().next().unwrap().clone());
        assert_eq!(run("switch_b", id, Some("replacement"), true), "null");
        let before = calls();
        retry_key(&pending, false); // A snapshot taken before the new acquire must also skip it.
        v8host::frame_async_drain();
        assert_eq!(calls(), before, "new holder cancels pending disable without toggling capture");
        done();
    }

    #[test]
    fn unavailable_or_wrong_shape_bindings_never_acquire() {
        let id = setup();
        let missing = eval_in_context_string("switch_a", &format!(
            "__s2_shared_entity_switch('missing',10,{id},2,'a',true)"));
        assert!(missing.contains("unavailable"));
        crate::gamedata_calls::register_game_package("@test/game", r#"{
          "signatures":{"Switch":{"linuxsteamrt64":{"module":"server","pattern":"55 48","resolve":"direct"}}},
          "calls":{"toggle":{"receiver":{"kind":"none"},"target":{"kind":"signature","name":"Switch"},
          "args":["int","bool"],"returns":"void"}}}"#);
        assert!(run("switch_a", id, Some("a"), true).contains("void(int,bool)"));
        assert!(calls().is_empty());
        assert!(LEASES.with(|l| l.borrow().is_empty()));
        done();
    }

    #[test]
    fn disconnect_without_js_subscribers_clears_every_owner_before_slot_reuse() {
        let id = setup(); run("switch_a", id, Some("a"), true); run("switch_b", id, Some("b"), true);
        let _ = crate::client::dispatch_client_event("disconnect", 2);
        assert_eq!(calls(), [true, false]);
        assert_eq!(run("switch_a", id, Some("fresh"), true), "null");
        run("switch_b", id, Some("b"), false);
        assert_eq!(calls(), [true, false, true], "departed owner cannot clear the new occupant's lease");
        done();
    }

    #[test]
    fn dead_entity_and_map_reset_never_disable_a_replacement_entity() {
        let id = setup(); run("switch_a", id, Some("old"), true);
        crate::entity_live::on_deleted(10, 123);
        let next = crate::entity_live::on_created(10, 456);
        assert_eq!(run("switch_b", next, Some("new"), true), "null");
        assert_eq!(run("switch_a", id, Some("old"), false), "null");
        assert_eq!(calls(), [true, true]);
        crate::entity_live::clear_for_map_transition();
        v8host::unload_plugin("switch_b");
        assert_eq!(calls(), [true, true]);
        let third = crate::entity_live::on_created(10, 789);
        assert_eq!(run("switch_a", third, Some("new-map"), true), "null");
        assert_eq!(calls(), [true, true, true]); done();
    }

    #[test]
    fn reentrant_transition_is_named_and_cleanup_defers_callbacks_until_after_sweep() {
        let id = setup();
        v8host::eval_in_context("switch_b", &format!(r#"
            globalThis.result = 'not-called';
            __s2_client_subscribe('settingschanged', () => {{
                globalThis.result = __s2_shared_entity_switch('toggle',10,{id},2,'nested',true);
            }});
        "#)).unwrap();
        REENTER.with(|r| r.set(true));
        assert_eq!(run("switch_a", id, Some("a"), true), "null");
        assert!(eval_in_context_string("switch_b", "globalThis.result").contains("transition in progress"));
        REENTER.with(|r| r.set(true));
        v8host::unload_plugin("switch_a");
        assert_eq!(DELIVERY.with(Cell::get), Some(crate::dispatch::Delivery::Deferred));
        // The shim would replay only after the owner sweep; replay uses current subscriber books.
        let _ = crate::client::replay_client_event("settingschanged", 2);
        assert_eq!(eval_in_context_string("switch_b", "JSON.stringify(globalThis.result)"), "null");
        assert_eq!(calls(), [true, false, true]); done();
    }
}
