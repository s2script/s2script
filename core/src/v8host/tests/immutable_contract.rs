//! Independent peer tests. Include as a sibling of value_copy under v8host::tests.
//! First three intentionally fail baseline103: immutable typed decoding is an explicit change.
use super::*;

fn astra_shape() -> crate::interop::Schema {
    serde_json::from_value(serde_json::json!({"kind":"object","fields":{
      "count":{"schema":{"kind":"number"},"optional":false}
    }})).unwrap()
}
fn astra_method(args: Vec<crate::interop::Field>, result: crate::interop::Schema) {
    protocol2_setup();
    let mut contract = published_contract("@x/counter").unwrap().as_ref().clone();
    contract.metadata.methods.insert("getCount".into(), crate::interop::Method { args, result });
    final_review_install_contract(contract);
}
fn astra_echo() {
    astra_method(vec![crate::interop::Field { schema: astra_shape(), optional: false }], astra_shape());
    eval_in_context("prod", "globalThis.calls=0;__s2_iface_publish('@x/counter',{getCount:p=>{calls++;globalThis.seen=p.count;return p}})").unwrap();
}
#[test]
fn astra_immutable_typed_decoder_cannot_replace_admitted_arguments_or_results() {
    let alterations = [
      "JSON.parse=function(){hits++;return [{count:'corrupt'}]}",
      "__s2_entref_reviver=function(){hits++;return {count:'corrupt'}}",
      "Object.defineProperty(globalThis,'JSON',{get(){hits++;throw Error('JSON getter')},configurable:true})",
      "Object.defineProperty(globalThis,'__s2_entref_reviver',{get(){hits++;throw Error('reviver getter')},configurable:true})",
      "Object.defineProperty(globalThis,'Array',{get(){hits++;throw Error('Array getter')},configurable:true})",
      "Object.defineProperty(Object.prototype,'__s2ref',{get(){hits++;throw Error('marker getter')},configurable:true})",
      "Object.defineProperty(Array.prototype,'__s2ref',{value:[7,17],configurable:true})",
      "JSON=new Proxy(JSON,{get(){hits++;throw Error('JSON proxy')}})",
    ];
    for target in ["prod", "cons"] {
        for alteration in alterations {
            astra_echo();
            eval_in_context(target, &format!("globalThis.hits=0;{alteration}")).unwrap();
            assert!(eval_in_context_bool("cons", "(()=>{const p={count:7};const q=__s2_iface_call('@x/counter','getCount',[p]);return q!==p&&q.count===7})()"), "{target}: {alteration}");
            assert_eq!(eval_in_context_string("prod", "String(seen)+','+String(calls)"), "7,1");
            assert_eq!(eval_in_context_string(target, "String(hits)"), "0", "{target}: {alteration}");
            shutdown();
        }
    }
}
#[test]
fn astra_immutable_structured_return_survives_decoder_mutation_during_reentry() {
    astra_echo();
    eval_in_context("cons", "globalThis.hits=0;__s2_iface_on('@x/counter','OnCountChanged',()=>{Object.defineProperty(globalThis,'JSON',{get(){hits++;throw Error('changed while call active')},configurable:true});Object.defineProperty(Object.prototype,'__s2ref',{get(){hits++;return [7,17]},configurable:true})})").unwrap();
    eval_in_context("prod", "__s2_iface_publish('@x/counter',{getCount:p=>{__s2_iface_emit('@x/counter','OnCountChanged',{count:1});return p}})").unwrap();
    assert!(eval_in_context_bool("cons", "__s2_iface_call('@x/counter','getCount',[{count:7}]).count===7&&hits===0"));
    shutdown();
}
#[test]
fn astra_immutable_primitive_return_does_not_read_replaced_decoder() {
    protocol2_setup();
    eval_in_context("cons", "globalThis.hits=0;Object.defineProperty(JSON,'parse',{get(){hits++;throw Error('parse getter')},configurable:true})").unwrap();
    assert_eq!(eval_in_context_string("cons", "String(__s2_iface_call('@x/counter','getCount',[]))+','+String(hits)"), "1,0");
    shutdown();
}
#[test]
fn astra_typed_decoder_cannot_repair_rejected_sources() {
    astra_echo();
    eval_in_context("prod", "globalThis.hits=0;JSON.parse=function(){hits++;return [{count:7}]}").unwrap();
    assert!(eval_in_context_bool("cons", "(()=>{try{__s2_iface_call('@x/counter','getCount',[{count:'bad'}]);return false}catch(e){return e.message.includes('InterfaceValueNotSerializable')}})()"));
    assert_eq!(eval_in_context_string("prod", "String(hits)+','+String(calls)"), "0,0");
    shutdown();
    astra_echo();
    eval_in_context("prod", "__s2_iface_publish('@x/counter',{getCount:p=>({count:'bad'})})").unwrap();
    eval_in_context("cons", "globalThis.hits=0;JSON.parse=function(){hits++;return {count:7}}").unwrap();
    assert!(eval_in_context_bool("cons", "(()=>{try{__s2_iface_call('@x/counter','getCount',[{count:7}]);return false}catch(e){return e.message.includes('InterfaceValueNotSerializable')}})()&&hits===0"));
    shutdown();
}
#[test]
fn astra_typed_owned_copy_retains_exact_depth_boundary() {
    for levels in [63, 64] {
        let mut shape = crate::interop::Schema::Number;
        for _ in 0..levels { shape = crate::interop::Schema::Array { item: Box::new(shape) }; }
        astra_method(vec![crate::interop::Field { schema: shape.clone(), optional: false }], shape);
        eval_in_context("prod", "globalThis.calls=0;__s2_iface_publish('@x/counter',{getCount:p=>{calls++;return p}})").unwrap();
        let expression = format!("(()=>{{let p=7;for(let i=0;i<{levels};i++)p=[p];try{{let q=__s2_iface_call('@x/counter','getCount',[p]);for(let i=0;i<{levels};i++)q=q[0];return q===7?'accepted':'changed'}}catch(e){{return e.message.includes('InterfaceValueNotSerializable')?'rejected':e.message}}}})()");
        assert_eq!(eval_in_context_string("cons", &expression), if levels==63 { "accepted" } else { "rejected" });
        assert_eq!(eval_in_context_string("prod", "String(calls)"), if levels==63 { "1" } else { "0" });
        shutdown();
    }
    let mut shape = crate::interop::Schema::Number;
    for _ in 0..64 { shape = crate::interop::Schema::Array { item: Box::new(shape) }; }
    astra_method(vec![], shape);
    eval_in_context("prod", "__s2_iface_publish('@x/counter',{getCount:()=>{let p=7;for(let i=0;i<64;i++)p=[p];return p}})").unwrap();
    assert!(eval_in_context_bool("cons", "(()=>{let p=__s2_iface_call('@x/counter','getCount',[]);for(let i=0;i<64;i++)p=p[0];return p===7})()"));
    shutdown();
}
#[test]
fn astra_typed_structured_result_is_not_delivered_after_participant_retirement() {
    for action in ["2", "true", "'cons'", "'prod'", "'owner'"] {
        astra_method(vec![], astra_shape());
        protocol2_install_teardown_probe();
        eval_in_context("cons", &format!("__s2_iface_on('@x/counter','OnCountChanged',()=>__test_interop_teardown({action}))")).unwrap();
        eval_in_context("prod", "__s2_iface_publish('@x/counter',{getCount:()=>{__s2_iface_emit('@x/counter','OnCountChanged',{count:1});return {count:7}}})").unwrap();
        assert!(eval_in_context_bool("cons", "(()=>{try{__s2_iface_call('@x/counter','getCount',[]);return false}catch(e){return e.message.includes('InterfaceUnavailable')}})()"), "{action}");
        shutdown();
    }
}
