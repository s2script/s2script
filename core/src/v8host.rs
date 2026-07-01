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

use crate::async_rt::{Pool, TimerKind, TimerQueue};
use crate::multiplexer::{self, Descriptor, DetourChange, HookResult, Phase, Priority};
use crate::plugin;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

static POOL: OnceLock<Pool> = OnceLock::new();
fn pool() -> &'static Pool {
    POOL.get_or_init(|| Pool::new(4))
}

pub type LogFn = extern "C" fn(c_int, *const c_char);

/// Native callback the host calls to ask the embedder to install/remove the
/// underlying engine detour for `OnGameFrame`.  `enable != 0` => install.
///
/// Defined here (not in `ffi.rs`) so `v8host` has no forward reference into the
/// FFI layer; Task 4's `ffi.rs` wires the real callback via `set_hook_request`.
pub type HookRequestFn = extern "C" fn(descriptor: *const c_char, enable: c_int);

// ---------------------------------------------------------------------------
// Engine-ops: C-ABI function pointers the shim implements and the core calls.
//
// Every Slice-3 engine touchpoint is a C++ call living shim-side; the core only
// ever sees these opaque C-ABI pointers (no Rust-side C++ vtable dispatch).  This
// is a `#[repr(C)]` mirror of `S2EngineOps` in shim/include/s2script_core.h; the
// two must stay in lockstep (contract, not layout — treadmill-checked).
//
// All fields are nullable (`Option<extern "C" fn ...>` is the null-optimized FFI
// representation): a null field degrades the matching native to a safe miss.  Only
// `schema_offset` is wired in Slice-3 Task 3; Tasks 4–5 fill the remaining fields.
// ---------------------------------------------------------------------------
pub type SchemaOffsetFn = extern "C" fn(cls: *const c_char, field: *const c_char) -> c_int;
pub type EntByIndexFn = extern "C" fn(idx: c_int) -> *mut c_void;
pub type DerefHandleFn = extern "C" fn(handle: c_uint) -> *mut c_void;
pub type EntStateChangedFn = extern "C" fn(ent: *mut c_void, offset: c_int);
pub type ConCommandRegisterFn = extern "C" fn(name: *const c_char);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct S2EngineOps {
    pub schema_offset: Option<SchemaOffsetFn>,
    pub ent_by_index: Option<EntByIndexFn>,
    pub deref_handle: Option<DerefHandleFn>,
    pub ent_state_changed: Option<EntStateChangedFn>,
    pub concommand_register: Option<ConCommandRegisterFn>,
}

static PLATFORM_INIT: Once = Once::new();

/// A JS handler stored as a persistent function reference.  `Clone` is required
/// because `Descriptor::snapshot` clones each `H`; `v8::Global` clone is a cheap
/// refcount bump.
#[derive(Clone)]
struct JsHandler {
    func: v8::Global<v8::Function>,
}

/// Per-plugin identity stamped on each plugin `v8::Context` via `Context::set_slot::<PluginId>`
/// (the spike-RECOMMENDED mechanism — a Rust-typed slot needs no scope to read and no side
/// table).  A native reads it back via `scope.get_current_context().get_slot::<PluginId>()`,
/// which resolves to the CALLING context's id (per-context, correct across the microtask
/// checkpoint).  The `Rc<PluginId>` is dropped when the context is GC'd (i.e. when its
/// `Global<Context>` is dropped from `PLUGINS` and the isolate reclaims it).
struct PluginId(String);

thread_local! {
    static LOGGER: std::cell::Cell<Option<LogFn>> = std::cell::Cell::new(None);
    static HOST: std::cell::RefCell<Option<Host>> = std::cell::RefCell::new(None);
    /// The single `OnGameFrame` descriptor / per-descriptor subscription registry.
    static FRAME: std::cell::RefCell<Descriptor<JsHandler>> =
        std::cell::RefCell::new(Descriptor::new("OnGameFrame"));
    /// Embedder callback for detour install/remove.  `None` until `set_hook_request`
    /// is called (Task 4); while `None`, `apply_detour` is a safe no-op.
    static HOOK_REQUEST: std::cell::Cell<Option<HookRequestFn>> = std::cell::Cell::new(None);
    /// Frame counter = number of `frame_async_drain` calls COMPLETED (starts at 0).  Used to
    /// schedule `Frame(target)` timers: a drain resolves `Frame(t)` when the PRE-increment value
    /// it reads satisfies `frame >= t`.  `NextTick` targets the current count (resolves next drain);
    /// `NextFrame` targets `current + 1` (resolves one drain later).
    static FRAME_COUNTER: std::cell::Cell<u64> = std::cell::Cell::new(0);
    /// Pending timer queue (Delay/NextTick/NextFrame).  Holds only `u64` ids; the promise lives
    /// in `RESOLVERS`.  Borrowed briefly in `make_timer_promise`/`frame_async_drain`/`refresh_detour`;
    /// NEVER held across `perform_microtask_checkpoint` (a continuation re-enters it).
    static TIMERS: std::cell::RefCell<TimerQueue> = std::cell::RefCell::new(TimerQueue::new());
    /// `async id → Global<PromiseResolver>`.  The Global is dropped (removed) when the timer fires.
    /// Cleared in `shutdown` BEFORE the isolate is dropped.  Never held across the checkpoint.
    static RESOLVERS: std::cell::RefCell<std::collections::HashMap<u64, v8::Global<v8::PromiseResolver>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Monotonic async-id allocator (1-based; 0 is reserved as "none").
    static NEXT_ASYNC_ID: std::cell::Cell<u64> = std::cell::Cell::new(1);
    /// Count of in-flight async-FFI jobs (Task 5 populates this); feeds the combined detour predicate.
    static PENDING_JOBS: std::cell::Cell<usize> = std::cell::Cell::new(0);
    /// Cached view of "is the OnGameFrame detour currently installed?" — the source of truth the
    /// combined lazy-detour reconciles against, so we only call `HOOK_REQUEST` on a real transition.
    static DETOUR_INSTALLED: std::cell::Cell<bool> = std::cell::Cell::new(false);
    /// Engine-ops table (copied by value at init from the shim's stack-local struct — the shim's
    /// pointer must NOT be retained past init).  `None` until `set_engine_ops` runs; while `None`
    /// (or a given field is null) the matching native degrades to a safe miss.
    static ENGINE_OPS: std::cell::Cell<Option<S2EngineOps>> = std::cell::Cell::new(None);
    /// `(class, field) → offset` cache backing `__s2_schema_offset`; keys are opaque JS strings
    /// (NO game names in core).  Reset on `shutdown` so a re-init can re-resolve (avoids a stale
    /// `-1` miss cached before the schema was loaded).
    static SCHEMA_OFFSETS: std::cell::RefCell<crate::schema::OffsetCache> =
        std::cell::RefCell::new(crate::schema::OffsetCache::new());
    /// `name → Global<Function>` map for registered ConCommands.  The shim calls back via
    /// `s2script_core_dispatch_concommand` (C-ABI) when a registered command fires.  Reset on
    /// `shutdown` (BEFORE the isolate is dropped — same discipline as `RESOLVERS`).
    static CONCOMMANDS: std::cell::RefCell<std::collections::HashMap<String, v8::Global<v8::Function>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Per-plugin `v8::Context` registry, keyed by plugin id — the multi-context path that will
    /// eventually replace the single shared `HOST.context` (Task 5 migrates the natives/dispatch
    /// onto it).  Each `Global<Context>` is stamped with a `PluginId` slot at creation.  ADDED
    /// ALONGSIDE `HOST` for this task: the existing single-context path is untouched.  Dropped
    /// (per id in `dispose_plugin_context`, or all in `shutdown`) while the isolate is still alive
    /// — same discipline as `RESOLVERS`/`CONCOMMANDS`.
    static PLUGINS: std::cell::RefCell<std::collections::HashMap<String, v8::Global<v8::Context>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Plugin registry (Task 2): generation counter + per-plugin teardown ledger, keyed by the
    /// same id string as `PLUGINS`.  Reset on `shutdown` so a re-init starts empty.
    static REGISTRY: std::cell::RefCell<plugin::Registry>
        = std::cell::RefCell::new(plugin::Registry::new());
}

/// Install the shim's engine-ops table (copied by value; see `ENGINE_OPS`).  Wired by `ffi.rs`.
pub fn set_engine_ops(ops: Option<S2EngineOps>) {
    ENGINE_OPS.with(|c| c.set(ops));
}

/// Install the embedder's detour-request callback.  Wired by `ffi.rs` (Task 4).
pub fn set_hook_request(f: Option<HookRequestFn>) {
    HOOK_REQUEST.with(|c| c.set(f));
}

/// Allocate the next async id (timers + Task-5 jobs share this space).
fn next_async_id() -> u64 {
    NEXT_ASYNC_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

/// Total in-flight async work: pending timers + pending jobs.  Reads TIMERS (brief borrow).
fn async_pending() -> usize {
    TIMERS.with(|t| t.borrow().len()) + PENDING_JOBS.with(|c| c.get())
}

/// Combined lazy-detour reconciler.  Desired = any onGameFrame subscriber OR any pending async.
/// Only pokes the embedder on a real transition, keeping `DETOUR_INSTALLED` the single source of
/// truth.  Borrows FRAME + TIMERS (via `async_pending`) — callers must hold NEITHER borrow.
fn refresh_detour() {
    let desired = FRAME.with(|f| f.borrow().enabled_count() > 0) || async_pending() > 0;
    let installed = DETOUR_INSTALLED.with(|c| c.get());
    if desired == installed {
        return;
    }
    DETOUR_INSTALLED.with(|c| c.set(desired));
    HOOK_REQUEST.with(|c| {
        if let Some(req) = c.get() {
            let name = CString::new("OnGameFrame").unwrap();
            req(name.as_ptr(), desired as c_int);
        }
    });
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
globalThis.Delay = (ms) => __s2_delay(ms || 0);
globalThis.NextTick = () => __s2_next_tick();
globalThis.NextFrame = () => __s2_next_frame();
globalThis.threadSleep = (ms) => __s2_thread_sleep(ms || 0);
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

        // The combined predicate supersedes the DetourChange the multiplexer returns; ignore it.
        let (id, _change) =
            FRAME.with(|f| f.borrow_mut().subscribe(priority, phase, "legacy".into(), JsHandler { func: global }));
        refresh_detour();
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
        // The combined predicate supersedes the DetourChange the multiplexer returns; ignore it.
        let _change = FRAME.with(|f| f.borrow_mut().unsubscribe(id));
        refresh_detour();
    }));
}

/// Shared helper for the timer natives: create a `PromiseResolver`, stash its `Global` under a
/// fresh async id, push the timer, reconcile the detour, and return the pending promise.
fn make_timer_promise<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    kind: TimerKind,
) -> v8::Local<'s, v8::Value> {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let id = next_async_id();
    RESOLVERS.with(|m| m.borrow_mut().insert(id, v8::Global::new(scope.as_ref(), resolver)));
    TIMERS.with(|t| t.borrow_mut().push(id, kind));
    refresh_detour();
    promise.into()
}

/// Native `__s2_delay(ms) -> Promise`.  Resolves after a wall-clock deadline.
fn s2_delay(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ms = args.get(0).integer_value(scope).unwrap_or(0);
        let ms = if ms > 0 { ms as u64 } else { 0 };
        let kind = TimerKind::Deadline(Instant::now() + Duration::from_millis(ms));
        let promise = make_timer_promise(scope, kind);
        rv.set(promise);
    }));
}

/// Native `__s2_next_tick() -> Promise`.  Resolves on the very next frame drain
/// (`Frame(FRAME_COUNTER)` → the next drain reads that same count and fires it).
fn s2_next_tick(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let target = FRAME_COUNTER.with(|c| c.get());
        let promise = make_timer_promise(scope, TimerKind::Frame(target));
        rv.set(promise);
    }));
}

/// Native `__s2_next_frame() -> Promise`.  Resolves exactly one frame later than `NextTick`
/// (`Frame(FRAME_COUNTER + 1)` → the drain after next).
fn s2_next_frame(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let target = FRAME_COUNTER.with(|c| c.get().wrapping_add(1));
        let promise = make_timer_promise(scope, TimerKind::Frame(target));
        rv.set(promise);
    }));
}

/// Native `__s2_thread_sleep(ms) -> Promise`.  Submits a blocking sleep to the worker pool;
/// the Promise resolves the next drain after the worker finishes.
fn s2_thread_sleep(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ms = args.get(0).integer_value(scope).unwrap_or(0);
        let ms = if ms > 0 { ms as u64 } else { 0 };
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let id = next_async_id();
        RESOLVERS.with(|m| m.borrow_mut().insert(id, v8::Global::new(scope.as_ref(), resolver)));
        PENDING_JOBS.with(|c| c.set(c.get() + 1));
        pool().submit(id, Box::new(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Ok(())
        }));
        refresh_detour();
        rv.set(promise.into());
    }));
}

/// Native `__s2_schema_offset(class, field) -> i32`.  Resolves a schema field's byte offset
/// within a class via the live SchemaSystem (through the shim's `schema_offset` engine-op),
/// caching the result.  Returns `-1` on any miss (no ops / null pointer / class or field not
/// found) and WARNs at most once per key.  `class`/`field` are OPAQUE JS strings — no game
/// identifiers appear in core.
///
/// Like the other natives, the body runs under `catch_unwind` because it is invoked as a V8
/// `FunctionCallback` from C++: a Rust panic must never unwind across the FFI boundary.  It does
/// NOT touch `HOST`; it borrows only `SCHEMA_OFFSETS` (and, transitively, the `ENGINE_OPS`/`LOGGER`
/// `Cell`s), none of which the shim's `schema_offset` call re-enters.
fn s2_schema_offset(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Default the return to the -1 miss sentinel up front: a panic anywhere below (e.g. an
        // allocation failure in the string/cache ops) then leaves a well-formed -1, never a JS
        // `undefined` — which would slip past pawn.js's `HEALTH < 0` guard and be used as an offset.
        rv.set_int32(-1);
        if args.length() < 2 {
            return;
        }
        let class = args.get(0).to_rust_string_lossy(scope);
        let field = args.get(1).to_rust_string_lossy(scope);

        // Live resolver: marshal to C strings and call the shim's engine-op (recon Q1 lives shim
        // side).  Degrades to `-1` if no ops table, a null `schema_offset`, or interior NULs.
        let live_raw = |c: &str, f: &str| -> i32 {
            let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return -1 };
            let Some(func) = ops.schema_offset else { return -1 };
            let (Ok(cc), Ok(cf)) = (CString::new(c), CString::new(f)) else { return -1 };
            func(cc.as_ptr(), cf.as_ptr())
        };
        let live_log = |msg: &str| {
            if let Some(l) = LOGGER.with(|l| l.get()) {
                if let Ok(cs) = CString::new(msg) {
                    l(0, cs.as_ptr());
                }
            }
        };

        let off =
            SCHEMA_OFFSETS.with(|c| c.borrow_mut().resolve(&class, &field, live_raw, live_log));
        rv.set_int32(off);
    }));
}

/// Native `__s2_entity_by_index(i: number) -> External|null`.
/// Calls the shim's `ent_by_index` engine-op (recon Q3 — manual chunk walk).
/// Returns a `v8::External` wrapping the opaque `CEntityInstance*`, or JS `null` on any miss.
/// Degrades (null + named WARN) if the ops table or the fn pointer is absent.
fn s2_entity_by_index(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 {
            rv.set_null();
            return;
        }
        let idx = args.get(0).integer_value(scope).unwrap_or(-1) as c_int;

        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else {
            log_warn("WARN: __s2_entity_by_index: no engine ops table");
            rv.set_null();
            return;
        };
        let Some(func) = ops.ent_by_index else {
            log_warn("WARN: __s2_entity_by_index: ent_by_index not wired in ops");
            rv.set_null();
            return;
        };
        let ptr = func(idx);
        if ptr.is_null() {
            rv.set_null();
        } else {
            let ext = v8::External::new(scope, ptr);
            rv.set(ext.into());
        }
    }));
}

/// Native `__s2_deref_handle(h: number) -> External|null`.
/// Calls the shim's `deref_handle` engine-op (recon Q4 — validates serial, null on stale).
/// Returns a `v8::External` wrapping `CEntityInstance*`, or JS `null` when the handle is stale.
fn s2_deref_handle(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 {
            rv.set_null();
            return;
        }
        let handle = args.get(0).uint32_value(scope).unwrap_or(0) as c_uint;

        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else {
            log_warn("WARN: __s2_deref_handle: no engine ops table");
            rv.set_null();
            return;
        };
        let Some(func) = ops.deref_handle else {
            log_warn("WARN: __s2_deref_handle: deref_handle not wired in ops");
            rv.set_null();
            return;
        };
        let ptr = func(handle);
        if ptr.is_null() {
            rv.set_null();
        } else {
            let ext = v8::External::new(scope, ptr);
            rv.set(ext.into());
        }
    }));
}

/// Native `__s2_ent_read_i32(ent: External, off: number) -> number`.
/// Unwraps the `v8::External` to a `*const u8` (opaque entity pointer) and calls
/// `entity::read_i32`.  Returns 0 on null base or negative offset (degrade-safe).
fn s2_ent_read_i32(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 {
            rv.set_int32(0);
            return;
        }
        let base = match v8::Local::<v8::External>::try_from(args.get(0)) {
            Ok(ext) => ext.value() as *const u8,
            Err(_) => {
                rv.set_int32(0);
                return;
            }
        };
        let off = args.get(1).integer_value(scope).unwrap_or(0) as i32;
        rv.set_int32(crate::entity::read_i32(base, off));
    }));
}

/// Native `__s2_ent_write_i32(ent: External, off: number, v: number)`.
/// Unwraps the `v8::External` to a `*mut u8` (opaque entity pointer) and calls
/// `entity::write_i32`.  No-op on null base or negative offset (degrade-safe).
fn s2_ent_write_i32(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 {
            return;
        }
        let base = match v8::Local::<v8::External>::try_from(args.get(0)) {
            Ok(ext) => ext.value() as *mut u8,
            Err(_) => return,
        };
        let off = args.get(1).integer_value(scope).unwrap_or(0) as i32;
        let val = args.get(2).integer_value(scope).unwrap_or(0) as i32;
        crate::entity::write_i32(base, off, val);
    }));
}

/// Native `__s2_ent_state_changed(ent: External, off: number)`.
/// Calls the shim's `ent_state_changed` engine-op (recon Q6 — virtual
/// `CEntityInstance::NetworkStateChanged`).  No return value; no-op on bad args.
fn s2_ent_state_changed(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 {
            return;
        }
        let ent = match v8::Local::<v8::External>::try_from(args.get(0)) {
            Ok(ext) => ext.value(),
            Err(_) => return,
        };
        if ent.is_null() {
            return;
        }
        let off = args.get(1).integer_value(scope).unwrap_or(0) as c_int;

        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else {
            log_warn("WARN: __s2_ent_state_changed: no engine ops table");
            return;
        };
        let Some(func) = ops.ent_state_changed else {
            log_warn("WARN: __s2_ent_state_changed: ent_state_changed not wired in ops");
            return;
        };
        func(ent, off);
    }));
}

/// Native `__s2_concommand(name: string, fn: (slot: number, argString: string) => void)`.
/// Stores the JS callback `Global<Function>` keyed by command name in `CONCOMMANDS`, then
/// calls `ops.concommand_register(name)` to register the raw ConCommand engine-side (shim).
/// Degrades (WARN) if ops/fn null; `catch_unwind`; does NOT touch `HOST`.
fn s2_concommand(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 {
            return;
        }
        let name = args.get(0).to_rust_string_lossy(scope);
        let func_local = match v8::Local::<v8::Function>::try_from(args.get(1)) {
            Ok(f) => f,
            Err(_) => return,
        };
        let global = v8::Global::new(scope.as_ref(), func_local);

        // Store the Global<Function> — CONCOMMANDS borrow is released before the engine call.
        CONCOMMANDS.with(|m| m.borrow_mut().insert(name.clone(), global));

        // Register the raw ConCommand engine-side via the shim's ops table.
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else {
            log_warn("WARN: __s2_concommand: no engine ops table");
            return;
        };
        let Some(func) = ops.concommand_register else {
            log_warn("WARN: __s2_concommand: concommand_register not wired in ops");
            return;
        };
        let Ok(cname) = CString::new(name.as_str()) else { return };
        func(cname.as_ptr());
    }));
}

/// Dispatch a ConCommand callback to the registered JS function.
///
/// Called from `ffi.rs`'s `s2script_core_dispatch_concommand` (C-ABI export), which the shim's
/// ConCommand trampoline invokes when a registered command fires.
///
/// **Re-entrancy discipline:** borrow `CONCOMMANDS`, CLONE the `Global<Function>`, DROP the
/// borrow — then open a `HOST` scope and call JS.  A command handler may call `__s2_concommand`
/// again (re-enters `CONCOMMANDS.borrow_mut()`); holding the borrow across the JS call would
/// panic.  No `CONCOMMANDS` borrow is held across the JS invocation.
pub(crate) fn dispatch_concommand(name: &str, slot: i32, args: &str) {
    // Phase 1: clone the Global, release the borrow before JS.
    let maybe_fn = CONCOMMANDS.with(|m| m.borrow().get(name).cloned());
    let Some(global) = maybe_fn else { return };

    // Phase 2: open a V8 scope (borrows HOST) and invoke the JS fn.
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &host.context);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Build JS arguments: (slot: number, args: string).
        let recv: v8::Local<v8::Value> = v8::undefined(scope).into();
        let slot_val: v8::Local<v8::Value> = v8::Number::new(scope, slot as f64).into();
        let Some(args_str) = v8::String::new(scope, args) else { return };
        let args_val: v8::Local<v8::Value> = args_str.into();

        // Per-call TryCatch so a throwing handler doesn't propagate outside dispatch.
        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

        let func = v8::Local::new(tc, &global);
        let _ = func.call(tc, recv, &[slot_val, args_val]);
    });
}

/// Shared logging helper for named WARNs in the engine-op natives above.
fn log_warn(msg: &str) {
    if let Some(l) = LOGGER.with(|l| l.get()) {
        if let Ok(cs) = CString::new(msg) {
            l(0, cs.as_ptr());
        }
    }
}

// ---------------------------------------------------------------------------
// Per-plugin context registry (Task 4 — first step of the single→multi refactor).
//
// ADDED ALONGSIDE the single-context `HOST` path, which is intentionally left intact:
// every existing native/dispatch/drain still runs on `HOST.context`.  These functions add a
// PARALLEL, per-plugin `v8::Context` registry (`PLUGINS`) + identity (`set_slot::<PluginId>`)
// on the SAME shared isolate that lives in `HOST`.  Task 5 migrates the existing surface onto
// this path; Task 6 hangs the teardown ledger off `REGISTRY`.
// ---------------------------------------------------------------------------

/// Read the CALLING context's plugin id from its `PluginId` slot (spike PROVE #2).
///
/// `get_current_context()` in a `FunctionCallback` returns the context of the currently running
/// JS (per-context, correct across the microtask checkpoint), so a native must read it FRESH on
/// each invocation.  Returns `None` for a context with no stamped id (e.g. the shared `HOST`
/// context, which is not a plugin context).
pub(crate) fn current_plugin(scope: &mut v8::PinScope) -> Option<String> {
    scope
        .get_current_context()
        .get_slot::<PluginId>()
        .map(|p| p.0.clone())
}

/// Native `__s2_current_plugin() -> string`.  Minimal per-context probe installed by
/// `create_plugin_context` (Task 5 replaces this with the full injected API).  Returns the
/// calling context's plugin id, or `""` if unstamped.
///
/// Like every native, the body runs under `catch_unwind` — a Rust panic must never unwind across
/// the V8/C++ FFI boundary (degrade-never-crash, spec §6).
fn s2_current_plugin(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = current_plugin(scope).unwrap_or_default();
        if let Some(s) = v8::String::new(scope, &id) {
            rv.set(s.into());
        }
    }));
}

/// Create a fresh per-plugin `v8::Context` on the shared isolate (borrowed from `HOST`), stamp it
/// with the plugin id via `set_slot::<PluginId>`, install the MINIMAL per-context API (just the
/// `__s2_current_plugin` probe for now — Task 5 adds the full injected API), store its
/// `Global<Context>` in `PLUGINS`, register the plugin in `REGISTRY`, and return the generation.
///
/// Panics only if called before `init` (no isolate yet) — an internal invariant, not an FFI path.
pub(crate) fn create_plugin_context(id: &str) -> u64 {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow
            .as_mut()
            .expect("create_plugin_context called before init");

        // Build the context in a nested block so the HandleScope borrow on the shared isolate is
        // released before we touch PLUGINS.  Mirrors `init`'s scope construction.
        let g_ctx = {
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Context::new(hs, Default::default());

            // Stamp the plugin identity (no scope needed — Rust-typed slot).
            let _ = ctx_local.set_slot(std::rc::Rc::new(PluginId(id.to_string())));

            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            // Minimal per-context install: the `__s2_current_plugin` probe native.
            let global_obj = ctx_local.global(scope);
            let key = v8::String::new(scope, "__s2_current_plugin").unwrap();
            let func = v8::Function::new(scope, s2_current_plugin).unwrap();
            global_obj.set(scope, key.into(), func.into());

            v8::Global::new(scope.as_ref(), ctx_local)
            // scope, hs, hs_storage drop here — the isolate borrow is released.
        };

        PLUGINS.with(|p| p.borrow_mut().insert(id.to_string(), g_ctx));
        REGISTRY.with(|r| r.borrow_mut().insert(id))
    })
}

/// Dispose a plugin's context: drop its `Global<Context>` (making the context GC-eligible while
/// the isolate is still alive) and remove it from both `PLUGINS` and `REGISTRY`.
///
/// NOTE: the `Global`s pointing INTO this context (handlers/resolvers/exports) must be dropped
/// BEFORE its `Global<Context>` — that ordered teardown is Task 6's ledger job.  For THIS task
/// (minimal per-context install, no such inner Globals yet) dropping the `Global<Context>` is
/// sufficient.
pub(crate) fn dispose_plugin_context(id: &str) {
    // Dropping the Global<Context> here (map removal) is safe: the isolate lives in HOST.
    PLUGINS.with(|p| {
        p.borrow_mut().remove(id);
    });
    REGISTRY.with(|r| {
        r.borrow_mut().remove(id);
    });
}

/// Enter the `id`'s plugin context and evaluate `src` in it (test/integration helper — the
/// per-plugin analogue of `eval`).  Uses the shared isolate from `HOST`; mirrors `eval`'s scope +
/// `TryCatch` construction.  Returns `Err` if `init` hasn't run, the id has no context, or the JS
/// fails to compile/run.
pub(crate) fn eval_in_context(id: &str, src: &str) -> Result<(), String> {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow
            .as_mut()
            .ok_or_else(|| "eval_in_context called before init".to_string())?;

        // Clone the plugin's Global<Context> out of PLUGINS (cheap refcount bump) so we don't hold
        // the PLUGINS borrow across the HandleScope on HOST.isolate.
        let g_ctx = PLUGINS
            .with(|p| p.borrow().get(id).cloned())
            .ok_or_else(|| format!("eval_in_context: no context for plugin '{}'", id))?;

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

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

pub fn init(logger: LogFn) -> Result<(), String> {
    ensure_platform();
    LOGGER.with(|l| l.set(Some(logger)));

    let mut isolate = v8::Isolate::new(v8::CreateParams::default());

    // We own the microtask checkpoint: with Explicit policy, await/.then continuations run ONLY
    // when we call perform_microtask_checkpoint() in frame_async_drain (once per frame).
    isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

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

        // Install the async timer primitives (Delay / NextTick / NextFrame) on the global object.
        let delay_key = v8::String::new(scope, "__s2_delay").unwrap();
        let delay_fn = v8::Function::new(scope, s2_delay).unwrap();
        global_obj.set(scope, delay_key.into(), delay_fn.into());
        let next_tick_key = v8::String::new(scope, "__s2_next_tick").unwrap();
        let next_tick_fn = v8::Function::new(scope, s2_next_tick).unwrap();
        global_obj.set(scope, next_tick_key.into(), next_tick_fn.into());
        let next_frame_key = v8::String::new(scope, "__s2_next_frame").unwrap();
        let next_frame_fn = v8::Function::new(scope, s2_next_frame).unwrap();
        global_obj.set(scope, next_frame_key.into(), next_frame_fn.into());
        let thread_sleep_key = v8::String::new(scope, "__s2_thread_sleep").unwrap();
        let thread_sleep_fn = v8::Function::new(scope, s2_thread_sleep).unwrap();
        global_obj.set(scope, thread_sleep_key.into(), thread_sleep_fn.into());

        // Install the schema-offset native (`__s2_schema_offset`) on the global object.
        let schema_offset_key = v8::String::new(scope, "__s2_schema_offset").unwrap();
        let schema_offset_fn = v8::Function::new(scope, s2_schema_offset).unwrap();
        global_obj.set(scope, schema_offset_key.into(), schema_offset_fn.into());

        // Install the entity-system natives (Task 4): entity-by-index, handle-deref,
        // i32 field read/write, and NetworkStateChanged.  Engine-dependent paths are
        // verified live in Task 7; the pure read/write helpers are unit-tested in entity.rs.
        let ent_by_idx_key = v8::String::new(scope, "__s2_entity_by_index").unwrap();
        let ent_by_idx_fn  = v8::Function::new(scope, s2_entity_by_index).unwrap();
        global_obj.set(scope, ent_by_idx_key.into(), ent_by_idx_fn.into());

        let deref_handle_key = v8::String::new(scope, "__s2_deref_handle").unwrap();
        let deref_handle_fn  = v8::Function::new(scope, s2_deref_handle).unwrap();
        global_obj.set(scope, deref_handle_key.into(), deref_handle_fn.into());

        let ent_read_i32_key = v8::String::new(scope, "__s2_ent_read_i32").unwrap();
        let ent_read_i32_fn  = v8::Function::new(scope, s2_ent_read_i32).unwrap();
        global_obj.set(scope, ent_read_i32_key.into(), ent_read_i32_fn.into());

        let ent_write_i32_key = v8::String::new(scope, "__s2_ent_write_i32").unwrap();
        let ent_write_i32_fn  = v8::Function::new(scope, s2_ent_write_i32).unwrap();
        global_obj.set(scope, ent_write_i32_key.into(), ent_write_i32_fn.into());

        let ent_state_changed_key = v8::String::new(scope, "__s2_ent_state_changed").unwrap();
        let ent_state_changed_fn  = v8::Function::new(scope, s2_ent_state_changed).unwrap();
        global_obj.set(scope, ent_state_changed_key.into(), ent_state_changed_fn.into());

        // Install __s2_concommand (Task 5): register a raw Source 2 ConCommand; the shim's
        // trampoline calls s2script_core_dispatch_concommand (C-ABI) when the command fires.
        let concommand_key = v8::String::new(scope, "__s2_concommand").unwrap();
        let concommand_fn  = v8::Function::new(scope, s2_concommand).unwrap();
        global_obj.set(scope, concommand_key.into(), concommand_fn.into());

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
    // snapshot() returns 4-tuples (SubId, Priority, owner, H); strip owner for run_chain.
    let snap4 = FRAME.with(|f| f.borrow().snapshot(phase));
    if snap4.is_empty() {
        return DispatchOutcome {
            result: HookResult::Continue,
            detour: DetourChange::None,
        };
    }
    let snap: Vec<_> = snap4.into_iter().map(|(id, prio, _owner, h)| (id, prio, h)).collect();

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
                        Ok(match ret.uint32_value(tc).unwrap_or(0) {
                            0 => HookResult::Continue,
                            1 => HookResult::Changed,
                            2 => HookResult::Handled,
                            3 => HookResult::Stop,
                            n => {
                                if let Some(f) = LOGGER.with(|l| l.get()) {
                                    if let Ok(c) = CString::new(format!(
                                        "WARN: onGameFrame handler returned out-of-range HookResult {n}; treating as Continue"
                                    )) {
                                        f(0, c.as_ptr());
                                    }
                                }
                                HookResult::Continue
                            }
                        })
                    }
                }
            }
        })
    });

    // Phase 3 — brief &mut FRAME borrow: error/auto-disable bookkeeping (the FRAME borrow is
    // released by the `.with` before we reconcile).  Route the actual install/remove through the
    // combined predicate so an auto-disable can't tear down the detour while async is still pending.
    let detour = FRAME.with(|f| f.borrow_mut().apply_errors(&outcome.errored));
    refresh_detour();
    DispatchOutcome {
        result: outcome.result,
        detour,
    }
}

/// Read a JS file from `path` and evaluate it in the HOST context (identical scope construction
/// to `eval`).  Engine-generic: the path is supplied by the caller; NO game identifiers appear
/// here.  On a read error logs a named WARN and returns (degrade-never-crash).  On a JS error
/// logs a WARN and returns (same policy).  A missing or unreadable cs2 JS file must never
/// crash or panic.
pub fn load_cs2_file(path: &str) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            log_warn(&format!("WARN: load_cs2_file: failed to read '{}': {}", path, e));
            return;
        }
    };
    if let Err(e) = eval(&src) {
        log_warn(&format!("WARN: load_cs2_file: JS error in '{}': {}", path, e));
    }
}

pub fn shutdown() {
    // Clear async state BEFORE dropping the isolate: RESOLVERS holds `Global`s into it, so their
    // handles must be reset while the isolate is still alive (HOST still owns it here).
    TIMERS.with(|t| *t.borrow_mut() = TimerQueue::new());
    RESOLVERS.with(|m| m.borrow_mut().clear());
    // Clear CONCOMMANDS BEFORE dropping the isolate — same discipline as RESOLVERS: the map holds
    // Global<Function>s into the isolate; dropping them while the isolate is alive is required.
    CONCOMMANDS.with(|m| m.borrow_mut().clear());
    // Drop all per-plugin contexts BEFORE the isolate: each `Global<Context>` points into the
    // isolate, so (like RESOLVERS/CONCOMMANDS) the handles must be released while the isolate is
    // still alive.  Task 6's ledger will additionally drop each plugin's inner Globals first.
    PLUGINS.with(|p| p.borrow_mut().clear());
    // Clear the plugin registry so a re-init starts with an empty generation space + no ledgers.
    REGISTRY.with(|r| *r.borrow_mut() = plugin::Registry::new());
    PENDING_JOBS.with(|c| c.set(0));
    DETOUR_INSTALLED.with(|c| c.set(false));
    // Drop the isolate and context.  The platform is never torn down.
    HOST.with(|h| {
        let _ = h.borrow_mut().take();
    });
    // Reset the descriptor so a re-init starts with a clean, empty registry.
    FRAME.with(|f| *f.borrow_mut() = Descriptor::new("OnGameFrame"));
    // Reset the frame counter so a re-init starts from zero.
    FRAME_COUNTER.with(|c| c.set(0));
    // Reset the schema-offset cache so a re-init re-resolves (a `-1` cached before the schema was
    // loaded must not persist across an init cycle).
    SCHEMA_OFFSETS.with(|c| *c.borrow_mut() = crate::schema::OffsetCache::new());
}

/// Per-frame async drain: resolve every due timer, advance the frame counter, then run the single
/// V8 microtask checkpoint for this frame.  Called once per Post-phase game frame (wired in `ffi.rs`).
///
/// **Re-entrancy discipline (load-bearing):** a resolved continuation (a `Delay`/`NextTick`
/// handler that itself calls `Delay`/`NextTick`/`NextFrame`/`onGameFrame`) re-enters the
/// TIMERS/RESOLVERS/FRAME thread-locals from INSIDE `perform_microtask_checkpoint`.  So we must
/// hold NONE of those borrows across the checkpoint: collect due ids (TIMERS borrow dropped),
/// remove+resolve each resolver (RESOLVERS borrow dropped per id), advance FRAME_COUNTER (Cell,
/// no borrow), THEN run the checkpoint.  Resolving a promise does NOT run JS under kExplicit — the
/// continuations wait for the checkpoint.  Holding HOST across the checkpoint is fine (no primitive
/// borrows HOST).  `refresh_detour` (borrows FRAME + TIMERS) runs only after the scope is dropped.
///
/// Task 5 will insert job (async-FFI) resolution before the checkpoint, using the same scope.
pub(crate) fn frame_async_drain() {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };

        // Open a HandleScope + ContextScope on the stored isolate/context (mirrors `eval`).
        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &host.context);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Resolve due timers using the PRE-increment counter (= drains completed so far).  A
        // `Frame(t)` timer fires when this `frame >= t`; a `Deadline(d)` fires when `now >= d`.
        let frame = FRAME_COUNTER.with(|c| c.get());
        let due = TIMERS.with(|t| t.borrow_mut().due(Instant::now(), frame));
        for id in due {
            if let Some(g) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) {
                let resolver = v8::Local::new(scope, &g);
                let undef = v8::undefined(scope);
                resolver.resolve(scope, undef.into());
            }
        }
        // Resolve completed threadpool jobs.
        while let Some((id, _res)) = pool().try_recv_completed() {
            if let Some(g) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) {
                PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));   // only for a job we own
                let resolver = v8::Local::new(scope, &g);
                let undef = v8::undefined(scope);
                resolver.resolve(scope, undef.into());
            }
        }

        // Advance the counter BEFORE the checkpoint so continuations observe the new count.
        FRAME_COUNTER.with(|c| c.set(frame.wrapping_add(1)));

        // The one microtask checkpoint for this frame — no TIMERS/RESOLVERS/FRAME borrow held.
        scope.perform_microtask_checkpoint();
    });
    // HOST + scope released: a just-completed last timer may make the detour undesired, or a
    // continuation may have queued new async keeping it desired.  Reconcile now.
    refresh_detour();
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

    // A no-op logger for tests that don't care about log output.
    extern "C" fn dummy_log_fn(_l: c_int, _m: *const c_char) {}
    fn dummy_logger() -> LogFn { dummy_log_fn }

    // Read `globalThis[name]` as a bool from the current isolate/context.
    fn read_bool_global(name: &str) -> bool {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("read_bool_global: no host");
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &host.context);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let global = ctx_local.global(scope);
            let key = v8::String::new(scope, name).unwrap();
            let val = global.get(scope, key.into()).unwrap_or_else(|| v8::undefined(scope).into());
            val.is_true()
        })
    }

    // Read `globalThis[name]` as an i32 from the current isolate/context (mirrors read_bool_global).
    fn read_i32_global(name: &str) -> i32 {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("read_i32_global: no host");
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &host.context);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let global = ctx_local.global(scope);
            let key = v8::String::new(scope, name).unwrap();
            let val = global.get(scope, key.into()).unwrap_or_else(|| v8::undefined(scope).into());
            val.integer_value(scope).unwrap_or(0) as i32
        })
    }

    // Read `globalThis[name]` as a String from the current isolate/context (mirrors read_bool_global).
    fn read_string_global(name: &str) -> String {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("read_string_global: no host");
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &host.context);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let global = ctx_local.global(scope);
            let key = v8::String::new(scope, name).unwrap();
            let val = global.get(scope, key.into()).unwrap_or_else(|| v8::undefined(scope).into());
            val.to_rust_string_lossy(scope)
        })
    }

    // Read `globalThis[name]` as a String from a specific PLUGIN context (enters the id's
    // Global<Context>, mirrors read_string_global but for the per-plugin registry).
    fn read_string_global_in(id: &str, name: &str) -> String {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("read_string_global_in: no host");
            let g_ctx = PLUGINS
                .with(|p| p.borrow().get(id).cloned())
                .expect("read_string_global_in: no context for id");
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let global = ctx_local.global(scope);
            let key = v8::String::new(scope, name).unwrap();
            let val = global.get(scope, key.into()).unwrap_or_else(|| v8::undefined(scope).into());
            val.to_rust_string_lossy(scope)
        })
    }

    // Two per-plugin contexts on the shared isolate each report their OWN id via the
    // `__s2_current_plugin` probe native (identity via `set_slot::<PluginId>` +
    // `get_current_context`), and disposing one removes it from PLUGINS.  The single-context HOST
    // path is untouched (this test never uses `eval`).
    #[test]
    fn two_contexts_have_distinct_plugin_identity() {
        init(dummy_logger()).unwrap();
        create_plugin_context("alpha");
        create_plugin_context("beta");
        // A tiny probe native reads current_plugin() and stashes it on the context global.
        eval_in_context("alpha", "globalThis.__who = __s2_current_plugin();").unwrap();
        eval_in_context("beta",  "globalThis.__who = __s2_current_plugin();").unwrap();
        assert_eq!(read_string_global_in("alpha", "__who"), "alpha");
        assert_eq!(read_string_global_in("beta",  "__who"), "beta");
        dispose_plugin_context("alpha");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("alpha")));
        shutdown();
    }

    // A recording hook-request callback: appends (descriptor, enable) to HOOKS.
    static HOOKS: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());
    extern "C" fn record_hook(name: *const c_char, enable: c_int) {
        let n = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        HOOKS.lock().unwrap().push((n, enable));
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

    #[test]
    fn microtasks_do_not_run_until_frame_drain() {
        init(dummy_logger()).unwrap();
        // With kExplicit, a resolved-promise continuation must NOT run during eval.
        eval("globalThis.__ran = false; Promise.resolve().then(() => { globalThis.__ran = true; });").unwrap();
        assert_eq!(read_bool_global("__ran"), false, "microtask ran before the drain");
        frame_async_drain(); // runs the checkpoint
        assert_eq!(read_bool_global("__ran"), true, "microtask did not run at the drain");
        shutdown();
    }

    #[test]
    fn onframe_handler_out_of_range_result_warns_and_continues() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval("onGameFrame(() => 99);").unwrap(); // 99 is out of range for HookResult
        let out = dispatch_onframe(crate::multiplexer::Phase::Pre, true, false, false);
        assert_eq!(out.result, crate::multiplexer::HookResult::Continue); // out-of-range → Continue
        let got = LOG.lock().unwrap().clone();
        assert!(
            got.iter().any(|m| m.to_lowercase().contains("out-of-range") || m.contains("99")),
            "expected an out-of-range warning, got: {:?}",
            got
        );
        shutdown();
    }

    #[test]
    fn delay_resolves_only_after_its_deadline() {
        init(dummy_logger()).unwrap();
        eval("globalThis.__d = false; Delay(30).then(() => { globalThis.__d = true; });").unwrap();
        frame_async_drain();                       // well before 30ms
        assert_eq!(read_bool_global("__d"), false);
        std::thread::sleep(std::time::Duration::from_millis(40));
        frame_async_drain();                       // now past the deadline
        assert_eq!(read_bool_global("__d"), true);
        shutdown();
    }

    #[test]
    fn next_frame_resolves_one_frame_later() {
        init(dummy_logger()).unwrap();
        eval("globalThis.__n = 0; NextFrame().then(() => { globalThis.__n = 1; });").unwrap();
        frame_async_drain(); // frame that schedules resolution for the NEXT frame → not yet
        // NextFrame targets FRAME_COUNTER+1 measured at call time; the drain that reaches it resolves it.
        assert_eq!(read_i32_global("__n"), 0);
        frame_async_drain();
        assert_eq!(read_i32_global("__n"), 1);
        shutdown();
    }

    #[test]
    fn delay_with_no_onframe_subscriber_still_requests_detour_install() {
        // Wire a recording request_hook (the ffi mock pattern) via set_hook_request BEFORE init.
        HOOKS.lock().unwrap().clear();
        set_hook_request(Some(record_hook));
        init(dummy_logger()).unwrap();
        eval("Delay(1000);").unwrap();  // pending async, zero onGameFrame subscribers
        assert!(HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 1),
                "Delay() should request the detour install");
        shutdown();
        set_hook_request(None);
    }

    #[test]
    fn async_completion_removes_detour_when_pending_reaches_zero() {
        HOOKS.lock().unwrap().clear();
        set_hook_request(Some(record_hook));
        init(dummy_logger()).unwrap();
        // Drain any stray pool completions from earlier tests so PENDING_JOBS starts clean.
        while pool().try_recv_completed().is_some() {}
        // With ZERO onGameFrame subscribers, start one async op that will complete on its own.
        // threadSleep(20) increments PENDING_JOBS → 1 and must drive an install.
        eval("threadSleep(20);").unwrap();
        // Assert the install was requested.
        assert!(
            HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 1),
            "threadSleep should request detour install"
        );
        // Drive the drain until the job completes and the remove fires.
        for _ in 0..500 {
            frame_async_drain();
            if HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Assert the remove transition was recorded: when PENDING_JOBS reached zero,
        // refresh_detour must have requested enable=0.
        assert!(
            HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 0),
            "async pending→0 must request detour remove"
        );
        // Assert install strictly precedes remove in HOOKS order, proving a real install→remove
        // transition rather than a spurious 0.
        let hooks = HOOKS.lock().unwrap();
        let install_idx = hooks
            .iter()
            .position(|(n, e)| n == "OnGameFrame" && *e == 1)
            .expect("install entry must be present");
        let remove_idx = hooks
            .iter()
            .skip(install_idx + 1)
            .position(|(n, e)| n == "OnGameFrame" && *e == 0)
            .map(|i| i + install_idx + 1)
            .expect("remove entry must follow install entry");
        assert!(
            install_idx < remove_idx,
            "install must precede remove in HOOKS: {:?}",
            *hooks
        );
        drop(hooks);
        shutdown();
        set_hook_request(None);
    }

    #[test]
    fn continuation_may_reenter_timer_primitives_during_checkpoint() {
        // Re-entrancy discipline: a resolved continuation that itself queues another timer
        // re-enters TIMERS/RESOLVERS from INSIDE perform_microtask_checkpoint. frame_async_drain
        // must hold no such borrow across the checkpoint, or this double-borrows and panics.
        init(dummy_logger()).unwrap();
        eval(r#"
            globalThis.__reentry = 0;
            NextTick().then(() => { NextTick().then(() => { globalThis.__reentry = 1; }); });
        "#).unwrap();
        // Drain 1 resolves the outer NextTick; its continuation queues the inner NextTick from
        // within the checkpoint (must not panic). A later drain resolves the inner → __reentry = 1.
        for _ in 0..5 { frame_async_drain(); }
        assert_eq!(read_i32_global("__reentry"), 1);
        shutdown();
    }

    #[test]
    fn thread_sleep_runs_off_thread_and_resolves_on_a_drain() {
        init(dummy_logger()).unwrap();
        eval("globalThis.__t = false; threadSleep(20).then(() => { globalThis.__t = true; });").unwrap();
        // Drive frames until the worker completes (bounded).
        let mut resolved = false;
        for _ in 0..500 {
            frame_async_drain();
            if read_bool_global("__t") { resolved = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(resolved, "threadSleep promise never resolved on a drain");
        shutdown();
    }

    /// `__s2_concommand` stores the JS callback in CONCOMMANDS; `dispatch_concommand` invokes it
    /// with (slot, argString).  This test exercises the store + dispatch path without the engine
    /// (calls `dispatch_concommand` directly, bypassing ConCommand registration).
    #[test]
    fn concommand_callback_receives_slot_and_args() {
        init(dummy_logger()).unwrap();
        eval(r#"
            globalThis.__cc = null;
            __s2_concommand("s2_test", function (slot, args) { globalThis.__cc = slot + ":" + args; });
        "#).unwrap();
        // Simulate the engine invoking the command (bypasses ConCommand registration):
        dispatch_concommand("s2_test", 3, "1234");
        assert_eq!(read_string_global("__cc"), "3:1234");
        shutdown();
    }

    /// `load_cs2_file` reads a JS file and evaluates it in the shared context (same scope
    /// construction as `eval`).  This verifies the load path is wired: a file that sets
    /// `globalThis.__loaded = 42` must be visible after the call.
    #[test]
    fn load_cs2_file_evaluates_in_context() {
        init(dummy_logger()).unwrap();
        let dir = std::env::temp_dir().join("s2_cs2_load_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("probe.js");
        std::fs::write(&f, "globalThis.__loaded = 41 + 1;").unwrap();
        load_cs2_file(f.to_str().unwrap());
        assert_eq!(read_i32_global("__loaded"), 42);
        shutdown();
    }

    /// Regression test: a stale completion from a prior isolate (id with no resolver in the current
    /// isolate) must NOT decrement PENDING_JOBS, or the detour would be removed while a real job is
    /// still in flight, causing the real completion to never be drained.
    ///
    /// Before the fix the unconditional decrement makes PENDING_JOBS go 1→0 on the stale id,
    /// causing the final assert to fail.  After the fix it stays at 1.
    #[test]
    fn stale_job_completion_does_not_undercount_pending() {
        init(dummy_logger()).unwrap();

        // Drain any completions left in the process-global pool from earlier tests.
        while pool().try_recv_completed().is_some() {}
        assert_eq!(
            PENDING_JOBS.with(|c| c.get()),
            0,
            "baseline: PENDING_JOBS should be 0 after draining strays"
        );

        // Submit a real in-flight job with a long sleep so it stays pending throughout.
        eval("threadSleep(1000).then(()=>{});").unwrap();
        assert_eq!(PENDING_JOBS.with(|c| c.get()), 1, "PENDING_JOBS should be 1 after submitting real job");

        // Inject a STALE completion for an id that has no resolver (mimics a prior isolate's leftover).
        // This does NOT touch PENDING_JOBS and stores no resolver.
        pool().submit(9_999_999, Box::new(|| Ok(())));

        // Wait briefly for the trivial stale job to land on the completion channel.
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Drain — the stale completion surfaces here; the 1000ms real job is still pending.
        frame_async_drain();

        assert_eq!(
            PENDING_JOBS.with(|c| c.get()),
            1,
            "stale completion must not undercount PENDING_JOBS"
        );

        shutdown();
    }
}
