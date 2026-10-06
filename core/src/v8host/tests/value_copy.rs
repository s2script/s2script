//! In-isolate typed method transport semantics and an opt-in full-contract benchmark.
use super::*;

#[test]
fn protocol2_no_ref_method_ignores_mutated_json_parse_and_reviver() {
    protocol2_setup();
    for owner in ["prod", "cons"] {
        eval_in_context(owner, r#"
          globalThis.parseCalls=0;globalThis.reviveCalls=0;
          const originalParse=JSON.parse; const originalReviver=__s2_entref_reviver;
          JSON.parse=function(...args){parseCalls++;return originalParse.apply(JSON,args)};
          __s2_entref_reviver=function(k,v){reviveCalls++;return originalReviver(k,v)};
        "#).unwrap();
    }
    assert_eq!(eval_in_context_string("cons", "String(__s2_iface_call('@x/counter','getCount',[]))"), "1");
    for owner in ["prod", "cons"] {
        assert_eq!(eval_in_context_string(owner,"String(parseCalls)+','+String(reviveCalls)"), "0,0");
    }
    shutdown();
}

fn echo_setup(schema: serde_json::Value) {
    protocol2_setup();
    let mut contract = published_contract("@x/counter").unwrap().as_ref().clone();
    let schema: crate::interop::Schema = serde_json::from_value(schema).unwrap();
    contract.metadata.methods.insert("getCount".into(), crate::interop::Method {
        args: vec![crate::interop::Field { schema: schema.clone(), optional: false }], result: schema,
    });
    final_review_install_contract(contract);
    eval_in_context("prod", "__s2_iface_publish('@x/counter',{getCount:p=>{globalThis.last=p;return p}})").unwrap();
}
#[test]
fn protocol2_plain_copy_preserves_keys_descriptors_proto_unicode_negative_zero_and_identity() {
    echo_setup(serde_json::json!({"kind":"object","fields":{
        "__proto__":{"schema":{"kind":"object","fields":{"tag":{"schema":{"kind":"string"},"optional":false}}},"optional":false},
        "10":{"schema":{"kind":"number"},"optional":false},
        "2":{"schema":{"kind":"number"},"optional":false},
        "a":{"schema":{"kind":"string"},"optional":false},
        "z":{"schema":{"kind":"array","item":{"kind":"number"}},"optional":false}
    }}));
    for owner in ["prod", "cons"] {
        eval_in_context(owner, "globalThis.setterHits=0;Object.defineProperty(Object.prototype,'a',{set(){setterHits++;throw Error('inherited setter')},configurable:true})").unwrap();
    }
    assert!(eval_in_context_bool("cons", r#"(()=>{
      const p=Object.create(null);
      Object.defineProperties(p,{'10':{value:-0},'2':{value:2},a:{value:'\u0000é漢😀�'},
        z:{value:[1,2]},__proto__:{value:{tag:'own'}}});
      // Object-literal __proto__ denotes a prototype, so define the data key explicitly.
      Object.defineProperty(p,'__proto__',{value:{tag:'own'}});
      const out=__s2_iface_call('@x/counter','getCount',[p]);
      if(out===p||out.z===p.z||Object.getPrototypeOf(out)!==Object.prototype||
         Object.getPrototypeOf(p)!==null||Object.keys(out).join(',')!=='2,10,__proto__,a,z'||
         !Object.is(out['10'],-0)||out.a!=='\u0000é漢😀�'||out.__proto__.tag!=='own')return false;
      for(const key of Object.keys(out)){const d=Object.getOwnPropertyDescriptor(out,key);if(!d.writable||!d.configurable||!d.enumerable)return false;}
      out.z[0]=99;out.__proto__.tag='changed';globalThis.out=out;
      return p.z[0]===1&&p.__proto__.tag==='own'&&setterHits===0;
    })()"#));
    assert!(eval_in_context_bool("prod", "last.z[0]===1&&last.__proto__.tag==='own'&&setterHits===0"));
    shutdown();
}
#[test]
fn protocol2_plain_copy_rejects_active_source_values_without_running_them() {
    echo_setup(serde_json::json!({"kind":"object","fields":{"count":{"schema":{"kind":"number"},"optional":false}}}));
    eval_in_context("cons","globalThis.getterHits=0;globalThis.proxyHits=0;globalThis.jsonHits=0;").unwrap();
    for payload in [
        "{get count(){getterHits++;return 1}}",
        "new Proxy({count:1},{ownKeys(){proxyHits++;return ['count']}})",
        "{count:1,toJSON(){jsonHits++;return {count:1}}}",
        "{count:NaN}", "{count:Infinity}", "{count:1n}", "{count:Symbol('x')}",
        "{count:()=>1}", "Object.assign({count:1},{extra:1})", "{count:'wrong'}",
    ] {
        assert!(eval_in_context_bool("cons",&format!(r#"(()=>{{try{{__s2_iface_call('@x/counter','getCount',[{payload}]);return false}}catch(e){{return e.message.includes('InterfaceValueNotSerializable')}}}})()"#)));
    }
    assert_eq!(eval_in_context_string("cons","[getterHits,proxyHits,jsonHits].join(',')"),"0,0,0");
    assert_eq!(eval_in_context_string("prod","String(typeof last)"),"undefined");
    shutdown();
}
#[test]
fn protocol2_plain_copy_rejects_holes_array_extras_cycles_and_lone_surrogates() {
    echo_setup(serde_json::json!({"kind":"array","item":{"kind":"string"}}));
    for payload in ["['ok',,'bad']", "Object.assign(['ok'],{extra:1})", "['\\ud800']", "['\\udfff']"] {
        assert!(eval_in_context_bool("cons",&format!(r#"(()=>{{try{{__s2_iface_call('@x/counter','getCount',[{payload}]);return false}}catch(e){{return e.message.includes('InterfaceValueNotSerializable')}}}})()"#)));
    }
    assert!(eval_in_context_bool("cons",r#"(()=>{let p=[];p.push(p);try{__s2_iface_call('@x/counter','getCount',[p]);return false}catch(e){return e.message.includes('InterfaceValueNotSerializable')}})()"#));
    shutdown();
}
#[test]
fn protocol2_ref_directions_keep_json_revival_and_context_bound_writable_refs() {
    echo_setup(serde_json::json!({"kind":"entityRef"}));
    for owner in ["prod","cons"] {
        eval_in_context(owner,"globalThis.parseCalls=0;const originalParse=JSON.parse;JSON.parse=function(...args){parseCalls++;return originalParse.apply(JSON,args)}").unwrap();
    }
    assert!(eval_in_context_bool("cons",r#"(()=>{const p=new __s2pkg_entity.EntityRef(7,17);const out=__s2_iface_call('@x/counter','getCount',[p]);return out!==p&&out instanceof __s2pkg_entity.EntityRef&&out.index===7&&out.id===17&&Object.getOwnPropertyDescriptor(out,'index').writable&&Object.getOwnPropertyDescriptor(out,'id').writable})()"#));
    for owner in ["prod","cons"] {assert_eq!(eval_in_context_string(owner,"String(parseCalls)"),"1");}
    shutdown();
}
#[test]
fn protocol2_legacy_directions_keep_mutable_json_parse_and_reviver() {
    let _=init(dummy_logger());
    set_plugin_publishes("prod",[("@legacy/copy".into(),crate::loader::PublishDecl {
        version:"1.0.0".into(),types_sha256:"test".into(),contract:None
    })].into_iter().collect());
    create_plugin_context("prod");
    set_plugin_imports("cons",vec![crate::interfaces::ImportSpec::new("@legacy/copy","^1.0.0",crate::interfaces::Kind::Hard)]);
    create_plugin_context("cons");
    eval_in_context("prod","__s2_iface_publish('@legacy/copy',{echo:p=>p})").unwrap();
    for owner in ["prod","cons"] {
        eval_in_context(owner,"globalThis.parseCalls=0;globalThis.reviveCalls=0;const originalParse=JSON.parse,originalRevive=__s2_entref_reviver;JSON.parse=function(...args){parseCalls++;return originalParse.apply(JSON,args)};__s2_entref_reviver=function(k,v){reviveCalls++;return originalRevive(k,v)}").unwrap();
    }
    assert!(eval_in_context_bool("cons","__s2_iface_call('@legacy/copy','echo',[{count:1}]).count===1"));
    for owner in ["prod","cons"] {assert!(eval_in_context_bool(owner,"parseCalls===1&&reviveCalls>0"));}
    shutdown();
}

/// Opt-in real V8 crossing benchmark: complete frozen publication metadata, stable DTO values.
/// No engine/native gameplay calls. Run both source arms with identical fixture and test bytes.
#[test]
#[ignore = "requires explicitly pinned external full-contract fixture and benchmark mode"]
fn protocol2_actual_full_contract_value_copy_benchmark() {
    let fixture_path=std::env::var("S2SCRIPT_VALUE_COPY_BENCH_FIXTURE").expect("explicit full-contract fixture");
    let arm=std::env::var("S2SCRIPT_VALUE_COPY_BENCH_ARM").expect("explicit benchmark arm");
    assert!(matches!(arm.as_str(),"baseline103"|"gated-candidate"|"immutable-no-ref"));
    let bundle:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(fixture_path).unwrap()).unwrap();
    let cases=[
      ("Core.matchesWorldActionContext", "core.matchesWorldActionContext(stamp)?1:0", 1.0),
      ("Core.matchesActor", "core.matchesActor(stamp.actor)?1:0", 1.0),
      ("Core.getGame", "core.getGame().participants", 8.0),
      ("Core.getWorldSettings", "core.getWorldSettings().roleGlowSeconds", 30.0),
      ("World.getRoundSettings", "world.getRoundSettings().roleGlowSeconds", 30.0),
      ("World.appliedModelOf", "world.appliedModelOf(stamp.actor).length", 38.0),
      ("Core.getWorldActionContext.refControl", "core.getWorldActionContext(7).pawn instanceof __s2pkg_entity.EntityRef?1:0", 1.0),
      ("World.restrictToTraitors.refControl", "world.restrictToTraitors(ref)?1:0", 1.0),
    ];
    const CALLS:usize=4096;
    for cohort in 0..4 {
        let _=init(dummy_logger());set_engine_ops(None);
        let mut imports=Vec::new();let mut imported=std::collections::HashMap::new();
        for (label,owner,expected_methods) in [("core","bench-core",70),("world","bench-world",14)] {
            let decl=&bundle[label];let name=decl["interface"].as_str().unwrap();
            let contract:crate::interop::Contract=serde_json::from_value(decl["contract"].clone()).unwrap();
            assert_eq!(contract.validate(),Ok(()));assert_eq!(contract.metadata.methods.len(),expected_methods);
            assert_eq!(contract.sha256,if label=="core" {"63f90d4530062b4b6ecbcd30955bd52be19b6a9af5eaa6808b8d04b3d075c1e6"} else {"d1feff58d0f96c0a7df141df0a3ddbacec9be11e1624eb62b8ab15ff9863014d"});
            let version=decl["version"].as_str().unwrap().to_string();let types=decl["typesSha256"].as_str().unwrap().to_string();
            set_plugin_publishes(owner,[(name.into(),crate::loader::PublishDecl {version,types_sha256:types.clone(),contract:Some(contract.clone())})].into_iter().collect());
            create_plugin_context(owner);
            let methods=serde_json::to_string(&contract.metadata.methods.keys().collect::<Vec<_>>()).unwrap();
            let source=format!(r#"
              const impl=Object.fromEntries({methods}.map(name=>[name,()=>undefined]));
              const stamp={{actor:{{slot:7,steamId:'fixture-connection',generation:3,roundEpoch:21}},mapEpoch:2,state:2,name:'fixture-name',role:2,alive:true,participating:true}};
              const overrides={{matchesWorldActionContext:s=>s.actor.slot===7&&s.actor.generation===3&&s.actor.roundEpoch===21&&s.mapEpoch===2&&s.state===2&&s.role===2&&s.alive&&s.participating,
                matchesActor:a=>a.slot===7&&a.generation===3&&a.roundEpoch===21,
                getGame:()=>({{state:2,winner:0,startedAt:1234,participants:8,roundsThisMap:7,epoch:21}}),
                getWorldSettings:()=>({{iconsEnabled:true,rolePanels:false,roleGlowSeconds:30}}),
                getRoundSettings:()=>({{iconsEnabled:true,rolePanels:false,roleGlowSeconds:30}}),
                appliedModelOf:()=> 'models/player/tm_phoenix_variantf.vmdl',
                getWorldActionContext:()=>({{...stamp,pawn:new __s2pkg_entity.EntityRef(9,42)}}),
                restrictToTraitors:r=>r instanceof __s2pkg_entity.EntityRef&&r.index===9&&r.id===42}};
              for(const method of Object.keys(impl)) if(Object.prototype.hasOwnProperty.call(overrides,method)) impl[method]=overrides[method];
              __s2_iface_publish({name:?},impl);
            "#);
            eval_in_context(owner,&source).unwrap();
            imports.push(crate::interfaces::ImportSpec {name:name.into(),range:"^2.0.0".into(),kind:crate::interfaces::Kind::Hard,compiled_types_sha256:Some(types)});
            imported.insert(name.to_string(),contract);
        }
        set_plugin_imports("bench-cons",imports);set_plugin_interop("bench-cons",imported);create_plugin_context("bench-cons");
        eval_in_context("bench-cons",r#"
          globalThis.core=__s2_require('@s2s-ttt/core');globalThis.world=__s2_require('@s2s-ttt/world');
          globalThis.stamp={actor:{slot:7,steamId:'fixture-connection',generation:3,roundEpoch:21},mapEpoch:2,state:2,name:'fixture-name',role:2,alive:true,participating:true};
          globalThis.ref=new __s2pkg_entity.EntityRef(9,42);globalThis.checksum=0;
        "#).unwrap();
        for position in 0..cases.len() {
            let case_index=(position+cohort*2)%cases.len();let (label,expression,expected)=cases[case_index];
            eval_in_context("bench-cons",&format!("for(let i=0;i<64;i++){{{expression};}}" )).unwrap();
            let source=format!("checksum=0;for(let i=0;i<{CALLS};i++){{checksum+=({expression});}}String(checksum)");
            let start=std::time::Instant::now();let checksum=eval_in_context_string("bench-cons",&source);let elapsed=start.elapsed().as_nanos();
            assert_eq!(checksum.parse::<f64>().unwrap(),CALLS as f64*expected,"{label}");
            println!("VALUE_COPY_BENCH {}",serde_json::json!({"arm":arm,"cohort":cohort,"position":position,"label":label,"calls":CALLS,"elapsedNs":elapsed,"checksum":checksum,
              "freshIsolateCohort":true,"fullCoreMethods":70,"fullWorldMethods":14,"engineGameplayCalls":0,"serverTimingClaim":false}));
        }
        shutdown();
    }
}
