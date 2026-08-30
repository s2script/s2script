//! Per-entity SDKHooks — SourceMod `SDKHook` / `SDKUnhook`.
//!
//! The table is books-gated host identity (`entity_live` id), never a raw pointer. First shipped
//! type is `OnTakeDamage`: `dispatch_damage` fans only callbacks whose hooked id matches the
//! victim. The `DispatchTraceAttack` detour stays process-wide; this module is the per-entity
//! fan-out, not `SH_ADD_MANUALVPHOOK`.

use crate::v8host::{
    current_plugin, engine_ops, next_sub_id, plugin_generation, set_native,
};
use std::cell::RefCell;
use std::os::raw::c_int;

const KIND_ON_TAKE_DAMAGE: &str = "OnTakeDamage";

struct Entry {
    owner: String,
    generation: u64,
    entity_id: u64,
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

/// Drop every hook on this host id (entity destroy).
pub(crate) fn drop_entity(id: u64) {
    HOOKS.with(|h| h.borrow_mut().retain(|e| e.entity_id != id));
}

/// Map transition: every entity dies.
pub(crate) fn drop_all() {
    HOOKS.with(|h| h.borrow_mut().clear());
}

pub(crate) fn register_stores() {
    crate::owner_stores::register(
        "SDKHOOKS",
        Box::new(|owner| {
            HOOKS.with(|h| h.borrow_mut().retain(|e| e.owner != owner));
        }),
        Box::new(|ids| {
            HOOKS.with(|h| h.borrow_mut().retain(|e| !ids.contains(&e.sub_id)));
        }),
        Box::new(|| {
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
        if crate::entity_live::engine_serial_for(index, id).is_none() {
            return;
        }
        let kind = args.get(2).to_rust_string_lossy(scope);
        if kind != KIND_ON_TAKE_DAMAGE {
            return;
        }
        let Ok(func) = v8::Local::<v8::Function>::try_from(args.get(3)) else {
            return;
        };
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = plugin_generation(&owner);
        let sub_id = next_sub_id();
        let handler = v8::Global::new(scope.as_ref(), func);
        HOOKS.with(|h| {
            h.borrow_mut().push(Entry {
                owner,
                generation,
                entity_id: id,
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
                v.remove(i);
                true
            } else {
                false
            }
        });
        rv.set_bool(removed);
    }));
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
                        __s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, "OnTouch", function () {{}});
                        return "no";
                    }} catch (e) {{ return String(e && e.message || e); }}
                }})()
                "#
            ),
        );
        assert!(
            msg.contains("SDKHook type 'OnTouch' is not supported"),
            "unsupported type must throw a named reason, got: {msg}"
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
}
