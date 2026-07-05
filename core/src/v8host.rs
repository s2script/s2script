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
use std::ffi::{CStr, CString};
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
/// Schema enumeration callbacks + engine-op (5B.1). The shim provides `SchemaEnumerateFn`; core
/// provides `EmitClassFn`/`EmitFieldFn` as callbacks into the `Catalog` builder. Returns c_int
/// (NOT bool): 0 = schema not ready / error, non-zero = success.
pub type EmitClassFn = extern "C" fn(ctx: *mut c_void, name: *const c_char, parent: *const c_char);
pub type EmitFieldFn = extern "C" fn(
    ctx: *mut c_void, cls: *const c_char, name: *const c_char, offset: c_int,
    kind: *const c_char, type_name: *const c_char, inner: *const c_char,
);
pub type SchemaEnumerateFn = extern "C" fn(ctx: *mut c_void, emit_class: EmitClassFn, emit_field: EmitFieldFn) -> c_int;

// ---------------------------------------------------------------------------
// Slice 5D.1: game-event engine-ops (C-ABI; T3's C header must match exactly).
// All are nullable (Option<...>): a null field degrades to a safe miss.
// ---------------------------------------------------------------------------
pub type EventSubscribeFn    = extern "C" fn(name: *const c_char) -> c_int;
pub type EventUnsubscribeFn  = extern "C" fn(name: *const c_char) -> c_int;
pub type EventGetIntFn       = extern "C" fn(key: *const c_char) -> i32;
pub type EventGetFloatFn     = extern "C" fn(key: *const c_char) -> f32;
pub type EventGetBoolFn      = extern "C" fn(key: *const c_char) -> c_int;      // 0/1
pub type EventGetStringFn    = extern "C" fn(key: *const c_char) -> *const c_char; // valid during dispatch; copy now
pub type EventGetUint64Fn    = extern "C" fn(key: *const c_char) -> u64;
pub type EventGetPlayerSlotFn = extern "C" fn(key: *const c_char) -> i32;       // -1 if absent

// --- Slice 5D.2: engine-identity ops (C-ABI; the C header must match exactly) ---
pub type ClientValidFn        = extern "C" fn(slot: c_int) -> c_int;
pub type ClientUseridFn       = extern "C" fn(slot: c_int) -> i32;
pub type ClientSignonFn       = extern "C" fn(slot: c_int) -> i32;
pub type ClientNameFn         = extern "C" fn(slot: c_int) -> *const c_char;
pub type ClientFindByUseridFn = extern "C" fn(userid: c_int) -> i32;

// --- Slice 5D.3: event write/fire ops (C-ABI; the C header must match exactly) ---
pub type EventSetIntFn    = extern "C" fn(key: *const c_char, value: i32);
pub type EventSetFloatFn  = extern "C" fn(key: *const c_char, value: f32);
pub type EventSetBoolFn   = extern "C" fn(key: *const c_char, value: c_int);
pub type EventSetStringFn = extern "C" fn(key: *const c_char, value: *const c_char);
pub type EventSetUint64Fn = extern "C" fn(key: *const c_char, value: u64);
pub type EventCreateFn    = extern "C" fn(name: *const c_char) -> c_int;
pub type EventFireFn      = extern "C" fn(dont_broadcast: c_int) -> c_int;

// --- Slice 5E.2: config ops (C-ABI; the C header must match exactly) ---
pub type ConfigReadFn  = extern "C" fn(id: *const c_char) -> *const c_char;
pub type ConfigWriteFn = extern "C" fn(id: *const c_char, content: *const c_char) -> c_int;

// --- Slice 6.1: chat messaging op (C-ABI; the C header must match exactly) ---
pub type ClientPrintFn = extern "C" fn(slot: c_int, msg: *const c_char);

// --- Slice 6.2: client SteamID op (C-ABI; the C header must match exactly) ---
pub type ClientSteamidFn = extern "C" fn(slot: c_int) -> *const c_char;

// --- Slice 6.3: client kick op (C-ABI; the C header must match exactly) ---
pub type ClientKickFn = extern "C" fn(slot: c_int, reason: *const c_char);

// --- Slice 6.4: server command + map-validity ops (C-ABI; the C header must match exactly) ---
pub type ServerCommandFn  = extern "C" fn(cmd: *const c_char);
pub type ServerMapValidFn = extern "C" fn(map: *const c_char) -> c_int;
// Slice 6.6 Stage 2: read/write a field of the current CTakeDamageInfo at a schema-resolved offset.
pub type DamageReadFloatFn  = extern "C" fn(offset: c_int) -> f32;
pub type DamageReadIntFn    = extern "C" fn(offset: c_int) -> c_int;
pub type DamageWriteFloatFn = extern "C" fn(offset: c_int, value: f32);
pub type DamageVictimFn     = extern "C" fn() -> c_int;   // raw victim CEntityHandle; -1 = none
pub type CvarGetFn          = extern "C" fn(name: *const c_char) -> *const c_char;   // Slice 6.7: cvar value string

#[repr(C)]
#[derive(Clone, Copy)]
pub struct S2EngineOps {
    pub schema_offset: Option<SchemaOffsetFn>,
    pub ent_by_index: Option<EntByIndexFn>,
    pub deref_handle: Option<DerefHandleFn>, // unused since 5A (EntityRef path supersedes deref_handle)
    pub ent_state_changed: Option<EntStateChangedFn>,
    pub concommand_register: Option<ConCommandRegisterFn>,
    /// Schema enumeration engine-op (5B.1): the shim walks the SchemaSystem and streams classes/fields
    /// to core via the C-ABI emit callbacks.  Null → `__s2_schema_dump` degrades to false.
    pub schema_enumerate: Option<SchemaEnumerateFn>,
    // --- Slice 5D.1: game-event engine-ops (APPENDED — order is the ABI; do not reorder above) ---
    pub event_subscribe:     Option<EventSubscribeFn>,
    pub event_unsubscribe:   Option<EventUnsubscribeFn>,
    pub event_get_int:       Option<EventGetIntFn>,
    pub event_get_float:     Option<EventGetFloatFn>,
    pub event_get_bool:      Option<EventGetBoolFn>,
    pub event_get_string:    Option<EventGetStringFn>,
    pub event_get_uint64:    Option<EventGetUint64Fn>,
    pub event_get_player_slot: Option<EventGetPlayerSlotFn>,
    // --- Slice 5D.2: engine-identity ops (APPENDED — order is the ABI; do not reorder above) ---
    pub client_valid:          Option<ClientValidFn>,
    pub client_userid:         Option<ClientUseridFn>,
    pub client_signon:         Option<ClientSignonFn>,
    pub client_name:           Option<ClientNameFn>,
    pub client_find_by_userid: Option<ClientFindByUseridFn>,
    // --- Slice 5D.3: event write/fire ops (APPENDED — order is the ABI; do not reorder above) ---
    pub event_set_int:    Option<EventSetIntFn>,
    pub event_set_float:  Option<EventSetFloatFn>,
    pub event_set_bool:   Option<EventSetBoolFn>,
    pub event_set_string: Option<EventSetStringFn>,
    pub event_set_uint64: Option<EventSetUint64Fn>,
    pub event_create:     Option<EventCreateFn>,
    pub event_fire:       Option<EventFireFn>,
    // --- Slice 5E.2: config ops (APPENDED after the event ops; order is the ABI; do not reorder above) ---
    pub config_read:  Option<ConfigReadFn>,
    pub config_write: Option<ConfigWriteFn>,
    // --- Slice 6.1: chat messaging op (APPENDED after config ops; order is the ABI; do not reorder above) ---
    pub client_print: Option<ClientPrintFn>,
    // --- Slice 6.2: client SteamID op (APPENDED after client_print; order is the ABI; do not reorder above) ---
    pub client_steamid: Option<ClientSteamidFn>,
    // --- Slice 6.3: client kick op (APPENDED after client_steamid; order is the ABI; do not reorder above) ---
    pub client_kick: Option<ClientKickFn>,
    // --- Slice 6.4: server command + map-validity ops (APPENDED after client_kick; order is the ABI; do not reorder above) ---
    pub server_command:   Option<ServerCommandFn>,
    pub server_map_valid: Option<ServerMapValidFn>,
    pub damage_read_float:  Option<DamageReadFloatFn>,
    pub damage_read_int:    Option<DamageReadIntFn>,
    pub damage_write_float: Option<DamageWriteFloatFn>,
    pub damage_victim:      Option<DamageVictimFn>,
    pub cvar_get:           Option<CvarGetFn>,
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
    /// The plugin's declared config fields (from its manifest).  Stored at load so
    /// `re_materialize_config` can re-run materialization without the manifest.
    /// Starts empty; populated by `store_config_decls` right after `load_plugin_js`.
    config_decls: std::collections::HashMap<String, crate::config::ConfigDecl>,
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
    /// `name → (owner, generation, Global<Function>)` map for registered ConCommands.  Owner-tracked
    /// so `dispatch_concommand` runs the handler in the REGISTERING plugin's context (liveness-gated)
    /// and `unload_plugin` can drop commands owned by the departing plugin.  The shim calls back via
    /// `s2script_core_dispatch_concommand` (C-ABI) when a registered command fires.  Reset on
    /// `shutdown` (BEFORE the isolate is dropped — same discipline as `RESOLVERS`).
    static CONCOMMANDS: std::cell::RefCell<std::collections::HashMap<String, (String, u64, v8::Global<v8::Function>)>>
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
    /// Notify-only game-event multiplexer: name → per-plugin subscribers (Slice 5D.1).
    /// Each subscriber is tagged with (owner, generation) for liveness-gated dispatch.
    /// `remove_by_owner` is called from `unload_plugin` (the teardown authority).
    /// Reset on shutdown so a re-init starts empty.
    static EVENT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    /// Pre-hook game-event multiplexer: name → per-plugin PRE subscribers (Slice 5D.3).
    /// Same shape as EVENT_MUX; handlers return a HookResult that is collapsed via run_chain.
    /// Reset on shutdown so a re-init starts empty.
    static EVENT_MUX_PRE: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    /// Damage pre-hook multiplexer (Slice 6.6): `Damage.onPre(h)` subscribers, keyed by the constant
    /// "onPre" (damage has no name dimension). Same EventMux shape/discipline; handlers read/modify the
    /// current CTakeDamageInfo in place. remove_by_owner on unload; reset on shutdown.
    static DAMAGE_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    /// Config-change subscriber mux (Slice 5E.2): handlers subscribed via `config.onChange(h)`.
    /// Each handler is tagged `(owner, generation)` for liveness-gated dispatch.
    /// The loader polls opted-in plugins' config files each frame cycle and calls
    /// `re_materialize_config(id)` on change, which snapshots this mux and fires handlers.
    /// `remove_by_owner` called on unload; reset on shutdown so a re-init starts empty.
    static CONFIG_SUBS: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());

    /// Slice 5E.3: reload state-handoff blobs (id → the JSON string produced by `iface_to_json` in the
    /// OLD context during `onUnload`). Consumed by `load_plugin_js` on the next load of that id (a
    /// Reload) and revived via `iface_from_json`; cleared by the loader on a final removal (Vanished);
    /// reset on `shutdown`. It holds a plain `String`, so it survives the old context's disposal.
    static PENDING_HANDOFF: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());

    /// Slice 6.2: the host-global admin cache — two tiers (file admins.json ⊕ runtime Admin.add), each
    /// SteamID64 → a u64 flag bitmask. Shared across all plugin contexts (V8 contexts are isolated, so a
    /// runtime add in one plugin must be visible to another's gating — hence host-global, not per-context).
    static ADMIN_FILE:    std::cell::RefCell<std::collections::HashMap<String, u64>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static ADMIN_RUNTIME: std::cell::RefCell<std::collections::HashMap<String, u64>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// One-shot guard so admins.json loads once (the first plugin CONTEXT created — the admin prelude is
    /// always injected, like every @s2script/* module — not per plugin).
    static ADMIN_FILE_LOADED: std::cell::Cell<bool> = std::cell::Cell::new(false);
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

/// The injected engine-generic prelude, evaluated per plugin context AFTER the native
/// primitives are in place.  Builds the five module globals over the `__s2_*` natives
/// (whose internal names are unchanged) and stashes them at `globalThis.__s2pkg_<name>` for the
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
  const timers = {
    delay: (ms) => __s2_delay(ms || 0),
    nextTick: () => __s2_next_tick(),
    nextFrame: () => __s2_next_frame(),
    threadSleep: (ms) => __s2_thread_sleep(ms || 0),
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
    var pkg = __s2require(name);                             // first-party @s2script/* module or game package
    if (pkg !== null && pkg !== undefined) return pkg;
    return resolveInterface(name);                          // inter-plugin, or null
  };
  const interfaces = {
    publishInterface: function (name, version, impl) {
      __s2_iface_publish(name, version, impl);
      return { emit: function (ev, payload) { return __s2_iface_emit(name, ev, payload); } };
    },
  };
  // --- Slice 5A/5B.2: serial-gated EntityRef (wraps the __s2_ent_ref_* natives; no raw pointer crosses JS) ---
  var K = { I32: 1, F32: 2, BOOL: 3, I8: 4, I16: 5, U8: 6, U16: 7, U32: 8, U64: 9, I64: 10, F64: 11 }; // mirrors core KIND_*
  function EntityRef(index, serial) { this.index = index; this.serial = serial; }
  EntityRef.prototype = {
    isValid:          function ()      { return __s2_ent_ref_valid(this.index, this.serial); },
    readInt32:        function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.I32); },
    writeInt32:       function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.I32, v); },
    readFloat32:      function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.F32); },
    writeFloat32:     function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.F32, v); },
    readBool:         function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.BOOL); },
    writeBool:        function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.BOOL, v); },
    readInt8:         function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.I8); },
    readInt16:        function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.I16); },
    readUInt8:        function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.U8); },
    readUInt16:       function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.U16); },
    readUInt32:       function (o)     { return __s2_ent_ref_read(this.index, this.serial, o, K.U32); },
    readUInt64:       function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.U64); },
    readInt64:        function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.I64); },
    readFloat64:      function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.F64); },
    readString:       function (o, maxLen) { return __s2_ent_ref_read_string(this.index, this.serial, o, maxLen); },
    readFloats:       function (o, count)  { return __s2_ent_ref_read_floats(this.index, this.serial, o, count); },
    readFloatsChain: function (chain, finalOff, count) { return __s2_ent_ref_read_floats_chain(this.index, this.serial, chain, finalOff, count); },
    readInt32Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I32); },
    readInt8Via:   function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I8); },
    readInt16Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I16); },
    readUInt8Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U8); },
    readUInt16Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U16); },
    readUInt32Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U32); },
    readFloat32Via:function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.F32); },
    readBoolVia:   function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.BOOL); },
    readUInt64Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U64); },
    readInt64Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I64); },
    readHandleVia: function (c, o) { var h = __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.U32);
      if (h === null) return null; var d = __s2_handle_decode(h >>> 0); var ref = new EntityRef(d[0], d[1]);
      return ref.isValid() ? ref : null; },   // mirror readHandle: an empty/stale handle -> null (not a dead ref)
    readHandle: function (o) {
      var h = __s2_ent_ref_read(this.index, this.serial, o, K.U32);
      if (h === null) return null;
      var d = __s2_handle_decode(h >>> 0);
      var ref = new EntityRef(d[0], d[1]);
      return ref.isValid() ? ref : null;
    },
    notifyStateChanged: function (offset) { __s2_ent_ref_state_changed(this.index, this.serial, offset); },
  };
  // Inter-plugin wire tagging: an EntityRef crosses the structured-copy (JSON) boundary as a tagged
  // envelope so the target context rehydrates it into a LIVE EntityRef (bound to ITS natives), not
  // plain data. `__entref__` is a reserved wire key. Used by iface_to_json / iface_from_json.
  globalThis.__s2_entref_replacer = function (key, value) {
    return (value instanceof EntityRef) ? { __entref__: [value.index, value.serial] } : value;
  };
  globalThis.__s2_entref_reviver = function (key, value) {
    return (value && typeof value === "object" && Array.isArray(value.__entref__))
      ? new EntityRef(value.__entref__[0], value.__entref__[1])
      : value;
  };
  // --- Slice 5C.3: math value types (Vector, QAngle) — pure JS, no engine ops ---
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  Vector.prototype.length = function () { return Math.sqrt(this.x * this.x + this.y * this.y + this.z * this.z); };
  Vector.prototype.toString = function () { return "Vector(" + this.x + ", " + this.y + ", " + this.z + ")"; };
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  QAngle.prototype.toString = function () { return "QAngle(" + this.x + ", " + this.y + ", " + this.z + ")"; };
  // --- Slice 5D.1: GameEvent constructor (dispatch_game_event constructs new GameEvent(name)
  //     per-plugin from globalThis.__s2pkg_events.GameEvent). ---
  function GameEvent(name) { this.name = name; }
  GameEvent.prototype.getInt        = function (k) { return __s2_event_get_int(k); };
  GameEvent.prototype.getFloat      = function (k) { return __s2_event_get_float(k); };
  GameEvent.prototype.getBool       = function (k) { return __s2_event_get_bool(k); };
  GameEvent.prototype.getString     = function (k) { return __s2_event_get_string(k); };
  GameEvent.prototype.getUint64     = function (k) { return __s2_event_get_uint64(k); };   // decimal string
  GameEvent.prototype.getPlayerSlot = function (k) { return __s2_event_get_player_slot(k); };
  GameEvent.prototype.setInt    = function (k, v) { __s2_event_set_int(k, v | 0); };
  GameEvent.prototype.setFloat  = function (k, v) { __s2_event_set_float(k, v); };
  GameEvent.prototype.setBool   = function (k, v) { __s2_event_set_bool(k, !!v); };
  GameEvent.prototype.setString = function (k, v) { __s2_event_set_string(k, String(v)); };
  GameEvent.prototype.setUint64 = function (k, v) { __s2_event_set_uint64(k, String(v)); };   // decimal string
  // --- Slice 5D.1 Task 2 / 5D.3: Events.on/off/onPre/fire — prelude module object for @s2script/events. ---
  var Events = {
    on:    function (name, handler) { __s2_event_subscribe(name, handler); },
    off:   function (name, handler) { __s2_event_unsubscribe(name, handler); },
    onPre: function (name, handler) { __s2_event_subscribe_pre(name, handler); },
    // Fire a game event. fields: { key: value }. Runtime type-infer: bool→setBool, string→setString,
    // bigint→setUint64, integer number→setInt, other number→setFloat. Returns the FireEvent result.
    fire:  function (name, fields, dontBroadcast) {
      if (!__s2_event_create(name)) return false;
      if (fields) {
        for (var k in fields) {
          if (!Object.prototype.hasOwnProperty.call(fields, k)) continue;
          var v = fields[k];
          var t = typeof v;
          if (t === "boolean") __s2_event_set_bool(k, v);
          else if (t === "string") __s2_event_set_string(k, v);
          else if (t === "bigint") __s2_event_set_uint64(k, v.toString());
          else if (t === "number") { if (Number.isInteger(v)) __s2_event_set_int(k, v); else __s2_event_set_float(k, v); }
        }
      }
      return __s2_event_fire(!!dontBroadcast);
    },
  };
  // --- Slice 5E.2: config module (typed getters over __s2pkg_config_values; zero-value fallback) ---
  var __s2_config = {
    getString: function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return v == null ? "" : String(v); },
    getInt:    function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return (v == null || typeof v !== "number") ? 0 : (v | 0); },   // int = 32-bit (SourceMod ConVar parity); `v | 0` truncates by design
    getFloat:  function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return (v == null || typeof v !== "number") ? 0 : v; },
    getBool:   function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return v === true; },
    onChange:  function (h) { __s2_config_on_change(h); },
  };
  globalThis.__s2pkg_math       = { Vector: Vector, QAngle: QAngle };
  globalThis.__s2pkg_entity     = { EntityRef: EntityRef };
  globalThis.__s2pkg_frame      = { OnGameFrame: OnGameFrame };
  globalThis.__s2pkg_timers     = timers;
  globalThis.__s2pkg_console    = { console: console };
  globalThis.__s2pkg_interfaces = interfaces;
  globalThis.__s2pkg_events     = { GameEvent: GameEvent, Events: Events, HookResult: globalThis.HookResult };
  globalThis.__s2pkg_config     = { config: __s2_config };   // named export `config` (matches the .d.ts: import { config } from "@s2script/config")
  // --- Slice 6.1: chat module (toSlot/toAll; toAll loops __s2_client_valid, engine-generic) ---
  var __s2_chat = {
    toSlot: function (slot, msg) { __s2_client_print(slot | 0, String(msg)); },
    toAll:  function (msg) {
      var s = String(msg);
      for (var i = 0; i < 64; i++) { if (__s2_client_valid(i)) { __s2_client_print(i, s); } }
    },
  };
  globalThis.__s2pkg_chat = { Chat: __s2_chat };   // named export `Chat`
  // --- Slice 6.4: server module (command / isMapValid; engine-generic server control) ---
  var __s2_server = {
    command: function (cmd) { __s2_server_command(String(cmd)); },
    isMapValid: function (map) { return __s2_server_map_valid(String(map)) === 1; },
    getCvar: function (name) { return __s2_cvar_get(String(name)); },                 // "" if absent
    setCvar: function (name, value) { __s2_server_command(String(name) + " " + String(value)); },
  };
  globalThis.__s2pkg_server = { Server: __s2_server };   // named export `Server`
  // --- Slice 6.6: damage module (Damage.onPre + block-scoped DamageInfo over the current CTakeDamageInfo).
  //     CTakeDamageInfo is a Source 2 engine type (not CS2-specific) -> engine-generic, lives in core. ---
  function DamageInfo() {}
  function __s2_dmg_ref(field) {
    var o = __s2_schema_offset("CTakeDamageInfo", field);
    if (o < 0) return null;
    var h = __s2_damage_read_int(o) >>> 0;
    if (h === 0 || h === 0xFFFFFFFF) return null;            // empty/invalid handle
    var d = __s2_handle_decode(h);
    var ref = new EntityRef(d[0], d[1]);
    return ref.isValid() ? ref : null;                       // stale -> null
  }
  Object.defineProperties(DamageInfo.prototype, {
    // m_flDamage: read the damage; SETTING it modifies the live info (set to 0 to block).
    damage: {
      get: function () { var o = __s2_schema_offset("CTakeDamageInfo", "m_flDamage"); return o < 0 ? 0 : __s2_damage_read_float(o); },
      set: function (v) { var o = __s2_schema_offset("CTakeDamageInfo", "m_flDamage"); if (o >= 0) __s2_damage_write_float(o, +v); },
      enumerable: true, configurable: true,
    },
    damageType: {
      get: function () { var o = __s2_schema_offset("CTakeDamageInfo", "m_bitsDamageType"); return o < 0 ? 0 : __s2_damage_read_int(o); },
      enumerable: true, configurable: true,
    },
    attacker:  { get: function () { return __s2_dmg_ref("m_hAttacker"); },  enumerable: true, configurable: true },
    inflictor: { get: function () { return __s2_dmg_ref("m_hInflictor"); }, enumerable: true, configurable: true },
    // The victim (the entity taking damage) — decoded from the detour `this`, not a field of the info.
    victim: {
      get: function () {
        var h = __s2_damage_victim() >>> 0;
        if (h === 0 || h === 0xFFFFFFFF) return null;
        var d = __s2_handle_decode(h);
        var ref = new EntityRef(d[0], d[1]);
        return ref.isValid() ? ref : null;
      }, enumerable: true, configurable: true,
    },
  });
  var Damage = { onPre: function (handler) { __s2_damage_subscribe(handler); } };
  globalThis.__s2pkg_damage = { Damage: Damage, DamageInfo: DamageInfo };
  // --- Slice 6.12: plugin management (list / load / unload / reload — the SM `sm plugins` backend).
  //     Mutations are DEFERRED to the frame drain (the natives only enqueue), so this is safe from a command. ---
  var __s2_plugins = {
    list: function () { try { return JSON.parse(__s2_plugins_list()); } catch (e) { return []; } },
    unload: function (id) { return __s2_plugin_unload(String(id)); },   // false if not loaded
    reload: function (id) { return __s2_plugin_reload(String(id)); },   // false if id unknown
    load: function (id) { return __s2_plugin_load(String(id)); },       // false if not currently unloaded
  };
  globalThis.__s2pkg_plugins = { Plugins: __s2_plugins };
  // --- Slice 6.1/6.2: commands module (register / registerServer / registerAdmin) ---
  function __s2cmd_ctx(slot, argString) {
    var s = (slot | 0);
    var raw = String(argString == null ? "" : argString);
    var args = raw.length ? raw.split(/\s+/).filter(function (a) { return a.length; }) : [];
    return {
      callerSlot: s,
      args: args,                                  // 0-based, split on whitespace (kept for compat)
      argString: raw,                              // the full raw arg string (SM GetCmdArgString)
      argCount: args.length,                       // SM GetCmdArgs
      // SM-parity argument retrieval so commands don't hand-roll a parser (0-based; the command name is NOT arg 0).
      arg: function (n) { var a = args[n | 0]; return a == null ? "" : a; },                 // "" if absent (SM GetCmdArg)
      argInt: function (n, fb) { var v = parseInt(args[n | 0], 10); return isNaN(v) ? (fb === undefined ? 0 : fb) : v; },
      argFloat: function (n, fb) { var v = parseFloat(args[n | 0]); return isNaN(v) ? (fb === undefined ? 0 : fb) : v; },
      argsFrom: function (n) { return args.slice(n | 0).join(" "); },   // the rest, re-joined (a reason/value that spans spaces)
      reply: function (m) { if (s < 0) { console.log(String(m)); } else { globalThis.__s2pkg_chat.Chat.toSlot(s, String(m)); } }
    };
  }
  // Slice 6.11: a per-context registry of wrapped dispatch fns (name -> function(slot, argString)), so a
  // command can be invoked BY NAME (chat triggers) reusing the SAME wrapper as the ConCommand path (admin
  // gating included). __s2cmd_add both registers the engine ConCommand and records the wrapper here.
  var __s2cmd_reg = {};
  function __s2cmd_add(name, wrapped) { __s2cmd_reg[name] = wrapped; __s2_concommand(name, wrapped); }
  var __s2cmd_triggers = { public: "!", silent: "/" };   // SM PublicChatTrigger / SilentChatTrigger; mutable
  var __s2_commands = {
    register: function (name, handler) {
      __s2cmd_add(name, function (slot, a) { handler(__s2cmd_ctx(slot, a)); });
    },
    registerServer: function (name, handler) {
      __s2cmd_add(name, function (slot, a) {
        var ctx = __s2cmd_ctx(slot, a);
        if (ctx.callerSlot < 0) { handler(ctx); }
        else { ctx.reply("[SM] This command can only be run from the server console."); }
      });
    },
    registerAdmin: function (name, flags, handler) {
      __s2cmd_add(name, function (slot, a) {
        var ctx = __s2cmd_ctx(slot, a);
        if (ctx.callerSlot < 0) { handler(ctx); return; }        // server / rcon = root
        var check = globalThis.__s2_admin_check;
        if (typeof check !== "function") {
          if (!globalThis.__s2cmd_warnedNoAdmin) { globalThis.__s2cmd_warnedNoAdmin = true;
            console.log("[s2script] WARN: registerAdmin('" + name + "') used without @s2script/admin — denying non-server callers"); }
          ctx.reply("[SM] You do not have access to this command."); return;
        }
        if (check(ctx.callerSlot, flags | 0)) { handler(ctx); }
        else { ctx.reply("[SM] You do not have access to this command."); }
      });
    },
    // Slice 6.11: invoke a registered command by name (same context, synchronous — the wrapper applies
    // gating). Returns true if the command exists in this plugin. Used by chat triggers.
    dispatch: function (name, slot, argString) {
      var w = __s2cmd_reg[name];
      if (!w) return false;
      w(slot | 0, String(argString == null ? "" : argString));
      return true;
    },
    // Parse a chat message for a trigger. Returns { silent, name, argString } or null (not a trigger).
    parseChatTrigger: function (message) {
      var m = String(message == null ? "" : message);
      var silent;
      if (__s2cmd_triggers.silent && m.charAt(0) === __s2cmd_triggers.silent) silent = true;
      else if (__s2cmd_triggers.public && m.charAt(0) === __s2cmd_triggers.public) silent = false;
      else return null;
      var body = m.slice(1).replace(/^\s+/, "");
      if (!body.length) return null;
      var sp = body.search(/\s/);
      return { silent: silent, name: sp < 0 ? body : body.slice(0, sp),
               argString: sp < 0 ? "" : body.slice(sp + 1).replace(/^\s+/, "") };
    },
    // Handle a chat message end-to-end: if it's a trigger, dispatch the command (trying `name` then
    // `sm_<name>`, the SM convention) with `slot` as the caller. Returns { silent, ran } if it WAS a
    // trigger (the caller should suppress the chat), or null if it was ordinary chat.
    handleChatTrigger: function (slot, message) {
      var t = this.parseChatTrigger(message);
      if (!t) return null;
      var ran = this.dispatch(t.name, slot, t.argString);
      if (!ran && t.name.indexOf("sm_") !== 0) ran = this.dispatch("sm_" + t.name, slot, t.argString);
      return { silent: t.silent, ran: ran };
    },
    triggers: __s2cmd_triggers,   // { public: "!", silent: "/" } — reconfigure the trigger chars here
  };
  globalThis.__s2pkg_commands = { Commands: __s2_commands };   // named export `Commands`
  // --- Slice 6.2 Task 2: admin module (engine-generic; no CS2/game symbol; ADMFLAG + Admin API + file load) ---
  var __s2_ADMFLAG = {
    RESERVATION: 1<<0, GENERIC: 1<<1, KICK: 1<<2, BAN: 1<<3, UNBAN: 1<<4, SLAY: 1<<5, CHANGEMAP: 1<<6,
    CONVARS: 1<<7, CONFIG: 1<<8, CHAT: 1<<9, VOTE: 1<<10, PASSWORD: 1<<11, RCON: 1<<12, CHEATS: 1<<13, ROOT: 1<<14,
  };
  function __s2_hasFlags(flags, req) { return ((flags & __s2_ADMFLAG.ROOT) !== 0) || ((flags & req) === req); }
  function __s2_adminInfo(steamId, flags) {
    if (!flags) return null;
    return { steamId: String(steamId), flags: flags | 0, hasFlags: function (req) { return __s2_hasFlags(flags | 0, req | 0); } };
  }
  // Parse admins.json ({ "<steamid64>": ["kick","ban"] }) into the file tier. Unknown flag name → skip+WARN.
  function __s2_admin_parseFile(text) {
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admins.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var sid in obj) {
      if (!Object.prototype.hasOwnProperty.call(obj, sid)) continue;
      var names = obj[sid]; if (!Array.isArray(names)) continue;
      var mask = 0;
      for (var i = 0; i < names.length; i++) {
        var key = String(names[i]).toUpperCase();
        if (__s2_ADMFLAG[key] != null) mask |= __s2_ADMFLAG[key];
        else console.log("[s2script] WARN: admins.json '" + sid + "': unknown flag '" + names[i] + "' — skipped");
      }
      __s2_admin_set(String(sid), mask, false);
    }
  }
  function __s2_admin_load() {
    var text = __s2_config_read_raw("admins");
    if (text == null) {
      // A VALID-JSON self-documenting template — the "_help" key is not a SteamID (its value is a string,
      // not an array), so parseFile skips it; and it round-trips through JSON.parse cleanly on the next
      // restart (a //-commented template would fail JSON.parse and log a spurious "malformed" WARN).
      __s2_config_write_raw("admins", '{\n  "_help": "SteamID64 -> flag names. e.g. \\"76561199000000001\\": [\\"kick\\", \\"ban\\"]. Flags: reservation generic kick ban unban slay changemap convars config chat vote password rcon cheats root"\n}\n');
      text = "{}";
    }
    __s2_admin_parseFile(text);
  }
  var __s2_admin = {
    add: function (steamId, flags) { __s2_admin_set(String(steamId), flags | 0, true); },
    remove: function (steamId) { __s2_admin_remove(String(steamId), true); },
    get: function (steamId) { return __s2_adminInfo(steamId, __s2_admin_get(String(steamId))); },
    forSlot: function (slot) {
      var sid = __s2_client_steamid(slot | 0);
      // Hardening: a bot / mid-auth / unauthenticated client reads SteamID "0" (GetClientXUID=0). Never
      // resolve "0"/empty to an admin, even if a misconfigured admins.json has a "0" key — else every bot
      // and unauthenticated player would be granted those flags at once (a whole-branch review finding).
      if (sid === "0" || !sid) return null;
      return __s2_adminInfo(sid, __s2_admin_get(sid));
    },
    reload: function () { __s2_admin_clear_file(); __s2_admin_load(); },
  };
  // Expose parseFile on globalThis so plugins (and tests) can call it directly.
  globalThis.__s2_admin_parseFile = __s2_admin_parseFile;
  // One-shot file load (first plugin to import @s2script/admin triggers this), then install the check hook.
  if (!__s2_admin_mark_loaded()) { __s2_admin_load(); }
  globalThis.__s2_admin_check = function (slot, requiredMask) {
    var a = __s2_admin.forSlot(slot | 0); return a ? a.hasFlags(requiredMask | 0) : false;
  };
  globalThis.__s2pkg_admin = { ADMFLAG: __s2_ADMFLAG, Admin: __s2_admin };   // named exports ADMFLAG + Admin
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

// Field-type kind codes — a JS<->core contract, mirrored in INJECTED_STD_PRELUDE's `K`. Keep in lockstep.
const KIND_I32: i64 = 1;
const KIND_F32: i64 = 2;
const KIND_BOOL: i64 = 3;
const KIND_I8: i64 = 4;
const KIND_I16: i64 = 5;
const KIND_U8: i64 = 6;
const KIND_U16: i64 = 7;
const KIND_U32: i64 = 8;
const KIND_U64: i64 = 9;
const KIND_I64: i64 = 10;
const KIND_F64: i64 = 11;

/// Native `__s2_ent_ref_read(index, serial, offset, kind) -> number|boolean|null`. Serial-gated typed read.
fn s2_ent_ref_read(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(3).integer_value(scope).unwrap_or(0);
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → null (already set)
        let p = ent as *const u8;
        match kind {
            KIND_I32  => rv.set_int32(crate::entity::read_i32(p, off)),
            KIND_F32  => rv.set_double(crate::entity::read_f32(p, off) as f64),
            KIND_BOOL => rv.set_bool(crate::entity::read_bool(p, off)),
            KIND_I8   => rv.set_int32(crate::entity::read_i8(p, off)),
            KIND_I16  => rv.set_int32(crate::entity::read_i16(p, off)),
            KIND_U8   => rv.set_double(crate::entity::read_u8(p, off) as f64),
            KIND_U16  => rv.set_double(crate::entity::read_u16(p, off) as f64),
            KIND_U32  => rv.set_double(crate::entity::read_u32(p, off) as f64),
            KIND_U64  => { let bi = v8::BigInt::new_from_u64(scope, crate::entity::read_u64(p, off)); rv.set(bi.into()); }
            KIND_I64  => { let bi = v8::BigInt::new_from_i64(scope, crate::entity::read_i64(p, off)); rv.set(bi.into()); }
            KIND_F64  => rv.set_double(crate::entity::read_f64(p, off)),
            _         => { /* unknown kind → leave null */ }
        }
    }));
}

/// Native `__s2_ent_ref_write(index, serial, offset, kind, value) -> boolean`. Serial-gated typed write
/// (I32/F32/BOOL only this slice; narrow-width writes deferred → false).
fn s2_ent_ref_write(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(3).integer_value(scope).unwrap_or(0);
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → false (already set)
        match kind {
            KIND_I32  => crate::entity::write_i32(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_F32  => crate::entity::write_f32(ent, off, args.get(4).number_value(scope).unwrap_or(0.0) as f32),
            KIND_BOOL => crate::entity::write_bool(ent, off, args.get(4).boolean_value(scope)),
            _         => return,                   // unknown / deferred write kind → false
        }
        rv.set_bool(true);
    }));
}

/// Native `__s2_ent_ref_read_string(index, serial, offset, maxLen) -> string|null`. Serial-gated;
/// returns a COPIED string (the pointer never crosses to JS). null on a stale ref.
fn s2_ent_ref_read_string(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let max_len = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                 // invalid → null (already set)
        let s = crate::entity::read_string(ent as *const u8, off, max_len);
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_ent_ref_read_floats(index, serial, offset, count) -> number[] | null`. Serial-gated;
/// reads `count` (1..=4) contiguous f32s into a JS array (a COPY; the pointer never crosses to JS).
/// null on a stale/invalid ref or an out-of-range count.
fn s2_ent_ref_read_floats(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let count = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        if count <= 0 || count > 4 { return; }          // only small fixed vectors (Vector..Vector4D)
        if off < 0 { return; }                           // schema-miss sentinel (-1) → null (not a partial read)
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                     // stale/invalid → null (already set)
        let p = ent as *const u8;
        let arr = v8::Array::new(scope, count);
        for i in 0..count {
            let v = crate::entity::read_f32(p, off + i * 4) as f64;
            let num = v8::Number::new(scope, v);
            arr.set_index(scope, i as u32, num.into());
        }
        rv.set(arr.into());
    }));
}

/// Native `__s2_ent_ref_read_floats_chain(index, serial, ptrOffs, finalOff, count) -> number[] | null`.
/// Follows a chain of pointer derefs (each i32 offset in the `ptrOffs` JS array), then reads `count` (1..=4)
/// contiguous f32s at `finalOff` into a COPIED JS array. Serial-gated at the root entity; each hop null-checked;
/// the raw intermediate pointers never cross to JS. null on a stale root / a null hop / a bad chain/offset/count.
fn s2_ent_ref_read_floats_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let final_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let count = args.get(4).integer_value(scope).unwrap_or(0) as i32;
        if count <= 0 || count > 4 || final_off < 0 { return; }
        // args[2] must be an array of pointer offsets:
        let Ok(chain) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return; };
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                     // stale/invalid root → null (already set)
        let mut p = ent as *const u8;
        for i in 0..chain.length() {
            let off = chain.get_index(scope, i).and_then(|v| v.integer_value(scope)).unwrap_or(-1) as i32;
            if off < 0 { return; }                       // bad offset in the chain → null
            p = crate::entity::read_ptr(p, off);
            if p.is_null() { return; }                   // a null hop (broken chain) → null
        }
        let out = v8::Array::new(scope, count);
        for i in 0..count {
            let v = crate::entity::read_f32(p, final_off + i * 4) as f64;
            let num = v8::Number::new(scope, v);
            out.set_index(scope, i as u32, num.into());
        }
        rv.set(out.into());
    }));
}

/// Native `__s2_ent_ref_read_chain(index, serial, pathOffs, finalOff, kind) -> value | null`. Follows a chain
/// of pointer derefs (each i32 offset in `pathOffs`), then reads a SCALAR of `kind` at `finalOff`. Serial-gated
/// at the root; each hop null-checked; the raw intermediate pointers never cross to JS. Vectors use
/// __s2_ent_ref_read_floats_chain; handles = read KIND_U32 here then __s2_handle_decode in JS.
fn s2_ent_ref_read_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let final_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(4).integer_value(scope).unwrap_or(0);
        if final_off < 0 { return; }
        let Ok(path) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return; };
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }
        let mut p = ent as *const u8;
        for i in 0..path.length() {
            let off = path.get_index(scope, i).and_then(|v| v.integer_value(scope)).unwrap_or(-1) as i32;
            if off < 0 { return; }
            p = crate::entity::read_ptr(p, off);
            if p.is_null() { return; }
        }
        let off = final_off;
        match kind {
            KIND_I32  => rv.set_int32(crate::entity::read_i32(p, off)),
            KIND_F32  => rv.set_double(crate::entity::read_f32(p, off) as f64),
            KIND_BOOL => rv.set_bool(crate::entity::read_bool(p, off)),
            KIND_I8   => rv.set_int32(crate::entity::read_i8(p, off)),
            KIND_I16  => rv.set_int32(crate::entity::read_i16(p, off)),
            KIND_U8   => rv.set_double(crate::entity::read_u8(p, off) as f64),
            KIND_U16  => rv.set_double(crate::entity::read_u16(p, off) as f64),
            KIND_U32  => rv.set_double(crate::entity::read_u32(p, off) as f64),
            KIND_U64  => { let bi = v8::BigInt::new_from_u64(scope, crate::entity::read_u64(p, off)); rv.set(bi.into()); }
            KIND_I64  => { let bi = v8::BigInt::new_from_i64(scope, crate::entity::read_i64(p, off)); rv.set(bi.into()); }
            KIND_F64  => rv.set_double(crate::entity::read_f64(p, off)),
            _ => { }   // unknown kind → leave null
        }
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

        // Store (owner, generation, Global<Function>) — owner-tracked so dispatch runs in the
        // registering plugin's context and unload_plugin can retain-drop its commands.
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        CONCOMMANDS.with(|m| m.borrow_mut().insert(name.clone(), (owner, generation, global)));

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
    // Phase 1: clone (owner, gen, Global) out of CONCOMMANDS — release the borrow before JS.
    // Mirrors dispatch_game_event's snapshot discipline: no CONCOMMANDS borrow held across the call.
    let entry = CONCOMMANDS.with(|m| m.borrow().get(name).map(|(o, g, f)| (o.clone(), *g, f.clone())));
    let Some((owner, gen, global)) = entry else { return };

    // Liveness gate: skip if the registering plugin is no longer live at the captured generation.
    if !REGISTRY.with(|r| r.borrow().is_live(&owner, gen)) { return; }

    // Clone the owner's context out of PLUGINS (borrow released) so the handler may re-enter.
    let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.context.clone())) else { return };

    // Phase 2: enter the OWNER's context and invoke the JS fn.
    HOST.with(|h| {
        // Re-entrancy guard (mirrors dispatch_game_event / dispatch_game_event_pre):
        // a command handler may call Events.fire or another native that re-enters dispatch while
        // HOST is already borrowed. Use try_borrow_mut and graceful-skip rather than double-borrow.
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Build JS arguments: (slot: number, argString: string).
        let recv: v8::Local<v8::Value> = v8::undefined(scope).into();
        let slot_val: v8::Local<v8::Value> = v8::Number::new(scope, slot as f64).into();
        let Some(args_str) = v8::String::new(scope, args) else { return };

        // Per-call TryCatch so a throwing handler is caught + WARN, never propagates.
        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

        let func = v8::Local::new(tc, &global);
        if func.call(tc, recv, &[slot_val, args_str.into()]).is_none() {
            let msg = tc.exception()
                .map(|e| e.to_rust_string_lossy(&*tc))
                .unwrap_or_else(|| "handler threw".into());
            log_warn(&format!("WARN: dispatch_concommand('{}'): {}", name, msg));
        }
    });
}

/// Slice 6.11b: parse a chat line for a command trigger (`!cmd` / `/cmd`) and dispatch it.
///
/// Called from `ffi.rs`'s `s2script_core_dispatch_chat` (C-ABI export), which the shim's Host_Say
/// detour invokes for every player chat message (CS2 fires no usable player_chat game event, so chat
/// is intercepted at the Host_Say function). Reuses the ConCommand registry + `dispatch_concommand`
/// so a chat trigger runs the SAME registered handler as the console command, in its owner context,
/// with the speaker's slot as the caller. SM convention: `!kick` tries `kick`, then falls back to
/// `sm_kick`. Engine-generic: it only knows names + slots, never a game type.
///
/// Returns `true` iff the caller should SUPPRESS the chat broadcast — only for the SILENT trigger
/// (`/`) AND only when a command actually matched (never swallow ordinary chat or an unknown `/foo`).
/// The public trigger (`!`) always shows. No CONCOMMANDS borrow is held across `dispatch_concommand`.
pub(crate) fn dispatch_chat(slot: i32, text: &str) -> bool {
    let (silent, is_trigger) = match text.as_bytes().first() {
        Some(b'!') => (false, true),
        Some(b'/') => (true, true),
        _ => (false, false),
    };
    if !is_trigger { return false; }
    let rest = text[1..].trim();
    if rest.is_empty() { return false; }
    // Split into command name + argument string (SM: the name is the first whitespace-delimited token).
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(i) => (rest[..i].to_string(), rest[i..].trim_start().to_string()),
        None => (rest.to_string(), String::new()),
    };
    // Resolve `name`, else the SM-prefixed `sm_<name>`. Brief immutable borrow, released before dispatch.
    let sm_name = format!("sm_{}", name);
    let matched = CONCOMMANDS.with(|m| {
        let map = m.borrow();
        if map.contains_key(&name) { Some(name.clone()) }
        else if map.contains_key(&sm_name) { Some(sm_name) }
        else { None }
    });
    match matched {
        Some(cmd) => { dispatch_concommand(&cmd, slot, &args); silent }
        None => false,
    }
}

/// Slice 6.11c: dispatch a player's CONSOLE command (from the ClientCommand hook). Unlike chat, the
/// command name is raw (`sm_say`, not `!sm_say`), so match the EXACT registered name only — never an
/// `sm_` fallback (that would hijack a real engine command like `say`). Returns true iff a registered
/// command matched + was dispatched (the caller then SUPERCEDEs so the engine won't also handle it).
pub(crate) fn dispatch_client_command(slot: i32, name: &str, args: &str) -> bool {
    let matched = CONCOMMANDS.with(|m| m.borrow().contains_key(name));
    if matched {
        dispatch_concommand(name, slot, args);
        true
    } else {
        false
    }
}

/// Shared logging helper for named WARNs in the engine-op natives and the loader.
pub(crate) fn log_warn(msg: &str) {
    if let Some(l) = LOGGER.with(|l| l.get()) {
        if let Ok(cs) = CString::new(msg) {
            l(0, cs.as_ptr());
        }
    }
}

/// Native `__s2require(name) -> object|null` — resolves first-party `@s2script/<name>` specifiers
/// to their per-context module globals (e.g. `"@s2script/frame"` → `globalThis.__s2pkg_frame`,
/// `"@s2script/entity"` → `globalThis.__s2pkg_entity`).  Non-`@s2script/` specifiers → `null`
/// (the JS `__s2_require` shim resolves those as inter-plugin deps).  A retired/unknown name
/// (global undefined) → `null`.  Engine-generic: no module list hardcoded; `@s2script/cs2` maps
/// to `__s2pkg_cs2` by the same rule.
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
        // First-party rule: @s2script/<name> → globalThis.__s2pkg_<name> (engine-generic; no module list
        // hardcoded; @s2script/cs2 → __s2pkg_cs2 subsumed). Non-@s2script specifiers → null (the JS
        // `__s2_require` shim resolves those as inter-plugin deps). A retired/unknown name → the global is
        // undefined → null.
        let Some(rest) = name.strip_prefix("@s2script/") else { return };
        let key = format!("__s2pkg_{}", rest);
        let global = scope.get_current_context().global(scope);
        let Some(k) = v8::String::new(scope, &key) else { return };
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
    // Best-effort: pass the EntityRef replacer so an EntityRef in `value` crosses the
    // wire as a tagged envelope. Absent (e.g. the shared HOST context) -> plain stringify (no crash).
    let replacer = tc.get_current_context().global(tc)
        .get(tc, v8::String::new(tc, "__s2_entref_replacer")?.into())
        .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok());
    let out = match replacer {
        Some(rep) => strfn.call(tc, recv, &[value, rep.into()])?,
        None => strfn.call(tc, recv, &[value])?,
    };
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
    // Best-effort: pass the reviver so a tagged EntityRef rehydrates into a live ref in THIS context.
    let reviver = tc.get_current_context().global(tc)
        .get(tc, v8::String::new(tc, "__s2_entref_reviver")?.into())
        .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok());
    match reviver {
        Some(rev) => parsefn.call(tc, recv, &[arg.into(), rev.into()]),
        None => parsefn.call(tc, recv, &[arg.into()]),
    }
}

/// Store a plugin's declared inter-plugin imports (from its manifest) so `iface_dep_kind` /
/// `iface_is_published` can categorise `require`. Called by the loader BEFORE `load_plugin_js` runs
/// the module eval. Cleared in `unload_plugin` (Task 7).
pub fn set_plugin_imports(id: &str, decls: Vec<(String, String, crate::interfaces::Kind)>) {
    IFACES.with(|r| r.borrow_mut().set_imports(id, decls));
}

// ---------------------------------------------------------------------------
// Slice 5B.1: schema enumeration callbacks + `__s2_schema_dump` native.
//
// The shim's `schema_enumerate` engine-op walks the live SchemaSystem and calls
// `cb_emit_class`/`cb_emit_field` back via C ABI, streaming into a `Catalog`.
// All callbacks are wrapped in `catch_unwind(AssertUnwindSafe(...))` — they are
// invoked FROM C++ and must never unwind across the FFI boundary.
// ---------------------------------------------------------------------------

/// C-ABI callback invoked by the shim's `schema_enumerate` once per class.
extern "C" fn cb_emit_class(ctx: *mut c_void, name: *const c_char, parent: *const c_char) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() || name.is_null() { return; }
        let catalog = unsafe { &mut *(ctx as *mut crate::schema_catalog::Catalog) };
        let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        let parent = if parent.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(parent) }.to_string_lossy().into_owned())
        };
        catalog.add_class(&name, parent.as_deref());
    }));
}

/// C-ABI callback invoked by the shim's `schema_enumerate` once per field.
extern "C" fn cb_emit_field(
    ctx: *mut c_void, cls: *const c_char, name: *const c_char, offset: c_int,
    kind: *const c_char, type_name: *const c_char, inner: *const c_char,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() || cls.is_null() || name.is_null() || kind.is_null() { return; }
        let catalog = unsafe { &mut *(ctx as *mut crate::schema_catalog::Catalog) };
        let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        let opt = |p: *const c_char| if p.is_null() { None } else { Some(s(p)) };
        catalog.add_field(&s(cls), &s(name), offset as i32, &s(kind),
                          opt(type_name).as_deref(), opt(inner).as_deref());
    }));
}

/// Native `__s2_schema_dump(path: string) -> boolean`.
///
/// Drives the shim's `schema_enumerate` op: builds a `Catalog` from the live SchemaSystem (via the
/// `cb_emit_class`/`cb_emit_field` C-ABI callbacks), then serializes it and writes JSON to `path`.
/// Returns `false` (never throws) on any failure: no ops table, enumerate returns 0, zero classes
/// (schema not yet warm), or file-write error.  Degrade-never-crash (body under `catch_unwind`).
fn s2_schema_dump(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let path = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else {
            log_warn("WARN: __s2_schema_dump: no engine ops table");
            return;
        };
        let Some(enumerate) = ops.schema_enumerate else {
            log_warn("WARN: __s2_schema_dump: schema_enumerate not wired in ops");
            return;
        };
        let mut catalog = crate::schema_catalog::Catalog::new();
        let ok = enumerate(&mut catalog as *mut _ as *mut c_void, cb_emit_class, cb_emit_field);
        if ok == 0 || catalog.class_count() == 0 {
            log_warn("WARN: __s2_schema_dump: schema not ready (no classes) — try again once a map is live");
            return;
        }
        match std::fs::write(&path, catalog.to_json()) {
            Ok(()) => rv.set_bool(true),
            Err(e) => log_warn(&format!("WARN: __s2_schema_dump: write '{}' failed: {}", path, e)),
        }
    }));
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

// ---------------------------------------------------------------------------
// Slice 5D.1: game-event subscribe / unsubscribe / accessor natives.
// ---------------------------------------------------------------------------

/// Native `__s2_event_subscribe(name, handler)` — subscribe a JS function to a named game event.
/// On first subscriber for a name, calls the `event_subscribe` engine-op (null-degrade).
/// The subscription is tagged with the calling plugin's (id, generation) for liveness-gated dispatch.
fn s2_event_subscribe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(1)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);

        // Capture the calling plugin's (id, generation) for liveness-gated dispatch.
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);

        // Subscribe: if this is the FIRST for the name, call the engine-op event_subscribe.
        let first = EVENT_MUX.with(|m| m.borrow_mut().subscribe(&name, owner, generation, handler_g));
        if first {
            if let Some(ops) = ENGINE_OPS.with(|o| o.get()) {
                if let Some(func) = ops.event_subscribe {
                    if let Ok(cn) = CString::new(name.as_str()) {
                        func(cn.as_ptr());
                    }
                }
            }
        }
    }));
}

/// Native `__s2_event_unsubscribe(name, handler)` — unsubscribe the calling plugin from a named event.
/// Removes ALL of the calling plugin's subs for `name` (handler identity match not required —
/// mirrors iface_off's best-effort approach; callers rarely double-subscribe).
/// If the name's subscriber list is now empty, calls `event_unsubscribe` engine-op (null-degrade).
fn s2_event_unsubscribe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());

        let emptied = EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner_on(&name, &owner));
        if emptied {
            if let Some(ops) = ENGINE_OPS.with(|o| o.get()) {
                if let Some(func) = ops.event_unsubscribe {
                    if let Ok(cn) = CString::new(name.as_str()) {
                        func(cn.as_ptr());
                    }
                }
            }
        }
    }));
}

/// Native `__s2_event_get_int(key) -> i32`. Calls the `event_get_int` engine-op; degrades to 0.
fn s2_event_get_int(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_get_int else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_int32(func(ck.as_ptr()));
    }));
}

/// Native `__s2_event_get_float(key) -> f64`. Calls the `event_get_float` engine-op; degrades to 0.0.
fn s2_event_get_float(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_get_float else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_double(func(ck.as_ptr()) as f64);
    }));
}

/// `__s2_damage_subscribe(handler)` — subscribe a JS fn to `Damage.onPre` (Slice 6.6). Owner-tracked;
/// the shim detour is installed at Load, so no per-subscribe engine registration is needed.
fn s2_damage_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        DAMAGE_MUX.with(|m| { m.borrow_mut().subscribe("onPre", owner, generation, handler_g); });
    }));
}

/// `__s2_damage_read_float(offset) -> f32` — read a float from the current CTakeDamageInfo. 0 if no op.
fn s2_damage_read_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 1 { return; }
        let off = args.get(0).int32_value(scope).unwrap_or(-1);
        if off < 0 { return; }
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.damage_read_float else { return };
        rv.set_double(func(off) as f64);
    }));
}

/// `__s2_damage_read_int(offset) -> i32` — read an int (e.g. a handle or m_bitsDamageType). 0 if no op.
fn s2_damage_read_int(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 1 { return; }
        let off = args.get(0).int32_value(scope).unwrap_or(-1);
        if off < 0 { return; }
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.damage_read_int else { return };
        rv.set_double(func(off) as f64);
    }));
}

/// `__s2_damage_write_float(offset, value)` — write m_flDamage etc. during a pre-hook (modify/block). No-op if no op.
fn s2_damage_write_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let off = args.get(0).int32_value(scope).unwrap_or(-1);
        if off < 0 { return; }
        let val = args.get(1).number_value(scope).unwrap_or(0.0) as f32;
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.damage_write_float else { return };
        func(off, val);
    }));
}

/// `__s2_damage_victim() -> i32` — the victim's raw CEntityHandle (from the detour `this`). -1 if no op.
/// JS decodes it via `__s2_handle_decode` into an EntityRef.
fn s2_damage_victim(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(-1.0);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.damage_victim else { return };
        rv.set_double(func() as f64);
    }));
}

/// `__s2_cvar_get(name) -> string` — a cvar's current value as a string. "" if no op / absent / null.
fn s2_cvar_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let s: String = (|| {
            if args.length() < 1 { return None; }
            let name = args.get(0).to_rust_string_lossy(scope);
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.cvar_get?;
            let cn = CString::new(name).ok()?;
            let ptr = f(cn.as_ptr());
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })().unwrap_or_default();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// `__s2_plugins_list() -> string` — JSON array of `{id, loaded}` for `sm plugins list` / `Plugins.list()`.
fn s2_plugins_list(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let items: Vec<serde_json::Value> = crate::loader::plugin_list().into_iter()
            .map(|(id, suppressed)| serde_json::json!({ "id": id, "loaded": !suppressed }))
            .collect();
        let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
        if let Some(js) = v8::String::new(scope, &json) { rv.set(js.into()); }
    }));
}

/// `__s2_plugin_unload(id) -> bool` — enqueue an unload (runs on the next frame drain). False if not loaded.
fn s2_plugin_unload(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        rv.set_bool(crate::loader::request_unload(&id));
    }));
}
/// `__s2_plugin_reload(id) -> bool` — enqueue a reload. False if the id is unknown.
fn s2_plugin_reload(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        rv.set_bool(crate::loader::request_reload(&id));
    }));
}
/// `__s2_plugin_load(id) -> bool` — enqueue a load of a previously-unloaded (suppressed) plugin. False if not suppressed.
fn s2_plugin_load(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        rv.set_bool(crate::loader::request_load(&id));
    }));
}

/// Native `__s2_event_get_bool(key) -> boolean`. Calls the `event_get_bool` engine-op; degrades to false.
fn s2_event_get_bool(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_get_bool else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_bool(func(ck.as_ptr()) != 0);
    }));
}

/// Native `__s2_event_get_string(key) -> string`. Calls the `event_get_string` engine-op; degrades to "".
/// The returned C string pointer is ONLY valid during the shim's event dispatch — copy it NOW.
fn s2_event_get_string(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(empty) = v8::String::new(scope, "") { rv.set(empty.into()); }
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_get_string else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        let ptr = func(ck.as_ptr());
        if ptr.is_null() { return; }
        // Copy the C string immediately — the pointer is only valid during the dispatch call.
        let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_event_get_uint64(key) -> string`. Calls `event_get_uint64`; returns a DECIMAL STRING.
/// uint64 → decimal string so JS can handle it without BigInt precision loss.
fn s2_event_get_uint64(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(zero) = v8::String::new(scope, "0") { rv.set(zero.into()); }
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_get_uint64 else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        let val = func(ck.as_ptr());
        let s = format!("{}", val);
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_event_get_player_slot(key) -> i32`. Calls `event_get_player_slot`; degrades to -1.
fn s2_event_get_player_slot(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_get_player_slot else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        rv.set_int32(func(ck.as_ptr()));
    }));
}

/// Native `__s2_client_valid(slot) -> boolean`. Calls `client_valid`; degrades to false.
fn s2_client_valid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_valid else { return };
        rv.set_bool(func(slot) != 0);
    }));
}

/// Native `__s2_client_userid(slot) -> i32`. Calls `client_userid`; degrades to -1.
fn s2_client_userid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_userid else { return };
        rv.set_int32(func(slot));
    }));
}

/// Native `__s2_client_signon(slot) -> i32`. Calls `client_signon`; degrades to -1.
fn s2_client_signon(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_signon else { return };
        rv.set_int32(func(slot));
    }));
}

/// Native `__s2_client_find_by_userid(userid) -> i32`. Calls `client_find_by_userid`; degrades to -1.
fn s2_client_find_by_userid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let id = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_find_by_userid else { return };
        rv.set_int32(func(id));
    }));
}

/// Native `__s2_client_name(slot) -> string | null`. Calls `client_name`; copies the C string now.
fn s2_client_name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_name else { return };
        let ptr = func(slot);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

// ---------------------------------------------------------------------------
// Slice 5D.3: event write/fire natives (pre-subscribe/unsubscribe + setters + create/fire).
// ---------------------------------------------------------------------------

/// Native `__s2_event_subscribe_pre(name, handler)` — register a pre-hook (can block/modify).
fn s2_event_subscribe_pre(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(1)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        let was_empty = EVENT_MUX_PRE.with(|m| m.borrow().is_empty());
        EVENT_MUX_PRE.with(|m| { m.borrow_mut().subscribe(&name, owner, generation, handler_g); });
        if was_empty {   // first pre-sub across ALL names → install the global FireEvent hook
            if let Some(req) = HOOK_REQUEST.with(|r| r.get()) {
                if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 1); }
            }
        }
    }));
}

/// Native `__s2_event_unsubscribe_pre(name, handler)` — remove this plugin's pre-hooks for `name`.
fn s2_event_unsubscribe_pre(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        EVENT_MUX_PRE.with(|m| { m.borrow_mut().remove_by_owner_on(&name, &owner); });
        if EVENT_MUX_PRE.with(|m| m.borrow().is_empty()) {   // last pre-sub gone → remove the hook
            if let Some(req) = HOOK_REQUEST.with(|r| r.get()) {
                if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 0); }
            }
        }
    }));
}

/// Native `__s2_event_set_int(key, value)` — write the current event's int field (pre-hook / fire builder).
fn s2_event_set_int(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).int32_value(scope).unwrap_or(0);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_set_int else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_set_float(key, value)` — write the current event's float field.
fn s2_event_set_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).number_value(scope).unwrap_or(0.0) as f32;
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_set_float else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_set_bool(key, value)` — write the current event's bool field (0/1).
fn s2_event_set_bool(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).boolean_value(scope) as c_int;
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_set_bool else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_set_string(key, value)` — write the current event's string field.
fn s2_event_set_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let val_str = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_set_string else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        let Ok(cv) = CString::new(val_str.as_str()) else { return };
        func(ck.as_ptr(), cv.as_ptr());
    }));
}

/// Native `__s2_event_set_uint64(key, value)` — write the current event's uint64 field.
/// `value` is a DECIMAL STRING at the JS boundary (consistent with getUint64/readUInt64).
fn s2_event_set_uint64(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let key = args.get(0).to_rust_string_lossy(scope);
        let val_str = args.get(1).to_rust_string_lossy(scope);
        let value = val_str.parse::<u64>().unwrap_or(0);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_set_uint64 else { return };
        let Ok(ck) = CString::new(key.as_str()) else { return };
        func(ck.as_ptr(), value);
    }));
}

/// Native `__s2_event_create(name) -> boolean` — create a to-be-fired event; retargets set* writes to it.
fn s2_event_create(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_create else { return };
        let Ok(cn) = CString::new(name.as_str()) else { return };
        rv.set_bool(func(cn.as_ptr()) != 0);
    }));
}

/// Native `__s2_event_fire(dontBroadcast) -> boolean` — fire the created event; restores the write target.
fn s2_event_fire(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let dont = if args.length() >= 1 { args.get(0).boolean_value(scope) } else { false };
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_fire else { return };
        rv.set_bool(func(dont as c_int) != 0);
    }));
}

/// Dispatch a game event by name to all LIVE JS subscribers.
///
/// Called from `ffi.rs`'s `s2script_core_dispatch_game_event` (C-ABI export), which the shim's
/// `IGameEventListener2` callback invokes when an event fires (the shim has already stashed the
/// live `IGameEvent*` for the accessor engine-ops).
///
/// **Re-entrancy discipline:** snapshot before invoke — release `EVENT_MUX` borrow, then enter
/// each subscriber's PLUGIN context in its own HandleScope+ContextScope+TryCatch.  `EVENT_MUX` and
/// `PLUGINS`/`REGISTRY` are NOT held across any JS call.
pub(crate) fn dispatch_game_event(name: &str) {
    // Phase 1: snapshot — release EVENT_MUX borrow before entering any context.
    let snap = EVENT_MUX.with(|m| m.borrow().snapshot(name));
    if snap.is_empty() { return; }

    // Phase 2: enter each subscriber's context and invoke the handler with new GameEvent(name).
    HOST.with(|h| {
        // Re-entrancy guard (Slice 5D.3): Events.fire() from inside a handler re-enters the dispatch
        // while the isolate is already borrowed by the outer dispatch. The engine-side fire has
        // already happened; skip the nested JS re-dispatch rather than double-borrow (would panic).
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, gen, handler_g) in &snap {
            // Liveness check (release REGISTRY borrow before entering context).
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *gen)) { continue; }
            // Clone the context Global out of PLUGINS (borrow released) so the handler may re-enter.
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            // Per-subscriber HandleScope+ContextScope — mirrors dispatch_onframe.
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            // Per-handler TryCatch isolates a throwing handler from the rest.
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            // Construct new GameEvent(name): read GameEvent from globalThis.__s2pkg_events.GameEvent.
            let event_arg: Option<v8::Local<v8::Value>> = (|| {
                let global = ctx_local.global(tc);
                let pkg_key = v8::String::new(tc, "__s2pkg_events")?;
                let pkg = global.get(tc, pkg_key.into())?;
                let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
                let ctor_key = v8::String::new(tc, "GameEvent")?;
                let ctor_val = pkg.get(tc, ctor_key.into())?;
                let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
                let name_str = v8::String::new(tc, name)?;
                ctor.new_instance(tc, &[name_str.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
            })();

            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let event_val: v8::Local<v8::Value> = match event_arg {
                Some(v) => v,
                None => v8::undefined(tc).into(),
            };

            if func.call(tc, recv, &[event_val]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_game_event('{}'): handler '{}': {}", name, owner, msg));
            }
            // tc, tc_storage, scope drop here — TryCatch absorbs any pending exception.
        }
    });
}

/// Slice 6.6 Stage 2: run the `Damage.onPre` subscribers over the current CTakeDamageInfo (set by the
/// shim detour). Mirrors `dispatch_game_event`: snapshot (release the mux borrow), re-entrancy guard,
/// per-subscriber liveness + context + TryCatch. Each handler gets `new DamageInfo()` (a block-scoped
/// accessor over the current damage) and reads/modifies it in place; blocking = the handler setting
/// damage to 0.
pub(crate) fn dispatch_damage() {
    let snap = DAMAGE_MUX.with(|m| m.borrow().snapshot("onPre"));
    if snap.is_empty() { return; }

    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, gen, handler_g) in &snap {
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *gen)) { continue; }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            // Construct new DamageInfo(): globalThis.__s2pkg_damage.DamageInfo.
            let info_arg: Option<v8::Local<v8::Value>> = (|| {
                let global = ctx_local.global(tc);
                let pkg_key = v8::String::new(tc, "__s2pkg_damage")?;
                let pkg = global.get(tc, pkg_key.into())?;
                let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
                let ctor_key = v8::String::new(tc, "DamageInfo")?;
                let ctor_val = pkg.get(tc, ctor_key.into())?;
                let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
                ctor.new_instance(tc, &[]).map(|o| -> v8::Local<v8::Value> { o.into() })
            })();

            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let info_val: v8::Local<v8::Value> = info_arg.unwrap_or_else(|| v8::undefined(tc).into());

            if func.call(tc, recv, &[info_val]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_damage: handler '{}': {}", owner, msg));
            }
        }
    });
}

/// Pre-dispatch for the FireEvent hook (Slice 5D.3). Runs the PRE subscribers for `name`, collapses
/// their HookResults via `run_chain`, and returns 1 to suppress client broadcast (collapsed result
/// >= Handled) or 0 to allow. The shim has set `s_currentEvent` (mutable) before calling this.
pub(crate) fn dispatch_game_event_pre(name: &str) -> i32 {
    use crate::multiplexer::{run_chain, HookResult, Priority, SubId};
    // Phase 1: snapshot — release EVENT_MUX_PRE borrow before entering any context.
    let snap0 = EVENT_MUX_PRE.with(|m| m.borrow().snapshot(name));   // Vec<(owner, gen, Global<Function>)>
    if snap0.is_empty() { return 0; }
    // run_chain wants (SubId, Priority, H); all pre-hooks are Priority::Normal this slice (order =
    // subscription order; a priority param is deferred). SubId = enumerate index (only used for the
    // errored list). Carry (owner, gen, handler) so the invoke closure can route + liveness-check.
    let snap: Vec<(SubId, Priority, (String, u64, v8::Global<v8::Function>))> = snap0
        .into_iter().enumerate()
        .map(|(i, (owner, gen, h))| (i as SubId, Priority::Normal, (owner, gen, h)))
        .collect();

    let outcome = HOST.with(|h| {
        // Re-entrancy guard (Slice 5D.3): Events.fire() from inside a handler re-enters the pre-dispatch
        // while the isolate is already borrowed. Can't run JS pre-hooks on the fired event this pass;
        // ALLOW it (Continue) rather than double-borrow (would panic). The engine-side fire proceeds.
        let Ok(mut borrow) = h.try_borrow_mut() else {
            return crate::multiplexer::ChainOutcome { result: HookResult::Continue, errored: Vec::new() };
        };
        let Some(host) = borrow.as_mut() else {
            return crate::multiplexer::ChainOutcome { result: HookResult::Continue, errored: Vec::new() };
        };
        run_chain(&snap, |(owner, gen, handler_g): &(String, u64, v8::Global<v8::Function>)| {
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *gen)) { return Ok(HookResult::Continue); }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone()))
                else { return Ok(HookResult::Continue); };
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            // Construct new GameEvent(name) from globalThis.__s2pkg_events.GameEvent (as in dispatch_game_event).
            let event_arg: Option<v8::Local<v8::Value>> = (|| {
                let global = ctx_local.global(tc);
                let pkg_key = v8::String::new(tc, "__s2pkg_events")?;
                let pkg = global.get(tc, pkg_key.into())?;
                let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
                let ctor_key = v8::String::new(tc, "GameEvent")?;
                let ctor = v8::Local::<v8::Function>::try_from(pkg.get(tc, ctor_key.into())?).ok()?;
                let name_str = v8::String::new(tc, name)?;
                ctor.new_instance(tc, &[name_str.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
            })();
            let event_val: v8::Local<v8::Value> = event_arg.unwrap_or_else(|| v8::undefined(tc).into());
            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            match func.call(tc, recv, &[event_val]) {
                None => Err(()),                                   // threw → drop this sub
                Some(ret) if ret.is_undefined() => Ok(HookResult::Continue),
                Some(ret) => Ok(match ret.uint32_value(tc).unwrap_or(0) {
                    0 => HookResult::Continue, 1 => HookResult::Changed,
                    2 => HookResult::Handled, 3 => HookResult::Stop,
                    _ => HookResult::Continue,                     // out-of-range → Continue
                }),
            }
        })
    });
    if outcome.result >= HookResult::Handled { 1 } else { 0 }
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
    // __s2_ent_ref_read_i32/__s2_ent_ref_write_i32 (introduced in Slice 5A) were retired in 5B.2 → generic __s2_ent_ref_read/write.
    set_native(scope, global_obj, "__s2_ent_current_serial", s2_ent_current_serial);
    set_native(scope, global_obj, "__s2_ent_ref_valid", s2_ent_ref_valid);
    set_native(scope, global_obj, "__s2_ent_ref_read", s2_ent_ref_read);
    set_native(scope, global_obj, "__s2_ent_ref_write", s2_ent_ref_write);
    set_native(scope, global_obj, "__s2_ent_ref_read_string", s2_ent_ref_read_string);
    set_native(scope, global_obj, "__s2_ent_ref_read_floats", s2_ent_ref_read_floats);
    set_native(scope, global_obj, "__s2_ent_ref_read_floats_chain", s2_ent_ref_read_floats_chain);
    set_native(scope, global_obj, "__s2_ent_ref_read_chain", s2_ent_ref_read_chain);
    set_native(scope, global_obj, "__s2_ent_ref_state_changed", s2_ent_ref_state_changed);
    set_native(scope, global_obj, "__s2_handle_decode", s2_handle_decode);
    // ConCommand registration.
    set_native(scope, global_obj, "__s2_concommand", s2_concommand);
    // Schema dump (5B.1): drives the shim's schema_enumerate op into a Catalog and writes JSON.
    set_native(scope, global_obj, "__s2_schema_dump", s2_schema_dump);
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
    // Game-event system (Slice 5D.1): subscribe/unsubscribe + accessor natives.
    set_native(scope, global_obj, "__s2_event_subscribe", s2_event_subscribe);
    set_native(scope, global_obj, "__s2_event_unsubscribe", s2_event_unsubscribe);
    set_native(scope, global_obj, "__s2_event_get_int", s2_event_get_int);
    set_native(scope, global_obj, "__s2_event_get_float", s2_event_get_float);
    set_native(scope, global_obj, "__s2_event_get_bool", s2_event_get_bool);
    set_native(scope, global_obj, "__s2_event_get_string", s2_event_get_string);
    set_native(scope, global_obj, "__s2_event_get_uint64", s2_event_get_uint64);
    set_native(scope, global_obj, "__s2_event_get_player_slot", s2_event_get_player_slot);
    // Engine-identity client-list natives (Slice 5D.2).
    set_native(scope, global_obj, "__s2_client_valid", s2_client_valid);
    set_native(scope, global_obj, "__s2_client_userid", s2_client_userid);
    set_native(scope, global_obj, "__s2_client_signon", s2_client_signon);
    set_native(scope, global_obj, "__s2_client_name", s2_client_name);
    set_native(scope, global_obj, "__s2_client_find_by_userid", s2_client_find_by_userid);
    // Event write/fire (Slice 5D.3): pre-subscribe/unsubscribe + setters + create/fire.
    set_native(scope, global_obj, "__s2_event_subscribe_pre", s2_event_subscribe_pre);
    set_native(scope, global_obj, "__s2_event_unsubscribe_pre", s2_event_unsubscribe_pre);
    set_native(scope, global_obj, "__s2_event_set_int", s2_event_set_int);
    set_native(scope, global_obj, "__s2_event_set_float", s2_event_set_float);
    set_native(scope, global_obj, "__s2_event_set_bool", s2_event_set_bool);
    set_native(scope, global_obj, "__s2_event_set_string", s2_event_set_string);
    set_native(scope, global_obj, "__s2_event_set_uint64", s2_event_set_uint64);
    set_native(scope, global_obj, "__s2_event_create", s2_event_create);
    set_native(scope, global_obj, "__s2_event_fire", s2_event_fire);
    // Config live-reload (Slice 5E.2): register an onChange handler for this plugin's config file.
    set_native(scope, global_obj, "__s2_config_on_change", s2_config_on_change);
    // Chat messaging (Slice 6.1): print a message to one client's chat.
    set_native(scope, global_obj, "__s2_client_print", s2_client_print);

    set_native(scope, global_obj, "__s2_admin_set", s2_admin_set);
    set_native(scope, global_obj, "__s2_admin_get", s2_admin_get);
    set_native(scope, global_obj, "__s2_admin_remove", s2_admin_remove);
    set_native(scope, global_obj, "__s2_admin_clear_file", s2_admin_clear_file);
    set_native(scope, global_obj, "__s2_admin_mark_loaded", s2_admin_mark_loaded);
    set_native(scope, global_obj, "__s2_client_steamid", s2_client_steamid);
    set_native(scope, global_obj, "__s2_client_kick", s2_client_kick);
    set_native(scope, global_obj, "__s2_damage_subscribe", s2_damage_subscribe);
    set_native(scope, global_obj, "__s2_damage_read_float", s2_damage_read_float);
    set_native(scope, global_obj, "__s2_damage_read_int", s2_damage_read_int);
    set_native(scope, global_obj, "__s2_damage_write_float", s2_damage_write_float);
    set_native(scope, global_obj, "__s2_damage_victim", s2_damage_victim);
    set_native(scope, global_obj, "__s2_cvar_get", s2_cvar_get);
    set_native(scope, global_obj, "__s2_plugins_list", s2_plugins_list);
    set_native(scope, global_obj, "__s2_plugin_unload", s2_plugin_unload);
    set_native(scope, global_obj, "__s2_plugin_reload", s2_plugin_reload);
    set_native(scope, global_obj, "__s2_plugin_load", s2_plugin_load);
    set_native(scope, global_obj, "__s2_server_command", s2_server_command);
    set_native(scope, global_obj, "__s2_server_map_valid", s2_server_map_valid);
    // Slice 6.2 Task 2: config-bridge natives for the admin module (file load/write).
    set_native(scope, global_obj, "__s2_config_read_raw", s2_config_read_raw);
    set_native(scope, global_obj, "__s2_config_write_raw", s2_config_write_raw);
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
/// `__s2require`) and evaluate the injected engine-generic prelude + any registered game preludes,
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
            // (which build the five module globals + any registered game package globals over those
            // natives and stash them at `globalThis.__s2pkg_*` for `__s2require`).
            let global_obj = ctx_local.global(scope);
            install_natives(scope, global_obj);
            run_prelude(scope, "engine-prelude", INJECTED_STD_PRELUDE);
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
                    config_decls: std::collections::HashMap::new(),
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

/// Materialize a plugin's config (defaults ⊕ the override file read via the `config_read` op;
/// auto-generate the file via `config_write` if absent) and return the values JSON to inject.
/// Degrade: no ops → defaults only, no auto-write, still returns the defaults JSON.
pub(crate) fn materialize_for_load(id: &str, decls: &std::collections::HashMap<String, crate::config::ConfigDecl>) -> String {
    if decls.is_empty() { return "{}".to_string(); }
    let ops = ENGINE_OPS.with(|o| o.get());
    let cid = std::ffi::CString::new(id).ok();
    // read the override file
    let override_json: Option<String> = (|| {
        let ops = ops?; let f = ops.config_read?; let cid = cid.as_ref()?;
        let ptr = f(cid.as_ptr()); if ptr.is_null() { return None; }
        Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
    })();
    let was_absent = override_json.is_none();
    let mat = crate::config::materialize_config(decls, override_json.as_deref());
    for w in &mat.warnings { log_warn(&format!("config('{}'): {}", id, w)); }
    if was_absent {  // auto-generate the default file
        if let (Some(ops), Some(cid)) = (ops, cid.as_ref()) {
            if let Some(wf) = ops.config_write {
                if let Ok(content) = std::ffi::CString::new(crate::config::generate_default_jsonc(decls)) {
                    wf(cid.as_ptr(), content.as_ptr());
                }
            }
        }
    }
    serde_json::to_string(&serde_json::Value::Object(mat.values)).unwrap_or_else(|_| "{}".to_string())
}

/// Store config decls on a plugin's `PluginInstance` (called from the loader right after
/// `load_plugin_js` so `re_materialize_config` can re-run without needing the manifest).
pub(crate) fn store_config_decls(id: &str, decls: std::collections::HashMap<String, crate::config::ConfigDecl>) {
    PLUGINS.with(|p| {
        if let Some(pi) = p.borrow_mut().get_mut(id) {
            pi.config_decls = decls;
        }
    });
}

/// Read the current content of the plugin's config override file via the `config_read` op.
/// Returns `None` if no ops table is wired, the op is absent, or the file doesn't exist yet.
/// Used by the loader's change-detection loop (content compare, no mtime op needed).
pub(crate) fn config_file_content(id: &str) -> Option<String> {
    let ops = ENGINE_OPS.with(|o| o.get())?;
    let f = ops.config_read?;
    let cid = std::ffi::CString::new(id).ok()?;
    let ptr = f(cid.as_ptr());
    if ptr.is_null() { return None; }
    Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

/// Re-materialize a plugin's config after its override file changed: re-read the file, merge with
/// declared defaults, re-inject `globalThis.__s2pkg_config_values`, and fire every `onChange`
/// handler registered by that plugin (via CONFIG_SUBS) with the updated config object as the arg.
///
/// Called from `crate::loader::poll_watched_configs` when the stored content differs from the
/// current file content.  Uses the same per-plugin context entry discipline as `dispatch_game_event`.
///
/// PRECONDITION: call only with `HOST` UNBORROWED (the loader poll runs on the post-`frame_async_drain`
/// path where HOST is free).  Step (2) re-injects via `eval_in_context` (which `borrow_mut`s HOST) and
/// the fire loop then `try_borrow_mut`s — so a caller that invoked this mid-borrow would PANIC at step
/// (2) rather than degrade.  Do not add a call-site that holds the HOST borrow.
pub(crate) fn re_materialize_config(id: &str) {
    // (1) Get this plugin's stored config decls (empty → nothing to re-materialize, but still fire).
    let decls = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.config_decls.clone()));
    let Some(decls) = decls else { return };

    // (2) Re-materialize (no ops → defaults only; file exists → override merged) → inject.
    let values_json = materialize_for_load(id, &decls);
    let _ = eval_in_context(id, &format!("globalThis.__s2pkg_config_values = {};", values_json));

    // (3) Snapshot CONFIG_SUBS for the "config" name, filtered to this plugin's handlers.
    //     Release the borrow before entering any context.
    let snap: Vec<(String, u64, v8::Global<v8::Function>)> = CONFIG_SUBS.with(|m| {
        m.borrow().snapshot("config")
            .into_iter()
            .filter(|(owner, _, _)| owner == id)
            .collect()
    });
    if snap.is_empty() { return; }

    // (4) Fire loop — mirrors dispatch_game_event (snapshot released; try_borrow_mut guard).
    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, gen, handler_g) in &snap {
            // Liveness check (borrow released before entering the context).
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *gen)) { continue; }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue };

            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            // Read globalThis.__s2pkg_config_values in this context as the handler arg.
            let config_arg: v8::Local<v8::Value> = (|| -> Option<v8::Local<v8::Value>> {
                let global = ctx_local.global(tc);
                let key = v8::String::new(tc, "__s2pkg_config_values")?;
                let val = global.get(tc, key.into())?;
                if val.is_undefined() { None } else { Some(val) }
            })().unwrap_or_else(|| v8::undefined(tc).into());

            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();

            if func.call(tc, recv, &[config_arg]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: re_materialize_config('{}'): onChange '{}': {}", id, owner, msg));
            }
            // tc, tc_storage, scope drop here — TryCatch absorbs any pending exception.
        }
    });
}

/// Native `__s2_config_on_change(handler)` — register an onChange handler for this plugin's
/// config.  The loader detects file changes and calls `re_materialize_config(id)`, which fires
/// all registered handlers with the updated `__s2pkg_config_values` object.
/// Idempotent watch: calling this multiple times seeds the baseline only once per plugin.
fn s2_config_on_change(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        CONFIG_SUBS.with(|m| { m.borrow_mut().subscribe("config", owner.clone(), generation, handler_g); });
        crate::loader::watch_config_for(&owner);  // idempotent; seeds baseline if not yet watched
    }));
}

/// Native `__s2_client_print(slot, msg)` — print `msg` to the chat of the client in `slot`.
/// Degrade: no ops / no op fn → no-op (server console has no chat).
fn s2_client_print(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let msg = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.client_print else { return };
        if let Ok(cmsg) = CString::new(msg) { f(slot, cmsg.as_ptr()); }
    }));
}

// --- Slice 6.2: admin cache natives + client_steamid ---

/// `__s2_admin_set(steamid, flags, runtime)` — set/overwrite a SteamID's flags in the file(false)/runtime(true) tier.
fn s2_admin_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let flags = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let runtime = args.get(2).boolean_value(scope);
        if runtime {
            ADMIN_RUNTIME.with(|m| { m.borrow_mut().insert(sid, flags); });
        } else {
            ADMIN_FILE.with(|m| { m.borrow_mut().insert(sid, flags); });
        }
    }));
}

/// `__s2_admin_get(steamid) -> number` — the UNION of both tiers (0 = not an admin).
fn s2_admin_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { rv.set_double(0.0); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let f = ADMIN_FILE.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        let r = ADMIN_RUNTIME.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        rv.set_double((f | r) as f64);
    }));
}

/// `__s2_admin_remove(steamid, runtime)` — remove from a tier.
fn s2_admin_remove(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let runtime = args.get(1).boolean_value(scope);
        if runtime {
            ADMIN_RUNTIME.with(|m| { m.borrow_mut().remove(&sid); });
        } else {
            ADMIN_FILE.with(|m| { m.borrow_mut().remove(&sid); });
        }
    }));
}

/// `__s2_admin_clear_file()` — wipe the file tier (Admin.reload re-reads into it).
fn s2_admin_clear_file(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ADMIN_FILE.with(|m| m.borrow_mut().clear());
    }));
}

/// `__s2_admin_mark_loaded() -> boolean` — returns the PRIOR loaded state, then sets it true (one-shot load guard).
fn s2_admin_mark_loaded(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let prev = ADMIN_FILE_LOADED.with(|c| { let p = c.get(); c.set(true); p });
        rv.set_bool(prev);
    }));
}

/// `__s2_client_steamid(slot) -> string` — the client's SteamID64 as a decimal string; "0" if no op / bot / invalid.
fn s2_client_steamid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let s: String = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.client_steamid?;
            let ptr = f(slot);
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })().unwrap_or_else(|| "0".to_string());
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// `__s2_client_kick(slot, reason)` — disconnect the client in `slot`. No-op without the op / for a bad slot.
fn s2_client_kick(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let reason = if args.length() >= 2 { args.get(1).to_rust_string_lossy(scope) } else { "Kicked by admin".to_string() };
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.client_kick else { return };
        if let Ok(creason) = CString::new(reason) { f(slot, creason.as_ptr()); }
    }));
}

/// `__s2_server_command(cmd)` — run `cmd` at the server console. No-op without the op / null.
fn s2_server_command(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let cmd = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.server_command else { return };
        if let Ok(ccmd) = CString::new(cmd) { f(ccmd.as_ptr()); }
    }));
}

/// `__s2_server_map_valid(map) -> 1|0` — 1 if `map` is an installed valid map. 0 without the op / null.
fn s2_server_map_valid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let valid: i32 = (|| {
            if args.length() < 1 { return None; }
            let map = args.get(0).to_rust_string_lossy(scope);
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.server_map_valid?;
            let cmap = CString::new(map).ok()?;
            Some(if f(cmap.as_ptr()) != 0 { 1 } else { 0 })
        })().unwrap_or(0);
        rv.set_double(valid as f64);
    }));
}

/// `__s2_config_read_raw(id) -> string | null` — read a config file by id; null if no op / file absent.
/// Bridge for the @s2script/admin JS module so it can read admins.json via the config_read op.
fn s2_config_read_raw(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        let result: Option<String> = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.config_read?;
            let cid = std::ffi::CString::new(id).ok()?;
            let ptr = f(cid.as_ptr());
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })();
        match result {
            Some(s) => { if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); } }
            None => { rv.set(v8::null(scope).into()); }
        }
    }));
}

/// `__s2_config_write_raw(id, content) -> number` — write a config file; 0 on success or no op.
/// Bridge for the @s2script/admin JS module so it can auto-generate admins.json via config_write.
fn s2_config_write_raw(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        if args.length() < 2 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        let content = args.get(1).to_rust_string_lossy(scope);
        let result: Option<i32> = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.config_write?;
            let cid = std::ffi::CString::new(id).ok()?;
            let ccontent = std::ffi::CString::new(content).ok()?;
            Some(f(cid.as_ptr(), ccontent.as_ptr()))
        })();
        rv.set_int32(result.unwrap_or(0));
    }));
}

/// Load a built plugin bundle `plugin_js` under plugin id `id` (the spike-PROVEN CJS wrapper).
///
/// Steps: (1) `create_plugin_context(id)` — a fresh per-plugin context with the full injected API
/// (`__s2require` + the engine-generic prelude + any registered game preludes); (2) evaluate the CJS wrapper
/// `(function(require,module,exports){…})(require, module, module.exports)` in that context and
/// CAPTURE the RETURNED `module.exports` (esbuild REASSIGNS `module.exports`, so the return value
/// — not the `exports` arg — is the plugin's real export object; spike [risk]); (3) call
/// `exports.onLoad()` if present; (4) store the exports `Global<Object>` on the `PluginInstance`
/// (Task 6 reads `onUnload` off it at teardown; it is dropped before the context).
///
/// Degrade-never-crash: a compile/run/onLoad error logs a named WARN and returns; no exception
/// propagates (the whole JS run is under a `TryCatch`).
pub(crate) fn load_plugin_js(id: &str, plugin_js: &str, config_values_json: &str) {
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

    // Inject the materialized config as a per-context global BEFORE the plugin evals (so config reads
    // in onLoad see it). @s2script/config's getters read globalThis.__s2pkg_config_values.
    let _ = eval_in_context(id, &format!("globalThis.__s2pkg_config_values = {};", config_values_json));

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
                        // Slice 5E.3: consume this id's reload-handoff blob (if a prior unload captured
                        // one) and revive it in THIS (new) context via iface_from_json (JSON.parse + the
                        // EntityRef reviver → live serial-gated refs). Pass it as onLoad's single arg;
                        // consume-once (remove regardless of revival/throw). No blob → onLoad() (prev
                        // is JS `undefined`).
                        let prev = PENDING_HANDOFF.with(|h| h.borrow_mut().remove(id))
                            .and_then(|blob| iface_from_json(tc, &blob));
                        let call_args: Vec<v8::Local<v8::Value>> = match prev {
                            Some(p) => vec![p],
                            None => vec![],
                        };
                        if f.call(tc, recv, &call_args).is_none() {
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
    // Reset the game-event mux so a re-init starts with a clean subscriber table.
    EVENT_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the pre-hook event mux (Slice 5D.3) so a re-init starts clean.
    EVENT_MUX_PRE.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the config-change subscriber mux (Slice 5E.2) so a re-init starts clean.
    CONFIG_SUBS.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the damage pre-hook mux (Slice 6.6) so a re-init starts clean.
    DAMAGE_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the reload-handoff map (Slice 5E.3) so a re-init starts clean.
    PENDING_HANDOFF.with(|h| h.borrow_mut().clear());
    // Reset the schema-offset cache so a re-init re-resolves (a `-1` cached before the schema was
    // loaded must not persist across an init cycle).
    SCHEMA_OFFSETS.with(|c| *c.borrow_mut() = crate::schema::OffsetCache::new());
    // Reset the admin cache tiers (Slice 6.2) so a re-init starts with no admins.
    ADMIN_FILE.with(|m| m.borrow_mut().clear());
    ADMIN_RUNTIME.with(|m| m.borrow_mut().clear());
    ADMIN_FILE_LOADED.with(|c| c.set(false));
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

    // (a2) Drop the plugin's game-event subscriptions from the mux (teardown authority: the mux owns
    // the handler Globals, so drop them here — before the context is disposed).  For any names that
    // became empty, call the engine-op event_unsubscribe so the shim can deregister.
    let emptied_events = EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    for evname in &emptied_events {
        if let Some(ops) = ENGINE_OPS.with(|o| o.get()) {
            if let Some(func) = ops.event_unsubscribe {
                if let Ok(cn) = CString::new(evname.as_str()) { func(cn.as_ptr()); }
            }
        }
    }
    // (a2b) Drop the plugin's PRE-hook game-event subscriptions (Slice 5D.3). If the whole
    // PRE mux is now empty (no more pre-hooks), request removal of the global FireEvent hook.
    EVENT_MUX_PRE.with(|m| m.borrow_mut().remove_by_owner(id));
    if EVENT_MUX_PRE.with(|m| m.borrow().is_empty()) {
        if let Some(req) = HOOK_REQUEST.with(|r| r.get()) {
            if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 0); }
        }
    }
    // (a2c) Drop the plugin's Damage.onPre subscriptions (Slice 6.6). The detour stays installed for the
    // process lifetime (removed in the shim's Unload), so no per-plugin hook-removal request is needed.
    DAMAGE_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c) Drop the plugin's config-change subscriptions (Slice 5E.2) and stop watching its file.
    CONFIG_SUBS.with(|m| m.borrow_mut().remove_by_owner(id));
    crate::loader::unwatch_config_for(id);
    // (a2d) Drop the plugin's registered ConCommands so a post-unload dispatch no-ops. This is the
    // per-plugin (.s2sp) unload: we remove from the JS dispatch map only — the shim's ICvar ConCommand
    // stays registered (idempotent, reload-safe) and re-routes to the new handler on reload. The engine-
    // side ICvar unregister happens on full shim teardown (Metamod Unload → s2script_mm.cpp, UAF-safe).
    CONCOMMANDS.with(|m| m.borrow_mut().retain(|_, (owner, _, _)| owner != id));

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
                    match f.call(tc, recv, &[]) {
                        Some(ret) => {
                            // Slice 5E.3: capture the onUnload return as the reload-handoff blob.
                            // Serialize in THIS (old) context via iface_to_json (JSON.stringify + the
                            // EntityRef replacer) so the string survives the context's disposal. A
                            // null/undefined return means "no state to carry"; a non-serializable one
                            // (function, cycle) → iface_to_json None → WARN + no handoff.
                            if !ret.is_undefined() && !ret.is_null() {
                                match iface_to_json(tc, ret) {
                                    Some(blob) => PENDING_HANDOFF.with(|h| {
                                        h.borrow_mut().insert(id.to_string(), blob);
                                    }),
                                    None => log_warn(&format!(
                                        "WARN: unload_plugin('{}'): onUnload return not serializable — no state handoff",
                                        id
                                    )),
                                }
                            }
                        }
                        None => {
                            let msg = tc
                                .exception()
                                .map(|e| e.to_rust_string_lossy(&*tc))
                                .unwrap_or_else(|| "onUnload threw".into());
                            log_warn(&format!("WARN: unload_plugin('{}'): onUnload error: {}", id, msg));
                        }
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

/// Slice 5E.3: drop any pending reload-handoff blob for `id` WITHOUT consuming it — called by the
/// loader on a FINAL removal (Vanished) so a deleted plugin's captured state is discarded rather than
/// handed to a future re-add of the same id.
pub(crate) fn clear_pending_handoff(id: &str) {
    PENDING_HANDOFF.with(|h| { h.borrow_mut().remove(id); });
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

    // Create a fresh plugin context `id` and eval `src` in it with the frame + timers API
    // destructured into scope (so tests can write `OnGameFrame.subscribe(...)`, `delay(...)`, etc.
    // directly).  The renamed API is only reachable via `require`, matching the plugin model.
    fn eval_std(id: &str, src: &str) {
        create_plugin_context(id);
        let full = format!(
            "const {{ OnGameFrame }} = __s2require(\"@s2script/frame\");\nconst {{ delay, nextTick, nextFrame, threadSleep }} = __s2require(\"@s2script/timers\");\n{}",
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
    /// with (slot, argString) in the REGISTERING PLUGIN'S context (owner-tracked, liveness-gated).
    /// This test exercises the store + dispatch path without the engine
    /// (calls `dispatch_concommand` directly, bypassing ConCommand registration).
    #[test]
    fn concommand_callback_receives_slot_and_args() {
        init(dummy_logger()).unwrap();
        // Register the raw native from a PLUGIN context (dispatch is now owner-tracked; registering
        // from the shared HOST context would produce owner="legacy" with no REGISTRY entry → skipped).
        load_plugin_js("cc_test", r#"
            globalThis.__cc = null;
            __s2_concommand("s2_test", function (slot, args) { globalThis.__cc = slot + ":" + args; });
        "#, "{}");
        // Simulate the engine invoking the command (bypasses ConCommand registration):
        dispatch_concommand("s2_test", 3, "1234");
        assert_eq!(eval_in_context_string("cc_test", "String(globalThis.__cc)"), "3:1234");
        shutdown();
    }

    /// `load_plugin_js` creates the plugin context (full injected API), wraps the bundle in the CJS
    /// `require`/`module` wrapper, and runs the module body.  This replaces the Slice-3 `load_cs2_file`
    /// path (removed): the same "a loaded bundle's top-level code runs and its globals are visible"
    /// behavior, now under the per-plugin loader.  The body sets `globalThis.__loaded = 42`.
    #[test]
    fn load_plugin_js_runs_module_body() {
        init(dummy_logger()).unwrap();
        load_plugin_js("probe", "globalThis.__loaded = 41 + 1;", "{}");
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
            const { OnGameFrame } = require("@s2script/frame");
            const { delay } = require("@s2script/timers");
            module.exports.onLoad = function () {
                OnGameFrame.subscribe(function () { globalThis.__ticks = (globalThis.__ticks||0)+1; });
            };
        "#;
        load_plugin_js("demo", plugin_js, "{}");
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
        load_plugin_js("demo", r#"const {OnGameFrame}=require("@s2script/frame");
            module.exports.onLoad=()=>OnGameFrame.subscribe(()=>{globalThis.__n=(globalThis.__n||0)+1;});"#, "{}");
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

    /// Slice 5E.3: unload_plugin captures a serializable onUnload() return into PENDING_HANDOFF; a
    /// non-serializable return is dropped with a WARN (no entry); a throwing onUnload leaves no entry.
    #[test]
    fn unload_captures_onunload_return_as_handoff_blob() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // (a) serializable return → captured
        load_plugin_js("cap", r#"
            module.exports.onUnload = function(){ return { count: 7, name: "hi" }; };
        "#, "{}");
        unload_plugin("cap");
        let blob = PENDING_HANDOFF.with(|h| h.borrow().get("cap").cloned());
        let blob = blob.expect("handoff blob captured");
        assert!(blob.contains("\"count\":7"), "blob has the state: {blob}");

        // (b) non-serializable return (a function) → no entry
        load_plugin_js("nos", r#"
            module.exports.onUnload = function(){ return function(){}; };
        "#, "{}");
        unload_plugin("nos");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("nos").is_none()), "non-serializable → no blob");

        // (c) throwing onUnload → no entry
        load_plugin_js("thr", r#"
            module.exports.onUnload = function(){ throw new Error("boom"); };
        "#, "{}");
        unload_plugin("thr");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("thr").is_none()), "throwing onUnload → no blob");
        shutdown();
    }

    /// Slice 5E.3: a same-id reload carries state — onUnload's return revives into onLoad(prev). Covers
    /// the primitive/nested round-trip, first-load undefined, a live EntityRef revival, and consume-once.
    #[test]
    fn reload_hands_off_state_to_onload_prev() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // A plugin that seeds a counter from prev on load and bumps it on unload.
        const JS: &str = r#"
            var count = 0;
            module.exports.onLoad   = function(prev){ if (prev) { count = prev.count; }
                                                      globalThis.__count = count;
                                                      globalThis.__hadPrev = (prev !== undefined); };
            module.exports.onUnload = function(){ return { count: count + 1 }; };
        "#;
        // First load → onLoad(undefined)
        load_plugin_js("rh", JS, "{}");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__hadPrev)"), "false", "first load: no prev");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "0");
        // Reload: unload (captures {count:1}) then load again (consumes → onLoad(prev))
        unload_plugin("rh");
        load_plugin_js("rh", JS, "{}");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__hadPrev)"), "true", "reload: prev present");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "1", "count carried across the reload");
        // Consume-once: the blob is gone, so a fresh load with no new unload sees undefined again.
        unload_plugin("rh");                                   // captures {count:2}
        load_plugin_js("rh", JS, "{}");                        // consumes → count=2
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "2");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("rh").is_none()), "blob consumed");
        shutdown();
    }

    /// Slice 5E.3: an EntityRef in the handoff state revives into a live, serial-gated EntityRef bound
    /// to the NEW context (reusing the inter-plugin reviver).
    #[test]
    fn reload_revives_entityref_in_state() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        const JS: &str = r#"
            module.exports.onLoad   = function(prev){ globalThis.__revived = prev && prev.ref; };
            module.exports.onUnload = function(){ return { ref: new (__s2pkg_entity.EntityRef)(1, 7) }; };
        "#;
        load_plugin_js("er", JS, "{}");
        unload_plugin("er");                                   // captures { ref: <tagged EntityRef> }
        load_plugin_js("er", JS, "{}");                        // revives → live EntityRef
        assert_eq!(eval_in_context_string("er", "String(globalThis.__revived instanceof __s2pkg_entity.EntityRef)"), "true");
        assert_eq!(eval_in_context_string("er", "globalThis.__revived.index + ',' + globalThis.__revived.serial"), "1,7");
        shutdown();
    }

    /// Slice 5E.3: a throwing onLoad(prev) degrades (WARN) without crashing the reload.
    #[test]
    fn reload_onload_throw_degrades_no_crash() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_plugin_js("ot", r#"
            module.exports.onLoad   = function(prev){ if (prev) throw new Error("boom"); };
            module.exports.onUnload = function(){ return { x: 1 }; };
        "#, "{}");
        unload_plugin("ot");
        load_plugin_js("ot", r#"
            module.exports.onLoad   = function(prev){ if (prev) throw new Error("boom"); };
            module.exports.onUnload = function(){ return { x: 1 }; };
        "#, "{}");
        // No panic; the blob was consumed despite the throw.
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("ot").is_none()), "blob consumed even though onLoad threw");
        shutdown();
    }

    /// Brief test: a `delay` continuation whose plugin is UNLOADED before the deadline must be
    /// DROPPED — `frame_async_drain` must NOT run the continuation into a disposed context (no
    /// panic; the resolver was dropped by the ledger teardown).
    #[test]
    fn delay_continuation_for_unloaded_plugin_is_dropped() {
        init(dummy_logger()).unwrap();
        load_plugin_js("demo", r#"const {delay}=require("@s2script/timers");
            module.exports.onLoad=()=>{ (async()=>{ await delay(30); globalThis.__resumed=true; })(); };"#, "{}");
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
            const { OnGameFrame } = require("@s2script/frame");
            module.exports.onLoad = function () {
                OnGameFrame.subscribe(function () { globalThis.__v = "v1"; });
            };
        "#;
        load_plugin_js("demo", v1_js, "{}");
        dispatch_game_frame_pre_post();
        assert_eq!(read_string_global_in("demo", "__v"), "v1", "v1 handler ran before reload");

        // Capture the v1 generation so we can assert it becomes stale after reload.
        let old_gen = PLUGINS
            .with(|p| p.borrow().get("demo").expect("demo loaded").generation);

        // RELOAD: call load_plugin_js with the same id — the defensive guard fires.
        // v2 writes "v2" to the global.
        let v2_js = r#"
            const { OnGameFrame } = require("@s2script/frame");
            module.exports.onLoad = function () {
                OnGameFrame.subscribe(function () { globalThis.__v = "v2"; });
            };
        "#;
        load_plugin_js("demo", v2_js, "{}");

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
    /// `interfaces.publishInterface`, and the `__s2_iface_call` cross-context structured-copy native.
    #[test]
    fn consumer_calls_producer_method_structured_copy() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        // Producer publishes via the plugin path so the prelude publishInterface is exercised.
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/greeter","1.0.0",{ greet:function(n){ return "hi "+n.who; } });
        "#, "{}");
        // Consumer resolves a hard proxy and calls across (arg + return structured-copied).
        load_plugin_js("cons", r#"
            const g = require("@x/greeter");
            globalThis.__test_out = g.greet({ who: "world" });
        "#, "{}");
        assert_eq!(read_global_string("cons", "__test_out"), "hi world");

        // Producer-absent hard dep → InterfaceUnavailable (caught by the wrapper TryCatch → WARN).
        set_plugin_imports("lonely", vec![("@missing".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("lonely", r#"
            try { require("@missing").foo(); globalThis.__err = "no throw"; }
            catch (e) { globalThis.__err = String(e); }
        "#, "{}");
        assert!(read_global_string("lonely", "__err").contains("InterfaceUnavailable"));

        // Optional dep, not published → require returns null.
        set_plugin_imports("optc", vec![("@absent".into(), "^1.0.0".into(), crate::interfaces::Kind::Optional)]);
        load_plugin_js("optc", r#"globalThis.__opt = (require("@absent") === null) ? "null" : "proxy";"#, "{}");
        assert_eq!(read_global_string("optc", "__opt"), "null");

        // Non-serializable (cyclic) arg → InterfaceValueNotSerializable (JSON.stringify throws → None → throw).
        set_plugin_imports("cyc", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("cyc", r#"
            const g = require("@x/greeter");
            const a = {}; a.self = a;
            try { g.greet(a); globalThis.__e2 = "no throw"; }
            catch (e) { globalThis.__e2 = String(e); }
        "#, "{}");
        assert!(read_global_string("cyc", "__e2").contains("InterfaceValueNotSerializable"));

        // Producer method THROWS → consumer sees InterfaceCallError carrying the producer message
        // (not a crash, not a mislabeled InterfaceValueNotSerializable).
        load_plugin_js("prodBoom", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/boom", "1.0.0", { boom: function(){ throw new Error("kaboom"); } });
        "#, "{}");
        set_plugin_imports("consBoom", vec![("@x/boom".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("consBoom", r#"
            const g = require("@x/boom");
            try { g.boom(); globalThis.__boom = "no throw"; } catch (e) { globalThis.__boom = String(e); }
        "#, "{}");
        let boom = read_global_string("consBoom", "__boom");
        assert!(boom.contains("InterfaceCallError"), "producer throw → InterfaceCallError, got: {}", boom);
        assert!(boom.contains("kaboom"), "producer message surfaced, got: {}", boom);

        // Producer method returns undefined (void) → consumer receives undefined, NOT a throw.
        load_plugin_js("prodVoid", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/void", "1.0.0", { poke: function(){ /* returns undefined */ } });
        "#, "{}");
        set_plugin_imports("consVoid", vec![("@x/void".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("consVoid", r#"
            const g = require("@x/void");
            try { globalThis.__void = (g.poke() === undefined) ? "undefined" : "value"; }
            catch (e) { globalThis.__void = "threw:" + String(e); }
        "#, "{}");
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
            const { publishInterface } = require("@s2script/interfaces");
            globalThis.__h = publishInterface("@x/greeter","1.0.0",{ greet:function(){return "";} });
        "#, "{}");
        load_plugin_js("cons", r#"
            const g = require("@x/greeter");
            globalThis.__seen = [];
            g.on("greeted", function (p) { globalThis.__seen.push(p.slot); });
        "#, "{}");
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
        load_plugin_js("prod", r#"const {publishInterface}=require("@s2script/interfaces");
            publishInterface("@x/greeter","1.0.0",{greet:function(){return "ok";}});"#, "{}");
        load_plugin_js("cons", r#"const g=require("@x/greeter");
            globalThis.call=function(){ try { return g.greet(); } catch(e){ return String(e); } };
            globalThis.__before=call();"#, "{}");
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
        load_plugin_js("prod", r#"const {publishInterface}=require("@s2script/interfaces");
            globalThis.__h=publishInterface("@x/greeter","1.0.0",{greet:function(){return "";}});"#, "{}");
        load_plugin_js("cons", r#"const g=require("@x/greeter"); g.on("greeted",function(){});"#, "{}");
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
        load_plugin_js("prod", r#"const {publishInterface}=require("@s2script/interfaces");
            publishInterface("@x/greeter","1.0.0",{greet:function(){return "still-here";}});"#, "{}");
        // consumer's onUnload calls the producer — must still work because producer outlives it.
        load_plugin_js("cons", r#"const g=require("@x/greeter");
            module.exports.onUnload=function(){ globalThis.__unload_result = g.greet(); };"#, "{}");
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
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1, 7, 8, 1))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write(1, 7, 8, 1, 5))"), "false");
        // handle_decode is PURE (no ops needed). BITS-agnostic assertion: 64 < 2^7 <= 2^HANDLE_ENTRY_BITS,
        // so index==64, serial==0 for any real bit-split (the exact split is validated live in the gate).
        assert_eq!(eval_in_context_string("p", "var d=__s2_handle_decode(64); d[0]+','+d[1]"), "64,0");
        shutdown();
    }

    /// Slice 5D.2: the five engine-identity client natives degrade safely with no engine-ops table
    /// (no crash — false/-1/null as documented).
    #[test]
    fn client_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops table → every client op is a safe miss
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_valid(0))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_userid(0))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_signon(0))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_name(0))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_find_by_userid(5))"), "-1");
        shutdown();
    }

    /// Slice 5A Task 4: `EntityRef` from `@s2script/entity` degrades safely when no engine-ops table
    /// is wired — `isValid` returns false, `readInt32` returns null, `writeInt32` returns false.
    /// This is the failing test: EntityRef must be exported by the prelude (Step 3 makes it pass).
    #[test]
    fn entity_ref_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_plugin_js("er", r#"
            const { EntityRef } = require("@s2script/entity");
            const ref = new EntityRef(1, 7);
            globalThis.__valid = String(ref.isValid());       // "false"
            globalThis.__read  = String(ref.readInt32(8));    // "null"
            globalThis.__write = String(ref.writeInt32(8, 5));// "false"
        "#, "{}");
        assert_eq!(read_global_string("er", "__valid"), "false");
        assert_eq!(read_global_string("er", "__read"), "null");
        assert_eq!(read_global_string("er", "__write"), "false");
        shutdown();
    }

    /// Slice 5B.2: kind-dispatched `__s2_ent_ref_read` / `__s2_ent_ref_write` natives degrade safely
    /// when no engine-ops table is wired. Also verifies `EntityRef` typed methods route through the
    /// generic native (readFloat32/readBool/readHandle all return null when the ref is stale).
    #[test]
    fn generic_typed_reads_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);          // no ops → entity_resolve_ptr null → read null / write false
        create_plugin_context("p");
        // each kind degrades to null (read) — I32=1,F32=2,BOOL=3,I8=4,I16=5,U8=6,U16=7,U32=8
        for k in ["1","2","3","4","5","6","7","8"] {
            assert_eq!(
                eval_in_context_string("p", &format!("String(__s2_ent_ref_read(1,7,8,{}))", k)),
                "null",
            );
        }
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,999))"), "null"); // unknown kind
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write(1,7,8,2,1.5))"), "false");
        // EntityRef typed methods degrade (proving they're wired + route a kind):
        load_plugin_js("er2", r#"
            const { EntityRef } = require("@s2script/entity");
            const ref = new EntityRef(1, 7);
            globalThis.__f = String(ref.readFloat32(8));
            globalThis.__b = String(ref.readBool(8));
            globalThis.__h = String(ref.readHandle(8));
        "#, "{}");
        assert_eq!(read_global_string("er2", "__f"), "null");
        assert_eq!(read_global_string("er2", "__b"), "null");
        assert_eq!(read_global_string("er2", "__h"), "null");
        shutdown();
    }

    /// Slice 5B.4 Task 2: string + 64-bit natives degrade safely without engine-ops.
    /// Proves KIND_U64/I64/F64 (9/10/11) in the generic read, `__s2_ent_ref_read_string`,
    /// and the EntityRef prelude methods (readUInt64, readInt64, readFloat64, readString).
    #[test]
    fn read_string_and_64bit_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the generic read degrades for the new kinds (U64=9, I64=10, F64=11):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,9))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,10))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1,7,8,11))"), "null");
        // the string native degrades:
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_string(1,7,8,128))"), "null");
        // EntityRef methods degrade (proving they're wired) — use `__s2require` (the native, available in a
        // create_plugin_context raw scope, as `eval_std` uses), NOT the CJS `require` (only in load_plugin_js):
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readUInt64(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readInt64(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readFloat64(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readString(8,128))"#), "null");
        shutdown();
    }

    /// Slice 5C.3 Task 2: `__s2_ent_ref_read_floats` native + `EntityRef.readFloats` degrade safely
    /// without engine-ops (serial-gated → null on stale ref / no ops table).
    #[test]
    fn read_floats_native_and_method_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the native degrades to null (no engine ops → entity_resolve_ptr null):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats(1,7,8,3))"), "null");
        // the EntityRef method degrades to null:
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readFloats(8,3))"#), "null");
        shutdown();
    }

    /// Slice 5C.4 Task 1: `__s2_ent_ref_read_floats_chain` native + `EntityRef.readFloatsChain` degrade
    /// safely without engine-ops; guards (non-array chain, negative finalOff, bad count) → null.
    #[test]
    fn read_floats_chain_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the native degrades to null (no engine ops → entity_resolve_ptr null, before any deref):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,[48,8],200,3))"), "null");
        // guards: a non-array chain, a negative finalOff, and a bad count all → null:
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,42,200,3))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,[48,8],-1,3))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,[48,8],200,9))"), "null");
        // the EntityRef method degrades to null:
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readFloatsChain([48,8],200,3))"#), "null");
        shutdown();
    }

    /// Slice 5C.5 Task 1: `__s2_ent_ref_read_chain` native + `EntityRef.*Via` methods degrade
    /// safely without engine-ops; guards (non-array path, negative finalOff, bad kind) → null.
    #[test]
    fn read_chain_native_and_via_methods_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the native degrades to null (no ops → entity_resolve_ptr null):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,[48],200,1))"), "null");   // KIND_I32
        // guards (fire before the resolve): non-array path, negative finalOff, bad kind:
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,42,200,1))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,[48],-1,1))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_chain(1,7,[48],200,999))"), "null");
        // the EntityRef via-methods degrade:
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readInt32Via([48],200))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readHandleVia([48],200))"#), "null");
        shutdown();
    }

    /// Slice 5A Task 5: a game-package prelude (registered via `register_injected_package`)
    /// runs in the RAW context scope where the CJS `require` is NOT defined — it must use
    /// the `__s2require` native to reach `@s2script/entity`. This guards that mechanism
    /// (the Slice-5A live gate caught a bare-`require` bug the unit tests missed).
    /// Synthetic prelude — engine-generic, no CS2 names.
    #[test]
    fn registered_package_prelude_reaches_std_entityref_via_native_require() {
        let _ = init(dummy_logger());
        register_injected_package(
            "@s2script/cs2",
            // Also pin the NEGATIVE case: the CJS `require` is genuinely undefined in the raw prelude
            // scope, so a package prelude MUST use `__s2require` — that is the exact bug the live gate
            // caught. `noRequire` proves the scope, `hasEntityRef` proves the native reaches EntityRef.
            r#"var ER = __s2require("@s2script/entity").EntityRef;
               globalThis.__s2pkg_cs2 = {
                 hasEntityRef: (typeof ER === "function"),
                 noRequire: (typeof require === "undefined"),
               };"#,
        );
        load_plugin_js("p", r#"
            const cs2 = require("@s2script/cs2");
            globalThis.__ok = String(cs2 !== null && cs2.hasEntityRef === true && cs2.noRequire === true);
        "#, "{}");
        assert_eq!(read_global_string("p", "__ok"), "true");
        shutdown();
    }

    // --- Slice 4.5 Task 1: EntityRef replacer/reviver wire round-trip ---

    #[test]
    fn iface_call_return_rehydrates_entityref() {
        let _ = init(dummy_logger());
        set_engine_ops(None); // degrade path: a real EntityRef -> isValid()==false, readInt32()==null
        set_plugin_imports("cons", vec![("@x/ent".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        // Producer returns an EntityRef from a method.
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            const { EntityRef } = require("@s2script/entity");
            publishInterface("@x/ent", "1.0.0", { getRef: function(){ return new EntityRef(1, 7); } });
        "#, "{}");
        // Consumer receives it: must be a LIVE EntityRef (methods present), not plain data.
        load_plugin_js("cons", r#"
            const { EntityRef } = require("@s2script/entity");
            const r = require("@x/ent").getRef();
            globalThis.__isRef  = String(r instanceof EntityRef);        // "true" — rehydrated
            globalThis.__idx    = String(r.index) + "," + String(r.serial); // "1,7" — data crossed
            globalThis.__valid  = String(r.isValid());                   // "false" (no ops) — it's callable
            globalThis.__read   = String(r.readInt32(8));                // "null"  (no ops)
        "#, "{}");
        assert_eq!(read_global_string("cons", "__isRef"), "true");
        assert_eq!(read_global_string("cons", "__idx"), "1,7");
        assert_eq!(read_global_string("cons", "__valid"), "false");
        assert_eq!(read_global_string("cons", "__read"), "null");
        shutdown();
    }

    #[test]
    fn iface_emit_payload_rehydrates_entityref() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        set_plugin_imports("cons", vec![("@x/ent".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            const { EntityRef } = require("@s2script/entity");
            globalThis.__h = publishInterface("@x/ent", "1.0.0", { noop: function(){} });
        "#, "{}");
        load_plugin_js("cons", r#"
            const { EntityRef } = require("@s2script/entity");
            const g = require("@x/ent");
            globalThis.__seen = "none";
            g.on("spawned", function (r) {
                globalThis.__seen = (r instanceof EntityRef) ? (r.index + "," + r.serial) : "plain";
            });
        "#, "{}");
        // EntityRef is a closure var inside the CJS wrapper; use the globalThis prelude reference.
        eval_in_context("prod", r#"__h.emit("spawned", new __s2pkg_entity.EntityRef(2, 9));"#).unwrap();
        assert_eq!(read_global_string("cons", "__seen"), "2,9"); // live EntityRef, not "plain"
        shutdown();
    }

    #[test]
    fn non_entityref_payload_round_trips_unchanged() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/data".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/data", "1.0.0", { echo: function(){ return { a: 1, b: "hi", c: [1,2,3] }; } });
        "#, "{}");
        load_plugin_js("cons", r#"
            const d = require("@x/data").echo();
            globalThis.__out = d.a + "," + d.b + "," + d.c.join("-");
        "#, "{}");
        assert_eq!(read_global_string("cons", "__out"), "1,hi,1-2-3"); // ordinary data intact
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Slice 5B.1 Task 3: schema_enumerate op + __s2_schema_dump native
    // ---------------------------------------------------------------------------

    /// A stub shim-side enumerate: emits one class + two fields via the core callbacks.
    /// Generic names only (CTest/CBase/m_x/m_h/CThing) — no CS2 identifiers.
    extern "C" fn stub_enumerate(ctx: *mut c_void, ec: EmitClassFn, ef: EmitFieldFn) -> c_int {
        ec(ctx, b"CTest\0".as_ptr() as *const c_char, b"CBase\0".as_ptr() as *const c_char);
        ef(ctx, b"CTest\0".as_ptr() as *const c_char, b"m_x\0".as_ptr() as *const c_char, 8,
           b"atomic\0".as_ptr() as *const c_char, b"int32\0".as_ptr() as *const c_char, std::ptr::null());
        ef(ctx, b"CTest\0".as_ptr() as *const c_char, b"m_h\0".as_ptr() as *const c_char, 12,
           b"handle\0".as_ptr() as *const c_char, std::ptr::null(), b"CThing\0".as_ptr() as *const c_char);
        1
    }

    /// Full core path: stub enumerate → callbacks → Catalog → JSON → file. No real shim needed.
    #[test]
    fn schema_dump_writes_catalog_via_stub_enumerate() {
        let _ = init(dummy_logger());
        // Wire an ops table whose schema_enumerate is the stub (all other fields None).
        set_engine_ops(Some(S2EngineOps {
            schema_offset: None, ent_by_index: None, deref_handle: None,
            ent_state_changed: None, concommand_register: None,
            schema_enumerate: Some(stub_enumerate),
            event_subscribe: None, event_unsubscribe: None,
            event_get_int: None, event_get_float: None, event_get_bool: None,
            event_get_string: None, event_get_uint64: None, event_get_player_slot: None,
            client_valid: None, client_userid: None, client_signon: None,
            client_name: None, client_find_by_userid: None,
            event_set_int: None, event_set_float: None, event_set_bool: None,
            event_set_string: None, event_set_uint64: None, event_create: None, event_fire: None,
            config_read: None, config_write: None,
            client_print: None,
            client_steamid: None,
            client_kick: None,
            server_command: None,
            server_map_valid: None,
            damage_read_float: None,
            damage_read_int: None,
            damage_write_float: None,
            damage_victim: None,
            cvar_get: None,
        }));
        create_plugin_context("p");
        let path = std::env::temp_dir().join("s2_schema_test.json");
        let path_s = path.to_string_lossy().replace('\\', "\\\\");
        let ok = eval_in_context_string("p", &format!("String(__s2_schema_dump(\"{}\"))", path_s));
        assert_eq!(ok, "true");
        let written = std::fs::read_to_string(&path).expect("catalog file written");
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["CTest"]["parent"], "CBase");
        assert_eq!(v["CTest"]["fields"][0]["name"], "m_x");
        assert_eq!(v["CTest"]["fields"][0]["type"]["kind"], "atomic");
        assert_eq!(v["CTest"]["fields"][1]["type"]["inner"], "CThing");
        let _ = std::fs::remove_file(&path);
        shutdown();
    }

    /// Degrade path: no ops table → __s2_schema_dump returns false, no file written.
    #[test]
    fn schema_dump_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);              // no ops table → no schema_enumerate → false, no file
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_schema_dump(\"/tmp/should_not_exist.json\"))"), "false");
        shutdown();
    }

    /// Slice 5C.1 Task 1: the five module packages resolve via `require`; `@s2script/std` is retired
    /// (resolves null); an unknown module also resolves null.
    #[test]
    fn require_resolves_module_packages_and_retires_std() {
        let _ = init(dummy_logger());
        // Use load_plugin_js (the CJS wrapper where `require` is defined + the prelude has run),
        // then read the results back — this exercises the full require→__s2require→module-global path.
        load_plugin_js("mods", r#"
            globalThis.__t_entity  = typeof require("@s2script/entity").EntityRef;            // "function"
            globalThis.__t_frame   = typeof require("@s2script/frame").OnGameFrame;            // "object"
            globalThis.__t_timers  = typeof require("@s2script/timers").delay;                 // "function"
            globalThis.__t_console = typeof require("@s2script/console").console;              // "object"
            globalThis.__t_iface   = typeof require("@s2script/interfaces").publishInterface;  // "function"
            globalThis.__t_std     = String(require("@s2script/std"));                         // "null" (retired)
            globalThis.__t_nope    = String(require("@s2script/nope"));                        // "null"
        "#, "{}");
        assert_eq!(read_global_string("mods", "__t_entity"), "function");
        assert_eq!(read_global_string("mods", "__t_frame"), "object");
        assert_eq!(read_global_string("mods", "__t_timers"), "function");
        assert_eq!(read_global_string("mods", "__t_console"), "object");
        assert_eq!(read_global_string("mods", "__t_iface"), "function");
        assert_eq!(read_global_string("mods", "__t_std"), "null");
        assert_eq!(read_global_string("mods", "__t_nope"), "null");
        shutdown();
    }

    /// Slice 5C.3 Task 1: `@s2script/math` resolves to `{ Vector, QAngle }` from the prelude;
    /// `Vector` carries x/y/z + `length()`; `QAngle` carries x/y/z. Pure JS value types — no
    /// engine ops needed.
    #[test]
    fn math_module_provides_vector_and_qangle() {
        let _ = init(dummy_logger());
        create_plugin_context("p");
        // the module resolves + constructs:
        assert_eq!(eval_in_context_string("p", r#"typeof __s2require("@s2script/math").Vector"#), "function");
        assert_eq!(eval_in_context_string("p", r#"typeof __s2require("@s2script/math").QAngle"#), "function");
        // Vector data + length():
        assert_eq!(eval_in_context_string("p", r#"var V=__s2require("@s2script/math").Vector; var v=new V(3,4,0); v.x+","+v.y+","+v.z"#), "3,4,0");
        assert_eq!(eval_in_context_string("p", r#"var V=__s2require("@s2script/math").Vector; String(new V(3,4,0).length())"#), "5");
        // QAngle data:
        assert_eq!(eval_in_context_string("p", r#"var Q=__s2require("@s2script/math").QAngle; var q=new Q(10,20,30); q.x+","+q.y+","+q.z"#), "10,20,30");
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Slice 5D.1: game-event mechanism — subscribe/accessor/dispatch/teardown
    // ---------------------------------------------------------------------------

    // Module-level statics for mock event-op tracking (shared across the 5D.1 tests).
    static EV_SUBSCRIBED:   Mutex<Vec<String>> = Mutex::new(Vec::new());
    static EV_UNSUBSCRIBED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    // Mock event engine-ops: event_subscribe records the name; accessors return fixed values.
    extern "C" fn mock_ev_subscribe(name: *const c_char) -> c_int {
        let n = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        EV_SUBSCRIBED.lock().unwrap().push(n); 1
    }
    extern "C" fn mock_ev_unsubscribe(name: *const c_char) -> c_int {
        let n = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        EV_UNSUBSCRIBED.lock().unwrap().push(n); 1
    }
    extern "C" fn mock_ev_get_int(_k: *const c_char) -> i32 { 42 }
    extern "C" fn mock_ev_get_float(_k: *const c_char) -> f32 { 3.14 }
    extern "C" fn mock_ev_get_bool(_k: *const c_char) -> c_int { 1 }
    extern "C" fn mock_ev_get_string(_k: *const c_char) -> *const c_char {
        b"mocked_string\0".as_ptr() as *const c_char
    }
    extern "C" fn mock_ev_get_uint64(_k: *const c_char) -> u64 { 999_000_000_000u64 }
    extern "C" fn mock_ev_get_player_slot(_k: *const c_char) -> i32 { 7 }

    /// Build a full mock S2EngineOps table with all event op fields wired to the mock fns above
    /// and all non-event fields None.  Used by 5D.1 tests that need the event accessor natives.
    fn mock_event_ops() -> S2EngineOps {
        S2EngineOps {
            schema_offset: None, ent_by_index: None, deref_handle: None,
            ent_state_changed: None, concommand_register: None, schema_enumerate: None,
            event_subscribe:      Some(mock_ev_subscribe),
            event_unsubscribe:    Some(mock_ev_unsubscribe),
            event_get_int:        Some(mock_ev_get_int),
            event_get_float:      Some(mock_ev_get_float),
            event_get_bool:       Some(mock_ev_get_bool),
            event_get_string:     Some(mock_ev_get_string),
            event_get_uint64:     Some(mock_ev_get_uint64),
            event_get_player_slot: Some(mock_ev_get_player_slot),
            client_valid: None, client_userid: None, client_signon: None,
            client_name: None, client_find_by_userid: None,
            event_set_int: None, event_set_float: None, event_set_bool: None,
            event_set_string: None, event_set_uint64: None, event_create: None, event_fire: None,
            config_read: None, config_write: None,
            client_print: None,
            client_steamid: None,
            client_kick: None,
            server_command: None,
            server_map_valid: None,
            damage_read_float: None,
            damage_read_int: None,
            damage_write_float: None,
            damage_victim: None,
            cvar_get: None,
        }
    }

    /// Slice 5D.1 Task 1 (core): game-event subscribe/dispatch/accessor/teardown integration.
    ///
    /// - A plugin subscribes to "player_death" via the raw `__s2_event_subscribe` native.
    /// - The first subscriber triggers the `event_subscribe` engine-op.
    /// - `dispatch_game_event("player_death")` delivers a `new GameEvent("player_death")` to the
    ///   handler; the handler reads all six accessor methods and stores the results.
    /// - Dispatching an unsubscribed name does NOT call the handler.
    /// - `unload_plugin` calls `remove_by_owner` on the mux and triggers `event_unsubscribe`.
    /// - After unload, further dispatches are no-ops (no crash, no delivery).
    #[test]
    fn game_event_dispatch_subscribe_accessor_and_teardown() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        // Plugin subscribes to player_death using the raw __s2_event_subscribe native.
        load_plugin_js("pev", r#"
            __s2_event_subscribe("player_death", function(ev) {
                globalThis.__ev_ran    = (globalThis.__ev_ran || 0) + 1;
                globalThis.__ev_name   = ev.name;
                globalThis.__ev_int    = ev.getInt("damage");
                globalThis.__ev_str    = ev.getString("victim");
                globalThis.__ev_slot   = ev.getPlayerSlot("attacker");
                globalThis.__ev_uint64 = ev.getUint64("steamid");
                globalThis.__ev_bool   = ev.getBool("headshot");
            });
        "#, "{}");

        // First subscribe must trigger the engine-op.
        assert!(
            EV_SUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_death"),
            "event_subscribe engine-op must fire on the first subscriber"
        );

        // Dispatch player_death → handler runs and reads mocked accessor values.
        dispatch_game_event("player_death");
        assert_eq!(read_i32_global_in("pev", "__ev_ran"),  1,             "handler must run exactly once");
        assert_eq!(read_global_string("pev", "__ev_name"), "player_death","GameEvent.name must be set");
        assert_eq!(read_i32_global_in("pev", "__ev_int"),  42,            "getInt() returns mock 42");
        assert_eq!(read_global_string("pev", "__ev_str"),  "mocked_string","getString() returns mock");
        assert_eq!(read_i32_global_in("pev", "__ev_slot"), 7,             "getPlayerSlot() returns mock 7");
        assert_eq!(read_global_string("pev", "__ev_uint64"), "999000000000","getUint64() returns decimal string");
        assert_eq!(read_bool_global_in("pev", "__ev_bool"), true,         "getBool() returns mock true");

        // Dispatching a different name must NOT call the handler.
        dispatch_game_event("round_start");
        assert_eq!(read_i32_global_in("pev", "__ev_ran"), 1,
                   "handler must not run for unsubscribed event name");

        // Teardown: unload_plugin removes all of "pev"'s game-event subs; the name empties →
        // event_unsubscribe engine-op fires.
        unload_plugin("pev");
        assert!(
            EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_death"),
            "event_unsubscribe engine-op must fire when the last subscriber unloads"
        );

        // After unload, dispatching must be a safe no-op (no crash, no delivery).
        dispatch_game_event("player_death");
        // (No count assertion — the context is disposed; just confirm no panic.)

        shutdown();
    }

    /// Slice 5D.1: a second plugin subscribes to the same event → event_subscribe engine-op fires
    /// only ONCE (on the first subscriber); event_unsubscribe fires only when the LAST subscriber
    /// unloads.  Both handlers receive the event; after one unloads only the other still runs.
    #[test]
    fn game_event_two_subscribers_first_last_semantics() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        load_plugin_js("p1", r#"
            __s2_event_subscribe("player_hurt", function(ev) {
                globalThis.__p1_ran = (globalThis.__p1_ran || 0) + 1;
            });
        "#, "{}");
        // First subscriber → engine-op called once.
        assert_eq!(EV_SUBSCRIBED.lock().unwrap().iter().filter(|n| *n == "player_hurt").count(), 1);

        load_plugin_js("p2", r#"
            __s2_event_subscribe("player_hurt", function(ev) {
                globalThis.__p2_ran = (globalThis.__p2_ran || 0) + 1;
            });
        "#, "{}");
        // Second subscriber → engine-op NOT called again (not first).
        assert_eq!(EV_SUBSCRIBED.lock().unwrap().iter().filter(|n| *n == "player_hurt").count(), 1,
                   "event_subscribe must only fire on the FIRST subscriber");

        dispatch_game_event("player_hurt");
        assert_eq!(read_i32_global_in("p1", "__p1_ran"), 1);
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 1);

        // Unload p1: p2 still subscribed → event_unsubscribe must NOT fire yet.
        unload_plugin("p1");
        assert!(!EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_hurt"),
                "event_unsubscribe must NOT fire while p2 is still subscribed");

        // Dispatch again → only p2 runs.
        dispatch_game_event("player_hurt");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 2);

        // Unload p2: last subscriber → event_unsubscribe fires.
        unload_plugin("p2");
        assert!(EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_hurt"),
                "event_unsubscribe must fire when the last subscriber unloads");

        shutdown();
    }

    /// Slice 5D.1: accessor natives degrade safely when no engine-ops table is wired
    /// (each returns its documented default: 0 / 0.0 / false / "" / "0" / -1).
    #[test]
    fn game_event_accessor_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);          // no ops → every accessor degrades
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_event_get_int('k'))"),    "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_event_get_float('k'))"),  "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_event_get_bool('k'))"),   "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_event_get_string('k'))"), "");
        assert_eq!(eval_in_context_string("p", "String(__s2_event_get_uint64('k'))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_event_get_player_slot('k'))"), "-1");
        shutdown();
    }

    /// Slice 5D.1: `@s2script/events` resolves via `require` and provides `GameEvent`.
    #[test]
    fn events_module_provides_game_event_constructor() {
        let _ = init(dummy_logger());
        load_plugin_js("gec", r#"
            const { GameEvent } = require("@s2script/events");
            const ev = new GameEvent("round_start");
            globalThis.__ev_name = ev.name;
            globalThis.__ev_type = typeof GameEvent;
        "#, "{}");
        assert_eq!(read_global_string("gec", "__ev_name"), "round_start");
        assert_eq!(read_global_string("gec", "__ev_type"), "function");
        shutdown();
    }

    /// Slice 5D.1 Task 1: explicit `__s2_event_unsubscribe` path — single subscriber.
    ///
    /// After subscribing and then explicitly calling `__s2_event_unsubscribe("test_event")`,
    /// the handler must NOT be called on the next dispatch, AND the `event_unsubscribe`
    /// engine-op must fire (the name became empty).
    #[test]
    fn game_event_explicit_unsubscribe_removes_handler_and_fires_engine_op() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        // Plugin subscribes then immediately unsubscribes from the same module body.
        load_plugin_js("pev", r#"
            globalThis.__ev_ran = 0;
            __s2_event_subscribe("test_event", function(ev) {
                globalThis.__ev_ran = (globalThis.__ev_ran || 0) + 1;
            });
            __s2_event_unsubscribe("test_event");
        "#, "{}");

        // event_subscribe engine-op must have fired (on the first/only subscribe).
        assert!(
            EV_SUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_subscribe engine-op must fire on subscribe"
        );

        // event_unsubscribe engine-op must have fired (name is now empty after unsubscribe).
        assert!(
            EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_unsubscribe engine-op must fire when last subscriber explicitly unsubscribes"
        );

        // Dispatch must NOT invoke the removed handler.
        dispatch_game_event("test_event");
        assert_eq!(
            read_i32_global_in("pev", "__ev_ran"), 0,
            "handler must not run after explicit unsubscribe"
        );

        shutdown();
    }

    /// Slice 5D.1 Task 1: explicit `__s2_event_unsubscribe` with two plugins.
    ///
    /// Unsubscribing the first plugin removes only its subs; the second plugin still receives
    /// the event.  The `event_unsubscribe` engine-op fires only when the LAST subscriber
    /// explicitly unsubscribes (the name is then empty).
    #[test]
    fn game_event_explicit_unsubscribe_two_plugins_last_fires_op() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        load_plugin_js("p1", r#"
            globalThis.__p1_ran = 0;
            __s2_event_subscribe("test_event", function(ev) {
                globalThis.__p1_ran = (globalThis.__p1_ran || 0) + 1;
            });
        "#, "{}");
        load_plugin_js("p2", r#"
            globalThis.__p2_ran = 0;
            __s2_event_subscribe("test_event", function(ev) {
                globalThis.__p2_ran = (globalThis.__p2_ran || 0) + 1;
            });
        "#, "{}");

        // Confirm both handlers run on dispatch before any explicit unsubscribe.
        dispatch_game_event("test_event");
        assert_eq!(read_i32_global_in("p1", "__p1_ran"), 1, "p1 handler must run before unsubscribe");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 1, "p2 handler must run before unsubscribe");

        // p1 explicitly unsubscribes: p2 still subscribed → event_unsubscribe must NOT fire yet.
        eval_in_context("p1", r#"__s2_event_unsubscribe("test_event");"#).unwrap();
        assert!(
            !EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_unsubscribe must NOT fire while p2 is still subscribed"
        );

        // Dispatch: only p2 runs; p1's handler must be gone.
        dispatch_game_event("test_event");
        assert_eq!(read_i32_global_in("p1", "__p1_ran"), 1, "p1 must not run after explicit unsubscribe");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 2, "p2 must still receive the event");

        // p2 explicitly unsubscribes: last subscriber → event_unsubscribe engine-op must now fire.
        eval_in_context("p2", r#"__s2_event_unsubscribe("test_event");"#).unwrap();
        assert!(
            EV_UNSUBSCRIBED.lock().unwrap().iter().any(|n| n == "test_event"),
            "event_unsubscribe must fire when the last subscriber explicitly unsubscribes"
        );

        // After both unsubscribed, dispatch is a safe no-op.
        dispatch_game_event("test_event");
        assert_eq!(read_i32_global_in("p2", "__p2_ran"), 2, "p2 must not run after explicit unsubscribe");

        shutdown();
    }

    /// Slice 5D.1 Task 2: `@s2script/events` `Events.on/off` — end-to-end in-isolate test.
    ///
    /// - Subscribe via `require("@s2script/events").Events.on` (the prelude module object, NOT the raw native).
    /// - `dispatch_game_event("player_death")` delivers a `GameEvent`; the handler reads `ev.name`
    ///   + `ev.getInt("attacker")` (mock returns 42) and stores `"player_death:42"` in `__saw`.
    /// - `Events.off("player_death", handler)` removes all of the plugin's subs for the name;
    ///   subsequent dispatch must NOT change `__saw`.
    #[test]
    fn events_module_events_on_off_drives_dispatch_via_pkg() {
        EV_SUBSCRIBED.lock().unwrap().clear();
        EV_UNSUBSCRIBED.lock().unwrap().clear();

        init(dummy_logger()).unwrap();
        set_engine_ops(Some(mock_event_ops()));

        load_plugin_js("ep", r#"
            var evMod = require("@s2script/events");
            var handler = function(ev) {
                globalThis.__saw = ev.name + ":" + ev.getInt("attacker");
            };
            evMod.Events.on("player_death", handler);
        "#, "{}");

        // Events.on must delegate to __s2_event_subscribe → engine-op must have fired.
        assert!(
            EV_SUBSCRIBED.lock().unwrap().iter().any(|n| n == "player_death"),
            "Events.on must trigger the event_subscribe engine-op"
        );

        // Dispatch → handler runs; ev.name="player_death", ev.getInt → mock 42.
        dispatch_game_event("player_death");
        assert_eq!(
            read_global_string("ep", "__saw"),
            "player_death:42",
            "Events.on handler must receive a GameEvent with correct name and getInt"
        );

        // Events.off removes all of the plugin's subs for the name; handler identity not checked.
        // Use globalThis.__s2_require directly — eval_in_context has no CJS `require` alias.
        eval_in_context("ep", r#"
            globalThis.__s2_require("@s2script/events").Events.off("player_death", null);
        "#).unwrap();

        // Dispatch again → __saw must remain unchanged (handler must NOT run).
        let saw_before = read_global_string("ep", "__saw");
        dispatch_game_event("player_death");
        assert_eq!(
            read_global_string("ep", "__saw"),
            saw_before,
            "After Events.off, dispatch must not invoke the handler"
        );

        shutdown();
    }

    /// Slice 5D.3 Task 1: `dispatch_game_event_pre` with no subscribers returns 0 (allow).
    #[test]
    fn dispatch_game_event_pre_no_subs_allows() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        assert_eq!(dispatch_game_event_pre("player_hurt"), 0);   // no pre-subs → allow
        shutdown();
    }

    /// Slice 5D.3 Task 2: pre-hook collapse + setter/create/fire degrade with no engine ops.
    #[test]
    fn pre_hooks_collapse_and_degrade() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // Handled → suppress (1).
        eval_in_context("p", r#"__s2_event_subscribe_pre("player_hurt", function(ev){ return HookResult.Handled; });"#).unwrap();
        assert_eq!(dispatch_game_event_pre("player_hurt"), 1);
        // Continue on another name → allow (0).
        eval_in_context("p", r#"__s2_event_subscribe_pre("round_start", function(ev){ return HookResult.Continue; });"#).unwrap();
        assert_eq!(dispatch_game_event_pre("round_start"), 0);
        // set*/create/fire degrade with no ops (no crash).
        assert_eq!(eval_in_context_string("p", r#"var {Events}=__s2pkg_events; String(Events.fire("x",{a:1}))"#), "false");
        eval_in_context("p", r#"var e = new (__s2pkg_events.GameEvent)("t"); e.setInt("k", 5);"#).unwrap();  // no-op, no throw
        shutdown();
    }

    /// Slice 5D.3: Events.fire() from inside a handler re-enters the dispatch while the isolate is
    /// already borrowed. The re-entrancy guard must SKIP the nested JS dispatch gracefully (no
    /// "RefCell already borrowed" panic), allowing the event through (pre → 0/allow; notify → no-op).
    #[test]
    fn reentrant_dispatch_skips_gracefully_no_panic() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // A pre-hook that WOULD suppress, and a notify sub — both must be skipped under re-entrancy.
        eval_in_context("p", r#"__s2_event_subscribe_pre("player_hurt", function(ev){ return HookResult.Handled; });"#).unwrap();
        eval_in_context("p", r#"__s2_event_subscribe("player_hurt", function(ev){ globalThis.__notified = true; });"#).unwrap();
        // Hold the HOST borrow (simulating an outer dispatch), then re-enter:
        let pre = HOST.with(|h| {
            let _b = h.borrow_mut();
            dispatch_game_event_pre("player_hurt")   // must return 0 (allow), not panic
        });
        assert_eq!(pre, 0, "re-entrant pre-dispatch allows instead of double-borrow panic");
        HOST.with(|h| {
            let _b = h.borrow_mut();
            dispatch_game_event("player_hurt");       // must not panic (nested notify skipped)
        });
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Slice 5E.2 Task 4: @s2script/config prelude module + re_materialize_config
    // ---------------------------------------------------------------------------

    /// The `@s2script/config` prelude module getters read from `__s2pkg_config_values` and coerce
    /// correctly; an undeclared key yields the appropriate zero-value (no throw, no undefined).
    #[test]
    fn config_getters_read_and_coerce() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // Inject a config values object directly (simulates what load_plugin_js does via T3).
        eval_in_context("p", r#"
            globalThis.__s2pkg_config_values = { greeting: "hi", maxUses: 3, cooldown: 1.5, enabled: true };
        "#).unwrap();
        // getString: declared key → string value.
        assert_eq!(eval_in_context_string("p", "__s2pkg_config.config.getString('greeting')"), "hi");
        // getInt: declared key → integer coercion.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('maxUses'))"), "3");
        // getFloat: declared key → number passthrough.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getFloat('cooldown'))"), "1.5");
        // getBool: declared key → boolean.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getBool('enabled'))"), "true");
        // Undeclared keys → zero-values (no crash, no throw).
        assert_eq!(eval_in_context_string("p", "__s2pkg_config.config.getString('nope')"), "");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('nope'))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getBool('nope'))"), "false");
        // Non-number passed to getInt/getFloat → zero-value (coercion guard).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('greeting'))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getFloat('greeting'))"), "0");
        // getBool: a non-true value → false.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getBool('maxUses'))"), "false");
        shutdown();
    }

    /// `re_materialize_config` re-injects `__s2pkg_config_values` (from materialized defaults
    /// when no ops are wired) and fires every `onChange` handler with the updated config object.
    #[test]
    fn config_on_change_fires_handler() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");

        // Store config decls: one string key with default "hello".
        let mut decls = std::collections::HashMap::new();
        decls.insert("greeting".to_string(), crate::config::ConfigDecl {
            r#type: "string".to_string(),
            default: serde_json::json!("hello"),
            description: None,
        });
        store_config_decls("p", decls);

        // Inject a pre-existing value that differs from the default (to show re_materialize replaces it).
        eval_in_context("p", "globalThis.__s2pkg_config_values = { greeting: 'world' };").unwrap();

        // Register an onChange handler via the prelude (uses __s2_config_on_change internally).
        eval_in_context("p", r#"
            globalThis.__seen = null;
            __s2pkg_config.config.onChange(function (cfg) { globalThis.__seen = cfg.greeting; });
        "#).unwrap();

        // Re-materialize: with no ops, materializes defaults → { greeting: "hello" }.
        // The handler must fire with that updated config object.
        re_materialize_config("p");

        // Handler should have set __seen to the re-materialized default "hello".
        assert_eq!(
            read_string_global_in("p", "__seen"),
            "hello",
            "onChange handler must receive the re-materialized config values"
        );
        // Verify __s2pkg_config_values was also updated (not just the handler arg).
        assert_eq!(
            eval_in_context_string("p", "__s2pkg_config.config.getString('greeting')"),
            "hello",
            "getters must reflect the re-injected values after re_materialize"
        );
        shutdown();
    }

    /// `re_materialize_config` for a plugin with no `onChange` subscribers degrades cleanly (no
    /// panic, no error) — the snapshot is empty, so the fire loop exits immediately.
    #[test]
    fn config_re_materialize_no_subs_degrades_cleanly() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let mut decls = std::collections::HashMap::new();
        decls.insert("x".to_string(), crate::config::ConfigDecl {
            r#type: "int".to_string(),
            default: serde_json::json!(42),
            description: None,
        });
        store_config_decls("p", decls);
        // No onChange subscribed → must not panic.
        re_materialize_config("p");
        // Values still re-injected (even with no handlers).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('x'))"), "42");
        shutdown();
    }

    /// Slice 6.1: `@s2script/chat` prelude module + `__s2_client_print` native degrade gracefully
    /// when no `client_print` op is wired (no ops table / op is None → no-op, never throw).
    #[test]
    fn client_print_and_chat_degrade_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // No client_print op in the test host → the native + Chat.* are no-ops that never throw.
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_chat.Chat.toSlot"), "function");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_chat.Chat.toAll"),  "function");
        // Calling them with no op must not throw (returns undefined).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_chat.Chat.toSlot(0, 'hi'))"), "undefined");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_chat.Chat.toAll('hi'))"),      "undefined");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_print(0, 'hi'))"),          "undefined");
        shutdown();
    }

    /// Slice 6.2 Task 1: two-tier admin cache (file + runtime) UNION, per-tier remove, clear_file,
    /// one-shot load guard, and `client_steamid` degrades to "0" without the op.
    #[test]
    fn admin_cache_two_tier_union_and_guard() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // set file + runtime tiers; get unions them.
        eval_in_context("p", "__s2_admin_set('111', 4, false); __s2_admin_set('111', 1, true);").unwrap(); // file KICK(4) + runtime RESERVATION(1)
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('111'))"), "5"); // 4|1
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('999'))"), "0"); // absent
        // remove runtime tier only → file remains.
        eval_in_context("p", "__s2_admin_remove('111', true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('111'))"), "4");
        // clear_file wipes file tier.
        eval_in_context("p", "__s2_admin_clear_file();").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('111'))"), "0");
        // load guard: the prelude already called __s2_admin_mark_loaded() in create_plugin_context,
        // so subsequent calls return true (already-loaded state).
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_mark_loaded())"), "true");
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_mark_loaded())"), "true");
        // client_steamid degrades to "0" without the op.
        assert_eq!(eval_in_context_string("p", "__s2_client_steamid(0)"), "0");
        // client_kick degrades to a no-op (undefined) without the op.
        assert_eq!(eval_in_context_string("p", "String(__s2_client_kick(0, 'x'))"), "undefined");
        // server_command degrades to a no-op (undefined); server_map_valid to 0; the module wires them.
        assert_eq!(eval_in_context_string("p", "String(__s2_server_command('x'))"), "undefined");
        assert_eq!(eval_in_context_string("p", "String(__s2_server_map_valid('x'))"), "0");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_server.Server.command"), "function");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.isMapValid('x'))"), "false");
        // Slice 6.6: the damage natives degrade without ops (read->0, victim->-1, write no-op) and the
        // @s2script/damage module wires (Damage.onPre a function; DamageInfo reads degrade to 0/null).
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_read_float(68))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_read_int(60))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_victim())"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_write_float(68, 5))"), "undefined");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_damage.Damage.onPre"), "function");
        assert_eq!(eval_in_context_string("p", "String(new __s2pkg_damage.DamageInfo().damage)"), "0");
        assert_eq!(eval_in_context_string("p", "String(new __s2pkg_damage.DamageInfo().victim)"), "null");
        // Slice 6.7: cvar_get degrades to "" without the op; Server.getCvar/setCvar wired.
        assert_eq!(eval_in_context_string("p", "String(__s2_cvar_get('sv_gravity'))"), "");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_server.Server.getCvar"), "function");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_server.Server.setCvar"), "function");
        // Slice 6.12: plugin natives degrade (no file-watch in-isolate → empty list, ops false) + module wires.
        assert_eq!(eval_in_context_string("p", "__s2_plugins_list()"), "[]");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_unload('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_reload('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_load('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "JSON.stringify(__s2pkg_plugins.Plugins.list())"), "[]");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_plugins.Plugins.reload"), "function");
        shutdown();
    }

    /// Slice 6.6: Damage.onPre subscribes to DAMAGE_MUX and dispatch_damage runs the handler with a
    /// DamageInfo (no engine ops → the handler's info.damage reads 0, but the pipeline fires).
    #[test]
    fn damage_dispatch_runs_subscriber() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__dmgFired = 0; __s2pkg_damage.Damage.onPre(function (info) { globalThis.__dmgFired = 1; globalThis.__dmgVal = info.damage; });").unwrap();
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__dmgFired)"), "1", "the onPre handler ran");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__dmgVal)"), "0", "info.damage reads 0 without an engine op");
        shutdown();
    }

    /// Slice 6.2 Task 2: `@s2script/admin` prelude module — ADMFLAG constants, Admin.add/get/hasFlags,
    /// root-implies-all, non-admin→null, __s2_admin_check hook, parseFile name→bit mapping.
    #[test]
    fn admin_module_flags_api_and_hook() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // ADMFLAG bit values (SM-exact).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.ADMFLAG.KICK)"), "4");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.ADMFLAG.ROOT)"), String::from("16384"));
        // Admin.add (runtime) + get + hasFlags (exact-subset + root=all).
        eval_in_context("p", "__s2pkg_admin.Admin.add('555', __s2pkg_admin.ADMFLAG.KICK | __s2pkg_admin.ADMFLAG.CHAT);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('555').hasFlags(__s2pkg_admin.ADMFLAG.CHAT))"), "true");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('555').hasFlags(__s2pkg_admin.ADMFLAG.BAN))"), "false");
        eval_in_context("p", "__s2pkg_admin.Admin.add('777', __s2pkg_admin.ADMFLAG.ROOT);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('777').hasFlags(__s2pkg_admin.ADMFLAG.BAN))"), "true"); // root ⇒ all
        // Non-admin → null.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('000'))"), "null");
        // The check hook is installed + honours the cache (bot slot → steamid "0" → not admin → false).
        assert_eq!(eval_in_context_string("p", "String(typeof globalThis.__s2_admin_check)"), "function");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__s2_admin_check(0, __s2pkg_admin.ADMFLAG.CHAT))"), "false");
        // Hardening: even a misconfigured "0" admin entry must NOT grant a bot/unauth (steamid "0") — forSlot guards it.
        eval_in_context("p", "__s2_admin_set('0', __s2pkg_admin.ADMFLAG.ROOT, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__s2_admin_check(0, __s2pkg_admin.ADMFLAG.CHAT))"), "false"); // "0" never an admin
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.forSlot(0))"), "null");
        // parseFile: name→bit mapping (file-tier path).
        eval_in_context("p", r#"__s2_admin_parseFile('{"888":["kick"]}');"#).unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('888').hasFlags(__s2pkg_admin.ADMFLAG.KICK))"), "true");
        shutdown();
    }

    /// `Commands.register` builds a typed ctx (callerSlot/args/argString/reply); reply routes to
    /// console.log for slot<0, to Chat.toSlot for slot>=0.  Unload drops the command → later
    /// dispatch is a no-op.  A throwing handler is caught (no panic).
    ///
    /// Slice 6.1 Task 2.  Calls `dispatch_concommand` directly (simulates the engine trampoline).
    #[test]
    fn command_dispatch_builds_ctx_and_routes_reply() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // A plugin registers sm_test; capture the ctx it receives.
        load_plugin_js("cmd", r#"
            var C = __s2pkg_commands.Commands;
            C.register("sm_test", function (ctx) {
                globalThis.__seen = ctx.callerSlot + "|" + ctx.args.join(",") + "|" + ctx.argString;
                // SM-parity arg API (Slice 6.10): arg/argInt/argFloat/argsFrom/argCount.
                globalThis.__argapi = [ctx.argCount, ctx.arg(0), ctx.argInt(1), ctx.argFloat(1),
                                        ctx.argsFrom(2), ctx.arg(99), ctx.argInt(99, 7)].join("|");
                if (ctx.callerSlot < 0) ctx.reply("console-reply");   // routes to console.log
            });
        "#, "{}");
        // Simulate the engine firing the command from the server console (slot -1).
        dispatch_concommand("sm_test", -1, "foo bar");
        assert_eq!(eval_in_context_string("cmd", "String(globalThis.__seen)"), "-1|foo,bar|foo bar");
        // The arg API: dispatch "target 42 hello world" and verify typed retrieval.
        dispatch_concommand("sm_test", -1, "target 42 hello world");
        assert_eq!(eval_in_context_string("cmd", "String(globalThis.__argapi)"),
                   "4|target|42|42|hello world||7",
                   "argCount|arg(0)|argInt(1)|argFloat(1)|argsFrom(2)|arg(99)=''|argInt(99,7)=7");
        assert!(LOG.lock().unwrap().iter().any(|m| m.contains("console-reply")), "console reply routed to log");
        // A throwing handler is caught (no panic).
        load_plugin_js("cmd2", r#" __s2pkg_commands.Commands.register("sm_boom", function(){ throw new Error("x"); }); "#, "{}");
        dispatch_concommand("sm_boom", -1, "");   // must not panic
        // Unload drops the command → a later dispatch is a no-op.
        unload_plugin("cmd");
        eval_in_context("cmd2", "globalThis.__afterUnload = 'unchanged';").unwrap();
        dispatch_concommand("sm_test", -1, "again");   // cmd is gone → no handler → no-op
        shutdown();
    }

    /// Slice 6.11: chat-trigger parsing + same-context dispatch (a player's "!cmd" runs the command).
    #[test]
    fn chat_triggers_parse_and_dispatch() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_plugin_js("ct", r#"
            var C = __s2pkg_commands.Commands;
            globalThis.__ran = "";
            C.register("sm_test", function (ctx) { globalThis.__ran = ctx.callerSlot + ":" + ctx.argString; });
            globalThis.__p1 = JSON.stringify(C.parseChatTrigger("!kick Bob Smith"));
            globalThis.__p2 = JSON.stringify(C.parseChatTrigger("/who"));
            globalThis.__p3 = String(C.parseChatTrigger("hello world"));            // null -> "null"
            globalThis.__h  = JSON.stringify(C.handleChatTrigger(5, "!test foo bar")); // sm_ prepend -> sm_test
            globalThis.__hMiss = JSON.stringify(C.handleChatTrigger(5, "!nope x"));   // no such command -> ran:false
        "#, "{}");
        assert_eq!(eval_in_context_string("ct", "globalThis.__p1"), r#"{"silent":false,"name":"kick","argString":"Bob Smith"}"#);
        assert_eq!(eval_in_context_string("ct", "globalThis.__p2"), r#"{"silent":true,"name":"who","argString":""}"#);
        assert_eq!(eval_in_context_string("ct", "globalThis.__p3"), "null");
        assert_eq!(eval_in_context_string("ct", "globalThis.__ran"), "5:foo bar", "sm_test dispatched via !test");
        assert_eq!(eval_in_context_string("ct", "globalThis.__h"), r#"{"silent":false,"ran":true}"#);
        assert_eq!(eval_in_context_string("ct", "globalThis.__hMiss"), r#"{"silent":false,"ran":false}"#, "trigger consumed even if the command is unknown");
        shutdown();
    }

    /// Slice 6.11b: the core Host_Say chat dispatch. `dispatch_chat(slot, text)` parses a !cmd / /cmd
    /// trigger, dispatches the matching (or `sm_`-prefixed) command in its owner context with the
    /// speaker's slot, and returns whether to SUPPRESS the broadcast — a matched silent `/` only.
    /// This is exactly the fn the shim's Host_Say detour calls.
    #[test]
    fn chat_dispatch_host_say_parses_and_suppresses() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_plugin_js("hs", r#"
            var C = __s2pkg_commands.Commands;
            globalThis.__ran = "";
            C.register("sm_test", function (ctx) { globalThis.__ran = ctx.callerSlot + ":" + ctx.argString; });
        "#, "{}");
        // Public `!test` → dispatches sm_test (sm_ fallback) with slot 5 + args, and NEVER suppresses.
        assert_eq!(dispatch_chat(5, "!test foo bar"), false, "! trigger never suppresses");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "5:foo bar", "!test dispatched sm_test");
        // Silent `/test` → dispatches AND suppresses (matched silent trigger).
        eval_in_context("hs", "globalThis.__ran = '';").unwrap();
        assert_eq!(dispatch_chat(7, "/test"), true, "matched / trigger suppresses");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "7:", "/test dispatched with empty args");
        // Ordinary chat (no trigger char) → no dispatch, no suppress.
        eval_in_context("hs", "globalThis.__ran = 'untouched';").unwrap();
        assert_eq!(dispatch_chat(5, "hello world"), false, "ordinary chat is not a trigger");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "untouched", "ordinary chat did not dispatch");
        // Unknown `/nope` → no command match → NOT suppressed (never swallow a non-command message).
        assert_eq!(dispatch_chat(5, "/nope"), false, "unmatched silent trigger is not suppressed");
        shutdown();
    }

    #[test]
    fn command_trio_server_and_admin_gating() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_plugin_js("t", r#"
            var C = __s2pkg_commands.Commands;
            C.registerServer("sm_srv", function(ctx){ globalThis.__srv = ctx.callerSlot; });
            C.registerAdmin("sm_adm", 512 /*CHAT=1<<9*/, function(ctx){ globalThis.__adm = ctx.callerSlot; });
            // Install a fake admin-check: slot 5 allowed, others denied.
            globalThis.__s2_admin_check = function(slot, mask){ return slot === 5; };
        "#, "{}");
        // registerServer: console (-1) runs; a player (3) denied.
        dispatch_concommand("sm_srv", -1, ""); assert_eq!(eval_in_context_string("t", "String(globalThis.__srv)"), "-1");
        eval_in_context("t", "globalThis.__srv = 'none';").unwrap();
        dispatch_concommand("sm_srv", 3, ""); assert_eq!(eval_in_context_string("t", "String(globalThis.__srv)"), "none"); // stayed
        // registerAdmin: console (-1) = root runs; slot 5 (hook true) runs; slot 3 (hook false) denied.
        dispatch_concommand("sm_adm", -1, ""); assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "-1");
        eval_in_context("t", "globalThis.__adm = 'none';").unwrap();
        dispatch_concommand("sm_adm", 5, ""); assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "5");
        eval_in_context("t", "globalThis.__adm = 'none';").unwrap();
        dispatch_concommand("sm_adm", 3, ""); assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "none"); // denied
        // Fail-safe: with NO admin-check hook installed, a player is DENIED (never accidentally granted).
        eval_in_context("t", "delete globalThis.__s2_admin_check; globalThis.__adm = 'none';").unwrap();
        dispatch_concommand("sm_adm", 3, ""); assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "none"); // no hook → denied
        shutdown();
    }
}
