//! Protocol 2 interface facades (`interop_facade.rs` and the prelude's facade builder).
//!
//! The strict-copy semantics are covered by running the whole protocol 2 suite with
//! `S2_TEST_FACADE=1`, which routes consumer-side `__s2_iface_call`s through facades. These tests
//! cover what only a facade has: binding, revocation, its private natives, and tampering with the
//! intrinsics it captured.
use super::*;

/// Producer and consumers as in `protocol2_setup`, with a facade for the consumer at `__c`.
fn facade_setup() {
    protocol2_setup();
    eval_in_context("cons", "globalThis.__c = __s2_require('@x/counter')").unwrap();
}

fn republish(value: &str) {
    let mut contract = None;
    PLUGIN_INTEROP.with(|p| contract = p.borrow().get("cons").and_then(|m| m.get("@x/counter")).cloned());
    let contract = contract.unwrap().as_ref().clone();
    set_plugin_publishes("prod", [("@x/counter".into(), crate::loader::PublishDecl {
        version: "1.0.0".into(), types_sha256: "a".repeat(64), contract: Some(contract),
    })].into_iter().collect());
    create_plugin_context("prod");
    eval_in_context("prod", &format!("__s2_iface_publish('@x/counter',{{getCount:function(){{return {value};}}}})")).unwrap();
}

#[test]
fn facade_is_a_frozen_object_with_one_function_per_contract_method() {
    facade_setup();
    assert!(eval_in_context_bool("cons", "Object.isFrozen(__c) && typeof __c.getCount === 'function' && typeof __c.on === 'function'"));
    assert_eq!(eval_in_context_string("cons", "Object.keys(__c).sort().join()"), "getCount,off,on");
    assert_eq!(eval_in_context_string("cons", "String(__c.getCount())"), "1");
    shutdown();
}

#[test]
fn facade_private_natives_are_gone_before_plugin_code_runs() {
    facade_setup();
    for name in interop_facade::BOOTSTRAP_ONLY.iter().chain(["__s2_interop_facade"].iter()) {
        assert_eq!(eval_in_context_string("cons", &format!("typeof globalThis.{name}")), "undefined", "{name}");
    }
    shutdown();
}

/// A method taken off the facade stops working when its producer unloads, and recovers when the
/// interface is published again, as a hard dependency does.
#[test]
fn facade_methods_are_revoked_with_their_producer_and_rebind_to_its_successor() {
    facade_setup();
    eval_in_context("cons", "globalThis.f = __c.getCount").unwrap();
    assert_eq!(eval_in_context_string("cons", "String(f())"), "1");
    assert_eq!(interop_facade::tracked_cells(), 1);
    unload_plugin("prod");
    // The sweep at teardown released the stale cell's references to the old producer.
    assert_eq!(interop_facade::tracked_cells(), 0);
    let outcome = eval_in_context_string("cons", "(()=>{try{return 'returned:'+f()}catch(e){return e.message}})()");
    assert!(outcome.starts_with("InterfaceUnavailable"), "{outcome}");
    republish("2");
    assert_eq!(eval_in_context_string("cons", "String(f())"), "2");
    shutdown();
}

/// A plugin that replaces the intrinsics the copy uses, in its own realm, after the facade was
/// built, changes nothing: no hook runs, and strict refusals still hold.
#[test]
fn facade_copies_use_captured_intrinsics() {
    let shape: crate::interop::Schema = serde_json::from_value(serde_json::json!({"kind":"object","fields":{
        "items":{"schema":{"kind":"array","item":{"kind":"number"}},"optional":false},
        "name":{"schema":{"kind":"string"},"optional":true}
    }})).unwrap();
    protocol2_setup();
    let mut contract = published_contract("@x/counter").unwrap().as_ref().clone();
    contract.metadata.methods.insert("getCount".into(), crate::interop::Method {
        args: vec![crate::interop::Field { schema: shape.clone(), optional: false }], result: shape,
    });
    final_review_install_contract(contract);
    eval_in_context("prod", "__s2_iface_publish('@x/counter',{getCount:p=>p})").unwrap();
    eval_in_context("cons", "globalThis.__c = __s2_require('@x/counter')").unwrap();
    for owner in ["prod", "cons"] {
        eval_in_context(owner, r#"
          globalThis.hits=0;globalThis.gd=Object.getOwnPropertyDescriptor;
          const hit=()=>{hits++;throw Error('hooked')};
          for (const [o,k] of [[Reflect,'ownKeys'],[Reflect,'apply'],[Object,'getOwnPropertyDescriptor'],[Object,'getPrototypeOf'],
                               [Object,'hasOwn'],[Object,'create'],[Object,'freeze'],[Array,'isArray'],[Array,'from'],[JSON,'parse'],
                               [String.prototype,'isWellFormed'],[Function.prototype,'call'],[Array.prototype,'slice']]) o[k]=hit;
          for (const k of ['0','1','items','name','length']) {
            Object.defineProperty(Object.prototype,k,{get:hit,set:hit,configurable:true});
          }
          Object.defineProperty(Array.prototype,'0',{get:hit,set:hit,configurable:true});
        "#).unwrap();
    }
    assert!(eval_in_context_bool("cons", r#"(()=>{
      const q=__c.getCount({items:[1,2],name:'x'});
      return gd(q,'name').value==='x'&&gd(gd(q,'items').value,'1').value===2;
    })()"#));
    for refused in [
        "new Proxy({items:[1]},{})",
        "{items:new Proxy([1],{})}",
        "Object.defineProperty({},'items',{get(){return [1]},enumerable:true})",
        "{items:[1],extra:1}",
        "{items:['\\ud800']}",
        "{items:[1],name:undefined}",
        "{items:[Infinity]}",
    ] {
        let outcome = eval_in_context_string("cons", &format!("(()=>{{try{{__c.getCount({refused});return 'accepted'}}catch(e){{return e.message}}}})()"));
        assert!(outcome.starts_with("InterfaceValueNotSerializable"), "{refused}: {outcome}");
    }
    for owner in ["prod", "cons"] {
        assert_eq!(eval_in_context_string(owner, "String(hits)"), "0", "{owner}");
    }
    shutdown();
}

/// Facade calls count against the same 32-call limit as native calls, and unwind it after errors.
#[test]
fn facade_calls_share_the_interop_depth_limit() {
    facade_setup();
    // The producer imports its own interface, so it can recurse through a facade of its own.
    let contract = published_contract("@x/counter").unwrap().as_ref().clone();
    set_plugin_imports("prod", vec![crate::interfaces::ImportSpec {
        name: "@x/counter".into(), range: "^1.0.0".into(), kind: crate::interfaces::Kind::Hard,
        compiled_types_sha256: Some("a".repeat(64)),
    }]);
    set_plugin_interop("prod", [("@x/counter".into(), contract)].into_iter().collect());
    eval_in_context("prod", r#"globalThis.self=__s2_require("@x/counter");globalThis.left=31;
      __s2_iface_publish("@x/counter",{getCount:function(){return left-->0?self.getCount():7;}});"#).unwrap();
    assert_eq!(eval_in_context_string("cons", "String(__c.getCount())"), "7");
    eval_in_context("prod", "left=32").unwrap();
    assert!(eval_in_context("cons", "__c.getCount()").unwrap_err().contains("InterfaceRecursionLimit"));
    assert_eq!(interop_facade::depth(), 0);
    eval_in_context("prod", "left=0").unwrap();
    assert_eq!(eval_in_context_string("cons", "String(__c.getCount())"), "7");
    shutdown();
}

/// While the profiler records, facade calls take the native path, which records their spans.
#[test]
fn facade_calls_take_the_native_path_while_profiling() {
    facade_setup();
    interop_facade::set_profiling(true);
    assert_eq!(eval_in_context_string("cons", "String(__c.getCount())"), "1");
    // The native path binds nothing.
    assert_eq!(interop_facade::tracked_cells(), 0);
    interop_facade::set_profiling(false);
    assert_eq!(eval_in_context_string("cons", "String(__c.getCount())"), "1");
    assert_eq!(interop_facade::tracked_cells(), 1);
    shutdown();
}

/// Per-call cost of the native path against a facade, for a number result, an object result,
/// and an object argument. Run with `--ignored --nocapture` in a release build.
#[test]
#[ignore = "benchmark"]
fn facade_bench_call_cost() {
    let shape: crate::interop::Schema = serde_json::from_value(serde_json::json!({"kind":"object","fields":{
        "id":{"schema":{"kind":"number"},"optional":false},
        "kills":{"schema":{"kind":"number"},"optional":false},
        "name":{"schema":{"kind":"string"},"optional":false}
    }})).unwrap();
    protocol2_setup();
    let mut contract = published_contract("@x/counter").unwrap().as_ref().clone();
    let number = crate::interop::Field { schema: crate::interop::Schema::Number, optional: false };
    contract.metadata.methods.insert("add".into(), crate::interop::Method {
        args: vec![number.clone(), number.clone()], result: crate::interop::Schema::Number,
    });
    contract.metadata.methods.insert("stats".into(), crate::interop::Method {
        args: vec![number.clone()], result: shape.clone(),
    });
    contract.metadata.methods.insert("store".into(), crate::interop::Method {
        args: vec![crate::interop::Field { schema: shape, optional: false }], result: crate::interop::Schema::Number,
    });
    final_review_install_contract(contract);
    eval_in_context("prod", r#"__s2_iface_publish('@x/counter',{getCount:()=>1,add:(a,b)=>a+b,
      stats:id=>({id:id,kills:3,name:'x'}),store:s=>s.kills})"#).unwrap();
    let out = eval_in_context_string("cons", r#"(function () {
      const c = __s2_require('@x/counter');
      const N = 200000;
      function run(f) {
        let best = Infinity, sink = 0;
        for (let r = 0; r < 5; r++) {
          const t0 = Number(__s2_hrtime_ns());
          for (let i = 0; i < N; i++) sink += f(i);
          best = Math.min(best, (Number(__s2_hrtime_ns()) - t0) / N);
        }
        return best.toFixed(1) + (sink === -1 ? "" : "");
      }
      const row = { id: 1, kills: 3, name: 'x' };
      return [
        "number: native " + run(i => __s2_iface_call('@x/counter','add',[i,1])) + " ns, facade " + run(i => c.add(i, 1)) + " ns",
        "object result: native " + run(i => __s2_iface_call('@x/counter','stats',[i]).kills) + " ns, facade " + run(i => c.stats(i).kills) + " ns",
        "object argument: native " + run(() => __s2_iface_call('@x/counter','store',[row])) + " ns, facade " + run(() => c.store(row)) + " ns",
      ].join("\n");
    })()"#);
    println!("{out}");
    shutdown();
}


/// A cell that rebinds after every change is still tracked once.
#[test]
fn facade_cells_are_tracked_once_across_rebinds() {
    facade_setup();
    for _ in 0..5 {
        assert_eq!(eval_in_context_string("cons", "String(__c.getCount())"), "1");
        interop_facade::revoke();
    }
    assert_eq!(interop_facade::tracked_cells(), 1);
    shutdown();
}

/// A teardown elsewhere during a call (an unrelated plugin unloading) bumps the epoch and sweeps
/// the in-flight cell. The participants are still live, so the call completes, and the result is
/// copied with the producer realm captured before the call.
#[test]
fn facade_call_survives_a_sweep_during_the_call() {
    let shape: crate::interop::Schema = serde_json::from_value(serde_json::json!({"kind":"object","fields":{
        "n":{"schema":{"kind":"number"},"optional":false}
    }})).unwrap();
    protocol2_setup();
    let mut contract = published_contract("@x/counter").unwrap().as_ref().clone();
    contract.metadata.methods.insert("getCount".into(), crate::interop::Method { args: vec![], result: shape });
    final_review_install_contract(contract);
    eval_in_context("prod", "__s2_iface_publish('@x/counter',{getCount:function(){__s2_test_sweep();return {n:5};}})").unwrap();
    eval_in_context("cons", "globalThis.__c = __s2_require('@x/counter')").unwrap();
    assert_eq!(eval_in_context_string("cons", "JSON.stringify([__c.getCount(), __c.getCount()])"), r#"[{"n":5},{"n":5}]"#);
    shutdown();
}
