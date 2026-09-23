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
fn record(event: &str) { ORDER.with(|o| o.borrow_mut().push(event.into())); }
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
    impl Drop for Pop { fn drop(&mut self) { BYPASS.with(|b| { b.borrow_mut().pop(); }); } }
    let _pop = Pop;
    let mut output = 0;
    let call = CALL.with(|c| c.get().unwrap());
    let ok = crate::nest::with_outbound(&args, || unsafe { call(generation, input, &mut output) });
    assert_eq!(ok, 1);
    rv.set_int32(output);
}
// This callback is reached only via stock KHook -> actual libffi PRE closure.
extern "C" fn inbound(phase: i32, caller: u64, input: i32, output: *mut i32) -> i32 {
    if phase == 1 { record("return"); return 0; }
    record("KHook-PRE");
    assert!(HOST.with(|h| h.try_borrow_mut().is_err()), "A must still be executing");
    let info = crate::nest::top().filter(|p| !p.is_null()).expect("real outbound nest token");
    let instances = INSTANCES.with(|i| i.borrow().clone());
    let bypass = BYPASS.with(|b| b.borrow().last().cloned().unwrap());
    assert_eq!(bypass.1, caller);
    let mut storage = unsafe { v8::CallbackScope::new(&*info) };
    let mut callback_scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let parent = &mut callback_scope;
    let mut selected = 0;
    for instance in instances {
        if (instance.owner.clone(), instance.generation) == bypass { continue; }
        if !owner_is_live(&instance.owner, instance.generation) { continue; }
        assert_eq!(instance.contract, 0x5332464e);
        let context = clone_plugin_context(&instance.owner).unwrap();
        let ctx = v8::Local::new(parent, &context);
        let scope = &mut v8::ContextScope::new(parent, ctx);
        let wrapper = v8::Local::new(scope, &instance.wrapper);
        let argument = v8::Integer::new(scope, input);
        let receiver = v8::undefined(scope).into();
        let decision = wrapper.call(scope, receiver, &[argument.into()]).expect("B wrapper ran synchronously");
        let object = decision.to_object(scope).expect("typed test decision");
        let action_key = v8::String::new(scope, "action").unwrap();
        let action = object.get(scope, action_key.into()).unwrap();
        assert!(action.is_int32()); assert_eq!(action.int32_value(scope), Some(2));
        let value_key = v8::String::new(scope, "returnValue").unwrap();
        let value = object.get(scope, value_key.into()).unwrap();
        assert!(value.is_int32());
        unsafe { *output = value.int32_value(scope).unwrap(); }
        selected += 1;
        println!("selected package=B owner={} generation={} contract={:x}", instance.owner, instance.generation, instance.contract);
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
        let mut host = h.borrow_mut(); let host = host.as_mut().unwrap();
        let context = clone_plugin_context(owner).unwrap();
        let mut storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let ctx = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, ctx);
        let global = ctx.global(scope);
        let event = v8::Function::new(scope, js_event).unwrap();
        let key = v8::String::new(scope, "__probeEvent").unwrap(); global.set(scope, key.into(), event.into());
        let call = v8::Function::new(scope, js_call).unwrap();
        let key = v8::String::new(scope, "__probeCall").unwrap(); global.set(scope, key.into(), call.into());
    });
    let script = format!("globalThis.__packageInstance = Object.freeze({{ contract: 0x5332464e, owner: '{owner}', generation: {generation} }}); globalThis.__wrapper = function(input) {{ if (__packageInstance.owner !== 'owner-b') throw Error('busy A selected'); __probeEvent('B-wrapper'); return {{action: 2, returnValue: input + 70}}; }};");
    eval_in_context(owner, &script).unwrap();
    HOST.with(|h| {
        let mut host = h.borrow_mut(); let host = host.as_mut().unwrap();
        let context = clone_plugin_context(owner).unwrap();
        let mut storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let ctx = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, ctx);
        let key = v8::String::new(scope, "__wrapper").unwrap();
        let value = ctx.global(scope).get(scope, key.into()).unwrap();
        let wrapper = v8::Local::<v8::Function>::try_from(value).unwrap();
        let instance = Instance { owner: owner.into(), generation, contract: 0x5332464e, wrapper: v8::Global::new(scope, wrapper) };
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
        let name = CString::new(name).unwrap(); let p = libc::dlsym(lib, name.as_ptr()); assert!(!p.is_null()); p
    }
    let create: Create = unsafe { std::mem::transmute(symbol(library, "s2fn_probe_create")) };
    let call: Call = unsafe { std::mem::transmute(symbol(library, "s2fn_probe_call")) };
    let remove: Remove = unsafe { std::mem::transmute(symbol(library, "s2fn_probe_remove")) };
    init(frame_tests::dummy_logger()).unwrap(); CALL.with(|c| c.set(Some(call)));
    install("owner-a"); install("owner-b");
    let b_generation = plugin_generation("owner-b");
    assert_eq!(unsafe { create(inbound) }, 1);
    let a_first = plugin_generation("owner-a");
    for run in 0..2 {
        ORDER.with(|o| o.borrow_mut().clear());
        eval_in_context("owner-a", "__probeEvent('A-before'); const result = __probeCall(7); if(result !== 77) throw Error('typed decision was lost'); __probeEvent('A-after');").unwrap();
        ORDER.with(|o| assert_eq!(&*o.borrow(), &["A-before", "KHook-PRE", "B-wrapper", "return", "A-after"]));
        println!("PASS synchronous order A-before -> KHook-PRE -> B-wrapper -> return -> A-after result=77 run={run}");
        if run == 0 {
            remove_instance("owner-a"); assert!(!owner_is_live("owner-a", a_first));
            assert!(owner_is_live("owner-b", b_generation));
            install("owner-a"); assert_ne!(plugin_generation("owner-a"), a_first);
        }
    }
    remove_instance("owner-a"); assert!(owner_is_live("owner-b", b_generation));
    remove_instance("owner-b");
    assert_eq!(unsafe { remove() }, 1); CALL.with(|c| c.set(None));
    shutdown(); unsafe { libc::dlclose(library); }
    println!("PASS real V8 owner-only bypass and independent A/B generation teardown");
}
