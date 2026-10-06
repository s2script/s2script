//! Actual private subscription/cursor proofs. Only native frame/slot boundaries are doubled.
use super::*;
use crate::engine_functions::{contract, instance, trusted};
use serde_json::json;

const ADAPTER: &str = "proof.receiver.v1";
const HASH: &str = proof::HASH;
const DAMAGE: &str = "legacy.damage.v1";
const DAMAGE_HASH: &str = "37e53fb0dfcb8d986cacaa28bba02394f9957e3600411dc5d8785e0e4bd0711e";
thread_local! {
    static RAW: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
    static TRACE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
extern "C" fn frame_read(
    _: i64,
    _: u64,
    _: u64,
    _: *const i8,
    selector: i32,
    _: u8,
    out: *mut S2FunctionValue,
    _: *mut i8,
    _: i32,
) -> i32 {
    let mut value = runtime::blank();
    if selector == -1 {
        value.kind = 8;
        value.flags = unsafe { (*out).flags };
        if let Some((index, serial)) = RAW.with(Cell::get) {
            value.aux = index as u32;
            value.bits = serial as u32 as u64;
        } else {
            value.aux = u32::MAX;
        }
    } else {
        value.kind = 2;
        value.bits = 7;
    }
    unsafe {
        *out = value;
    }
    1
}
extern "C" fn instance_read(
    _: *const S2FunctionInstanceAccess,
    selector: i32,
    out: *mut S2FunctionValue,
    why: *mut i8,
    cap: i32,
) -> i32 {
    unsafe {
        (*out).flags = 2;
    }
    frame_read(1, 1, 1, std::ptr::null(), selector, 8, out, why, cap)
}
extern "C" fn commit(
    _: i64,
    _: u64,
    _: u64,
    _: *const i8,
    _: i32,
    _: *const S2FunctionValue,
    _: *mut i8,
    _: i32,
) -> i32 {
    1
}
extern "C" fn write(
    _: i64,
    _: u64,
    _: u64,
    _: *const i8,
    selector: i32,
    value: *const S2FunctionValue,
    _: *mut i8,
    _: i32,
) -> i32 {
    if selector == -1 {
        let value = unsafe { *value };
        RAW.with(|raw| raw.set(Some((value.aux as i32, value.bits as u32 as i32))));
    }
    1
}
extern "C" fn field_read(
    _: *const S2FunctionInstanceAccess,
    _: i32,
    _: u32,
    _: *mut S2FunctionValue,
    _: *mut i8,
    _: i32,
) -> i32 {
    0
}
extern "C" fn field_write(
    _: *const S2FunctionInstanceAccess,
    _: i32,
    _: u32,
    _: *const S2FunctionValue,
    _: *mut i8,
    _: i32,
) -> i32 {
    0
}
extern "C" fn prepare_instance(
    binding: u64,
    _: *const S2FunctionInstanceOwner,
    _: *const i8,
    _: *const i8,
    _: *const i8,
    out: *mut S2FunctionInstancePrepared,
    _: *mut i8,
    _: i32,
) -> i32 {
    unsafe {
        *out = S2FunctionInstancePrepared {
            version: 1,
            struct_size: 24,
            target: 1,
            capability: binding,
        };
    }
    1
}
extern "C" fn activate(_: u64, _: *const S2FunctionInstanceOwner, _: *mut i8, _: i32) -> i32 {
    1
}
extern "C" fn release(_: u64) -> i32 {
    1
}
fn action(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _: v8::ReturnValue) {
    let command = args.get(0).to_rust_string_lossy(scope);
    match command.as_str() {
        "delete" => {
            crate::entity_live::on_deleted(901, 71);
        }
        "recreate" => {
            crate::entity_live::on_created(901, 72);
            RAW.with(|raw| raw.set(Some((901, 72))));
        }
        "map" => {
            crate::entity_live::clear_for_map_transition();
            crate::entity_live::on_created(901, 71);
        }
        "retire-b" => {
            // Exercise the real unload admission/store-retirement boundary. The full unload
            // callback runs after this outer frame; calling it under the fixture's HOST borrow
            // would be an unsupported direct test-harness entry.
            PLUGINS.with(|plugins| {
                plugins.borrow_mut().get_mut("receiver-b").unwrap().phase = plugin::Phase::Unloading;
            });
            drop_owner(&OwnerKey::plugin("receiver-b", plugin_generation("receiver-b")));
        }
        "nested" => crate::nest::with_outbound(&args, || drive()),
        "overlay-b" => {
            let id = crate::entity_live::lookup(902).unwrap().0;
            // A test-only native boundary stages an edit through the real lease acceptance path.
            // Public JS receiver mutation is readonly; no production setter is added.
            LEASES.with(|leases| {
                let leases = leases.borrow();
                let lease = leases.last().unwrap();
                lease.pending_edits.borrow_mut().insert(
                    -1,
                    (
                        EntityProjection::Nullable
                            .value(Some(projection::EntityReference { index: 902, id }))
                            .unwrap(),
                        lease.binding.function.canonical_id.clone(),
                    ),
                );
            });
        }
        label => TRACE.with(|trace| trace.borrow_mut().push(label.into())),
    }
}
fn install_action(id: &str) {
    proof::install_generic_test_native(id);
    with_host_isolate(|isolate| {
        let mut storage = v8::HandleScope::new(isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let context = clone_plugin_context(id).unwrap();
        let context = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, context);
        let f = v8::Function::new(scope, action).unwrap();
        let global = context.global(scope);
        set(scope, global, "act", f.into()).unwrap();
    })
    .unwrap();
}
enum Package {
    Ordinary(PreparedPackageReceipt),
    Trusted(trusted::TrustedPackageActivation),
}
struct Harness {
    package: Package,
    owners: Vec<String>,
}
fn start(count: usize, trusted_receiver: bool, stock_damage: bool) -> Harness {
    scalar_transport_tests::init_transport();
    let mut ops = engine_ops().unwrap();
    ops.function_frame_read = Some(frame_read);
    ops.function_frame_write = Some(write);
    ops.function_frame_field_read = Some(field_read);
    ops.function_frame_field_write = Some(field_write);
    ops.function_frame_commit = Some(commit);
    ops.function_prepare_instance = Some(prepare_instance);
    ops.function_instance_activate = Some(activate);
    ops.function_instance_release = Some(release);
    ops.function_frame_read_instance = Some(instance_read);
    set_engine_ops(Some(ops));
    TRACE.with(|trace| trace.borrow_mut().clear());
    let ids = (0..8)
        .map(|i| crate::entity_live::on_created(901 + i, 71 + i))
        .collect::<Vec<_>>();
    RAW.with(|raw| raw.set(Some((901, 71))));
    let (adapter, hash) = if stock_damage {
        (DAMAGE, DAMAGE_HASH)
    } else {
        (ADAPTER, HASH)
    };
    let source = if stock_damage {
        let damage = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../games/cs2/js/adapters/damage.js"),
        )
        .unwrap();
        format!(
            r#"globalThis.__s2_adapter_contracts={{'{DAMAGE}':'{DAMAGE_HASH}'}};
          globalThis.providers={{}};globalThis.wrapperEntries=0;globalThis.handlerEntries=0;
          Object.defineProperty(globalThis,'__s2_sdkhook_provider_register',{{configurable:true,value:(type,provider)=>{{providers[type]=provider;}}}});
          globalThis.__s2_ent_ref_valid=()=>true;
          {{const subscribe=__s2_function_adapter_subscribe;
            globalThis.__s2_function_adapter_subscribe=(...args)=>{{const fn=args[3];args[3]=frame=>{{wrapperEntries++;return fn(frame);}};return subscribe(...args);}};
          }}
          {damage}"#
        )
    } else {
        format!(
            r#"(()=>{{const subscribe=__s2_function_adapter_subscribe;
          globalThis.add=(binding,phase,callback,...pair)=>subscribe(binding,'{adapter}',phase,callback,...pair);
          __s2_function_adapter_register('{adapter}','{hash}',{{pre(d){{while(d.cursor.invokeNext()!==null){{}}return 0;}},post(d){{while(d.cursor.invokeNext()!==null){{}}}}}});
        }})()"#
        )
    };
    let host = HostPackageOwner::mint("@proof/receiver").unwrap();
    let manifest =
        ImplementationManifestHash::new(contract::hash_bytes(source.as_bytes())).unwrap();
    let package = if trusted_receiver {
        let mut policy = json!({"id":"generic.v2","version":1,"surfaces":["pre","post"],"selfCall":"bypass-own-hooks","suppression":"generic"});
        policy["contractHash"] = contract::hash(&policy).into();
        let f = json!({"localName":"takeDamageOld","requirement":"required",
          "target":{"kind":"signature","module":"server","pattern":"55","resolve":"direct","derivation":"identity","candidateValidate":{},"targetValidate":{"prologue":"55"}},
          "signature":{"platform":"linux-x86_64-sysv","memberReceiver":true,
            "receiver":{"name":"victim","native":"ptr","projection":{"id":"entity?","version":1},"mutable":[],"nullable":false},
            "parameters":[{"name":"x","native":"i32","projection":{"id":"i32","version":1},"mutable":["pre"],"nullable":false}],
            "returns":{"name":"","native":"i32","projection":{"id":"i32","version":1},"mutable":[],"nullable":false},
            "fingerprint":"linux-x86_64-sysv:entity:i32(i32)","stackCopyBytes":128,"instances":[],"scratch":[]},
          "policy":policy,"adapter":{"id":adapter,"contractHash":hash,"postOverride":false}});
        let selected = "{}";
        let artifact = json!({"schemaVersion":1,"ownerId":"@proof/receiver","selectedOffsets":selected,
            "selectedOffsetsSha256":contract::hash_bytes(selected.as_bytes()),"functions":[f]});
        Package::Trusted(
            trusted::activate_trusted(
                &host,
                source.into(),
                manifest,
                artifact.to_string().as_bytes(),
                None,
                unsafe { instance::SynchronousRecordLifetime::registered_native_target() },
            )
            .unwrap(),
        )
    } else {
        Package::Ordinary(register_prepared_package(host, source.into(), manifest).unwrap())
    };
    let owners = (0..count)
        .map(|i| format!("receiver-{}", char::from(b'a' + i as u8)))
        .collect::<Vec<_>>();
    for id in &owners {
        frame_tests::load_body(id, "return {};", "{}");
        install_action(id);
        let binding = match &package {
            Package::Ordinary(source) => {
                let b = proof::entity_binding(id, false, false, true);
                authorize_binding(
                    source,
                    &OwnerKey::plugin(id, plugin_generation(id)),
                    b,
                    adapter,
                    hash,
                )
                .unwrap();
                b
            }
            Package::Trusted(_) => {
                registry::named_binding(&host_owner(), "takeDamageOld")
                    .unwrap()
                    .id
            }
        };
        eval_in_context(
            id,
            &format!("globalThis.binding={binding}n;globalThis.ids={ids:?};"),
        )
        .unwrap();
    }
    Harness { package, owners }
}
fn host_owner() -> OwnerKey {
    PACKAGES.with(|p| {
        p.borrow()
            .values()
            .find(|p| p.owner.key().id == "@proof/receiver")
            .unwrap()
            .owner
            .key()
            .clone()
    })
}
fn drive_allow_map_retirement(retired: bool) {
    let token = registry::next_id().unwrap();
    let info = S2FunctionFrameInfo {
        version: 1,
        struct_size: 48,
        frame_token: token,
        native_epoch: token,
        invocation_id: token,
        suppressed_owner: 0,
        parameter_count: 1,
        flags: 0,
    };
    assert_eq!(
        crate::ffi::s2script_core_dispatch_function(1, &info, 0),
        i32::from(!retired)
    );
    assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 1), 1);
    let errors = proof::take_dispatch_errors();
    if retired {
        assert_eq!(
            errors,
            ["adapter threw"],
            "the current map invalidates the existing adapter cursor lease"
        );
    } else {
        assert!(errors.is_empty(), "{errors:?}");
    }
}
fn drive() {
    drive_allow_map_retirement(false);
}
fn finish(h: Harness) {
    for id in h.owners {
        unload_plugin(&id);
    }
    match h.package {
        Package::Ordinary(source) => drop(source),
        Package::Trusted(active) => drop(active),
    }
    set_engine_ops(None);
    shutdown();
}

#[test]
fn receiver_constraint_stock_damage_enters_one_of_eight_real_wrappers() {
    for contexts in [1, 8] {
        let h = start(contexts, true, true);
        for i in 0..8 {
            let owner = &h.owners[i % contexts];
            eval_in_context(owner,&format!("providers.OnTakeDamage.hook({},ids[{i}],()=>{{handlerEntries++;}});providers.OnTakeDamagePost.hook({},ids[{i}],()=>{{handlerEntries++;}});",901+i,901+i)).unwrap();
        }
        drive();
        let mut entered = 0;
        let mut handled = 0;
        for owner in &h.owners {
            entered += frame_tests::eval_in_context_string(owner, "String(wrapperEntries)")
                .parse::<u32>()
                .unwrap();
            handled += frame_tests::eval_in_context_string(owner, "String(handlerEntries)")
                .parse::<u32>()
                .unwrap();
        }
        assert_eq!(handled, 2);
        assert_eq!(
            entered, 2,
            "PRE and POST each enter exactly one of eight wrappers, {contexts} context(s)"
        );
        finish(h);
    }
}

#[test]
fn receiver_constraint_private_admission_rejects_invalid_pairs_without_coercion() {
    let h = start(1, false, false);
    eval_in_context("receiver-a",r#"{
      let coerced=0,denied=0;
      const evil={valueOf(){coerced++;return 901}};
      const bad=[[901],['901',ids[0]],[evil,ids[0]],[-1,ids[0]],[901.5,ids[0]],[901,0],[901,NaN],[901,Infinity],[901,2**53],[901,ids[1]],[901,1n]];
      for(const pair of bad){try{add(binding,'pre',()=>{},...pair)}catch(_){denied++}}
      if(denied!==bad.length||coerced!==0)throw Error('invalid pair admitted/coerced');
      globalThis.normal=add(binding,'pre',()=>act('plain'));
      globalThis.filtered=add(binding,'pre',()=>act('match'),901,ids[0]);
    }"#).unwrap();
    drive();
    assert_eq!(TRACE.with(|t| t.borrow().clone()), ["plain", "match"]);
    finish(h);
}

#[test]
fn receiver_constraint_keeps_generic_interleaving_and_matches_accepted_overlay_at_cursor_position()
{
    for accept in [false, true] {
        let h = start(2, true, false);
        // Stage through the real callback lease; only a valid decision publishes the edit.
        eval_in_context("receiver-a",&format!("add('takeDamageOld','pre',()=>{{act('first');act('overlay-b');return {}; }},901,ids[0]);",if accept {"0"}else{"'invalid'"})).unwrap();
        let generic = proof::entity_binding("receiver-b", false, false, true);
        eval_in_context("receiver-b",&format!("globalThis.observer=__proofSubscribeGeneric({generic}n,'pre',true,()=>act('generic'));add('takeDamageOld','pre',()=>act('old'),901,ids[0]);add('takeDamageOld','pre',()=>act('new'),902,ids[1]);")).unwrap();
        drive();
        let trace = TRACE.with(|t| t.borrow().clone());
        assert!(
            trace.contains(&"generic".into()),
            "generic observer must remain delivered"
        );
        assert_eq!(
            trace.iter().filter(|x| **x == "old").count(),
            usize::from(!accept)
        );
        assert_eq!(
            trace.iter().filter(|x| **x == "new").count(),
            usize::from(accept)
        );
        finish(h);
    }
}

#[test]
fn receiver_constraint_disposal_serial_replacement_and_map_retirement_skip_later_wrappers() {
    for mode in ["dispose", "delete", "recreate", "map", "retire-b"] {
        let h = start(2, true, false);
        eval_in_context(
            "receiver-a",
            &format!(
                "add('takeDamageOld','pre',()=>{{act('first');{} }},901,ids[0]);",
                if mode == "dispose" {
                    "late.dispose();".to_string()
                } else {
                    format!("act('{mode}');")
                }
            ),
        )
        .unwrap();
        // Dispose inside the original adapter context; other scenarios retire the remote listener.
        let owner = if mode == "dispose" {
            "receiver-a"
        } else {
            "receiver-b"
        };
        eval_in_context(
            owner,
            "globalThis.late=add('takeDamageOld','pre',()=>act('late'),901,ids[0]);",
        )
        .unwrap();
        if mode != "dispose" {
            eval_in_context(
                "receiver-b",
                "add('takeDamageOld','post',()=>act('post-late'),901,ids[0]);",
            )
            .unwrap();
        }
        drive_allow_map_retirement(mode == "map");
        assert_eq!(TRACE.with(|t| t.borrow().clone()), ["first"]);
        if matches!(mode, "map" | "recreate") {
            drive();
            assert_eq!(
                TRACE.with(|t| t.borrow().clone()),
                ["first"],
                "retired constraints cannot reacquire a same-index successor on the next frame"
            );
        }
        finish(h);
    }
}

#[test]
fn receiver_constraint_nested_dispatch_keeps_busy_context_and_outer_order() {
    let h = start(2, true, false);
    eval_in_context(
        "receiver-a",
        "add('takeDamageOld','pre',()=>{act('outer');act('nested');act('resumed');},901,ids[0]);",
    )
    .unwrap();
    eval_in_context("receiver-b","add('takeDamageOld','pre',()=>act('matching'),901,ids[0]);add('takeDamageOld','pre',()=>act('irrelevant'),902,ids[1]);").unwrap();
    drive();
    assert_eq!(
        TRACE.with(|t| t.borrow().clone()),
        ["outer", "matching", "resumed", "matching"]
    );
    assert_eq!(proof::pending_invocations(), 0);
    finish(h);
}
