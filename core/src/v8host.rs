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

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::Once;

pub type LogFn = extern "C" fn(c_int, *const c_char);

static PLATFORM_INIT: Once = Once::new();

thread_local! {
    static LOGGER: std::cell::Cell<Option<LogFn>> = std::cell::Cell::new(None);
    static HOST: std::cell::RefCell<Option<Host>> = std::cell::RefCell::new(None);
}

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

        // scope.as_ref() gives &Isolate (via AsRef<Isolate> for ContextScope).
        v8::Global::new(scope.as_ref(), ctx_local)
        // scope, hs, hs_storage drop here — borrow on isolate is released.
    };

    HOST.with(|h| *h.borrow_mut() = Some(Host { isolate, context }));
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

pub fn shutdown() {
    // Drop the isolate and context.  The platform is never torn down.
    HOST.with(|h| {
        let _ = h.borrow_mut().take();
    });
}
