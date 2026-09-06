//! Connection/entity-bound, owner-checked surface arbitration. Rendering policy and binding
//! names are supplied by the game adapter. The host retains only owned data, never JS callbacks.
//!
//! A winning reservation hides/releases the outgoing presentation before returning `ready`.
//! Activation asserts that the adapter has successfully painted. A restored winner remains
//! `waiting` until a later host frame, then `ready` until a fresh successful paint/activation.
//! Suspension bindings must be idempotent: failed retirement is retried by the host. Adapters
//! must invalidate their local paint caches after suspension; these calls bypass those caches.
//! Game descriptors have exactly the existing __s2_game_call_invoke permission boundary: the
//! host chooses the first-party game owner, and plugin-private descriptors cannot be selected.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CString;
use serde::Deserialize;
use crate::v8host::{current_plugin, engine_ops, set_native};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key { surface: String, index: i32, entity: u64, slot: i32, client: u64 }
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture { call: String, token: String }
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Suspend { call: String, args: Vec<serde_json::Value> }
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Adapter {
    capture: Option<Capture>,
    suspend: Option<Suspend>,
}
#[derive(Clone, Debug)]
struct Lease {
    token: u64, owner: String, generation: u64, key: Key, priority: i32,
    game: Option<String>, adapter: Adapter,
    activated: bool, activated_in: Option<u64>, ready_after: u64,
}
#[derive(Default)]
struct Registry { leases: HashMap<u64, Lease>, pending: Vec<Lease>, frame: u64 }
impl Registry {
    fn winner(&self, key: &Key) -> Option<u64> {
        self.leases.values().filter(|l| &l.key == key)
            .max_by_key(|l| (l.priority, l.token)).map(|l| l.token)
    }
    fn state(&self, token: u64, owner: &str) -> &'static str {
        let Some(l) = self.leases.get(&token).filter(|l| l.owner == owner) else { return "invalid" };
        if self.winner(&l.key) != Some(token) { return "covered"; }
        if self.pending.iter().any(|p| p.key == l.key) || self.frame < l.ready_after { return "waiting"; }
        if l.activated { "active" } else { "ready" }
    }
    fn suspend(&mut self, token: u64) -> Option<Lease> {
        let l = self.leases.get_mut(&token)?;
        l.activated = false;
        l.ready_after = self.frame.saturating_add(1);
        Some(l.clone())
    }
    fn remove(&mut self, token: u64) -> Option<(Lease, bool)> {
        let was_winner = self.winner(&self.leases.get(&token)?.key) == Some(token);
        let removed = self.leases.remove(&token)?;
        if was_winner {
            if let Some(next) = self.winner(&removed.key) { self.suspend(next); }
        }
        // Only the winner owns presentation effects, including a potentially partial paint
        // before activation. A covered token either never painted or was retired at takeover.
        // Preserve this fact across removal: its hide action may name the winner's SAME root.
        Some((removed, was_winner))
    }
}
thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
    // Never reset on map change, plugin reload, or host re-init.
    static NEXT_TOKEN: Cell<u64> = const { Cell::new(1) };
    static TRANSITION: Cell<bool> = const { Cell::new(false) };
}
struct Transition;
impl Transition {
    fn enter() -> Result<Self, Failure> {
        if TRANSITION.with(|b| b.replace(true)) { Err(fail("Busy", "surface transition in progress")) }
        else { Ok(Self) }
    }
}
impl Drop for Transition { fn drop(&mut self) { TRANSITION.with(|b| b.set(false)); } }
#[derive(Debug)]
struct Failure { code: &'static str, message: String }
fn fail(code: &'static str, message: impl Into<String>) -> Failure { Failure { code, message: message.into() } }
fn identity_live(l: &Lease) -> bool {
    crate::client::matches(l.key.slot, l.key.client)
        && crate::entity_live::engine_serial_for(l.key.index, l.key.entity).is_some()
}
fn live(l: &Lease) -> bool {
    identity_live(l) && crate::v8host::owner_is_live(&l.owner, l.generation)
}
fn bounded(s: &str) -> bool { !s.is_empty() && s.len() <= 256 && !s.contains('\0') }

fn suspension_plan(game: &str, action: &Suspend) -> Result<crate::gamedata_calls::InvokePlan, String> {
    let plan = crate::gamedata_calls::plan(game, &action.call)
        .ok_or_else(|| format!("unavailable: {}: {}", action.call, crate::gamedata_calls::status(game, &action.call)))?;
    if plan.receiverless || plan.via.is_some() || plan.ret_code != crate::gamedata_calls::RET_VOID
        || plan.args.first().map(String::as_str) != Some("int")
        || plan.args.iter().skip(1).any(|kind| !matches!(kind.as_str(), "int" | "bool" | "string" | "utlstring")) {
        return Err("surface suspension needs entity void(int, scalar/string...) binding without via".into());
    }
    Ok(plan)
}
fn suspension_args(plan: &crate::gamedata_calls::InvokePlan, action: &Suspend) -> Result<(), Failure> {
    if plan.args.len() != action.args.len() + 1 {
        return Err(fail("InvalidArgument", "surface suspension argument count does not match binding"));
    }
    for (kind, arg) in plan.args.iter().skip(1).zip(&action.args) {
        let valid = match kind.as_str() {
            "int" => arg.as_i64().is_some_and(|v| i32::try_from(v).is_ok()),
            "bool" => arg.is_boolean(),
            "string" | "utlstring" => arg.as_str().is_some_and(bounded),
            _ => false,
        };
        if !valid { return Err(fail("InvalidArgument", "surface suspension argument does not match binding")); }
    }
    Ok(())
}
fn validate_adapter(adapter: &Adapter, game: Option<&str>) -> Result<(), Failure> {
    if adapter.capture.is_none() && adapter.suspend.is_none() { return Ok(()); }
    let game = game.ok_or_else(|| fail("Unavailable", "surface binding needs a game package"))?;
    if let Some(c) = &adapter.capture {
        if !bounded(&c.call) || !bounded(&c.token) { return Err(fail("InvalidArgument", "invalid capture binding/token")); }
        let p = crate::gamedata_calls::plan(game, &c.call).ok_or_else(|| fail("Unavailable", "capture binding unavailable"))?;
        if p.receiverless || p.via.is_some() || p.ret_code != crate::gamedata_calls::RET_VOID || p.args != ["int", "bool"] {
            return Err(fail("InvalidArgument", "capture needs entity void(int,bool) binding without via"));
        }
    }
    if let Some(s) = &adapter.suspend {
        if !bounded(&s.call) || s.args.len() > 7 { return Err(fail("InvalidArgument", "invalid suspension binding/arguments")); }
        let plan = suspension_plan(game, s).map_err(|e| fail("Unavailable", e))?;
        suspension_args(&plan, s)?;
    }
    Ok(())
}
fn invoke_suspend(l: &Lease, action: &Suspend) -> Result<(), String> {
    let game = l.game.as_deref().ok_or("surface game package unavailable")?;
    if crate::gamedata_calls::game_package_owner().as_deref() != Some(game) { return Err("surface game package changed".into()); }
    let plan = suspension_plan(game, action)?;
    suspension_args(&plan, action).map_err(|e| e.message)?;
    let serial = crate::entity_live::engine_serial_for(l.key.index, l.key.entity).ok_or("surface entity expired")?;
    let invoke = engine_ops().and_then(|o| o.engine_call_invoke).ok_or("surface engine operation unavailable")?;
    let mut gp = vec![l.key.slot as u64];
    let mut kinds = vec![crate::gamedata_calls::GP_SCALAR];
    let mut strings = Vec::new();
    for (kind, arg) in plan.args.iter().skip(1).zip(&action.args) {
        match kind.as_str() {
            "int" => { gp.push(arg.as_i64().unwrap() as u64); kinds.push(crate::gamedata_calls::GP_SCALAR); }
            "bool" => { gp.push(u64::from(arg.as_bool().unwrap())); kinds.push(crate::gamedata_calls::GP_SCALAR); }
            _ => {
                gp.push(strings.len() as u64);
                strings.push(CString::new(arg.as_str().unwrap()).map_err(|_| "invalid surface string")?);
                kinds.push(if kind == "utlstring" { crate::gamedata_calls::GP_UTLSTRING } else { crate::gamedata_calls::GP_STRING });
            }
        }
    }
    let ptrs: Vec<_> = strings.iter().map(|s| s.as_ptr()).collect();
    let mut ret = 0;
    let bypass_ids = crate::gamedata_hooks::bypass_ids_for_call(game, &action.call);
    let ops = engine_ops();
    let bypass = match (ops.and_then(|o| o.hook_arm_bypass), ops.and_then(|o| o.hook_disarm_bypass)) {
        (Some(arm), Some(disarm)) => Some((arm, disarm)), _ => None,
    };
    if let Some((arm, _)) = bypass { for id in &bypass_ids { arm(*id); } }
    let ok = crate::dispatch::defer_while(|| invoke(plan.call_id, l.key.index, serial, -1,
        gp.as_ptr(), kinds.as_ptr(), gp.len() as i32, std::ptr::null(), 0,
        ptrs.as_ptr(), std::ptr::null(), plan.ret_code, &mut ret));
    if let Some((_, disarm)) = bypass { for id in &bypass_ids { disarm(*id); } }
    if ok == 0 { Err(format!("surface suspension {} invocation failed", action.call)) } else { Ok(()) }
}
fn retire_presentation(l: &Lease) -> Result<(), String> {
    if !identity_live(l) { return Ok(()); }
    let hidden = l.adapter.suspend.as_ref().map_or(Ok(()), |s| invoke_suspend(l, s));
    // Hiding can fail. Capture retirement still runs, and is tracked by the existing switch.
    let capture = if let (Some(c), Some(game)) = (&l.adapter.capture, &l.game) {
        crate::shared_entity_switch::release_recorded(&l.owner, game, &c.call,
            l.key.index, l.key.entity, l.key.slot, &c.token)
    } else { Ok(()) };
    hidden.and(capture)
}
fn retire_or_queue(l: Lease) -> Result<(), String> {
    match retire_presentation(&l) {
        Ok(()) => Ok(()),
        Err(e) => {
            if identity_live(&l) {
                REGISTRY.with(|r| { let mut r = r.borrow_mut();
                    if !r.pending.iter().any(|p| p.token == l.token) { r.pending.push(l); }
                });
            }
            Err(e)
        }
    }
}

fn reserve(owner: &str, key: Key, priority: i32, adapter: Adapter) -> Result<u64, Failure> {
    let _transition = Transition::enter()?;
    if !crate::client::matches(key.slot, key.client) { return Err(fail("StaleClient", "surface client expired")); }
    if crate::entity_live::engine_serial_for(key.index, key.entity).is_none() { return Err(fail("NotReady", "surface entity unavailable")); }
    let game = crate::gamedata_calls::game_package_owner();
    validate_adapter(&adapter, game.as_deref())?;
    let generation = crate::v8host::plugin_generation(owner);
    if !crate::v8host::owner_is_live(owner, generation) { return Err(fail("Released", "surface owner expired")); }
    let old = REGISTRY.with(|r| {
        let r = r.borrow();
        if r.leases.values().filter(|l| l.owner == owner).count() >= 1024 || r.leases.len() + r.pending.len() >= 16384 {
            return Err(fail("PoolExhausted", "surface lease limit reached"));
        }
        if r.pending.iter().any(|l| l.key == key) { return Err(fail("Busy", "surface retirement pending")); }
        Ok(r.winner(&key).filter(|t| r.leases[t].priority <= priority))
    })?;
    if let Some(old) = old {
        let outgoing = REGISTRY.with(|r| r.borrow_mut().suspend(old)).unwrap();
        retire_or_queue(outgoing).map_err(|e| fail("Unavailable", e))?;
    }
    let token = NEXT_TOKEN.with(|n| { let t = n.get(); n.set(t.checked_add(1).expect("surface token space exhausted")); t });
    let l = Lease { token, owner: owner.into(), generation, key, priority, game, adapter,
        activated: false, activated_in: None, ready_after: 0 };
    // Engine calls may synchronously retire the client/entity or unload the caller.
    if !live(&l) { return Err(fail("Released", "surface lifetime changed during reservation")); }
    REGISTRY.with(|r| { r.borrow_mut().leases.insert(token, l); });
    Ok(token)
}
fn state(owner: &str, token: u64) -> &'static str {
    REGISTRY.with(|r| {
        let r = r.borrow();
        if !r.leases.get(&token).is_some_and(live) { "invalid" } else { r.state(token, owner) }
    })
}
fn activate(owner: &str, token: u64) -> bool {
    if TRANSITION.with(Cell::get) || state(owner, token) != "ready" { return false; }
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let l = r.leases.get_mut(&token).unwrap();
        l.activated = true; l.activated_in = crate::dispatch::current_epoch();
    });
    true
}
fn active(owner: &str, token: u64) -> bool {
    if state(owner, token) != "active" { return false; }
    REGISTRY.with(|r| {
        let r = r.borrow(); let l = &r.leases[&token];
        l.activated_in.is_none() || l.activated_in != crate::dispatch::current_epoch()
    })
}
fn release(owner: &str, token: u64) -> bool {
    let Ok(_transition) = Transition::enter() else { return false };
    let removed = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        if !r.leases.get(&token).is_some_and(|l| l.owner == owner) { return None; }
        r.remove(token)
    });
    let Some((l, was_winner)) = removed else { return false };
    if was_winner {
        if let Err(e) = retire_or_queue(l) { crate::v8host::log_warn(&e); }
    }
    true
}
fn remove_where(mut matches: impl FnMut(&Lease) -> bool) {
    let removed = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let tokens: Vec<_> = r.leases.values().filter(|l| matches(l)).map(|l| l.token).collect();
        tokens.into_iter().filter_map(|t| r.remove(t))
            .filter_map(|(l, was_winner)| was_winner.then_some(l)).collect::<Vec<_>>()
    });
    for l in removed { if let Err(e) = retire_or_queue(l) { crate::v8host::log_warn(&e); } }
}
pub(crate) fn clear_client(slot: i32, client: u64) {
    remove_where(|l| l.key.slot == slot && l.key.client == client);
    REGISTRY.with(|r| r.borrow_mut().pending.retain(|l| l.key.slot != slot || l.key.client != client));
}
pub(crate) fn prune_dead() {
    remove_where(|l| !identity_live(l));
    REGISTRY.with(|r| r.borrow_mut().pending.retain(identity_live));
}
pub(crate) fn reset() { REGISTRY.with(|r| *r.borrow_mut() = Registry::default()); }
pub(crate) fn advance_frame() {
    let Ok(_transition) = Transition::enter() else { return };
    let pending = REGISTRY.with(|r| { let mut r = r.borrow_mut(); r.frame = r.frame.saturating_add(1); std::mem::take(&mut r.pending) });
    for l in pending {
        let key = l.key.clone();
        if retire_or_queue(l).is_ok() {
            REGISTRY.with(|r| { let mut r = r.borrow_mut();
                if let Some(t) = r.winner(&key) { r.suspend(t); }
            });
        }
    }
}
pub(crate) fn register_store() {
    crate::owner_stores::register("SURFACE_LEASES", Box::new(|owner| remove_where(|l| l.owner == owner)),
        Box::new(|_| {}), Box::new(reset));
}
fn token(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments) -> u64 {
    if !args.get(0).is_string() { return 0; }
    args.get(0).to_rust_string_lossy(scope).strip_prefix("surface:").and_then(|s| s.parse().ok()).unwrap_or(0)
}
fn native_reserve(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = current_plugin(scope).ok_or_else(|| fail("Released", "missing plugin context"))?;
        if matches!(crate::v8host::plugin_phase(&owner), Some(crate::plugin::Phase::Unloading | crate::plugin::Phase::Failed)) {
            return Err(fail("Released", "surface owner is unloading"));
        }
        if !args.get(0).is_string() || !args.get(1).is_int32() || !args.get(2).is_number()
            || !args.get(3).is_int32() || !args.get(4).is_int32() || !args.get(5).is_string() {
            return Err(fail("InvalidArgument", "invalid surface reservation arguments"));
        }
        let surface = args.get(0).to_rust_string_lossy(scope);
        let entity = args.get(2).number_value(scope).unwrap_or(0.0);
        let data = args.get(5).to_rust_string_lossy(scope);
        if !bounded(&surface) || data.len() > 4096 || entity < 1.0 || entity > 9007199254740991.0 || entity.fract() != 0.0 {
            return Err(fail("InvalidArgument", "invalid surface name/entity/adapter data"));
        }
        let adapter: Adapter = serde_json::from_str(&data).map_err(|_| fail("InvalidArgument", "invalid surface adapter data"))?;
        let slot = args.get(3).int32_value(scope).unwrap_or(-1);
        let key = Key { surface, index: args.get(1).int32_value(scope).unwrap_or(-1), entity: entity as u64,
            slot, client: crate::client::generation(slot) };
        reserve(&owner, key, args.get(4).int32_value(scope).unwrap_or(0), adapter)
    })).unwrap_or_else(|_| Err(fail("Unavailable", "surface reservation internal failure")));
    let value = match result {
        Ok(t) => serde_json::json!({"ok":true,"value":format!("surface:{t}")}),
        Err(e) => serde_json::json!({"ok":false,"error":{"code":e.code,"message":e.message}}),
    };
    if let Some(s) = v8::String::new(scope, &value.to_string()) {
        if let Some(v) = v8::json::parse(scope, s) { rv.set(v); }
    }
}
fn native_state(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let t = token(scope, &args);
    let owner = current_plugin(scope).unwrap_or_default();
    if let Some(s) = v8::String::new(scope, state(&owner, t)) { rv.set(s.into()); }
}
macro_rules! bool_native {
    ($name:ident, $op:ident) => {
        fn $name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
            rv.set_bool(false);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let t = token(scope, &args);
                if let Some(owner) = current_plugin(scope) { rv.set_bool($op(&owner, t)); }
            }));
        }
    };
}
bool_native!(native_activate, activate);
bool_native!(native_active, active);
bool_native!(native_release, release);
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    set_native(scope, global, "__s2_surface_reserve", native_reserve);
    set_native(scope, global, "__s2_surface_activate", native_activate);
    set_native(scope, global, "__s2_surface_active", native_active);
    set_native(scope, global, "__s2_surface_state", native_state);
    set_native(scope, global, "__s2_surface_release", native_release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::{self, frame_tests::{dummy_logger, eval_in_context_string}};
    fn setup() -> u64 {
        v8host::init(dummy_logger()).unwrap();
        for owner in ["surface_a", "surface_b"] { v8host::create_plugin_context(owner); }
        crate::client::begin(2);
        crate::entity_live::on_created(10, 123)
    }
    fn claim(owner: &str, id: u64, priority: i32, adapter: Adapter) -> u64 {
        reserve(owner, Key { surface: "focus".into(), index: 10, entity: id,
            slot: 2, client: crate::client::generation(2) }, priority, adapter).unwrap()
    }
    fn js(owner: &str, source: &str) -> String { eval_in_context_string(owner, source) }
    fn query(owner: &str, op: &str, t: u64) -> String {
        js(owner, &format!("String(__s2_surface_{op}('surface:{t}'))"))
    }

    #[test]
    fn competing_owners_priority_latest_tie_rollback_and_next_frame_restore() {
        let id = setup();
        let a = claim("surface_a", id, 10, Adapter::default());
        assert!(activate("surface_a", a));
        let low = claim("surface_b", id, 9, Adapter::default());
        assert_eq!(state("surface_b", low), "covered");
        assert!(!activate("surface_b", low));
        assert!(active("surface_a", a));
        let b = claim("surface_b", id, 10, Adapter::default());
        assert_eq!(state("surface_a", a), "covered");
        assert!(!active("surface_a", a));
        assert_eq!(state("surface_b", b), "ready");
        assert!(!active("surface_b", b));
        // Failed paint releases the reservation; restoration requires a later-frame paint.
        assert!(release("surface_b", b));
        assert_eq!(state("surface_a", a), "waiting");
        assert!(!activate("surface_a", a));
        advance_frame();
        assert_eq!(state("surface_a", a), "ready");
        assert!(!active("surface_a", a));
        assert!(activate("surface_a", a));
        assert!(release("surface_b", low));
        assert!(active("surface_a", a), "releasing a covered token leaves winner intact");
        v8host::shutdown();
    }

    #[test]
    fn pure_registry_covered_removal_preserves_active_winner_and_keys_are_independent() {
        fn lease(token: u64, priority: i32) -> Lease {
            Lease { token, priority, owner: "a".into(), generation: 1,
                key: Key { surface: "generic".into(), index: 1, entity: 1, slot: 2, client: 1 },
                game: None, adapter: Adapter::default(), activated: true, activated_in: None, ready_after: 0 }
        }
        let mut r = Registry::default();
        r.leases.insert(1, lease(1, 20)); r.leases.insert(2, lease(2, 0));
        let mut other = lease(3, 100); other.key.slot = 3; r.leases.insert(3, other);
        assert_eq!(r.state(1, "a"), "active");
        assert_eq!(r.state(2, "a"), "covered");
        assert_eq!(r.state(3, "a"), "active");
        r.remove(2); assert_eq!(r.state(1, "a"), "active");
        assert_eq!(r.state(1, "b"), "invalid");
    }

    #[test]
    fn tokens_are_owner_checked_and_unload_restores_without_js_cleanup() {
        let id = setup();
        let a = claim("surface_a", id, 0, Adapter::default()); activate("surface_a", a);
        let b = claim("surface_b", id, 0, Adapter::default()); activate("surface_b", b);
        assert_eq!(query("surface_a", "state", b), "invalid");
        for op in ["activate", "active", "release"] { assert_eq!(query("surface_a", op, b), "false"); }
        v8host::unload_plugin("surface_b");
        assert_eq!(state("surface_a", a), "waiting");
        v8host::frame_async_drain();
        assert_eq!(state("surface_a", a), "ready");
        assert!(!active("surface_a", a));
        assert!(activate("surface_a", a));
        v8host::create_plugin_context("surface_b");
        assert_eq!(query("surface_b", "release", b), "false");
        let new_b = claim("surface_b", id, 0, Adapter::default());
        assert!(new_b > b);
        v8host::shutdown();
    }

    #[test]
    fn disconnect_entity_replacement_map_and_reinit_invalidate_tokens() {
        let id = setup();
        let a = claim("surface_a", id, 0, Adapter::default()); activate("surface_a", a);
        let departed = crate::client::generation(2);
        crate::client::end(2, departed);
        assert!(REGISTRY.with(|r| r.borrow().leases.is_empty()));
        crate::client::begin(2);
        let b = claim("surface_b", id, 0, Adapter::default()); activate("surface_b", b);
        assert!(!release("surface_a", a));
        crate::client::end(2, departed);
        assert!(active("surface_b", b));
        let next_id = crate::entity_live::on_created(10, 456);
        assert!(!active("surface_b", b));
        let c = claim("surface_a", next_id, 0, Adapter::default());
        crate::entity_live::on_deleted(10, 123); // Stale engine serial must not retire replacement.
        assert!(activate("surface_a", c));
        crate::entity_live::clear_for_map_transition();
        assert_eq!(state("surface_a", c), "invalid");
        assert!(REGISTRY.with(|r| r.borrow().leases.is_empty() && r.borrow().pending.is_empty()));
        v8host::shutdown();
        let id = setup();
        assert!(claim("surface_a", id, 0, Adapter::default()) > c);
        v8host::shutdown();
    }

    #[test]
    fn activating_in_outer_dispatch_cannot_consume_same_or_nested_delivery() {
        let id = setup();
        let a = claim("surface_a", id, 0, Adapter::default()); activate("surface_a", a);
        js("surface_a", &format!(r#"
            __s2_event_subscribe('handoff', function() {{ __s2_surface_release('surface:{a}'); }});
        "#));
        js("surface_b", &format!(r#"
            var actions = 0;
            __s2_event_subscribe('handoff', function() {{
                var r = __s2_surface_reserve('focus',10,{id},2,0,'{{}}');
                token = r.value;
                __s2_surface_activate(token);
            }});
            __s2_event_subscribe('handoff', function() {{ if (__s2_surface_active(token)) actions++; }});
        "#));
        let _ = crate::events::dispatch_game_event("handoff");
        assert_eq!(js("surface_b", "String(actions)"), "0");
        assert_eq!(js("surface_b", "String(__s2_surface_active(token))"), "true");
        let b = claim("surface_b", id, 1, Adapter::default());
        {
            let _outer = crate::dispatch::DispatchScope::enter();
            assert!(activate("surface_b", b));
            let _nested = crate::dispatch::DispatchScope::enter();
            assert!(!active("surface_b", b));
        }
        assert!(active("surface_b", b));
        v8host::shutdown();
    }

    #[test]
    fn malformed_native_data_fails_with_structured_codes_and_zero_claims() {
        let id = setup();
        for args in [format!("'f',10,{id},2,1.5,'{{}}'"), format!("'f',10,{id},2,NaN,'{{}}'"),
            format!("'f',10,{id},2,0,'{{\"unexpected\":true}}'"), "'f',10,0,2,0,'{}'".into()] {
            assert_eq!(js("surface_a", &format!("__s2_surface_reserve({args}).error.code")), "InvalidArgument");
        }
        assert_eq!(js("surface_a", &format!("__s2_surface_reserve('f',10,{id},3,0,'{{}}').error.code")), "StaleClient");
        assert!(REGISTRY.with(|r| r.borrow().leases.is_empty()));
        v8host::shutdown();
    }

    thread_local! {
        static EFFECTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static FAIL_HIDE: Cell<bool> = const { Cell::new(false) };
        static FAIL_OFF: Cell<bool> = const { Cell::new(false) };
        static REENTER: Cell<bool> = const { Cell::new(false) };
        static END_CLIENT: Cell<bool> = const { Cell::new(false) };
    }
    extern "C" fn resolve(_: *const std::ffi::c_char, _: *const std::ffi::c_char, _: *const std::ffi::c_char,
        _: *const std::ffi::c_char, _: *const std::ffi::c_char, _: i32, _: *const std::ffi::c_char,
        _: *mut std::ffi::c_char, _: i32) -> i32 { 7 }
    extern "C" fn invoke(_: i32, _: i32, _: i32, _: i32, gp: *const u64, kinds: *const u8,
        n: i32, _: *const f64, _: i32, strings: *const *const std::ffi::c_char,
        _: *const f32, _: i32, _: *mut u64) -> i32 {
        let slot = unsafe { *gp as i32 };
        assert_eq!(slot, 2);
        if n == 2 {
            let on = unsafe { *gp.add(1) != 0 };
            EFFECTS.with(|e| e.borrow_mut().push(format!("capture:{on}")));
            if !on && FAIL_OFF.with(Cell::get) { return 0; }
        } else {
            assert_eq!(n, 4);
            assert_eq!(unsafe { *kinds.add(1) }, crate::gamedata_calls::GP_UTLSTRING);
            let root = unsafe { std::ffi::CStr::from_ptr(*strings.add(*gp.add(1) as usize)) }.to_str().unwrap();
            EFFECTS.with(|e| e.borrow_mut().push(format!("hide:{root}")));
            if REENTER.with(|v| v.replace(false)) {
                let id = crate::entity_live::lookup(10).unwrap().0;
                let r = reserve("surface_b", Key { surface: "focus".into(), index: 10,
                    entity: id, slot: 2, client: crate::client::generation(2) }, 20, Adapter::default());
                assert_eq!(r.unwrap_err().code, "Busy");
                let a = REGISTRY.with(|r| r.borrow().leases.values().find(|l| l.owner == "surface_a").unwrap().token);
                assert!(!release("surface_a", a));
                assert!(!active("surface_a", a));
            }
            if END_CLIENT.with(|v| v.replace(false)) { crate::client::end(2, crate::client::generation(2)); }
            if FAIL_HIDE.with(Cell::get) { return 0; }
        }
        1
    }
    fn setup_engine() -> u64 {
        let id = setup();
        EFFECTS.with(|e| e.borrow_mut().clear());
        FAIL_HIDE.with(|v| v.set(false)); FAIL_OFF.with(|v| v.set(false));
        REENTER.with(|v| v.set(false)); END_CLIENT.with(|v| v.set(false));
        v8host::set_engine_ops(Some(crate::v8host::S2EngineOps {
            engine_call_resolve: Some(resolve), engine_call_invoke: Some(invoke),
            ..crate::v8host::frame_tests::mock_event_ops()
        }));
        crate::gamedata_calls::register_game_package("@test/game", r#"{
            "signatures":{"Action":{"linuxsteamrt64":{"module":"server","pattern":"55 48","resolve":"direct"}}},
            "calls":{
                "toggle":{"receiver":{"kind":"entity"},"target":{"kind":"signature","name":"Action"},"args":["int","bool"],"returns":"void"},
                "suspend":{"receiver":{"kind":"entity"},"target":{"kind":"signature","name":"Action"},"args":["int","utlstring","utlstring","int"],"returns":"void"}
            }}"#);
        id
    }
    fn presentation() -> Adapter {
        Adapter { capture: Some(Capture { call: "toggle".into(), token: "panel:root".into() }),
            suspend: Some(Suspend { call: "suspend".into(), args: vec!["root".into(), "hidden".into(), 1.into()] }) }
    }
    fn capture(owner: &str, id: u64) {
        assert_eq!(js(owner, &format!("JSON.stringify(__s2_shared_entity_switch('toggle',10,{id},2,'panel:root',true))")), "null");
    }
    fn effects() -> Vec<String> { EFFECTS.with(|e| e.borrow().clone()) }
    fn done_engine() { v8host::set_engine_ops(None); v8host::shutdown(); }

    #[test]
    fn cursor_false_transfer_hides_and_releases_recorded_capture_before_ready() {
        let id = setup_engine();
        let a = claim("surface_a", id, 10, presentation()); capture("surface_a", id); activate("surface_a", a);
        let low = claim("surface_b", id, 9, Adapter::default());
        assert_eq!(effects(), ["capture:true"]);
        assert_eq!(state("surface_b", low), "covered");
        REENTER.with(|v| v.set(true));
        let b = claim("surface_b", id, 10, Adapter::default()); // cursor:false replacement.
        assert_eq!(effects(), ["capture:true", "hide:root", "capture:false"]);
        assert_eq!(state("surface_b", b), "ready");
        assert!(!active("surface_a", a));
        activate("surface_b", b);
        release("surface_b", b);
        assert_eq!(state("surface_a", a), "waiting");
        advance_frame();
        assert_eq!(effects(), ["capture:true", "hide:root", "capture:false"], "host never repaints or enables restored capture");
        capture("surface_a", id); assert!(activate("surface_a", a));
        assert_eq!(effects().last().unwrap(), "capture:true");
        done_engine();
    }

    #[test]
    fn transition_failure_rolls_back_without_stranded_capture_or_interaction() {
        for fail_hide in [true, false] {
            let id = setup_engine();
            let a = claim("surface_a", id, 0, presentation()); capture("surface_a", id); activate("surface_a", a);
            if fail_hide { FAIL_HIDE.with(|v| v.set(true)); } else { FAIL_OFF.with(|v| v.set(true)); }
            let result = reserve("surface_b", Key { surface: "focus".into(), index: 10,
                entity: id, slot: 2, client: crate::client::generation(2) }, 0, Adapter::default());
            assert_eq!(result.unwrap_err().code, "Unavailable");
            assert_eq!(effects(), ["capture:true", "hide:root", "capture:false"]);
            assert_eq!(state("surface_a", a), "waiting");
            assert!(!active("surface_a", a));
            assert_eq!(REGISTRY.with(|r| r.borrow().leases.len()), 1);
            advance_frame(); assert_eq!(state("surface_a", a), "waiting");
            FAIL_HIDE.with(|v| v.set(false)); FAIL_OFF.with(|v| v.set(false));
            advance_frame(); assert_eq!(state("surface_a", a), "waiting");
            advance_frame(); assert_eq!(state("surface_a", a), "ready");
            assert!(!active("surface_a", a));
            capture("surface_a", id); assert!(activate("surface_a", a));
            assert!(REGISTRY.with(|r| r.borrow().pending.is_empty()));
            done_engine();
        }
    }

    #[test]
    fn unload_hides_via_host_binding_and_cleans_capture_without_js() {
        let id = setup_engine();
        let a = claim("surface_a", id, 0, presentation()); capture("surface_a", id); activate("surface_a", a);
        v8host::unload_plugin("surface_a");
        assert_eq!(effects(), ["capture:true", "hide:root", "capture:false"]);
        assert!(REGISTRY.with(|r| r.borrow().leases.is_empty()));
        done_engine();
    }

    #[test]
    fn lifetime_change_during_suspend_never_commits_replacement_reservation() {
        let id = setup_engine();
        let a = claim("surface_a", id, 0, presentation()); capture("surface_a", id); activate("surface_a", a);
        END_CLIENT.with(|v| v.set(true));
        let result = reserve("surface_b", Key { surface: "focus".into(), index: 10,
            entity: id, slot: 2, client: crate::client::generation(2) }, 0, Adapter::default());
        assert_eq!(result.unwrap_err().code, "Released");
        assert!(REGISTRY.with(|r| r.borrow().leases.is_empty() && r.borrow().pending.is_empty()));
        assert!(!active("surface_a", a));
        done_engine();
    }

    #[test]
    fn pending_retirement_is_discarded_immediately_when_connection_ends() {
        let id = setup_engine();
        let a = claim("surface_a", id, 0, presentation()); capture("surface_a", id); activate("surface_a", a);
        FAIL_HIDE.with(|v| v.set(true));
        assert!(release("surface_a", a));
        assert_eq!(REGISTRY.with(|r| r.borrow().pending.len()), 1);
        crate::client::end(2, crate::client::generation(2));
        assert!(REGISTRY.with(|r| r.borrow().pending.is_empty()));
        done_engine();
    }

    #[test]
    fn suspension_resolves_only_host_game_descriptors_and_revalidates_argument_shapes() {
        let id = setup_engine();
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["surface_a"]}"#).unwrap();
        crate::gamedata_calls::register_plugin("surface_a", r#"{
            "signatures":{"Action":{"linuxsteamrt64":{"module":"server","pattern":"55 48","resolve":"direct"}}},
            "calls":{"privateAction":{"receiver":{"kind":"entity"},"target":{"kind":"signature","name":"Action"},
                "args":["int","utlstring","utlstring","int"],"returns":"void"}}}"#);
        assert!(crate::gamedata_calls::plan("surface_a", "privateAction").is_some());
        let mut adapter = presentation(); adapter.suspend.as_mut().unwrap().call = "privateAction".into();
        let key = Key { surface: "focus".into(), index: 10, entity: id, slot: 2, client: crate::client::generation(2) };
        assert_eq!(reserve("surface_a", key.clone(), 0, adapter).unwrap_err().code, "Unavailable");
        let mut bad = presentation(); bad.suspend.as_mut().unwrap().args[2] = "wrong scalar".into();
        assert_eq!(reserve("surface_a", key, 0, bad).unwrap_err().code, "InvalidArgument");
        assert!(effects().is_empty());
        assert!(REGISTRY.with(|r| r.borrow().leases.is_empty()));
        done_engine();
    }

    #[test]
    fn focus_capture_retirement_preserves_unrelated_manual_capture() {
        let id = setup_engine();
        let a = claim("surface_a", id, 0, presentation()); capture("surface_a", id); activate("surface_a", a);
        assert_eq!(js("surface_a", &format!("JSON.stringify(__s2_shared_entity_switch('toggle',10,{id},2,'manual',true))")), "null");
        let b = claim("surface_b", id, 0, Adapter::default()); activate("surface_b", b);
        assert_eq!(effects(), ["capture:true", "hide:root"], "manual capture is independent of focus");
        assert_eq!(js("surface_a", &format!("JSON.stringify(__s2_shared_entity_switch('toggle',10,{id},2,'manual',false))")), "null");
        assert_eq!(effects(), ["capture:true", "hide:root", "capture:false"], "recorded panel capture was retired");
        done_engine();
    }

    #[test]
    fn failed_release_blocks_new_paint_until_host_retirement_succeeds() {
        let id = setup_engine();
        let a = claim("surface_a", id, 0, presentation()); capture("surface_a", id); activate("surface_a", a);
        FAIL_HIDE.with(|v| v.set(true));
        assert!(release("surface_a", a));
        assert!(!release("surface_a", a));
        let key = Key { surface: "focus".into(), index: 10, entity: id, slot: 2, client: crate::client::generation(2) };
        assert_eq!(reserve("surface_b", key.clone(), 0, Adapter::default()).unwrap_err().code, "Busy");
        FAIL_HIDE.with(|v| v.set(false)); advance_frame();
        let b = reserve("surface_b", key, 0, Adapter::default()).unwrap();
        assert_eq!(state("surface_b", b), "ready");
        assert!(activate("surface_b", b));
        done_engine();
    }

    #[test]
    fn covered_shared_root_release_and_unload_never_retire_winners_presentation() {
        for previously_presented in [false, true] {
            for unload in [false, true] {
                let id = setup_engine();
                // Both adapters name the same physical root and capture token. A covered
                // reservation either never painted or was already retired during takeover.
                let a = claim("surface_a", id, 10, presentation());
                capture("surface_a", id); assert!(activate("surface_a", a));
                let b = claim("surface_b", id, if previously_presented { 20 } else { 0 }, presentation());
                let (covered_owner, covered, winner_owner, winner) = if previously_presented {
                    capture("surface_b", id); assert!(activate("surface_b", b));
                    ("surface_a", a, "surface_b", b)
                } else { ("surface_b", b, "surface_a", a) };
                assert_eq!(state(covered_owner, covered), "covered");
                let before = effects();
                if unload { v8host::unload_plugin(covered_owner); }
                else { assert!(release(covered_owner, covered)); }
                assert_eq!(effects(), before, "covered cleanup must not hide the shared root");
                assert!(active(winner_owner, winner));
                assert!(REGISTRY.with(|r| r.borrow().pending.is_empty()));
                done_engine();
            }
        }
    }

    #[test]
    fn malformed_suspension_values_are_invalid_arguments_without_engine_effects() {
        let id = setup_engine();
        let key = Key { surface: "focus".into(), index: 10, entity: id, slot: 2, client: crate::client::generation(2) };
        for (index, value) in [(2, serde_json::json!("wrong scalar")), (2, serde_json::json!(2147483648i64)),
            (2, serde_json::json!(1.5)), (0, serde_json::json!("")), (0, serde_json::json!("nul\0string")),
            (0, serde_json::json!("x".repeat(257))), (0, serde_json::json!(true)),
            (0, serde_json::json!(null)), (2, serde_json::json!({"value": 1}))] {
            let mut adapter = presentation(); adapter.suspend.as_mut().unwrap().args[index] = value;
            assert_eq!(reserve("surface_a", key.clone(), 0, adapter).unwrap_err().code, "InvalidArgument");
        }
        let mut wrong_count = presentation(); wrong_count.suspend.as_mut().unwrap().args.pop();
        assert_eq!(reserve("surface_a", key.clone(), 0, wrong_count).unwrap_err().code, "InvalidArgument");
        let wrong_bool = Adapter { capture: None, suspend: Some(Suspend { call: "toggle".into(), args: vec![1.into()] }) };
        assert_eq!(reserve("surface_a", key, 0, wrong_bool).unwrap_err().code, "InvalidArgument");
        assert_eq!(js("surface_a", &format!(r#"
            __s2_surface_reserve('focus',10,{id},2,0,
                JSON.stringify({{suspend:{{call:'suspend',args:['root','hidden','wrong']}}}})).error.code
        "#)), "InvalidArgument");
        assert!(effects().is_empty());
        assert!(REGISTRY.with(|r| r.borrow().leases.is_empty()));
        done_engine();
    }
    #[test]
    fn native_reservation_is_noninteractive_until_explicit_activation() {
        v8host::init(dummy_logger()).unwrap();
        v8host::create_plugin_context("surface_a");
        crate::client::begin(2);
        let id = crate::entity_live::on_created(10, 123);
        let got = eval_in_context_string("surface_a", &format!(r#"
            var r = __s2_surface_reserve('focus',10,{id},2,0,'{{}}');
            var token = r.value;
            JSON.stringify([r.ok, typeof token, __s2_surface_state(token),
              __s2_surface_active(token), __s2_surface_activate(token), __s2_surface_active(token)])
        "#));
        assert_eq!(got, r#"[true,"string","ready",false,true,true]"#);
        v8host::shutdown();
    }
}
