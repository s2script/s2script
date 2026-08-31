//! Per-entity SDKHooks — SourceMod `SDKHook` / `SDKUnhook`.
//!
//! The table is books-gated host identity (`entity_live` id), never a raw pointer. `OnTakeDamage`
//! fans out from the process-wide `DispatchTraceAttack` detour. The Touch family is per-entity
//! SourceHook (`sdkhook_vp_add` / `SH_ADD_MANUALHOOK`), not `SH_ADD_MANUALVPHOOK`.

use crate::dispatch::{fan_out_collapsing, Instrument, StopAt};
use crate::multiplexer::HookResult;
use crate::v8host::{
    build_entity_ref, clone_plugin_context, current_plugin, engine_ops, log_warn, next_sub_id,
    owner_is_live, plugin_generation, set_native, with_host_isolate,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::CString;
use std::os::raw::c_int;

const KIND_ON_TAKE_DAMAGE: &str = "OnTakeDamage";
const KIND_ON_TAKE_DAMAGE_POST: &str = "OnTakeDamagePost";
pub(crate) const KIND_SET_TRANSMIT: &str = "SetTransmit";

struct Entry {
    owner: String,
    generation: u64,
    entity_id: u64,
    entity_index: i32,
    engine_serial: i32,
    kind: String,
    handler: v8::Global<v8::Function>,
    sub_id: u64,
}

thread_local! {
    static HOOKS: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn packed_handle(index: i32, serial: i32) -> i32 {
    let bits = crate::entity::HANDLE_ENTRY_BITS;
    ((serial as u32) << bits | ((index as u32) & ((1u32 << bits) - 1))) as i32
}

/// Wiki VP name → (gamedata / VP type without Post, post flag).
fn vp_kind(kind: &str) -> Option<(&'static str, c_int)> {
    match kind {
        "StartTouch" => Some(("StartTouch", 0)),
        "StartTouchPost" => Some(("StartTouch", 1)),
        "Touch" => Some(("Touch", 0)),
        "TouchPost" => Some(("Touch", 1)),
        "EndTouch" => Some(("EndTouch", 0)),
        "EndTouchPost" => Some(("EndTouch", 1)),
        "Blocked" => Some(("Blocked", 0)),
        "BlockedPost" => Some(("Blocked", 1)),
        "Spawn" => Some(("Spawn", 0)),
        "SpawnPost" => Some(("Spawn", 1)),
        "Think" => Some(("Think", 0)),
        "ThinkPost" => Some(("Think", 1)),
        "PreThink" => Some(("PreThink", 0)),
        "PreThinkPost" => Some(("PreThink", 1)),
        "PostThink" => Some(("PostThink", 0)),
        "PostThinkPost" => Some(("PostThink", 1)),
        "Use" => Some(("Use", 0)),
        "UsePost" => Some(("Use", 1)),
        "GetMaxHealth" => Some(("GetMaxHealth", 0)),
        "ShouldCollide" => Some(("ShouldCollide", 0)),
        "VPhysicsUpdate" => Some(("VPhysicsUpdate", 0)),
        "VPhysicsUpdatePost" => Some(("VPhysicsUpdate", 1)),
        "GroundEntChangedPost" => Some(("GroundEntChangedPost", 1)),
        "CanBeAutobalanced" => Some(("CanBeAutobalanced", 0)),
        "Reload" => Some(("Reload", 0)),
        "WeaponCanUse" => Some(("WeaponCanUse", 0)),
        "WeaponCanUsePost" => Some(("WeaponCanUse", 1)),
        "WeaponCanSwitchTo" => Some(("WeaponCanSwitchTo", 0)),
        "WeaponCanSwitchToPost" => Some(("WeaponCanSwitchTo", 1)),
        "WeaponDrop" => Some(("WeaponDrop", 0)),
        "WeaponDropPost" => Some(("WeaponDrop", 1)),
        "WeaponEquip" => Some(("WeaponEquip", 0)),
        "WeaponEquipPost" => Some(("WeaponEquip", 1)),
        "WeaponSwitch" => Some(("WeaponSwitch", 0)),
        "WeaponSwitchPost" => Some(("WeaponSwitch", 1)),
        _ => None,
    }
}

fn is_known_kind(kind: &str) -> bool {
    kind == KIND_ON_TAKE_DAMAGE
        || kind == KIND_ON_TAKE_DAMAGE_POST
        || kind == KIND_SET_TRANSMIT
        || vp_kind(kind).is_some()
}

fn vp_add(index: i32, serial: i32, ty: &str, post: c_int) -> bool {
    let Some(ops) = engine_ops() else {
        return false;
    };
    let Some(f) = ops.sdkhook_vp_add else {
        return false;
    };
    let Ok(c) = CString::new(ty) else {
        return false;
    };
    f(index, serial, c.as_ptr(), post) != 0
}

fn vp_remove(index: i32, serial: i32, ty: &str, post: c_int) {
    let Some(ops) = engine_ops() else {
        return;
    };
    let Some(f) = ops.sdkhook_vp_remove else {
        return;
    };
    let Ok(c) = CString::new(ty) else {
        return;
    };
    let _ = f(index, serial, c.as_ptr(), post);
}

fn vp_drop(index: i32, serial: i32) {
    let Some(ops) = engine_ops() else {
        return;
    };
    if let Some(f) = ops.sdkhook_vp_drop {
        let _ = f(index, serial);
    }
}

fn still_has(entity_id: u64, kind: &str) -> bool {
    HOOKS.with(|h| h.borrow().iter().any(|e| e.entity_id == entity_id && e.kind == kind))
}

fn unhook_vp_if_last(index: i32, serial: i32, entity_id: u64, kind: &str) {
    let Some((ty, post)) = vp_kind(kind) else {
        return;
    };
    if still_has(entity_id, kind) {
        return;
    }
    vp_remove(index, serial, ty, post);
}

/// Host id of the current damage victim, or `None` when there is no op / no live books match.
fn current_victim_id() -> Option<u64> {
    let ops = engine_ops()?;
    let f = ops.damage_victim?;
    let raw = f();
    if raw < 0 {
        return None;
    }
    let (index, serial) = crate::entity::decode_handle(raw as u32);
    crate::entity_live::adopt(index, serial)
}

/// Snapshot `OnTakeDamage` callbacks whose hooked identity is the current victim, subscribe order.
pub(crate) fn snapshot_ontakedamage() -> Vec<(String, u64, v8::Global<v8::Function>)> {
    snapshot_damage_kind(KIND_ON_TAKE_DAMAGE)
}

/// Snapshot `OnTakeDamagePost` callbacks for the current victim.
pub(crate) fn snapshot_ontakedamage_post() -> Vec<(String, u64, v8::Global<v8::Function>)> {
    snapshot_damage_kind(KIND_ON_TAKE_DAMAGE_POST)
}

fn snapshot_damage_kind(kind: &str) -> Vec<(String, u64, v8::Global<v8::Function>)> {
    let Some(vid) = current_victim_id() else {
        return Vec::new();
    };
    snapshot_kind(vid, kind)
}

pub(crate) fn snapshot_kind(entity_id: u64, kind: &str) -> Vec<(String, u64, v8::Global<v8::Function>)> {
    HOOKS.with(|h| {
        h.borrow()
            .iter()
            .filter(|e| e.entity_id == entity_id && e.kind == kind)
            .map(|e| (e.owner.clone(), e.generation, e.handler.clone()))
            .collect()
    })
}

pub(crate) fn kind_active(kind: &str) -> bool {
    HOOKS.with(|h| h.borrow().iter().any(|e| e.kind == kind))
}

/// Unique `(index, engine_serial)` pairs for `kind`, subscribe order.
pub(crate) fn snapshot_kind_entities(kind: &str) -> Vec<(i32, i32)> {
    HOOKS.with(|h| {
        let mut seen = HashSet::new();
        h.borrow()
            .iter()
            .filter(|e| e.kind == kind)
            .filter_map(|e| {
                seen.insert((e.entity_index, e.engine_serial))
                    .then_some((e.entity_index, e.engine_serial))
            })
            .collect()
    })
}

/// Drop every hook on this host id (entity destroy). SH_REMOVE leftover VP hooks via `vp_drop`.
pub(crate) fn drop_entity(id: u64) {
    let coords = HOOKS.with(|h| {
        h.borrow().iter().find(|e| e.entity_id == id).map(|e| (e.entity_index, e.engine_serial))
    });
    if let Some((index, serial)) = coords {
        vp_drop(index, serial);
    }
    HOOKS.with(|h| h.borrow_mut().retain(|e| e.entity_id != id));
}

/// Map transition: unhook every VP then clear. Engine is still up (unlike process shutdown).
pub(crate) fn drop_all() {
    let pairs: Vec<(i32, i32)> = HOOKS.with(|h| {
        let mut seen = HashSet::new();
        h.borrow()
            .iter()
            .filter_map(|e| {
                if vp_kind(&e.kind).is_some() {
                    seen.insert((e.entity_index, e.engine_serial)).then_some((e.entity_index, e.engine_serial))
                } else {
                    None
                }
            })
            .collect()
    });
    for (index, serial) in pairs {
        vp_drop(index, serial);
    }
    HOOKS.with(|h| h.borrow_mut().clear());
}

pub(crate) fn register_stores() {
    crate::owner_stores::register(
        "SDKHOOKS",
        Box::new(|owner| {
            let going: Vec<(i32, i32, u64, String)> = HOOKS.with(|h| {
                h.borrow()
                    .iter()
                    .filter(|e| e.owner == owner)
                    .map(|e| (e.entity_index, e.engine_serial, e.entity_id, e.kind.clone()))
                    .collect()
            });
            HOOKS.with(|h| h.borrow_mut().retain(|e| e.owner != owner));
            for (index, serial, id, kind) in going {
                unhook_vp_if_last(index, serial, id, &kind);
            }
        }),
        Box::new(|ids| {
            let going: Vec<(i32, i32, u64, String)> = HOOKS.with(|h| {
                h.borrow()
                    .iter()
                    .filter(|e| ids.contains(&e.sub_id))
                    .map(|e| (e.entity_index, e.engine_serial, e.entity_id, e.kind.clone()))
                    .collect()
            });
            HOOKS.with(|h| h.borrow_mut().retain(|e| !ids.contains(&e.sub_id)));
            for (index, serial, id, kind) in going {
                unhook_vp_if_last(index, serial, id, &kind);
            }
        }),
        Box::new(|| {
            // Process teardown: contents only — no engine-op follow-up (owner_stores contract).
            HOOKS.with(|h| h.borrow_mut().clear());
        }),
    );
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_sdkhook", s2_sdkhook);
    set_native(scope, global_obj, "__s2_sdkunhook", s2_sdkunhook);
}

/// `__s2_sdkhook(index, id, type, callback) -> bool`
fn s2_sdkhook(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 4 {
            return;
        }
        let index = args.get(0).int32_value(scope).unwrap_or(-1);
        let id = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let Some(serial) = crate::entity_live::engine_serial_for(index, id) else {
            return;
        };
        let kind = args.get(2).to_rust_string_lossy(scope);
        if !is_known_kind(&kind) {
            return;
        }
        let Ok(func) = v8::Local::<v8::Function>::try_from(args.get(3)) else {
            return;
        };
        if let Some((ty, post)) = vp_kind(&kind) {
            let first = !still_has(id, &kind);
            if first && !vp_add(index, serial, ty, post) {
                return;
            }
        }
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = plugin_generation(&owner);
        let sub_id = next_sub_id();
        let handler = v8::Global::new(scope.as_ref(), func);
        HOOKS.with(|h| {
            h.borrow_mut().push(Entry {
                owner,
                generation,
                entity_id: id,
                entity_index: index,
                engine_serial: serial,
                kind,
                handler,
                sub_id,
            });
        });
        rv.set_bool(true);
    }));
}

/// `__s2_sdkunhook(index, id, type, callback) -> bool`
fn s2_sdkunhook(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 4 {
            return;
        }
        let id = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let kind = args.get(2).to_rust_string_lossy(scope);
        let Ok(func) = v8::Local::<v8::Function>::try_from(args.get(3)) else {
            return;
        };
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let removed = HOOKS.with(|h| {
            let mut v = h.borrow_mut();
            if let Some(i) = v.iter().position(|e| {
                e.owner == owner
                    && e.entity_id == id
                    && e.kind == kind
                    && {
                        let local = v8::Local::new(scope, &e.handler);
                        local.strict_equals(func.into())
                    }
            }) {
                let coords = (v[i].entity_index, v[i].engine_serial, v[i].entity_id, v[i].kind.clone());
                v.remove(i);
                Some(coords)
            } else {
                None
            }
        });
        if let Some((index, serial, eid, k)) = removed {
            unhook_vp_if_last(index, serial, eid, &k);
            rv.set_bool(true);
        }
    }));
}

/// Fan-out a Touch-family VP virtual. `type_name` is the wiki name (`Touch` / `TouchPost`).
/// Returns collapsed `HookResult` as `0..=3`.
pub(crate) fn dispatch_touch(
    this_index: i32,
    this_serial: i32,
    other_handle: i32,
    post: c_int,
    type_name: &str,
) -> c_int {
    let Some(this_id) = crate::entity_live::adopt(this_index, this_serial) else {
        return HookResult::Continue as c_int;
    };
    let snap = snapshot_kind(this_id, type_name);
    if snap.is_empty() {
        return HookResult::Continue as c_int;
    }
    let stop_at = if post != 0 { StopAt::Never } else { StopAt::Stop };
    let label = format!("sdkhook:{type_name}");
    let result = fan_out_collapsing(
        &snap,
        &label,
        Instrument::breadcrumb(&label),
        stop_at,
        |tc| {
            let this_ref = build_entity_ref(tc, this_index, this_id);
            let other_val: v8::Local<v8::Value> = if other_handle < 0 {
                v8::null(tc).into()
            } else {
                let (oi, os) = crate::entity::decode_handle(other_handle as u32);
                match crate::entity_live::adopt(oi, os) {
                    Some(oid) => build_entity_ref(tc, oi, oid),
                    None => v8::null(tc).into(),
                }
            };
            Some(vec![this_ref, other_val])
        },
    );
    result as c_int
}

/// `Spawn` / `Think` / `Use` / `Reload` pre collapse at `Stop`; void types (PreThink, VPhysics, *Post, …)
/// never read the return.
fn collapsing_stop(type_name: &str) -> StopAt {
    match type_name {
        "Spawn" | "Think" | "Use" | "Reload" => StopAt::Stop,
        _ => StopAt::Never,
    }
}

fn handle_to_ref<'s>(tc: &mut v8::PinScope<'s, '_>, handle: i32) -> v8::Local<'s, v8::Value> {
    if handle < 0 {
        return v8::null(tc).into();
    }
    let (oi, os) = crate::entity::decode_handle(handle as u32);
    match crate::entity_live::adopt(oi, os) {
        Some(oid) => build_entity_ref(tc, oi, oid),
        None => v8::null(tc).into(),
    }
}

fn hook_result_from_ret(tc: &mut v8::PinScope, ret: v8::Local<v8::Value>) -> HookResult {
    if !ret.is_number() {
        return HookResult::Continue;
    }
    match ret.uint32_value(tc).unwrap_or(0) {
        1 => HookResult::Changed,
        2 => HookResult::Handled,
        3 => HookResult::Stop,
        _ => HookResult::Continue,
    }
}

/// Walk the snapshot on the host isolate. `build_args` runs inside the per-handler TryCatch;
/// `after` sees the same Locals plus the return (None on throw). Used by GetMaxHealth / boolean
/// types that must inspect the return Local after the call.
fn each_handler<A, R>(
    snap: &[(String, u64, v8::Global<v8::Function>)],
    label: &str,
    stop_at: StopAt,
    mut build_args: A,
    mut after: R,
) -> HookResult
where
    A: for<'s> FnMut(&mut v8::PinScope<'s, '_>) -> Option<Vec<v8::Local<'s, v8::Value>>>,
    R: for<'s> FnMut(
        &mut v8::PinScope<'s, '_>,
        &[v8::Local<'s, v8::Value>],
        Option<v8::Local<'s, v8::Value>>,
    ) -> HookResult,
{
    if snap.is_empty() {
        return HookResult::Continue;
    }
    match with_host_isolate(|isolate| {
        let mut result = HookResult::Continue;
        for (owner, generation, handler_g) in snap {
            if stop_at != StopAt::Never {
                let truncated = match stop_at {
                    StopAt::Stop => result == HookResult::Stop,
                    StopAt::Handled => result >= HookResult::Handled,
                    StopAt::Never => false,
                };
                if truncated {
                    break;
                }
            }
            if !owner_is_live(owner, *generation) {
                continue;
            }
            let Some(g_ctx) = clone_plugin_context(owner) else {
                continue;
            };
            let _crash_guard = crate::crash::breadcrumb::enter_dispatch(owner, label);
            let mut hs_storage = v8::HandleScope::new(isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let Some(args) = build_args(tc) else {
                continue;
            };
            let func = v8::Local::new(tc, handler_g);
            let ret = match func.call(tc, recv, &args) {
                Some(ret) => Some(ret),
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!("WARN: {label}: handler '{owner}': {msg}"));
                    None
                }
            };
            let hr = after(tc, &args, ret);
            if hr > result {
                result = hr;
            }
        }
        result
    }) {
        Ok(r) => r,
        Err(_) => HookResult::Continue,
    }
}

/// This-only VP virtuals (Spawn/Think/PreThink/PostThink/VPhysics/GroundEntChangedPost).
pub(crate) fn dispatch_this(this_index: i32, this_serial: i32, post: c_int, type_name: &str) -> c_int {
    let _ = post;
    let Some(this_id) = crate::entity_live::adopt(this_index, this_serial) else {
        return HookResult::Continue as c_int;
    };
    let snap = snapshot_kind(this_id, type_name);
    if snap.is_empty() {
        return HookResult::Continue as c_int;
    }
    let label = format!("sdkhook:{type_name}");
    let result = fan_out_collapsing(
        &snap,
        &label,
        Instrument::breadcrumb(&label),
        collapsing_stop(type_name),
        |tc| Some(vec![build_entity_ref(tc, this_index, this_id)]),
    );
    result as c_int
}

/// `Use` / `UsePost`. `activator_handle` / `caller_handle` are packed `CEntityHandle` ints (`-1` = null).
pub(crate) fn dispatch_use(
    this_index: i32,
    this_serial: i32,
    activator_handle: i32,
    caller_handle: i32,
    use_type: i32,
    value: f32,
    post: c_int,
    type_name: &str,
) -> c_int {
    let Some(this_id) = crate::entity_live::adopt(this_index, this_serial) else {
        return HookResult::Continue as c_int;
    };
    let snap = snapshot_kind(this_id, type_name);
    if snap.is_empty() {
        return HookResult::Continue as c_int;
    }
    let stop_at = if post != 0 { StopAt::Never } else { collapsing_stop(type_name) };
    let label = format!("sdkhook:{type_name}");
    let result = fan_out_collapsing(
        &snap,
        &label,
        Instrument::breadcrumb(&label),
        stop_at,
        |tc| {
            let this_ref = build_entity_ref(tc, this_index, this_id);
            let act = handle_to_ref(tc, activator_handle);
            let caller = handle_to_ref(tc, caller_handle);
            let ty = v8::Integer::new(tc, use_type).into();
            let val = v8::Number::new(tc, value as f64).into();
            Some(vec![this_ref, act, caller, ty, val])
        },
    );
    result as c_int
}

/// GetMaxHealth: mutate `{ maxHealth }` in place. SUPERCEDE when collapsed result `>= Handled`.
pub(crate) fn dispatch_getmaxhealth(this_index: i32, this_serial: i32, max_health: &mut i32) -> c_int {
    let Some(this_id) = crate::entity_live::adopt(this_index, this_serial) else {
        return HookResult::Continue as c_int;
    };
    let snap = snapshot_kind(this_id, "GetMaxHealth");
    if snap.is_empty() {
        return HookResult::Continue as c_int;
    }
    let current = Cell::new(*max_health);
    let result = each_handler(
        &snap,
        "sdkhook:GetMaxHealth",
        StopAt::Stop,
        |tc| {
            let obj = v8::Object::new(tc);
            let key = v8::String::new(tc, "maxHealth")?;
            let val = v8::Integer::new(tc, current.get());
            obj.set(tc, key.into(), val.into());
            Some(vec![obj.into()])
        },
        |tc, args, ret| {
            if let Some(arg) = args.first() {
                if let Ok(obj) = v8::Local::<v8::Object>::try_from(*arg) {
                    if let Some(k) = v8::String::new(tc, "maxHealth") {
                        if let Some(v) = obj.get(tc, k.into()) {
                            if let Some(n) = v.int32_value(tc) {
                                current.set(n);
                            }
                        }
                    }
                }
            }
            match ret {
                Some(r) => hook_result_from_ret(tc, r),
                None => HookResult::Continue,
            }
        },
    );
    *max_health = current.get();
    result as c_int
}

/// ShouldCollide: last defined boolean wins; default `orig` (1/0). Not HookResult collapse.
pub(crate) fn dispatch_shouldcollide(
    this_index: i32,
    this_serial: i32,
    collision_group: i32,
    contents_mask: i32,
    orig: c_int,
) -> c_int {
    let Some(this_id) = crate::entity_live::adopt(this_index, this_serial) else {
        return orig;
    };
    let snap = snapshot_kind(this_id, "ShouldCollide");
    if snap.is_empty() {
        return orig;
    }
    let out = Cell::new(orig != 0);
    each_handler(
        &snap,
        "sdkhook:ShouldCollide",
        StopAt::Never,
        |tc| {
            let this_ref = build_entity_ref(tc, this_index, this_id);
            let group = v8::Integer::new(tc, collision_group).into();
            let mask = v8::Integer::new(tc, contents_mask).into();
            let orig_v = v8::Boolean::new(tc, orig != 0).into();
            Some(vec![this_ref, group, mask, orig_v])
        },
        |_tc, _args, ret| {
            if let Some(ret) = ret {
                if ret.is_boolean() {
                    out.set(ret.is_true());
                }
            }
            HookResult::Continue
        },
    );
    if out.get() { 1 } else { 0 }
}

/// Source 2 player controllers sit at entity index `slot + 1` (1..=64). Never invent slot 0.
fn client_slot_for_entity(index: i32) -> Option<i32> {
    const MAX_CLIENTS: i32 = 64;
    if !(1..=MAX_CLIENTS).contains(&index) {
        return None;
    }
    let slot = index - 1;
    let ops = engine_ops()?;
    let f = ops.client_valid?;
    (f(slot) != 0).then_some(slot)
}

fn build_client<'s>(scope: &mut v8::PinScope<'s, '_>, slot: i32) -> Option<v8::Local<'s, v8::Value>> {
    let global = scope.get_current_context().global(scope);
    let pkg_key = v8::String::new(scope, "__s2pkg_clients")?;
    let pkg = global.get(scope, pkg_key.into())?;
    let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
    let ctor_key = v8::String::new(scope, "Client")?;
    let ctor_val = pkg.get(scope, ctor_key.into())?;
    let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
    let slot_v = v8::Integer::new(scope, slot);
    ctor.new_instance(scope, &[slot_v.into()]).map(|o| o.into())
}

/// CanBeAutobalanced: last defined boolean wins. No Client → skip callbacks, return `orig`.
pub(crate) fn dispatch_canbeautobalanced(this_index: i32, this_serial: i32, orig: c_int) -> c_int {
    let Some(this_id) = crate::entity_live::adopt(this_index, this_serial) else {
        return orig;
    };
    let Some(slot) = client_slot_for_entity(this_index) else {
        return orig;
    };
    let snap = snapshot_kind(this_id, "CanBeAutobalanced");
    if snap.is_empty() {
        return orig;
    }
    let out = Cell::new(orig != 0);
    each_handler(
        &snap,
        "sdkhook:CanBeAutobalanced",
        StopAt::Never,
        |tc| {
            let client = build_client(tc, slot)?;
            let orig_v = v8::Boolean::new(tc, orig != 0).into();
            Some(vec![client, orig_v])
        },
        |_tc, _args, ret| {
            if let Some(ret) = ret {
                if ret.is_boolean() {
                    out.set(ret.is_true());
                }
            }
            HookResult::Continue
        },
    );
    if out.get() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, mock_event_ops};
    use crate::v8host::{
        create_plugin_context, dispatch_damage, dispatch_damage_post, eval_in_context, init, load_plugin_js, plugin_phase,
        set_engine_ops, shutdown, unload_plugin, S2EngineOps,
    };
    use std::cell::Cell;
    use std::os::raw::c_char;
    use std::sync::Mutex;

    thread_local! {
        static FAKE_VICTIM: Cell<i32> = const { Cell::new(-1) };
        static VP_ADDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static VP_REMOVES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static VP_DROPS: RefCell<Vec<(i32, i32)>> = const { RefCell::new(Vec::new()) };
        static VP_ADD_OK: Cell<i32> = const { Cell::new(1) };
        static CLIENT_VALID: Cell<i32> = const { Cell::new(-1) };
    }
    static DMG_WRITE_REC: Mutex<Option<(i32, f32)>> = Mutex::new(None);

    extern "C" fn fake_damage_victim() -> c_int {
        FAKE_VICTIM.with(|c| c.get())
    }
    extern "C" fn rec_damage_write_float(offset: c_int, value: f32) {
        *DMG_WRITE_REC.lock().unwrap() = Some((offset, value));
    }
    extern "C" fn fake_dmg_schema_offset(_cls: *const c_char, _field: *const c_char) -> c_int {
        68
    }
    extern "C" fn fake_vp_add(index: c_int, serial: c_int, ty: *const c_char, post: c_int) -> c_int {
        let name = if ty.is_null() {
            String::new()
        } else {
            unsafe { std::ffi::CStr::from_ptr(ty) }.to_string_lossy().into_owned()
        };
        VP_ADDS.with(|v| v.borrow_mut().push(format!("{name}:{post}:{index}:{serial}")));
        VP_ADD_OK.with(|c| c.get())
    }
    extern "C" fn fake_vp_remove(index: c_int, serial: c_int, ty: *const c_char, post: c_int) -> c_int {
        let name = if ty.is_null() {
            String::new()
        } else {
            unsafe { std::ffi::CStr::from_ptr(ty) }.to_string_lossy().into_owned()
        };
        VP_REMOVES.with(|v| v.borrow_mut().push(format!("{name}:{post}:{index}:{serial}")));
        1
    }
    extern "C" fn fake_vp_drop(index: c_int, serial: c_int) -> c_int {
        VP_DROPS.with(|v| v.borrow_mut().push((index, serial)));
        1
    }

    fn seed(index: i32, serial: i32) -> u64 {
        crate::entity_live::reset_for_tests();
        let id = crate::entity_live::on_created(index, serial);
        FAKE_VICTIM.with(|c| c.set(packed_handle(index, serial)));
        id
    }

    fn ops_with_victim() -> S2EngineOps {
        S2EngineOps {
            damage_victim: Some(fake_damage_victim),
            ..mock_event_ops()
        }
    }

    fn ops_with_vp() -> S2EngineOps {
        VP_ADDS.with(|v| v.borrow_mut().clear());
        VP_REMOVES.with(|v| v.borrow_mut().clear());
        VP_DROPS.with(|v| v.borrow_mut().clear());
        VP_ADD_OK.with(|c| c.set(1));
        S2EngineOps {
            sdkhook_vp_add: Some(fake_vp_add),
            sdkhook_vp_remove: Some(fake_vp_remove),
            sdkhook_vp_drop: Some(fake_vp_drop),
            ..mock_event_ops()
        }
    }

    fn hook_js(index: i32, id: u64, body: &str) -> String {
        format!(
            r#"
            globalThis.__cb = function (info) {{ {body} }};
            String(__s2_sdkhook({index}, {id}, "OnTakeDamage", globalThis.__cb))
            "#
        )
    }

    #[test]
    fn sdkhook_false_on_stale_entity() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", r#"String(__s2_sdkhook(99, 1, "OnTakeDamage", function () {}))"#),
            "false"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_dispatch_only_matching_victim() {
        let _ = init(dummy_logger());
        let id_a = seed(5, 1);
        let id_b = crate::entity_live::on_created(6, 2);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        assert_eq!(eval_in_context_string("p", &hook_js(5, id_a, "globalThis.__a++;")), "true");
        assert_eq!(eval_in_context_string("p", &hook_js(6, id_b, "globalThis.__b++;")), "true");
        FAKE_VICTIM.with(|c| c.set(packed_handle(5, 1)));
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "0", "other entity must not run");
        shutdown();
    }

    #[test]
    fn sdkhook_no_victim_runs_nobody() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0;").unwrap();
        eval_in_context_string("p", &hook_js(5, id, "globalThis.__n++;"));
        FAKE_VICTIM.with(|c| c.set(-1));
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__n)"), "0");
        shutdown();
    }

    #[test]
    fn sdkhook_handled_does_not_stop_the_chain() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string("p", &hook_js(5, id, "globalThis.__a++; return HookResult.Handled;"));
        eval_in_context_string("p", &hook_js(5, id, "globalThis.__b++; return HookResult.Continue;"));
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "1");
        shutdown();
    }

    #[test]
    fn sdkhook_stop_truncates_the_chain() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string("p", &hook_js(5, id, "globalThis.__a++; return HookResult.Stop;"));
        eval_in_context_string("p", &hook_js(5, id, "globalThis.__b++;"));
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "0");
        shutdown();
    }

    #[test]
    fn sdkhook_handled_zeroes_live_damage() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        *DMG_WRITE_REC.lock().unwrap() = None;
        set_engine_ops(Some(S2EngineOps {
            schema_offset: Some(fake_dmg_schema_offset),
            damage_write_float: Some(rec_damage_write_float),
            damage_victim: Some(fake_damage_victim),
            ..mock_event_ops()
        }));
        create_plugin_context("p");
        eval_in_context_string("p", &hook_js(5, id, "return HookResult.Handled;"));
        dispatch_damage();
        assert_eq!(*DMG_WRITE_REC.lock().unwrap(), Some((68, 0.0)));
        shutdown();
    }

    #[test]
    fn sdkunhook_removes_one_callback() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0; globalThis.__cb = function () { globalThis.__n++; };").unwrap();
        eval_in_context_string(
            "p",
            &format!(r#"String(__s2_sdkhook(5, {id}, "OnTakeDamage", globalThis.__cb))"#),
        );
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(r#"String(__s2_sdkunhook(5, {id}, "OnTakeDamage", globalThis.__cb))"#),
            ),
            "true"
        );
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__n)"), "0");
        shutdown();
    }

    #[test]
    fn sdkhook_destroy_unhooks() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0;").unwrap();
        eval_in_context_string("p", &hook_js(5, id, "globalThis.__n++;"));
        drop_entity(id);
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__n)"), "0");
        shutdown();
    }

    #[test]
    fn sdkhook_works_after_settle() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        load_plugin_js(
            "hookmore",
            r#"module.exports.OnPluginStart = function () {};"#,
            "{}",
        );
        assert_eq!(plugin_phase("hookmore"), Some(crate::plugin::Phase::Active));
        eval_in_context("hookmore", "globalThis.__n=0;").unwrap();
        assert_eq!(
            eval_in_context_string("hookmore", &hook_js(5, id, "globalThis.__n++;")),
            "true"
        );
        dispatch_damage();
        assert_eq!(eval_in_context_string("hookmore", "String(globalThis.__n)"), "1");
        shutdown();
    }

    #[test]
    fn sdkhook_unload_clears() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        load_plugin_js(
            "u",
            r#"
            module.exports.OnPluginStart = function () {
                globalThis.__n = 0;
                globalThis.__cb = function () { globalThis.__n++; };
            };
            "#,
            "{}",
        );
        eval_in_context_string("u", &format!(r#"String(__s2_sdkhook(5, {id}, "OnTakeDamage", globalThis.__cb))"#));
        unload_plugin("u");
        // Context is gone; dispatch must not panic. Victim still set.
        dispatch_damage();
        shutdown();
    }

    #[test]
    fn sdkhook_unknown_type_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(r#"String(__s2_sdkhook(5, {id}, "OnTouch", function () {{}}))"#),
            ),
            "false"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_prelude_throws_on_unsupported_type() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        let msg = eval_in_context_string(
            "p",
            &format!(
                r#"
                (function () {{
                    try {{
                        __s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, "NotAType", function () {{}});
                        return "no";
                    }} catch (e) {{ return String(e && e.message || e); }}
                }})()
                "#
            ),
        );
        assert!(
            msg.contains("SDKHook type 'NotAType' is not supported"),
            "unknown type must throw a named reason, got: {msg}"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_prelude_null_entity_returns_false() {
        let _ = init(dummy_logger());
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string(
                "p",
                r#"String(__s2pkg_sdkhooks.SDKHook(null, "OnTakeDamage", function () {}))"#,
            ),
            "false"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_prelude_touch_without_op_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(r#"String(__s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, "Touch", function () {{}}))"#),
            ),
            "false",
            "wiki Touch with no VP op degrades to false (does not throw)"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_touch_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(r#"String(__s2_sdkhook(5, {id}, "Touch", function () {{}}))"#),
            ),
            "true"
        );
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["Touch:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_touch_second_add_same_does_not() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context_string("p", &format!(r#"String(__s2_sdkhook(5, {id}, "Touch", function () {{}}))"#));
        eval_in_context_string("p", &format!(r#"String(__s2_sdkhook(5, {id}, "Touch", function () {{}}))"#));
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds.len(), 1, "second callback on the same entity+type must not re-install SourceHook");
        shutdown();
    }

    #[test]
    fn sdkhook_touch_op_zero_does_not_push() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        VP_ADD_OK.with(|c| c.set(0));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(r#"String(__s2_sdkhook(5, {id}, "Touch", function () {{}}))"#),
            ),
            "false"
        );
        shutdown();
        // Re-init so a later dispatch would not find a leaked entry. Table is process-global.
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        let id = crate::entity_live::on_created(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0;").unwrap();
        // No entry from the failed add; a successful add + dispatch is a different test.
        let _ = (id,);
        shutdown();
    }

    #[test]
    fn sdkhook_touch_handled_does_not_stop() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(r#"
                globalThis.__h1 = function () {{ globalThis.__a++; return HookResult.Handled; }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(__s2_sdkhook(5, {id}, "Touch", globalThis.__h1) && __s2_sdkhook(5, {id}, "Touch", globalThis.__h2))
            "#),
        );
        let r = dispatch_touch(5, 1, -1, 0, "Touch");
        assert_eq!(r, HookResult::Handled as c_int);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "1");
        shutdown();
    }

    #[test]
    fn sdkhook_touch_stop_truncates() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(r#"
                globalThis.__h1 = function () {{ globalThis.__a++; return HookResult.Stop; }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(__s2_sdkhook(5, {id}, "Touch", globalThis.__h1) && __s2_sdkhook(5, {id}, "Touch", globalThis.__h2))
            "#),
        );
        let r = dispatch_touch(5, 1, -1, 0, "Touch");
        assert_eq!(r, HookResult::Stop as c_int);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "0");
        shutdown();
    }

    #[test]
    fn sdkhook_touch_unhook_last_calls_remove() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context_string(
            "p",
            &format!(r#"
                globalThis.__cb = function () {{}};
                String(__s2_sdkhook(5, {id}, "Touch", globalThis.__cb))
            "#),
        );
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(r#"String(__s2_sdkunhook(5, {id}, "Touch", globalThis.__cb))"#),
            ),
            "true"
        );
        let removes = VP_REMOVES.with(|v| v.borrow().clone());
        assert_eq!(removes, vec!["Touch:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_touch_destroy_drop() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context_string(
            "p",
            &format!(r#"String(__s2_sdkhook(5, {id}, "Touch", function () {{}}))"#),
        );
        drop_entity(id);
        let drops = VP_DROPS.with(|v| v.borrow().clone());
        assert_eq!(drops, vec![(5, 1)]);
        shutdown();
    }

    #[test]
    fn sdkhook_touch_passes_other_ref() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        let id = crate::entity_live::on_created(5, 1);
        let _other = crate::entity_live::on_created(6, 2);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context(
            "p",
            r#"
            globalThis.__idx = -1;
            globalThis.__other = -1;
            globalThis.__cb = function (ent, other) {
                globalThis.__idx = ent.index;
                globalThis.__other = other ? other.index : -2;
            };
            "#,
        )
        .unwrap();
        eval_in_context_string(
            "p",
            &format!(r#"String(__s2_sdkhook(5, {id}, "Touch", globalThis.__cb))"#),
        );
        let other_h = packed_handle(6, 2);
        dispatch_touch(5, 1, other_h, 0, "Touch");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__idx)"), "5");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__other)"), "6");
        shutdown();
    }

    fn hook_named(index: i32, id: u64, kind: &str) -> String {
        format!(r#"String(__s2_sdkhook({index}, {id}, "{kind}", function () {{}}))"#)
    }

    #[test]
    fn sdkhook_spawn_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", &hook_named(5, id, "Spawn")), "true");
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["Spawn:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_spawn_second_add_same_does_not() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context_string("p", &hook_named(5, id, "Spawn"));
        eval_in_context_string("p", &hook_named(5, id, "Spawn"));
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds.len(), 1, "second Spawn callback must not re-install SourceHook");
        shutdown();
    }

    #[test]
    fn sdkhook_spawn_missing_op_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "Spawn")),
            "false",
            "wiki Spawn with no VP op degrades to false"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_think_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", &hook_named(5, id, "Think")), "true");
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["Think:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_think_post_add_is_post_flag() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", &hook_named(5, id, "ThinkPost")), "true");
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["Think:1:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_use_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", &hook_named(5, id, "Use")), "true");
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["Use:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_getmaxhealth_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", &hook_named(5, id, "GetMaxHealth")), "true");
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["GetMaxHealth:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_shouldcollide_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", &hook_named(5, id, "ShouldCollide")), "true");
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["ShouldCollide:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_canbeautobalanced_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "CanBeAutobalanced")),
            "true",
            "SDKHook may record CanBeAutobalanced even when the instance is not a Client"
        );
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["CanBeAutobalanced:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_prelude_think_without_op_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(r#"String(__s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, "Think", function () {{}}))"#),
            ),
            "false",
            "wiki Think with no VP op degrades to false (does not throw)"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_spawn_handled_supercede() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__h1 = function () {{ globalThis.__a++; return HookResult.Handled; }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(__s2_sdkhook(5, {id}, "Spawn", globalThis.__h1) && __s2_sdkhook(5, {id}, "Spawn", globalThis.__h2))
            "#
            ),
        );
        let r = dispatch_this(5, 1, 0, "Spawn");
        assert_eq!(r, HookResult::Handled as c_int, "Handled SUPERCEDEs the original Spawn virtual");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "1");
        shutdown();
    }

    #[test]
    fn sdkhook_think_handled_supercede() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__h1 = function () {{ globalThis.__a++; return HookResult.Handled; }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(__s2_sdkhook(5, {id}, "Think", globalThis.__h1) && __s2_sdkhook(5, {id}, "Think", globalThis.__h2))
            "#
            ),
        );
        let r = dispatch_this(5, 1, 0, "Think");
        assert_eq!(r, HookResult::Handled as c_int, "Handled SUPERCEDEs the original Think virtual");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "1");
        shutdown();
    }

    #[test]
    fn sdkhook_use_passes_args() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        let id = crate::entity_live::on_created(5, 1);
        let _act = crate::entity_live::on_created(6, 2);
        let _caller = crate::entity_live::on_created(7, 3);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context(
            "p",
            r#"
            globalThis.__seen = "";
            globalThis.__cb = function (ent, act, caller, type, value) {
                globalThis.__seen = [ent.index, act && act.index, caller && caller.index, type, value].join(",");
                return HookResult.Handled;
            };
            "#,
        )
        .unwrap();
        eval_in_context_string("p", &format!(r#"String(__s2_sdkhook(5, {id}, "Use", globalThis.__cb))"#));
        let r = dispatch_use(5, 1, packed_handle(6, 2), packed_handle(7, 3), 1, 0.5, 0, "Use");
        assert_eq!(r, HookResult::Handled as c_int);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__seen)"), "5,6,7,1,0.5");
        shutdown();
    }

    #[test]
    fn sdkhook_getmaxhealth_mutates_and_supercede() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__cb = function (info) {{ info.maxHealth = 50; return HookResult.Handled; }};
                String(__s2_sdkhook(5, {id}, "GetMaxHealth", globalThis.__cb))
            "#
            ),
        );
        let mut max = 100;
        let r = dispatch_getmaxhealth(5, 1, &mut max);
        assert_eq!(r, HookResult::Handled as c_int);
        assert_eq!(max, 50, "mutated maxHealth is read back after fan-out");
        shutdown();
    }

    #[test]
    fn sdkhook_shouldcollide_last_boolean_wins() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__h1 = function () {{ globalThis.__n++; return true; }};
                globalThis.__h2 = function () {{ globalThis.__n++; return false; }};
                String(__s2_sdkhook(5, {id}, "ShouldCollide", globalThis.__h1) && __s2_sdkhook(5, {id}, "ShouldCollide", globalThis.__h2))
            "#
            ),
        );
        let r = dispatch_shouldcollide(5, 1, 2, 3, 1);
        assert_eq!(r, 0, "last defined boolean return wins (false)");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__n)"), "2");
        shutdown();
    }

    #[test]
    fn sdkhook_shouldcollide_void_keeps_original() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__cb = function () {{}};
                String(__s2_sdkhook(5, {id}, "ShouldCollide", globalThis.__cb))
            "#
            ),
        );
        let r = dispatch_shouldcollide(5, 1, 0, 0, 1);
        assert_eq!(r, 1, "void return keeps the original result");
        shutdown();
    }

    extern "C" fn fake_client_valid(slot: c_int) -> c_int {
        CLIENT_VALID.with(|c| if c.get() == slot { 1 } else { 0 })
    }

    fn ops_with_vp_client() -> S2EngineOps {
        let mut ops = ops_with_vp();
        ops.client_valid = Some(fake_client_valid);
        ops
    }

    #[test]
    fn sdkhook_canbeautobalanced_passes_client() {
        let _ = init(dummy_logger());
        // Controller entity index = slot + 1. Slot 3 → index 4.
        let id = seed(4, 1);
        CLIENT_VALID.with(|c| c.set(3));
        set_engine_ops(Some(ops_with_vp_client()));
        create_plugin_context("p");
        eval_in_context(
            "p",
            r#"
            globalThis.__slot = -1;
            globalThis.__orig = null;
            globalThis.__cb = function (client, orig) {
                globalThis.__slot = client.slot;
                globalThis.__orig = orig;
                return false;
            };
            "#,
        )
        .unwrap();
        eval_in_context_string(
            "p",
            &format!(r#"String(__s2_sdkhook(4, {id}, "CanBeAutobalanced", globalThis.__cb))"#),
        );
        let r = dispatch_canbeautobalanced(4, 1, 1);
        assert_eq!(r, 0);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__slot)"), "3");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__orig)"), "true");
        shutdown();
    }

    #[test]
    fn sdkhook_canbeautobalanced_skips_without_client() {
        let _ = init(dummy_logger());
        let id = seed(99, 1);
        CLIENT_VALID.with(|c| c.set(-1));
        set_engine_ops(Some(ops_with_vp_client()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__cb = function () {{ globalThis.__n++; return false; }};
                String(__s2_sdkhook(99, {id}, "CanBeAutobalanced", globalThis.__cb))
            "#
            ),
        );
        let r = dispatch_canbeautobalanced(99, 1, 1);
        assert_eq!(r, 1, "no Client → skip callbacks and return origRet");
        assert_eq!(
            eval_in_context_string("p", "String(globalThis.__n)"),
            "0",
            "must not invent Client(0) when the hooked entity has no client"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_ontakedamage_post_records_without_vp() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "OnTakeDamagePost")),
            "true",
            "OnTakeDamagePost is the DTA mux, not a VP"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_ontakedamage_post_does_not_run_on_pre_dispatch() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__cb = function () {{ globalThis.__n++; }};
                String(__s2_sdkhook(5, {id}, "OnTakeDamagePost", globalThis.__cb))
            "#
            ),
        );
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__n)"), "0");
        dispatch_damage_post();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__n)"), "1");
        shutdown();
    }

    #[test]
    fn sdkhook_ontakedamage_post_handled_does_not_zero() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        *DMG_WRITE_REC.lock().unwrap() = None;
        set_engine_ops(Some(S2EngineOps {
            schema_offset: Some(fake_dmg_schema_offset),
            damage_write_float: Some(rec_damage_write_float),
            damage_victim: Some(fake_damage_victim),
            ..mock_event_ops()
        }));
        create_plugin_context("p");
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__cb = function () {{ return HookResult.Handled; }};
                String(__s2_sdkhook(5, {id}, "OnTakeDamagePost", globalThis.__cb))
            "#
            ),
        );
        dispatch_damage_post();
        assert_eq!(*DMG_WRITE_REC.lock().unwrap(), None);
        shutdown();
    }

    #[test]
    fn sdkhook_ontakedamage_post_setter_is_ignored() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        *DMG_WRITE_REC.lock().unwrap() = None;
        set_engine_ops(Some(S2EngineOps {
            schema_offset: Some(fake_dmg_schema_offset),
            damage_write_float: Some(rec_damage_write_float),
            damage_victim: Some(fake_damage_victim),
            ..mock_event_ops()
        }));
        create_plugin_context("p");
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__pre = function (info) {{ info.damage = 50; }};
                globalThis.__post = function (info) {{ info.damage = 99; }};
                String(__s2_sdkhook(5, {id}, "OnTakeDamage", globalThis.__pre)
                    && __s2_sdkhook(5, {id}, "OnTakeDamagePost", globalThis.__post))
            "#
            ),
        );
        dispatch_damage();
        assert_eq!(*DMG_WRITE_REC.lock().unwrap(), Some((68, 50.0)), "pre-hook setter must write");
        *DMG_WRITE_REC.lock().unwrap() = None;
        dispatch_damage_post();
        assert_eq!(
            *DMG_WRITE_REC.lock().unwrap(),
            None,
            "OnTakeDamagePost must not write through DamageInfo.damage"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_ontakedamage_post_stop_does_not_truncate() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__h1 = function () {{ globalThis.__a++; return HookResult.Stop; }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(__s2_sdkhook(5, {id}, "OnTakeDamagePost", globalThis.__h1)
                    && __s2_sdkhook(5, {id}, "OnTakeDamagePost", globalThis.__h2))
            "#
            ),
        );
        dispatch_damage_post();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "1");
        shutdown();
    }

    #[test]
    fn sdkhook_alive_without_backing_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_victim()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "OnTakeDamageAlive")),
            "false"
        );
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "OnTakeDamageAlivePost")),
            "false"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_traceattack_without_backing_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "TraceAttack")),
            "false"
        );
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "TraceAttackPost")),
            "false"
        );
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "FireBulletsPost")),
            "false"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_prelude_alive_does_not_throw() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string(
                "p",
                &format!(
                    r#"String(__s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, "OnTakeDamageAlive", function () {{}}))"#
                ),
            ),
            "false",
            "wiki Alive with no engine backing degrades to false (does not throw)"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_weapondrop_missing_op_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "WeaponDrop")),
            "false",
            "wiki WeaponDrop with no VP op degrades to false"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_weapondrop_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "WeaponDrop")),
            "true"
        );
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["WeaponDrop:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_weapondrop_dispatch_passes_weapon() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        let id = crate::entity_live::on_created(5, 1);
        let _wpn = crate::entity_live::on_created(6, 2);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context(
            "p",
            r#"
            globalThis.__idx = -1;
            globalThis.__wpn = -1;
            globalThis.__cb = function (ent, weapon) {
                globalThis.__idx = ent.index;
                globalThis.__wpn = weapon ? weapon.index : -2;
            };
            "#,
        )
        .unwrap();
        eval_in_context_string(
            "p",
            &format!(r#"String(__s2_sdkhook(5, {id}, "WeaponDrop", globalThis.__cb))"#),
        );
        let wpn_h = packed_handle(6, 2);
        let r = dispatch_touch(5, 1, wpn_h, 0, "WeaponDrop");
        assert_eq!(r, HookResult::Continue as c_int);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__idx)"), "5");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__wpn)"), "6");
        shutdown();
    }

    #[test]
    fn sdkhook_reload_first_add_calls_op() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "Reload")),
            "true"
        );
        let adds = VP_ADDS.with(|v| v.borrow().clone());
        assert_eq!(adds, vec!["Reload:0:5:1".to_string()]);
        shutdown();
    }

    #[test]
    fn sdkhook_reloadpost_without_backing_returns_false() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_named(5, id, "ReloadPost")),
            "false",
            "ReloadPost is not a this-void VP twin; no bool-return thunk yet"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_reload_stop_truncates() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        set_engine_ops(Some(ops_with_vp()));
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__h1 = function () {{ globalThis.__a++; return HookResult.Stop; }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(__s2_sdkhook(5, {id}, "Reload", globalThis.__h1)
                    && __s2_sdkhook(5, {id}, "Reload", globalThis.__h2))
            "#
            ),
        );
        let r = dispatch_this(5, 1, 0, "Reload");
        assert_eq!(r, HookResult::Stop as c_int);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "0");
        shutdown();
    }
}
