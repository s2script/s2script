use super::*;
use crate::engine_functions::{contract, overrides, registry, tests};

#[test]
fn optional_binding_reports_unavailable_reason() {
    init(frame_tests::dummy_logger()).unwrap();
    create_plugin_context("@demo/fire");
    let value = tests::fixture();
    let parsed = contract::parse(
        &value.to_string(),
        "@demo/fire",
        &tests::summary(&value),
        &["engine:calls".into()],
    )
    .unwrap();
    let candidate = overrides::prepare(parsed, "archive-test", vec![]).unwrap();
    let owner = contract::OwnerKey::plugin("@demo/fire", plugin_generation("@demo/fire"));
    let receipt = registry::prepare_owner(&owner.id, candidate).unwrap();
    registry::activate_owner(receipt, owner).unwrap();
    let result = eval_in_context(
        "@demo/fire",
        r#"(() => {
        const f = __s2pkg_unsafe.Engine.function('fire');
        if (f.available !== false || !f.status.reason || f.status.canonicalId !== '@demo/fire::fire') throw Error('missing unavailable status');
        if ('call' in f || 'onPre' in f || !Object.isFrozen(f) || !Object.isFrozen(f.status.provenance)) throw Error('bad facade');
        if (f.status.provenance.archiveHash !== 'archive-test') throw Error('bad provenance');
    })()"#,
    );
    shutdown();
    result.unwrap();
}

fn scalar_owner(id: &str, configure: impl FnOnce(&mut serde_json::Value)) -> u64 {
    frame_tests::load_body(id, "return {};", "{}");
    function_adapter::proof::prepared_binding(id, configure)
}
fn js(id: &str, source: &str) {
    eval_in_context(id, source).unwrap();
}
fn finish() {
    shutdown();
    set_engine_ops(None);
}

#[test]
fn required_facade_surfaces_scalar_calls_and_disposable_subscription_states() {
    function_adapter::scalar_transport_tests::init_transport();
    let mut ops = engine_ops().unwrap();
    ops.function_hook_status = Some(status);
    set_engine_ops(Some(ops));
    STATE.with(|s| s.set(1));
    scalar_owner("public-call", |_| {});
    js(
        "public-call",
        r#"
        globalThis.f = __s2pkg_unsafe.Engine.function('fire');
        if (!f.available || f.call(17) !== 17 || !Object.isFrozen(f)) throw Error('call');
        if (f.status.hookObservation !== 'not-requested' || !f.status.provenance.required) throw Error('initial status');
        globalThis.s = f.onPre(() => {});
        if (!Object.isFrozen(s) || s.status !== 'pending' || s.reason !== null || f.status.hookObservation !== 'pending') throw Error('pending');
        let bad = false; try { f.call('17'); } catch (_) { bad = true; } if (!bad) throw Error('typed argument');
    "#,
    );
    STATE.with(|s| s.set(2));
    js(
        "public-call",
        "if(s.status !== 'active' || f.status.hookObservation !== 'active') throw Error('active');",
    );
    STATE.with(|s| s.set(5));
    js(
        "public-call",
        "if(s.status !== 'failed' || !s.reason || f.status.hookObservation !== 'failed') throw Error('failed'); if(!s.dispose() || s.dispose() || s.status !== 'disposed' || s.reason !== null) throw Error('dispose');",
    );
    scalar_owner("call-only", |f| {
        f["policy"]["surfaces"] = serde_json::json!(["call"]);
        f["policy"]["suppression"] = "none".into();
    });
    js(
        "call-only",
        "const f=__s2pkg_unsafe.Engine.function('fire'); if ('onPre' in f || 'onPost' in f || f.call(9)!==9) throw Error('surfaces');",
    );
    scalar_owner("pre-only", |f| {
        f["policy"]["surfaces"] = serde_json::json!(["pre"]);
    });
    js(
        "pre-only",
        r#"const f=__s2pkg_unsafe.Engine.function('fire');
        if ('call' in f || 'onPost' in f || typeof f.onPre!=='function') throw Error('pre surfaces');
        let rejected=0; for(const op of [()=>f.onPre(async()=>{}),()=>f.onPre({observeOnly:false},()=>{}),()=>__s2pkg_unsafe.Engine.function('unknown')]) {try{op()}catch(_){rejected++}}
        if(rejected!==3)throw Error('public admission');"#,
    );
    finish();
}
thread_local! { static STATE: std::cell::Cell<u32> = const {std::cell::Cell::new(1)}; }
extern "C" fn status(_: i64, out: *mut S2FunctionHookStatus, _: *mut i8, _: i32) -> i32 {
    unsafe {
        *out = S2FunctionHookStatus {
            state: STATE.with(|s| s.get()),
            reserved: 0,
            receipt: 1,
        };
    }
    1
}

#[test]
fn public_pre_mutation_observers_post_and_typed_suppression_use_real_v8_dispatch() {
    function_adapter::scalar_transport_tests::init_transport();
    scalar_owner("subscriber", |f| {
        f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
    });
    scalar_owner("caller", |_| {});
    js(
        "subscriber",
        r#"
        globalThis.f = __s2pkg_unsafe.Engine.function('fire');
        globalThis.seen = [];
        globalThis.m = f.onPre(v => { v.x += 3; seen.push(v.x); return 1; });
        globalThis.o = f.onPre({observeOnly:true}, v => { let refused=false; try { v.x = 99; } catch (_) { refused=true; } if(!refused) throw Error('observer mutable'); seen.push(v.x); });
        globalThis.p = f.onPost(v => { let refused=false; try { v.x = 99; } catch (_) { refused=true; } if(!refused) throw Error('post mutable'); seen.push(v.returnValue); });
    "#,
    );
    js(
        "caller",
        "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==4) throw Error('mutated result');",
    );
    js(
        "subscriber",
        "if(JSON.stringify(seen)!=='[7,7,4]') throw Error(JSON.stringify(seen)); m.dispose(); o.dispose(); p.dispose(); globalThis.s=f.onPre(v=>({action:2,returnValue:42})); globalThis.post=f.onPost(v=>seen.push(v.returnValue));",
    );
    js(
        "caller",
        "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==42) throw Error('typed suppression');",
    );
    js(
        "subscriber",
        "if(seen[3]!==42) throw Error('effective post'); s.dispose(); globalThis.bare=f.onPre(()=>2);",
    );
    js(
        "caller",
        "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==4) throw Error('bare suppression accepted');",
    );
    finish();
}

#[test]
fn suppression_none_rejects_untyped_js_decisions_without_committing_their_edits() {
    function_adapter::scalar_transport_tests::init_transport();
    scalar_owner("quiet-sub", |f| {
        f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
        f["policy"]["suppression"] = "none".into();
    });
    scalar_owner("quiet-caller", |_| {});
    js("quiet-sub", r#"
        globalThis.f=__s2pkg_unsafe.Engine.function('fire');
        globalThis.seen=[];
        globalThis.bad=f.onPre(v=>{v.x=99; return {action:2,returnValue:77};});
        globalThis.witness=f.onPre({observeOnly:true},v=>seen.push(v.x));
    "#);
    js("quiet-caller", "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==4)throw Error('invalid suppression committed');");
    js("quiet-sub", "if(JSON.stringify(seen)!=='[4]')throw Error('invalid edit leaked to observer'); bad.dispose(); globalThis.bad=f.onPre(v=>{v.x=99; return 3;});");
    js("quiet-caller", "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==4)throw Error('Stop bypassed suppression:none');");
    js("quiet-sub", "if(JSON.stringify(seen)!=='[4,4]')throw Error('Stop edit leaked to observer'); bad.dispose(); globalThis.good=f.onPre(v=>{v.x=8; return 1;});");
    js("quiet-caller", "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==4)throw Error('changed/continue broken');");
    js("quiet-sub", "if(JSON.stringify(seen)!=='[4,4,8]')throw Error('Changed edit hidden'); globalThis.late=f.onPre(v=>{v.x=99;return 3;}); globalThis.tail=f.onPre({observeOnly:true},v=>seen.push(v.x));");
    js("quiet-caller", "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==4)throw Error('invalid later Stop committed');");
    js("quiet-sub", "if(JSON.stringify(seen)!=='[4,4,8,8,8]')throw Error('prior accepted edit lost'); late.dispose(); globalThis.thrown=f.onPre(v=>{v.x=99;throw Error('bad decision');});");
    js("quiet-caller", "if(__s2pkg_unsafe.Engine.function('fire').call(4)!==4)throw Error('throw committed');");
    js("quiet-sub", "if(JSON.stringify(seen)!=='[4,4,8,8,8,8,8]')throw Error('throw leaked edit');");
    finish();
}

#[test]
fn captured_facade_and_subscription_cannot_reach_replacement_generation() {
    function_adapter::scalar_transport_tests::init_transport();
    scalar_owner("captured", |_| {});
    js(
        "captured",
        "globalThis.f=__s2pkg_unsafe.Engine.function('fire'); globalThis.s=f.onPre(()=>{});",
    );
    let stale = with_host_isolate(|isolate| {
        let mut storage=v8::HandleScope::new(isolate);
        let mut hs=unsafe {std::pin::Pin::new_unchecked(&mut storage)}.init();
        let context=clone_plugin_context("captured").unwrap();
        let context=v8::Local::new(&mut hs, &context);
        let scope=&mut v8::ContextScope::new(&mut hs,context);
        let code=v8::String::new(scope, "(()=>{let n=0;for(const op of [()=>f.call(1),()=>f.available,()=>f.status,()=>s.status,()=>s.dispose()]){try{op()}catch(_){n++}}return n;})").unwrap();
        let value=v8::Script::compile(scope,code,None).unwrap().run(scope).unwrap();
        v8::Global::new(scope,value)
    }).unwrap();
    unload_plugin("captured");
    scalar_owner("captured", |_| {});
    with_host_isolate(|isolate| {
        let mut storage = v8::HandleScope::new(isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let context = clone_plugin_context("captured").unwrap();
        let context = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, context);
        let value = v8::Local::new(scope, &stale);
        let key = v8::String::new(scope, "stale").unwrap();
        context.global(scope).set(scope, key.into(), value);
    })
    .unwrap();
    js(
        "captured",
        "if(stale()!==5 || __s2pkg_unsafe.Engine.function('fire').call(8)!==8) throw Error('generation fence');",
    );
    drop(stale);
    finish();
}

#[test]
fn public_entity_calls_hooks_and_serial_gating_use_the_shared_projection_codec() {
    function_adapter::entity_transport_tests::init_public_transport();
    let a = function_adapter::entity_transport_tests::seed_public_entity(901, 71);
    let b = function_adapter::entity_transport_tests::seed_public_entity(902, 72);
    for id in ["entity-public", "entity-public-caller"] {
        frame_tests::load_body(id, "return {};", "{}");
        function_adapter::proof::entity_binding(id, true, true, false);
        js(
            id,
            &format!(
                "globalThis.a=new __s2pkg_entity.EntityRef(901,{a});globalThis.b=new __s2pkg_entity.EntityRef(902,{b});globalThis.f=__s2pkg_unsafe.Engine.function('fire');"
            ),
        );
    }
    js(
        "entity-public",
        "globalThis.seen=[];globalThis.pre=f.onPre(v=>{seen.push(v.optional.id);v.optional=b;return 1;});globalThis.post=f.onPost(v=>seen.push(v.returnValue.id));",
    );
    js(
        "entity-public-caller",
        &format!(
            "const out=f.call(a);if(out.id!=={b} || out.index!==902 || !(out instanceof __s2pkg_entity.EntityRef))throw Error('entity call');"
        ),
    );
    js(
        "entity-public",
        &format!(
            "if(JSON.stringify(seen)!==JSON.stringify([{a},{b}]))throw Error(JSON.stringify(seen));pre.dispose();post.dispose();if(f.call(null)!==null)throw Error('nullable');"
        ),
    );
    crate::entity_live::on_deleted(901, 71);
    js(
        "entity-public-caller",
        "if(f.call(a)!==null)throw Error('stale nullable identity'); let bad=false;try{f.call({index:902,id:b.id})}catch(_){bad=true}if(!bad)throw Error('forged entity');",
    );
    crate::entity_live::on_deleted(902, 72);
    finish();
}

#[test]
fn activation_failure_releases_exact_staged_ledger_entries_before_partial_unload() {
    init(frame_tests::dummy_logger()).unwrap();
    let generation = create_plugin_context("@demo/fire");
    let owner = contract::OwnerKey::plugin("@demo/fire", generation);
    let before = REGISTRY.with(|r| r.borrow().active_resource_count(&owner.id, generation));
    let mut value = tests::fixture();
    let mut other = value["functions"][0].clone();
    other["localName"] = "other".into();
    other["canonicalId"] = "@demo/fire::other".into();
    value["functions"].as_array_mut().unwrap().push(other);
    tests::seal(&mut value);
    let parsed = contract::parse(
        &value.to_string(),
        &owner.id,
        &tests::summary(&value),
        &["engine:calls".into()],
    )
    .unwrap();
    let candidate = overrides::prepare(parsed, "ledger-test", vec![]).unwrap();
    let receipt = registry::prepare_owner(&owner.id, candidate).unwrap();
    registry::ACTIVATION_LEDGER_LIMIT.with(|limit| limit.set(Some(1)));
    assert!(registry::activate_owner(receipt, owner.clone()).is_err());
    assert!(owner_is_live(&owner.id, generation));
    assert_eq!(
        REGISTRY.with(|r| r.borrow().active_resource_count(&owner.id, generation)),
        before
    );
    assert!(registry::owner_bindings(&owner).is_empty());
    shutdown();
}
