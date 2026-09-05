//! `SDKHook_SetTransmit` — CheckTransmit POST fan-out (AND-merge hide-only).
//!
//! No extra SourceHook and no sdkhooks gamedata. The shim calls these FFI entry points from
//! `Hook_CheckTransmit` after `Transmit.setVisibleTo` bit clears. An empty SetTransmit table
//! must not enter JS (`s2script_core_sdkhook_settransmit_active` is a maintained per-kind count).

use crate::dispatch::{fan_out_collapsing, Instrument, StopAt};
use crate::multiplexer::HookResult;
use crate::sdkhooks::{kind_active, snapshot_kind, snapshot_kind_entities, KIND_SET_TRANSMIT};
use crate::v8host::build_entity_ref;
use std::os::raw::c_int;
use std::panic::catch_unwind;

/// `new globalThis.__s2pkg_clients.Client(slot)` — isolate tests work without `client_valid`.
/// Returns `None` if the constructor is missing; the caller skips that callback (never a fake slot 0).
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

pub(crate) fn dispatch_settransmit(this_index: i32, this_serial: i32, viewer_slot: i32) -> c_int {
    if viewer_slot < 0 {
        return HookResult::Continue as c_int;
    }
    let Some(this_id) = crate::entity_live::adopt(this_index, this_serial) else {
        return HookResult::Continue as c_int;
    };
    let snap = snapshot_kind(this_id, KIND_SET_TRANSMIT);
    if snap.is_empty() {
        return HookResult::Continue as c_int;
    }
    fan_out_collapsing(
        &snap,
        "sdkhook:SetTransmit",
        Instrument::breadcrumb("sdkhook:SetTransmit"),
        StopAt::Stop,
        |tc| {
            let this_ref = build_entity_ref(tc, this_index, this_id);
            let client = build_client(tc, viewer_slot)?;
            Some(vec![this_ref, client])
        },
    ) as c_int
}

/// Constant-time maintained count. 1 = at least one SetTransmit callback; 0 = skip JS entirely.
/// `catch_unwind` → 0 (fail-open: do not hide on a core panic).
#[no_mangle]
pub extern "C" fn s2script_core_sdkhook_settransmit_active() -> c_int {
    catch_unwind(|| if kind_active(KIND_SET_TRANSMIT) { 1 } else { 0 }).unwrap_or(0)
}

/// Fan-out SetTransmit for `(entity, viewer_slot)`. Returns collapsed `HookResult` 0..=3.
/// Shim clears the viewer's bit when `>= 2` (Handled). `catch_unwind` → 0.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_sdkhook_settransmit(
    this_index: c_int,
    this_serial: c_int,
    viewer_slot: c_int,
) -> c_int {
    catch_unwind(|| dispatch_settransmit(this_index, this_serial, viewer_slot)).unwrap_or(0)
}

/// Fill `idx`/`ser` with unique hooked `(index, serial)` pairs, up to `cap`. Returns the count.
/// `catch_unwind` / null / cap<=0 → 0.
#[no_mangle]
pub extern "C" fn s2script_core_sdkhook_settransmit_snapshot(
    idx: *mut c_int,
    ser: *mut c_int,
    cap: c_int,
) -> c_int {
    catch_unwind(|| {
        if idx.is_null() || ser.is_null() || cap <= 0 {
            return 0;
        }
        let pairs = snapshot_kind_entities(KIND_SET_TRANSMIT);
        let n = pairs.len().min(cap as usize);
        for (i, &(index, serial)) in pairs.iter().take(n).enumerate() {
            unsafe {
                *idx.add(i) = index;
                *ser.add(i) = serial;
            }
        }
        n as c_int
    })
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::HookResult;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string};
    use crate::v8host::{
        create_plugin_context, eval_in_context, init, load_plugin_js, shutdown, unload_plugin,
    };

    fn seed(index: i32, serial: i32) -> u64 {
        crate::entity_live::reset_for_tests();
        crate::entity_live::on_created(index, serial)
    }

    fn hook_js(index: i32, id: u64, body: &str) -> String {
        format!(
            r#"
            globalThis.__cb = function (entity, client) {{ {body} }};
            String(__s2pkg_sdkhooks.SDKHook(
              {{index:{index},id:{id}}},
              __s2pkg_sdkhooks.SDKHookType.SetTransmit,
              globalThis.__cb
            ))
            "#
        )
    }

    #[test]
    fn sdkhook_settransmit_active_empty() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        assert_eq!(
            s2script_core_sdkhook_settransmit_active(),
            0,
            "empty SetTransmit table must not look active (zero JS on CheckTransmit)"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_settransmit_prelude_records_without_vp() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", &hook_js(5, id, "")),
            "true",
            "SetTransmit records without a VP op (CheckTransmit mux, not SourceHook)"
        );
        assert_eq!(s2script_core_sdkhook_settransmit_active(), 1);
        let mut idx = [0; 4];
        let mut ser = [0; 4];
        let n = s2script_core_sdkhook_settransmit_snapshot(idx.as_mut_ptr(), ser.as_mut_ptr(), 4);
        assert_eq!(n, 1);
        assert_eq!((idx[0], ser[0]), (5, 1));
        shutdown();
    }

    #[test]
    fn sdkhook_settransmit_entity_enumeration_ignores_other_kinds() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        create_plugin_context("p");
        for index in 1..=100 {
            let id = crate::entity_live::on_created(index, index);
            assert_eq!(
                eval_in_context_string(
                    "p",
                    &format!(r#"String(__s2_sdkhook({index}, {id}, "OnTakeDamage", function () {{}}))"#),
                ),
                "true"
            );
        }
        let id = crate::entity_live::on_created(101, 101);
        assert_eq!(eval_in_context_string("p", &hook_js(101, id, "")), "true");
        let (entities, visited) =
            crate::sdkhooks::snapshot_kind_entities_with_work(KIND_SET_TRANSMIT);
        assert_eq!(entities, vec![(101, 101)]);
        assert_eq!(
            visited, 1,
            "shared enumeration path must visit only the SetTransmit entity, not 100 other kinds"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_settransmit_membership_tracks_first_and_last_subscription() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        let id_a = crate::entity_live::on_created(5, 1);
        let id_b = crate::entity_live::on_created(6, 2);
        create_plugin_context("p");
        eval_in_context(
            "p",
            &format!(r#"
                globalThis.__a1 = function () {{}};
                globalThis.__a2 = function () {{}};
                globalThis.__b = function () {{}};
                __s2_sdkhook(5, {id_a}, "SetTransmit", globalThis.__a1);
                __s2_sdkhook(6, {id_b}, "SetTransmit", globalThis.__b);
                __s2_sdkhook(5, {id_a}, "SetTransmit", globalThis.__a2);
            "#),
        ).unwrap();
        assert_eq!(snapshot_kind_entities(KIND_SET_TRANSMIT), vec![(5, 1), (6, 2)]);
        assert_eq!(eval_in_context_string(
            "p", &format!(r#"String(__s2_sdkunhook(5, {id_a}, "SetTransmit", globalThis.__a1))"#)
        ), "true");
        assert_eq!(
            snapshot_kind_entities(KIND_SET_TRANSMIT),
            vec![(6, 2), (5, 1)],
            "entity order follows the earliest remaining subscription"
        );
        assert_eq!(eval_in_context_string(
            "p", &format!(r#"String(__s2_sdkunhook(5, {id_a}, "SetTransmit", globalThis.__a2))"#)
        ), "true");
        assert_eq!(snapshot_kind_entities(KIND_SET_TRANSMIT), vec![(6, 2)]);
        assert!(kind_active(KIND_SET_TRANSMIT));
        crate::sdkhooks::drop_entity(id_b);
        assert!(snapshot_kind_entities(KIND_SET_TRANSMIT).is_empty());
        assert!(!kind_active(KIND_SET_TRANSMIT));
        shutdown();
    }

    #[test]
    fn sdkhook_settransmit_owner_removal_reorders_by_earliest_remaining_subscription() {
        let _ = init(dummy_logger());
        crate::entity_live::reset_for_tests();
        let id_a = crate::entity_live::on_created(5, 1);
        let id_b = crate::entity_live::on_created(6, 2);
        for owner in ["a-first", "b", "a-last"] {
            load_plugin_js(owner, "module.exports.OnPluginStart = function () {};", "{}");
        }
        assert_eq!(
            eval_in_context_string("a-first", &hook_js(5, id_a, "")),
            "true"
        );
        assert_eq!(eval_in_context_string("b", &hook_js(6, id_b, "")), "true");
        assert_eq!(
            eval_in_context_string("a-last", &hook_js(5, id_a, "")),
            "true"
        );
        assert_eq!(snapshot_kind_entities(KIND_SET_TRANSMIT), vec![(5, 1), (6, 2)]);

        unload_plugin("a-first");
        assert_eq!(
            snapshot_kind_entities(KIND_SET_TRANSMIT),
            vec![(6, 2), (5, 1)],
            "owner teardown must use the same reorder path as explicit unhook"
        );
        shutdown();
    }

    #[test]
    fn sdkhook_settransmit_handled_returns_gte_2() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0; globalThis.__slot=-1;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__h1 = function (entity, client) {{
                    globalThis.__a++;
                    globalThis.__slot = client.slot;
                    globalThis.__idx = entity.index;
                    return HookResult.Handled;
                }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(
                    __s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, __s2pkg_sdkhooks.SDKHookType.SetTransmit, globalThis.__h1)
                    && __s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, __s2pkg_sdkhooks.SDKHookType.SetTransmit, globalThis.__h2)
                )
                "#
            ),
        );
        let r = s2script_core_dispatch_sdkhook_settransmit(5, 1, 3);
        assert!(r >= 2, "Handled must return >= 2 so the shim clears the bit, got {r}");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "1", "Handled does not skip later callbacks");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__slot)"), "3");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__idx)"), "5");
        shutdown();
    }

    #[test]
    fn sdkhook_settransmit_stop_truncates() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0;").unwrap();
        eval_in_context_string(
            "p",
            &format!(
                r#"
                globalThis.__h1 = function () {{ globalThis.__a++; return HookResult.Stop; }};
                globalThis.__h2 = function () {{ globalThis.__b++; }};
                String(
                    __s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, __s2pkg_sdkhooks.SDKHookType.SetTransmit, globalThis.__h1)
                    && __s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, __s2pkg_sdkhooks.SDKHookType.SetTransmit, globalThis.__h2)
                )
                "#
            ),
        );
        let r = s2script_core_dispatch_sdkhook_settransmit(5, 1, 3);
        assert_eq!(r, HookResult::Stop as c_int);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__b)"), "0");
        shutdown();
    }

    #[test]
    fn sdkhook_settransmit_continue_returns_0() {
        let _ = init(dummy_logger());
        let id = seed(5, 1);
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__n=0;").unwrap();
        eval_in_context_string("p", &hook_js(5, id, "globalThis.__n++;"));
        let r = s2script_core_dispatch_sdkhook_settransmit(5, 1, 3);
        assert_eq!(r, HookResult::Continue as c_int);
        assert_eq!(eval_in_context_string("p", "String(globalThis.__n)"), "1");
        shutdown();
    }
}
