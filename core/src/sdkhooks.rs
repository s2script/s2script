//! Per-entity SDKHooks — SourceMod `SDKHook` / `SDKUnhook`.
//!
//! The table is books-gated host identity (`entity_live` id), never a raw pointer. `OnTakeDamage`
//! fans out from the process-wide `DispatchTraceAttack` detour. The Touch family is per-entity
//! SourceHook (`sdkhook_vp_add` / `SH_ADD_MANUALHOOK`), not `SH_ADD_MANUALVPHOOK`.

use crate::dispatch::{fan_out_collapsing, Instrument, StopAt};
use crate::multiplexer::HookResult;
use crate::v8host::{
    build_entity_ref, current_plugin, engine_ops, next_sub_id, plugin_generation, set_native,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::CString;
use std::os::raw::c_int;

const KIND_ON_TAKE_DAMAGE: &str = "OnTakeDamage";

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

/// Wiki Touch-family name → (gamedata / VP type without Post, post flag).
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
        _ => None,
    }
}

fn is_known_kind(kind: &str) -> bool {
    kind == KIND_ON_TAKE_DAMAGE || vp_kind(kind).is_some()
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
    let Some(vid) = current_victim_id() else {
        return Vec::new();
    };
    HOOKS.with(|h| {
        h.borrow()
            .iter()
            .filter(|e| e.entity_id == vid && e.kind == KIND_ON_TAKE_DAMAGE)
            .map(|e| (e.owner.clone(), e.generation, e.handler.clone()))
            .collect()
    })
}

fn snapshot_kind(entity_id: u64, kind: &str) -> Vec<(String, u64, v8::Global<v8::Function>)> {
    HOOKS.with(|h| {
        h.borrow()
            .iter()
            .filter(|e| e.entity_id == entity_id && e.kind == kind)
            .map(|e| (e.owner.clone(), e.generation, e.handler.clone()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, mock_event_ops};
    use crate::v8host::{
        create_plugin_context, dispatch_damage, eval_in_context, init, load_plugin_js, plugin_phase,
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
}
