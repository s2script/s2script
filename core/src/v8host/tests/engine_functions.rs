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

#[test]
fn package_lookup_is_bootstrap_scoped() {
    function_adapter::scalar_transport_tests::init_transport();
    let package = function_adapter::register_prepared_package(
        contract::HostPackageOwner::mint("@proof/package-functions").unwrap(),
        "globalThis.savedLookup=__s2_package_function;".into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    frame_tests::load_body("package-bootstrap", "return {};", "{}");
    let result = eval_in_context(
        "package-bootstrap",
        "if(typeof savedLookup!=='function')throw Error('missing');",
    );
    drop(package);
    finish();
    assert!(
        result.is_ok(),
        "private bootstrap lookup missing: {result:?}"
    );
}

pub(super) fn package_candidate(
    id: &str,
    empty: bool,
) -> crate::engine_functions::provenance::PreparedCandidate {
    package_candidate_config(id, empty, |_| {})
}
fn package_candidate_config(
    id: &str,
    empty: bool,
    configure: impl FnOnce(&mut serde_json::Value),
) -> crate::engine_functions::provenance::PreparedCandidate {
    let mut value = tests::fixture();
    value["ownerId"] = id.into();
    let f = &mut value["functions"][0];
    f["canonicalId"] = format!("{id}::fire").into();
    f["requirement"] = "required".into();
    f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:i32(i32)".into();
    f["abi"]["parameters"] = serde_json::json!([{"name":"x","native":"i32","projection":{"id":"i32","version":1},"mutable":[]}]);
    f["abi"]["returns"] = serde_json::json!({"native":"i32","projection":{"id":"i32","version":1}});
    f["policy"]["surfaces"] = serde_json::json!(["call", "pre", "post"]);
    f["policy"]["suppression"] = "generic".into();
    if empty {
        value["functions"] = serde_json::json!([]);
    }
    configure(&mut value);
    tests::seal(&mut value);
    let mut summary = tests::summary(&value);
    if !empty {
        for f in summary["functions"].as_array_mut().unwrap() {
            f["suppresses"] = true.into();
        }
    }
    let parsed = contract::parse(
        &value.to_string(),
        id,
        &summary,
        &["engine:calls".into(), "engine:hooks".into()],
    )
    .unwrap();
    overrides::prepare(parsed, "package-archive", vec![]).unwrap()
}

#[test]
fn package_functions_seal_exact_host_and_retire_without_a_plugin_entry() {
    function_adapter::scalar_transport_tests::init_transport();
    let host = contract::HostPackageOwner::mint("@proof/owner").unwrap();
    let wrong = contract::HostPackageOwner::mint("@proof/owner").unwrap();
    let prepared =
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap();
    assert!(registry::activate_package_owner(prepared, &wrong).is_err());
    assert!(registry::owner_bindings(host.key()).is_empty());
    let prepared =
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap();
    let active = registry::activate_package_owner(prepared, &host).unwrap();
    let binding = registry::named_binding(host.key(), "fire").unwrap();
    assert_eq!(binding.provenance.archive_hash, "package-archive");
    assert!(!owner_is_live(&host.key().id, host.key().generation));
    assert!(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).is_err()
    );
    drop(active);
    assert!(registry::binding(binding.id, host.key()).is_err());
    assert!(registry::owner_bindings(host.key()).is_empty());
    assert!(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).is_err()
    );
    drop(binding);
    for empty in [true, false] {
        let host = contract::HostPackageOwner::mint("@proof/empty").unwrap();
        let first =
            registry::prepare_package_owner(&host, package_candidate(&host.key().id, empty))
                .unwrap();
        let second =
            registry::prepare_package_owner(&host, package_candidate(&host.key().id, empty))
                .unwrap();
        let active = registry::activate_package_owner(first, &host).unwrap();
        assert!(registry::activate_package_owner(second, &host).is_err());
        drop(active);
    }
    finish();
}

#[test]
fn package_functions_two_parents_without_declarations_share_calls_and_retire_generic_subscriptions()
{
    function_adapter::scalar_transport_tests::init_transport();
    let host = contract::HostPackageOwner::mint("@proof/shared-functions").unwrap();
    let prepared =
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap();
    let active = registry::activate_package_owner(prepared, &host).unwrap();
    let package = function_adapter::register_prepared_package(
        host.clone(),
        r#"
        globalThis.events=[];
        globalThis.factory=__s2_package_function;
        globalThis.f=factory('fire');
        globalThis.pre=f.onPre(v=>{events.push('pre:'+v.x);});
        globalThis.post=f.onPost(v=>{events.push('post:'+v.returnValue);});
    "#
        .into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    for id in ["package-a", "package-b"] {
        frame_tests::load_body(id, "return {};", "{}");
        assert!(
            registry::owner_bindings(&contract::OwnerKey::plugin(id, plugin_generation(id)))
                .is_empty()
        );
        js(id, "let refused=0;for(const op of [()=>factory('fire'),()=>__s2pkg_unsafe.Engine.function('fire'),()=>__s2pkg_unsafe.Engine.function('@proof/shared-functions::fire')]){try{op()}catch(_){refused++}}if(refused!==3)throw Error('authority leak');");
    }
    js(
        "package-a",
        "if(f.call(8)!==8 || events.length)throw Error('caller bypass');",
    );
    js(
        "package-b",
        "if(JSON.stringify(events)!=='[\"pre:8\",\"post:8\"]')throw Error(JSON.stringify(events));",
    );
    unload_plugin("package-a");
    js("package-b", "if(f.call(9)!==9)throw Error('peer died');");
    frame_tests::load_body("package-a", "return {};", "{}");
    js("package-a", "if(f.call(10)!==10)throw Error('reload');");
    drop(active);
    for id in ["package-a", "package-b"] {
        js(id,"let n=0;for(const op of [()=>f.call(1),()=>f.available,()=>f.status,()=>f.onPre(()=>{})]){try{op()}catch(_){n++}}if(n!==4)throw Error('retired capability');");
    }
    assert!(registry::owner_bindings(host.key()).is_empty());
    drop(package);
    finish();
}

fn copy_global(from: &str, to: &str, name: &str, target: &str) {
    with_host_isolate(|isolate| {
        let mut storage = v8::HandleScope::new(isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let source = clone_plugin_context(from).unwrap();
        let source = v8::Local::new(&mut hs, &source);
        let value = {
            let scope = &mut v8::ContextScope::new(&mut hs, source);
            let key = v8::String::new(scope, name).unwrap();
            source.global(scope).get(scope, key.into()).unwrap()
        };
        let destination = clone_plugin_context(to).unwrap();
        let destination = v8::Local::new(&mut hs, &destination);
        let scope = &mut v8::ContextScope::new(&mut hs, destination);
        let key = v8::String::new(scope, target).unwrap();
        destination.global(scope).set(scope, key.into(), value);
    })
    .unwrap();
}

#[test]
fn package_functions_foreign_context_cannot_reuse_a_live_native_facade() {
    function_adapter::scalar_transport_tests::init_transport();
    let host = contract::HostPackageOwner::mint("@proof/context").unwrap();
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap(),
        &host,
    )
    .unwrap();
    let package = function_adapter::register_prepared_package(
        host,
        r#"globalThis.f=__s2_package_function('fire');globalThis.direct=f.call;"#.into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    for id in ["context-a", "context-b"] {
        frame_tests::load_body(id, "return {};", "{}");
    }
    copy_global("context-a", "context-b", "direct", "foreign");
    let result=eval_in_context("context-b","let rejected=false;try{foreign(3)}catch(_){rejected=true}if(!rejected)throw Error('foreign native accepted');");
    unload_plugin("context-a");
    frame_tests::load_body("context-a", "return {};", "{}");
    let stale=eval_in_context("context-b","let staleRejected=false;try{foreign(4)}catch(_){staleRejected=true}if(!staleRejected)throw Error('stale package native accepted');");
    js("context-a", "if(f.call(6)!==6)throw Error('replacement package facade');");
    drop(active);
    drop(package);
    finish();
    result.unwrap();
    stale.unwrap();
}

#[test]
fn package_source_retirement_cannot_reactivate_captured_instance_tokens() {
    function_adapter::scalar_transport_tests::init_transport();
    let host = contract::HostPackageOwner::mint("@proof/one-source").unwrap();
    let source: std::sync::Arc<str> = "globalThis.lookup=__s2_package_function;".into();
    let hash = contract::ImplementationManifestHash::new("a".repeat(64)).unwrap();
    let receipt =
        function_adapter::register_prepared_package(host.clone(), source.clone(), hash.clone())
            .unwrap();
    drop(receipt);
    let reopened = function_adapter::register_prepared_package(host, source, hash);
    let refused = reopened.is_err();
    drop(reopened);
    finish();
    assert!(refused, "retired source capability reused");
}

#[test]
fn package_functions_reject_foreign_subscription_callback_before_acquisition() {
    function_adapter::scalar_transport_tests::init_transport();
    let host = contract::HostPackageOwner::mint("@proof/callback-context").unwrap();
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap(),
        &host,
    )
    .unwrap();
    let package = function_adapter::register_prepared_package(
        host,
        "globalThis.f=__s2_package_function('fire');globalThis.wrapper=()=>{};".into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    for id in ["callback-a", "callback-b"] {
        frame_tests::load_body(id, "return {};", "{}");
    }
    copy_global("callback-a", "callback-b", "wrapper", "foreign");
    let result=eval_in_context("callback-b","let n=0;try{f.onPre(foreign)}catch(_){n++}if(n!==1)throw Error('foreign callback admitted');");
    drop(active);
    drop(package);
    finish();
    result.unwrap();
}

thread_local! {
    static RETIRING: std::cell::RefCell<Option<registry::ActivePackageFunctions>> = const { std::cell::RefCell::new(None) };
    static TARGET_RELEASES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static HOOK_RELEASES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
extern "C" fn count_target_release(_: i64) -> i32 {
    TARGET_RELEASES.with(|n| n.set(n.get() + 1));
    1
}
extern "C" fn count_hook_release(_: i64) -> i32 {
    HOOK_RELEASES.with(|n| n.set(n.get() + 1));
    1
}
fn retire_from_callback(
    _: &mut v8::PinScope,
    _: v8::FunctionCallbackArguments,
    _: v8::ReturnValue,
) {
    let active = RETIRING.with(|r| r.borrow_mut().take());
    drop(active);
    assert_eq!(
        TARGET_RELEASES.with(|n| n.get()),
        0,
        "target freed while outbound frame holds it"
    );
}
fn install_retirement_native(id: &str) {
    with_host_isolate(|isolate| {
        let mut storage = v8::HandleScope::new(isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let context = clone_plugin_context(id).unwrap();
        let context = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, context);
        let function = v8::Function::new(scope, retire_from_callback).unwrap();
        let key = v8::String::new(scope, "retirePackage").unwrap();
        context
            .global(scope)
            .set(scope, key.into(), function.into());
    })
    .unwrap();
}

#[test]
fn package_functions_retirement_revokes_live_views_and_defers_target_release_until_unwind() {
    function_adapter::scalar_transport_tests::init_transport();
    TARGET_RELEASES.with(|n| n.set(0));
    HOOK_RELEASES.with(|n| n.set(0));
    let mut ops = engine_ops().unwrap();
    ops.function_target_release = Some(count_target_release);
    ops.function_hook_release = Some(count_hook_release);
    set_engine_ops(Some(ops));
    let host = contract::HostPackageOwner::mint("@proof/retire-in-frame").unwrap();
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap(),
        &host,
    )
    .unwrap();
    RETIRING.with(|r| *r.borrow_mut() = Some(active));
    let package = function_adapter::register_prepared_package(
        host,
        r#"
        globalThis.f=__s2_package_function('fire');
        globalThis.refused=false;
        f.onPre(v=>{retirePackage();try{v.x}catch(_){refused=true}});
    "#
        .into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    for id in ["retire-a", "retire-b"] {
        frame_tests::load_body(id, "return {};", "{}");
        install_retirement_native(id);
    }
    js(
        "retire-a",
        "if(f.call(7)!==7)throw Error('held call failed');",
    );
    let result = eval_in_context(
        "retire-b",
        "if(!refused)throw Error('retired callback view remained authorized');",
    );
    assert_eq!(TARGET_RELEASES.with(|n| n.get()), 1);
    assert_eq!(HOOK_RELEASES.with(|n| n.get()), 2);
    for id in ["retire-a", "retire-b"] {
        assert_eq!(
            function_adapter::proof::counts(id, plugin_generation(id)),
            (0, 0)
        );
    }
    assert_eq!(function_adapter::proof::pending_invocations(), 0);
    drop(package);
    finish();
    result.unwrap();
}

#[test]
fn package_functions_bootstrap_failure_revokes_captured_facades_and_partial_subscriptions() {
    for ending in [
        "throw Error('bootstrap failure');",
        "f.onPost(async()=>{});",
    ] {
        function_adapter::scalar_transport_tests::init_transport();
        let host = contract::HostPackageOwner::mint("@proof/failed-bootstrap").unwrap();
        let active = registry::activate_package_owner(
            registry::prepare_package_owner(&host, package_candidate(&host.key().id, false))
                .unwrap(),
            &host,
        )
        .unwrap();
        let package = function_adapter::register_prepared_package(
            host.clone(),
            format!("globalThis.f=__s2_package_function('fire');f.onPre(()=>{{}});{ending}").into(),
            contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
        )
        .unwrap();
        let generation = create_plugin_context("failed-bootstrap");
        assert!(is_failed("failed-bootstrap"));
        assert_eq!(
            function_adapter::proof::counts("failed-bootstrap", generation),
            (0, 0)
        );
        js("failed-bootstrap","let n=0;for(const op of [()=>f.call(1),()=>f.status,()=>f.onPre(()=>{})]){try{op()}catch(_){n++}}if(n!==3)throw Error('failed instance still authorized');");
        assert!(
            registry::named_binding(host.key(), "fire").is_ok(),
            "bootstrap failure retired process binding"
        );
        unload_plugin("failed-bootstrap");
        drop(active);
        drop(package);
        finish();
    }
}

#[test]
fn package_functions_colliding_generation_never_becomes_the_native_caller() {
    function_adapter::scalar_transport_tests::init_transport();
    let mut host = contract::HostPackageOwner::mint("@proof/collision").unwrap();
    frame_tests::load_body("collision-parent", "return {};", "{}");
    let mut parent = plugin_generation("collision-parent");
    while host.key().generation != parent {
        if host.key().generation < parent {
            host = contract::HostPackageOwner::mint("@proof/collision").unwrap();
        } else {
            unload_plugin("collision-parent");
            frame_tests::load_body("collision-parent", "return {};", "{}");
            parent = plugin_generation("collision-parent");
        }
    }
    // This live plugin has no bindings. A generation-based scan cannot recover it.
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap(),
        &host,
    )
    .unwrap();
    let package=function_adapter::register_prepared_package(host.clone(),"globalThis.f=__s2_package_function('fire');globalThis.events=[];f.onPre(v=>{events.push(v.x);if(v.x===5 && f.call(3)!==3)throw Error('nested');});f.onPost(v=>events.push(v.returnValue));".into(),contract::ImplementationManifestHash::new("a".repeat(64)).unwrap()).unwrap();
    // Bootstrap exact existing parent to preserve the deliberately equal numbers.
    with_host_isolate(|isolate| {
        let mut storage = v8::HandleScope::new(isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let context = clone_plugin_context("collision-parent").unwrap();
        let context = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, context);
        function_adapter::bootstrap(scope, "collision-parent", parent).unwrap();
    })
    .unwrap();
    frame_tests::load_body("collision-peer", "return {};", "{}");
    js(
        "collision-parent",
        "if(f.call(5)!==5 || events.length)throw Error('parent busy/bypass');",
    );
    js(
        "collision-peer",
        "if(JSON.stringify(events)!=='[5,5]')throw Error(JSON.stringify(events));",
    );
    js("collision-peer", "events.length=0;if(f.call(5)!==5 || events.length)throw Error('peer bypass');");
    js("collision-parent", "if(JSON.stringify(events)!=='[5,5]')throw Error('package generation suppressed wrong parent');");
    assert_eq!(host.key().generation, parent);
    let binding = registry::named_binding(host.key(), "fire").unwrap();
    assert!(
        crate::engine_functions::runtime::call(binding.target.unwrap(), Some(host.key()), &[])
            .is_err()
    );
    drop(binding);
    assert!(
        registry::owner_bindings(&contract::OwnerKey::plugin("collision-parent", parent))
            .is_empty()
    );
    drop(active);
    drop(package);
    finish();
}

#[test]
fn package_functions_named_post_needs_exact_association_and_host_grant() {
    use crate::engine_functions::policy::{AdapterContract, HostAdapterGrant};
    for grant in [false, true] {
        function_adapter::scalar_transport_tests::init_transport();
        let host = contract::HostPackageOwner::mint("@proof/named-package").unwrap();
        let active = registry::activate_package_owner(
            registry::prepare_package_owner(&host, package_candidate(&host.key().id, false))
                .unwrap(),
            &host,
        )
        .unwrap();
        let binding = registry::named_binding(host.key(), "fire").unwrap();
        let semantic = "proof.package-post.v1";
        let next_hash = "c".repeat(64);
        let hash = if grant {
            next_hash.as_str()
        } else {
            function_adapter::proof::HASH
        };
        let grants = if grant {
            vec![HostAdapterGrant::override_return(
                &host,
                AdapterContract {
                    id: semantic.into(),
                    version: 1,
                    contract_hash: hash.into(),
                },
            )
            .unwrap()]
        } else {
            vec![]
        };
        let source = format!(
            r#"
            globalThis.f=__s2_package_function('fire');globalThis.events=[];
            const subscribe=__s2_function_adapter_subscribe,register=__s2_function_adapter_register;
            register('{semantic}','{hash}',{{post(d){{
                events.push('overrideReturn' in d.frame);
                if('overrideReturn' in d.frame)d.frame.overrideReturn(41);
                while(d.cursor.invokeNext()!==null){{}}
            }}}});
            globalThis.named=()=>subscribe('fire','{semantic}','post',v=>events.push(v.returnValue));
            globalThis.late=()=>register('late','{hash}',{{pre(){{}}}});
            globalThis.forged=()=>subscribe(999999n,'{semantic}','post',()=>{{}});
        "#
        );
        let package = function_adapter::register_prepared_package_with_authorities(
            host.clone(),
            source.into(),
            contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
            grants,
        )
        .unwrap();
        for id in ["named-a", "named-b"] {
            frame_tests::load_body(id, "return {};", "{}");
        }
        js("named-b","let n=0;for(const op of [named,late,forged]){try{op()}catch(_){n++}}if(n!==3)throw Error('ungranted authority');");
        let wrong = contract::HostPackageOwner::mint("@proof/wrong-package").unwrap();
        let wrong_package = function_adapter::register_prepared_package(
            wrong,
            "0;".into(),
            contract::ImplementationManifestHash::new("b".repeat(64)).unwrap(),
        )
        .unwrap();
        assert!(function_adapter::authorize_binding(
            &wrong_package,
            host.key(),
            binding.id,
            semantic,
            hash
        )
        .is_err());
        function_adapter::authorize_binding(
            &package,
            host.key(),
            binding.id,
            semantic,
            &"b".repeat(64),
        )
        .unwrap();
        assert!(eval_in_context("named-b", "named()").is_err());
        function_adapter::authorize_binding(&package, host.key(), binding.id, semantic, hash)
            .unwrap();
        js("named-b", "named();");
        let expected = if grant { 41 } else { 7 };
        js(
            "named-a",
            &format!("if(f.call(7)!=={expected})throw Error('POST authority');"),
        );
        js("named-b",&format!("if(JSON.stringify(events)!=='[{grant},{expected}]')throw Error(JSON.stringify(events));"));
        drop(wrong_package);
        drop(binding);
        drop(active);
        drop(package);
        finish();
    }
}

#[test]
fn package_functions_map_invalidates_entities_without_rebootstrapping_package() {
    function_adapter::entity_transport_tests::init_public_transport();
    let entity = function_adapter::entity_transport_tests::seed_public_entity(901, 71);
    let host = contract::HostPackageOwner::mint("@proof/package-entity").unwrap();
    let candidate = package_candidate_config(&host.key().id, false, |value| {
        let f = &mut value["functions"][0];
        f["target"]["pattern"] = "50".into();
        f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:ptr(ptr)".into();
        f["abi"]["parameters"][0]["native"] = "ptr".into();
        f["abi"]["parameters"][0]["projection"]["id"] = "entity".into();
        f["abi"]["returns"]["native"] = "ptr".into();
        f["abi"]["returns"]["projection"]["id"] = "entity".into();
    });
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, candidate).unwrap(),
        &host,
    )
    .unwrap();
    let package = function_adapter::register_prepared_package(
        host.clone(),
        "globalThis.boots=(globalThis.boots||0)+1;globalThis.f=__s2_package_function('fire');"
            .into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    for id in ["map-a", "map-b"] {
        frame_tests::load_body(id, "return {};", "{}");
        js(id,&format!("globalThis.entity=new __s2pkg_entity.EntityRef(901,{entity});if(f.call(entity).id!=={entity})throw Error('entity call');"));
    }
    crate::entity_live::clear_for_map_transition();
    for id in ["map-a", "map-b"] {
        js(id,"let refused=false;try{f.call(entity)}catch(_){refused=true}if(!refused || boots!==1 || !f.available)throw Error('map lifetime');");
    }
    assert_eq!(active.owner(), host.key());
    assert!(registry::named_binding(host.key(), "fire").is_ok());
    drop(active);
    drop(package);
    finish();
}

pub(super) struct PackageServiceProof {
    host: contract::HostPackageOwner,
    active: registry::ActivePackageFunctions,
    source: function_adapter::PreparedPackageReceipt,
    target: i64,
    receipt: u64,
}
pub(super) fn package_service_begin() -> PackageServiceProof {
    let host = contract::HostPackageOwner::mint("@proof/service-package").unwrap();
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap(),
        &host,
    )
    .unwrap();
    let target = registry::named_binding(host.key(), "fire")
        .unwrap()
        .target
        .unwrap();
    let source=function_adapter::register_prepared_package(host.clone(),r#"
        globalThis.processFunction=__s2_package_function('fire');globalThis.processEvents=[];
        globalThis.processPre=processFunction.onPre(v=>{processEvents.push('pre:'+v.x);if(v.x===5 && processFunction.call(3)!==3)throw Error('nested process call');});
        globalThis.processPost=processFunction.onPost(v=>{processEvents.push('post:'+v.returnValue);});
    "#.into(),contract::ImplementationManifestHash::new("a".repeat(64)).unwrap()).unwrap();
    for id in ["process-a", "process-b"] {
        frame_tests::load_body(id, "return {};", "{}");
    }
    scalar_owner("process-plugin", |_| {});
    let plugin_owner =
        contract::OwnerKey::plugin("process-plugin", plugin_generation("process-plugin"));
    let plugin_binding = registry::named_binding(&plugin_owner, "fire").unwrap();
    assert_eq!(
        plugin_binding.target,
        Some(target),
        "package/plugin split the native physical Service record"
    );
    // The plugin also bootstrapped the source; remove its package subscriptions
    // so this third participant proves ordinary public generic admission.
    js(
        "process-plugin",
        "processPre.dispose();processPost.dispose();",
    );
    js("process-plugin","globalThis.publicFunction=__s2pkg_unsafe.Engine.function('fire');globalThis.publicEvents=[];publicFunction.onPre(v=>{publicEvents.push('pre:'+v.x);});publicFunction.onPost(v=>{publicEvents.push('post:'+v.returnValue);});");
    let incompatible = contract::HostPackageOwner::mint("@proof/incompatible-package").unwrap();
    let candidate = package_candidate_config(&incompatible.key().id, false, |value| {
        let f = &mut value["functions"][0];
        f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:u64(u64)".into();
        f["abi"]["parameters"][0]["native"] = "u64".into();
        f["abi"]["parameters"][0]["projection"]["id"] = "u64".into();
        f["abi"]["returns"]["native"] = "u64".into();
        f["abi"]["returns"]["projection"]["id"] = "u64".into();
    });
    assert!(
        registry::prepare_package_owner(&incompatible, candidate).is_err(),
        "incompatible physical ABI admitted"
    );
    assert!(registry::owner_bindings(incompatible.key()).is_empty());
    PackageServiceProof {
        host,
        active,
        source,
        target,
        receipt: 0,
    }
}
pub(super) fn package_service_ready(state: &mut PackageServiceProof) -> bool {
    let status = crate::engine_functions::runtime::status(state.target).unwrap();
    if status.state != 2 {
        return false;
    }
    state.receipt = status.receipt;
    true
}
pub(super) fn package_service_exercise(state: &PackageServiceProof) {
    for id in ["process-a", "process-b"] {
        js(id, "processEvents.length=0;");
    }
    js("process-plugin", "publicEvents.length=0;");
    js("process-a","if(processFunction.call(5)!==5 || processEvents.length)throw Error('process parent bypass');");
    js("process-b","if(JSON.stringify(processEvents)!=='[\"pre:5\",\"post:5\"]')throw Error(JSON.stringify(processEvents));");
    js("process-plugin","if(JSON.stringify(publicEvents)!=='[\"pre:3\",\"post:3\",\"pre:5\",\"post:5\"]')throw Error(JSON.stringify(publicEvents));");
    let old = plugin_generation("process-a");
    unload_plugin("process-a");
    assert_eq!(function_adapter::proof::counts("process-a", old), (0, 0));
    js(
        "process-b",
        "if(processFunction.call(7)!==7)throw Error('peer invalidated');",
    );
    frame_tests::load_body("process-a", "return {};", "{}");
    assert_ne!(old, plugin_generation("process-a"));
    js(
        "process-a",
        "if(processFunction.call(9)!==9)throw Error('reloaded parent');",
    );
    assert_eq!(
        crate::engine_functions::runtime::status(state.target)
            .unwrap()
            .receipt,
        state.receipt,
        "reload replaced shared physical registration"
    );
    assert!(registry::named_binding(state.host.key(), "fire").is_ok());
}
pub(super) fn package_service_finish(state: PackageServiceProof) {
    drop(state.active);
    for id in ["process-a", "process-b"] {
        assert_eq!(
            function_adapter::proof::counts(id, plugin_generation(id)).1,
            0
        );
        js(id,"let n=0;try{processFunction.call(1)}catch(_){n++}if(n!==1)throw Error('process retirement');");
    }
    js(
        "process-plugin",
        "if(publicFunction.call(11)!==11)throw Error('public peer retired');",
    );
    drop(state.source);
    for id in ["process-a", "process-b", "process-plugin"] {
        unload_plugin(id);
    }
    assert_eq!(function_adapter::proof::pending_invocations(), 0);
    println!("PASS exact process package owner: shared native Service/registration with Plugin, incompatible ABI rejection, two parents without declarations, nested peer fanout/bypass, reload, independent retirement");
}

thread_local! { static PREPARATIONS:std::cell::Cell<usize>=const{std::cell::Cell::new(0)}; }
extern "C" fn fail_second_prepare(
    _: *const i8,
    _: *const i8,
    _: *const i8,
    _: *const i8,
    why: *mut i8,
    cap: i32,
) -> i64 {
    let n = PREPARATIONS.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    if n == 2 {
        let message = b"fixture target unavailable\0";
        if cap >= message.len() as i32 {
            unsafe {
                std::ptr::copy_nonoverlapping(message.as_ptr(), why.cast(), message.len());
            }
        }
        0
    } else {
        1
    }
}
#[test]
fn package_functions_preparation_failures_and_retained_leases_release_without_publication() {
    function_adapter::scalar_transport_tests::init_transport();
    let mut ops = engine_ops().unwrap();
    ops.function_prepare = Some(fail_second_prepare);
    ops.function_target_release = Some(count_target_release);
    set_engine_ops(Some(ops));
    PREPARATIONS.with(|n| n.set(0));
    TARGET_RELEASES.with(|n| n.set(0));
    let host = contract::HostPackageOwner::mint("@proof/rollback").unwrap();
    let candidate = package_candidate_config(&host.key().id, false, |value| {
        let mut second = value["functions"][0].clone();
        second["localName"] = "second".into();
        second["canonicalId"] = "@proof/rollback::second".into();
        value["functions"].as_array_mut().unwrap().push(second);
    });
    assert!(registry::prepare_package_owner(&host, candidate).is_err());
    assert_eq!(TARGET_RELEASES.with(|n| n.get()), 1);
    assert!(registry::owner_bindings(host.key()).is_empty());
    let lease = std::rc::Rc::new(String::from("retained loader lease"));
    let weak = std::rc::Rc::downgrade(&lease);
    let mut prepared =
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap();
    assert!(prepared.retained_bytes() > 0);
    prepared.retain(lease);
    drop(prepared);
    assert!(weak.upgrade().is_none());
    assert_eq!(TARGET_RELEASES.with(|n| n.get()), 2);
    let lease = std::rc::Rc::new(String::from("active lease"));
    let weak = std::rc::Rc::downgrade(&lease);
    let mut prepared =
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap();
    prepared.retain(lease);
    let active = registry::activate_package_owner(prepared, &host).unwrap();
    let hold = registry::named_binding(host.key(), "fire").unwrap();
    drop(active);
    assert!(weak.upgrade().is_some());
    assert_eq!(TARGET_RELEASES.with(|n| n.get()), 2);
    drop(hold);
    assert!(weak.upgrade().is_none());
    assert_eq!(TARGET_RELEASES.with(|n| n.get()), 3);
    let optional = contract::HostPackageOwner::mint("@proof/optional-package").unwrap();
    PREPARATIONS.with(|n| n.set(1));
    let candidate = package_candidate_config(&optional.key().id, false, |v| {
        v["functions"][0]["requirement"] = "optional".into()
    });
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&optional, candidate).unwrap(),
        &optional,
    )
    .unwrap();
    let binding = registry::named_binding(optional.key(), "fire").unwrap();
    assert!(binding.target.is_none());
    assert!(binding
        .unavailable
        .as_deref()
        .unwrap()
        .contains("fixture target unavailable"));
    assert_eq!(binding.provenance.archive_hash, "package-archive");
    drop(binding);
    drop(active);
    finish();
}

#[test]
fn package_functions_nested_callback_queues_real_loader_unload_until_frame_boundary() {
    use std::io::Write;
    function_adapter::scalar_transport_tests::init_transport();
    let host = contract::HostPackageOwner::mint("@proof/queued-unload").unwrap();
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap(),
        &host,
    )
    .unwrap();
    let package = function_adapter::register_prepared_package(
        host.clone(),
        r#"
        globalThis.f=__s2_package_function('fire');globalThis.events=[];
        f.onPre(v=>{events.push('pre');if(v.x===5){
            if(!__s2_plugin_unload('queued-b'))throw Error('unload not queued');
            if(f.call(3)!==3)throw Error('nested call after queued unload');
            events.push('held');
        }});f.onPost(v=>events.push('post'));
    "#
        .into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    let root = std::env::temp_dir().join(format!(
        "s2-package-unload-{}-{}",
        std::process::id(),
        host.key().generation
    ));
    std::fs::create_dir_all(&root).unwrap();
    for id in ["queued-a", "queued-b"] {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in [
            (
                "manifest.json",
                serde_json::json!({"id":id,"version":"1.0.0","apiVersion":"3.x"}).to_string(),
            ),
            ("plugin.js", "module.exports.OnPluginStart=()=>{};".into()),
        ] {
            writer
                .start_file(name, zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        std::fs::write(
            root.join(format!("{id}.s2sp")),
            writer.finish().unwrap().into_inner(),
        )
        .unwrap();
    }
    crate::loader::set_plugins_dir(root.to_str().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while plugin_phase("queued-a") != Some(plugin::Phase::Active)
        || plugin_phase("queued-b") != Some(plugin::Phase::Active)
    {
        crate::loader::poll_plugins();
        assert!(
            std::time::Instant::now() < deadline,
            "loader did not activate package parents: {:?}",
            frame_tests::LOG.lock().unwrap()
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let generation = plugin_generation("queued-b");
    js("queued-a", "if(f.call(5)!==5)throw Error('outer call');");
    js("queued-b","if(JSON.stringify(events)!=='[\"pre\",\"held\",\"post\"]')throw Error(JSON.stringify(events));");
    assert!(
        owner_is_live("queued-b", generation),
        "queued unload freed the callback context"
    );
    crate::loader::poll_plugins();
    assert!(!owner_is_live("queued-b", generation));
    assert_eq!(
        function_adapter::proof::counts("queued-b", generation),
        (0, 0)
    );
    assert_eq!(function_adapter::proof::pending_invocations(), 0);
    js(
        "queued-a",
        "if(f.call(9)!==9)throw Error('peer/process invalidated');",
    );
    assert!(registry::named_binding(host.key(), "fire").is_ok());
    crate::loader::shutdown_worker();
    drop(active);
    drop(package);
    finish();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_functions_register_refuses_foreign_adapter_callback() {
    function_adapter::scalar_transport_tests::init_transport();
    for id in ["register-a", "register-b"] {
        frame_tests::load_body(id, "return {};", "{}");
    }
    js("register-a", "globalThis.foreign=()=>{};");
    copy_global("register-a", "register-b", "foreign", "foreign");
    let host = contract::HostPackageOwner::mint("@proof/foreign-adapter").unwrap();
    let package = function_adapter::register_prepared_package(
        host,
        format!(
            "__s2_function_adapter_register('proof.foreign.v1','{}',{{pre:foreign}});",
            function_adapter::proof::HASH
        )
        .into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    let result = with_host_isolate(|isolate| {
        let mut storage = v8::HandleScope::new(isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let context = clone_plugin_context("register-b").unwrap();
        let context = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, context);
        function_adapter::bootstrap(scope, "register-b", plugin_generation("register-b"))
    })
    .unwrap();
    drop(package);
    finish();
    assert!(result.is_err(), "foreign adapter callback admitted");
}

#[test]
fn package_functions_retirement_requires_full_kind_id_generation() {
    function_adapter::scalar_transport_tests::init_transport();
    let host = contract::HostPackageOwner::mint("@proof/exact-retire").unwrap();
    let active = registry::activate_package_owner(
        registry::prepare_package_owner(&host, package_candidate(&host.key().id, false)).unwrap(),
        &host,
    )
    .unwrap();
    let source = function_adapter::register_prepared_package(
        host.clone(),
        "globalThis.f=__s2_package_function('fire');".into(),
        contract::ImplementationManifestHash::new("a".repeat(64)).unwrap(),
    )
    .unwrap();
    frame_tests::load_body("exact-retire", "return {};", "{}");
    function_adapter::drop_package(&contract::OwnerKey::plugin(
        &host.key().id,
        host.key().generation,
    ));
    let result = eval_in_context(
        "exact-retire",
        "if(f.call(2)!==2)throw Error('wrong-kind retirement');",
    );
    let mut wrong_id = host.key().clone();
    wrong_id.id = "@proof/not-this-package".into();
    function_adapter::drop_package(&wrong_id);
    let second = eval_in_context(
        "exact-retire",
        "if(f.call(3)!==3)throw Error('wrong-id retirement');",
    );
    drop(active);
    drop(source);
    finish();
    result.unwrap();
    second.unwrap();
}
