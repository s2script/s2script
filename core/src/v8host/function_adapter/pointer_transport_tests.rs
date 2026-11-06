//! The `ptr` projection end to end through a declared engine function: a JS `Ptr` crosses as its
//! address, the native result comes back as a `Ptr`, null is a null pointer, and anything that is
//! not a `Ptr` (a bare number included) is refused before the engine is called.
use super::*;
use std::cell::Cell;

thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static SEEN: Cell<u64> = const { Cell::new(u64::MAX) };
}

extern "C" fn prepare(_: *const i8, _: *const i8, _: *const i8, _: *const i8, _: *mut i8, _: i32) -> i64 {
    11
}
extern "C" fn release(_: i64) -> i32 {
    1
}
/// The engine function returns its argument advanced by 8 bytes (null stays null).
extern "C" fn call(_: i64, _: u64, args: *const S2FunctionValue, argc: i32, out: *mut S2FunctionValue, _: *mut i8, _: i32) -> i32 {
    CALLS.with(|c| c.set(c.get() + 1));
    let input = unsafe { *args };
    let request = unsafe { *out };
    if argc != 1 || input.kind != 8 || input.flags != projection::POINTER_FLAG
        || request.kind != 8 || request.flags != projection::POINTER_FLAG
    {
        return 0;
    }
    SEEN.with(|s| s.set(input.bits));
    unsafe { *out = S2FunctionValue { bits: if input.bits == 0 { 0 } else { input.bits + 8 }, ..request } };
    1
}

#[test]
fn ptr_arguments_and_returns_cross_as_ptr() {
    init(frame_tests::dummy_logger()).unwrap();
    set_engine_ops(Some(S2EngineOps {
        function_prepare: Some(prepare),
        function_call: Some(call),
        function_target_release: Some(release),
        ..Default::default()
    }));
    frame_tests::load_body("ptr-call", "return {};", "{}");
    proof::prepared_binding("ptr-call", |f| {
        f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:ptr(ptr)".into();
        f["abi"]["parameters"] = serde_json::json!([{"name":"base","native":"ptr","projection":{"id":"ptr","version":1},"mutable":[]}]);
        f["abi"]["returns"] = serde_json::json!({"native":"ptr","projection":{"id":"ptr","version":1}});
        f["policy"]["surfaces"] = serde_json::json!(["call"]);
        f["policy"]["suppression"] = "none".into();
    });
    let out = frame_tests::eval_in_context_string("ptr-call", r#"
        (function () {
          const { Ptr, Engine } = __s2pkg_unsafe;
          const f = Engine.function('fire');
          const r = f.call(Ptr.from(0x20000));
          const refused = [0x20000, "0x20000", {}, { address: 0x20000 }].filter(function (v) {
            try { f.call(v); return false; } catch (_) { return true; }
          }).length;
          return [r instanceof Ptr, r.address === 0x20008, f.call(null) === null, refused].join(",");
        })()
    "#);
    assert_eq!(out, "true,true,true,4");
    // Two engine calls ran (the Ptr and the null); the refused values never reached it.
    assert_eq!(CALLS.with(Cell::get), 2);
    assert_eq!(SEEN.with(Cell::get), 0);
    shutdown();
    set_engine_ops(None);
}

#[test]
fn ptr_projection_transport_is_range_checked() {
    let request = projection::request("ptr", "ptr").unwrap();
    assert_eq!((request.kind, request.flags), (8, projection::POINTER_FLAG));
    assert!(projection::request("u64", "ptr").is_err());
    for bits in [0u64, 65536, (1 << 47) - 1] {
        let wire = S2FunctionValue { bits, ..request };
        assert!(matches!(projection::decode(wire, "ptr", "ptr"), Ok(ProjectedValue::Pointer(b)) if b == bits));
        assert_eq!(projection::encode(ProjectedValue::Pointer(bits)).unwrap().bits, bits);
    }
    for bits in [1u64, 4096, 1 << 47, u64::MAX] {
        assert!(projection::decode(S2FunctionValue { bits, ..request }, "ptr", "ptr").is_err());
        assert!(projection::encode(ProjectedValue::Pointer(bits)).is_err());
    }
    // An entity identity is never read as a raw address, nor the reverse.
    assert!(projection::decode(S2FunctionValue { flags: 1, aux: 3, bits: 7, ..request }, "ptr", "ptr").is_err());
    assert!(projection::decode(S2FunctionValue { aux: 1, ..request }, "ptr", "ptr").is_err());
}
