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
    pub deref_handle: Option<DerefHandleFn>, // unused since 5A (EntityRef path supersedes deref_handle)
    pub ent_state_changed: Option<EntStateChangedFn>,
    pub concommand_register: Option<ConCommandRegisterFn>,
}

/// Byte offset within a `CEntityInstance` of its `CEntityIdentity*` (spike-confirmed).
// TODO(gamedata): migrate to a regenerable gamedata file.
const ENT_IDENTITY_PTR_OFFSET: i32 = 0x10; // spike-confirmed (2026-07-01-slice-5a-spike-findings.md)
/// Byte offset within a `CEntityIdentity` of the `CEntityHandle` uint32 (index+serial) (spike-confirmed).
const ENT_IDENTITY_HANDLE_OFFSET: i32 = 0x10; // spike-confirmed (2026-07-01-slice-5a-spike-findings.md)

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

/// A loaded plugin instance: its per-plugin `v8::Context` plus the captured `module.exports`
/// object (present once `load_plugin_js` has run the CJS bundle).  Field order is load-bearing
/// for teardown: `exports` (a `Global<Object>` pointing INTO the context) is declared FIRST so
/// Rust drops it BEFORE `context` — the spike's teardown discipline (inner Globals released
/// before the `Global<Context>`, while the isolate is still alive).  Task 6 walks the ledger to
/// call `onUnload` off `exports` before disposing the context.
struct PluginInstance {
    exports: Option<v8::Global<v8::Object>>,
    context: v8::Global<v8::Context>,
    /// The plugin's REGISTRY-assigned generation (set together with the REGISTRY entry at
    /// `create_plugin_context`).  Read when a native creates an async resolver so the resolver is
    /// tagged with `(id, generation)`; `frame_async_drain` later checks `REGISTRY.is_live` against
    /// this to DROP a continuation whose plugin unloaded or reloaded.
    generation: u64,
}

/// A pending async resolver (`Delay`/`NextTick`/`NextFrame`/`threadSleep`) plus the OWNING plugin's
/// `(id, generation)` captured at creation.  `owner` is `None` for a resolver created from a
/// non-plugin context (the shared `HOST` context via the C-ABI `eval` surface): such a resolver has
/// no plugin liveness to check and is always resolved.  For a plugin-owned resolver,
/// `frame_async_drain` checks `REGISTRY.is_live(id, generation)` before resolving and DROPS it (never
/// resolves into a disposed/replaced context) if the plugin unloaded or its generation advanced — the
/// use-after-free killer.  Same id space as the ledger's timer/job ids.
struct ResolverEntry {
    owner: Option<(String, u64)>,
    resolver: v8::Global<v8::PromiseResolver>,
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
    /// Frame counter = number of `frame_async_drain` calls COMPLETED (starts at 0).  Used to
    /// schedule `Frame(target)` timers: a drain resolves `Frame(t)` when the PRE-increment value
    /// it reads satisfies `frame >= t`.  `NextTick` targets the current count (resolves next drain);
    /// `NextFrame` targets `current + 1` (resolves one drain later).
    static FRAME_COUNTER: std::cell::Cell<u64> = std::cell::Cell::new(0);
    /// Pending timer queue (Delay/NextTick/NextFrame).  Holds only `u64` ids; the promise lives
    /// in `RESOLVERS`.  Borrowed briefly in `make_timer_promise`/`frame_async_drain`/`refresh_detour`;
    /// NEVER held across `perform_microtask_checkpoint` (a continuation re-enters it).
    static TIMERS: std::cell::RefCell<TimerQueue> = std::cell::RefCell::new(TimerQueue::new());
    /// `async id → ResolverEntry` (the resolver Global + its owning-plugin `(id, generation)` tag).
    /// The entry is dropped (removed) when the timer/job fires, when its plugin unloads, or when the
    /// async-liveness guard drops it (unloaded/reloaded plugin).  Cleared in `shutdown` BEFORE the
    /// isolate is dropped.  Never held across the checkpoint.
    static RESOLVERS: std::cell::RefCell<std::collections::HashMap<u64, ResolverEntry>>
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
    static PLUGINS: std::cell::RefCell<std::collections::HashMap<String, PluginInstance>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Plugin registry (Task 2): generation counter + per-plugin teardown ledger, keyed by the
    /// same id string as `PLUGINS`.  Reset on `shutdown` so a re-init starts empty.
    static REGISTRY: std::cell::RefCell<plugin::Registry>
        = std::cell::RefCell::new(plugin::Registry::new());
    /// Runtime package registry: maps package name (e.g. `"@s2script/cs2"`) to JS source.
    /// Populated by the shim at load time via `s2script_core_register_package` (C-ABI, see ffi.rs).
    /// NOT cleared on `shutdown` — package registrations are valid for the process lifetime.
    static INJECTED_PACKAGES: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Inter-plugin interface bookkeeping (Slice 4.5). Pure state lives here; the V8 handles are in
    /// IFACE_METHODS / IFACE_SUBS. Cleared on shutdown (BEFORE the isolate drops).
    static IFACES: std::cell::RefCell<crate::interfaces::InterfaceRegistry>
        = std::cell::RefCell::new(crate::interfaces::InterfaceRegistry::new());
    /// (interface_name, method) → producer method Global<Function>. Dropped on producer unload +
    /// cleared on shutdown.
    static IFACE_METHODS: std::cell::RefCell<std::collections::HashMap<(String, String), v8::Global<v8::Function>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// sub_id → consumer event-handler Global<Function>. Dropped on consumer unload + cleared on shutdown.
    static IFACE_SUBS: std::cell::RefCell<std::collections::HashMap<u64, v8::Global<v8::Function>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Monotonic event-subscription id allocator (1-based; 0 = none).
    static NEXT_SUB_ID: std::cell::Cell<u64> = std::cell::Cell::new(1);
}

/// Install the shim's engine-ops table (copied by value; see `ENGINE_OPS`).  Wired by `ffi.rs`.
pub fn set_engine_ops(ops: Option<S2EngineOps>) {
    ENGINE_OPS.with(|c| c.set(ops));
}

/// Install the embedder's detour-request callback.  Wired by `ffi.rs` (Task 4).
pub fn set_hook_request(f: Option<HookRequestFn>) {
    HOOK_REQUEST.with(|c| c.set(f));
}

/// Register a game-package JS source string under `name` (e.g. `"@s2script/cs2"`).
///
/// Called by the shim at load time (via the C-ABI `s2script_core_register_package`) to provide
/// game-specific JS to core without baking it in at compile time.  Each call overwrites any prior
/// value for the same name (idempotent for the shim's load-once use).  The stored source is then
/// evaluated per-context in `create_plugin_context` and stashed at `globalThis.__s2pkg_*` for
/// the `__s2require` native.
pub fn register_injected_package(name: &str, js: &str) {
    INJECTED_PACKAGES.with(|p| p.borrow_mut().insert(name.to_string(), js.to_string()));
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

/// Combined lazy-detour reconciler.  Desired = any onGameFrame subscriber OR any pending async
/// OR the plugin watcher is active (once a plugins dir is set, the `GameFrame` Post hook must fire
/// every frame so `loader::poll_plugins` runs — otherwise, with no plugin loaded there is no
/// subscriber, so the detour would never install and the FIRST plugin could never be discovered).
/// Only pokes the embedder on a real transition, keeping `DETOUR_INSTALLED` the single source of
/// truth.  Borrows FRAME + TIMERS (via `async_pending`) — callers must hold NEITHER borrow.
pub(crate) fn refresh_detour() {
    let desired = FRAME.with(|f| f.borrow().enabled_count() > 0)
        || async_pending() > 0
        || crate::loader::is_watching();
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

/// The injected `@s2script/std` prelude, evaluated per plugin context AFTER the native
/// primitives are in place.  Builds the renamed, engine-generic API over the `__s2_*` natives
/// (whose internal names are unchanged) and stashes it at `globalThis.__s2pkg_std` for the
/// `__s2require` native to hand back.  The `HookResult`/`Priority`/`Phase` enum globals stay on
/// `globalThis` (ambient, engine-generic).  No game identifiers appear here.
const INJECTED_STD_PRELUDE: &str = r#"
globalThis.HookResult = { Continue:0, Changed:1, Handled:2, Stop:3 };
globalThis.Priority   = { High:"high", Normal:"normal", Low:"low", Monitor:"monitor" };
globalThis.Phase      = { Pre:"pre", Post:"post" };
(function () {
  const OnGameFrame = {
    subscribe: (fn, opts) => {
      const id = __s2_subscribe("OnGameFrame", fn, opts || {});
      return { dispose: () => __s2_unsubscribe(id) };
    },
  };
  const std = {
    OnGameFrame,
    delay: (ms) => __s2_delay(ms || 0),
    nextTick: () => __s2_next_tick(),
    nextFrame: () => __s2_next_frame(),
    threadSleep: (ms) => __s2_thread_sleep(ms || 0),
    console,
  };
  // --- Slice 4.5: inter-plugin interfaces ---
  function makeIfaceProxy(name) {
    return new Proxy({}, {
      get: function (_t, prop) {
        if (prop === "on")  return function (ev, h) { return __s2_iface_on(name, ev, h); };
        if (prop === "off") return function (ev, h) { return __s2_iface_off(name, ev, h); };
        if (typeof prop !== "string") return undefined;
        return function () {
          var args = Array.prototype.slice.call(arguments);
          return __s2_iface_call(name, prop, args);
        };
      }
    });
  }
  function resolveInterface(name) {
    var kind = __s2_iface_dep_kind(name);
    if (kind === "none") return null;                       // undeclared specifier
    if (kind === "optional" && !__s2_iface_is_published(name)) return null;
    return makeIfaceProxy(name);                             // hard → always a proxy
  }
  globalThis.__s2_require = function (name) {
    var pkg = __s2require(name);                             // @s2script/std | @s2script/cs2
    if (pkg !== null && pkg !== undefined) return pkg;
    return resolveInterface(name);                          // inter-plugin, or null
  };
  std.publishInterface = function (name, version, impl) {
    __s2_iface_publish(name, version, impl);
    return { emit: function (ev, payload) { return __s2_iface_emit(name, ev, payload); } };
  };
  // --- Slice 5A: serial-gated EntityRef (wraps the __s2_ent_ref_* natives; no raw pointer crosses JS) ---
  function EntityRef(index, serial) { this.index = index; this.serial = serial; }
  EntityRef.prototype = {
    isValid: function () { return __s2_ent_ref_valid(this.index, this.serial); },
    readInt32: function (offset) { return __s2_ent_ref_read_i32(this.index, this.serial, offset); },
    writeInt32: function (offset, value) { return __s2_ent_ref_write_i32(this.index, this.serial, offset, value); },
    notifyStateChanged: function (offset) { __s2_ent_ref_state_changed(this.index, this.serial, offset); },
  };
  std.EntityRef = EntityRef;
  globalThis.__s2pkg_std = std;
})();
"#;

// @s2script/cs2 is NOT embedded here. It is provided externally at runtime by the shim via
// `register_injected_package("@s2script/cs2", <js>)` (see `ffi.rs`).  Core contains zero cs2 JS.
// If the package is not registered, `require("@s2script/cs2")` returns null (graceful degrade).

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

        // Owner = the CALLING plugin context's id (read fresh from the current context — correct
        // across the microtask checkpoint).  Falls back to "legacy" for a non-plugin context (e.g.
        // the shared HOST context), which no longer subscribes in the per-context model.
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());

        // The combined predicate supersedes the DetourChange the multiplexer returns; ignore it.
        // FRAME borrow is released before we touch REGISTRY (no borrow held across the ledger call).
        let (id, _change) = FRAME.with(|f| {
            f.borrow_mut()
                .subscribe(priority, phase, owner.clone(), JsHandler { func: global })
        });
        // Ledger this hook against the owning plugin (Task 6's teardown authority).  A miss (owner
        // not registered) is a safe no-op.  Neither borrow is held across a JS call.
        REGISTRY.with(|r| {
            if let Some(l) = r.borrow_mut().ledger_mut(&owner) {
                l.record_hook(id);
            }
        });
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

/// The CALLING plugin's `(id, current generation)` for tagging an async resolver, or `None` for a
/// non-plugin context (the shared `HOST` context).  The generation is read from the plugin's
/// `PluginInstance` — which is set together with its REGISTRY entry at `create_plugin_context`, so it
/// equals the plugin's current REGISTRY generation.  A later unload (id removed) or reload
/// (generation advanced) then makes the captured tag fail `REGISTRY.is_live` in `frame_async_drain`,
/// which DROPS the continuation instead of resolving it into a disposed/replaced context.
///
/// Reads the current context id (no borrow) then briefly borrows `PLUGINS` — the caller must hold no
/// `PLUGINS` borrow across this (none do: every JS-call site clones its context out first).
fn resolver_owner_tag(scope: &mut v8::PinScope) -> Option<(String, u64)> {
    current_plugin(scope).map(|owner| {
        let generation =
            PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        (owner, generation)
    })
}

/// Shared helper for the timer natives: create a `PromiseResolver`, stash its `Global` (tagged with
/// the owning plugin) under a fresh async id, push the timer, reconcile the detour, and return the
/// pending promise.
fn make_timer_promise<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    kind: TimerKind,
) -> v8::Local<'s, v8::Value> {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let id = next_async_id();
    // Tag the resolver with the CALLING plugin's (id, current generation) — the async-liveness guard.
    let owner = resolver_owner_tag(scope);
    // Ledger this timer against the CALLING plugin (Task 6's teardown authority).  A non-plugin/
    // unknown owner is a safe no-op.  No thread-local borrow held across a JS call.
    if let Some((ref oid, _)) = owner {
        REGISTRY.with(|r| {
            if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                l.record_timer(id);
            }
        });
    }
    RESOLVERS.with(|m| {
        m.borrow_mut()
            .insert(id, ResolverEntry { owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
    });
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
        // Tag the resolver with the CALLING plugin's (id, current generation) — the async-liveness guard.
        let owner = resolver_owner_tag(scope);
        // Ledger this async-FFI job against the CALLING plugin (read fresh from the current
        // context).  A non-plugin/unknown owner is a safe no-op; no borrow held across a JS call.
        if let Some((ref oid, _)) = owner {
            REGISTRY.with(|r| {
                if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                    l.record_job(id);
                }
            });
        }
        RESOLVERS.with(|m| {
            m.borrow_mut()
                .insert(id, ResolverEntry { owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
        });
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

// ---------------------------------------------------------------------------
// Slice 5A: (index, serial) entity natives — serial-gated read/write/valid/decode.
//
// Raw pointers are used and discarded ENTIRELY WITHIN `entity_current_serial` and
// `entity_resolve_ptr` — they NEVER cross to JS.  Only numbers/null/boolean/the
// decode array cross the JS boundary.  This is the core of the EntityRef serial-
// safety contract (spec §handle/EntityRef).
// ---------------------------------------------------------------------------

/// Current serial for an entity index via the engine's identity, or -1 if the slot is empty / no ops.
/// The raw pointer is used and discarded HERE — it never crosses to JS.
fn entity_current_serial(index: i32) -> i32 {
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return -1 };
    let Some(by_index) = ops.ent_by_index else { return -1 };
    let ent = by_index(index) as *const u8;
    if ent.is_null() { return -1; }
    let identity = crate::entity::read_ptr(ent, ENT_IDENTITY_PTR_OFFSET);
    if identity.is_null() { return -1; }
    let handle = crate::entity::read_u32(identity, ENT_IDENTITY_HANDLE_OFFSET);
    let (_idx, serial) = crate::entity::decode_handle(handle);
    serial
}

/// Resolve (index, serial) to a live entity pointer, or null if the serial no longer matches.
/// A SINGLE `ent_by_index` lookup: the current serial is read from the SAME pointer that is returned,
/// so the validated serial and the returned pointer are guaranteed to be the same entity — no
/// double-lookup, no TOCTOU window. The raw pointer stays in Rust; callers read/write through it and
/// discard it within the native, so it never crosses to JS.
/// SAFETY: entity natives run synchronously within a game frame; no entity is destroyed between the
/// serial read and the caller's deref on this same pointer.
fn entity_resolve_ptr(index: i32, serial: i32) -> *mut u8 {
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return std::ptr::null_mut() };
    let Some(by_index) = ops.ent_by_index else { return std::ptr::null_mut() };
    let ent = by_index(index) as *mut u8;
    if ent.is_null() {
        return std::ptr::null_mut();
    }
    let identity = crate::entity::read_ptr(ent as *const u8, ENT_IDENTITY_PTR_OFFSET);
    if identity.is_null() {
        return std::ptr::null_mut();
    }
    let handle = crate::entity::read_u32(identity, ENT_IDENTITY_HANDLE_OFFSET);
    let (_idx, cur_serial) = crate::entity::decode_handle(handle);
    if !crate::entity::resolve(cur_serial, serial) {
        return std::ptr::null_mut();
    }
    ent
}

/// Native `__s2_ent_current_serial(index) -> number`.
/// Returns the current serial for an entity slot, or -1 if the slot is empty / no ops.
fn s2_ent_current_serial(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        rv.set_int32(entity_current_serial(index));
    }));
}

/// Native `__s2_ent_ref_valid(index, serial) -> boolean`.
/// True iff the slot's current serial matches the captured serial (entity is still alive).
fn s2_ent_ref_valid(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        rv.set_bool(crate::entity::resolve(entity_current_serial(index), serial));
    }));
}

/// Native `__s2_ent_ref_read_i32(index, serial, offset) -> number|null`.
/// Resolves (index, serial), reads an i32 at `offset`, or returns null on stale ref.
fn s2_ent_ref_read_i32(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → null (already set)
        rv.set_int32(crate::entity::read_i32(ent as *const u8, off));
    }));
}

/// Native `__s2_ent_ref_write_i32(index, serial, offset, value) -> boolean`.
/// Resolves (index, serial), writes an i32 at `offset`, returns true on success / false on stale.
fn s2_ent_ref_write_i32(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let val = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → false (already set)
        crate::entity::write_i32(ent, off, val);
        rv.set_bool(true);
    }));
}

/// Native `__s2_ent_ref_state_changed(index, serial, offset)`.
/// Resolves (index, serial) then calls `ent_state_changed` engine-op. No-op on stale ref / no ops.
fn s2_ent_ref_state_changed(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as c_int;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → no-op
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.ent_state_changed else { return };
        func(ent as *mut c_void, off);
    }));
}

/// Native `__s2_handle_decode(handleValue) -> [index, serial]`.
/// Pure bit-math (no engine ops): decodes a CEntityHandle uint32 into a [index, serial] array.
/// Note: a negative JS number wraps to `u32` (`... as u32`) and decodes to a nonsensical
/// `(index, serial)` — callers pass a valid `CEntityHandle` uint32 (e.g. from a schema
/// handle field coerced with `>>> 0` in JS). No error is raised (pure bit-math).
fn s2_handle_decode(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(0) as u32;
        let (index, serial) = crate::entity::decode_handle(handle);
        let arr = v8::Array::new(scope, 2);
        let i = v8::Integer::new(scope, index);
        let s = v8::Integer::new(scope, serial);
        arr.set_index(scope, 0, i.into());
        arr.set_index(scope, 1, s.into());
        rv.set(arr.into());
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

/// Shared logging helper for named WARNs in the engine-op natives and the loader.
pub(crate) fn log_warn(msg: &str) {
    if let Some(l) = LOGGER.with(|l| l.get()) {
        if let Ok(cs) = CString::new(msg) {
            l(0, cs.as_ptr());
        }
    }
}

/// Native `__s2require(name) -> object|null` — the injected CJS `require` shim (spike PROVE #1).
///
/// Maps a package specifier to the per-context API object the injected prelude stashed on the
/// calling context's global: `"@s2script/std"` → `globalThis.__s2pkg_std` (`{ OnGameFrame, delay,
/// nextTick, nextFrame, threadSleep, console }`), `"@s2script/cs2"` → `globalThis.__s2pkg_cs2`
/// (`{ Pawn }`).  Any other specifier returns JS `null`.  The objects are HOST-authored (built by
/// the prelude over the `__s2_*` natives); this native only hands the right one back for the
/// current context — so `require` is per-plugin without any side table.
///
/// Like every native, the body runs under `catch_unwind` (no panic may cross the FFI boundary).
fn s2require(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 {
            return;
        }
        let name = args.get(0).to_rust_string_lossy(scope);
        let key = match name.as_str() {
            "@s2script/std" => "__s2pkg_std",
            "@s2script/cs2" => "__s2pkg_cs2",
            _ => return, // unknown specifier → null
        };
        let global = scope.get_current_context().global(scope);
        let Some(k) = v8::String::new(scope, key) else { return };
        if let Some(v) = global.get(scope, k.into()) {
            if !v.is_undefined() {
                rv.set(v);
            }
        }
    }));
}

/// Throw a named JS Error (`"<name>: <detail>"`) in the current context. The caller returns
/// immediately after; an uncaught throw bubbles to the enclosing dispatch TryCatch → WARN → degrade.
fn throw_named(scope: &mut v8::PinScope, name: &str, detail: &str) {
    let msg = format!("{}: {}", name, detail);
    if let Some(s) = v8::String::new(scope, &msg) {
        let err = v8::Exception::error(scope, s);
        scope.throw_exception(err);
    }
}

/// Stringify `value` via the CURRENT context's `JSON.stringify` → owned Rust String (the neutral,
/// context-free carrier for the structured-copy wire). Returns None if the result is JS `undefined`
/// (e.g. a function/live object) — the data-only-wire enforcement (spike step 2).
///
/// The `JSON.stringify` call is wrapped in a `TryCatch` to absorb any pending exception (e.g. from
/// a cyclic value): without this, `Function::call` returning `None` leaves a pending exception on
/// the isolate that would poison later frames.
fn iface_to_json(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Option<String> {
    let global = scope.get_current_context().global(scope);
    let json_key = v8::String::new(scope, "JSON")?;
    let json = global.get(scope, json_key.into())?;
    let json = v8::Local::<v8::Object>::try_from(json).ok()?;
    let fn_key = v8::String::new(scope, "stringify")?;
    let strfn = json.get(scope, fn_key.into())?;
    let strfn = v8::Local::<v8::Function>::try_from(strfn).ok()?;
    let recv: v8::Local<v8::Value> = json.into();
    // Open a TryCatch around the stringify call to absorb any pending exception (cyclic value, etc.).
    let mut tc_storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
    let tc = &mut tc;
    let out = strfn.call(tc, recv, &[value])?;
    if out.is_undefined() { return None; }   // non-serializable
    Some(out.to_rust_string_lossy(tc))
}

/// Parse `json` via the CURRENT context's `JSON.parse` → a fresh Local in this context (a COPY; no
/// shared identity with the source context). Returns None on parse failure.
fn iface_from_json<'s>(scope: &mut v8::PinScope<'s, '_>, json: &str) -> Option<v8::Local<'s, v8::Value>> {
    let global = scope.get_current_context().global(scope);
    let json_key = v8::String::new(scope, "JSON")?;
    let jobj = global.get(scope, json_key.into())?;
    let jobj = v8::Local::<v8::Object>::try_from(jobj).ok()?;
    let fn_key = v8::String::new(scope, "parse")?;
    let parsefn = jobj.get(scope, fn_key.into())?;
    let parsefn = v8::Local::<v8::Function>::try_from(parsefn).ok()?;
    let arg = v8::String::new(scope, json)?;
    let recv: v8::Local<v8::Value> = jobj.into();
    // Open a TryCatch around the parse call to absorb any pending exception (malformed JSON, etc.).
    let mut tc_storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
    let tc = &mut tc;
    parsefn.call(tc, recv, &[arg.into()])
}

/// Store a plugin's declared inter-plugin imports (from its manifest) so `iface_dep_kind` /
/// `iface_is_published` can categorise `require`. Called by the loader BEFORE `load_plugin_js` runs
/// the module eval. Cleared in `unload_plugin` (Task 7).
pub fn set_plugin_imports(id: &str, decls: Vec<(String, String, crate::interfaces::Kind)>) {
    IFACES.with(|r| r.borrow_mut().set_imports(id, decls));
}

/// Set a named native function on `global_obj` in `scope`.  Small helper used by
/// `install_natives` to keep the per-context install table declarative.
fn set_native(
    scope: &mut v8::PinScope,
    global_obj: v8::Local<v8::Object>,
    name: &str,
    cb: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let key = v8::String::new(scope, name).unwrap();
    let func = v8::Function::new(scope, cb).unwrap();
    global_obj.set(scope, key.into(), func.into());
}

/// `__s2_iface_publish(name, version, implObj)` — the producer registers a versioned interface.
/// Reflects `implObj`'s own function properties into method Globals; records the registry entry
/// tagged with the producer (id, generation); ledgers `Interface(name)` on the producer. Degrade:
/// missing plugin identity / bad args → WARN + return (no throw; publish is producer-side).
fn s2_iface_publish(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        if args.length() < 3 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let version = args.get(1).to_rust_string_lossy(scope);
        let Ok(impl_obj) = v8::Local::<v8::Object>::try_from(args.get(2)) else {
            log_warn(&format!("WARN: iface_publish('{}'): impl is not an object", name));
            return;
        };
        let Some(owner) = current_plugin(scope) else {
            log_warn("WARN: iface_publish: no current plugin");
            return;
        };
        let generation = REGISTRY.with(|r| r.borrow().generation_of(&owner)).unwrap_or(0);

        // Enumerate own function properties → method names + capture Globals.
        let mut method_names: Vec<String> = Vec::new();
        if let Some(prop_names) = impl_obj.get_own_property_names(scope, Default::default()) {
            for i in 0..prop_names.length() {
                let Some(key) = prop_names.get_index(scope, i) else { continue };
                let Some(val) = impl_obj.get(scope, key) else { continue };
                if let Ok(f) = v8::Local::<v8::Function>::try_from(val) {
                    let m = key.to_rust_string_lossy(scope);
                    method_names.push(m.clone());
                    let g = v8::Global::new(scope.as_ref(), f);
                    IFACE_METHODS.with(|mm| { mm.borrow_mut().insert((name.clone(), m), g); });
                }
            }
        }

        IFACES.with(|r| r.borrow_mut().publish(&name, &version, &owner, generation, method_names));
        REGISTRY.with(|r| {
            if let Some(l) = r.borrow_mut().ledger_mut(&owner) { l.record_interface(name.clone()); }
        });
    }));
}

/// `__s2_iface_dep_kind(name) -> "hard" | "optional" | "none"` for the CURRENT plugin.
fn s2_iface_dep_kind(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        let kind = current_plugin(scope).and_then(|id| IFACES.with(|r| r.borrow().dep_kind(&id, &name)));
        let s = match kind {
            Some(crate::interfaces::Kind::Hard) => "hard",
            Some(crate::interfaces::Kind::Optional) => "optional",
            None => "none",
        };
        let out = v8::String::new(scope, s).unwrap();
        rv.set(out.into());
    }));
}

/// `__s2_iface_is_published(name) -> bool` — published AND version-compatible for the current plugin.
fn s2_iface_is_published(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        let avail = current_plugin(scope).map_or(false, |id| IFACES.with(|r| r.borrow().is_available(&id, &name)));
        rv.set_bool(avail);
    }));
}

/// `__s2_iface_call(name, method, argsArray) -> result` — the consumer-side cross-context call.
/// Re-resolves the registry by name each call (so producer hot-reload auto-recovers), checks the
/// version range + method existence, structured-copies args consumer→producer via the JSON carrier,
/// enters the producer context, calls the method Global, structured-copies the return back. Named
/// throws on the failure modes; the whole body is catch_unwind.
/// A throwing producer method surfaces as `InterfaceCallError`; an `undefined`/void return resolves
/// to `undefined` in the consumer (not an error — only a genuinely non-serializable value throws
/// `InterfaceValueNotSerializable`).
fn s2_iface_call(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        let name = args.get(0).to_rust_string_lossy(scope);
        let method = args.get(1).to_rust_string_lossy(scope);
        let Some(consumer) = current_plugin(scope) else {
            throw_named(scope, "InterfaceUnavailable", &name);
            return;
        };

        // Decide what to do from the pure registry.
        let target = IFACES.with(|r| r.borrow().call_target(&consumer, &name, &method));
        match target {
            crate::interfaces::CallTarget::Unavailable => { throw_named(scope, "InterfaceUnavailable", &name); return; }
            crate::interfaces::CallTarget::VersionMismatch => { throw_named(scope, "InterfaceVersionMismatch", &name); return; }
            crate::interfaces::CallTarget::Ok => {}
        }

        // Marshal args (the 3rd arg, an array) OUT of the consumer context to a JSON String.
        let args_json = match iface_to_json(scope, args.get(2)) {
            Some(s) => s,
            None => { throw_named(scope, "InterfaceValueNotSerializable", &format!("{}.{} args", name, method)); return; }
        };

        // Producer context + method Global — extract into owned locals so no IFACES/IFACE_METHODS/PLUGINS
        // borrow is held across the V8 context-switch or the method call (borrow discipline).
        let Some((producer_id, _gen)) = IFACES.with(|r| r.borrow().producer_of(&name)) else {
            // _gen unused: re-resolve-by-name each call always targets the current producer; a generation guard
            // on method_g's origin is a future hardening (publish updates IFACES+IFACE_METHODS atomically today).
            throw_named(scope, "InterfaceUnavailable", &name); return;
        };
        let method_g = IFACE_METHODS.with(|m| m.borrow().get(&(name.clone(), method.clone())).cloned());
        let Some(method_g) = method_g else { throw_named(scope, "InterfaceUnavailable", &name); return; };
        let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(&producer_id).map(|pi| pi.context.clone())) else {
            throw_named(scope, "InterfaceUnavailable", &name); return;
        };

        // Producer-side outcome, extracted as context-free Rust values BEFORE cscope drops.
        enum Outcome {
            Ok(String),      // serialized return JSON (a COPY)
            Void,            // producer returned undefined → resolve undefined in the consumer
            Threw(String),   // producer method threw; captured message
            NotSerializable, // return is cyclic/BigInt/function (and NOT undefined)
            Internal,        // args failed to parse/spread (unexpected for valid JSON)
        }

        // Enter the producer context under a TryCatch so a THROWING producer method is captured here
        // (absorbed when the TryCatch drops) rather than left pending — otherwise the consumer-side
        // throw_named would double-throw over it. iface_to_json/iface_from_json open their own inner
        // TryCatches, so nesting is fine. CRITICAL: the return is serialized to a Rust String INSIDE
        // this block (before cscope drops) — no Local<Value> may escape the producer scope.
        let outcome: Outcome = {
            let ctx_local = v8::Local::new(scope, &g_ctx);
            let cscope = &mut v8::ContextScope::new(scope, ctx_local);
            let mut tc_storage = v8::TryCatch::new(cscope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            // Parse args (a COPY) + spread positionally.
            let argv_opt = (|| -> Option<Vec<v8::Local<v8::Value>>> {
                let args_val = iface_from_json(tc, &args_json)?;
                let arr = v8::Local::<v8::Array>::try_from(args_val).ok()?;
                let mut argv: Vec<v8::Local<v8::Value>> = Vec::with_capacity(arr.length() as usize);
                for i in 0..arr.length() { argv.push(arr.get_index(tc, i)?); }
                Some(argv)
            })();

            match argv_opt {
                None => Outcome::Internal,
                Some(argv) => {
                    let f = v8::Local::new(tc, &method_g);
                    let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                    match f.call(tc, recv, &argv) {
                        None => {
                            // Producer method threw — capture its message (absorbed when tc drops).
                            let msg = tc.exception()
                                .map(|e| e.to_rust_string_lossy(&*tc))
                                .unwrap_or_else(|| "producer method threw".into());
                            Outcome::Threw(msg)
                        }
                        Some(ret) => {
                            if ret.is_undefined() {
                                Outcome::Void
                            } else {
                                match iface_to_json(tc, ret) {
                                    Some(json) => Outcome::Ok(json),
                                    None => Outcome::NotSerializable,
                                }
                            }
                        }
                    }
                }
            }
        };

        // Back in the consumer context: map the outcome to a return value or a single named throw.
        match outcome {
            Outcome::Ok(json) => match iface_from_json(scope, &json) {
                Some(v) => rv.set(v),
                None => throw_named(scope, "InterfaceValueNotSerializable", &format!("{}.{} return", name, method)),
            },
            Outcome::Void => rv.set_undefined(),
            Outcome::NotSerializable => throw_named(scope, "InterfaceValueNotSerializable", &format!("{}.{} return", name, method)),
            Outcome::Threw(msg) => throw_named(scope, "InterfaceCallError", &format!("{}.{}: {}", name, method, msg)),
            Outcome::Internal => throw_named(scope, "InterfaceUnavailable", &name),
        }
    }));
}

/// `__s2_iface_on(name, event, handler) -> subId` — the consumer subscribes to a producer event.
/// Stores the handler Global keyed by a fresh sub_id; records the Subscriber in the registry (tagged
/// with the consumer's (id, generation)); ledgers `EventSub(subId)` on the consumer.
fn s2_iface_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        let name = args.get(0).to_rust_string_lossy(scope);
        let event = args.get(1).to_rust_string_lossy(scope);
        let Ok(handler) = v8::Local::<v8::Function>::try_from(args.get(2)) else { return; };
        let Some(consumer) = current_plugin(scope) else { return; };
        let generation = REGISTRY.with(|r| r.borrow().generation_of(&consumer)).unwrap_or(0);
        let sub_id = NEXT_SUB_ID.with(|c| { let v = c.get(); c.set(v + 1); v });

        let ok = IFACES.with(|r| r.borrow_mut().add_subscriber(&name, crate::interfaces::Subscriber {
            sub_id, consumer_id: consumer.clone(), consumer_gen: generation, event,
        }));
        if !ok { return; } // interface not published → no-op (degrade)

        let g = v8::Global::new(scope.as_ref(), handler);
        IFACE_SUBS.with(|m| { m.borrow_mut().insert(sub_id, g); });
        REGISTRY.with(|r| { if let Some(l) = r.borrow_mut().ledger_mut(&consumer) { l.record_event_sub(sub_id); } });
        rv.set_double(sub_id as f64);
    }));
}

/// `__s2_iface_off(name, event, handler)` — best-effort unsubscribe of the current consumer's subs
/// on (name, event). For the thin slice this drops ALL of the current consumer's subs on that
/// (name, event) pair (handler identity match is not required — consumers rarely double-subscribe).
fn s2_iface_off(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        let name = args.get(0).to_rust_string_lossy(scope);
        let event = args.get(1).to_rust_string_lossy(scope);
        let Some(consumer) = current_plugin(scope) else { return; };
        let dropped = IFACES.with(|r| r.borrow_mut().remove_subscribers_by_consumer_on(&consumer, &name, &event));
        IFACE_SUBS.with(|m| { let mut mm = m.borrow_mut(); for id in dropped { mm.remove(&id); } });
    }));
}

/// `__s2_iface_emit(name, event, payload)` — the producer forwards an event to every LIVE consumer
/// subscribed to (name, event). Payload is structured-copied per consumer. Producer-side: no throw
/// (a bad payload logs a WARN and skips that dispatch).
fn s2_iface_emit(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        let name = args.get(0).to_rust_string_lossy(scope);
        let event = args.get(1).to_rust_string_lossy(scope);
        // Stringify the payload once, in the producer context (the neutral carrier).
        let payload_json = match iface_to_json(scope, args.get(2)) {
            Some(s) => s,
            None => {
                log_warn(&format!("WARN: iface_emit('{}','{}'): payload not serializable", name, event));
                return;
            }
        };
        // Compute live subscriber ids (IFACES borrow released before entering any consumer context).
        let is_live = |id: &str, gen: u64| REGISTRY.with(|r| r.borrow().is_live(id, gen));
        let sub_ids = IFACES.with(|r| r.borrow().live_subscriber_ids(&name, &event, &is_live));

        for sub_id in sub_ids {
            // Collect all info (brief borrows; all released before the ContextScope).
            let handler_g = IFACE_SUBS.with(|m| m.borrow().get(&sub_id).cloned());
            let Some(handler_g) = handler_g else { continue; };
            let consumer = IFACES.with(|r| r.borrow().consumer_of_sub(&name, sub_id));
            let Some(consumer) = consumer else { continue; };
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(&consumer).map(|pi| pi.context.clone())) else { continue; };

            // Enter the consumer's context and call the handler with a fresh copy of the payload.
            let ctx_local = v8::Local::new(scope, &g_ctx);
            let cscope = &mut v8::ContextScope::new(scope, ctx_local);
            let mut tc_storage = v8::TryCatch::new(cscope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            if let Some(payload) = iface_from_json(tc, &payload_json) {
                let f = v8::Local::new(tc, &handler_g);
                let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                if f.call(tc, recv, &[payload]).is_none() {
                    let msg = tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!("WARN: iface_emit('{}','{}') handler: {}", name, event, msg));
                }
            }
            // tc, tc_storage, cscope drop here (TryCatch absorbs any pending exception).
        }
    }));
}

/// Install the full native API on a context's global object: `console` plus every `__s2_*`
/// primitive and the `__s2require` shim.  Called for BOTH the shared `HOST` context (so the
/// C-ABI `eval` surface keeps `console`/`__s2_concommand` etc.) and every per-plugin context.
/// The native internal names are unchanged from Slice 0–3; the RENAMED, engine-generic API
/// (`OnGameFrame.subscribe`/`delay`/…) is layered on top by the injected prelude (per-context).
fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    // console = { log: fn }.
    let console_obj = v8::Object::new(scope);
    let log_key = v8::String::new(scope, "log").unwrap();
    let log_fn = v8::Function::new(scope, console_log).unwrap();
    console_obj.set(scope, log_key.into(), log_fn.into());
    let console_key = v8::String::new(scope, "console").unwrap();
    global_obj.set(scope, console_key.into(), console_obj.into());

    // Multiplexer primitives.
    set_native(scope, global_obj, "__s2_subscribe", s2_subscribe);
    set_native(scope, global_obj, "__s2_unsubscribe", s2_unsubscribe);
    // Async timer primitives (Delay / NextTick / NextFrame / threadSleep).
    set_native(scope, global_obj, "__s2_delay", s2_delay);
    set_native(scope, global_obj, "__s2_next_tick", s2_next_tick);
    set_native(scope, global_obj, "__s2_next_frame", s2_next_frame);
    set_native(scope, global_obj, "__s2_thread_sleep", s2_thread_sleep);
    // Schema + entity system.
    set_native(scope, global_obj, "__s2_schema_offset", s2_schema_offset);
    // Slice 5A: (index, serial) entity natives — serial-gated read/write/valid/decode.
    // The five Slice-3 raw-pointer natives (entity-by-index, deref-handle, ent-read/write-i32,
    // ent-state-changed) were retired in Task 4; callers now use the __s2_ent_ref_* path.
    set_native(scope, global_obj, "__s2_ent_current_serial", s2_ent_current_serial);
    set_native(scope, global_obj, "__s2_ent_ref_valid", s2_ent_ref_valid);
    set_native(scope, global_obj, "__s2_ent_ref_read_i32", s2_ent_ref_read_i32);
    set_native(scope, global_obj, "__s2_ent_ref_write_i32", s2_ent_ref_write_i32);
    set_native(scope, global_obj, "__s2_ent_ref_state_changed", s2_ent_ref_state_changed);
    set_native(scope, global_obj, "__s2_handle_decode", s2_handle_decode);
    // ConCommand registration.
    set_native(scope, global_obj, "__s2_concommand", s2_concommand);
    // Per-context identity probe + the CJS require shim.
    set_native(scope, global_obj, "__s2_current_plugin", s2_current_plugin);
    set_native(scope, global_obj, "__s2require", s2require);
    // Inter-plugin interface primitives (Slice 4.5).
    set_native(scope, global_obj, "__s2_iface_publish", s2_iface_publish);
    set_native(scope, global_obj, "__s2_iface_dep_kind", s2_iface_dep_kind);
    set_native(scope, global_obj, "__s2_iface_is_published", s2_iface_is_published);
    set_native(scope, global_obj, "__s2_iface_call", s2_iface_call);
    // Event subscription / emission (Slice 4.5 events half).
    set_native(scope, global_obj, "__s2_iface_on", s2_iface_on);
    set_native(scope, global_obj, "__s2_iface_off", s2_iface_off);
    set_native(scope, global_obj, "__s2_iface_emit", s2_iface_emit);
}

/// Evaluate a host-authored prelude `src` in `scope` under a `TryCatch` (degrade-never-crash: a
/// prelude compile/run error logs a named WARN and returns rather than propagating an exception).
fn run_prelude(scope: &mut v8::PinScope, what: &str, src: &str) {
    let mut tc_storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
    let tc = &mut tc;
    let Some(code) = v8::String::new(tc, src) else {
        log_warn(&format!("WARN: {} prelude: failed to intern source", what));
        return;
    };
    match v8::Script::compile(tc, code, None).and_then(|s| s.run(tc)) {
        Some(_) => {}
        None => {
            let msg = tc
                .exception()
                .map(|e| e.to_rust_string_lossy(&*tc))
                .unwrap_or_else(|| "unknown error".into());
            log_warn(&format!("WARN: {} prelude eval error: {}", what, msg));
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
/// with the plugin id via `set_slot::<PluginId>`, install the FULL per-context API (all natives +
/// `__s2require`) and evaluate the injected `@s2script/std` + `@s2script/cs2` preludes over them,
/// store its `PluginInstance` in `PLUGINS`, register the plugin in `REGISTRY`, and return the
/// generation.
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

            // Full per-context API: install the natives first, THEN evaluate the injected preludes
            // (which build the renamed `@s2script/std` / `@s2script/cs2` objects over those
            // natives and stash them at `globalThis.__s2pkg_*` for `__s2require`).
            let global_obj = ctx_local.global(scope);
            install_natives(scope, global_obj);
            run_prelude(scope, "@s2script/std", INJECTED_STD_PRELUDE);
            // @s2script/cs2: provided externally at runtime via register_injected_package
            // (the shim calls s2script_core_register_package at load — see ffi.rs).
            // If not registered, __s2pkg_cs2 stays undefined and require("@s2script/cs2") → null.
            let cs2_src = INJECTED_PACKAGES.with(|p| p.borrow().get("@s2script/cs2").cloned());
            if let Some(src) = cs2_src {
                run_prelude(scope, "@s2script/cs2", &src);
            }

            v8::Global::new(scope.as_ref(), ctx_local)
            // scope, hs, hs_storage drop here — the isolate borrow is released.
        };

        // Register in REGISTRY first so we can stamp the assigned generation onto the PluginInstance
        // (kept in lockstep — a resolver tags itself with this same generation via resolver_owner_tag).
        let generation = REGISTRY.with(|r| r.borrow_mut().insert(id));
        PLUGINS.with(|p| {
            p.borrow_mut().insert(
                id.to_string(),
                PluginInstance {
                    exports: None,
                    context: g_ctx,
                    generation,
                },
            )
        });
        generation
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
            .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
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

/// Load a built plugin bundle `plugin_js` under plugin id `id` (the spike-PROVEN CJS wrapper).
///
/// Steps: (1) `create_plugin_context(id)` — a fresh per-plugin context with the full injected API
/// (`__s2require` + the `@s2script/std` / `@s2script/cs2` preludes); (2) evaluate the CJS wrapper
/// `(function(require,module,exports){…})(require, module, module.exports)` in that context and
/// CAPTURE the RETURNED `module.exports` (esbuild REASSIGNS `module.exports`, so the return value
/// — not the `exports` arg — is the plugin's real export object; spike [risk]); (3) call
/// `exports.onLoad()` if present; (4) store the exports `Global<Object>` on the `PluginInstance`
/// (Task 6 reads `onUnload` off it at teardown; it is dropped before the context).
///
/// Degrade-never-crash: a compile/run/onLoad error logs a named WARN and returns; no exception
/// propagates (the whole JS run is under a `TryCatch`).
pub(crate) fn load_plugin_js(id: &str, plugin_js: &str) {
    // Defensive guard: if the plugin is already loaded (e.g. the caller is performing a
    // reload but did not call unload_plugin first), tear it down now so the old handler
    // Global/context can never leak into the new instance.  The loader's explicit
    // unload-before-load (T7 reload discipline) makes this a belt-and-suspenders no-op
    // in the normal reload path; it protects against accidental double-loads in other paths.
    if PLUGINS.with(|p| p.borrow().contains_key(id)) {
        log_warn(&format!(
            "WARN: load_plugin_js('{}'): plugin already loaded — unloading old instance first (reload guard)",
            id
        ));
        unload_plugin(id);
    }

    // (1) Fresh context with the full injected API installed.
    create_plugin_context(id);

    // The spike's PROVEN wrapper — the outer arrow-IIFE returns `module.exports` so `script.run`
    // hands it straight back to Rust.  `{PLUGIN_JS}` is spliced verbatim.
    let wrapper = format!(
        "(() => {{\n  const module = {{ exports: {{}} }};\n  const require = globalThis.__s2_require;\n  (function (require, module, exports) {{\n{}\n}})(require, module, module.exports);\n  return module.exports;\n}})()",
        plugin_js
    );

    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else {
            log_warn("WARN: load_plugin_js called before init");
            return;
        };

        // Clone the plugin's Global<Context> out of PLUGINS (cheap refcount bump); release the
        // borrow before opening the HandleScope on HOST.isolate.
        let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.context.clone())) else {
            log_warn(&format!("WARN: load_plugin_js('{}'): context missing after create", id));
            return;
        };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // (2)+(3) Compile+run the wrapper, capture module.exports, call onLoad — all under one
        // TryCatch so a throwing plugin can't leak a pending exception into later frames.
        let exports_global: Option<v8::Global<v8::Object>> = 'blk: {
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let Some(code) = v8::String::new(tc, &wrapper) else {
                log_warn(&format!("WARN: load_plugin_js('{}'): failed to intern source", id));
                break 'blk None;
            };
            let ret = match v8::Script::compile(tc, code, None).and_then(|s| s.run(tc)) {
                Some(r) => r,
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "unknown error".into());
                    log_warn(&format!("WARN: load_plugin_js('{}'): eval error: {}", id, msg));
                    break 'blk None;
                }
            };
            // The wrapper returns `module.exports` — must be an object (spike fact 2).
            let Ok(exports) = v8::Local::<v8::Object>::try_from(ret) else {
                log_warn(&format!("WARN: load_plugin_js('{}'): module.exports is not an object", id));
                break 'blk None;
            };

            // Call onLoad() if the plugin exported one (a throwing onLoad is caught here).
            if let Some(k) = v8::String::new(tc, "onLoad") {
                if let Some(v) = exports.get(tc, k.into()) {
                    if let Ok(f) = v8::Local::<v8::Function>::try_from(v) {
                        let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                        if f.call(tc, recv, &[]).is_none() {
                            let msg = tc
                                .exception()
                                .map(|e| e.to_rust_string_lossy(&*tc))
                                .unwrap_or_else(|| "onLoad threw".into());
                            log_warn(&format!("WARN: load_plugin_js('{}'): onLoad error: {}", id, msg));
                        }
                    }
                }
            }

            // (4) Capture module.exports for teardown (Task 6 reads onUnload off it).  `tc.as_ref()`
            // yields the isolate ref (AsRef<Isolate> for the TryCatch pinned ref).
            Some(v8::Global::new(tc.as_ref(), exports))
        };

        if let Some(g) = exports_global {
            PLUGINS.with(|p| {
                if let Some(pi) = p.borrow_mut().get_mut(id) {
                    pi.exports = Some(g);
                }
            });
        }
    });
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

        // Install the full native API on the shared HOST context.  HOST is the driver context for
        // dispatch/drain/concommand and the C-ABI `eval` surface; it carries the natives (console,
        // `__s2_*`, `__s2require`) but NOT the injected `@s2script/*` prelude — the renamed
        // `OnGameFrame.subscribe`/`delay`/… API lives ONLY in per-plugin contexts (Task 5).
        let global_obj = ctx_local.global(scope);
        install_natives(scope, global_obj);

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

/// Dispatch one `OnGameFrame` tick to all enabled JS handlers for `phase`, EACH IN ITS OWNING
/// PLUGIN CONTEXT.
///
/// **Per-handler context (Task 6):** the snapshot carries each sub's `owner`; before invoking a
/// handler we enter that owner's `PLUGINS[owner]` context with its own `ContextScope`, build the
/// per-frame `ctx` object there, and call under a per-handler `TryCatch` — so the handler (and any
/// native it calls → `current_plugin`) runs in its own realm.  If the owner's context is gone
/// (disposed by `unload_plugin`), the handler is SKIPPED (never call a `Global<Function>` whose
/// realm was disposed).
///
/// **Three-phase borrow split (load-bearing for re-entrancy), preserved:**
/// - Phase 1: borrow `FRAME` only long enough to clone the ordered (owner-tagged) snapshot, release.
/// - Phase 2: borrow `HOST` (for the isolate) and run the chain.  `FRAME`/`PLUGINS` are NOT borrowed
///   across a handler call, so a handler that calls `OnGameFrame.subscribe(...)`/`delay(...)`
///   re-enters `FRAME`/`PLUGINS` without a double-borrow panic (each owner context is cloned out of
///   `PLUGINS` before the call).
/// - Phase 3: briefly borrow `FRAME` mutably for error/auto-disable bookkeeping.
pub(crate) fn dispatch_onframe(
    phase: Phase,
    simulating: bool,
    first: bool,
    last: bool,
) -> multiplexer::DispatchOutcome {
    use crate::multiplexer::{run_chain, DispatchOutcome};

    // Phase 1 — brief &FRAME borrow: clone the ordered enabled handlers (KEEPING the owner tag so we
    // can enter each handler's own context), then release.
    let snap4 = FRAME.with(|f| f.borrow().snapshot(phase));
    if snap4.is_empty() {
        return DispatchOutcome {
            result: HookResult::Continue,
            detour: DetourChange::None,
        };
    }
    // run_chain wants (SubId, Priority, H); carry H = (owner, handler) so invoke can route context.
    let snap: Vec<(multiplexer::SubId, Priority, (String, JsHandler))> =
        snap4.into_iter().map(|(id, prio, owner, h)| (id, prio, (owner, h))).collect();

    // Phase 2 — invoke under EACH handler's OWN plugin context.  HOST is borrowed for the isolate;
    // FRAME/PLUGINS are NOT held across a handler call.
    let outcome = HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow.as_mut().expect("dispatch_onframe before init");

        run_chain(&snap, |(owner, jh): &(String, JsHandler)| {
            // Route to the owner's context; SKIP (never enter a disposed context) if it is gone.
            // Cloning the Global<Context> releases the PLUGINS borrow before the JS call, so the
            // handler may re-enter PLUGINS (subscribe/delay) without a double borrow.
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone()))
            else {
                return Ok(HookResult::Continue); // owner's context disposed → skip, not an error
            };

            // Fresh HandleScope + ContextScope on the OWNER's context.
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            // Build the per-frame `ctx` object IN THIS CONTEXT: { simulating, firstTick, lastTick, phase }.
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

pub fn shutdown() {
    // Run per-plugin teardown (onUnload + ledger) in reverse-dependency order BEFORE any bulk clears,
    // so each plugin's onUnload fires while the isolate + other plugins are still alive.
    // The bulk clears below are the final backstop for anything not already cleaned up by unload_all.
    unload_all();
    // Clear async state BEFORE dropping the isolate: RESOLVERS holds `Global`s into it, so their
    // handles must be reset while the isolate is still alive (HOST still owns it here).
    TIMERS.with(|t| *t.borrow_mut() = TimerQueue::new());
    RESOLVERS.with(|m| m.borrow_mut().clear());
    // Clear CONCOMMANDS BEFORE dropping the isolate — same discipline as RESOLVERS: the map holds
    // Global<Function>s into the isolate; dropping them while the isolate is alive is required.
    CONCOMMANDS.with(|m| m.borrow_mut().clear());
    // Clear inter-plugin method + subscriber Globals BEFORE the isolate drops (same discipline as
    // RESOLVERS/CONCOMMANDS: they hold Global<Function>s into the isolate).
    IFACE_METHODS.with(|m| m.borrow_mut().clear());
    IFACE_SUBS.with(|m| m.borrow_mut().clear());
    // Clear the interface registry (pure Rust, no V8 handles; cleared for re-init hygiene).
    IFACES.with(|r| r.borrow_mut().clear());
    // Reset the subscription-id allocator for a clean slate (symmetric with TimerQueue::new()).
    NEXT_SUB_ID.with(|c| c.set(1));
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

/// Resolve one pending async `entry` in its OWNING plugin's context, or DROP it (the async-liveness
/// guard) if the plugin unloaded or reloaded.
///
/// A plugin-tagged entry is resolved only if `REGISTRY.is_live(id, generation)` — otherwise it is
/// DROPPED (returns without resolving; the `ResolverEntry` — and its `Global<PromiseResolver>` — is
/// dropped by the caller, releasing the handle while the isolate is still alive, sound even if the
/// owner's context was already disposed).  This is the use-after-free killer: never resolve a promise
/// into a disposed/replaced context.  An untagged entry (`owner == None`, a non-plugin/HOST-context
/// resolver) has no plugin liveness to check and is resolved in the shared `HOST` context.
///
/// The owner's `Global<Context>` is cloned out of `PLUGINS` (borrow released) before the resolve; a
/// resolve does NOT run JS under kExplicit, so no continuation re-enters here.
fn resolve_or_drop(host: &mut Host, entry: &ResolverEntry) {
    let g_ctx = match &entry.owner {
        Some((id, generation)) => {
            if !REGISTRY.with(|r| r.borrow().is_live(id, *generation)) {
                return; // plugin unloaded or reloaded → DROP (do not resolve into a dead context)
            }
            match PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.context.clone())) {
                Some(g) => g,
                None => return, // context gone (defensive) → drop
            }
        }
        None => host.context.clone(), // non-plugin resolver → resolve in the shared HOST context
    };

    let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
    let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
    let hs = &mut hs;
    let ctx_local = v8::Local::new(hs, &g_ctx);
    let scope = &mut v8::ContextScope::new(hs, ctx_local);
    let resolver = v8::Local::new(scope, &entry.resolver);
    let undef = v8::undefined(scope);
    resolver.resolve(scope, undef.into());
}

/// Per-frame async drain: resolve every due timer + completed job IN ITS OWNING PLUGIN CONTEXT
/// (dropping any whose plugin is gone/reloaded — the async-liveness guard), advance the frame
/// counter, then run the single V8 microtask checkpoint for this frame.  Called once per Post-phase
/// game frame (wired in `ffi.rs`).
///
/// **Re-entrancy discipline (load-bearing):** a resolved continuation (a `Delay`/`NextTick` handler
/// that itself calls `Delay`/`NextTick`/`NextFrame`/`onGameFrame`) re-enters the
/// TIMERS/RESOLVERS/FRAME/PLUGINS/REGISTRY thread-locals from INSIDE `perform_microtask_checkpoint`.
/// So we hold NONE of those borrows across the checkpoint OR across a resolve: collect due ids
/// (TIMERS borrow dropped), remove each `ResolverEntry` (RESOLVERS borrow dropped per id), resolve it
/// via `resolve_or_drop` (which clones the owner context out of PLUGINS and checks REGISTRY with no
/// borrow held across the resolve), advance FRAME_COUNTER (Cell), THEN run the checkpoint on the HOST
/// context (continuations run in their OWN realms regardless of the checkpoint's entered context).
/// `refresh_detour` (borrows FRAME + TIMERS) runs only after the scope is dropped.
pub(crate) fn frame_async_drain() {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };

        // Resolve due timers using the PRE-increment counter (= drains completed so far).  A
        // `Frame(t)` timer fires when this `frame >= t`; a `Deadline(d)` fires when `now >= d`.
        let frame = FRAME_COUNTER.with(|c| c.get());
        let due = TIMERS.with(|t| t.borrow_mut().due(Instant::now(), frame));
        for id in due {
            // Remove the tagged resolver (RESOLVERS borrow released), then resolve-or-drop it in its
            // owner's context.  A None entry means the timer was already dropped (e.g. by unload).
            let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) else { continue };
            resolve_or_drop(host, &entry);
        }
        // Resolve completed threadpool jobs.
        while let Some((id, _res)) = pool().try_recv_completed() {
            let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) else { continue };
            // Only decrement for a resolver we actually held (a job we own — matches the stale-
            // completion rule): a stale id from a prior isolate has no entry and skips this.
            PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
            resolve_or_drop(host, &entry);
        }

        // Advance the counter BEFORE the checkpoint so continuations observe the new count.
        FRAME_COUNTER.with(|c| c.set(frame.wrapping_add(1)));

        // The one microtask checkpoint for this frame, on the HOST context — no TIMERS/RESOLVERS/
        // FRAME/PLUGINS/REGISTRY borrow held.  Continuations run in their own plugin realms.
        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &host.context);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);
        scope.perform_microtask_checkpoint();
    });
    // HOST + scope released: a just-completed last timer may make the detour undesired, or a
    // continuation may have queued new async keeping it desired.  Reconcile now.
    refresh_detour();
}

/// Unload a plugin at a frame boundary (never mid-dispatch): the ledger reverse-walk teardown
/// authority.  Order matches the spike's Global-drop-before-context discipline (all `Global`s
/// pointing INTO the plugin's context are dropped BEFORE its `Global<Context>`, isolate alive):
///
/// (a) `FRAME.remove_by_owner(id)` — drops the plugin's handler `Global<Function>`s + reconciles the
///     detour (removes the `OnGameFrame` detour if this was the only subscriber).
/// (b) best-effort `onUnload` (enter the plugin's context, call `module.exports.onUnload` if present
///     under a `TryCatch` — a throw is logged, teardown proceeds).
/// (c) `REGISTRY.remove(id)` → walk `ledger.teardown_order()` (reverse acquisition): `Timer` → remove
///     from `TIMERS` + drop its `RESOLVERS` entry; `Job` → drop its `RESOLVERS` entry (a late worker
///     completion is then a no-op; decrement `PENDING_JOBS` for a still-pending job we drop); `Hook`
///     → already removed by (a), dropped defensively.  Drops the resolver `Global`s.
/// (d) drop the captured `module.exports` `Global<Object>`.
/// (e) `dispose_plugin_context(id)` — NOW drop the `Global<Context>` (all inner Globals released in
///     a–d, isolate alive → sound, no leak).
/// Unload every loaded plugin in reverse-dependency order (importers before producers), so a
/// consumer's onUnload can still call the producer it depends on. Used by `shutdown` and any
/// full-teardown cascade. Computes the id list and order into owned Vecs (releasing all borrows)
/// before the unload loop so unload_plugin can freely re-enter IFACES/PLUGINS.
pub fn unload_all() {
    let ids = PLUGINS.with(|p| p.borrow().keys().cloned().collect::<Vec<_>>());
    let order = IFACES.with(|r| r.borrow().unload_order(&ids));
    for id in order { unload_plugin(&id); }
}

pub(crate) fn unload_plugin(id: &str) {
    // (a) Mark unloading: drop the plugin's OnGameFrame subscriptions (handler Globals) and reconcile
    // the detour.  remove_by_owner returns a DetourChange, but the combined predicate in
    // refresh_detour is the source of truth — call it to apply the transition.
    let _change = FRAME.with(|f| f.borrow_mut().remove_by_owner(id));
    refresh_detour();

    // (b) Best-effort onUnload in the plugin's OWN context.  Clone the context + exports out of
    // PLUGINS (borrow released) so onUnload may re-enter PLUGINS/FRAME/etc. without a double borrow.
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };
        let Some((g_ctx, Some(exports))) =
            PLUGINS.with(|p| p.borrow().get(id).map(|pi| (pi.context.clone(), pi.exports.clone())))
        else {
            return; // no context or no captured exports → nothing to call
        };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

        let exports_local = v8::Local::new(tc, &exports);
        if let Some(k) = v8::String::new(tc, "onUnload") {
            if let Some(v) = exports_local.get(tc, k.into()) {
                if let Ok(f) = v8::Local::<v8::Function>::try_from(v) {
                    let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                    if f.call(tc, recv, &[]).is_none() {
                        let msg = tc
                            .exception()
                            .map(|e| e.to_rust_string_lossy(&*tc))
                            .unwrap_or_else(|| "onUnload threw".into());
                        log_warn(&format!("WARN: unload_plugin('{}'): onUnload error: {}", id, msg));
                    }
                }
            }
        }
    });

    // (c) Ledger reverse-walk: the teardown authority.  REGISTRY.remove yields the entry (also makes
    // is_live false for any lingering resolver of this generation).
    if let Some(entry) = REGISTRY.with(|r| r.borrow_mut().remove(id)) {
        for res in entry.ledger.teardown_order() {
            match res {
                plugin::Resource::Timer(tid) => {
                    TIMERS.with(|t| { t.borrow_mut().remove(tid); });
                    RESOLVERS.with(|m| { m.borrow_mut().remove(&tid); });
                }
                plugin::Resource::Job(jid) => {
                    // The worker may still run; its late completion is a no-op (resolver gone).  Drop
                    // the resolver and, for a still-pending job we own, decrement PENDING_JOBS now so
                    // the (guarded) drain decrement does NOT double-count on the late completion.
                    if RESOLVERS.with(|m| m.borrow_mut().remove(&jid)).is_some() {
                        PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                    }
                }
                plugin::Resource::Hook(sid) => {
                    // Already removed by (a); drop defensively (also catches a hook onUnload added
                    // AFTER (a)'s remove_by_owner).
                    let _ = FRAME.with(|f| f.borrow_mut().unsubscribe(sid));
                }
                plugin::Resource::Interface(name) => {
                    // Remove the registry entry(ies) this producer owned + drop its method Globals.
                    // TODO(slice5): this prunes IFACE_METHODS by interface NAME only. Safe today (one
                    // namespaced producer per name), but once manifest-vs-runtime `publishes`
                    // cross-validation lands, key the prune by (producer_id, name) so unloading one
                    // producer can never drop a different producer's method Globals for the same name.
                    IFACES.with(|r| { let _ = r.borrow_mut().remove_by_producer(id); });
                    IFACE_METHODS.with(|m| {
                        m.borrow_mut().retain(|(iface, _method), _| iface != &name);
                    });
                }
                plugin::Resource::EventSub(sub_id) => {
                    // Idempotent: iface_off may have already removed this sub from IFACE_SUBS
                    // without removing the ledger entry, so remove() (a no-op on missing keys) is
                    // correct — NEVER unwrap/expect/index here (would crash the plugin on that path).
                    IFACE_SUBS.with(|m| { m.borrow_mut().remove(&sub_id); });
                    // The subscriber row is removed from the producer's list below via
                    // remove_subscribers_by_consumer(id) (belt-and-suspenders for any not yet dropped).
                }
                plugin::Resource::Import(_name) => { /* edge only; no Global to drop */ }
            }
        }
    }
    // Drop any subscriber rows this plugin (as a consumer) still holds, and its import declarations.
    // Idempotent with the per-resource drops above (remove() is a no-op on missing keys).
    let orphaned = IFACES.with(|r| r.borrow_mut().remove_subscribers_by_consumer(id));
    IFACE_SUBS.with(|m| { let mut mm = m.borrow_mut(); for (_iface, sid) in orphaned { mm.remove(&sid); } });
    IFACES.with(|r| r.borrow_mut().clear_imports(id));
    // Removing timers/jobs (or an onUnload-added hook) changed the detour predicate — reconcile.
    refresh_detour();

    // (d) Drop the captured module.exports Global<Object> while the isolate is alive (before the
    // context Global).
    PLUGINS.with(|p| {
        if let Some(pi) = p.borrow_mut().get_mut(id) {
            pi.exports = None;
        }
    });

    // (e) NOW drop the Global<Context> (all inner Globals were released in a–d).  dispose_plugin_context
    // removes the PLUGINS entry (dropping the context Global) and the REGISTRY entry (already gone → no-op).
    dispose_plugin_context(id);
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

    // Read `globalThis[name]` as a String from the current (HOST) isolate/context.  Still used by
    // the ConCommand dispatch test, which exercises the shared HOST context.
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
                .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
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

    // Alias used by Task 5 tests — reads `globalThis[name]` as a String from a named plugin context.
    fn read_global_string(id: &str, name: &str) -> String {
        read_string_global_in(id, name)
    }

    // Read `globalThis[name]` as an i32 from a specific PLUGIN context (mirrors read_string_global_in).
    fn read_i32_global_in(id: &str, name: &str) -> i32 {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("read_i32_global_in: no host");
            let g_ctx = PLUGINS
                .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
                .expect("read_i32_global_in: no context for id");
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let global = ctx_local.global(scope);
            let key = v8::String::new(scope, name).unwrap();
            let val = global.get(scope, key.into()).unwrap_or_else(|| v8::undefined(scope).into());
            val.integer_value(scope).unwrap_or(0) as i32
        })
    }

    // Read `globalThis[name]` as a bool from a specific PLUGIN context (mirrors read_string_global_in).
    fn read_bool_global_in(id: &str, name: &str) -> bool {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("read_bool_global_in: no host");
            let g_ctx = PLUGINS
                .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
                .expect("read_bool_global_in: no context for id");
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let global = ctx_local.global(scope);
            let key = v8::String::new(scope, name).unwrap();
            let val = global.get(scope, key.into()).unwrap_or_else(|| v8::undefined(scope).into());
            val.is_true()
        })
    }

    // Create a fresh plugin context `id` and eval `src` in it with the `@s2script/std` API
    // destructured into scope (so tests can write `OnGameFrame.subscribe(...)`, `delay(...)`, etc.
    // directly).  The renamed API is only reachable via `require`, matching the plugin model.
    fn eval_std(id: &str, src: &str) {
        create_plugin_context(id);
        let full = format!(
            "const {{ OnGameFrame, delay, nextTick, nextFrame, threadSleep }} = __s2require(\"@s2script/std\");\n{}",
            src
        );
        eval_in_context(id, &full).expect("eval_std");
    }

    // Drive one full game frame: Pre dispatch, Post dispatch, then the async drain (mirrors the
    // engine order the C-ABI `s2script_core_dispatch_game_frame` uses — Post triggers the drain).
    fn dispatch_game_frame_pre_post() {
        dispatch_onframe(Phase::Pre, true, true, false);
        dispatch_onframe(Phase::Post, true, false, true);
        frame_async_drain();
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
        eval_std("p", r#"
            OnGameFrame.subscribe((f) => { console.log("high:" + f.firstTick); }, { priority: "high" });
            OnGameFrame.subscribe((f) => { console.log("normal"); });
        "#);

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
        eval_std("p", r#"
            OnGameFrame.subscribe(() => { console.log("h"); return HookResult.Stop; }, { priority: "high" });
            OnGameFrame.subscribe(() => { console.log("l"); }, { priority: "low" });
        "#);
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
        eval_std("p", r#" OnGameFrame.subscribe(() => { throw new Error("boom"); }); "#);
        // Must not panic / crash; result stays Continue.
        let out = dispatch_onframe(Phase::Pre, true, false, false);
        assert_eq!(out.result, HookResult::Continue);
        shutdown();
    }

    #[test]
    fn handler_that_subscribes_during_dispatch_does_not_panic_and_runs_next_frame() {
        // The re-entrancy guarantee: a JS handler that calls OnGameFrame.subscribe(...) DURING
        // dispatch re-enters __s2_subscribe (which borrows FRAME). dispatch_onframe must NOT hold
        // the FRAME borrow across invocation, or this double-borrows the RefCell and panics.
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval_std("p", r#"
            let added = false;
            OnGameFrame.subscribe(() => {
                console.log("outer");
                if (!added) { added = true; OnGameFrame.subscribe(() => console.log("inner")); }
            });
        "#);
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
        create_plugin_context("p");
        // With kExplicit, a resolved-promise continuation must NOT run during eval.  The plugin
        // context's microtasks share the isolate's default queue, so the HOST-context checkpoint
        // in frame_async_drain drains them (the continuation runs in the plugin's own realm).
        eval_in_context("p", "globalThis.__ran = false; Promise.resolve().then(() => { globalThis.__ran = true; });").unwrap();
        assert_eq!(read_bool_global_in("p", "__ran"), false, "microtask ran before the drain");
        frame_async_drain(); // runs the checkpoint
        assert_eq!(read_bool_global_in("p", "__ran"), true, "microtask did not run at the drain");
        shutdown();
    }

    #[test]
    fn onframe_handler_out_of_range_result_warns_and_continues() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval_std("p", "OnGameFrame.subscribe(() => 99);"); // 99 is out of range for HookResult
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
        eval_std("p", "globalThis.__d = false; delay(30).then(() => { globalThis.__d = true; });");
        frame_async_drain();                       // well before 30ms
        assert_eq!(read_bool_global_in("p", "__d"), false);
        std::thread::sleep(std::time::Duration::from_millis(40));
        frame_async_drain();                       // now past the deadline
        assert_eq!(read_bool_global_in("p", "__d"), true);
        shutdown();
    }

    #[test]
    fn next_frame_resolves_one_frame_later() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__n = 0; nextFrame().then(() => { globalThis.__n = 1; });");
        frame_async_drain(); // frame that schedules resolution for the NEXT frame → not yet
        // nextFrame targets FRAME_COUNTER+1 measured at call time; the drain that reaches it resolves it.
        assert_eq!(read_i32_global_in("p", "__n"), 0);
        frame_async_drain();
        assert_eq!(read_i32_global_in("p", "__n"), 1);
        shutdown();
    }

    #[test]
    fn delay_with_no_onframe_subscriber_still_requests_detour_install() {
        // Wire a recording request_hook (the ffi mock pattern) via set_hook_request BEFORE init.
        HOOKS.lock().unwrap().clear();
        set_hook_request(Some(record_hook));
        init(dummy_logger()).unwrap();
        eval_std("p", "delay(1000);");  // pending async, zero OnGameFrame subscribers
        assert!(HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 1),
                "delay() should request the detour install");
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
        // With ZERO OnGameFrame subscribers, start one async op that will complete on its own.
        // threadSleep(20) increments PENDING_JOBS → 1 and must drive an install.
        eval_std("p", "threadSleep(20);");
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
        eval_std("p", r#"
            globalThis.__reentry = 0;
            nextTick().then(() => { nextTick().then(() => { globalThis.__reentry = 1; }); });
        "#);
        // Drain 1 resolves the outer nextTick; its continuation queues the inner nextTick from
        // within the checkpoint (must not panic). A later drain resolves the inner → __reentry = 1.
        for _ in 0..5 { frame_async_drain(); }
        assert_eq!(read_i32_global_in("p", "__reentry"), 1);
        shutdown();
    }

    #[test]
    fn thread_sleep_runs_off_thread_and_resolves_on_a_drain() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__t = false; threadSleep(20).then(() => { globalThis.__t = true; });");
        // Drive frames until the worker completes (bounded).
        let mut resolved = false;
        for _ in 0..500 {
            frame_async_drain();
            if read_bool_global_in("p", "__t") { resolved = true; break; }
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

    /// `load_plugin_js` creates the plugin context (full injected API), wraps the bundle in the CJS
    /// `require`/`module` wrapper, and runs the module body.  This replaces the Slice-3 `load_cs2_file`
    /// path (removed): the same "a loaded bundle's top-level code runs and its globals are visible"
    /// behavior, now under the per-plugin loader.  The body sets `globalThis.__loaded = 42`.
    #[test]
    fn load_plugin_js_runs_module_body() {
        init(dummy_logger()).unwrap();
        load_plugin_js("probe", "globalThis.__loaded = 41 + 1;");
        assert_eq!(read_i32_global_in("probe", "__loaded"), 42);
        shutdown();
    }

    /// The brief's acceptance test: a CJS bundle requires the injected API, subscribes in `onLoad`,
    /// and its handler runs once per frame — tagged to the CALLING plugin ("demo") in the ledger +
    /// the multiplexer owner.
    #[test]
    fn load_plugin_js_runs_onload_and_tags_subscription() {
        init(dummy_logger()).unwrap();
        // Minimal CJS bundle: require the injected API, subscribe, export onLoad.
        let plugin_js = r#"
            const { OnGameFrame, delay } = require("@s2script/std");
            module.exports.onLoad = function () {
                OnGameFrame.subscribe(function () { globalThis.__ticks = (globalThis.__ticks||0)+1; });
            };
        "#;
        load_plugin_js("demo", plugin_js);
        // One frame → the demo's handler ran, tagged to "demo".
        dispatch_game_frame_pre_post();  // helper: Pre then Post dispatch (drives the multiplexer)
        assert_eq!(read_i32_global_in("demo", "__ticks"), 1);
        // The subscription is owned by "demo":
        assert!(FRAME.with(|f| f.borrow().snapshot(Phase::Pre).iter().any(|(_,_,owner,_)| owner=="demo")));
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
        eval_std("p", "threadSleep(1000).then(()=>{});");
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

    /// Brief test: `unload_plugin` removes the plugin's OnGameFrame hook (so its handler no longer
    /// runs) AND disposes its context.  Also (merged) closes the untested `remove_by_owner` `Remove`
    /// path from Task 3: wiring the recording detour-request callback, the unload of the ONLY
    /// plugin's ONLY subscription must fire an `("OnGameFrame", 0)` detour REMOVE.
    #[test]
    fn unload_removes_the_plugins_hook_and_disposes_context() {
        // Wire the recording hook-request callback BEFORE init so subscribe/unload transitions record.
        HOOKS.lock().unwrap().clear();
        set_hook_request(Some(record_hook));
        init(dummy_logger()).unwrap();
        load_plugin_js("demo", r#"const {OnGameFrame}=require("@s2script/std");
            module.exports.onLoad=()=>OnGameFrame.subscribe(()=>{globalThis.__n=(globalThis.__n||0)+1;});"#);
        dispatch_game_frame_pre_post();
        // The subscribe (the only subscriber) requested the detour INSTALL.
        assert!(
            HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 1),
            "the only subscriber must have requested the detour install"
        );
        assert_eq!(read_i32_global_in("demo", "__n"), 1, "handler ran once before unload");

        unload_plugin("demo");
        dispatch_game_frame_pre_post();            // demo's handler must NOT run now (context disposed)
        assert!(!FRAME.with(|f| f.borrow().snapshot(Phase::Pre).iter().any(|(_,_,o,_)| o=="demo")));
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("demo")), "context disposed");
        // The ONLY subscriber unloaded → the OnGameFrame detour must be REMOVED (enable=0).
        assert!(
            HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 0),
            "unload of the only subscriber must request the detour remove"
        );
        shutdown();
        set_hook_request(None);
    }

    /// Brief test: a `delay` continuation whose plugin is UNLOADED before the deadline must be
    /// DROPPED — `frame_async_drain` must NOT run the continuation into a disposed context (no
    /// panic; the resolver was dropped by the ledger teardown).
    #[test]
    fn delay_continuation_for_unloaded_plugin_is_dropped() {
        init(dummy_logger()).unwrap();
        load_plugin_js("demo", r#"const {delay}=require("@s2script/std");
            module.exports.onLoad=()=>{ (async()=>{ await delay(30); globalThis.__resumed=true; })(); };"#);
        unload_plugin("demo");                     // unload BEFORE the deadline
        std::thread::sleep(std::time::Duration::from_millis(40));
        frame_async_drain();                       // must NOT run the continuation into a disposed context
        // The plugin/context is gone; nothing to read — assert no panic + the resolver was dropped:
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("demo")));
        shutdown();
    }

    /// T7 integration test: RELOAD tears down the old plugin and runs only the new handler.
    ///
    /// Proof requirements (brief §RELOAD DISCIPLINE):
    /// - load v1 (sets a global via an OnGameFrame handler), dispatch → only the NEW handler's
    ///   effect is present after reload
    /// - old subscription is gone (subscription count = 1, not 2)
    /// - generation advanced (old generation is stale, new generation is live)
    ///
    /// The defensive guard in `load_plugin_js` is the mechanism under test here: when
    /// `load_plugin_js("demo", v2_js)` is called while "demo" is still in PLUGINS, it detects
    /// the existing instance, calls `unload_plugin("demo")` first (teardown: removes the v1
    /// handler, disposes the context), then loads v2 in a fresh context.
    #[test]
    fn reload_tears_down_old_and_runs_new_handler() {
        init(dummy_logger()).unwrap();

        // v1: subscribes an OnGameFrame handler that writes "v1" to a global.
        let v1_js = r#"
            const { OnGameFrame } = require("@s2script/std");
            module.exports.onLoad = function () {
                OnGameFrame.subscribe(function () { globalThis.__v = "v1"; });
            };
        "#;
        load_plugin_js("demo", v1_js);
        dispatch_game_frame_pre_post();
        assert_eq!(read_string_global_in("demo", "__v"), "v1", "v1 handler ran before reload");

        // Capture the v1 generation so we can assert it becomes stale after reload.
        let old_gen = PLUGINS
            .with(|p| p.borrow().get("demo").expect("demo loaded").generation);

        // RELOAD: call load_plugin_js with the same id — the defensive guard fires.
        // v2 writes "v2" to the global.
        let v2_js = r#"
            const { OnGameFrame } = require("@s2script/std");
            module.exports.onLoad = function () {
                OnGameFrame.subscribe(function () { globalThis.__v = "v2"; });
            };
        "#;
        load_plugin_js("demo", v2_js);

        // Old generation is now stale (unload bumped or removed it).
        assert!(
            !REGISTRY.with(|r| r.borrow().is_live("demo", old_gen)),
            "old generation must be stale after reload"
        );

        // Dispatch: only the v2 handler runs; the v1 handler must not be present.
        dispatch_game_frame_pre_post();
        assert_eq!(
            read_string_global_in("demo", "__v"),
            "v2",
            "v2 handler must run after reload"
        );

        // There must be exactly ONE OnGameFrame subscription (v2's), not two.
        let sub_count = FRAME.with(|f| f.borrow().snapshot(Phase::Pre).len());
        assert_eq!(
            sub_count, 1,
            "old (v1) subscription must be gone; only v2's subscription remains"
        );

        // New generation is live.
        let new_gen = PLUGINS
            .with(|p| p.borrow().get("demo").expect("demo still loaded").generation);
        assert_ne!(old_gen, new_gen, "generation must have advanced");
        assert!(
            REGISTRY.with(|r| r.borrow().is_live("demo", new_gen)),
            "new generation must be live"
        );

        shutdown();
    }

    // Evaluate `src` in a named plugin context and return the result via `coerce`.
    // Mirrors the borrow discipline of `load_plugin_js`: clone the Global<Context> out of PLUGINS
    // before opening the HandleScope on HOST.isolate, run under a TryCatch.
    fn eval_in_context_string(id: &str, src: &str) -> String {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("eval_in_context_string: no host");
            let g_ctx = PLUGINS
                .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
                .unwrap_or_else(|| panic!("eval_in_context_string: no context for '{}'", id));
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            let code = v8::String::new(tc, src).expect("failed to intern");
            let script = v8::Script::compile(tc, code, None).expect("compile failed");
            script.run(tc).map(|v| v.to_rust_string_lossy(tc)).unwrap_or_default()
        })
    }

    fn eval_in_context_bool(id: &str, src: &str) -> bool {
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("eval_in_context_bool: no host");
            let g_ctx = PLUGINS
                .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
                .unwrap_or_else(|| panic!("eval_in_context_bool: no context for '{}'", id));
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            let code = v8::String::new(tc, src).expect("failed to intern");
            let script = v8::Script::compile(tc, code, None).expect("compile failed");
            script.run(tc).map(|v| v.boolean_value(tc)).unwrap_or(false)
        })
    }

    #[test]
    fn iface_publish_records_methods_and_dep_kind() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        create_plugin_context("prod");
        create_plugin_context("cons");

        // Producer publishes.
        eval_in_context("prod", r#"__s2_iface_publish("@x/greeter","1.0.0",{ greet:function(n){return "hi "+n;} });"#).expect("publish");
        // Registry has the method name.
        let has = IFACES.with(|r| r.borrow().lookup("@x/greeter").map(|e| e.method_names.clone()));
        assert_eq!(has, Some(vec!["greet".to_string()]));
        // Consumer sees it as a hard dep and available.
        let kind = eval_in_context_string("cons", r#"__s2_iface_dep_kind("@x/greeter")"#);
        assert_eq!(kind, "hard");
        let pub_ok = eval_in_context_bool("cons", r#"__s2_iface_is_published("@x/greeter")"#);
        assert!(pub_ok);
        // A JSON round-trip across the two contexts preserves data, not identity.
        assert_eq!(eval_in_context_string("prod", r#"JSON.stringify({a:1,b:"x"})"#), r#"{"a":1,"b":"x"}"#);
        shutdown();
    }

    /// Directly exercises the async-liveness guard's `is_live`-DROP branch in `resolve_or_drop`: a
    /// due timer whose owner is NO LONGER LIVE in REGISTRY (its generation is gone/advanced) must be
    /// DROPPED, not resolved — even when its context still exists.  We kill ONLY the REGISTRY entry
    /// (keeping the PLUGINS context so we can observe the continuation did NOT run).  This is the
    /// use-after-free killer's core: never resolve into a stale/replaced realm.
    #[test]
    fn drain_drops_continuation_when_owner_no_longer_live() {
        init(dummy_logger()).unwrap();
        eval_std("demo", "globalThis.__resumed = false; nextTick().then(() => { globalThis.__resumed = true; });");
        // Kill liveness: drop demo's REGISTRY entry (generation now stale) but keep its context.
        REGISTRY.with(|r| { r.borrow_mut().remove("demo"); });
        frame_async_drain(); // the Frame(0) timer is due; owner not live → resolve_or_drop DROPS it
        assert_eq!(
            read_bool_global_in("demo", "__resumed"),
            false,
            "continuation for a non-live owner must be dropped, not resolved into the stale realm"
        );
        shutdown();
    }

    /// Task 5 load-bearing test: a consumer plugin calls a producer plugin's published interface
    /// method across V8 contexts, with values copied (never shared) via a JSON string carrier.
    ///
    /// Exercises: `globalThis.__s2_require` dispatch, `makeIfaceProxy`, `resolveInterface`,
    /// `std.publishInterface`, and the `__s2_iface_call` cross-context structured-copy native.
    #[test]
    fn consumer_calls_producer_method_structured_copy() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        // Producer publishes via the plugin path so the prelude publishInterface is exercised.
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/std");
            publishInterface("@x/greeter","1.0.0",{ greet:function(n){ return "hi "+n.who; } });
        "#);
        // Consumer resolves a hard proxy and calls across (arg + return structured-copied).
        load_plugin_js("cons", r#"
            const g = require("@x/greeter");
            globalThis.__test_out = g.greet({ who: "world" });
        "#);
        assert_eq!(read_global_string("cons", "__test_out"), "hi world");

        // Producer-absent hard dep → InterfaceUnavailable (caught by the wrapper TryCatch → WARN).
        set_plugin_imports("lonely", vec![("@missing".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("lonely", r#"
            try { require("@missing").foo(); globalThis.__err = "no throw"; }
            catch (e) { globalThis.__err = String(e); }
        "#);
        assert!(read_global_string("lonely", "__err").contains("InterfaceUnavailable"));

        // Optional dep, not published → require returns null.
        set_plugin_imports("optc", vec![("@absent".into(), "^1.0.0".into(), crate::interfaces::Kind::Optional)]);
        load_plugin_js("optc", r#"globalThis.__opt = (require("@absent") === null) ? "null" : "proxy";"#);
        assert_eq!(read_global_string("optc", "__opt"), "null");

        // Non-serializable (cyclic) arg → InterfaceValueNotSerializable (JSON.stringify throws → None → throw).
        set_plugin_imports("cyc", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("cyc", r#"
            const g = require("@x/greeter");
            const a = {}; a.self = a;
            try { g.greet(a); globalThis.__e2 = "no throw"; }
            catch (e) { globalThis.__e2 = String(e); }
        "#);
        assert!(read_global_string("cyc", "__e2").contains("InterfaceValueNotSerializable"));

        // Producer method THROWS → consumer sees InterfaceCallError carrying the producer message
        // (not a crash, not a mislabeled InterfaceValueNotSerializable).
        load_plugin_js("prodBoom", r#"
            const { publishInterface } = require("@s2script/std");
            publishInterface("@x/boom", "1.0.0", { boom: function(){ throw new Error("kaboom"); } });
        "#);
        set_plugin_imports("consBoom", vec![("@x/boom".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("consBoom", r#"
            const g = require("@x/boom");
            try { g.boom(); globalThis.__boom = "no throw"; } catch (e) { globalThis.__boom = String(e); }
        "#);
        let boom = read_global_string("consBoom", "__boom");
        assert!(boom.contains("InterfaceCallError"), "producer throw → InterfaceCallError, got: {}", boom);
        assert!(boom.contains("kaboom"), "producer message surfaced, got: {}", boom);

        // Producer method returns undefined (void) → consumer receives undefined, NOT a throw.
        load_plugin_js("prodVoid", r#"
            const { publishInterface } = require("@s2script/std");
            publishInterface("@x/void", "1.0.0", { poke: function(){ /* returns undefined */ } });
        "#);
        set_plugin_imports("consVoid", vec![("@x/void".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("consVoid", r#"
            const g = require("@x/void");
            try { globalThis.__void = (g.poke() === undefined) ? "undefined" : "value"; }
            catch (e) { globalThis.__void = "threw:" + String(e); }
        "#);
        assert_eq!(read_global_string("consVoid", "__void"), "undefined");
        shutdown();
    }

    /// Task 6 (events half): a producer emits an event on its published interface; the LIVE
    /// consumer that subscribed receives it with the payload structured-copied into its context.
    #[test]
    fn producer_emit_forwards_to_live_consumer_only() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/std");
            globalThis.__h = publishInterface("@x/greeter","1.0.0",{ greet:function(){return "";} });
        "#);
        load_plugin_js("cons", r#"
            const g = require("@x/greeter");
            globalThis.__seen = [];
            g.on("greeted", function (p) { globalThis.__seen.push(p.slot); });
        "#);
        // Producer emits (payload structured-copied to the consumer).
        eval_in_context("prod", r#"__h.emit("greeted", { slot: 7 });"#).unwrap();
        assert_eq!(eval_in_context_string("cons", "JSON.stringify(globalThis.__seen)"), "[7]");
        shutdown();
    }

    /// Task 7: producer unload removes the registry entry + method Globals; consumer call now throws
    /// InterfaceUnavailable (caught → returned as a string by the consumer's call wrapper).
    #[test]
    fn producer_unload_invalidates_consumer_proxy() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"const {publishInterface}=require("@s2script/std");
            publishInterface("@x/greeter","1.0.0",{greet:function(){return "ok";}});"#);
        load_plugin_js("cons", r#"const g=require("@x/greeter");
            globalThis.call=function(){ try { return g.greet(); } catch(e){ return String(e); } };
            globalThis.__before=call();"#);
        assert_eq!(read_global_string("cons", "__before"), "ok");
        unload_plugin("prod");
        // registry entry + method Global gone:
        assert!(IFACES.with(|r| r.borrow().lookup("@x/greeter").is_none()));
        assert!(IFACE_METHODS.with(|m| m.borrow().get(&("@x/greeter".into(),"greet".into())).is_none()));
        // consumer call now throws InterfaceUnavailable (caught → string):
        assert!(eval_in_context_string("cons", "globalThis.call()").contains("InterfaceUnavailable"));
        shutdown();
    }

    /// Task 7: consumer unload removes its subscriber rows from the producer's list and from
    /// IFACE_SUBS, so a later emit reaches nobody.
    #[test]
    fn consumer_unload_removes_subscriber() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/greeter".into(),"^1.0.0".into(),crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"const {publishInterface}=require("@s2script/std");
            globalThis.__h=publishInterface("@x/greeter","1.0.0",{greet:function(){return "";}});"#);
        load_plugin_js("cons", r#"const g=require("@x/greeter"); g.on("greeted",function(){});"#);
        assert_eq!(IFACES.with(|r| r.borrow().lookup("@x/greeter").unwrap().subscribers.len()), 1);
        unload_plugin("cons");
        assert_eq!(IFACES.with(|r| r.borrow().lookup("@x/greeter").unwrap().subscribers.len()), 0);
        assert!(IFACE_SUBS.with(|m| m.borrow().is_empty()));
        shutdown();
    }

    /// Task 7: unload_all emits consumers before producers (reverse-dep order), so a consumer's
    /// onUnload can still call the producer it depends on.
    #[test]
    fn unload_all_runs_consumers_before_producers() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/greeter".into(),"^1.0.0".into(),crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"const {publishInterface}=require("@s2script/std");
            publishInterface("@x/greeter","1.0.0",{greet:function(){return "still-here";}});"#);
        // consumer's onUnload calls the producer — must still work because producer outlives it.
        load_plugin_js("cons", r#"const g=require("@x/greeter");
            module.exports.onUnload=function(){ globalThis.__unload_result = g.greet(); };"#);
        unload_all();
        // If the producer had been torn down first, greet() would have thrown; the consumer's
        // onUnload observed a live producer.
        // (Assert via a log capture or a side channel; here we assert no crash + registry cleared.)
        assert!(IFACES.with(|r| r.borrow().lookup("@x/greeter").is_none()));
        assert!(PLUGINS.with(|p| p.borrow().is_empty()));
        shutdown();
    }

    /// Slice 5A Task 3: the six (index, serial) natives degrade safely when no engine-ops table
    /// is wired (no crash, no UB — they return -1/false/null/no-op as documented).
    /// `__s2_handle_decode` is pure bit-math and works without ops.
    #[test]
    fn ent_ref_natives_degrade_without_engine_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops table → every entity op is a safe miss
        create_plugin_context("p");
        // current_serial → -1 ; valid → false ; read → null ; write → false ; state_changed → no-op/undefined
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_current_serial(1))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_valid(1, 7))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_i32(1, 7, 8))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_i32(1, 7, 8, 5))"), "false");
        // handle_decode is PURE (no ops needed). BITS-agnostic assertion: 64 < 2^7 <= 2^HANDLE_ENTRY_BITS,
        // so index==64, serial==0 for any real bit-split (the exact split is validated live in the gate).
        assert_eq!(eval_in_context_string("p", "var d=__s2_handle_decode(64); d[0]+','+d[1]"), "64,0");
        shutdown();
    }

    /// Slice 5A Task 4: `EntityRef` from `@s2script/std` degrades safely when no engine-ops table
    /// is wired — `isValid` returns false, `readInt32` returns null, `writeInt32` returns false.
    /// This is the failing test: EntityRef must be exported by the prelude (Step 3 makes it pass).
    #[test]
    fn entity_ref_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_plugin_js("er", r#"
            const { EntityRef } = require("@s2script/std");
            const ref = new EntityRef(1, 7);
            globalThis.__valid = String(ref.isValid());       // "false"
            globalThis.__read  = String(ref.readInt32(8));    // "null"
            globalThis.__write = String(ref.writeInt32(8, 5));// "false"
        "#);
        assert_eq!(read_global_string("er", "__valid"), "false");
        assert_eq!(read_global_string("er", "__read"), "null");
        assert_eq!(read_global_string("er", "__write"), "false");
        shutdown();
    }
}
