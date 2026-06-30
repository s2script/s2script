//! V8 engine host: platform init-once, per-init isolate+context, thread-local state.
//!
//! # Safety / design notes
//! - The V8 platform is initialized exactly once per process (via `Once`) and is never
//!   torn down.  The cdylib is linked `-Wl,-z,nodelete` so the address stays live for
//!   the process lifetime, making the platform leak intentional and safe.
//! - `HOST` and `LOGGER` are `thread_local!` because the engine is only ever driven from
//!   a single dedicated main thread.
//! - All public fns are called exclusively from `ffi.rs` which wraps them in
//!   `catch_unwind`; panics must not cross the FFI boundary.
//! - `eval` is intentionally an arbitrary-JS-execution surface — it is the purpose of
//!   this crate (CS2 game scripting engine).

use crate::multiplexer::{self, Descriptor, DetourChange, HookResult, Phase, Priority};
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::Once;

pub type LogFn = extern "C" fn(c_int, *const c_char);

/// Native callback the host calls to ask the embedder to install/remove the
/// underlying engine detour for `OnGameFrame`.  `enable != 0` => install.
///
/// Defined here (not in `ffi.rs`) so `v8host` has no forward reference into the
/// FFI layer; Task 4's `ffi.rs` wires the real callback via `set_hook_request`.
pub type HookRequestFn = extern "C" fn(descriptor: *const c_char, enable: c_int);

static PLATFORM_INIT: Once = Once::new();

/// A JS handler stored as a persistent function reference.  `Clone` is required
/// because `Descriptor::snapshot` clones each `H`; `v8::Global` clone is a cheap
/// refcount bump.
#[derive(Clone)]
struct JsHandler {
    func: v8::Global<v8::Function>,
}

thread_local! {
    static LOGGER: std::cell::Cell<Option<LogFn>> = std::cell::Cell::new(None);
    static HOST: std::cell::RefCell<Option<Host>> = std::cell::RefCell::new(None);
    /// The single `OnGameFrame` descriptor / per-descriptor subscription registry.
    static FRAME: std::cell::RefCell<Descriptor<JsHandler>> =
        std::cell::RefCell::new(Descriptor::new("OnGameFrame"));
    /// Embedder callback for detour install/remove.  `None` until `set_hook_request`
    /// is called (Task 4); while `None`, `apply_detour` is a safe no-op.
    static HOOK_REQUEST: std::cell::Cell<Option<HookRequestFn>> = std::cell::Cell::new(None);
}

/// Install the embedder's detour-request callback.  Wired by `ffi.rs` (Task 4).
pub fn set_hook_request(f: Option<HookRequestFn>) {
    HOOK_REQUEST.with(|c| c.set(f));
}

/// Provisional JS prelude installed once per context, AFTER the native primitives
/// and `console` are in place.  Defines the user-facing `onGameFrame` plus the
/// `HookResult` / `Priority` / `Phase` enum-like globals.
const PRELUDE: &str = r#"
globalThis.HookResult = { Continue:0, Changed:1, Handled:2, Stop:3 };
globalThis.Priority   = { High:"high", Normal:"normal", Low:"low", Monitor:"monitor" };
globalThis.Phase      = { Pre:"pre", Post:"post" };
globalThis.onGameFrame = (fn, opts) => {
  const id = __s2_subscribe("OnGameFrame", fn, opts || {});
  return { dispose: () => __s2_unsubscribe(id) };
};
"#;

/// Initialize the V8 platform exactly once for the process.  Never torn down.
fn ensure_platform() {
    PLATFORM_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

struct Host {
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
}

/// The `console.log` implementation installed on every new context.
///
/// Signature matches the HRTB required by `MapFnTo<FunctionCallback>` in v8 150:
///   `for<'s, 'i> Fn(&mut PinScope<'s, 'i>, FunctionCallbackArguments<'s>, ReturnValue<'s, Value>)`
///
/// The body is wrapped in `catch_unwind(AssertUnwindSafe(...))` because this
/// function is invoked as a V8 `FunctionCallback` from C++.  A Rust panic that
/// unwinds through V8's C++ frames is undefined behaviour (spec §6: no panic
/// may cross the FFI boundary).  Swallowing the panic here is safe: the log
/// output is simply lost for that call, which is acceptable.
fn console_log(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let msg = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            String::new()
        };
        LOGGER.with(|l| {
            if let Some(f) = l.get() {
                if let Ok(c) = CString::new(msg) {
                    f(0, c.as_ptr());
                }
            }
        });
    }));
}

/// Native `__s2_subscribe(name, fn, opts) -> id`.  Installed on the global object.
///
/// Like `console_log`, the body runs under `catch_unwind` because it is invoked
/// as a V8 `FunctionCallback` from C++: a Rust panic must never unwind across the
/// FFI boundary.  Note this does NOT touch `HOST` — it works entirely from the
/// `scope` V8 hands it — and the only thread-local it borrows is `FRAME`, so it is
/// safe to call re-entrantly from inside `dispatch_onframe` (which holds `HOST` but
/// not `FRAME`).
fn s2_subscribe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 {
            return;
        }
        // arg0: descriptor name — only "OnGameFrame" is supported.
        if args.get(0).to_rust_string_lossy(scope) != "OnGameFrame" {
            return;
        }
        // arg1: the handler function, stored as a persistent Global.
        let func_local = match v8::Local::<v8::Function>::try_from(args.get(1)) {
            Ok(f) => f,
            Err(_) => return,
        };
        let global = v8::Global::new(scope.as_ref(), func_local);

        // arg2: optional { priority, phase } strings → enums (defaults Normal / Pre).
        let mut priority = Priority::Normal;
        let mut phase = Phase::Pre;
        if args.length() >= 3 {
            if let Ok(opts) = v8::Local::<v8::Object>::try_from(args.get(2)) {
                if let Some(k) = v8::String::new(scope, "priority") {
                    if let Some(v) = opts.get(scope, k.into()) {
                        if v.is_string() {
                            priority = match v.to_rust_string_lossy(scope).as_str() {
                                "high" => Priority::High,
                                "low" => Priority::Low,
                                "monitor" => Priority::Monitor,
                                _ => Priority::Normal,
                            };
                        }
                    }
                }
                if let Some(k) = v8::String::new(scope, "phase") {
                    if let Some(v) = opts.get(scope, k.into()) {
                        if v.is_string() {
                            phase = match v.to_rust_string_lossy(scope).as_str() {
                                "post" => Phase::Post,
                                _ => Phase::Pre,
                            };
                        }
                    }
                }
            }
        }

        let (id, change) =
            FRAME.with(|f| f.borrow_mut().subscribe(priority, phase, JsHandler { func: global }));
        apply_detour(change);
        rv.set_double(id as f64);
    }));
}

/// Native `__s2_unsubscribe(id)`.  Installed on the global object.
fn s2_unsubscribe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 {
            return;
        }
        let id = args.get(0).integer_value(scope).unwrap_or(0) as multiplexer::SubId;
        let change = FRAME.with(|f| f.borrow_mut().unsubscribe(id));
        apply_detour(change);
    }));
}

/// Forward a detour change to the embedder, if one is registered.  No-op when no
/// `HOOK_REQUEST` callback is installed (e.g. all of Task 3's tests).
fn apply_detour(change: DetourChange) {
    if let DetourChange::None = change {
        return;
    }
    HOOK_REQUEST.with(|c| {
        if let Some(req) = c.get() {
            let enable = matches!(change, DetourChange::Install) as c_int;
            let name = CString::new("OnGameFrame").unwrap();
            req(name.as_ptr(), enable);
        }
    });
}

pub fn init(logger: LogFn) -> Result<(), String> {
    ensure_platform();
    LOGGER.with(|l| l.set(Some(logger)));

    let mut isolate = v8::Isolate::new(v8::CreateParams::default());

    // Build the context inside a nested block so the HandleScope borrow on
    // `isolate` is released before we move `isolate` into `Host`.
    let context = {
        // v8 150: HandleScope::new() returns a ScopeStorage that must be pinned
        // before use.  The unsafe Pin is sound because `hs_storage` is never
        // moved after this point (it is immediately shadowed by the PinnedRef).
        let mut hs_storage = v8::HandleScope::new(&mut isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        // hs: &mut PinScope<'_, '_, ()>  (HandleScope without a context yet)

        // Context::new takes &PinScope<'s, '_, ()> — passes through auto-deref.
        let ctx_local = v8::Context::new(hs, Default::default());

        // ContextScope::new casts the inner HandleScope from ()  →  Context type.
        // After this, `scope` derefs to PinScope<'_, '_, Context>.
        // ctx_local is Copy so it is copied into ContextScope::new, remaining
        // available for use below.
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Install `console = { log: fn }` on the global object.
        let global_obj = ctx_local.global(scope);
        let console_obj = v8::Object::new(scope);
        let log_key = v8::String::new(scope, "log").unwrap();
        let log_fn = v8::Function::new(scope, console_log).unwrap();
        console_obj.set(scope, log_key.into(), log_fn.into());
        let console_key = v8::String::new(scope, "console").unwrap();
        global_obj.set(scope, console_key.into(), console_obj.into());

        // Install the native multiplexer primitives on the global object.
        let sub_key = v8::String::new(scope, "__s2_subscribe").unwrap();
        let sub_fn = v8::Function::new(scope, s2_subscribe).unwrap();
        global_obj.set(scope, sub_key.into(), sub_fn.into());
        let unsub_key = v8::String::new(scope, "__s2_unsubscribe").unwrap();
        let unsub_fn = v8::Function::new(scope, s2_unsubscribe).unwrap();
        global_obj.set(scope, unsub_key.into(), unsub_fn.into());

        // scope.as_ref() gives &Isolate (via AsRef<Isolate> for ContextScope).
        v8::Global::new(scope.as_ref(), ctx_local)
        // scope, hs, hs_storage drop here — borrow on isolate is released.
    };

    HOST.with(|h| *h.borrow_mut() = Some(Host { isolate, context }));

    // Provisional prelude — defines `onGameFrame` etc. on top of the natives.
    eval(PRELUDE)?;
    Ok(())
}

pub fn eval(src: &str) -> Result<(), String> {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow
            .as_mut()
            .ok_or_else(|| "s2script_core_eval called before init".to_string())?;

        // Create HandleScope from the stored OwnedIsolate.
        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;

        // Materialise the stored Global<Context> into a Local for the scope.
        let ctx_local = v8::Local::new(hs, &host.context);

        // Enter the context.  The ContextScope upgrades the inner HandleScope
        // type parameter from ()  →  Context, which is required by Script::compile,
        // to_rust_string_lossy, and similar APIs.
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Wrap in TryCatch so JS exceptions are caught rather than panicking.
        // TryCatch also requires pinning in v8 150.
        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;
        // tc: &mut PinnedRef<'_, TryCatch<'_, 'obj, HandleScope<'iso, Context>>>
        // *tc (via Deref): PinnedRef<'obj, HandleScope<'iso, Context>>  ← PinScope

        let code = v8::String::new(tc, src)
            .ok_or_else(|| "failed to intern source string in V8".to_string())?;

        let script = match v8::Script::compile(tc, code, None) {
            Some(s) => s,
            None => {
                return Err(tc
                    .exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "unknown JavaScript error (compile)".into()));
            }
        };

        match script.run(tc) {
            Some(_) => Ok(()),
            None => Err(tc
                .exception()
                .map(|e| e.to_rust_string_lossy(&*tc))
                .unwrap_or_else(|| "unknown JavaScript error (run)".into())),
        }
    })
}

/// Dispatch one `OnGameFrame` tick to all enabled JS handlers for `phase`.
///
/// **Three-phase borrow split (load-bearing for re-entrancy):**
/// - Phase 1: borrow `FRAME` only long enough to clone the ordered snapshot, then
///   RELEASE it.
/// - Phase 2: borrow `HOST` (for the V8 scope) and run the chain.  `FRAME` is NOT
///   borrowed here, so a handler that calls `onGameFrame(...)` re-enters
///   `__s2_subscribe` → `FRAME.borrow_mut()` without a double-borrow panic.
/// - Phase 3: briefly borrow `FRAME` mutably for error/auto-disable bookkeeping.
pub(crate) fn dispatch_onframe(
    phase: Phase,
    simulating: bool,
    first: bool,
    last: bool,
) -> multiplexer::DispatchOutcome {
    use crate::multiplexer::{run_chain, DispatchOutcome};

    // Phase 1 — brief &FRAME borrow: clone the ordered enabled handlers, then release.
    let snap = FRAME.with(|f| f.borrow().snapshot(phase));
    if snap.is_empty() {
        return DispatchOutcome {
            result: HookResult::Continue,
            detour: DetourChange::None,
        };
    }

    // Phase 2 — invoke under the V8 context.  HOST is borrowed; FRAME is NOT.
    let outcome = HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow.as_mut().expect("dispatch_onframe before init");

        // Open HandleScope + ContextScope on the stored isolate/context (mirrors `eval`).
        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &host.context);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Build the per-frame `ctx` object once: { simulating, firstTick, lastTick, phase }.
        let ctx_obj = v8::Object::new(scope);
        let k = v8::String::new(scope, "simulating").unwrap();
        let v = v8::Boolean::new(scope, simulating);
        ctx_obj.set(scope, k.into(), v.into());
        let k = v8::String::new(scope, "firstTick").unwrap();
        let v = v8::Boolean::new(scope, first);
        ctx_obj.set(scope, k.into(), v.into());
        let k = v8::String::new(scope, "lastTick").unwrap();
        let v = v8::Boolean::new(scope, last);
        ctx_obj.set(scope, k.into(), v.into());
        let k = v8::String::new(scope, "phase").unwrap();
        let v = v8::String::new(scope, if phase == Phase::Pre { "pre" } else { "post" }).unwrap();
        ctx_obj.set(scope, k.into(), v.into());

        let recv: v8::Local<v8::Value> = v8::undefined(scope).into();
        let ctx_val: v8::Local<v8::Value> = ctx_obj.into();

        run_chain(&snap, |jh: &JsHandler| {
            // Per-handler TryCatch isolates a throwing handler from the rest of the chain.
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let func = v8::Local::new(tc, &jh.func);
            match func.call(tc, recv, &[ctx_val]) {
                // Exception thrown (or otherwise empty): treat as an error for this id.
                None => Err(()),
                Some(ret) => {
                    if ret.is_undefined() {
                        Ok(HookResult::Continue)
                    } else {
                        Ok(match ret.uint32_value(tc) {
                            Some(1) => HookResult::Changed,
                            Some(2) => HookResult::Handled,
                            Some(3) => HookResult::Stop,
                            _ => HookResult::Continue,
                        })
                    }
                }
            }
        })
    });

    // Phase 3 — brief &mut FRAME borrow: error/auto-disable bookkeeping; act on any Remove.
    let detour = FRAME.with(|f| f.borrow_mut().apply_errors(&outcome.errored));
    apply_detour(detour);
    DispatchOutcome {
        result: outcome.result,
        detour,
    }
}

pub fn shutdown() {
    // Drop the isolate and context.  The platform is never torn down.
    HOST.with(|h| {
        let _ = h.borrow_mut().take();
    });
    // Reset the descriptor so a re-init starts with a clean, empty registry.
    FRAME.with(|f| *f.borrow_mut() = Descriptor::new("OnGameFrame"));
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::multiplexer::{Phase, HookResult};
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};
    use std::sync::Mutex;

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
    extern "C" fn logger(_l: c_int, m: *const c_char) {
        LOG.lock().unwrap().push(unsafe { CStr::from_ptr(m) }.to_string_lossy().into_owned());
    }

    #[test]
    fn two_js_handlers_compose_on_onframe() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // High-priority logs "high"; Normal logs "normal". Both Pre. Console logs prove order.
        eval(r#"
            onGameFrame((f) => { console.log("high:" + f.firstTick); }, { priority: "high" });
            onGameFrame((f) => { console.log("normal"); });
        "#).unwrap();

        let out = dispatch_onframe(Phase::Pre, true, true, false);
        assert_eq!(out.result, HookResult::Continue);
        let got = LOG.lock().unwrap().clone();
        let hi = got.iter().position(|m| m.contains("high:true"));
        let no = got.iter().position(|m| m.contains("normal"));
        assert!(hi.is_some() && no.is_some() && hi < no, "order wrong: {:?}", got);
        shutdown();
    }

    #[test]
    fn stop_at_high_skips_low_handler() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval(r#"
            onGameFrame(() => { console.log("h"); return HookResult.Stop; }, { priority: "high" });
            onGameFrame(() => { console.log("l"); }, { priority: "low" });
        "#).unwrap();
        let out = dispatch_onframe(Phase::Pre, true, false, false);
        assert_eq!(out.result, HookResult::Stop);
        let got = LOG.lock().unwrap().clone();
        assert!(got.iter().any(|m| m == "h"));
        assert!(!got.iter().any(|m| m == "l"), "low must be skipped: {:?}", got);
        shutdown();
    }

    #[test]
    fn throwing_handler_is_isolated() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval(r#" onGameFrame(() => { throw new Error("boom"); }); "#).unwrap();
        // Must not panic / crash; result stays Continue.
        let out = dispatch_onframe(Phase::Pre, true, false, false);
        assert_eq!(out.result, HookResult::Continue);
        shutdown();
    }

    #[test]
    fn handler_that_subscribes_during_dispatch_does_not_panic_and_runs_next_frame() {
        // The re-entrancy guarantee: a JS handler that calls onGameFrame(...) DURING dispatch
        // re-enters __s2_subscribe (which borrows FRAME). dispatch_onframe must NOT hold the FRAME
        // borrow across invocation, or this double-borrows the RefCell and panics.
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval(r#"
            let added = false;
            onGameFrame(() => {
                console.log("outer");
                if (!added) { added = true; onGameFrame(() => console.log("inner")); }
            });
        "#).unwrap();
        // Frame 1: only "outer" runs; it subscribes "inner" mid-dispatch (must not panic).
        dispatch_onframe(Phase::Pre, true, false, false);
        // Frame 2: both run (the snapshot now includes "inner").
        dispatch_onframe(Phase::Pre, true, false, false);
        let got = LOG.lock().unwrap().clone();
        assert_eq!(got.iter().filter(|m| *m == "outer").count(), 2);
        assert_eq!(got.iter().filter(|m| *m == "inner").count(), 1); // not run frame 1, run frame 2
        shutdown();
    }
}
