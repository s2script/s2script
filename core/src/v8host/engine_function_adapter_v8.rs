//! Test-only S2 feasibility transport. No production/public binding is installed.
//! Unlike generic fan_out_inner, this explicitly parses a typed test decision.
use super::*;
use std::cell::{Cell, RefCell};
use std::ffi::CString;

type Call = unsafe extern "C" fn(u64, i32, *mut i32) -> i32;
type Callback = extern "C" fn(i32, u64, i32, *mut i32) -> i32;
type Create = unsafe extern "C" fn(Callback) -> i32;
type Remove = unsafe extern "C" fn() -> i32;
#[derive(Clone)]
struct Instance {
    owner: String,
    generation: u64,
    contract: u64,
    wrapper: v8::Global<v8::Function>,
}
thread_local! {
    static INSTANCES: RefCell<Vec<Instance>> = const { RefCell::new(Vec::new()) };
    static ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static CALL: Cell<Option<Call>> = const { Cell::new(None) };
    static BYPASS: RefCell<Vec<(String, u64)>> = const { RefCell::new(Vec::new()) };
}
fn record(event: &str) {
    ORDER.with(|o| o.borrow_mut().push(event.into()));
}
fn js_event(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _: v8::ReturnValue) {
    record(&args.get(0).to_rust_string_lossy(scope));
}
fn js_call(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let context = scope.get_current_context();
    let owner = context.get_slot::<PluginId>().unwrap().0.clone();
    let generation = context.get_slot::<InteropGeneration>().unwrap().0;
    assert!(owner_is_live(&owner, generation));
    let input = args.get(0).int32_value(scope).unwrap();
    BYPASS.with(|b| b.borrow_mut().push((owner.clone(), generation)));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            BYPASS.with(|b| {
                b.borrow_mut().pop();
            });
        }
    }
    let _pop = Pop;
    let mut output = 0;
    let call = CALL.with(|c| c.get().unwrap());
    let ok = crate::nest::with_outbound(&args, || unsafe { call(generation, input, &mut output) });
    assert_eq!(ok, 1);
    rv.set_int32(output);
}
// This callback is reached only via stock KHook -> actual libffi PRE closure.
extern "C" fn inbound(phase: i32, caller: u64, input: i32, output: *mut i32) -> i32 {
    if phase == 1 {
        record("return");
        return 0;
    }
    record("KHook-PRE");
    assert!(
        HOST.with(|h| h.try_borrow_mut().is_err()),
        "A must still be executing"
    );
    let info = crate::nest::top()
        .filter(|p| !p.is_null())
        .expect("real outbound nest token");
    let instances = INSTANCES.with(|i| i.borrow().clone());
    let bypass = BYPASS.with(|b| b.borrow().last().cloned().unwrap());
    assert_eq!(bypass.1, caller);
    let mut storage = unsafe { v8::CallbackScope::new(&*info) };
    let mut callback_scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let parent = &mut callback_scope;
    let mut selected = 0;
    for instance in instances {
        if (instance.owner.clone(), instance.generation) == bypass {
            continue;
        }
        if !owner_is_live(&instance.owner, instance.generation) {
            continue;
        }
        assert_eq!(instance.contract, 0x5332464e);
        let context = clone_plugin_context(&instance.owner).unwrap();
        let ctx = v8::Local::new(parent, &context);
        let scope = &mut v8::ContextScope::new(parent, ctx);
        let wrapper = v8::Local::new(scope, &instance.wrapper);
        let argument = v8::Integer::new(scope, input);
        let receiver = v8::undefined(scope).into();
        let decision = wrapper
            .call(scope, receiver, &[argument.into()])
            .expect("B wrapper ran synchronously");
        let object = decision.to_object(scope).expect("typed test decision");
        let action_key = v8::String::new(scope, "action").unwrap();
        let action = object.get(scope, action_key.into()).unwrap();
        assert!(action.is_int32());
        assert_eq!(action.int32_value(scope), Some(2));
        let value_key = v8::String::new(scope, "returnValue").unwrap();
        let value = object.get(scope, value_key.into()).unwrap();
        assert!(value.is_int32());
        unsafe {
            *output = value.int32_value(scope).unwrap();
        }
        selected += 1;
        println!(
            "selected package=B owner={} generation={} contract={:x}",
            instance.owner, instance.generation, instance.contract
        );
    }
    assert_eq!(selected, 1, "exactly B, never A's busy instance");
    2
}
fn install(owner: &str) {
    // Normal host installation mints the context and generation; the package
    // instance below is a deliberately test-only transport with the same contract.
    frame_tests::load_body(owner, "return {};", "{}");
    let generation = plugin_generation(owner);
    assert!(owner_is_live(owner, generation));
    HOST.with(|h| {
        let mut host = h.borrow_mut();
        let host = host.as_mut().unwrap();
        let context = clone_plugin_context(owner).unwrap();
        let mut storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let ctx = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, ctx);
        let global = ctx.global(scope);
        let event = v8::Function::new(scope, js_event).unwrap();
        let key = v8::String::new(scope, "__probeEvent").unwrap();
        global.set(scope, key.into(), event.into());
        let call = v8::Function::new(scope, js_call).unwrap();
        let key = v8::String::new(scope, "__probeCall").unwrap();
        global.set(scope, key.into(), call.into());
    });
    let script = format!("globalThis.__packageInstance = Object.freeze({{ contract: 0x5332464e, owner: '{owner}', generation: {generation} }}); globalThis.__wrapper = function(input) {{ if (__packageInstance.owner !== 'owner-b') throw Error('busy A selected'); __probeEvent('B-wrapper'); return {{action: 2, returnValue: input + 70}}; }};");
    eval_in_context(owner, &script).unwrap();
    HOST.with(|h| {
        let mut host = h.borrow_mut();
        let host = host.as_mut().unwrap();
        let context = clone_plugin_context(owner).unwrap();
        let mut storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let ctx = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, ctx);
        let key = v8::String::new(scope, "__wrapper").unwrap();
        let value = ctx.global(scope).get(scope, key.into()).unwrap();
        let wrapper = v8::Local::<v8::Function>::try_from(value).unwrap();
        let instance = Instance {
            owner: owner.into(),
            generation,
            contract: 0x5332464e,
            wrapper: v8::Global::new(scope, wrapper),
        };
        INSTANCES.with(|i| i.borrow_mut().push(instance));
    });
    println!("installed package owner={owner} generation={generation} contract=5332464e");
}
fn remove_instance(owner: &str) {
    INSTANCES.with(|i| i.borrow_mut().retain(|i| i.owner != owner));
    unload_plugin(owner);
}
#[test]
#[ignore = "requires exact-head real stock-provider bridge; run scripts/test-engine-function-v8-adapter.sh"]
fn busy_caller_stock_provider_spike() {
    assert!(cfg!(all(target_os = "linux", target_arch = "x86_64")));
    let path = std::env::var("S2FN_V8_BRIDGE").expect("absolute real bridge path required");
    assert!(std::path::Path::new(&path).is_absolute());
    let path = CString::new(path).unwrap();
    let library = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    assert!(!library.is_null(), "cannot load real stock-provider bridge");
    unsafe fn symbol(lib: *mut libc::c_void, name: &str) -> *mut libc::c_void {
        let name = CString::new(name).unwrap();
        let p = libc::dlsym(lib, name.as_ptr());
        assert!(!p.is_null());
        p
    }
    let create: Create = unsafe { std::mem::transmute(symbol(library, "s2fn_probe_create")) };
    let call: Call = unsafe { std::mem::transmute(symbol(library, "s2fn_probe_call")) };
    let remove: Remove = unsafe { std::mem::transmute(symbol(library, "s2fn_probe_remove")) };
    init(frame_tests::dummy_logger()).unwrap();
    CALL.with(|c| c.set(Some(call)));
    install("owner-a");
    install("owner-b");
    let b_generation = plugin_generation("owner-b");
    assert_eq!(unsafe { create(inbound) }, 1);
    let a_first = plugin_generation("owner-a");
    for run in 0..2 {
        ORDER.with(|o| o.borrow_mut().clear());
        eval_in_context("owner-a", "__probeEvent('A-before'); const result = __probeCall(7); if(result !== 77) throw Error('typed decision was lost'); __probeEvent('A-after');").unwrap();
        ORDER.with(|o| {
            assert_eq!(
                &*o.borrow(),
                &["A-before", "KHook-PRE", "B-wrapper", "return", "A-after"]
            )
        });
        println!("PASS synchronous order A-before -> KHook-PRE -> B-wrapper -> return -> A-after result=77 run={run}");
        if run == 0 {
            remove_instance("owner-a");
            assert!(!owner_is_live("owner-a", a_first));
            assert!(owner_is_live("owner-b", b_generation));
            install("owner-a");
            assert_ne!(plugin_generation("owner-a"), a_first);
        }
    }
    remove_instance("owner-a");
    assert!(owner_is_live("owner-b", b_generation));
    remove_instance("owner-b");
    assert_eq!(unsafe { remove() }, 1);
    CALL.with(|c| c.set(None));
    shutdown();
    unsafe {
        libc::dlclose(library);
    }
    println!("PASS real V8 owner-only bypass and independent A/B generation teardown");
}

// The Task 6 gate deliberately retains the old feasibility regression above,
// but registers all callbacks/subscriptions below through production bootstrap.
mod production {
    use super::*;
    use crate::engine_functions::{contract::OwnerKey, registry, runtime};
    use crate::v8host::function_adapter::{self, proof, borrowed_proof};
    use std::collections::BTreeMap;
    type CreateProduction = unsafe extern "C" fn(
        extern "C" fn(i64, *const S2FunctionFrameInfo, i32) -> i32,
        extern "C" fn(),
        *mut S2EngineOps,
    ) -> i32;
    type FrameProduction = unsafe extern "C" fn(i32) -> i32;
    type EngineCallProduction = unsafe extern "C" fn(i32, *mut i32) -> i32;
    thread_local! {
        static PACKAGE:RefCell<Option<function_adapter::PreparedPackageReceipt>>=const{RefCell::new(None)};
        static BINDINGS:RefCell<BTreeMap<String,u64>>=const{RefCell::new(BTreeMap::new())};
        static ENTITY_SLOT:Cell<Option<proof::EntitySlot>>=const{Cell::new(None)};
        static ENTITY_STATE:RefCell<Option<proof::EntityConformance>>=const{RefCell::new(None)};
        static POST_STATE:RefCell<Option<proof::PostConformance>>=const{RefCell::new(None)};
        static POST_PEER:Cell<Option<FrameProduction>>=const{Cell::new(None)};
        static PROCESS_STATE:RefCell<Option<super::super::engine_function_tests::PackageServiceProof>>=const{RefCell::new(None)};
        static ENGINE_CALL:Cell<Option<EngineCallProduction>>=const{Cell::new(None)};
        static COPY_PROCESS:RefCell<Option<proof::CopyProcess>>=const{RefCell::new(None)};
        static COPY_STATE:RefCell<Option<proof::CopyConformance>>=const{RefCell::new(None)};
        static COPY_ENGINE:Cell<Option<unsafe extern "C" fn(*const i8,*const i8)->i32>>=const{Cell::new(None)};
        static COPY_PEER:Cell<Option<FrameProduction>>=const{Cell::new(None)};
        static COPY_READY:Cell<bool>=const{Cell::new(false)};
        static PROCESS_READY:Cell<bool>=const{Cell::new(false)};
        static RECORD_STATE:RefCell<Option<borrowed_proof::State>>=const{RefCell::new(None)};
        static CURSOR_STATE:RefCell<Option<borrowed_proof::State>>=const{RefCell::new(None)};
        static RECORD_ENGINE:Cell<Option<borrowed_proof::EngineCall>>=const{Cell::new(None)};
        static RECORD_PEER:Cell<Option<FrameProduction>>=const{Cell::new(None)};
        static RECORD_READY:Cell<bool>=const{Cell::new(false)};
        static STEP:Cell<usize>=const{Cell::new(0)};
        static FAILURE:RefCell<Option<String>>=const{RefCell::new(None)};
    }
    fn call(
        scope: &mut v8::PinScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let context = scope.get_current_context();
        let id = context.get_slot::<PluginId>().unwrap().0.clone();
        let generation = context.get_slot::<InteropGeneration>().unwrap().0;
        let owner = OwnerKey::plugin(&id, generation);
        let binding = BINDINGS.with(|b| b.borrow()[&id]);
        let binding = registry::binding(binding, &owner).unwrap();
        let input = args.get(0).int32_value(scope).unwrap();
        let mut value = runtime::blank();
        value.kind = 2;
        value.bits = input as u32 as u64;
        match crate::nest::with_outbound(&args, || {
            runtime::call(binding.target.unwrap(), Some(&owner), &[value])
        }) {
            Ok(value) => rv.set_int32(value.bits as i32),
            Err(error) => {
                let text = v8::String::new(scope, &error).unwrap();
                let exception = v8::Exception::error(scope, text);
                scope.throw_exception(exception);
            }
        }
    }
    fn load(id: &str) {
        frame_tests::load_body(id, "return {};", "{}");
        HOST.with(|h| {
            let mut host = h.borrow_mut();
            let host = host.as_mut().unwrap();
            let context = clone_plugin_context(id).unwrap();
            let mut storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = v8::Local::new(&mut hs, &context);
            let scope = &mut v8::ContextScope::new(&mut hs, context);
            let function = v8::Function::new(scope, call).unwrap();
            let key = v8::String::new(scope, "__proofCall").unwrap();
            context
                .global(scope)
                .set(scope, key.into(), function.into());
        });
        let binding = PACKAGE.with(|p| proof::bind(p.borrow().as_ref().unwrap(), id));
        BINDINGS.with(|b| b.borrow_mut().insert(id.into(), binding));
    }
    fn exercise() {
        eval_in_context("owner-a","proofEvents.length=0;if(__proofCall(7)!==80)throw Error('nested typed return');if(proofEvents.length)throw Error('busy caller A ran');").unwrap();
        eval_in_context("owner-b","if(proofEvents.join(',')!=='adapter,wrapper,post,post-wrapper:80')throw Error('B selection order: '+proofEvents);proofEvents.length=0;{let refused=0;try{savedView.x}catch(_){refused++}try{savedCursor.invokeNext()}catch(_){refused++}if(refused!==2)throw Error('stale callback facade');}").unwrap();
        assert_eq!(function_adapter::proof::pending_invocations(), 0);
        println!("PASS production A busy -> B adapter -> B SubscriberCursor wrapper -> nested both-busy original -> typed return 80, matched PRE/POST");
    }
    extern "C" fn frame_step() {
        let result = std::panic::catch_unwind(|| match STEP.with(Cell::get) {
            0 => {
                load("owner-a");
                load("owner-b");
                let id = BINDINGS.with(|b| b.borrow()["owner-a"]);
                let owner = OwnerKey::plugin("owner-a", plugin_generation("owner-a"));
                let binding = registry::binding(id, &owner).unwrap();
                assert_eq!(runtime::status(binding.target.unwrap()).unwrap().state, 1);
                let (a, ha) = proof::provenance("owner-a");
                let (b, hb) = proof::provenance("owner-b");
                assert_ne!(a.parent, b.parent);
                assert_eq!(a.package_owner, b.package_owner);
                assert_eq!(ha, hb);
                println!("production parent A={a:?} B={b:?} actual_manifest_sha256={ha}");
                exercise();
                assert_eq!(runtime::status(binding.target.unwrap()).unwrap().state, 2);
            }
            1 => {
                let a = plugin_generation("owner-a");
                let b = plugin_generation("owner-b");
                unload_plugin("owner-a");
                assert_eq!(proof::counts("owner-a", a), (0, 0));
                assert_eq!(proof::counts("owner-b", b), (1, 2));
                load("owner-a");
                assert_ne!(plugin_generation("owner-a"), a);
                exercise();
                create_plugin_context("never-active");
                let generation = plugin_generation("never-active");
                PACKAGE.with(|p| proof::bind(p.borrow().as_ref().unwrap(), "never-active"));
                unload_plugin("never-active");
                assert_eq!(proof::counts("never-active", generation), (0, 0));
            }
            2 => {
                unload_plugin("owner-a");
                unload_plugin("owner-b");
                BINDINGS.with(|b| b.borrow_mut().clear());
                assert_eq!(function_adapter::proof::pending_invocations(), 0);
            }
            3 => exercise(),
            4 => proof::policy_conformance(|| {
                // New ordinary generic observers retain the physical target while
                // the completed A/B scenario releases its named subscriptions.
                for owner in ["owner-a", "owner-b"] {
                    eval_in_context(
                        owner,
                        "proofSubscription.dispose();proofPostSubscription.dispose();",
                    )
                    .unwrap();
                }
            }),
            5..=8 => {
                let step = STEP.with(Cell::get) - 5;
                assert!(ENTITY_STATE.with(|s| s.borrow().is_none()));
                let state = proof::entity_begin(
                    step >= 2, step % 2 == 1,
                    ENTITY_SLOT.with(Cell::get).unwrap(), true,
                );
                ENTITY_STATE.with(|s| *s.borrow_mut() = Some(state));
            }
            9 | 10 => {
                // Never retain a RefCell borrow across V8/native reentry.
                let mut state = ENTITY_STATE.with(|s| s.borrow_mut().take()).unwrap();
                if STEP.with(Cell::get) == 9 {
                    proof::entity_probe(&mut state);
                } else {
                    proof::entity_advance(&mut state);
                }
                ENTITY_STATE.with(|s| *s.borrow_mut() = Some(state));
            }
            11 => POST_STATE.with(|s|*s.borrow_mut()=Some(proof::post_begin())),
            12 => {
                let mut state=POST_STATE.with(|s|s.borrow_mut().take()).unwrap();
                proof::post_probe(&mut state);POST_STATE.with(|s|*s.borrow_mut()=Some(state));
            }
            13 => {
                proof::post_exercise("js",41,Some(7));
                proof::post_exercise("rust",41,Some(7));
                proof::post_exercise("skip",63,None);
                proof::post_exercise("nested",41,Some(7));
                let peer=POST_PEER.with(Cell::get).unwrap();
                assert_eq!(unsafe{peer(1)},1);proof::post_exercise("earlier-equal",71,Some(7));
                assert_eq!(unsafe{peer(2)},1);proof::post_exercise("earlier-stronger",72,None);
                assert_eq!(unsafe{peer(3)},1);
            }
            14 => proof::post_exercise("later-equal", 41, Some(7)),
            15 => {
                assert_eq!(unsafe { POST_PEER.with(Cell::get).unwrap()(0) }, 1);
                proof::post_finish(POST_STATE.with(|s| s.borrow_mut().take()).unwrap());
            }
            16 => PROCESS_STATE.with(|s| {
                *s.borrow_mut() = Some(super::super::engine_function_tests::package_service_begin())
            }),
            17 => {
                let mut state = PROCESS_STATE.with(|s| s.borrow_mut().take()).unwrap();
                PROCESS_READY.with(|r| {
                    r.set(super::super::engine_function_tests::package_service_ready(
                        &mut state,
                        ENGINE_CALL.with(Cell::get).unwrap(),
                        false,
                    ))
                });
                PROCESS_STATE.with(|s| *s.borrow_mut() = Some(state));
            }
            18 => {
                let mut state = PROCESS_STATE.with(|s| s.borrow_mut().take()).unwrap();
                super::super::engine_function_tests::package_service_add_public(&state);
                assert!(super::super::engine_function_tests::package_service_ready(
                    &mut state,
                    ENGINE_CALL.with(Cell::get).unwrap(),
                    true
                ));
                super::super::engine_function_tests::package_service_exercise(&state);
                PROCESS_STATE.with(|s| *s.borrow_mut() = Some(state));
            }
            19 => super::super::engine_function_tests::package_service_finish(
                PROCESS_STATE.with(|s| s.borrow_mut().take()).unwrap(),
            ),
            20 => {assert_eq!(unsafe{COPY_PEER.with(Cell::get).unwrap()(1)},1);COPY_STATE.with(|s|*s.borrow_mut()=Some(proof::copy_begin()));},
            21 => {let mut state=COPY_STATE.with(|s|s.borrow_mut().take()).unwrap();proof::copy_probe(&mut state);COPY_READY.with(|r|r.set(state.ready));COPY_STATE.with(|s|*s.borrow_mut()=Some(state));},
            22 => {
                proof::copy_exercise();
                unsafe{COPY_PEER.with(Cell::get).unwrap()(2)};proof::copy_peer_exercise();
                assert_eq!(unsafe{COPY_PEER.with(Cell::get).unwrap()(2)},2);unsafe{COPY_PEER.with(Cell::get).unwrap()(3)};
                proof::copy_mode("carry");
                let input=CString::new("raw-input").unwrap();let expected=CString::new("same-copied-result").unwrap();
                assert_eq!(unsafe{COPY_ENGINE.with(Cell::get).unwrap()(input.as_ptr(),expected.as_ptr())},1);
            },
            23 => proof::copy_finish(COPY_STATE.with(|s|s.borrow_mut().take()).unwrap()),
            24 => proof::copy_borrowed_begin(),
            25 => COPY_READY.with(|s|s.set(proof::copy_borrowed_probe())),
            26 => proof::copy_borrowed_finish(),
            27 => COPY_PROCESS.with(|s|*s.borrow_mut()=Some(proof::copy_process_begin())),
            28 => COPY_READY.with(|s|s.set(proof::copy_process_probe())),
            29 => {proof::copy_process_exercise();proof::copy_process_finish(COPY_PROCESS.with(|s|s.borrow_mut().take()).unwrap());},
            30 => {RECORD_STATE.with(|s|*s.borrow_mut()=Some(borrowed_proof::begin(RECORD_ENGINE.with(Cell::get).unwrap(),ENTITY_SLOT.with(Cell::get).unwrap())));},
            31 => {RECORD_READY.with(|r|r.set(RECORD_STATE.with(|s|borrowed_proof::ready(s.borrow().as_ref().unwrap()))));},
            32 => {RECORD_STATE.with(|s|borrowed_proof::exercise(s.borrow().as_ref().unwrap()));assert_eq!(borrowed_proof::call(1)[9],1.);},
            33 => {assert_eq!(unsafe{RECORD_PEER.with(Cell::get).unwrap()(4)},1);borrowed_proof::abort(RECORD_STATE.with(|s|s.borrow_mut().take()).unwrap());},
            34 => {assert_eq!(unsafe{RECORD_PEER.with(Cell::get).unwrap()(5)},1);CURSOR_STATE.with(|s|*s.borrow_mut()=Some(borrowed_proof::cursor_begin(RECORD_ENGINE.with(Cell::get).unwrap())));},
            35 => {RECORD_READY.with(|r|r.set(CURSOR_STATE.with(|s|borrowed_proof::cursor_ready(s.borrow().as_ref().unwrap()))));},
            36 => {borrowed_proof::cursor_exercise(true);},
            37 => {borrowed_proof::cursor_abort(CURSOR_STATE.with(|s|s.borrow_mut().take()).unwrap());assert_eq!(unsafe{RECORD_PEER.with(Cell::get).unwrap()(0)},1);},
            38 => {assert_eq!(unsafe{RECORD_PEER.with(Cell::get).unwrap()(3)},1);},
            39 => {RECORD_READY.with(|r|r.set(borrowed_proof::observers_ready()));},
            40 => {assert_eq!(unsafe{RECORD_PEER.with(Cell::get).unwrap()(1)},1);},
            41 => {
                let mut output=[0.;18];
                assert_eq!(unsafe{RECORD_ENGINE.with(Cell::get).unwrap()(0,output.as_mut_ptr())},1);
                RECORD_READY.with(|r|r.set(output[16..18]==[1.,0.]));
            },
            _ => panic!("unexpected frame callback"),
        });
        if let Err(error) = result {
            let message = error
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or("frame proof panic".into());
            FAILURE.with(|f| *f.borrow_mut() = Some(message));
        }
    }
    #[test]
    #[ignore = "requires real stock provider; run scripts/test-engine-function-v8-adapter.sh --stock-provider"]
    fn production_registry_outer_frame() {
        assert!(cfg!(all(target_os = "linux", target_arch = "x86_64")));
        let path = CString::new(std::env::var("S2FN_V8_BRIDGE").unwrap()).unwrap();
        let library = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        assert!(!library.is_null());
        unsafe fn symbol(lib: *mut libc::c_void, name: &str) -> *mut libc::c_void {
            let name = CString::new(name).unwrap();
            let p = libc::dlsym(lib, name.as_ptr());
            assert!(!p.is_null());
            p
        }
        let create: CreateProduction =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_create")) };
        let frame: FrameProduction =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_frame")) };
        let engine_call: EngineCallProduction =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_engine_call")) };
        ENGINE_CALL.with(|s| s.set(Some(engine_call)));
        COPY_ENGINE.with(|s|s.set(Some(unsafe{std::mem::transmute(symbol(library,"s2fn_production_copy_engine_call"))})));
        COPY_PEER.with(|s|s.set(Some(unsafe{std::mem::transmute(symbol(library,"s2fn_production_copy_peer"))})));
        let copy_borrowed:Remove=unsafe{std::mem::transmute(symbol(library,"s2fn_production_copy_borrowed_call"))};
        let copy_escaped:Remove=unsafe{std::mem::transmute(symbol(library,"s2fn_production_copy_escaped_check"))};
        let empty: Remove =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_empty")) };
        let close: Remove =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_close")) };
        let entity_slot: proof::EntitySlot =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_entity_slot")) };
        ENTITY_SLOT.with(|s| s.set(Some(entity_slot)));
        let peer:FrameProduction=unsafe{std::mem::transmute(symbol(library,"s2fn_production_post_peer_mode"))};
        POST_PEER.with(|s|s.set(Some(peer)));
        RECORD_ENGINE.with(|s|s.set(Some(unsafe{std::mem::transmute(symbol(library,"s2fn_production_record_call"))})));
        RECORD_PEER.with(|s|s.set(Some(unsafe{std::mem::transmute(symbol(library,"s2fn_production_record_peer"))})));
        init(frame_tests::logger).unwrap();
        PACKAGE.with(|p| *p.borrow_mut() = Some(proof::package()));
        FAILURE.with(|f| *f.borrow_mut() = None);
        let mut ops = S2EngineOps::default();
        assert_eq!(
            unsafe {
                create(
                    crate::ffi::s2script_core_dispatch_function,
                    frame_step,
                    &mut ops,
                )
            },
            1
        );
        set_engine_ops(Some(ops));
        let add_peer: Remove =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_add_peer")) };
        let peer_calls: Remove =
            unsafe { std::mem::transmute(symbol(library, "s2fn_production_peer_calls")) };
        let drive = |step| {
            STEP.with(|s| s.set(step));
            assert_eq!(unsafe { frame(1) }, 1);
            assert!(
                FAILURE.with(|f| f.borrow().is_none()),
                "{:?}; bootstrap logs: {:?}",
                FAILURE.with(|f| f.borrow().clone()),
                frame_tests::LOG.lock().unwrap()
            );
        };
        drive(0); // New target first-patched under the real unrelated outer observation.
        assert_eq!(unsafe { add_peer() }, 1);
        drive(1);
        let peer_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { peer_calls() } == 0 && std::time::Instant::now() < peer_deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
            drive(3);
        }
        assert!(
            unsafe { peer_calls() } > 0,
            "external stock peer must observe the live package call"
        );
        drive(4); // Additional policy proof; original A/B scenario already passed independently.
        drive(2);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { empty() } == 0 && std::time::Instant::now() < deadline {
            assert_eq!(unsafe { frame(0) }, 1);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            unsafe { empty() },
            1,
            "normal frames must automatically collect after last core subscriber"
        );
        for step in 5..=8 {
            drive(step); // Identity subscriptions return a genuine outer frame first.
            for stage in [proof::EntityStage::Identity, proof::EntityStage::Member] {
                assert_eq!(ENTITY_STATE.with(|s| s.borrow().as_ref().unwrap().stage), stage);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    drive(9); // Exactly one probe in each later genuine frame.
                    if ENTITY_STATE.with(|s| s.borrow().as_ref().unwrap().ready) {
                        break;
                    }
                    assert!(std::time::Instant::now() < deadline, "entity readiness timeout: {}",
                        ENTITY_STATE.with(|s| s.borrow().as_ref().unwrap().detail.clone()));
                    // The real frame and every checked observation have returned.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                drive(10); // Independent semantics; identity stage registers member then returns.
            }
            let state = ENTITY_STATE.with(|s| s.borrow_mut().take()).unwrap();
            assert_eq!(state.stage, proof::EntityStage::Done);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while unsafe { empty() } == 0 && std::time::Instant::now() < deadline {
                assert_eq!(unsafe { frame(0) }, 1);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(
                unsafe { empty() },
                1,
                "entity scenario must retire before next preparation order"
            );
        }
        drive(11);
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
        loop {
            drive(12);
            if POST_STATE.with(|s|s.borrow().as_ref().unwrap().ready) {break;}
            assert!(std::time::Instant::now()<deadline,"trusted POST readiness timeout");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        drive(13);
        // Prove actual peer delivery; an accepted pending hook is insufficient.
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
        loop {
            drive(14);
            if unsafe{peer(3)}==2 {break;}
            assert!(std::time::Instant::now()<deadline,"later equal peer delivery timeout");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        drive(15);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { empty() } == 0 && std::time::Instant::now() < deadline {
            assert_eq!(unsafe { frame(0) }, 1);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(unsafe { empty() }, 1);
        let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            frame_tests::LOG.lock().unwrap().clear();
            PROCESS_READY.with(|r| r.set(false));
            drive(16);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                drive(17);
                if PROCESS_READY.with(Cell::get) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "process package hook readiness timeout: {}",
                    PROCESS_STATE.with(|s| s.borrow().as_ref().unwrap().detail.clone())
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            drive(18);
            drive(19);
            println!("PASS actual compiler-authored target engine entry: package-only and mixed public/package PRE/POST, owner0/no-nest, shared native Service");
        }));
        let copy_result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
            // Retire previous physical identity targets before changing projection representation.
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            while unsafe{empty()}==0 && std::time::Instant::now()<deadline {assert_eq!(unsafe{frame(0)},1);std::thread::sleep(std::time::Duration::from_millis(1));}
            assert_eq!(unsafe{empty()},1);
            drive(20);let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(21);if COPY_READY.with(Cell::get){break}assert!(std::time::Instant::now()<deadline,"copied hook readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            drive(22);assert_eq!(unsafe{copy_escaped()},1);drive(23);assert_eq!(unsafe{copy_escaped()},1);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            while unsafe{empty()}==0 && std::time::Instant::now()<deadline {assert_eq!(unsafe{frame(0)},1);std::thread::sleep(std::time::Duration::from_millis(1));}
            assert_eq!(unsafe{empty()},1);COPY_READY.with(|s|s.set(false));drive(24);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(25);if COPY_READY.with(Cell::get){break}assert!(std::time::Instant::now()<deadline,"borrowed copy readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            assert_eq!(unsafe{copy_borrowed()},9);drive(26);assert_eq!(unsafe{copy_escaped()},1);
            COPY_READY.with(|s|s.set(false));drive(27);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(28);if COPY_READY.with(Cell::get){break}assert!(std::time::Instant::now()<deadline,"process copy readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            drive(29);assert_eq!(unsafe{copy_escaped()},1);
            println!("PASS actual Service/V8 copied aliases, recall edits, suppression, strict marshalling, exact carried deliveries, nested lease expiry, POST original/effective returns and escaped native reads after owner retirement");
        }));
        let record_result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            while unsafe{empty()}==0 && std::time::Instant::now()<deadline {assert_eq!(unsafe{frame(0)},1);std::thread::sleep(std::time::Duration::from_millis(1));}
            assert_eq!(unsafe{empty()},1);drive(40);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(41);if RECORD_READY.with(Cell::get){break;}assert!(std::time::Instant::now()<deadline,"record later observer readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            drive(30);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(31);if RECORD_READY.with(Cell::get){break;}assert!(std::time::Instant::now()<deadline,"record Service/peer readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            // Registration acceptance does not establish physical order: the
            // provider may requeue an insertion. Observe Service before adding
            // the early paired observer, then observe both before assertions.
            drive(38);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(39);if RECORD_READY.with(Cell::get){break;}assert!(std::time::Instant::now()<deadline,"record early observer readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            drive(32);drive(33);
            // Keep the later paired observer installed. Drain the early observer
            // before reinstalling it after the new Service hook.
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            while unsafe{empty()}==0 && std::time::Instant::now()<deadline {assert_eq!(unsafe{frame(0)},1);std::thread::sleep(std::time::Duration::from_millis(1));}
            assert_eq!(unsafe{empty()},1);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(41);if RECORD_READY.with(Cell::get){break;}assert!(std::time::Instant::now()<deadline,"record cursor retained later observer readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            drive(34);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(35);if RECORD_READY.with(Cell::get){break;}assert!(std::time::Instant::now()<deadline,"record cursor Service readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            drive(38);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            loop{drive(39);if RECORD_READY.with(Cell::get){break;}assert!(std::time::Instant::now()<deadline,"record cursor early observer readiness timeout");std::thread::sleep(std::time::Duration::from_millis(1));}
            drive(36);drive(37);
        }));
        if let Some(state)=RECORD_STATE.with(|s|s.borrow_mut().take()){borrowed_proof::abort(state);}
        if let Some(state)=CURSOR_STATE.with(|s|s.borrow_mut().take()){borrowed_proof::cursor_abort(state);}
        unsafe{RECORD_PEER.with(Cell::get).unwrap()(0)};
        RECORD_ENGINE.with(|s|s.set(None));RECORD_PEER.with(|s|s.set(None));
        if let Some(state)=COPY_STATE.with(|s|s.borrow_mut().take()){proof::copy_abort(state);}
        for id in ["copy-vector","copy-borrowed","copy-borrowed-caller"]{unload_plugin(id);}
        if let Some(state)=COPY_PROCESS.with(|s|s.borrow_mut().take()){proof::copy_process_abort(state);}
        COPY_PEER.with(|s|s.set(None));
        COPY_ENGINE.with(|s|s.set(None));
        // Drain RAII owners while the isolate, native Service and all TLS maps
        // are still alive. Thread-local destruction must not mask the first panic.
        let abandoned = PROCESS_STATE.with(|s| s.borrow_mut().take());
        if let Some(state) = abandoned {
            super::super::engine_function_tests::package_service_abort(state);
        }
        if process_result.is_err() {
            for id in ["process-a", "process-b", "process-plugin"] {
                unload_plugin(id);
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { empty() } == 0 && std::time::Instant::now() < deadline {
            assert_eq!(unsafe { frame(0) }, 1);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let process_drained = unsafe { empty() } == 1;
        ENGINE_CALL.with(|s| s.set(None));
        POST_PEER.with(|s| s.set(None));
        ENTITY_SLOT.with(|s| s.set(None));
        let closed = unsafe { close() };
        PACKAGE.with(|p| p.borrow_mut().take());
        set_engine_ops(None);
        shutdown();
        if closed == 1 {
            if copy_result.is_ok(){assert_eq!(unsafe{copy_escaped()},1);}
            unsafe {
                libc::dlclose(library);
            }
        }
        if let Err(error) = process_result {
            eprintln!("process failure cleanup: drained={process_drained} closed={closed}");
            std::panic::resume_unwind(error);
        }
        if let Err(error)=copy_result{std::panic::resume_unwind(error);}
        if let Err(error)=record_result{std::panic::resume_unwind(error);}
        assert!(
            process_drained,
            "process package native resources did not retire"
        );
        assert_eq!(closed, 1);
        println!("PASS production Service/sink/strong Rust export, real checked outer frames, unload/reload, never-Active cleanup and no-core-frame-subscriber native maintenance");
    }
}
