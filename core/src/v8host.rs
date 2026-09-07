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
use crate::dispatch::{
    fan_out, fan_out_collapsing, fan_out_inner, set_after_handler, Delivery, Instrument, StopAt,
};
use crate::multiplexer::{self, Descriptor, DetourChange, HookResult, Phase, Priority};
use crate::plugin;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

mod lifecycle;
mod natives;
mod interop_wire;
mod interop_lifetime;
use interop_wire::*;
mod timers;

pub use lifecycle::unload_all;
#[allow(unused_imports)] // retained parent-module surface for existing internal/test callers
pub(crate) use lifecycle::{
    clear_failed, clear_pending_handoff, config_file_content, create_plugin_context, current_frame,
    dispose_plugin_context, eval_in_context, failed_plugin_ids, finalize_loading_plugins,
    iface_published, iface_published_types_sha256, is_failed, is_loading, load_plugin_js,
    materialize_for_load, materialize_for_load_snapshot, plugin_phase, queue_pending_reload,
    read_engine_config, re_materialize_config, re_materialize_config_snapshot, reconcile_initial_config_snapshot, set_failed,
    set_plugin_version, store_config_decls, unload_plugin,
};
use lifecycle::{s2_config_on_change, s2_handoff_take, s2_load_failed, s2_load_settled};
use natives::install_natives;
use timers::{
    s2_delay, s2_next_frame, s2_next_tick, s2_thread_sleep, s2_timer_alive, s2_timer_create,
    s2_timer_kill,
};

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
// Generated from core/engine-ops.jsonc (A7). The table stays ONE whole struct
// in this module — do not split it across feature files.
// ---------------------------------------------------------------------------
include!("engine_ops.generated.rs");

/// The engine-ops table as copied at init, for the modules outside `v8host` that need an op
/// (`gamedata_calls`' resolve/invoke). `None` until `set_engine_ops` runs; a null field inside it
/// degrades that op's caller to a named miss.
/// The embedder's install/remove-detour callback, for features that arm a global engine hook
/// (`events`' PRE mux). Mirrors `engine_ops` — an accessor so the thread-local stays private.
pub(crate) fn hook_request() -> Option<HookRequestFn> {
    HOOK_REQUEST.with(|c| c.get())
}

pub(crate) fn engine_ops() -> Option<S2EngineOps> {
    ENGINE_OPS.with(|o| o.get())
}

/// Why [`with_host_isolate`] could not hand Dispatch the isolate.
///
/// `Busy` is a live `HOST` borrow (`#63` no-handle re-entry). Notify reports `Deferred`.
/// `Absent` is core not initialized (`HOST` is `None`). That is `Delivered`, never `Deferred` —
/// deferring would loop forever on the next drain.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HostAccess {
    Busy,
    Absent,
}

/// Run `f` with the host isolate, or report why it could not be taken.
///
/// Does not expose `HOST`. Dispatch must call this only AFTER the nest-token
/// CallbackScope path, so a published outbound native never takes this borrow.
pub(crate) fn with_host_isolate<R>(
    f: impl FnOnce(&mut v8::OwnedIsolate) -> R,
) -> Result<R, HostAccess> {
    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else {
            return Err(HostAccess::Busy);
        };
        let Some(host) = borrow.as_mut() else {
            return Err(HostAccess::Absent);
        };
        Ok(f(&mut host.isolate))
    })
}

/// Generation-gated liveness. Does not expose `REGISTRY`.
pub(crate) fn owner_is_live(owner: &str, generation: u64) -> bool {
    REGISTRY.with(|r| r.borrow().is_live(owner, generation))
}

/// Clone a plugin context out of the table so the handler may re-enter `PLUGINS`.
/// Does not expose `PLUGINS`.
pub(crate) fn clone_plugin_context(owner: &str) -> Option<v8::Global<v8::Context>> {
    PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone()))
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
/// Immutable context-origin token. REGISTRY remains the liveness authority;
/// protocol 2 compares this token to its current generation before every crossing.
struct InteropGeneration(u64);

/// A loaded plugin instance: its per-plugin `v8::Context` plus the captured `module.exports`
/// object (present once `load_plugin_js` has run the CJS bundle).  Field order is load-bearing
/// for teardown: `exports` (a `Global<Object>` pointing INTO the context) is declared FIRST so
/// Rust drops it BEFORE `context` — the spike's teardown discipline (inner Globals released
/// before the `Global<Context>`, while the isolate is still alive).  Task 6 walks the ledger to
/// call `onUnload` off `exports` before disposing the context.
struct PluginInstance {
    /// The plugin's settled `PluginHooks` object (`{ onUnload?, state? }`) once its factory settled
    /// OK — stored by `__s2_load_settled`. `None` until the factory settles (or if it returns no
    /// hooks). Declared FIRST so Rust drops it BEFORE `context` (teardown discipline: inner Globals
    /// released before the `Global<Context>`). (Field kept named `exports` to minimize churn.)
    exports: Option<v8::Global<v8::Object>>,
    context: v8::Global<v8::Context>,
    // NOTE: no `generation` field. The generation lives ONLY in REGISTRY (`generation_of`) — a
    // copy here was "kept in lockstep" but readable in the window where the two diverge (prelude
    // eval runs after REGISTRY registration, before this instance lands in PLUGINS), which is
    // exactly how prelude-time subscriptions got stamped with the never-live 0 (P0-1). One
    // authority, no lockstep to maintain.
    /// The plugin's declared config fields (from its manifest).  Stored at load so
    /// `re_materialize_config` can re-run materialization without the manifest.
    /// Starts empty; populated by `store_config_decls` right after `load_plugin_js`.
    config_decls: std::collections::HashMap<String, crate::config::ConfigEntry>,
    /// Lifecycle phase (design spec §5). Starts `Loading` at `create_plugin_context`; reaches
    /// `Active` in `finalize_loading_plugins` once the factory settled + the ctx armed; `Unloading`
    /// during teardown.
    phase: crate::plugin::Phase,
}

/// One in-flight factory load (design spec §5). Tracks the frame the load started (for the timeout),
/// its settle state, and whether a reload was queued while it was still loading.
struct LoadingEntry {
    started_frame: u64,
    state: SettleState,
    pending_reload: bool,
}

/// The settle state of an in-flight factory load.
enum SettleState {
    InFlight,
    Settled,
    Failed(String),
}

/// Load timeout: a factory promise that never settles within ~30s (at 64Hz) → `Failed`
/// (design spec §5.2, resolved decision #4).
pub(crate) const LOAD_TIMEOUT_FRAMES: u64 = 1920;

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
    /// Boot instant for the breadcrumb's uptime field (set once in `init`).
    static UPTIME_START: std::cell::Cell<Option<Instant>> = std::cell::Cell::new(None);
    /// Pending timer queue (Delay/NextTick/NextFrame).  Holds only `u64` ids; the promise lives
    /// in the Jobs resolver map.  Borrowed briefly in `make_timer_promise`/`frame_async_drain`/`refresh_detour`;
    /// NEVER held across `perform_microtask_checkpoint` (a continuation re-enters it).
    static TIMERS: std::cell::RefCell<TimerQueue> = std::cell::RefCell::new(TimerQueue::new());
    /// Callback timers (`Timers.after`/`Timers.every`) — distinct from the Jobs resolver map, which
    /// holds one-shot Promise resolvers. A callback timer's function must SURVIVE firing when it
    /// repeats, so it cannot live in a map the drain removes from unconditionally. Keyed by the same
    /// async-id space as TIMERS/jobs so ledger teardown reaches all three by one id.
    static TIMER_CBS: std::cell::RefCell<std::collections::HashMap<u64, TimerCallback>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Timer ids killed since the last drain step. The drain REMOVES a callback from TIMER_CBS
    /// before firing it (so a callback cannot observe a half-updated map), which means "did this
    /// callback kill itself?" cannot be answered by looking in TIMER_CBS — it is already absent
    /// either way. `__s2_timer_kill` records the id here instead, and the drain consults it before
    /// re-arming. Without this a self-killing repeater fires forever.
    static TIMER_KILLED: std::cell::RefCell<std::collections::HashSet<u64>>
        = std::cell::RefCell::new(std::collections::HashSet::new());
    /// Pending unhandled rejections awaiting end-of-frame confirmation (D-2): promise identity
    /// hash → (message, stack). kPromiseHandlerAddedAfterReject removes its entry; whatever
    /// survives to the frame_async_drain flush is reported. Cleared on shutdown.
    static PENDING_REJECTS: std::cell::RefCell<std::collections::HashMap<i32, (String, String)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());

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
    /// Per-plugin `v8::Context` registry, keyed by plugin id — the multi-context path that will
    /// eventually replace the single shared `HOST.context` (Task 5 migrates the natives/dispatch
    /// onto it).  Each `Global<Context>` is stamped with a `PluginId` slot at creation.  ADDED
    /// ALONGSIDE `HOST` for this task: the existing single-context path is untouched.  Dropped
    /// (per id in `dispose_plugin_context`, or all in `shutdown`) while the isolate is still alive
    /// — same discipline as the Jobs resolver map / `CONCOMMANDS`.
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
    // EVENT_MUX / EVENT_MUX_PRE / EVENT_RECIPIENTS moved to `crate::events`.
    // Damage fan-out is per-entity SDKHooks (`crate::sdkhooks`), not a global mux.
    /// Config-change subscriber mux (Slice 5E.2): handlers subscribed via `config.onChange(h)`.
    /// Each handler is tagged `(owner, generation)` for liveness-gated dispatch.
    /// The loader polls opted-in plugins' config files each frame cycle and calls
    /// `re_materialize_config(id)` on change, which snapshots this mux and fires handlers.
    /// `remove_by_owner` called on unload; reset on shutdown so a re-init starts empty.
    static CONFIG_SUBS: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    // CLIENT_CMD_SUBS / CHAT_MSG_SUBS / CONCOMMANDS / COMMAND_META moved to `crate::commands`.
    // CLIENT_MUX moved to `crate::client`.

    /// Map-start subscribers (clientlist-fakeconvar-onmapstart slice). Fixed key "" (map-start has
    /// no name dimension, like CHAT_MSG_SUBS); notify-only.
    static MAP_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    /// Precache subscribers (Sound slice). Fixed key "" (a precache-manifest build has no name
    /// dimension, like MAP_MUX); notify-only. The stored handler is the PRELUDE's wrapper closure —
    /// it constructs the block-scoped PrecacheContext and calls the plugin's handler.
    static PRECACHE_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    // COOKIE_CACHED_MUX / _PENDING moved to `crate::cookies`.
    // WS_EVENT_MUX / WS_EVENT_PENDING moved to `crate::ws`.


    /// Entity-I/O slice: `Entity.onOutput(classname, output, handler)` subscriber mux, keyed by the
    /// literal string `"<classname>\0<output>"` (a NUL separator — classnames/outputs never contain one).
    /// `"*"` is a valid wildcard for either half (matched at dispatch by querying all 4 combinations).
    /// Unlike the process-wide damage detour / `CHAT_MSG_SUBS` (installed once, unconditionally, for the
    /// process lifetime), the `FireOutputInternal` detour here is likewise installed unconditionally at
    /// shim Load — so there is no per-subscribe engine-op and no engine-op on empty teardown. Dispatch is
    /// SYNCHRONOUS (the detour blocks on it, mirrors SDKHooks OnTakeDamage / `EVENT_MUX_PRE`, NOT the post-drain
    /// `*_PENDING` muxes) so a handler's `HookResult` can suppress the output before the original runs.
    /// `remove_by_owner` on unload; reset on shutdown so a re-init starts empty.
    static OUTPUT_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    /// `Server.onCvarChange(name, handler)` subscribers, keyed by cvar name (`"*"` = every cvar).
    /// Fed by the shim's ONE `ICvar::InstallGlobalChangeCallback`, installed unconditionally at Load —
    /// so there is no per-subscribe engine op. NOTIFY-only: the engine's global change callback runs
    /// AFTER the value has already changed, so there is nothing to veto and handlers return nothing
    /// (`ICvar::CallFilterCallback` would be the vetoing path — out of scope, see the design spec).
    /// `remove_by_owner` on unload; reset on shutdown so a re-init starts empty.
    static CVAR_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    /// Usercmd primitive Task 2: `UserCmd.onRun(handler)` subscriber mux, keyed by the constant "onRun"
    /// (usercmd has no name dimension, like damage's single OnTakeDamage type). Dispatch is SYNCHRONOUS (the
    /// Task-3 per-tick input-processing detour blocks on it, mirrors SDKHooks OnTakeDamage / `OUTPUT_MUX`) so a handler's
    /// returned `HookResult` can block the original input for that tick. The detour installs LAZILY on
    /// the first-ever subscribe (via the `usercmd_hook_install` engine op — see `s2_usercmd_subscribe`),
    /// mirroring `ENTITY_MUX`'s `entity_listener_install` trigger. `remove_by_owner` on unload; reset on
    /// shutdown so a re-init starts empty.
    static USERCMD_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    // UserMessage interception: USERMSG_MUX / _IDS / _RESOLVE moved to `crate::usermsg`, which owns
    // this feature's state, natives, dispatch and teardown together.

    // ENTITY_MUX moved to `crate::entity`.

    /// Slice 5E.3: reload state-handoff blobs (id → the JSON string produced by `iface_to_json` in the
    /// OLD context during `onUnload`). Consumed by `load_plugin_js` on the next load of that id (a
    /// Reload) and revived via `iface_from_json`; cleared by the loader on a final removal (Vanished);
    /// reset on `shutdown`. It holds a plain `String`, so it survives the old context's disposal.
    static PENDING_HANDOFF: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());

    /// L1 lifecycle v2: in-flight factory loads (id → LoadingEntry). A plugin sits here between
    /// `create_plugin_context` and its `Active`/`Failed` transition in `finalize_loading_plugins`.
    /// Reset on `shutdown`.
    static LOADING: std::cell::RefCell<std::collections::HashMap<String, LoadingEntry>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// L1 lifecycle v2: plugins whose load FAILED (context already disposed) — reason kept for
    /// `sm plugins list` (spec §5/§8). Cleared for an id on a fresh `create_plugin_context`, and in
    /// bulk on `shutdown`.
    static FAILED_PLUGINS: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// L1 lifecycle v2: id → manifest version, set by the loader before `load_plugin_js` so the
    /// `Active`-transition breadcrumb (fired in `finalize_loading_plugins`) can carry it without the
    /// manifest. Reset on `shutdown`.
    static MANIFEST_VERSIONS: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());

    // Admin cache state moved to `crate::admin`; ban cache state to `crate::bans`.

    /// TopMenu registry (adminmenu framework). Ordered tabs (deduped by id) + items owned by a
    /// plugin. Item `onSelect` is a Global<Function> held like a command handler (NOT marshalled;
    /// invoked in the owner's context on select). Owner-scoped teardown mirrors CONCOMMANDS.
    /// A tab is what the dashboard renders as one selectable page; `addCategory(name)` is the
    /// id==title form of `addTab`.
    static TOPMENU_CATEGORIES: std::cell::RefCell<Vec<TopMenuTab>> = std::cell::RefCell::new(Vec::new());
    static TOPMENU_ITEMS: std::cell::RefCell<std::collections::HashMap<String, TopMenuItem>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Slots+ids queued by __s2_topmenu_select (called under the isolate borrow from a menu onSelect);
    /// fanned out post-frame by dispatch_pending_topmenu_select (ffi.rs, HOST free). Same discipline as
    /// cookies::COOKIE_CACHED_PENDING — sidesteps the re-entrant double-borrow.
    static TOPMENU_PENDING: std::cell::RefCell<Vec<(String, i32)>> = std::cell::RefCell::new(Vec::new());
    /// Monotonic insertion counter → each item's `seq`, so `snapshot` renders items in REGISTRATION order
    /// (a HashMap iterates in random per-instance order that would shuffle across restarts; the spec commits
    /// the MVP to insertion order). A re-added id reuses its existing seq so a plugin reload doesn't reorder.
    static TOPMENU_SEQ: std::cell::Cell<u64> = std::cell::Cell::new(0);
}

// Declarative inbound hooks. Their own `thread_local!` block only because the one above is already
// at the `thread_local_inner!` macro recursion limit — the file has four such blocks for the same
// reason.
thread_local! {
    /// Subscribers to a gamedata-DECLARED engine detour, keyed by `hook_key(owner, name)` — the
    /// hook's identity, not the subscriber's. One channel per declared hook; any plugin may
    /// subscribe to any declared hook through its generated `ctx` namespace, and `remove_by_owner`
    /// tears a SUBSCRIBER's rows down (the DESCRIPTOR's teardown is `gamedata_hooks::drop_owner`,
    /// keyed by the declaring owner — two different senses of "owner" over one id space).
    ///
    /// Dispatch is SYNCHRONOUS and COLLAPSING (the thunk blocks on it and suppresses the original
    /// engine call at >= Handled), which is why it can never be deferred — see `dispatch_hook`.
    /// The detour installs LAZILY on subscribe (`gamedata_hooks::subscribe`), mirroring
    /// `USERCMD_MUX`'s `usercmd_hook_install` trigger.
    static HOOK_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());

    /// The inbound hook dispatch currently running: the thunk's own stack-frame arg view, plus the
    /// hook it belongs to (so an accessor failure can be reported as a NAMED degrade of that hook).
    ///
    /// SAVE/RESTORE, not set/clear — the same discipline `engine_hooks.cpp`'s `g_activeView` uses on
    /// the other side of the boundary, and for the same reason: a dispatch can nest. `None` between
    /// dispatches, which is what makes every accessor fail closed outside one: the pointer is a
    /// STACK FRAME that dies with the thunk, so "is it still live?" cannot be answered from the
    /// pointer's own contents.
    static ACTIVE_HOOK: std::cell::RefCell<Option<ActiveHook>>
        = const { std::cell::RefCell::new(None) };

    /// Monotonic dispatch counter — the source of `ActiveHook::epoch`. Never reset, including at
    /// shutdown: a view object rooted in a plugin context that survives a re-init must not have its
    /// epoch collide with a fresh dispatch's. At one dispatch per tick it would take ~9 billion
    /// years to wrap.
    static HOOK_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// Pickup-gate vote collection. Its own block: the one above is at the thread_local_inner! limit.
thread_local! {
    /// Live acquire-fold session, save/restored around a nested dispatch.
    static ACQUIRE: std::cell::RefCell<Option<AcquireSession>> =
        const { std::cell::RefCell::new(None) };

    /// Post-phase `skipped` flag, published only while `dispatch_hook_post` builds its view.
    static HOOK_POST_SKIPPED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

struct AcquireSession {
    view: *mut std::ffi::c_void,
    votes: Vec<crate::acquire::AcquireVote>,
    wrote: bool,
}

/// A registered TopMenu tab (dashboard page). `id` is the addItem grouping key; `title` is the label.
struct TopMenuTab {
    id: String,
    title: String,
}

/// Ensure `id` exists in registration order. `title` of `Some` replaces the label; `None` keeps
/// an existing title (or uses `id` when inserting).
fn topmenu_ensure_tab(id: String, title: Option<String>) {
    TOPMENU_CATEGORIES.with(|c| {
        let mut tabs = c.borrow_mut();
        if let Some(tab) = tabs.iter_mut().find(|tab| tab.id == id) {
            if let Some(title) = title {
                tab.title = title;
            }
            return;
        }
        let title = title.unwrap_or_else(|| id.clone());
        tabs.push(TopMenuTab { id, title });
    });
}

/// A registered TopMenu item. `on_select` is invoked in `owner`'s context (liveness-gated by `generation`).
/// `seq` is a monotonic insertion index — `snapshot` sorts by it for stable, registration-order rendering.
struct TopMenuItem {
    category: String,
    name: String,
    flags: i64,
    sheets: Vec<String>,
    owner: String,
    generation: u64,
    seq: u64,
    on_select: v8::Global<v8::Function>,
}

/// checktransmit slice: per-plugin entity-visibility rules.
/// INVARIANT: all owners' entries for one index share ONE serial (enforced in s2_transmit_set —
/// the op validates the incoming serial is the live one, so different-serial entries are stale
/// and evicted). The shim holds only the AND-merged mask per index; this table is the policy
/// source of truth so unload/reset can recompute the merge.
#[derive(Clone, Copy)]
struct TransmitRule { serial: i32, mask: u64 }
impl crate::fold::AndFold for TransmitRule {
    fn and_fold(self, other: Self) -> Self {
        Self { serial: other.serial, mask: self.mask & other.mask }
    }
}
thread_local! {
    static TRANSMIT_RULES: std::cell::RefCell<crate::fold::FoldTable<i32, TransmitRule>> =
        std::cell::RefCell::new(crate::fold::FoldTable::new());
    static VOICE_RULES: std::cell::RefCell<crate::fold::FoldTable<i32, u64>> =
        std::cell::RefCell::new(crate::fold::FoldTable::new());
}

/// Recompute the AND-merged mask for `index` across every owner's rule and push it to the shim
/// (transmit_set), or clear the shim entry when no rule remains (transmit_clear).
fn transmit_recompute_and_push(index: i32) {
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
    match TRANSMIT_RULES.with(|r| r.borrow().merged(&index)) {
        Some(rule) => { if let Some(f) = ops.transmit_set { f(index, rule.serial, rule.mask); } }
        None => { if let Some(f) = ops.transmit_clear { f(index); } }
    }
}

/// AND-merge every owner's rule for `sender`. `None` when no owner has one — which is distinct from
/// `Some(0)` ("audible to nobody"), the distinction `s_voiceHasRule` exists for on the shim side.
fn voice_merged(sender: i32) -> Option<u64> {
    VOICE_RULES.with(|r| r.borrow().merged(&sender))
}

/// Recompute and push to the shim: set the merged mask, or clear when no rule remains.
fn voice_recompute_and_push(sender: i32) {
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
    match voice_merged(sender) {
        Some(mask) => { if let Some(f) = ops.voice_audible_set { f(sender, mask); } }
        None => { if let Some(f) = ops.voice_audible_clear { f(sender); } }
    }
}

/// Unload/resetAll teardown: drop every rule `owner` holds and re-push each sender it touched.
fn voice_remove_owner(owner: &str) {
    let touched = VOICE_RULES.with(|r| r.borrow_mut().remove_owner(owner));
    for s in touched { voice_recompute_and_push(s); }
}

/// Drop EVERY owner's rule for one sender slot, and push the clear.
///
/// Called on client disconnect (slot-reuse hygiene). A hearability rule is authored about the player
/// who occupied the slot, and the engine recycles slots — so a surviving rule would silence, or grant
/// hearing to, the next occupant with no plugin action. This mirrors the mute's own disconnect clear
/// in the shim; unlike `voice_remove_owner` it crosses owners, because the departing player is not
/// any one plugin's concern.
pub(crate) fn voice_clear_slot(sender: i32) {
    let touched = VOICE_RULES.with(|r| r.borrow_mut().clear_key(&sender));
    if touched { voice_recompute_and_push(sender); }
}

#[cfg(test)]
fn voice_rules_clear_for_test() { VOICE_RULES.with(|r| r.borrow_mut().clear()); }
#[cfg(test)]
fn voice_set_rule_for_test(owner: &str, sender: i32, mask: u64) {
    VOICE_RULES.with(|r| r.borrow_mut().insert(owner, sender, mask));
}
#[cfg(test)]
fn voice_merged_for_test(sender: i32) -> Option<u64> { voice_merged(sender) }

/// Allocate the next monotonic subscription id (1-based; 0 = none). The single allocator behind
/// every EventMux-family row id and the inter-plugin `iface_on` sub id, so a Scope's ids never
/// collide with another store's. Reset to 1 on shutdown.
/// A `thread_local!` subscription store, as taken by `subscribe_into`.
pub(crate) type ChannelStore =
    std::thread::LocalKey<std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>>;

/// The subscribe-native core, owned once.
///
/// Nineteen `__s2_*` subscribe natives repeated this verbatim: pull the handler `Local` out of the
/// args, root it as a `Global`, resolve the CALLING plugin from the context slot, look up that
/// plugin's current generation (the reload-liveness token dispatch checks later), allocate a
/// subscription id, and store the row. Only four things varied — which argument holds the handler,
/// the channel key, the store, and whether an engine-op follow-up fires on the first subscriber.
///
/// The `owner` fallback of `"legacy"` is preserved from every copy: a subscription from a non-plugin
/// context (the shared HOST context, or a raw eval in tests) is still stored and still dispatches; it
/// simply never matches a plugin id for liveness or teardown.
///
/// Returns `(sub_id, was_first_on_this_channel)`, or `None` when the argument is not a function — in
/// which case nothing was stored and the caller should leave its return value alone. `was_first` is
/// what the callers key their engine-op follow-up on (`event_subscribe`, installing a detour, …).
pub(crate) fn subscribe_into(
    scope: &mut v8::PinScope,
    args: &v8::FunctionCallbackArguments,
    store: &'static ChannelStore,
    key: &str,
    handler_arg: i32,
) -> Option<(u64, bool)> {
    let func_local = v8::Local::<v8::Function>::try_from(args.get(handler_arg)).ok()?;
    let handler_g = v8::Global::new(scope.as_ref(), func_local);
    let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
    // Stamp the generation from REGISTRY (the authority dispatch's `is_live` checks), NOT from
    // PLUGINS: the plugin is registered in REGISTRY before its preludes run, but its
    // PluginInstance only lands in PLUGINS after the context is built — so a prelude-time
    // subscription read through PLUGINS would stamp the never-live sentinel 0 and be silently
    // dropped by dispatch (the P0-1 defect: prelude subs worked for exactly one plugin).
    let generation = plugin_generation(&owner);
    let sub_id = next_sub_id();
    let first = store.with(|m| m.borrow_mut().subscribe(key, sub_id, owner, generation, handler_g));
    Some((sub_id, first))
}

pub(crate) fn next_sub_id() -> u64 {
    NEXT_SUB_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

/// Native `__s2_scope_dispose(ids: number[])` (L1 lifecycle v2, Task 3). A `ctx.createScope()`'s
/// `clear()`/`dispose()` hands its tracked subscription ids here; the call sweeps every owner-scoped
/// store's `remove_by_ids` (the same self-registered registry `unload_plugin` walks by owner), so a
/// Scope tears down its subs without a per-store JS API. Degrade-never-crash: a non-array arg or an
/// empty list is a no-op.
fn s2_scope_dispose(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(0)) else { return };
        let mut ids = Vec::with_capacity(arr.length() as usize);
        for i in 0..arr.length() {
            if let Some(v) = arr.get_index(scope, i) {
                if let Some(n) = v.number_value(scope) { ids.push(n as u64); }
            }
        }
        crate::owner_stores::sweep_ids(&ids);
    }));
}

/// Unload/resetAll teardown: drop every rule owned by `owner`, re-pushing each affected index.
fn transmit_remove_owner(owner: &str) {
    let indices = TRANSMIT_RULES.with(|r| r.borrow_mut().remove_owner(owner));
    for i in indices { transmit_recompute_and_push(i); }
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

// Pending work includes producer leases and post-checkpoint callback obligations.
thread_local! { static MICROTASK_DRAIN_NEEDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
pub(crate) fn request_microtask_drain() {
    MICROTASK_DRAIN_NEEDED.with(|v| v.set(true));
    refresh_detour();
}
fn async_pending() -> usize {
    TIMERS.with(|t| t.borrow().len())
        + DUE_TIMERS.with(|q| q.borrow().len())
        + crate::jobs::pending()
        + crate::async_limits::obligations()
        + usize::from(MICROTASK_DRAIN_NEEDED.with(|v| v.get()))
        + usize::from(
            crate::cookies::pending_cached()
                || crate::ws::pending_events()
                || crate::net::pending_events(),
        )
}

// ---------------------------------------------------------------------------
// Jobs adapter — isolate / context / liveness / ledger facts the Jobs spine
// needs. Named `jobs_*` so this is a Jobs capability, not a shared
// owner-context abstraction (Dispatch is a separate slice and is not on this
// branch). Jobs never names `HOST` / `PLUGINS` / `REGISTRY`.
// ---------------------------------------------------------------------------

/// The calling plugin's `(id, current generation)` for tagging an async resolver.
pub(crate) fn jobs_owner_tag(scope: &mut v8::PinScope) -> Option<(String, u64)> {
    resolver_owner_tag(scope)
}

/// Ledger a `Job` against `owner`. A missing ledger (unknown owner) is a no-op.
pub(crate) fn jobs_record_job(owner: &str, generation: u64, id: u64) {
    record_resource(owner, generation, plugin::Resource::Job(id));
}

pub(crate) fn release_resource(owner: &str, generation: u64, resource: &plugin::Resource) -> bool {
    REGISTRY.with(|r| r.borrow_mut().release(owner, generation, resource))
}

fn record_resource(owner: &str, generation: u64, resource: plugin::Resource) -> bool {
    REGISTRY.with(|r| r.borrow_mut().record(owner, generation, resource))
}

/// Generation-gated liveness. Does not expose `REGISTRY`.
pub(crate) fn jobs_owner_is_live(owner: &str, generation: u64) -> bool {
    REGISTRY.with(|r| r.borrow().is_live(owner, generation))
}

/// Clone a plugin context out of the table so a settle may re-enter `PLUGINS`.
/// Does not expose `PLUGINS`.
pub(crate) fn jobs_clone_plugin_context(owner: &str) -> Option<v8::Global<v8::Context>> {
    PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone()))
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

// Canonical framework config templates — the ONE source (no generated copy, no drift gate). Written
// verbatim to the operator's configs/ file on first boot when it is absent (see the admin loader and
// the db loader in INJECTED_STD_PRELUDE). Kept as raw JSON files so they are diffable + reviewable.
pub(crate) const ADMINS_TEMPLATE: &str = include_str!("../config-templates/admins.json");
pub(crate) const ADMIN_GROUPS_TEMPLATE: &str = include_str!("../config-templates/admin_groups.json");
pub(crate) const ADMIN_OVERRIDES_TEMPLATE: &str = include_str!("../config-templates/admin_overrides.json");
pub(crate) const DATABASES_TEMPLATE: &str = include_str!("../config-templates/databases.json");

/// Build the JS that injects `globalThis.__s2_TEMPLATES` (name -> file-content string). Each value is
/// `serde_json::to_string`'d — a JSON string literal is also a valid JS string literal — so the exact
/// file bytes cross into V8 unaltered. Evaluated in every plugin context just BEFORE the engine
/// prelude, so the admin/db loaders can read the template they should write on first boot.
fn config_templates_prelude() -> String {
    let s = |t: &str| serde_json::to_string(t).unwrap_or_else(|_| "\"{}\\n\"".to_string());
    format!(
        "globalThis.__s2_TEMPLATES = {{ \"admins\": {}, \"admin_groups\": {}, \"admin_overrides\": {}, \"databases\": {} }};",
        s(ADMINS_TEMPLATE),
        s(ADMIN_GROUPS_TEMPLATE),
        s(ADMIN_OVERRIDES_TEMPLATE),
        s(DATABASES_TEMPLATE),
    )
}

/// The injected engine-generic prelude, evaluated per plugin context AFTER the native
/// primitives are in place.  Builds the five module globals over the `__s2_*` natives
/// (whose internal names are unchanged) and stashes them at `globalThis.__s2pkg_<name>` for the
/// `__s2require` native to hand back.  The `HookResult`/`Priority`/`Phase` enum globals stay on
/// `globalThis` (ambient, engine-generic).  No game identifiers appear here.
///
/// The body lives in `core/js/prelude.js` and is baked in at COMPILE time by `include_str!` — the
/// binary is still self-contained (nothing is read from disk at runtime, so a missing or corrupt
/// file is a build error, never a boot-time degrade). Keeping it as a `.js` file rather than a Rust
/// string literal is what lets it be linted and edited with JS tooling, and makes a JS-only change
/// reviewable as JS. It is ONE file because the whole body shares one IIFE scope (`(function () {`
/// … `})();`) — splitting it per module would make concatenation order silently load-bearing.
///
/// prelude.js's own file line N is no longer V8 line N now that colors.js is concatenated ahead
/// of it below: V8 line = colors.js's line count + 1 (the joining "\n") + N. Historically file
/// line N WAS V8 line N — the file starts directly at `globalThis.HookResult`, where the old
/// `r#"` literal opened with a newline and shifted every reported line by one.
// colors.js FIRST: it sets globalThis.__s2_colors, which prelude.js's chat and console
// funnels call. Same ordering contract as games/cs2/js (activity.js before pawn.js).
const INJECTED_STD_PRELUDE: &str =
    concat!(include_str!("../js/colors.js"), "\n", include_str!("../js/prelude.js"));

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
        if let Some(generation) = REGISTRY.with(|r| r.borrow().generation_of(&owner)) {
            record_resource(&owner, generation, plugin::Resource::Hook(id));
        }
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
        let owner = resolver_owner_tag(scope);
        if let Some((ref owner, generation)) = owner {
            if !release_resource(owner, generation, &plugin::Resource::Hook(id)) { return; }
        }
        // The combined predicate supersedes the DetourChange the multiplexer returns; ignore it.
        let _change = FRAME.with(|f| f.borrow_mut().unsubscribe(id));
        refresh_detour();
    }));
}

/// The CALLING plugin's `(id, current generation)` for tagging an async resolver, or `None` for a
/// non-plugin context (the shared `HOST` context).  The generation is read from `REGISTRY` — the
/// same books `frame_async_drain` checks — so a tag captured at prelude-eval time (REGISTRY entry
/// present, `PluginInstance` not yet in PLUGINS) is already the real generation.  A later unload
/// (id removed) or reload (generation advanced) then makes the captured tag fail
/// `REGISTRY.is_live` in `frame_async_drain`, which DROPS the continuation instead of resolving it
/// into a disposed/replaced context.
///
/// Reads the current context id (no borrow) then briefly borrows `REGISTRY` — the caller must hold
/// no `REGISTRY` borrow across this (none do: every JS-call site clones its context out first).
fn resolver_owner_tag(scope: &mut v8::PinScope) -> Option<(String, u64)> {
    current_plugin(scope).map(|owner| {
        let generation = plugin_generation(&owner);
        (owner, generation)
    })
}

thread_local! {
    static TIMER_LEASES:std::cell::RefCell<std::collections::HashMap<u64,crate::async_limits::Lease>>=std::cell::RefCell::new(std::collections::HashMap::new());
    static DUE_TIMERS:std::cell::RefCell<std::collections::VecDeque<u64>>=const {std::cell::RefCell::new(std::collections::VecDeque::new())};
    static POLL_CURSOR:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};
    static CALLBACK_CURSOR:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};
    static PARKED_HTTP:std::cell::RefCell<Option<crate::http::FetchCompletion>>=const {std::cell::RefCell::new(None)};
    static PARKED_DB:std::cell::RefCell<Option<crate::db::DbCompletion>>=const {std::cell::RefCell::new(None)};
}
// ---------------------------------------------------------------------------
// WebSocket (client) Task 2: __s2_ws_* natives + signal routing + teardown.
// Mirrors s2_fetch (the connect native)/resolve_fetch (-> resolve_ws_connect)/
// cookies::s2_cookie_on_cached (the subscribe)/cookies::dispatch_pending_cached (-> dispatch_pending_ws_events).
// ---------------------------------------------------------------------------

/// Native `__s2_ws_connect(url) -> Promise<connId>`.  MIRRORS `s2_fetch`'s resolver/ledger/pending
/// block exactly (a `Job` resource — teardown drops its `RESOLVERS` entry before the context
/// disposes, and a completion for an unloaded/reloaded plugin is DROPPED by the async-liveness
/// guard in the drain step, never resolved), except the SAME fresh async id is used as BOTH the
/// connect-resolver id (in `RESOLVERS`) AND the ws connection id (`ws::connect`'s `conn_id`), and
/// the connection is additionally ledgered as a `WsConn` resource (teardown authority) so an
/// unclosed connection is closed even if the plugin never calls `close()`.  Hands off to
/// `crate::ws::connect` (the process-global tokio+tungstenite engine, Task 1) — the calling
/// (main/game) thread never blocks; the Promise resolves on a LATER `frame_async_drain` via
/// `resolve_ws_connect`.
#[cfg(test)]
static TEST_WS_REQUEST_HEADER_CAPACITY: std::sync::Mutex<Option<(usize, usize)>> =
    std::sync::Mutex::new(None);

fn s2_ws_connect(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let owner = resolver_owner_tag(scope);
        let owner_string = owner.as_ref().map(|o| o.0.clone()).unwrap_or_default();
        let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        let url =
            crate::jobs::copy_string(scope, args.get(0), &mut lease).map_err(|e| e.to_string())?;
        let mut headers = Vec::new();
        let headers_key = v8::String::new(scope, "headers").unwrap();
        let obj = v8::Local::<v8::Object>::try_from(args.get(1))
            .ok()
            .and_then(|o| o.get(scope, headers_key.into()))
            .and_then(|v| v8::Local::<v8::Object>::try_from(v).ok());
        if let Some(obj) = obj {
            if let Some(names) = obj.get_own_property_names(scope, Default::default()) {
                for i in 0..names.length() {
                    let Some(k) = names.get_index(scope, i) else {
                        continue;
                    };
                    let Some(v) = obj.get(scope, k) else { continue };
                    let k = crate::jobs::copy_string(scope, k, &mut lease)
                        .map_err(|e| e.to_string())?;
                    let v = crate::jobs::copy_string(scope, v, &mut lease)
                        .map_err(|e| e.to_string())?;
                    crate::jobs::push_request_header(&mut headers, k, v);
                }
            }
        }
        #[cfg(test)]
        {
            let actual = headers.capacity() * std::mem::size_of::<(String, String)>()
                + headers
                    .iter()
                    .map(|(k, v)| k.capacity() + v.capacity())
                    .sum::<usize>()
                + url.capacity();
            *TEST_WS_REQUEST_HEADER_CAPACITY.lock().unwrap() =
                Some((actual, crate::async_limits::domain().jobs.snapshot().bytes));
        }
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        crate::ws::connect_owned(
            id,
            url,
            owner_string,
            owner.as_ref().map_or(0, |o| o.1),
            headers,
            lease,
        )?;
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        if let Some((oid, generation)) = owner {
            record_resource(&oid, generation, plugin::Resource::WsConn(id));
        }
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

/// Resolve (or drop, on the async-liveness guard) a completed `__s2_ws_connect` job in its OWNING
/// plugin's context — MIRRORS `resolve_fetch`'s owner-liveness + context-clone +
/// HandleScope/ContextScope preamble exactly, but resolves with the conn-id `Number` on `Ok`
/// (the plugin's `WebSocket.connect` prelude then wraps it into a handle), or rejects with an
/// `Error` on `Err` (a connect failure — bad host/port/handshake).
fn resolve_ws_connect(
    host: &mut Host,
    entry: &crate::jobs::ResolverEntry,
    id: u64,
    result: Result<(), String>,
) {
    crate::jobs::settle_if_live(&mut host.isolate, &host.context, entry, |scope, resolver| {
        match result {
            Ok(()) => {
                let id_val = v8::Number::new(scope, id as f64);
                resolver.resolve(scope, id_val.into());
            }
            Err(e) => {
                let msg = v8::String::new(scope, &e)
                    .unwrap_or_else(|| v8::String::new(scope, "ws connect error").unwrap());
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Net (raw TCP + UDP client sockets) Task 2: __s2_net_* natives + Uint8Array
// marshalling + signal routing + teardown. MIRRORS the WebSocket spine above
// verbatim (s2_ws_connect/resolve_ws_connect/s2_ws_send/close/on/
// dispatch_pending_ws_events), except payloads are RAW BINARY BYTES — the one
// net-new mechanism is the `Uint8Array <-> Vec<u8>` marshalling (js_bytes_arg /
// bytes_to_uint8array), which COPIES in BOTH directions (a raw backing store /
// pointer NEVER crosses the boundary).
// ---------------------------------------------------------------------------



/// Native `__s2_net_tcp_connect(host, port) -> Promise<connId>`. MIRRORS `s2_ws_connect`'s
/// resolver/`resolver_owner_tag`/ledger(`record_job` + `record_net_conn`)/`RESOLVERS`/`PENDING_JOBS`/
/// `refresh_detour`/return-promise block exactly (ONE fresh async id is BOTH the connect-resolver id
/// AND the net `conn_id`; the connection is ledgered as a `NetConn` so an unclosed socket is dropped
/// at teardown), except the hand-off is `crate::net::connect_tcp`. The calling (game) thread never
/// blocks; the Promise resolves on a LATER `frame_async_drain` via `resolve_net_connect`.
fn s2_net_tcp_connect(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let owner = resolver_owner_tag(scope);
        let owner_string = owner.as_ref().map(|o| o.0.clone()).unwrap_or_default();
        let generation = owner.as_ref().map_or(0, |o| o.1);
        let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        let host =
            crate::jobs::copy_string(scope, args.get(0), &mut lease).map_err(|e| e.to_string())?;
        let port = args.get(1).integer_value(scope).unwrap_or(0) as u16;
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        crate::net::connect_tcp_owned(id, host, port, owner_string, generation, lease)?;
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        if let Some((oid, generation)) = owner {
            record_resource(&oid, generation, plugin::Resource::NetConn(id));
        }
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

/// Native `__s2_net_udp_bind() -> Promise<connId>`. Same block as `s2_net_tcp_connect`, hand-off
/// `crate::net::bind_udp` (a UDP socket bound to an ephemeral local port; the Promise resolves once
/// the socket is bound, or rejects on a bind failure).
fn s2_net_udp_bind(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let owner = resolver_owner_tag(scope);
        let owner_string = owner.as_ref().map(|o| o.0.clone()).unwrap_or_default();
        let generation = owner.as_ref().map_or(0, |o| o.1);
        let lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        crate::net::bind_udp_owned(id, owner_string, generation, lease)?;
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        if let Some((oid, generation)) = owner {
            record_resource(&oid, generation, plugin::Resource::NetConn(id));
        }
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

/// Resolve (or drop, on the async-liveness guard) a completed `__s2_net_tcp_connect`/`_udp_bind` job
/// in its OWNING plugin's context — a verbatim copy of `resolve_ws_connect` (resolves with the
/// conn-id `Number` on `Ok`, rejects with an `Error` on `Err` = a connect/bind failure; the
/// owner-liveness DROP preamble is identical — never resolve into a dead/replaced context).
fn resolve_net_connect(
    host: &mut Host,
    entry: &crate::jobs::ResolverEntry,
    id: u64,
    result: Result<(), String>,
) {
    crate::jobs::settle_if_live(&mut host.isolate, &host.context, entry, |scope, resolver| {
        match result {
            Ok(()) => {
                let id_val = v8::Number::new(scope, id as f64);
                resolver.resolve(scope, id_val.into());
            }
            Err(e) => {
                let msg = v8::String::new(scope, &e)
                    .unwrap_or_else(|| v8::String::new(scope, "net connect error").unwrap());
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
            }
        }
    });
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
        rv.set_int32(schema_offset_cached(&class, &field));
    }));
}

/// The cached `(class, field) → offset` resolver behind `__s2_schema_offset`, callable from Rust.
/// Returns `-1` on any miss (no ops table / null `schema_offset` / interior NULs / class or field not
/// found) and WARNs at most once per key. `class`/`field` are OPAQUE strings — no game identifier
/// appears in core. Extracted so the plugin-declared-call `receiver.via` hop resolves through the
/// SAME cache as JS rather than a second, drifting one (spec §5: "live-resolved, never baked").
pub(crate) fn schema_offset_cached(class: &str, field: &str) -> i32 {
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
    SCHEMA_OFFSETS.with(|c| c.borrow_mut().resolve(class, field, live_raw, live_log))
}

// ---------------------------------------------------------------------------
// E1 entity-liveness: (index, host-id) entity natives — BOOKS-FIRST resolution.
//
// Liveness is decided by the HOST'S BOOKS (`entity_live`), never by reading the
// entity's own (possibly freed) memory. A JS `EntityRef` carries `{index, id}` where
// `id` is a host-minted u64 (f64-safe on the wire); the old engine serial never
// crosses to JS. Raw pointers are used and discarded ENTIRELY WITHIN
// `entity_resolve_ptr` — they NEVER cross the JS boundary.  Only numbers/null/boolean/
// the decode array cross.  This is the core of the EntityRef liveness contract
// (north-star §3.1, Candidate D).
// ---------------------------------------------------------------------------


/// (index-arg, id-arg) → (index, stored engine serial), for shim ops that serial-gate
/// internally. None = the books say not-live — the op is never called (fail-closed).
pub(crate) fn ent_op_serial(scope: &mut v8::PinScope, idx_arg: v8::Local<v8::Value>, id_arg: v8::Local<v8::Value>) -> Option<(i32, i32)> {
    let index = idx_arg.integer_value(scope).unwrap_or(-1) as i32;
    let id = js_ent_id(scope, id_arg);
    let serial = crate::entity_live::engine_serial_for(index, id)?;
    Some((index, serial))
}


// Field-type kind codes moved to `crate::entity` with the natives that match on them.










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

/// Parse a JS EntityRef id (f64 on the wire; host-minted u64). 0 = invalid/never-live.
/// Integral, ≥1, ≤2^53 (exact-f64 range) — anything else fails closed.
pub(crate) fn js_ent_id(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> u64 {
    let n = v.number_value(scope).unwrap_or(0.0);
    if !n.is_finite() || n < 1.0 || n > 9_007_199_254_740_992.0 || n.fract() != 0.0 { return 0; }
    n as u64
}


/// Native `__s2_handle_adopt(handleU32) -> [index, id] | null`. THE raw-handle minting
/// path: decode (pure bit-math) then adopt from the books — engine-serial match yields
/// the table's host id; mismatch/absent → null. A dangling handle field can never mint
/// a live ref (north-star §3.1).
fn s2_handle_adopt(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let handle = args.get(0).integer_value(scope).unwrap_or(0) as u32;
        let (index, serial) = crate::entity::decode_handle(handle);
        let Some(id) = crate::entity_live::adopt(index, serial) else { return };
        let arr = v8::Array::new(scope, 2);
        let iv = v8::Integer::new(scope, index);
        let dv = v8::Number::new(scope, id as f64);
        arr.set_index(scope, 0, iv.into());
        arr.set_index(scope, 1, dv.into());
        rv.set(arr.into());
    }));
}

// Fan-out invocation policy (`fan_out` / `fan_out_collapsing` / `Delivery`) lives in `crate::dispatch`.

/// Deliver a map-start notification to the `Server.onMapStart` subscribers. Called from ffi.rs's
/// `s2script_core_dispatch_map_start` (the shim's INetworkServerService::StartupServer POST hook).
/// Mirrors `dispatch_client_event` verbatim: snapshot (release the mux borrow), `try_borrow_mut`
/// re-entrancy guard, per-subscriber `is_live` + context clone + HandleScope/ContextScope/TryCatch +
/// WARN-on-throw. Notify-only — each handler is called with the single String `map` and its return
/// is ignored.
///
/// `dispatch_map_start` = **bookkeeping** (the breadcrumb map name here; `entity_live::
/// clear_for_map_transition` in `ffi.rs`) + the JS fan-out. Replaying the bookkeeping half would
/// wipe the entity books a frame INTO the new map, killing every ref minted since map start — hence
/// the split (contract §6.1). The shim queues `replay_map_start`, never this entry.
pub(crate) fn dispatch_map_start(map: &str) -> Delivery {
    crate::crash::breadcrumb::set_map(map);
    replay_map_start(map)
}

/// The JS half of `dispatch_map_start`, and NOTHING else — safe to run a frame late.
pub(crate) fn replay_map_start(map: &str) -> Delivery {
    let snap = MAP_MUX.with(|m| m.borrow().snapshot(""));
    fan_out(&snap, "dispatch_map_start", Instrument::none(), |tc| {
        Some(vec![v8::String::new(tc, map)?.into()])
    })
}

/// Fan a cvar change out to `Server.onCvarChange` subscribers for that exact name AND for `"*"`.
/// NOTIFY-only (the engine has already applied the value). Mirrors `dispatch_map_start`: snapshot
/// first so no mux borrow is held across JS, per-handler TryCatch so one thrower cannot stop the
/// rest, and a `try_borrow_mut` graceful-skip so a handler that itself sets a cvar (re-entering this
/// dispatch) is skipped rather than double-borrowing the isolate.
///
/// This path carries NO bookkeeping, so `dispatch` and `replay` are the same work; both names exist
/// so the shim's queue has one uniform `replay_*` vocabulary and a future bookkeeping half has an
/// obvious home that the replay cannot reach.
pub(crate) fn dispatch_cvar_change(name: &str, new_value: &str, old_value: &str) -> Delivery {
    replay_cvar_change(name, new_value, old_value)
}

/// The JS half of `dispatch_cvar_change` — safe to run a frame late (the engine has ALREADY applied
/// the value; this is pure notification).
pub(crate) fn replay_cvar_change(name: &str, new_value: &str, old_value: &str) -> Delivery {
    // A "*" subscriber hears every cvar; a named one hears only its own. Both snapshots are taken
    // before any JS runs, and a "*" fire must not deliver twice to the wildcard subscribers.
    let mut snap = CVAR_MUX.with(|m| m.borrow().snapshot(name));
    if name != "*" { snap.extend(CVAR_MUX.with(|m| m.borrow().snapshot("*"))); }
    fan_out(&snap, &format!("dispatch_cvar_change('{}')", name), Instrument::none(), |tc| {
        Some(vec![
            v8::String::new(tc, name)?.into(),
            v8::String::new(tc, new_value)?.into(),
            v8::String::new(tc, old_value)?.into(),
        ])
    })
}

/// E1 repair sweep (north-star §7, the E0-V4 contingency): armed by the map-start books
/// clear, runs ONCE at the next SIMULATING frame — reconciles the books against a
/// chunk-walk snapshot of live identity slots (system-owned memory only; the shim's
/// ent_snapshot op). Covers entities created before StartupServer POST / before the
/// listener attached (first boot map, preallocated controllers). Fail-closed: with no
/// op the books stay purely listener-fed and an unseen entity reads null.
/// ASSUMPTION TO CONFIRM AT THE LIVE GATE (E0-V4): the first simulating frame is a
/// verified-clean moment — the new map's entity system is live and populated.
pub(crate) fn entity_repair_sweep_if_armed(simulating: bool) {
    if !simulating { return; }
    if !crate::entity_live::take_repair_armed() { return; }
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
    let Some(snapshot) = ops.ent_snapshot else { return };
    const CAP: usize = 32768;   // MAX_TOTAL_ENTITIES ceiling (CS2: 16k entries + headroom)
    let mut idxs = vec![0i32; CAP];
    let mut sers = vec![0i32; CAP];
    let n = snapshot(idxs.as_mut_ptr(), sers.as_mut_ptr(), CAP as i32);
    let n = (n.max(0) as usize).min(CAP);
    let pairs: Vec<(i32, i32)> = (0..n).map(|i| (idxs[i], sers[i])).collect();
    crate::entity_live::repair_reconcile(&pairs);
}

/// Deliver a precache-manifest-build notification to the `Sound.onPrecache` subscribers. Called
/// from ffi.rs's `s2script_core_dispatch_precache` (the shim's CGameRulesGameSystem::
/// OnPrecacheResource MANUAL hook, which stashes the live IResourceManifest* around this call so
/// the `sound_precache_add` op can AddResource into it — block-scoped: the stash is cleared when
/// the hook returns, so a handler must use its PrecacheContext synchronously). Mirrors
/// `dispatch_map_start` verbatim: snapshot (release the mux borrow), `try_borrow_mut` re-entrancy
/// guard, per-subscriber `is_live` + context clone + HandleScope/ContextScope/TryCatch +
/// WARN-on-throw. Notify-only — each handler is called with NO args (the prelude wrapper builds
/// the PrecacheContext) and its return is ignored.
///
/// NOT DEFERRABLE, and deliberately so: this is the one notify-only entry that fails the deferral
/// test on semantics rather than on its signature. The manifest the handlers write into is
/// block-scoped shim-side (`s_currentPrecacheManifest`, cleared when the hook returns) AND consumed
/// by the engine the moment the hook returns, so a replayed handler's `sound_precache_add` would
/// write into a null-or-freed `IEntityResourceManifest*` and mean nothing even if it were safe. Its
/// `Delivery` is discarded here on purpose — a re-entrant precache keeps today's graceful skip.
pub(crate) fn dispatch_precache() {
    let snap = PRECACHE_MUX.with(|m| m.borrow().snapshot(""));
    let _ = fan_out(&snap, "dispatch_precache", Instrument::none(), |_tc| Some(vec![]));
}






/// Shared logging helper for named WARNs in the engine-op natives and the loader.
pub(crate) fn log_warn(msg: &str) {
    if let Some(l) = LOGGER.with(|l| l.get()) {
        if let Ok(cs) = CString::new(msg) {
            l(0, cs.as_ptr());
        }
    }
}

/// Native `__s2require(name) -> object|null` — resolves first-party builtin specifiers to their
/// per-context module globals under BOTH spellings: the consolidated `@s2script/sdk/<cap>` and the
/// legacy `@s2script/<cap>` (e.g. `"@s2script/sdk/frame"` or `"@s2script/frame"` → `globalThis.__s2pkg_frame`).
/// Bare `@s2script/sdk` (no capability) maps to `globalThis.__s2pkg_sdk`, the engine-generic
/// authoring barrel. ORDER IS LOAD-BEARING: `@s2script/sdk/` is stripped BEFORE the shorter
/// `@s2script/`, which also matches `@s2script/sdk/<cap>` and would strip to the garbage cap
/// `sdk/<cap>`. Non-`@s2script/` specifiers → `null` (the JS `__s2_require` shim resolves those as
/// inter-plugin deps).  A retired/unknown name (global undefined) → `null`. Engine-generic: no
/// module list hardcoded; `@s2script/cs2` maps to `__s2pkg_cs2` via the plain `@s2script/` strip.
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
        // Dual-prefix (packaging consolidation): a builtin resolves as BOTH the consolidated
        // `@s2script/sdk/<cap>` and the legacy `@s2script/<cap>` — both map to `__s2pkg_<cap>`.
        // ORDER IS LOAD-BEARING: the shorter `@s2script/` also matches `@s2script/sdk/entity`
        // and would strip to `sdk/entity` → `__s2pkg_sdk/entity` garbage — try `@s2script/sdk/`
        // FIRST. Bare `@s2script/sdk` (no capability) falls to the plain strip → `__s2pkg_sdk`,
        // the engine-generic authoring barrel (populated by the prelude). Still generic — no
        // module list hardcoded; `@s2script/cs2` keeps riding the plain `@s2script/` strip.
        let Some(rest) = name
            .strip_prefix("@s2script/sdk/")
            .or_else(|| name.strip_prefix("@s2script/"))
        else {
            return;
        };
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
pub fn set_plugin_imports(id: &str, decls: Vec<crate::interfaces::ImportSpec>) {
    let new_names: Vec<String> = decls.iter().map(|decl| decl.name.clone()).collect();
    let old_names = IFACES.with(|r| r.borrow_mut().set_imports(id, decls));
    if let Some(generation) = REGISTRY.with(|r| r.borrow().generation_of(id)) {
        for name in old_names { release_resource(id, generation, &plugin::Resource::Import(name)); }
        for name in new_names { record_resource(id, generation, plugin::Resource::Import(name)); }
    }
}

thread_local! {
    /// plugin_id → the manifest's `publishes` map. The SOLE source of an interface's version
    /// (spec §4.3): JS never carries one. Set by the loader before load_plugin_js.
    static PLUGIN_PUBLISHES: std::cell::RefCell<
        std::collections::HashMap<String, std::collections::HashMap<String, crate::loader::PublishDecl>>
    > = std::cell::RefCell::new(std::collections::HashMap::new());

    /// plugin_id → interface names it tried to publish but never declared. Recorded when
    /// `s2_iface_publish` refuses one, and read by `reconcile_publishes` so the load fails.
    /// Without this, a plugin with NO `publishes` map at all could call `publishInterface`,
    /// have it silently refused, and still run — the declared→owned check alone never sees it.
    static UNDECLARED_PUBLISHES: std::cell::RefCell<
        std::collections::HashMap<String, Vec<String>>
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Record a plugin's declared `publishes` map (from its manifest) before its context loads.
pub fn set_plugin_publishes(
    plugin_id: &str,
    publishes: std::collections::HashMap<String, crate::loader::PublishDecl>,
) {
    PLUGIN_PUBLISHES.with(|p| { p.borrow_mut().insert(plugin_id.to_string(), publishes); });
}

thread_local! {
    static PLUGIN_INTEROP: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashMap<String,crate::interop::Contract>>> = Default::default();
}
pub fn set_plugin_interop(
    id: &str,
    contracts: std::collections::HashMap<String, crate::interop::Contract>,
) {
    PLUGIN_INTEROP.with(|m| {
        m.borrow_mut().insert(id.into(), contracts);
    });
}

/// Drop a plugin's publishes map (teardown).
pub fn clear_plugin_publishes(plugin_id: &str) {
    PLUGIN_PUBLISHES.with(|p| { p.borrow_mut().remove(plugin_id); });
    PLUGIN_INTEROP.with(|p| { p.borrow_mut().remove(plugin_id); });
    UNDECLARED_PUBLISHES.with(|p| { p.borrow_mut().remove(plugin_id); });
}

/// Post-load reconciliation (design spec §4.3): did `plugin_id` actually publish every interface
/// its manifest declares, and own each one?  Returns `Err(reason)` if not; the LOADER turns that
/// into a teardown, which is what makes an inconsistent manifest "fail the load" rather than load
/// green.
///
/// Why here and not at publish time: `s2_iface_publish` already refuses an UNDECLARED name, but
/// that only covers one direction.  A typo — manifest declares `@x/greeter`, code publishes
/// `@x/greetr` — refuses the stray publish and then loads happily with `@x/greeter` unpublished,
/// leaving consumers to discover it as `InterfaceUnavailable` at runtime.  That is exactly the
/// silent-drift class this design exists to remove, so the manifest is treated as a contract:
/// declare it and you must publish it.  (A plugin that publishes CONDITIONALLY therefore cannot
/// declare the interface — a deliberate constraint, same posture as `publishes ⇒ types`.)
///
/// Note this also catches the §4.8 loser: a second producer's publish is refused, so its declared
/// name is owned by the incumbent, and reconciliation fails its load — which tears down its
/// context and, with it, the `PublishHandle` its prelude handed back.
pub fn reconcile_publishes(plugin_id: &str) -> Result<(), String> {
    // Direction 1 — published but never declared. `s2_iface_publish` already refused the
    // registration; failing the LOAD too is what makes forgetting `publishes` entirely loud
    // rather than a log line under a running plugin.
    let mut undeclared: Vec<String> = UNDECLARED_PUBLISHES.with(|p| {
        p.borrow().get(plugin_id).cloned().unwrap_or_default()
    });
    undeclared.sort();
    undeclared.dedup();

    // Direction 2 — declared but not owned after the load.
    let declared: Vec<String> = PLUGIN_PUBLISHES.with(|p| {
        p.borrow().get(plugin_id).map(|m| m.keys().cloned().collect()).unwrap_or_default()
    });
    let mut missing: Vec<String> = declared
        .into_iter()
        .filter(|name| {
            IFACES.with(|r| {
                // Not published at all, or published by someone else → not honoured.
                r.borrow().lookup(name).map(|e| e.producer_id != plugin_id).unwrap_or(true)
            })
        })
        .collect();
    missing.sort();   // deterministic message (HashMap iteration order is not stable)

    if undeclared.is_empty() && missing.is_empty() {
        return Ok(());
    }

    // Report BOTH directions. A typo trips both at once — published ["@x/greetr"] + declared
    // ["@x/greeter"] unowned — and naming the pair together IS the diagnosis. Reporting only the
    // first would hand the author half of it.
    let mut parts: Vec<String> = Vec::new();
    if !undeclared.is_empty() {
        parts.push(format!(
            "published {:?} without declaring {} in the manifest `publishes`",
            undeclared,
            if undeclared.len() == 1 { "it" } else { "them" },
        ));
    }
    if !missing.is_empty() {
        parts.push(format!(
            "declares {:?} in `publishes` but does not own {} after load",
            missing,
            if missing.len() == 1 { "it" } else { "them" },
        ));
    }
    Err(format!(
        "{} (a typo in the publishInterface name trips both; otherwise: a publish that never \
         ran, a missing s2script.publishes entry, or another plugin already owns the interface)",
        parts.join("; "),
    ))
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
    kind: *const c_char, type_name: *const c_char, inner: *const c_char, size: c_int,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() || cls.is_null() || name.is_null() || kind.is_null() { return; }
        let catalog = unsafe { &mut *(ctx as *mut crate::schema_catalog::Catalog) };
        let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        let opt = |p: *const c_char| if p.is_null() { None } else { Some(s(p)) };
        catalog.add_field(&s(cls), &s(name), offset as i32, &s(kind),
                          opt(type_name).as_deref(), opt(inner).as_deref(), size as i32);
    }));
}

/// C-ABI callback invoked by the shim's `schema_enumerate` once per ENUMERATOR.
extern "C" fn cb_emit_enum(
    ctx: *mut c_void, enum_name: *const c_char, size: c_int, enumerator: *const c_char, value: i64,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ctx.is_null() || enum_name.is_null() || enumerator.is_null() { return; }
        let catalog = unsafe { &mut *(ctx as *mut crate::schema_catalog::Catalog) };
        let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        catalog.add_enum(&s(enum_name), size as i32, &s(enumerator), value);
    }));
}

/// Native `__s2_schema_dump(path: string, enumsPath?: string) -> boolean`.
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
        // Optional: enums go in their OWN file. The catalog serializes as a bare class map, so a
        // sibling section would change that shape for every existing consumer.
        let enums_path = if args.length() >= 2 && !args.get(1).is_null_or_undefined() {
            Some(args.get(1).to_rust_string_lossy(scope))
        } else {
            None
        };
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else {
            log_warn("WARN: __s2_schema_dump: no engine ops table");
            return;
        };
        let Some(enumerate) = ops.schema_enumerate else {
            log_warn("WARN: __s2_schema_dump: schema_enumerate not wired in ops");
            return;
        };
        let mut catalog = crate::schema_catalog::Catalog::new();
        let ok = enumerate(&mut catalog as *mut _ as *mut c_void, cb_emit_class, cb_emit_field, cb_emit_enum);
        if ok == 0 || catalog.class_count() == 0 {
            log_warn("WARN: __s2_schema_dump: schema not ready (no classes) — try again once a map is live");
            return;
        }
        if let Err(e) = std::fs::write(&path, catalog.to_json()) {
            log_warn(&format!("WARN: __s2_schema_dump: write '{}' failed: {}", path, e));
            return;
        }
        // A requested enums file that cannot be written is a FAILURE, not a partial success: the
        // caller asked for both, and returning true would leave a stale enum table paired with a
        // fresh catalog — the two would disagree about which enums exist.
        if let Some(ep) = enums_path {
            if let Err(e) = std::fs::write(&ep, catalog.enums_json()) {
                log_warn(&format!("WARN: __s2_schema_dump: write '{}' failed: {}", ep, e));
                return;
            }
            log_warn(&format!("__s2_schema_dump: {} enums -> {}", catalog.enum_count(), ep));
        }
        rv.set_bool(true);
    }));
}

/// Set a named native function on `global_obj` in `scope`.  Small helper used by
/// `install_natives` to keep the per-context install table declarative.
/// `__s2_v8_heap_used()` -> Number (bytes). The V8 isolate's used_heap_size — the analog of
/// .NET's `GC.GetTotalMemory`. Isolate-wide (all plugin contexts share one isolate); per-plugin
/// memory would use V8's per-context MeasureMemory API. Degrades to -1 on panic.
fn s2_v8_heap_used(
    _scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(-1.0);
        let stats = _scope.get_heap_statistics();
        rv.set_double(stats.used_heap_size() as f64);
    }));
}

/// `__s2_v8_gc()` — force a full GC (V8 low-memory-notification), the analog of `GC.Collect()`.
/// Dev/benchmark instrumentation. Degrades to a no-op on panic.
fn s2_v8_gc(
    _scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        _scope.low_memory_notification();
    }));
}

/// `__s2_hrtime_ns()` -> Number — nanoseconds since a process-start monotonic base (f64 is exact to
/// ~104 days). The analog of .NET's `Stopwatch.GetTimestamp`; lets a plugin time a single
/// sub-microsecond op directly instead of loop-amortizing against Date.now (ms).
fn s2_hrtime_ns(
    _scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(-1.0);
        static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let base = BASE.get_or_init(std::time::Instant::now);
        rv.set_double(base.elapsed().as_nanos() as f64);
    }));
}

pub(crate) fn set_native(
    scope: &mut v8::PinScope,
    global_obj: v8::Local<v8::Object>,
    name: &str,
    cb: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let key = v8::String::new(scope, name).unwrap();
    let func = v8::Function::new(scope, cb).unwrap();
    global_obj.set(scope, key.into(), func.into());
}

/// `__s2_iface_publish(name, implObj)` — the producer registers an interface it DECLARED.
/// The version is injected from the plugin's manifest `publishes` map (spec §4.3): a plugin may
/// never type a version string. Refuses (WARN + return, no throw — publish is producer-side) when:
/// the name is absent from the manifest, or another live producer already owns it (spec §4.8).
fn s2_iface_publish(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        if args.length() < 2 {
            return;
        }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(impl_obj) = v8::Local::<v8::Object>::try_from(args.get(1)) else {
            log_warn(&format!(
                "WARN: iface_publish('{}'): impl is not an object",
                name
            ));
            return;
        };
        let Some(owner) = current_plugin(scope) else {
            log_warn("WARN: iface_publish: no current plugin");
            return;
        };

        // The manifest is the sole source of the version. An undeclared name never registers.
        let Some(decl) =
            PLUGIN_PUBLISHES.with(|p| p.borrow().get(&owner).and_then(|m| m.get(&name)).cloned())
        else {
            log_warn(&format!(
                "WARN: iface_publish('{}'): plugin '{}' did not declare this interface in its \
                 manifest `publishes` — refusing",
                name, owner
            ));
            // Remember it so `reconcile_publishes` fails this plugin's LOAD. The declared→owned
            // check cannot see this case: a plugin that declares nothing has nothing to reconcile,
            // so without this it would run on with its interface silently unpublished.
            UNDECLARED_PUBLISHES.with(|p| {
                p.borrow_mut()
                    .entry(owner.clone())
                    .or_default()
                    .push(name.clone());
            });
            return;
        };

        let generation = REGISTRY
            .with(|r| r.borrow().generation_of(&owner))
            .unwrap_or(0);

        // Enumerate own function properties → method names + capture Globals.
        let mut method_names: Vec<String> = Vec::new();
        let mut captured: Vec<(String, v8::Global<v8::Function>)> = Vec::new();
        if let Some(prop_names) = impl_obj.get_own_property_names(scope, Default::default()) {
            for i in 0..prop_names.length() {
                let Some(key) = prop_names.get_index(scope, i) else {
                    continue;
                };
                let Some(val) = impl_obj.get(scope, key) else {
                    continue;
                };
                if let Ok(f) = v8::Local::<v8::Function>::try_from(val) {
                    let m = key.to_rust_string_lossy(scope);
                    method_names.push(m.clone());
                    captured.push((m, v8::Global::new(scope.as_ref(), f)));
                }
            }
        }

        if let Some(contract) = &decl.contract {
            if !live_interop_context(scope, &owner) {
                throw_named(scope, "InterfaceUnavailable", &owner);
                return;
            }
            if contract.validate().is_err()
                || method_names.len() != contract.metadata.methods.len()
                || method_names
                    .iter()
                    .any(|m| !contract.metadata.methods.contains_key(m))
            {
                throw_named(
                    scope,
                    "InterfaceContractError",
                    &format!("{} implementation methods disagree with metadata", name),
                );
                return;
            }
        }

        // Register FIRST: a REJECTED publish must not leave method Globals behind (a rejected
        // second producer's functions would otherwise shadow the incumbent's in IFACE_METHODS,
        // which is keyed by name).
        if let Err(e) = IFACES.with(|r| {
            r.borrow_mut().publish(
                &name,
                &decl.version,
                &decl.types_sha256,
                &owner,
                generation,
                method_names,
            )
        }) {
            log_warn(&format!("WARN: iface_publish('{}'): {}", name, e));
            return;
        }
        for (m, g) in captured {
            IFACE_METHODS.with(|mm| {
                mm.borrow_mut().insert((name.clone(), m), g);
            });
        }
        // Same-owner publish is an in-place replacement in InterfaceRegistry, so replace its one
        // ownership row too. A first publish simply has no prior row to release.
        release_resource(
            &owner,
            generation,
            &plugin::Resource::Interface(name.clone()),
        );
        record_resource(
            &owner,
            generation,
            plugin::Resource::Interface(name.clone()),
        );
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

/// Select the declared protocol without depending on provider availability during load buffering.
fn s2_iface_verified_import(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        rv.set_bool(current_plugin(scope).is_some_and(|id| {
            PLUGIN_INTEROP.with(|p| p.borrow().get(&id).is_some_and(|m| m.contains_key(&name)))
        }));
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
        let avail = current_plugin(scope).map_or(false, |id| {
            IFACES.with(|r| r.borrow().is_available(&id, &name))
                && checked_contract(&id, &name).is_ok()
        });
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
fn s2_iface_call(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
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
            crate::interfaces::CallTarget::Unavailable => {
                throw_named(scope, "InterfaceUnavailable", &name);
                return;
            }
            crate::interfaces::CallTarget::VersionMismatch => {
                throw_named(scope, "InterfaceVersionMismatch", &name);
                return;
            }
            crate::interfaces::CallTarget::TypesMismatch => {
                throw_named(scope, "InterfaceTypesMismatch", &name);
                return;
            }
            crate::interfaces::CallTarget::Ok => {}
        }

        let contract = match checked_contract(&consumer, &name) {
            Ok(c) => c,
            Err(e) => {
                throw_named(scope, e, &name);
                return;
            }
        };
        let _depth = if contract.is_some() {
            let Some(g) = InteropGuard::enter(scope) else {
                return;
            };
            Some(g)
        } else {
            None
        };
        if contract.is_some() && !live_interop_context(scope, &consumer) {
            throw_named(scope, "InterfaceUnavailable", &consumer);
            return;
        }
        let method_schema = contract
            .as_ref()
            .and_then(|c| c.metadata.methods.get(&method))
            .cloned();
        if contract.is_some() && method_schema.is_none() {
            throw_named(scope, "InterfaceUnknownMethod", &method);
            return;
        }
        if let Some((producer, generation)) = IFACES.with(|r| r.borrow().producer_of(&name)) {
            if !REGISTRY.with(|r| r.borrow().is_live(&producer, generation)) {
                throw_named(scope, "InterfaceUnavailable", &name);
                return;
            }
        }
        // Marshal args (the 3rd arg, an array) OUT of the consumer context to a JSON String.
        let args_json = match if let Some(schema) = &method_schema {
            strict_json(scope, args.get(2))
                .filter(|(_, v)| schema.accepts_args(v))
                .map(|(s, _)| s)
        } else {
            iface_to_json(scope, args.get(2))
        } {
            Some(s) => s,
            None => {
                throw_named(
                    scope,
                    "InterfaceValueNotSerializable",
                    &format!("{}.{} args", name, method),
                );
                return;
            }
        };

        // Producer context + method Global — extract into owned locals so no IFACES/IFACE_METHODS/PLUGINS
        // borrow is held across the V8 context-switch or the method call (borrow discipline).
        let Some((producer_id, producer_generation)) = IFACES.with(|r| r.borrow().producer_of(&name)) else {
            throw_named(scope, "InterfaceUnavailable", &name);
            return;
        };
        let method_g =
            IFACE_METHODS.with(|m| m.borrow().get(&(name.clone(), method.clone())).cloned());
        let Some(method_g) = method_g else {
            throw_named(scope, "InterfaceUnavailable", &name);
            return;
        };
        let Some(g_ctx) =
            PLUGINS.with(|p| p.borrow().get(&producer_id).map(|pi| pi.context.clone()))
        else {
            throw_named(scope, "InterfaceUnavailable", &name);
            return;
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
                for i in 0..arr.length() {
                    argv.push(arr.get_index(tc, i)?);
                }
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
                            let msg = tc
                                .exception()
                                .map(|e| e.to_rust_string_lossy(&*tc))
                                .unwrap_or_else(|| "producer method threw".into());
                            Outcome::Threw(msg)
                        }
                        Some(ret) => {
                            if let Some(schema) = &method_schema {
                                if observe_thenable(tc, ret) {
                                    Outcome::NotSerializable
                                } else if ret.is_undefined() {
                                    if matches!(schema.result, crate::interop::Schema::Void) {
                                        Outcome::Void
                                    } else {
                                        Outcome::NotSerializable
                                    }
                                } else {
                                    match strict_json(tc, ret)
                                        .filter(|(_, v)| schema.result.accepts(v))
                                    {
                                        Some((json, _)) => Outcome::Ok(json),
                                        None => Outcome::NotSerializable,
                                    }
                                }
                            } else if ret.is_undefined() {
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

        // Reentrant notifications/thenables may retire either participant. The context slot
        // retains the caller's original generation, even if its registry ID was replaced.
        if contract.is_some()
            && (!live_interop_context(scope, &consumer)
                || !REGISTRY.with(|r| r.borrow().is_live(&producer_id, producer_generation))
                || IFACES.with(|r| r.borrow().producer_of(&name))
                    != Some((producer_id, producer_generation)))
        {
            throw_named(scope, "InterfaceUnavailable", &name);
            return;
        }
        // Back in the consumer context: map the outcome to a return value or a single named throw.
        match outcome {
            Outcome::Ok(json) => match iface_from_json(scope, &json) {
                Some(v) => rv.set(v),
                None => throw_named(
                    scope,
                    "InterfaceValueNotSerializable",
                    &format!("{}.{} return", name, method),
                ),
            },
            Outcome::Void => rv.set_undefined(),
            Outcome::NotSerializable => throw_named(
                scope,
                "InterfaceValueNotSerializable",
                &format!("{}.{} return", name, method),
            ),
            Outcome::Threw(msg) => throw_named(
                scope,
                "InterfaceCallError",
                &format!("{}.{}: {}", name, method, msg),
            ),
            Outcome::Internal => throw_named(scope, "InterfaceUnavailable", &name),
        }
    }));
}

/// `__s2_iface_on(name, event, handler) -> subId` — the consumer subscribes to a producer event.
/// Stores the handler Global keyed by a fresh sub_id; records the Subscriber in the registry (tagged
/// with the consumer's (id, generation)); ledgers `EventSub(subId)` on the consumer.
fn s2_iface_on(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        let name = args.get(0).to_rust_string_lossy(scope);
        let event = args.get(1).to_rust_string_lossy(scope);
        let Ok(handler) = v8::Local::<v8::Function>::try_from(args.get(2)) else {
            return;
        };
        let Some(consumer) = current_plugin(scope) else {
            return;
        };
        let token = args.get(3).integer_value(scope).unwrap_or(0) as u64;
        if token != 0 && !interop_lifetime::subscription_authorized(scope, &name, token) {
            throw_named(scope, "InterfaceRegistrationClosed", "expired or unrelated attachment");
            return;
        }
        let contract = match checked_contract(&consumer, &name) {
            Ok(c) => c,
            Err(e) => {
                throw_named(scope, e, &name);
                return;
            }
        };
        if let Some(contract) = contract {
            if !live_interop_context(scope, &consumer) {
                throw_named(scope, "InterfaceUnavailable", &consumer);
                return;
            }
            if !interop_lifetime::subscription_authorized(scope, &name, token) {
                throw_named(scope, "InterfaceRegistrationClosed", "on requires load or its synchronous attachment");
                return;
            }
            if !IFACES.with(|r| r.borrow().is_available(&consumer, &name)) {
                throw_named(scope, "InterfaceUnavailable", &name);
                return;
            }
            if !contract.metadata.forwards.contains_key(&event) {
                throw_named(scope, "InterfaceUnknownForward", &event);
                return;
            }
        }
        let generation = REGISTRY
            .with(|r| r.borrow().generation_of(&consumer))
            .unwrap_or(0);
        let sub_id = NEXT_SUB_ID.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        });

        let ok = IFACES.with(|r| {
            r.borrow_mut().add_subscriber(
                &name,
                crate::interfaces::Subscriber {
                    sub_id,
                    consumer_id: consumer.clone(),
                    consumer_gen: generation,
                    event,
                },
            )
        });
        if !ok {
            return;
        } // interface not published → no-op (degrade)

        let g = v8::Global::new(scope.as_ref(), handler);
        IFACE_SUBS.with(|m| {
            m.borrow_mut().insert(sub_id, g);
        });
        record_resource(&consumer, generation, plugin::Resource::EventSub(sub_id));
        interop_lifetime::track_subscription(token, sub_id);
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
        if !live_interop_context(scope, &consumer) {
            throw_named(scope, "InterfaceUnavailable", &consumer);
            return;
        }
        let dropped = IFACES.with(|r| r.borrow_mut().remove_subscribers_by_consumer_on(&consumer, &name, &event));
        IFACE_SUBS.with(|m| { let mut mm = m.borrow_mut(); for id in &dropped { mm.remove(id); } });
        if let Some(generation) = REGISTRY.with(|r| r.borrow().generation_of(&consumer)) {
            for id in dropped { release_resource(&consumer, generation, &plugin::Resource::EventSub(id)); }
        }
    }));
}

/// Notifications and synchronous decision forwards share authority, copying and snapshot traversal.
fn s2_iface_emit(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    s2_iface_forward(scope, args, rv, false);
}
fn s2_iface_dispatch(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    s2_iface_forward(scope, args, rv, true);
}
fn s2_iface_forward(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
    decision: bool,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        let name = args.get(0).to_rust_string_lossy(scope);
        let event = args.get(1).to_rust_string_lossy(scope);
        if let Some(owner) = current_plugin(scope) {
            if !live_interop_context(scope, &owner) {
                throw_named(scope, "InterfaceUnavailable", &owner);
                return;
            }
        }
        let contract = published_contract(&name);
        let published = IFACES.with(|r| r.borrow().producer_of(&name));
        let producer = published
            .as_ref()
            .map(|(id, _)| id.as_str())
            .unwrap_or_default();
        if decision && contract.is_none() {
            throw_named(
                scope,
                "InterfaceContractError",
                "dispatch requires a protocol 2 provider",
            );
            return;
        }
        let _depth = if contract.is_some() {
            let Some(g) = InteropGuard::enter(scope) else {
                return;
            };
            Some(g)
        } else {
            None
        };
        if contract.is_some() {
            let owner = current_plugin(scope);
            let published = IFACES.with(|r| r.borrow().producer_of(&name));
            if !published.as_ref().map_or(false, |(id, g)| {
                Some(id) == owner.as_ref() && REGISTRY.with(|r| r.borrow().is_live(id, *g))
            }) {
                throw_named(scope, "InterfaceProviderMismatch", &name);
                return;
            }
        }
        let forward = contract
            .as_ref()
            .and_then(|c| c.metadata.forwards.get(&event));
        if contract.is_some() && forward.is_none() {
            throw_named(scope, "InterfaceUnknownForward", &event);
            return;
        }
        if forward.is_some_and(|f| decision == (f.kind == "notification")) {
            throw_named(
                scope,
                "InterfaceForwardKindMismatch",
                &format!("{}.{}", name, event),
            );
            return;
        }
        // Validate and copy the entire payload before executing any listener.
        let mut payload_json = match if let Some(forward) = forward {
            strict_json(scope, args.get(2))
                .filter(|(_, v)| forward.payload.accepts(v))
                .map(|(s, _)| s)
        } else {
            iface_to_json(scope, args.get(2))
        } {
            Some(s) => s,
            None => {
                if contract.is_some() {
                    throw_named(
                        scope,
                        "InterfaceValueNotSerializable",
                        &format!("{}.{} payload", name, event),
                    );
                    return;
                }
                log_warn(&format!(
                    "WARN: iface_emit('{}','{}'): payload not serializable",
                    name, event
                ));
                return;
            }
        };
        // Compute live subscriber ids (IFACES borrow released before entering any consumer context).
        let is_live = |id: &str, gen: u64| REGISTRY.with(|r| r.borrow().is_live(id, gen));
        let sub_ids = IFACES.with(|r| r.borrow().live_subscriber_ids(&name, &event, &is_live));

        let mut collapsed = HookResult::Continue;
        for sub_id in sub_ids {
            if contract.is_some() && IFACES.with(|r| r.borrow().producer_of(&name)) != published {
                break;
            }
            if !IFACES.with(|r| {
                r.borrow()
                    .live_subscriber_ids(&name, &event, &is_live)
                    .contains(&sub_id)
            }) {
                continue;
            }
            // Collect all info (brief borrows; all released before the ContextScope).
            let handler_g = IFACE_SUBS.with(|m| m.borrow().get(&sub_id).cloned());
            let Some(handler_g) = handler_g else {
                continue;
            };
            let consumer = IFACES.with(|r| r.borrow().consumer_of_sub(&name, sub_id));
            let Some(consumer) = consumer else {
                continue;
            };
            if contract.is_some() && checked_contract(&consumer, &name).is_err() {
                continue;
            }
            let Some(g_ctx) =
                PLUGINS.with(|p| p.borrow().get(&consumer).map(|pi| pi.context.clone()))
            else {
                continue;
            };

            // Enter the consumer's context and call the handler with a fresh copy of the payload.
            let ctx_local = v8::Local::new(scope, &g_ctx);
            let cscope = &mut v8::ContextScope::new(scope, ctx_local);
            let mut tc_storage = v8::TryCatch::new(cscope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            if let Some(payload) = iface_from_json(tc, &payload_json) {
                let f = v8::Local::new(tc, &handler_g);
                let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                let result = f.call(tc, recv, &[payload]);
                if let Some(result) = result {
                    if contract.is_some() && observe_thenable(tc, result) {
                        log_warn(&format!("InterfaceSynchronousContractError: provider {} consumer {} forward {}.{} returned a thenable",producer,consumer,name,event));
                    } else if decision && !live_interop_context(tc, &consumer) {
                        log_warn(&format!("InterfaceUnavailable: provider {} consumer {} forward {}.{} returned from a stale generation",producer,consumer,name,event));
                    } else if decision {
                        let validated = strict_json(tc, result).and_then(|(_, value)| {
                            let (action, patch) = forward?.response(&value)?;
                            let updated = if let Some(patch) = patch {
                                let mut payload: serde_json::Value =
                                    serde_json::from_str(&payload_json).ok()?;
                                payload.as_object_mut()?.extend(patch.clone());
                                if !forward?.payload.accepts(&payload) {
                                    return None;
                                }
                                Some(serde_json::to_string(&payload).ok()?)
                            } else {
                                None
                            };
                            Some((action, updated))
                        });
                        if let Some((action, updated)) = validated {
                            if let Some(updated) = updated {
                                payload_json = updated;
                            }
                            collapsed = collapsed.max(action);
                            if action == HookResult::Stop {
                                break;
                            }
                        } else {
                            log_warn(&format!("InterfaceInvalidForwardResponse: provider {} consumer {} forward {}.{}",producer,consumer,name,event));
                        }
                    }
                } else {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!(
                        "WARN: iface_emit('{}','{}') provider '{}' consumer '{}': {}",
                        name, event, producer, consumer, msg
                    ));
                }
            }
            // tc, tc_storage, cscope drop here (TryCatch absorbs any pending exception).
        }
        if decision {
            if IFACES.with(|r| r.borrow().producer_of(&name)) != published
                || !published
                    .as_ref()
                    .is_some_and(|(id, g)| REGISTRY.with(|r| r.borrow().is_live(id, *g)))
            {
                throw_named(scope, "InterfaceUnavailable", &name);
                return;
            }
            if forward.is_some_and(|f| f.kind == "hook") {
                rv.set_double(collapsed as u32 as f64);
            } else {
                let result_json = format!(
                    "{{\"result\":{},\"payload\":{}}}",
                    collapsed as u32, payload_json
                );
                if let Some(result) = iface_from_json(scope, &result_json) {
                    rv.set(result);
                }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Slice 5D.1: game-event subscribe / unsubscribe / accessor natives.
// ---------------------------------------------------------------------------





/// `__s2_usercmd_subscribe(handler)` — subscribe a JS fn to `UserCmd.onRun` (usercmd primitive Task 2).
/// Owner-tracked, fixed mux key "onRun" (usercmd has no name dimension). On the FIRST-EVER subscribe (the mux was empty), calls the (Task 3) `usercmd_hook_install`
/// engine op so the shim lazily installs its per-tick input-processing detour — mirrors `s2_entity_listener_on`'s
/// lazy-install trigger (zero overhead when no plugin subscribes). Degrade-never-crash: no op → the
/// subscribe still records, the engine just never delivers.
fn s2_usercmd_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        // WHOLE-STORE emptiness, not the helper's per-channel `was_first`: the input detour is
        // installed once for the process, not once per channel. Sampled BEFORE subscribing.
        let first_ever = USERCMD_MUX.with(|m| m.borrow().is_empty());
        let Some((sub_id, _)) = subscribe_into(scope, &args, &USERCMD_MUX, "onRun", 0) else { return };
        if first_ever {
            if let Some(func) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.usercmd_hook_install) {
                let _ = func();
            }
        }
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_usercmd_read(field) -> number` — read a scalar/angle/impulse field of the CURRENT usercmd
/// (usercmd primitive Task 3). `field`: 0 forwardMove, 1 sideMove, 2 upMove, 3 pitch, 4 yaw, 5 roll, 6
/// impulse (the ENGINE-GENERIC numeric enum — the shim alone maps it onto the Source2-shared
/// usercmd.proto nesting). Valid only during a `UserCmd.onRun` dispatch (the shim's `s_currentUserCmd`
/// is block-scoped, mirrors `s_currentDamageInfo`). Degrades to `0` with no op / out of dispatch.
fn s2_usercmd_read(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 1 { return; }
        let field = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.usercmd_read else { return };
        rv.set_double(func(field));
    }));
}

/// `__s2_usercmd_write(field, value)` — write a scalar/angle/impulse field of the CURRENT usercmd
/// (usercmd primitive Task 3). Same `field` enum as `__s2_usercmd_read`. No-op with no op / out of
/// dispatch; the shim guards `is_repeated()`/`cpp_type()` before any protobuf `Set*` (an `is_repeated`
/// scalar `Set*` aborts the whole process). No auto-subtick-clear (the spike verdict: a coarse write
/// alone takes effect) — see `__s2_usercmd_clear_subtick` for the separate opt-in helper.
fn s2_usercmd_write(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let field = args.get(0).int32_value(scope).unwrap_or(-1);
        let value = args.get(1).number_value(scope).unwrap_or(0.0);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.usercmd_write else { return };
        func(field, value);
    }));
}

/// `__s2_usercmd_read_buttons() -> bigint` — the current usercmd's pressed-button mask
/// (a 64-bit button-state value; usercmd primitive Task 3). Degrades to `0n` with no
/// op / out of dispatch (never `undefined` — `buttons` is always a `bigint` per the spec).
fn s2_usercmd_read_buttons(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let value: u64 = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let func = ops.usercmd_read_buttons?;
            Some(func())
        })().unwrap_or(0);
        let bi = v8::BigInt::new_from_u64(scope, value);
        rv.set(bi.into());
    }));
}

/// `__s2_usercmd_write_buttons(mask)` — overwrite the current usercmd's pressed-button mask (usercmd
/// primitive Task 3). `mask` is a JS `bigint` (any numeric-representable value is coerced via
/// `to_big_int`; a non-bigint/non-numeric argument degrades to `0`). No-op with no op / out of dispatch.
fn s2_usercmd_write_buttons(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let mask = args.get(0).to_big_int(scope).map(|bi| bi.u64_value().0).unwrap_or(0);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.usercmd_write_buttons else { return };
        func(mask);
    }));
}

/// `__s2_usercmd_clear_subtick()` — drop the current usercmd's subtick moves (usercmd primitive Task
/// 3). Exposed as an OPTIONAL helper (`Cmd.clearSubtickMoves()`) — the spike verdict found a coarse
/// `forwardMove`/`sideMove`/`upMove` write alone already takes effect, so the write ops never call this
/// automatically. No-op with no op / out of dispatch / no subtick moves on this build.
fn s2_usercmd_clear_subtick(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.usercmd_clear_subtick else { return };
        func();
    }));
}





/// `__s2_map_start_subscribe(handler)` — subscribe a JS fn to the map-start event. Owner-tracked
/// (mirrors `__s2_chat_on_message`); fixed mux key "". The handler receives the map name string.
fn s2_map_start_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Some((sub_id, _first)) = subscribe_into(scope, &args, &MAP_MUX, "", 0) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_precache_subscribe(handler)` — subscribe a JS fn to the precache-manifest-build event.
/// Owner-tracked (mirrors `__s2_map_start_subscribe`); fixed mux key "". The handler is called
/// with no args during `dispatch_precache`; the `@s2script/sound` prelude wrapper constructs the
/// block-scoped PrecacheContext.
fn s2_precache_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Some((sub_id, _first)) = subscribe_into(scope, &args, &PRECACHE_MUX, "", 0) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

// `__s2_cookie_on_cached` / `__s2_cookie_dispatch_cached` moved to `crate::cookies`.

thread_local! {
    /// `OnTakeDamagePost` must not write through `DamageInfo.damage` (spec: info is read-only).
    static DAMAGE_WRITES_FROZEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn with_damage_writes_frozen(frozen: bool, f: impl FnOnce()) {
    DAMAGE_WRITES_FROZEN.with(|c| {
        let prev = c.get();
        c.set(frozen);
        f();
        c.set(prev);
    });
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

/// `__s2_damage_write_float(offset, value)` — write m_flDamage etc. during a pre-hook (modify/block).
/// No-op if no op, or during `OnTakeDamagePost` (info is read-only after the original ran).
fn s2_damage_write_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if DAMAGE_WRITES_FROZEN.with(|c| c.get()) {
            return;
        }
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

/// `__s2_cvar_set(name, value) -> boolean` — write a cvar through ICvar now (not ServerCommand).
/// False if the op is missing, the cvar is absent, or the string cannot become that type.
fn s2_cvar_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.cvar_set else { return };
        let Ok(cn) = CString::new(name) else { return };
        let Ok(cv) = CString::new(value) else { return };
        rv.set_bool(crate::nest::with_outbound(&args, || f(cn.as_ptr(), cv.as_ptr())) != 0);
    }));
}

/// Native `__s2_convar_register(name, helpOrNull, flags, type, defaultStr, minOrNull, maxOrNull) -> i32`.
/// Over the `convar_register` op. Degrades to 0 with no op; never throws (catch_unwind + safe default).
fn s2_convar_register(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(func) = ops.and_then(|o| o.convar_register) else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(c_name) = std::ffi::CString::new(name) else { return };
        // helpOrNull / minOrNull / maxOrNull: JS null/undefined -> C null pointer.
        let opt_cstr = |scope: &mut v8::PinScope, v: v8::Local<v8::Value>| -> Option<std::ffi::CString> {
            if v.is_null_or_undefined() { return None; }
            std::ffi::CString::new(v.to_rust_string_lossy(scope)).ok()
        };
        let c_help = opt_cstr(scope, args.get(1));
        let flags = args.get(2).number_value(scope).unwrap_or(0.0) as u64;
        let ty = args.get(3).int32_value(scope).unwrap_or(-1);
        let def = args.get(4).to_rust_string_lossy(scope);
        let Ok(c_def) = std::ffi::CString::new(def) else { return };
        let c_min = opt_cstr(scope, args.get(5));
        let c_max = opt_cstr(scope, args.get(6));
        let r = unsafe {
            func(c_name.as_ptr(),
                 c_help.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                 flags, ty, c_def.as_ptr(),
                 c_min.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                 c_max.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()))
        };
        rv.set_int32(r);
    }));
}

/// Native `__s2_transmit_set(index, serial, viewerSlots[]) -> boolean` — replace the calling
/// plugin's visibility rule for the entity: transmit ONLY to the given viewer slots (empty array
/// = hidden from everyone). The u64 mask is folded core-side from the JS number array (no BigInt
/// on any boundary). The shim op serial-gates at registration; stale ref / full table / missing
/// op / disabled descriptor degrade to `false`. Other owners' entries on this index with a
/// DIFFERENT serial are evicted after the op accepts (ours is the live serial; theirs are dead).
fn s2_transmit_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return };
        let mut mask: u64 = 0;
        for i in 0..arr.length() {
            let Some(v) = arr.get_index(scope, i) else { return };
            let slot = v.integer_value(scope).unwrap_or(-1);
            if !(0..64).contains(&slot) { return; }   // the JS wrapper throws first; belt-and-braces
            mask |= 1u64 << (slot as u32);
        }
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        // Candidate merged mask: AND with every OTHER owner's same-serial rule on this index.
        let merged = TRANSMIT_RULES.with(|r| {
            r.borrow().fold_except(&owner, &index, TransmitRule { serial, mask }, |ru| ru.serial == serial).mask
        });
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.transmit_set) else { return };
        if f(index, serial, merged) == 0 { return; }
        TRANSMIT_RULES.with(|r| {
            let mut map = r.borrow_mut();
            // Evict stale (different-serial) entries on this index — the op just validated `serial`
            // is the live one, so any other serial in this slot belongs to a dead entity.
            map.evict_at(&index, |ru| ru.serial != serial);
            map.insert(owner, index, TransmitRule { serial, mask });
        });
        rv.set_bool(true);
    }));
}

/// Native `__s2_transmit_reset(index, serial) -> boolean` — remove the calling plugin's rule for
/// the entity (the serial must match the recorded rule), then re-push the remaining merge (or
/// clear the shim entry when this was the last rule).
fn s2_transmit_reset(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let removed = TRANSMIT_RULES.with(|r| {
            let mut map = r.borrow_mut();
            match map.get(&owner, &index) {
                Some(ru) if ru.serial == serial => { map.remove(&owner, &index); true }
                _ => false,
            }
        });
        if removed {
            transmit_recompute_and_push(index);
            rv.set_bool(true);
        }
    }));
}

/// Native `__s2_transmit_reset_all()` — remove all of the calling plugin's rules.
fn s2_transmit_reset_all(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        transmit_remove_owner(&owner);
    }));
}

/// Native `__s2_transmit_stats() -> {snapshots, entries, bitsCleared, nsLast, nsMax} | null`.
/// Null when the op is unassigned (old shim) — the capability is absent, not zero.
fn s2_transmit_stats(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.transmit_stats) else { return };
        let mut out = [0u64; 5];
        f(out.as_mut_ptr());
        let obj = v8::Object::new(scope);
        for (i, name) in ["snapshots", "entries", "bitsCleared", "nsLast", "nsMax"].iter().enumerate() {
            let k = v8::String::new(scope, name).unwrap();
            let v = v8::Number::new(scope, out[i] as f64);
            obj.set(scope, k.into(), v.into());
        }
        rv.set(obj.into());
    }));
}

/// Native `__s2_voice_audible_set(sender, receiversArray) -> boolean` — replace the calling
/// plugin's hearability rule for `sender`: audible ONLY to the given receiver slots (an empty array
/// = audible to nobody, which is NOT the same as having no rule). The u64 mask is folded core-side
/// from the JS number array (no BigInt on any boundary). Degraded voice validation / a sender out of
/// range / a missing op all degrade to `false`.
fn s2_voice_audible_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let sender = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        if !(0..64).contains(&sender) { return; }
        let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(1)) else { return };
        let mut mask: u64 = 0;
        for i in 0..arr.length() {
            let Some(v) = arr.get_index(scope, i) else { return };
            let slot = v.integer_value(scope).unwrap_or(-1);
            if !(0..64).contains(&slot) { return; }
            mask |= 1u64 << (slot as u32);
        }
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        // Candidate merged mask: AND this rule with every OTHER owner's rule for the same sender.
        let merged = VOICE_RULES.with(|r| r.borrow().fold_except(&owner, &sender, mask, |_| true));
        // PUSH FIRST, PERSIST ONLY ON SUCCESS — the s2_transmit_set ordering. Inserting before the
        // push would leave core holding a rule the shim rejected (e.g. voice degraded), so the two
        // would disagree and a later unrelated recompute would silently apply a phantom rule.
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_audible_set else { return };
        if f(sender, merged) == 0 { return; }
        VOICE_RULES.with(|r| r.borrow_mut().insert(owner, sender, mask));
        rv.set_bool(true);
    }));
}

/// Native `__s2_voice_audible_clear(sender) -> boolean` — drops only the CALLER's rule, then
/// re-pushes the remaining merge (or clears the shim entry when this was the last rule).
fn s2_voice_audible_clear(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let sender = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        if !(0..64).contains(&sender) { return; }
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let removed = VOICE_RULES.with(|r| r.borrow_mut().remove(&owner, &sender).is_some());
        if removed { voice_recompute_and_push(sender); }
        rv.set_bool(removed);
    }));
}

/// Native `__s2_voice_reset_all()` — remove all of the calling plugin's rules.
/// A dedicated native, not a 64-iteration loop in JS: mirrors `__s2_transmit_reset_all`, and
/// `voice_remove_owner` already recomputes exactly the senders that were touched.
fn s2_voice_reset_all(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        voice_remove_owner(&owner);
    }));
}

/// Native `__s2_voice_audible_stats() -> {calls, entries, rewrites} | null`.
/// Null when the op is unassigned (old shim) — the capability is ABSENT, not zero.
fn s2_voice_audible_stats(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_audible_stats else { return };
        let mut out = [0u64; 3];
        if f(out.as_mut_ptr()) == 0 { return; }
        let obj = v8::Object::new(scope);
        for (k, v) in [("calls", out[0]), ("entries", out[1]), ("rewrites", out[2])] {
            let Some(key) = v8::String::new(scope, k) else { return };
            let val = v8::Number::new(scope, v as f64);
            obj.set(scope, key.into(), val.into());
        }
        rv.set(obj.into());
    }));
}

/// `__s2_plugins_list() -> string` — JSON array of `{id, loaded, state}` for `sm plugins list` /
/// `Plugins.list()`. `state` is one of running|loading|waiting|failed|unloaded (L1 lifecycle v2);
/// `loaded` is kept and is exactly `state === "running"`.
fn s2_plugins_list(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let items: Vec<serde_json::Value> = crate::loader::plugin_list().into_iter()
            .map(|(id, state)| serde_json::json!({ "id": id, "loaded": state == "running", "state": state }))
            .collect();
        let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
        if let Some(js) = v8::String::new(scope, &json) { rv.set(js.into()); }
    }));
}


/// `__s2_topmenu_add_category(name)` — append a tab if absent (order = insertion; deduped).
/// Title defaults to `name`. `addTab` is the form that sets a distinct label.
fn s2_topmenu_add_category(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        topmenu_ensure_tab(name, None);
    }));
}

/// `__s2_topmenu_add_tab(id, title)` — register/replace a dashboard tab. Deduped by id;
/// a later call updates the title and keeps insertion order.
fn s2_topmenu_add_tab(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        let title = args.get(1).to_rust_string_lossy(scope);
        topmenu_ensure_tab(id, Some(title));
    }));
}

/// `__s2_topmenu_add_item(category, id, name, flags, onSelectFn, sheets?)` — register/replace an item
/// owned by current_plugin. Auto-creates the category (order hint). Mirrors s2_concommand's owner+gen+Global
/// store. `sheets` defaults to `["admin"]`; only the known sheet names are accepted.
fn s2_topmenu_add_item(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 5 { return; }
        let category = args.get(0).to_rust_string_lossy(scope);
        let id = args.get(1).to_rust_string_lossy(scope);
        let name = args.get(2).to_rust_string_lossy(scope);
        let flags = args.get(3).integer_value(scope).unwrap_or(0);
        let func_local = match v8::Local::<v8::Function>::try_from(args.get(4)) { Ok(f) => f, Err(_) => return };
        let sheets = if args.length() < 6 || args.get(5).is_undefined() {
            vec!["admin".to_string()]
        } else {
            let Ok(sheet_array) = v8::Local::<v8::Array>::try_from(args.get(5)) else {
                throw_named(scope, "TopMenuInvalidSheet", "sheets must be an array of \"admin\" or \"menu\"");
                return;
            };
            let mut sheets = Vec::with_capacity(sheet_array.length() as usize);
            for index in 0..sheet_array.length() {
                let Some(value) = sheet_array.get_index(scope, index) else {
                    throw_named(scope, "TopMenuInvalidSheet", &format!("sheets[{}] is missing", index));
                    return;
                };
                if !value.is_string() {
                    throw_named(scope, "TopMenuInvalidSheet", &format!("sheets[{}] must be a string", index));
                    return;
                }
                let sheet = value.to_rust_string_lossy(scope);
                if sheet != "admin" && sheet != "menu" {
                    throw_named(scope, "TopMenuInvalidSheet", &format!("unknown sheet name {:?}", sheet));
                    return;
                }
                sheets.push(sheet);
            }
            sheets
        };
        let on_select = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = plugin_generation(&owner);
        topmenu_ensure_tab(category.clone(), None);
        // Reuse the existing seq on a re-add (reload) so positions stay stable; else take the next counter.
        let seq = TOPMENU_ITEMS.with(|m| m.borrow().get(&id).map(|it| it.seq))
            .unwrap_or_else(|| TOPMENU_SEQ.with(|c| { let s = c.get(); c.set(s + 1); s }));
        TOPMENU_ITEMS.with(|m| m.borrow_mut().insert(id, TopMenuItem { category, name, flags, sheets, owner, generation, seq, on_select }));
    }));
}

/// `__s2_topmenu_snapshot() -> { categories: string[], tabs: [{id, title}], items: [...] }`
/// (metadata only). `categories` is tab ids in registration order (compat); `tabs` carries titles.
fn s2_topmenu_snapshot(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (cats, tabs): (Vec<String>, Vec<serde_json::Value>) = TOPMENU_CATEGORIES.with(|c| {
            let tabs = c.borrow();
            (
                tabs.iter().map(|t| t.id.clone()).collect(),
                tabs.iter().map(|t| serde_json::json!({ "id": t.id, "title": t.title })).collect(),
            )
        });
        // Sort by seq → items render in registration order (stable across restarts), not random HashMap order.
        let items: Vec<serde_json::Value> = TOPMENU_ITEMS.with(|m| {
            let b = m.borrow();
            let mut entries: Vec<(&String, &TopMenuItem)> = b.iter().collect();
            entries.sort_by_key(|(_, it)| it.seq);
            entries.into_iter().map(|(id, it)| {
                serde_json::json!({ "id": id, "category": it.category, "name": it.name, "flags": it.flags, "sheets": it.sheets })
            }).collect()
        });
        let obj = serde_json::json!({ "categories": cats, "tabs": tabs, "items": items });
        // serialize to a JS value via the JSON string round-trip (the established snapshot pattern).
        if let Some(s) = v8::String::new(scope, &obj.to_string()) {
            if let Some(parsed) = v8::json::parse(scope, s) { rv.set(parsed); }
        }
    }));
}

/// `__s2_topmenu_select(id, slot)` — QUEUE a select for post-drain dispatch (never synchronous — a menu
/// onSelect calls this under the isolate borrow).
fn s2_topmenu_select(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        let slot = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        TOPMENU_PENDING.with(|q| q.borrow_mut().push((id, slot)));
    }));
}

/// Fan out queued TopMenu selects to each item's owner context. Called from ffi.rs AFTER
/// frame_async_drain() (HOST free). Mirrors cookies::dispatch_pending_cached / dispatch_concommand.
pub(crate) fn dispatch_pending_topmenu_select() {
    let pending: Vec<(String, i32)> = TOPMENU_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() { return; }
    for (id, slot) in pending {
        // snapshot (owner, gen, Global) — release TOPMENU_ITEMS borrow before entering a context.
        let entry = TOPMENU_ITEMS.with(|m| m.borrow().get(&id).map(|it| (it.owner.clone(), it.generation, it.on_select.clone())));
        let Some((owner, gen, global)) = entry else { continue };   // stale id -> no-op
        if !REGISTRY.with(|r| r.borrow().is_live(&owner, gen)) { continue; }
        let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.context.clone())) else { continue };
        HOST.with(|h| {
            let Ok(mut borrow) = h.try_borrow_mut() else { return };
            let Some(host) = borrow.as_mut() else { return };
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let slot_val: v8::Local<v8::Value> = v8::Number::new(tc, slot as f64).into();
            let func = v8::Local::new(tc, &global);
            if func.call(tc, recv, &[slot_val]).is_none() {
                let msg = tc.exception().map(|e| e.to_rust_string_lossy(&*tc)).unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_pending_topmenu_select('{}'): {}", id, msg));
            }
        });
    }
}

/// Read a JS array's first 3 elements as `[f32; 3]`. Non-array / missing/short-array elements
/// read as `0.0` — a defensive default (a malformed `start`/`end`/`mins`/`maxs` arg degrades to a
/// zero component rather than a native panic).
fn read_vec3(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> [f32; 3] {
    let mut out = [0f32; 3];
    if let Ok(arr) = v8::Local::<v8::Array>::try_from(v) {
        for i in 0..3u32 {
            if let Some(el) = arr.get_index(scope, i) {
                out[i as usize] = el.number_value(scope).unwrap_or(0.0) as f32;
            }
        }
    }
    out
}

/// Construct `new Vector(x, y, z)` via the injected `__s2pkg_math.Vector` constructor, looked up
/// fresh from the calling context's global (the trace native holds no cached class reference).
/// Falls back to `undefined` if `@s2script/math` isn't installed on this context (defensive; the
/// trace module always sits alongside math in the prelude, so this should not happen in practice).
fn build_vector<'s>(scope: &mut v8::PinScope<'s, '_>, x: f32, y: f32, z: f32) -> v8::Local<'s, v8::Value> {
    let val: Option<v8::Local<'s, v8::Value>> = (|| {
        let global = scope.get_current_context().global(scope);
        let pkg_key = v8::String::new(scope, "__s2pkg_math")?;
        let pkg = global.get(scope, pkg_key.into())?;
        let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
        let ctor_key = v8::String::new(scope, "Vector")?;
        let ctor_val = pkg.get(scope, ctor_key.into())?;
        let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
        let xv = v8::Number::new(scope, x as f64);
        let yv = v8::Number::new(scope, y as f64);
        let zv = v8::Number::new(scope, z as f64);
        ctor.new_instance(scope, &[xv.into(), yv.into(), zv.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
    })();
    match val {
        Some(v) => v,
        None => v8::undefined(scope).into(),
    }
}

/// Construct `new EntityRef(index, id)` via the injected `__s2pkg_entity.EntityRef` constructor —
/// `id` is the HOST-MINTED books id (an f64 on the wire), never a raw engine serial. The framework
/// mints refs by adopting a decoded handle / slot into the books (a raw handle/serial never crosses
/// to JS). Falls back to `null` if `@s2script/entity` isn't installed on this context.
pub(crate) fn build_entity_ref<'s>(scope: &mut v8::PinScope<'s, '_>, index: i32, id: u64) -> v8::Local<'s, v8::Value> {
    let val: Option<v8::Local<'s, v8::Value>> = (|| {
        let global = scope.get_current_context().global(scope);
        let pkg_key = v8::String::new(scope, "__s2pkg_entity")?;
        let pkg = global.get(scope, pkg_key.into())?;
        let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
        let ctor_key = v8::String::new(scope, "EntityRef")?;
        let ctor_val = pkg.get(scope, ctor_key.into())?;
        let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
        let idx_v = v8::Integer::new(scope, index);
        let id_v = v8::Number::new(scope, id as f64);
        ctor.new_instance(scope, &[idx_v.into(), id_v.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
    })();
    match val {
        Some(v) => v,
        None => v8::null(scope).into(),
    }
}

/// `__s2_trace(startArr, endArr, minsArr, maxsArr, interactsWith, interactsExclude, ignoreIdx,
/// ignoreSerial) -> TraceHit` — the ray-trace slice's sole native, over the `trace_shape` engine op
/// (`CNavPhysicsInterface::TraceShape`, RTTI-resolved shim-side; ENGINE-GENERIC, no CS2 names here).
///
/// Degrade-never-crash: no `trace_shape` op (vtable unresolved, or the shim isn't wired at all —
/// e.g. every in-isolate test) builds a MISS `TraceHit` (`didHit:false, fraction:1, endPos:end,
/// normal:(0,0,0), entity:null, allSolid:false`) — `endPos` defaults to the requested `end` so a
/// degraded trace still reports a sensible endpoint. The op itself returning 0 (unavailable at
/// call time) degrades identically.
///
/// The hit entity crosses back ONLY as a raw `hitEntHandle` int (`GetRefEHandle().ToInt()`, or -1
/// for no hit) — never a raw pointer. `hitEntHandle < 0` → `entity: null`; otherwise the handle is
/// decoded (pure bit-math, mirrors `__s2_handle_decode`) and validated live (`entity_resolve_ptr`,
/// the same check `EntityRef.isValid()` performs) before constructing a serial-gated `EntityRef` —
/// a same-frame stale handle (should not happen, but defensive) degrades to `null` rather than a
/// ref that instantly reads dead.
fn s2_trace(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let start = read_vec3(scope, args.get(0));
        let end   = read_vec3(scope, args.get(1));
        let mins  = read_vec3(scope, args.get(2));
        let maxs  = read_vec3(scope, args.get(3));
        // TraceMask values are JS numbers well under 2^53 (the largest composite mask sets bit 21);
        // `number_value` -> `u64` round-trips exactly.
        let interacts_with    = args.get(4).number_value(scope).unwrap_or(0.0) as u64;
        let interacts_exclude = args.get(5).number_value(scope).unwrap_or(0.0) as u64;
        // E1: arg 6/7 = the ignore entity's (index, host-id); translate to the engine serial the
        // trace_shape op expects. A miss (no ignore entity / dead ref) → (-1, -1) = "no ignore entity".
        let (ignore_idx, ignore_serial) = ent_op_serial(scope, args.get(6), args.get(7))
            .map_or((-1 as c_int, -1 as c_int), |(i, s)| (i as c_int, s as c_int));

        let ops = ENGINE_OPS.with(|o| o.get());
        let (did_hit, fraction, endpos, normal, all_solid, hit_ent_handle) =
            match ops.and_then(|o| o.trace_shape) {
                Some(func) => {
                    let mut out = S2TraceResult {
                        did_hit: 0, fraction: 1.0, endpos: [0.0; 3], normal: [0.0; 3],
                        all_solid: 0, hit_ent_handle: -1,
                    };
                    let ok = func(
                        start.as_ptr(), end.as_ptr(), mins.as_ptr(), maxs.as_ptr(),
                        interacts_with, interacts_exclude, ignore_idx, ignore_serial,
                        &mut out as *mut S2TraceResult,
                    );
                    if ok != 0 {
                        (out.did_hit != 0, out.fraction, out.endpos, out.normal, out.all_solid != 0, out.hit_ent_handle)
                    } else {
                        (false, 1.0, end, [0.0; 3], false, -1) // op present but unavailable -> MISS
                    }
                }
                None => (false, 1.0, end, [0.0; 3], false, -1), // no op at all (e.g. every in-isolate test) -> MISS
            };

        let entity_val: v8::Local<v8::Value> = if hit_ent_handle < 0 {
            v8::null(scope).into()
        } else {
            let (index, serial) = crate::entity::decode_handle(hit_ent_handle as u32);
            match crate::entity_live::adopt(index, serial) {
                Some(id) => build_entity_ref(scope, index, id),   // books-adopted (raw serial never crosses)
                None => v8::null(scope).into(),                   // absent/mismatched books → null (fail-closed)
            }
        };
        let end_pos_val = build_vector(scope, endpos[0], endpos[1], endpos[2]);
        let normal_val  = build_vector(scope, normal[0], normal[1], normal[2]);

        let obj = v8::Object::new(scope);
        if let Some(k) = v8::String::new(scope, "didHit") {
            let v = v8::Boolean::new(scope, did_hit);
            obj.set(scope, k.into(), v.into());
        }
        if let Some(k) = v8::String::new(scope, "fraction") {
            let v = v8::Number::new(scope, fraction as f64);
            obj.set(scope, k.into(), v.into());
        }
        if let Some(k) = v8::String::new(scope, "endPos") { obj.set(scope, k.into(), end_pos_val); }
        if let Some(k) = v8::String::new(scope, "normal") { obj.set(scope, k.into(), normal_val); }
        if let Some(k) = v8::String::new(scope, "entity") { obj.set(scope, k.into(), entity_val); }
        if let Some(k) = v8::String::new(scope, "startSolid") {
            let v = v8::Boolean::new(scope, all_solid);
            obj.set(scope, k.into(), v.into());
        }
        rv.set(obj.into());
    }));
}





/// Native `__s2_user_message_create(name) -> int` (1 ok / 0 fail). Over the `user_message_create` op
/// (FindNetworkMessagePartial + AllocateMessage into the shim's single-target). Degrades to 0 with no
/// op / a null-bearing name.
fn s2_user_message_create(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let name = args.get(0).to_rust_string_lossy(scope);
        let cn = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(f) = ops.and_then(|o| o.user_message_create) {
            rv.set_int32(f(cn.as_ptr()));
        }
    }));
}

/// Native `__s2_user_message_set_int(field, value) -> int`. Reflection set by cpp_type (shim-side).
/// Degrades to 0 with no op / a null-bearing field.
fn s2_user_message_set_int(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let field = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).integer_value(scope).unwrap_or(0);
        let fc = match std::ffi::CString::new(field) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(f) = ops.and_then(|o| o.user_message_set_int) {
            rv.set_int32(f(fc.as_ptr(), value));
        }
    }));
}

/// Native `__s2_user_message_set_float(field, value) -> int`. Reflection SetFloat/SetDouble. Degrades
/// to 0 with no op / a null-bearing field.
fn s2_user_message_set_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let field = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).number_value(scope).unwrap_or(0.0);
        let fc = match std::ffi::CString::new(field) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(f) = ops.and_then(|o| o.user_message_set_float) {
            rv.set_int32(f(fc.as_ptr(), value));
        }
    }));
}

/// Native `__s2_user_message_set_string(field, value) -> int`. Reflection SetString. Degrades to 0
/// with no op / a null-bearing field or value.
fn s2_user_message_set_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let field = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).to_rust_string_lossy(scope);
        let fc = match std::ffi::CString::new(field) { Ok(c) => c, Err(_) => return };
        let vc = match std::ffi::CString::new(value) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(f) = ops.and_then(|o| o.user_message_set_string) {
            rv.set_int32(f(fc.as_ptr(), vc.as_ptr()));
        }
    }));
}

/// Native `__s2_user_message_set_bool(field, value) -> int`. Reflection SetBool. `value` is 0/1.
/// Degrades to 0 with no op / a null-bearing field.
fn s2_user_message_set_bool(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let field = args.get(0).to_rust_string_lossy(scope);
        let value = args.get(1).integer_value(scope).unwrap_or(0) as i32;
        let fc = match std::ffi::CString::new(field) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(f) = ops.and_then(|o| o.user_message_set_bool) {
            rv.set_int32(f(fc.as_ptr(), value));
        }
    }));
}

/// Native `__s2_user_message_send(slotsArrayOrNull) -> boolean`. Over the `user_message_send` op.
/// arg0 null/undefined -> broadcast (`func(null, -1)`); an array -> collect its ints into a `Vec<i32>`
/// and pass `(ptr, len)`. Returns `true` iff the op returned 1 (delivered to >=1 real client).
/// Degrades to `false` with no op.
fn s2_user_message_send(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.user_message_send) else { return };
        let arg0 = args.get(0);
        if arg0.is_null_or_undefined() {
            rv.set_bool(crate::nest::with_outbound(&args, || f(std::ptr::null(), -1)) == 1);
            return;
        }
        let slots_arr = match v8::Local::<v8::Array>::try_from(arg0) { Ok(a) => a, Err(_) => return };
        let n = slots_arr.length();
        let mut slots: Vec<i32> = Vec::with_capacity(n as usize);
        for i in 0..n {
            let s = match slots_arr.get_index(scope, i) {
                Some(v) => v.integer_value(scope).unwrap_or(-1) as i32,
                None => -1,
            };
            slots.push(s);
        }
        rv.set_bool(crate::nest::with_outbound(&args, || f(slots.as_ptr(), slots.len() as i32)) == 1);
    }));
}

// UserMessage interception (usermsg-hook slice): the 8 `__s2_usermsg_*` natives moved to
// `crate::usermsg`, alongside their mux, dispatch and teardown.


/// Native `__s2_collision_activate(index, serial) -> boolean`. Serial-gated; over the
/// `collision_activate` op (CCollisionProperty partition registration). Degrades to `false` with no
/// op / a stale ref. Never throws.
fn s2_collision_activate(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.collision_activate) { rv.set_bool(func(index, serial) != 0); }
    }));
}


/// Native `__s2_sound_emit(soundName, entIndex, entSerial, slotsArray, volume) -> number`. Over the
/// `sound_emit` op. Reads the JS slot array into a `Vec<i32>` (mirrors `__s2_user_message_send`); a
/// non-array slots arg -> an empty set (the op returns 0 — caller requested no recipients). An
/// all-bot-skipped non-empty request still calls the engine shim-side (plays to nobody). Returns the
/// SndOpEventGuid as a uint32 number, 0 = failed. Degrades to 0 with no op; never throws.
fn s2_sound_emit(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_uint32(0);
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.sound_emit) else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(c_name) = std::ffi::CString::new(name) else { return };
        let ent_index = args.get(1).integer_value(scope).unwrap_or(0) as i32;
        // E1: arg 2 = the source entity's host-id. id 0 = worldspawn / no-entity sentinel → engine
        // serial -1 ("no serial gate", entIndex used directly shim-side). An id present but not live
        // is a dead entity → fail closed (rv stays 0, no emit).
        let ent_id = js_ent_id(scope, args.get(2));
        let ent_serial = if ent_id == 0 { -1 } else {
            match crate::entity_live::engine_serial_for(ent_index, ent_id) { Some(s) => s, None => return }
        };
        let mut slots: Vec<i32> = Vec::new();
        if let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(3)) {
            let n = arr.length();
            slots.reserve(n as usize);
            for i in 0..n {
                let s = match arr.get_index(scope, i) {
                    Some(v) => v.integer_value(scope).unwrap_or(-1) as i32,
                    None => -1,
                };
                slots.push(s);
            }
        }
        let volume = args.get(4).number_value(scope).unwrap_or(1.0) as f32;
        let guid = f(c_name.as_ptr(), ent_index, ent_serial, slots.as_ptr(), slots.len() as i32, volume);
        rv.set_uint32(guid as u32);
    }));
}

/// Native `__s2_sound_precache_add(path) -> boolean`. Over the `sound_precache_add` op — valid only
/// during a precache-hook dispatch (block-scoped; the shim's manifest stash is null otherwise).
/// Degrades to `false` with no op / no active manifest / a NUL in the path. Never throws.
fn s2_sound_precache_add(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.sound_precache_add) else { return };
        let path = args.get(0).to_rust_string_lossy(scope);
        // Optional: enums go in their OWN file. The catalog serializes as a bare class map, so a
        // sibling section would change that shape for every existing consumer.
        let enums_path = if args.length() >= 2 && !args.get(1).is_null_or_undefined() {
            Some(args.get(1).to_rust_string_lossy(scope))
        } else {
            None
        };
        let Ok(c_path) = std::ffi::CString::new(path) else { return };
        rv.set_bool(f(c_path.as_ptr()) == 1);
    }));
}





/// Native `__s2_output_subscribe(classname, output, handler)`. Subscribes a JS fn to `Entity.onOutput`
/// (entity-I/O slice); owner-tracked in `OUTPUT_MUX` keyed `"<classname>\0<output>"`. The
/// `FireOutputInternal` detour is installed unconditionally at shim Load, so no per-subscribe engine
/// registration is needed (mirrors `s2_chat_on_message`).
fn s2_output_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let classname = args.get(0).to_rust_string_lossy(scope);
        let output = args.get(1).to_rust_string_lossy(scope);
        let key = format!("{}\0{}", classname, output);
        // The FireOutputInternal detour stays installed for the process lifetime — no follow-up.
        let Some((sub_id, _)) = subscribe_into(scope, &args, &OUTPUT_MUX, &key, 2) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// Native `__s2_output_unsubscribe(classname, output)`. Removes the CURRENT plugin's subscriptions for
/// the `(classname, output)` key (best-effort, mirrors `EventMux::remove_by_owner_on` — V8 `Global`s
/// can't be compared by identity, so this drops ALL of the caller's subs for that exact key). Available
/// as a primitive; `Entity.onOutput` this slice has no matching `offOutput` — cleanup on unload/reload
/// runs via `remove_by_owner`, not this native.
/// Native `__s2_cvar_on_change(name, handler) -> subId`. `name` is a cvar name or `"*"`.
fn s2_cvar_on_change(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        if name.is_empty() { return; }
        let Some((sub_id, _first)) = subscribe_into(scope, &args, &CVAR_MUX, &name, 1) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// Native `__s2_cvar_off_change(name)`. Drops the CURRENT plugin's subscriptions for that name
/// (best-effort by owner, mirroring `s2_output_unsubscribe` — V8 Globals are not identity-comparable).
fn s2_cvar_off_change(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        CVAR_MUX.with(|m| { m.borrow_mut().remove_by_owner_on(&name, &owner); });
    }));
}

fn s2_output_unsubscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let classname = args.get(0).to_rust_string_lossy(scope);
        let output = args.get(1).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let key = format!("{}\0{}", classname, output);
        OUTPUT_MUX.with(|m| { m.borrow_mut().remove_by_owner_on(&key, &owner); });
    }));
}




// ---------------------------------------------------------------------------
// Plugin-declared engine calls (`@s2script/sdk/unsafe`). THREE natives and no registration native:
// core registered every descriptor itself at plugin load from the packed `gamedata.json`, so JS can
// only ask BY NAME — it can never hand core a declaration. Core owns every marshalling decision
// (arg classification, the string/vector temporaries, the entity pack/unpack, the lazy `via` hop),
// which is what keeps the prelude a thin shim and keeps raw pointers out of JS entirely (spec §4/§10).
// ---------------------------------------------------------------------------

/// Read a named property off a JS object. `None` when the value is not an object or the property is
/// absent — a missing property is a degrade input, never an error.
fn obj_prop<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    v: v8::Local<v8::Value>,
    key: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let obj = v8::Local::<v8::Object>::try_from(v).ok()?;
    let k = v8::String::new(scope, key)?;
    obj.get(scope, k.into())
}

/// Pack an `EntityRef` arg into the `(index, engine serial)` pair the shim's entity arg slot carries.
/// A null / non-EntityRef / stale ref packs `(-1, -1)`, which the shim's range guard turns into a
/// NULL pointer argument — a legitimate "no entity" (the generated types spell exactly that as
/// `EntityRef | null`), never a wild pointer. The engine serial never crosses to JS: it is read out
/// of the HOST'S BOOKS here from the ref's `(index, id)`.
fn pack_entity_arg(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> u64 {
    const NO_ENTITY: u64 = 0xffff_ffff_ffff_ffff; // (index -1, serial -1)
    let idx_v = obj_prop(scope, v, "index");
    let index = idx_v.and_then(|x| x.integer_value(scope)).unwrap_or(-1) as i32;
    let id_v = obj_prop(scope, v, "id");
    let id = match id_v {
        Some(x) => js_ent_id(scope, x),
        None => 0,
    };
    let Some(serial) = crate::entity_live::engine_serial_for(index, id) else { return NO_ENTITY };
    ((index as u32 as u64) << 32) | (serial as u32 as u64)
}

// The plugin id these four natives key on is ALWAYS the calling context's own
// (`current_plugin`), never a caller-supplied string. `gamedata_calls::prepare` gates the
// `engine:calls` permission once, at registration, so a descriptor's presence in the registry IS
// its authorization — and these raw natives sit on every plugin's global object. Taking the id as
// an argument would let any plugin drive the engine calls the operator allow-listed for a
// DIFFERENT plugin. A context with no plugin identity (the shared HOST context) fails closed.

/// The GAME PACKAGE's descriptor owner, for the `__s2_game_call_*` natives (A5b, spec §9.1b).
///
/// Same discipline as `current_plugin` above, one tier over: the owner id is core's own reserved id
/// for the registered game package, never anything JS supplied. The game package's prelude
/// (`pawn.js`) runs in the raw context scope of EVERY plugin context, so these natives are reachable
/// from any plugin — by design, and not a widening: they replace natives that are unconditionally
/// callable from any plugin today, and they can only INVOKE what the shim registered, never declare.
fn game_call_owner() -> Option<String> {
    crate::gamedata_calls::game_package_owner()
}

/// The named reason reported when no game package has registered gamedata at all — a distinct
/// answer from "that owner declared no such call", because the fixes differ.
const NO_GAME_PACKAGE: &str = "no game package has registered gamedata with this host";

/// Native `__s2_engine_call_ready(callName) -> boolean`. True iff the descriptor passed
/// every LOAD-time gate (allow-list + op + resolve/validate). This is what `Engine.call()` keys
/// callable-or-null on, so it deliberately ignores a pending `via` hop (spec §11).
fn s2_engine_call_ready(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    let owner = current_plugin(scope);
    engine_call_ready_for(scope, args, rv, owner);
}

/// `__s2_game_call_ready(callName)` — the game-package-scoped sibling (see `game_call_owner`).
fn s2_game_call_ready(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    engine_call_ready_for(scope, args, rv, game_call_owner());
}

fn engine_call_ready_for(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue, owner: Option<String>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let Some(pid) = owner else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        rv.set_bool(crate::gamedata_calls::is_ready(&pid, &name));
    }));
}

/// Native `__s2_engine_call_receiverless(callName) -> boolean`. True for a descriptor
/// declaring `receiver.kind: "none"` — the generated callable then takes no leading `self`.
fn s2_engine_call_receiverless(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    let owner = current_plugin(scope);
    engine_call_receiverless_for(scope, args, rv, owner);
}

/// `__s2_game_call_receiverless(callName)` — the game-package-scoped sibling.
fn s2_game_call_receiverless(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    engine_call_receiverless_for(scope, args, rv, game_call_owner());
}

fn engine_call_receiverless_for(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue, owner: Option<String>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let Some(pid) = owner else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        rv.set_bool(crate::gamedata_calls::is_receiverless(&pid, &name));
    }));
}

/// Native `__s2_engine_call_status(callName) -> string`. `"available"`, or the named reason
/// the descriptor is not (spec §12) — for diagnostics and operator reports.
fn s2_engine_call_status(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    let owner = current_plugin(scope);
    // No plugin identity = the shared HOST context: unchanged behaviour, a bare "unavailable".
    engine_call_status_for(scope, args, rv, owner, "unavailable");
}

/// `__s2_game_call_status(callName)` — the game-package-scoped sibling.
fn s2_game_call_status(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    engine_call_status_for(scope, args, rv, game_call_owner(), NO_GAME_PACKAGE);
}

fn engine_call_status_for(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue, owner: Option<String>, no_owner_reason: &str) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Default to a NAMED reason up front: a panic below then still yields a sentence an operator
        // can act on, never JS `undefined`. With no owner it is also the ANSWER — for the
        // game-scoped native, "no game package registered" is a different fix from "not declared".
        if let Some(s) = v8::String::new(scope, no_owner_reason) { rv.set(s.into()); }
        if args.length() < 1 { return; }
        let Some(pid) = owner else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        let status = crate::gamedata_calls::status(&pid, &name);
        if let Some(s) = v8::String::new(scope, &status) { rv.set(s.into()); }
    }));
}

/// Native `__s2_engine_call_invoke(callName, selfIndex, selfId, argsArray) -> value`.
///
/// JS passes ONLY the receiver identity and the raw arg values; core looks the descriptor up to
/// obtain the shim call id, the arg kinds, the return kind and (lazily) the `via` sub-object offset.
/// Every failure is a no-op returning `null` (spec §12 "Call" row): a stale receiver, an unresolved
/// `via` offset, an interior NUL in a string arg, or a missing op. `string` args are marshalled into
/// temporaries that live exactly as long as this call (spec §4's documented author's risk), and a
/// `returns: "entity"` result is a packed handle run through the books-gated adopt path — a raw
/// pointer can never mint a ref.
fn s2_engine_call_invoke(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    let owner = current_plugin(scope);
    engine_call_invoke_for(scope, args, rv, owner);
}

/// `__s2_game_call_invoke(callName, selfIndex, selfId, argsArray)` — the game-package-scoped
/// sibling (see `game_call_owner`). Identical marshalling; only the owner id differs.
fn s2_game_call_invoke(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    engine_call_invoke_for(scope, args, rv, game_call_owner());
}

fn engine_call_invoke_for(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue, owner: Option<String>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 3 { return; }
        let Some(pid) = owner else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        // The registry borrow is released HERE (the plan is cloned): the engine call below may
        // synchronously fire an output/event that dispatches into JS and calls Engine.call again.
        let Some(plan) = crate::gamedata_calls::plan(&pid, &name) else { return };
        // Receiver: books-gated (index, id) → (index, engine serial). Not-live → no-op, never a deref.
        //
        // A receiverless descriptor (`receiver.kind: "none"`) has no entity to gate: it passes the
        // shim's receiverless sentinel (index < 0) so no resolve is attempted and the first declared
        // arg takes the slot `this` would have occupied.
        let (index, serial) = if plan.receiverless {
            (-1, 0)
        } else {
            match ent_op_serial(scope, args.get(1), args.get(2)) {
                Some(pair) => pair,
                None => return,
            }
        };

        // `receiver.via`: resolved LAZILY through the same cached resolver JS's `__s2_schema_offset`
        // uses — schema resolves at map-live, not at Load (spec §11). A miss no-ops THIS invocation
        // and flips `Engine.status(name)` to a named reason; it clears on the first success, so the
        // descriptor recovers once the map is live instead of being permanently dead.
        let subobj_off = match &plan.via {
            None => -1,
            Some((class, field)) => {
                let off = schema_offset_cached(class, field);
                if off < 0 {
                    crate::gamedata_calls::set_via_miss(
                        &pid,
                        &name,
                        Some("receiver sub-object offset unresolved (schema resolves at map-live)".to_string()),
                    );
                    return;
                }
                crate::gamedata_calls::set_via_miss(&pid, &name, None);
                off
            }
        };

        // Marshal the args into the two SysV register sequences, preserving order within each class.
        // `strs`/`vecs` carry the indirect payloads; a GP slot holds their INDEX (bounded by the slot
        // count, which is what the shim re-validates).
        let js_args = v8::Local::<v8::Array>::try_from(args.get(3)).ok();
        let mut gp: Vec<u64> = Vec::new();
        let mut gp_kind: Vec<u8> = Vec::new();
        let mut fp: Vec<f64> = Vec::new();
        let mut strs_owned: Vec<CString> = Vec::new();
        let mut vecs: Vec<f32> = Vec::new();
        for (i, kind) in plan.args.iter().enumerate() {
            let mut v: v8::Local<v8::Value> = v8::undefined(scope).into();
            if let Some(a) = js_args {
                if let Some(x) = a.get_index(scope, i as u32) { v = x; }
            }
            match kind.as_str() {
                "float" => fp.push(v.number_value(scope).unwrap_or(0.0)),
                "bool" => {
                    gp.push(if v.boolean_value(scope) { 1 } else { 0 });
                    gp_kind.push(crate::gamedata_calls::GP_SCALAR);
                }
                "int" => {
                    gp.push(v.integer_value(scope).unwrap_or(0) as u64);
                    gp_kind.push(crate::gamedata_calls::GP_SCALAR);
                }
                "entity" => {
                    gp.push(pack_entity_arg(scope, v));
                    gp_kind.push(crate::gamedata_calls::GP_ENTITY);
                }
                // `string` and `utlstring` build the SAME buffer and differ only in the kind byte:
                // the shim passes the char* directly for one, and the address of a call-scoped
                // `{ char* }` temporary (a CUtlString) for the other.
                "string" | "utlstring" => {
                    let s = v.to_rust_string_lossy(scope);
                    // An interior NUL would silently truncate into a DIFFERENT string — no-op instead.
                    let Ok(c) = CString::new(s) else { return };
                    strs_owned.push(c);
                    gp.push((strs_owned.len() - 1) as u64);
                    gp_kind.push(if kind == "utlstring" {
                        crate::gamedata_calls::GP_UTLSTRING
                    } else {
                        crate::gamedata_calls::GP_STRING
                    });
                }
                "vector" => {
                    let xv = obj_prop(scope, v, "x");
                    let x = xv.and_then(|a| a.number_value(scope)).unwrap_or(0.0) as f32;
                    let yv = obj_prop(scope, v, "y");
                    let y = yv.and_then(|a| a.number_value(scope)).unwrap_or(0.0) as f32;
                    let zv = obj_prop(scope, v, "z");
                    let z = zv.and_then(|a| a.number_value(scope)).unwrap_or(0.0) as f32;
                    vecs.extend_from_slice(&[x, y, z]);
                    gp.push((vecs.len() / 3 - 1) as u64);
                    gp_kind.push(crate::gamedata_calls::GP_VECTOR);
                }
                // Unknown kind: registration already rejected it with a named reason — belt-and-braces.
                _ => return,
            }
        }
        let strs: Vec<*const c_char> = strs_owned.iter().map(|c| c.as_ptr()).collect();

        let Some(func) = engine_ops().and_then(|o| o.engine_call_invoke) else { return };

        // THE BYPASS LATCH. Any hook in this SAME owner that names this call as its `bypassWith`
        // must not fire for OUR OWN outbound invocation — SourceMod's g_pIgnoreTerminateDetour /
        // blockhook. Other plugins still see the side-effect events (round_end / player_spawn).
        //
        // ARM IMMEDIATELY BEFORE, DISARM IMMEDIATELY AFTER — straight-line, with no `return` between
        // them. The latch is one-shot and the thunk clears it by TAKING it, but the take only
        // happens if the call reached the hooked function: `func` can return 0 without calling
        // anything (a stale receiver, an unresolved sub-object), and the latch would then stay armed
        // to swallow the next GENUINE engine-driven invocation. That is spec §10's "clear it on both
        // paths", and it is why `hook_disarm_bypass` exists at all.
        let bypass_ids = crate::gamedata_hooks::bypass_ids_for_call(&pid, &name);
        let (arm_fn, disarm_fn) = if bypass_ids.is_empty() {
            (None, None) // the overwhelmingly common case: no hook names this call
        } else {
            let o = engine_ops();
            match (o.and_then(|o| o.hook_arm_bypass), o.and_then(|o| o.hook_disarm_bypass)) {
                (Some(a), Some(d)) => (Some(a), Some(d)),
                // ARM ONLY IF WE CAN DISARM. An ops table with one and not the other (an older
                // shim) would otherwise arm a latch nothing ever clears — precisely the leak this
                // pairing exists to prevent, and worse than the thing it was meant to fix: losing
                // the SM "our own call does not fire our own hook" semantic costs one spurious
                // dispatch, while a stuck latch silently swallows a genuine engine-driven one.
                _ => {
                    warn_once_bypass_unpairable();
                    (None, None)
                }
            }
        };
        for id in &bypass_ids {
            if let Some(arm) = arm_fn { arm(*id); }
        }
        let mut ret: u64 = 0;
        let ok = crate::nest::with_outbound(&args, || {
            func(
                plan.call_id, index, serial, subobj_off,
                gp.as_ptr(), gp_kind.as_ptr(), gp.len() as i32,
                fp.as_ptr(), fp.len() as i32,
                strs.as_ptr(), vecs.as_ptr(),
                plan.ret_code, &mut ret,
            )
        });
        for id in &bypass_ids {
            if let Some(disarm) = disarm_fn { disarm(*id); }
        }

        if ok == 0 { return; } // shim-side degrade (stale receiver / absent sub-object) → null

        match plan.ret_code {
            crate::gamedata_calls::RET_VOID => rv.set_undefined(),
            crate::gamedata_calls::RET_BOOL => rv.set_bool(ret != 0),
            crate::gamedata_calls::RET_INT => rv.set_int32(ret as u32 as i32),
            crate::gamedata_calls::RET_FLOAT => rv.set_double(f32::from_bits(ret as u32) as f64),
            crate::gamedata_calls::RET_ENTITY => {
                // The shim handed back a packed CEntityHandle read off the entity's own identity;
                // only the books can turn it into a live ref (a dangling handle yields null).
                //
                // 0xFFFFFFFF is the shim's "no entity" sentinel (kInvalidEntityHandle) and must be
                // rejected BEFORE decoding. It cannot be 0, because 0 decodes to the perfectly legal
                // (index 0, serial 0) — an absent entity would otherwise be indistinguishable from a
                // live handle to entity slot 0.
                if ret as u32 != crate::gamedata_calls::INVALID_ENTITY_HANDLE {
                    let (i, s) = crate::entity::decode_handle(ret as u32);
                    if let Some(id) = crate::entity_live::adopt(i, s) {
                        rv.set(build_entity_ref(scope, i, id));
                    }
                }
            }
            _ => {}
        }
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















/// Native `__s2_translations_read(lang, name) -> string | null`. Mirrors `s2_client_name`'s
/// call/copy pattern but takes two string args and calls `translations_read`.
fn s2_translations_read(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 2 { return; }
        let lang = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.translations_read else { return };
        let c_lang = std::ffi::CString::new(lang).unwrap_or_default();
        let c_name = std::ffi::CString::new(name).unwrap_or_default();
        let ptr = func(c_lang.as_ptr(), c_name.as_ptr());
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

// ---------------------------------------------------------------------------
// Slice 5D.3: event write/fire natives (pre-subscribe/unsubscribe + setters + create/fire).
// ---------------------------------------------------------------------------













/// Slice 6.6 Stage 2: run OnTakeDamage SDKHooks over the current CTakeDamageInfo (set by the
/// shim detour). Mirrors `dispatch_game_event`: snapshot (release the table borrow), re-entrancy guard,
/// per-subscriber liveness + context + TryCatch. Each handler gets `new DamageInfo()` (a block-scoped
/// accessor over the current damage) and reads/modifies it in place; blocking = the handler setting
/// damage to 0.
/// Zero the live CTakeDamageInfo damage — the block power behind an OnTakeDamage SDKHook
/// returning `>= HookResult.Handled` (locked decision #8). Reuses the exact write path the JS
/// `DamageInfo.damage = 0` setter takes: resolve `m_flDamage`'s schema offset, then the
/// `damage_write_float` engine op with `0.0`. (CTakeDamageInfo is a Source 2 engine type, not a
/// game-specific one — engine-generic, like the rest of the damage module.) No-op if the offset is
/// unresolved or the op is absent (degrade-never-crash).
fn zero_current_damage() {
    let live_raw = |c: &str, f: &str| -> i32 {
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return -1 };
        let Some(func) = ops.schema_offset else { return -1 };
        let (Ok(cc), Ok(cf)) = (CString::new(c), CString::new(f)) else { return -1 };
        func(cc.as_ptr(), cf.as_ptr())
    };
    let live_log = |_msg: &str| {};
    let off = SCHEMA_OFFSETS.with(|c| c.borrow_mut().resolve("CTakeDamageInfo", "m_flDamage", live_raw, live_log));
    if off < 0 { return; }
    if let Some(func) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.damage_write_float) {
        func(off, 0.0);
    }
}

pub(crate) fn dispatch_damage() {
    // Handled zeroes live damage AFTER the collapse (does not skip later observers). Stop truncates.
    with_damage_writes_frozen(false, || {
        dispatch_damage_kind(
            crate::sdkhooks::snapshot_ontakedamage(),
            "dispatch_damage",
            "damage:onPre",
            StopAt::Stop,
            true,
        );
    });
}

/// `OnTakeDamagePost` — after the original DTA ran. Return is ignored; Handled does not zero.
/// `DamageInfo.damage` assignment is frozen (spec: info is read-only on the post-hook).
pub(crate) fn dispatch_damage_post() {
    with_damage_writes_frozen(true, || {
        dispatch_damage_kind(
            crate::sdkhooks::snapshot_ontakedamage_post(),
            "dispatch_damage_post",
            "damage:onPost",
            StopAt::Never,
            false,
        );
    });
}

fn dispatch_damage_kind(
    snap: Vec<(String, u64, v8::Global<v8::Function>)>,
    label: &'static str,
    breadcrumb: &'static str,
    stop_at: StopAt,
    zero_on_handled: bool,
) {
    let result = fan_out_collapsing(
        &snap,
        label,
        Instrument::breadcrumb(breadcrumb),
        stop_at,
        |tc| {
            let info: Option<v8::Local<v8::Value>> = (|| {
                let global = tc.get_current_context().global(tc);
                let pkg_key = v8::String::new(tc, "__s2pkg_damage")?;
                let pkg = global.get(tc, pkg_key.into())?;
                let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
                let ctor_key = v8::String::new(tc, "DamageInfo")?;
                let ctor_val = pkg.get(tc, ctor_key.into())?;
                let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
                ctor.new_instance(tc, &[]).map(|o| -> v8::Local<v8::Value> { o.into() })
            })();
            Some(vec![info.unwrap_or_else(|| v8::undefined(tc).into())])
        },
    );
    if zero_on_handled && result >= HookResult::Handled {
        zero_current_damage();
    }
}

/// Usercmd primitive Task 2: run the `UserCmd.onRun` subscribers over the current tick's input (the
/// Task-3 shim detour sets the current `s_currentUserCmd` before calling this, and reads the
/// possibly-modified fields back after). Mirrors `dispatch_damage`'s snapshot + `try_borrow_mut`
/// re-entrancy guard, but (a) takes the firing player's `slot`, (b) fetches the prelude's SINGLETON
/// `Cmd` object (MF-3 — one shared accessor object over `globalThis.__s2pkg_usercmd.Cmd`, NOT a
/// per-handler `new Cmd()` — the DamageInfo precedent doesn't apply here) + builds a block-scoped
/// `{slot}` ctx, and (c) collapses each handler's returned int into a `HookResult` via `run_chain`
/// (mirrors `dispatch_output`/`dispatch_game_event_pre`, NOT `dispatch_damage` which is void) — the
/// Task-3 detour supersedes (blocks) the original input for that tick when the result is >= Handled.
/// Degrades to `undefined` if `@s2script/usercmd` never registered its prelude (Cmd absent) so a
/// handler still runs rather than being skipped.
pub(crate) fn dispatch_usercmd(slot: i32) -> i32 {
    let snap = USERCMD_MUX.with(|m| m.borrow().snapshot("onRun"));
    let result = fan_out_collapsing(&snap, "dispatch_usercmd", Instrument::none(), StopAt::Stop, |tc| {
        // Fetch the prelude's SINGLETON Cmd: globalThis.__s2pkg_usercmd.Cmd (Task 4 registers
        // it; degrade to `undefined` if the module never loaded in this context).
        let cmd_arg: Option<v8::Local<v8::Value>> = (|| {
            let global = tc.get_current_context().global(tc);
            let pkg_key = v8::String::new(tc, "__s2pkg_usercmd")?;
            let pkg = global.get(tc, pkg_key.into())?;
            let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
            let cmd_key = v8::String::new(tc, "Cmd")?;
            pkg.get(tc, cmd_key.into())
        })();
        let cmd_val: v8::Local<v8::Value> = cmd_arg.unwrap_or_else(|| v8::undefined(tc).into());

        // Build the block-scoped ctx = { slot }.
        let ctx_obj = v8::Object::new(tc);
        if let Some(k) = v8::String::new(tc, "slot") {
            let v = v8::Integer::new(tc, slot);
            ctx_obj.set(tc, k.into(), v.into());
        }
        let ctx_val: v8::Local<v8::Value> = ctx_obj.into();
        Some(vec![cmd_val, ctx_val])
    });
    result as i32
}

// ---------------------------------------------------------------------------
// Declarative inbound hooks — the dispatch half (spec §6).
//
// The engine calls a compiled thunk; the thunk calls `s2script_core_dispatch_hook(hookId, argView)`;
// this is where that lands. Everything below is engine-generic: the hook is identified by a slot id
// core itself handed out, and its params are named by the DESCRIPTOR, never by core.
// ---------------------------------------------------------------------------

/// The inbound hook dispatch currently running (see the `ACTIVE_HOOK` thread-local).
pub(crate) struct ActiveHook {
    /// The thunk's own stack-frame arg view. OPAQUE — core never dereferences it; it only hands it
    /// back to the shim's accessors, which liveness-gate it against their own record of the live
    /// view. It dies with the thunk's frame, which is why nothing may retain it.
    view: *mut std::ffi::c_void,
    /// The declaring owner + hook name, so an accessor failure can be reported as a NAMED degrade of
    /// THIS hook rather than as a silent zero.
    owner: String,
    name: String,
    /// THE BINDING TOKEN: a monotonic id for THIS dispatch, minted in `dispatch_hook` and stamped
    /// into every accessor the view's properties carry.
    ///
    /// Without it, a view object is bound to nothing. Its accessors would carry only a param index
    /// and read whichever dispatch happened to be active, so a view stashed out of one hook's
    /// handler would silently REBIND to the next hook's live frame: on a shape both hooks share, the
    /// shim's bounds-and-class check passes, and a `mutable` param of the FIRST hook becomes a write
    /// primitive into the SECOND hook's args — past that hook's own `mutable` allow-list, with no
    /// degrade. An epoch rather than the hook id, because the same trap exists between two dispatches
    /// of the SAME hook, where a hook id would match.
    epoch: u64,
}

/// The `HOOK_MUX` channel key for a declared hook.
///
/// Both halves are needed: two owners may each declare a hook called `onRespawn`, and they are
/// different detours with different subscribers. The separator is NUL, which cannot occur in either
/// half — both arrive as JSON object keys that crossed the C ABI as NUL-terminated strings — so no
/// owner/name pair can be spelled to collide with another.
fn hook_key(owner: &str, name: &str) -> String {
    format!("{}\u{0}{}", owner, name)
}

/// Map the owner id JS passes to the id the registry keys on.
///
/// A game package's hooks are registered under the RESERVED owner id (`game-package:@s2script/cs2`),
/// which JS can neither see nor spell; its generated binding passes the plain package name. Any
/// other string is a plugin's own id and is used as given.
///
/// Subscribing is deliberately NOT the privileged operation: `engine:hooks` gates DECLARING a hook
/// (which is what patches bytes and what an operator authorizes). A plugin subscribing to a hook
/// another owner declared is the intended path — that is what the generated `ctx` namespaces are —
/// and matches SourceMod, where any plugin subscribing to `CS_OnTerminateRound` installs the detour.
fn hook_owner_id(arg: &str) -> String {
    let reserved = crate::gamedata_calls::reserved_owner_id(arg);
    if crate::gamedata_calls::game_package_owner().as_deref() == Some(reserved.as_str()) {
        return reserved;
    }
    arg.to_string()
}

/// The live dispatch's `(view, owner, name, epoch)`, or `None` outside one. Copied out so the
/// `ACTIVE_HOOK` borrow is released before anything else runs.
fn active_hook() -> Option<(*mut std::ffi::c_void, String, String, u64)> {
    ACTIVE_HOOK
        .with(|a| a.borrow().as_ref().map(|h| (h.view, h.owner.clone(), h.name.clone(), h.epoch)))
}

/// What an accessor is allowed to do, decided BEFORE the frame is touched.
enum HookAccess {
    /// Bound to the live dispatch: proceed.
    Live { view: *mut std::ffi::c_void, owner: String, name: String, idx: i32 },
    /// No dispatch is running at all — the view outlived its thunk. Self-evident to the caller (a
    /// read yields `undefined`), and the descriptor is fine, so this is not a degrade.
    Dead,
    /// A DIFFERENT dispatch is running: this view belongs to a finished one and must not touch the
    /// frame that is live now. Carries the CURRENT hook, because that is the frame that was nearly
    /// read or written and the one an operator needs named.
    Rebound { owner: String, name: String, idx: i32 },
}

/// Decode an accessor's `data` — `[paramIndex, epoch]` — and check it against the live dispatch.
///
/// The epoch comparison is the whole point: it is what BINDS a view object to the one dispatch that
/// built it. See `ActiveHook::epoch`.
fn hook_access(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments) -> HookAccess {
    let (idx, epoch) = match v8::Local::<v8::Array>::try_from(args.data()) {
        Ok(a) => (
            a.get_index(scope, 0).and_then(|v| v.int32_value(scope)).unwrap_or(-1),
            a.get_index(scope, 1).and_then(|v| v.number_value(scope)).unwrap_or(-1.0) as u64,
        ),
        // Unreachable — `build_hook_view` is the only thing that builds these — but a malformed
        // `data` must fail closed rather than default to param 0 of the live frame.
        Err(_) => (-1, u64::MAX),
    };
    match active_hook() {
        None => HookAccess::Dead,
        Some((view, owner, name, live_epoch)) => {
            if live_epoch == epoch {
                HookAccess::Live { view, owner, name, idx }
            } else {
                HookAccess::Rebound { owner, name, idx }
            }
        }
    }
}

/// WARN once per process that the bypass latch cannot be used because the ops table has an arm
/// without a disarm. Once, because the alternative is a line per `Engine.call` on a hooked function.
fn warn_once_bypass_unpairable() {
    thread_local! {
        static WARNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    WARNED.with(|w| {
        if !w.get() {
            w.set(true);
            log_warn(
                "WARN: [engine-hooks] this shim exposes hook_arm_bypass without hook_disarm_bypass, \
                 so the bypass latch is DISABLED: a plugin-issued call will fire that call's own \
                 hook (SourceMod suppresses it). Arming without a disarm would be worse — the latch \
                 would stay set and swallow the next engine-driven invocation.",
            );
        }
    });
}

/// The named reason a rebound access is refused. One wording, used by both accessors, because the
/// two failures are the same failure.
fn rebound_reason(idx: i32, what: &str) -> String {
    format!(
        "a hook view from a FINISHED dispatch tried to {} param #{} of this one — the view is \
         block-scoped and the access was REFUSED (holding one past its handler would read or write \
         another hook's live arguments, past its own 'mutable' list)",
        what, idx
    )
}

/// Read positional param `idx` out of the live arg view. Returns `(value, is_float)`, or `None` when
/// the shim refused the read.
///
/// The float/int class is discovered by PROBING rather than mirrored from a core-side table: the
/// shim owns each shape's param layout and class-checks every accessor, so asking it is the only
/// answer that cannot drift. `f32` is tried first; a class mismatch is a clean -1 there, never a
/// reinterpretation of the bits.
/// What a hook param turned out to be. The shape's table decides — each reader rejects a param
/// whose class is not its own, so exactly one of these can succeed for a given index.
enum HookParamValue {
    /// `is_float` preserves the reader's class so the WRITE path can keep refusing values the
    /// param cannot represent (an f32 overflow, an out-of-range i32) instead of coercing them.
    Num { value: f64, is_float: bool },
    Text(String),
}

fn hook_param_read(view: *mut std::ffi::c_void, idx: i32) -> Option<HookParamValue> {
    let ops = ENGINE_OPS.with(|o| o.get())?;
    if let Some(f) = ops.hook_read_f32 {
        let mut out: f32 = 0.0;
        if f(view, idx, &mut out) == 0 {
            return Some(HookParamValue::Num { value: out as f64, is_float: true });
        }
    }
    if let Some(f) = ops.hook_read_i32 {
        let mut out: i32 = 0;
        if f(view, idx, &mut out) == 0 {
            return Some(HookParamValue::Num { value: out as f64, is_float: false });
        }
    }
    // Text params. The buffer matches the view's own capacity; the shim always NUL-terminates and
    // bounds the copy by BOTH capacities, so a short read here can only truncate, never overrun.
    if let Some(f) = ops.hook_read_str {
        let mut buf = [0i8; 128];
        if f(view, idx, buf.as_mut_ptr(), buf.len() as c_int) == 0 {
            let bytes: Vec<u8> = buf
                .iter()
                .take_while(|c| **c != 0)
                .map(|c| *c as u8)
                .collect();
            return Some(HookParamValue::Text(String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    None
}

/// The getter behind every param of a hook view. The param index rides in the function's `data`, so
/// one callback serves every param of every hook.
///
/// A failed read yields `undefined` AND a named degrade — never `0`. That distinction is the whole
/// point: a handler that reads `view.delay` and gets a plausible-looking `0` cannot tell it apart
/// from the engine genuinely passing zero, and would act on a value that was never there.
fn s2_hook_param_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        let (view, owner, name, idx) = match hook_access(scope, &args) {
            // Outside a dispatch there is no frame to read: the view a handler stashed died with
            // the thunk. `undefined` with no degrade — the descriptor is fine, the timing is not.
            HookAccess::Dead => return,
            HookAccess::Rebound { owner, name, idx } => {
                crate::gamedata_hooks::note_miss(&owner, &name, Some(rebound_reason(idx, "read")));
                return;
            }
            HookAccess::Live { view, owner, name, idx } => (view, owner, name, idx),
        };
        match hook_param_read(view, idx) {
            Some(HookParamValue::Num { value, .. }) => rv.set_double(value),
            Some(HookParamValue::Text(t)) => {
                match v8::String::new(scope, &t) {
                    Some(js) => rv.set(js.into()),
                    None => return,
                }
            }
            None => crate::gamedata_hooks::note_miss(
                &owner,
                &name,
                Some(format!(
                    "param #{} could not be read from the arg view (a stale binding, or a shape with \
                     no such param) — handlers see `undefined`, never a 0",
                    idx
                )),
            ),
        }
    }));
}

/// The setter behind a `mutable` param. A read-only param has no setter at all, so assigning to one
/// throws in strict mode — which every plugin is (pure ESM).
///
/// The write class comes from the same probe the getter uses, so a `mutable` param is written back
/// through the accessor that matches the shape's actual class or not at all.
fn s2_hook_param_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let value = args.get(0).number_value(scope).unwrap_or(f64::NAN);
        let (view, owner, name, idx) = match hook_access(scope, &args) {
            // Outside a dispatch there is no frame to write. Worth a line even though the hook
            // cannot be named from here (the view is gone, and with it the hook it belonged to): a
            // LOST WRITE is not self-evident to the caller the way a read of `undefined` is.
            HookAccess::Dead => {
                log_warn(
                    "WARN: a hook view was written outside its dispatch — the view is block-scoped \
                     and the write is IGNORED",
                );
                return;
            }
            HookAccess::Rebound { owner, name, idx } => {
                crate::gamedata_hooks::note_miss(&owner, &name, Some(rebound_reason(idx, "write")));
                return;
            }
            HookAccess::Live { view, owner, name, idx } => (view, owner, name, idx),
        };
        // No `else { return }` on the ops lookup: an absent op is a LOST WRITE like any other and
        // must reach the named degrade below rather than vanish.
        let ops = ENGINE_OPS.with(|o| o.get());
        let ok = match hook_param_read(view, idx) {
            // A value that cannot be represented in the param's class is REFUSED, not coerced. The
            // getter's rule, applied to writes: `v.reason = "abc"` is NaN, and `NaN as i32`
            // saturates to 0 — the engine would receive a plausible-looking zero and the handler
            // would never learn its write was nonsense. Same for a magnitude outside i32.
            //
            // The float arm needs BOTH halves. `!is_finite()` catches NaN and an f64 infinity, but
            // 1e300 is a perfectly finite f64 whose `as f32` is `f32::INFINITY` — so finiteness
            // alone would let `view.delay = scale * base` overflow into `+inf`, hand that to the
            // engine, and report a SUCCESSFUL write. A silently wrong value is worse than a refusal.
            Some(HookParamValue::Num { is_float: true, .. })
                if !value.is_finite() || value.abs() > f32::MAX as f64 => None,
            Some(HookParamValue::Num { is_float: false, .. })
                if !(value >= i32::MIN as f64 && value <= i32::MAX as f64) => None,
            Some(HookParamValue::Num { is_float: true, .. }) => ops.and_then(|o| o.hook_write_f32).map(|f| f(view, idx, value as f32)),
            Some(HookParamValue::Num { is_float: false, .. }) => ops.and_then(|o| o.hook_write_i32).map(|f| f(view, idx, value as i32)),
            // A text param is read-only: there is nowhere to put a written string that the engine
            // would ever look at (the view holds a COPY made after the engine handed the pointer over).
            Some(HookParamValue::Text(_)) => None,
            None => None,
        };
        if ok != Some(0) {
            crate::gamedata_hooks::note_miss(
                &owner,
                &name,
                Some(format!(
                    "param #{} could not be written back to the arg view (a stale binding, or a \
                     value the param's class cannot represent — which is REFUSED, never coerced) — \
                     the engine will see the ORIGINAL value",
                    idx
                )),
            );
        } else {
            // A successful write during an acquire session is a vote-eligible `result` write.
            ACQUIRE.with(|a| {
                if let Some(s) = a.borrow_mut().as_mut() {
                    s.wrote = true;
                }
            });
        }
    }));
}

/// Build the block-scoped view object one handler receives.
///
/// Params are ACCESSOR properties, not a snapshot: reads hit the live frame, so a second handler
/// sees what the first one wrote, and a write reaches the engine because the thunk re-reads the view
/// before calling the original. A snapshot object would have to be copied back after each handler,
/// which is impossible here — each handler runs in its OWN plugin context, so each gets its own
/// object.
fn build_hook_view<'s>(
    tc: &mut v8::PinScope<'s, '_>,
    plan: &crate::gamedata_hooks::HookPlan,
    view: *mut std::ffi::c_void,
    epoch: u64,
) -> Option<Vec<v8::Local<'s, v8::Value>>> {
    let obj = v8::Object::new(tc);
    for (i, pname) in plan.params.iter().enumerate() {
        let key = v8::String::new(tc, pname)?;
        // `[paramIndex, epoch]` — the index alone would let this accessor operate on WHATEVER
        // dispatch is live when it is called. The epoch binds it to this one. See `ActiveHook`.
        let data_arr = v8::Array::new(tc, 2);
        let iv = v8::Integer::new(tc, i as i32);
        let ev = v8::Number::new(tc, epoch as f64);
        data_arr.set_index(tc, 0, iv.into());
        data_arr.set_index(tc, 1, ev.into());
        let data: v8::Local<v8::Value> = data_arr.into();
        let getter: v8::Local<v8::Value> =
            v8::Function::builder(s2_hook_param_get).data(data).build(tc)?.into();
        // No setter at all for a read-only param — `undefined` is how V8 spells "accessor with no
        // setter", which makes an assignment throw under strict mode instead of silently vanishing.
        let post = HOOK_POST_SKIPPED.with(|c| c.get().is_some());
        let setter: v8::Local<v8::Value> = if plan.writable[i] && !post {
            v8::Function::builder(s2_hook_param_set).data(data).build(tc)?.into()
        } else {
            v8::undefined(tc).into()
        };
        let desc = v8::PropertyDescriptor::new_from_get_set(getter, setter);
        obj.define_property(tc, key.into(), &desc);
    }
    // The receiver, when the descriptor surfaces one: a books-gated EntityRef, exactly like every
    // other entity crossing into JS. NO RAW POINTER — the shim hands back a packed CEntityHandle its
    // own books already vouched for, and `entity_live::adopt` re-decides liveness here.
    if let Some(rname) = &plan.receiver {
        let key = v8::String::new(tc, rname)?;
        let mut handle: u32 = 0;
        // Liveness is decided here; the OBJECT is minted by `build_entity_ref`, the one path every
        // entity crossing into JS takes.
        //
        // It is not a formality. An `EntityRef` is a class with methods, and — the part that bites
        // silently — with NAMED `.index`/`.id` properties, which is what `pack_entity_arg` reads
        // when the ref is passed back to any native. A hand-rolled `[index, id]` array would put
        // those at NUMERIC indices, so the packer would read `undefined` twice and compute
        // "no entity": a live, just-respawned player would reach an engine call looking absent,
        // with no error anywhere. A thrown `isValid is not a function` would at least be loud.
        let live: Option<(i32, u64)> = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.hook_receiver_handle?;
            // -1 is NORMAL, not a degrade: a detour `this` is frequently not an entity at all (a
            // rules/services singleton is the motivating case), and `null` is the honest answer.
            if f(view, &mut handle) != 0 {
                return None;
            }
            let (index, serial) = crate::entity::decode_handle(handle);
            Some((index, crate::entity_live::adopt(index, serial)?))
        })();
        let ent: v8::Local<v8::Value> = match live {
            Some((index, id)) => build_entity_ref(tc, index, id),
            None => v8::null(tc).into(),
        };
        obj.set(tc, key.into(), ent);
    }
    if let Some(skipped) = HOOK_POST_SKIPPED.with(|c| c.get()) {
        let key = v8::String::new(tc, "skipped")?;
        let val: v8::Local<v8::Value> = v8::Boolean::new(tc, skipped).into();
        obj.set(tc, key.into(), val);
    }
    Some(vec![obj.into()])
}

/// `__s2_hook_on(owner, hookName, handler)` — subscribe to a gamedata-declared engine detour.
///
/// Called from the generated `ctx` namespace, never by hand. Records the subscription in `HOOK_MUX`
/// (owner-tracked, so the ledger tears it down at unload like any other) and then asks
/// `gamedata_hooks::subscribe` to ensure the detour is installed — LAZILY, idempotently, and on
/// every subscribe rather than only the first, so a hook whose first install failed recovers instead
/// of staying dead for the process.
///
/// Degrade-never-crash: a hook that could not be installed still records its subscription (the
/// ledger and teardown stay uniform) and WARNs by name; it simply never fires.
fn s2_hook_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 3 {
            return;
        }
        let owner = hook_owner_id(&args.get(0).to_rust_string_lossy(scope));
        let name = args.get(1).to_rust_string_lossy(scope);
        let key = hook_key(&owner, &name);
        let Some((sub_id, _)) = subscribe_into(scope, &args, &HOOK_MUX, &key, 2) else { return };
        if let Err(reason) = crate::gamedata_hooks::subscribe(&owner, &name) {
            log_warn(&format!(
                "WARN: hook_on('{}', '{}'): the detour is not installed, so this handler will not \
                 fire: {}",
                owner, name, reason
            ));
        }
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_engine_hook_ready(hookName) -> boolean`. True iff this plugin's descriptor passed every
/// load-time gate. The owner is the calling context — JS cannot name another plugin.
fn s2_engine_hook_ready(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let Some(pid) = current_plugin(scope) else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        rv.set_bool(crate::gamedata_hooks::status(&pid, &name) == "available");
    }));
}

/// `__s2_engine_hook_status(hookName) -> string`. `"available"`, or the named degrade reason.
fn s2_engine_hook_status(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(s) = v8::String::new(scope, "unavailable") { rv.set(s.into()); }
        if args.length() < 1 { return; }
        let Some(pid) = current_plugin(scope) else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        let status = crate::gamedata_hooks::status(&pid, &name);
        if let Some(s) = v8::String::new(scope, &status) { rv.set(s.into()); }
    }));
}

/// `__s2_engine_hook_on(hookName, handler)` — subscribe this plugin to one of ITS OWN declared
/// hooks. Same body as `__s2_hook_on`, but the owner is the calling context, never an argument,
/// so a plugin cannot attach to another owner's detour through `Engine.hook`.
fn s2_engine_hook_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 2 { return; }
        let Some(owner) = current_plugin(scope) else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        let key = hook_key(&owner, &name);
        let Some((sub_id, _)) = subscribe_into(scope, &args, &HOOK_MUX, &key, 1) else { return };
        if let Err(reason) = crate::gamedata_hooks::subscribe(&owner, &name) {
            log_warn(&format!(
                "WARN: hook_on('{}', '{}'): the detour is not installed, so this handler will not \
                 fire: {}",
                owner, name, reason
            ));
        }
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// Run the subscribers of one declaratively-declared engine hook and return the collapsed
/// `HookResult` the thunk applies (>= Handled suppresses the original engine call entirely).
///
/// THIS DISPATCH IS NEVER DEFERRED, and that is load-bearing rather than a default. `argView` points
/// at the thunk's own STACK FRAME: it is valid for exactly the duration of this call and dies when
/// the thunk returns. A replayed dispatch a frame later would hand JS a dead frame, every accessor
/// would fail (the shim's liveness gate refuses a view that is not the one being dispatched), and
/// every param would read as a degrade. So a re-entrant dispatch is SKIPPED — the engine action
/// proceeds unhooked, which is the safe direction — and `s2script_core_dispatch_hook` never returns
/// `S2_DISPATCH_DEFERRED`.
///
/// It goes through `fan_out_inner` rather than `fan_out_collapsing` for exactly one reason: to SEE
/// that skip. `fan_out_collapsing` discards `Delivery` by construction, which is right for every
/// other pre-hook but made this one vanish — the bypass latch does not cover every JS→engine path
/// to a hooked address (see `gamedata_hooks::note_reentrant_skip`), so the case is reachable, and an
/// unnamed degrade is the failure mode this whole module is built to refuse. The collapsed
/// `HookResult` is used identically; only the `Delivery` is newly inspected.
pub(crate) fn dispatch_hook(hook_id: i32, arg_view: *mut std::ffi::c_void) -> i32 {
    // An id core never handed out — a detour installed by a PREVIOUS core (Metamod reload) — has no
    // descriptor. Continue, so the engine proceeds unhooked.
    let Some((owner, name)) = crate::gamedata_hooks::hook_for_id(hook_id) else { return 0 };
    // Same hook already on the stack: giveNamedItem from onCanAcquire, etc. Skip and name —
    // not a nest, not a queue.
    let same = ACTIVE_HOOK.with(|a| {
        a.borrow()
            .as_ref()
            .is_some_and(|h| h.owner == owner && h.name == name)
    });
    if same {
        crate::gamedata_hooks::note_reentrant_skip(&owner, &name);
        return 0;
    }
    let Some(plan) = crate::gamedata_hooks::plan(&owner, &name) else { return 0 };
    let snap = HOOK_MUX.with(|m| m.borrow().snapshot(&hook_key(&owner, &name)));
    if snap.is_empty() {
        return 0;
    }

    // Publish the frame for exactly the duration of the fan-out. SAVE/RESTORE rather than
    // set/clear: a handler can make the engine call another hooked function, and the inner dispatch
    // must hand the outer one its view back — including its epoch, so the outer frame's accessors
    // still bind after the inner dispatch has come and gone.
    let epoch = HOOK_EPOCH.with(|e| {
        let next = e.get().wrapping_add(1);
        e.set(next);
        next
    });
    let prev = ACTIVE_HOOK.with(|a| {
        a.borrow_mut().replace(ActiveHook {
            view: arg_view,
            owner: owner.clone(),
            name: name.clone(),
            epoch,
        })
    });
    let is_acquire = plan.shape == 3; // this_i64_i32_i64 — see gamedata_hooks::SHAPES
    let prev_acq = if is_acquire {
        ACQUIRE.with(|a| {
            a.borrow_mut().replace(AcquireSession {
                view: arg_view,
                votes: Vec::new(),
                wrote: false,
            })
        })
    } else {
        None
    };
    let prev_after = if is_acquire {
        set_after_handler(Some(acquire_after_handler))
    } else {
        None
    };
    let label = format!("dispatch_hook('{}.{}')", owner, name);
    let (result, delivery) =
        fan_out_inner(&snap, &label, Instrument::breadcrumb(&label), StopAt::Stop, |tc| {
            build_hook_view(tc, &plan, arg_view, epoch)
        });
    if is_acquire {
        set_after_handler(prev_after);
        let session = ACQUIRE.with(|a| {
            let cur = a.borrow_mut().take();
            *a.borrow_mut() = prev_acq;
            cur
        });
        if let Some(mut session) = session {
            crate::acquire::order_votes(&mut session.votes);
            let (folded, _) = crate::acquire::fold_acquire(&session.votes, None);
            if let Some(ops) = ENGINE_OPS.with(|o| o.get()) {
                if let Some(w) = ops.hook_write_i32 {
                    let _ = w(arg_view, 1, folded);
                    let _ = w(arg_view, 2, if session.votes.is_empty() { 0 } else { 1 });
                }
            }
        }
    }
    ACTIVE_HOOK.with(|a| *a.borrow_mut() = prev);
    // Nothing ran. The `Continue` above is still the right answer for the thunk (never a replay —
    // the frame is gone), but the skip is now NAMED instead of silent, and rate-limited to once per
    // hook because this can fire on every engine call.
    if delivery == Delivery::Deferred {
        crate::gamedata_hooks::note_reentrant_skip(&owner, &name);
    }
    result as i32
}

fn acquire_after_handler(hr: HookResult) {
    ACQUIRE.with(|a| {
        let mut slot = a.borrow_mut();
        let Some(s) = slot.as_mut() else { return };
        // Param 1 of the acquire shape is an i32; a text param here would mean the shape table
        // and this call site disagree, so treat anything else as 0 rather than guessing.
        let result = match hook_param_read(s.view, 1) {
            Some(HookParamValue::Num { value, .. }) => value as i32,
            _ => 0,
        };
        match hr {
            HookResult::Continue => {
                s.wrote = false;
            }
            HookResult::Changed => {
                s.votes.push(crate::acquire::AcquireVote { result, skip_original: false });
                s.wrote = false;
            }
            HookResult::Handled | HookResult::Stop => {
                let r = if s.wrote { result } else { crate::acquire::ACQUIRE_IMPLICIT_DENY };
                s.votes.push(crate::acquire::AcquireVote { result: r, skip_original: true });
                s.wrote = false;
            }
        }
    });
}

/// Post-phase spectator mux. Readonly view. `HookResult` ignored. Always runs if subscribed,
/// including after a Pre skip (`skipped: true`).
pub(crate) fn dispatch_hook_post(hook_id: i32, arg_view: *mut std::ffi::c_void, skipped: bool) -> i32 {
    let Some((owner, name)) = crate::gamedata_hooks::hook_for_id(hook_id) else { return 0 };
    let Some(plan) = crate::gamedata_hooks::plan(&owner, &name) else { return 0 };
    let snap = HOOK_MUX.with(|m| m.borrow().snapshot(&hook_key_post(&owner, &name)));
    if snap.is_empty() {
        return 0;
    }
    let epoch = HOOK_EPOCH.with(|e| {
        let next = e.get().wrapping_add(1);
        e.set(next);
        next
    });
    let prev = ACTIVE_HOOK.with(|a| {
        a.borrow_mut().replace(ActiveHook {
            view: arg_view,
            owner: owner.clone(),
            name: name.clone(),
            epoch,
        })
    });
    let prev_skipped = HOOK_POST_SKIPPED.with(|c| c.replace(Some(skipped)));
    let label = format!("dispatch_hook_post('{}.{}')", owner, name);
    let (_, delivery) = fan_out_inner(&snap, &label, Instrument::breadcrumb(&label), StopAt::Never, |tc| {
        build_hook_view(tc, &plan, arg_view, epoch)
    });
    HOOK_POST_SKIPPED.with(|c| c.set(prev_skipped));
    ACTIVE_HOOK.with(|a| *a.borrow_mut() = prev);
    if delivery == Delivery::Deferred {
        crate::gamedata_hooks::note_reentrant_skip(&owner, &name);
    }
    0
}

fn hook_key_post(owner: &str, name: &str) -> String {
    format!("{}\u{0}{}\u{0}post", owner, name)
}

/// `__s2_hook_on_post(owner, hookName, handler)` — subscribe to the Post spectator of a declared hook.
fn s2_hook_on_post(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 3 {
            return;
        }
        let owner = hook_owner_id(&args.get(0).to_rust_string_lossy(scope));
        let name = args.get(1).to_rust_string_lossy(scope);
        let key = hook_key_post(&owner, &name);
        let Some((sub_id, _)) = subscribe_into(scope, &args, &HOOK_MUX, &key, 2) else { return };
        if let Err(reason) = crate::gamedata_hooks::subscribe(&owner, &name) {
            log_warn(&format!(
                "WARN: hook_on_post('{}', '{}'): the detour is not installed, so this handler will not \
                 fire: {}",
                owner, name, reason
            ));
        }
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_hook_q_u16(qslot, class, field)` — u16 at the live view's q[qslot] + schema offset.
/// Game package supplies the class/field names; the pointer never crosses to JS.
fn s2_hook_q_u16(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        let Some((view, _, _, _)) = active_hook() else { return };
        if args.length() < 3 {
            return;
        }
        let qslot = args.get(0).int32_value(scope).unwrap_or(-1);
        let class = args.get(1).to_rust_string_lossy(scope);
        let field = args.get(2).to_rust_string_lossy(scope);
        let off = schema_offset_cached(&class, &field);
        if off < 0 {
            return;
        }
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.hook_read_u16_at_q else { return };
        let mut out: u16 = 0;
        if f(view, qslot, off, &mut out) != 0 {
            return;
        }
        rv.set_uint32(out as u32);
    }));
}

/// `__s2_hook_self_matches(entityRef, offset)` — does this live entity's pointer-at-offset equal
/// the detour `this`? Used by the game package to hop a services sub-object back to its pawn.
fn s2_hook_self_matches(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((view, _, _, _)) = active_hook() else { return };
        if args.length() < 2 {
            return;
        }
        let packed = pack_entity_arg(scope, args.get(0));
        const NO_ENTITY: u64 = 0xffff_ffff_ffff_ffff;
        if packed == NO_ENTITY {
            return;
        }
        let index = (packed >> 32) as i32;
        let serial = packed as u32 as i32;
        let offset = args.get(1).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.hook_self_matches_field else { return };
        rv.set_bool(f(view, index, serial, offset) == 1);
    }));
}


/// Synchronous output dispatch (entity-I/O slice). Called from `ffi.rs`'s
/// `s2script_core_dispatch_output` (a C-ABI export), which the shim's `FireOutputInternal` detour
/// calls with the firing entity's classname, the output name, packed activator/caller
/// `CEntityHandle` ints (-1 = none), the output's value as a string, and the delay. Runs every
/// `Entity.onOutput` subscriber whose key matches `(class,output)`, `(class,"*")`, `("*",output)`, or
/// `("*","*")`, collapses their returned `HookResult`s via `run_chain`, and returns the collapsed
/// value (0 Continue .. 3 Stop) — the caller supersedes (suppresses) the original `FireOutputInternal`
/// call when the result is >= Handled. Mirrors `dispatch_game_event_pre` / `dispatch_damage` (the
/// SYNCHRONOUS pre-hook pattern — a handler must be able to block), NOT the post-drain
/// `dispatch_pending_*` path. A `try_borrow_mut` graceful-skip guards re-entrancy (a handler firing
/// another output mid-dispatch without a nest token is skipped (`#63`); `acceptInput` publishes one).
pub(crate) fn dispatch_output(classname: &str, output: &str, act_handle: i32, caller_handle: i32, value: &str, delay: f32) -> i32 {
    // Snapshot every matching key, releasing the OUTPUT_MUX borrow before any JS runs. Dedup keys
    // that collapse onto the same string (a literal "*" classname/output would be unusual but
    // harmless) so a subscriber is never invoked twice for the same fire.
    let keys = [
        format!("{}\0{}", classname, output),
        format!("{}\0*", classname),
        format!("*\0{}", output),
        "*\0*".to_string(),
    ];
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut snap: Vec<(String, u64, v8::Global<v8::Function>)> = Vec::new();
    for k in &keys {
        if !seen.insert(k.as_str()) { continue; }
        snap.extend(OUTPUT_MUX.with(|m| m.borrow().snapshot(k)));
    }
    let result = fan_out_collapsing(&snap, "dispatch_output", Instrument::none(), StopAt::Stop, |tc| {
        // Build the ev object directly (no JS constructor needed — the data is already in hand,
        // unlike GameEvent/DamageInfo which read live shim state via further op calls).
        // NOTE: -1 is the EXACT sentinel the shim emits for "no entity" (a null pActivator/
        // pCaller), never a broad sign test — a live CEntityHandle::ToInt() packs a 17-bit
        // serial into the packed int's upper bits (HANDLE_ENTRY_BITS=15 in entity.rs), so a
        // real handle whose serial has climbed to >= 65536 is a genuinely negative i32 and
        // must still decode, not be misread as "none" (the same exact-sentinel convention the
        // engine-call `entity` return uses: reject `INVALID_ENTITY_HANDLE` and decode the rest).
        let activator_val: v8::Local<v8::Value> = if act_handle == -1 {
            v8::null(tc).into()
        } else {
            let (ai, aser) = crate::entity::decode_handle(act_handle as u32);
            match crate::entity_live::adopt(ai, aser) {
                Some(id) => build_entity_ref(tc, ai, id),
                None => v8::null(tc).into(),
            }
        };
        let caller_val: v8::Local<v8::Value> = if caller_handle == -1 {
            v8::null(tc).into()
        } else {
            let (ci, cser) = crate::entity::decode_handle(caller_handle as u32);
            match crate::entity_live::adopt(ci, cser) {
                Some(id) => build_entity_ref(tc, ci, id),
                None => v8::null(tc).into(),
            }
        };

        let ev_obj = v8::Object::new(tc);
        if let Some(k) = v8::String::new(tc, "output") {
            if let Some(v) = v8::String::new(tc, output) { ev_obj.set(tc, k.into(), v.into()); }
        }
        if let Some(k) = v8::String::new(tc, "activator") { ev_obj.set(tc, k.into(), activator_val); }
        if let Some(k) = v8::String::new(tc, "caller") { ev_obj.set(tc, k.into(), caller_val); }
        if let Some(k) = v8::String::new(tc, "value") {
            if let Some(v) = v8::String::new(tc, value) { ev_obj.set(tc, k.into(), v.into()); }
        }
        if let Some(k) = v8::String::new(tc, "delay") {
            let v = v8::Number::new(tc, delay as f64);
            ev_obj.set(tc, k.into(), v.into());
        }
        Some(vec![ev_obj.into()])
    });
    result as i32
}

/// Native `__s2_crash_set_game(name, build)` — the engine-generic setter the GAME PACKAGE calls to
/// stamp the game identity into the crash breadcrumb (core never knows which game; spec §5).
fn s2_crash_set_game(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        let build = args.get(1).uint32_value(scope).unwrap_or(0);
        crate::crash::breadcrumb::set_game(&name, build);
    }));
}

/// Native `__s2_server_build() -> number` — the engine's build number via the appended
/// `server_build_number` op (IVEngineServer2::GetBuildVersion; engine-generic). 0 = unavailable.
fn s2_server_build(
    _scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = ENGINE_OPS
            .with(|o| o.get())
            .and_then(|ops| ops.server_build_number)
            .map(|f| f())
            .unwrap_or(0);
        rv.set_int32(n);
    }));
}

/// Native `__s2_crash_test(kind) -> bool` — the deliberate-crash harness (spec §10). REFUSED
/// (returns false) unless crashreporter.json sets dev_test:true. kinds: "segv"/"abort" raise a
/// real native fault via the shim op; "panic" raises a Rust panic (recovered by catch_unwind,
/// reported by the panic hook); "js" is plugin-side (a plain throw) and unknown kinds refuse.
fn s2_crash_test(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if !crate::crash::config::load().dev_test {
            log_warn("WARN: __s2_crash_test refused (crashreporter.json dev_test is not true)");
            return;
        }
        let kind = args.get(0).to_rust_string_lossy(scope);
        match kind.as_str() {
            "panic" => {
                rv.set_bool(true);
                panic!("deliberate crash-harness panic (sm_crashtest panic)");
            }
            "segv" | "abort" => {
                let Some(f) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.crash_test_native) else {
                    log_warn("WARN: __s2_crash_test: crash_test_native op unavailable");
                    return;
                };
                rv.set_bool(true);
                f(if kind == "segv" { 0 } else { 1 }); // does not return
            }
            _ => {}
        }
    }));
}

/// The deferred-dispatch selftest's opt-in gate: `S2_DEFER_SELFTEST` set to ANYTHING arms it.
///
/// DEV-ONLY, off by default, and the gate is on INSTALLATION rather than on the call — with the
/// variable unset (every production process) `__s2_defer_selftest` is not a property of any
/// context's global at all, so no plugin can reach the synthetic re-entrancy even by accident.
/// Read on every `install_natives` (once per context, a `getenv`) rather than cached in a
/// `OnceLock`, so a test can arm and disarm it around a context and get the honest answer both
/// ways. The shim re-checks the SAME variable before doing anything, so arming needs the process
/// environment, not just a core-side flag.
fn defer_selftest_armed() -> bool {
    std::env::var_os("S2_DEFER_SELFTEST").is_some()
}

/// Native `__s2_defer_selftest() -> number` — DEV-ONLY (`S2_DEFER_SELFTEST`), the synthetic
/// re-entrancy that makes the deferred-dispatch queue's GAME-EVENT path live-provable.
///
/// The queue's scalar variants have natural triggers, but the game-event variant needs the engine
/// to dispatch an event back into core WHILE CORE HOLDS THE BORROW, and nothing on a bot-only dev
/// server does that: `Events.fire()` from a handler is delivered synchronously (CS2 does not route
/// a JS-fired event back through our listener inside the borrow), and `slay()`'s `player_death`
/// arrives a frame later from the engine's OWN delivery, not our drain. The genuine trigger is an
/// engine call that fires an event synchronously inside the borrow — which is exactly what A5b's
/// `Respawn`/`TerminateRound` descriptors now are, but reaching them needs a live player on a real
/// server, so the synthetic path stays the bot-only-server proof. This is the same reason
/// `S2_DAMAGE_SELFTEST` exists for the damage detour, and it carries the same discipline: env-gated,
/// off by default, loudly labelled, and NOT to be run in production — it dispatches a REAL event
/// name with FAKE field values to every subscribed plugin.
///
/// This native is only the doorway. Everything the selftest does happens in the shim, because the
/// shim is the side that owns an `IGameEvent` and the queue — and because the event name is a game
/// fact. Calling it from JS is what makes it work: core is inside `HOST.borrow_mut()` for the whole
/// native call, so the shim's dispatch into core MUST report `S2_DISPATCH_DEFERRED`.
///
/// Returns 1 (deferred + queued — the path ran), 0 (refused or degraded: op absent, shim gate off,
/// no event manager, `CreateEvent` failed, or duplication unavailable), or -1 (the dispatch was NOT
/// deferred, i.e. the isolate was free and the run proves nothing).
fn s2_defer_selftest(
    _scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(f) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.defer_selftest) else {
            log_warn("WARN: __s2_defer_selftest: defer_selftest op unavailable");
            rv.set_int32(0);
            return;
        };
        rv.set_int32(f());
    }));
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

/// The REGISTERING plugin's generation, for every subscribe-time liveness stamp (this module's
/// `subscribe_into`/`resolver_owner_tag` and the owner-tracked stores outside it).
///
/// Read from `REGISTRY`, NOT from `PLUGINS`: REGISTRY is the authority every dispatch-side
/// `is_live` check consults, and `create_plugin_context` registers the plugin there BEFORE
/// evaluating its preludes — so a prelude-time subscription already stamps the real generation
/// (`PLUGINS` only gets its `PluginInstance` after the context is built, which is what used to
/// stamp prelude subs with 0). `0` when `id` is not a registered plugin (the shared HOST /
/// `"legacy"` path) — a never-live sentinel the registry can no longer mint for a real plugin.
pub(crate) fn plugin_generation(id: &str) -> u64 {
    REGISTRY.with(|r| r.borrow().generation_of(id)).unwrap_or(0)
}

/// Isolate-wide promise-reject callback (registered once in `init`). Runs inside V8 while our
/// code is on the stack (during a checkpoint/eval), so a CallbackScope is the ONLY legal scope.
/// Never touches HOST (already borrowed by the caller); only the PENDING_REJECTS map.
unsafe extern "C" fn promise_reject_cb(msg: v8::PromiseRejectMessage) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        use v8::PromiseRejectEvent::*;
        let mut storage = unsafe { v8::CallbackScope::new(&msg) };
        let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let scope = &mut scope;
        let promise = msg.get_promise();
        let id = promise.get_identity_hash().get();
        match msg.get_event() {
            PromiseRejectWithNoHandler => {
                let (text, stack) = match msg.get_value() {
                    Some(v) => {
                        let text = v.to_rust_string_lossy(scope);
                        let stack = v8::Local::<v8::Object>::try_from(v)
                            .ok()
                            .and_then(|o| {
                                let k = v8::String::new(scope, "stack")?;
                                o.get(scope, k.into())
                            })
                            .map(|s| s.to_rust_string_lossy(scope))
                            .unwrap_or_default();
                        (text, stack)
                    }
                    None => ("unhandled rejection".to_string(), String::new()),
                };
                PENDING_REJECTS.with(|m| m.borrow_mut().insert(id, (text, stack)));
            }
            PromiseHandlerAddedAfterReject => {
                PENDING_REJECTS.with(|m| { m.borrow_mut().remove(&id); });
            }
            PromiseRejectAfterResolved | PromiseResolveAfterResolved => {}
        }
    }));
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



// Admin cache natives moved to `crate::admin`; ban cache natives + `ban_check` to `crate::bans`.


// Cookie cache natives moved to `crate::cookies`, which already owned the store — the feature is
// now whole in one module (state, natives, dispatch, teardown).




/// `__s2_voice_set_muted(slot, on)` -> bool. Voice-control slice: set/clear the shim-side per-slot
/// voice-mute flag (sender -> all receivers, enforced by the shim's SetClientListening rewrite).
/// Returns false when degraded (no op / bad slot / voice descriptor disabled) — the prelude setter
/// ignores it (degrade contract: inert no-op; the shim logs the named reason once).
fn s2_voice_set_muted(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 2 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let on = if args.get(1).boolean_value(scope) { 1 } else { 0 };
        if !crate::client::guarded(scope, &args, slot, 2) { return; }
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_set_muted else { return };
        rv.set_bool(f(slot, on) != 0);
    }));
}

/// `__s2_voice_get_muted(slot)` -> i32 (1 muted / 0 not / -1 degraded-or-invalid). The prelude getter
/// maps `=== 1` to boolean, so degraded reads are `false` (never a phantom mute).
fn s2_voice_get_muted(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        if !crate::client::guarded(scope, &args, slot, 1) { return; }
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_get_muted else { return };
        rv.set_int32(f(slot));
    }));
}





fn s2_server_command(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let cmd = args.get(0).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.server_command else { return };
        if let Ok(ccmd) = CString::new(cmd) {
            crate::nest::with_outbound(&args, || f(ccmd.as_ptr()));
        }
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

/// `__s2_server_max_clients() -> number` — the server's max client count. 0 without the op / null.
fn s2_server_max_clients(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = scope;
        let n: i32 = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.server_max_clients?;
            Some(f())
        })().unwrap_or(0);
        rv.set_double(n as f64);
    }));
}

/// `__s2_server_map_name() -> string` — the current map name (BSP). "" without the op / null.
fn s2_server_map_name(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let s: String = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.server_map_name?;
            let ptr = f();
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })().unwrap_or_default();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// `__s2_server_game_time() -> number` — the map time (GetGlobals()->curtime) in seconds. 0 without the op / null.
fn s2_server_game_time(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = scope;
        let t: f32 = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.server_game_time?;
            Some(f())
        })().unwrap_or(0.0);
        rv.set_double(t as f64);
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

/// `__s2_config_read_file(name) -> string | null` — raw configs-dir file read (name includes its
/// extension, e.g. "maplist.txt"); null if no op / file absent / name rejected (".."/empty).
/// Slice nominations Task 1. Mirrors `s2_config_read_raw`.
fn s2_config_read_file(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        ENGINE_OPS.with(|c| {
            let ops = c.get();
            if let Some(func) = ops.and_then(|o| o.config_read_file) {
                let cname = std::ffi::CString::new(name).unwrap_or_default();
                let p = func(cname.as_ptr());
                if !p.is_null() {
                    let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
                    if let Some(v) = v8::String::new(scope, &s) { rv.set(v.into()); }
                }
            }
        });
    }));
}

/// `__s2_config_write_file(name, content)` — raw configs-dir file write (creates/overwrites); a
/// no-op (never throws) with no op / a rejected name (".."/empty). Slice nominations Task 1.
/// Mirrors `s2_config_read_raw`'s ENGINE_OPS/CString access pattern.
fn s2_config_write_file(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let content = args.get(1).to_rust_string_lossy(scope);
        ENGINE_OPS.with(|c| {
            let ops = c.get();
            if let Some(func) = ops.and_then(|o| o.config_write_file) {
                // Abort on an interior NUL (either arg) rather than truncate — a content with an embedded
                // NUL must leave the target untouched, not write an empty/truncated file.
                let (Ok(cn), Ok(cc)) = (std::ffi::CString::new(name), std::ffi::CString::new(content)) else { return };
                func(cn.as_ptr(), cc.as_ptr());
            }
        });
    }));
}

// ---------------------------------------------------------------------------
// Slice DB Task 3: the `__s2_sqlite_*` natives, over `crate::db` (Task 1) + the `db_data_dir`
// engine op (Task 2). Every native returns a real `Promise` (the async API contract). `open`/
// `close` resolve/reject INLINE (no I/O to await — `open` does its one blocking file-open eagerly
// on the calling thread before spawning the actor; `close` just signals Shutdown). `query`/
// `execute` run OFF the game thread on a per-connection actor (`db::submit_query`/
// `submit_execute` hand off a `Command` and return immediately); the Promise resolves later via
// the shared `resolve_db` spine (mirrors the remote sqlx driver's `s2_db_remote_query`/
// `s2_db_remote_execute` exactly). A connection handle is ledgered against the CALLING plugin
// (`record_db_conn`) so an unclosed connection is closed at teardown (`Resource::DbConn` arm in
// `unload_plugin`). Degrade-never-crash: every body runs under `catch_unwind`; a bad handle / SQL
// error rejects the Promise, never panics/throws synchronously.
// ---------------------------------------------------------------------------

/// Copy SQL and parameters only after reserving their normalized bytes.
fn bounded_db_input(
    scope: &mut v8::PinScope,
    sql: v8::Local<v8::Value>,
    params: v8::Local<v8::Value>,
    lease: &mut crate::async_limits::JobLease,
) -> Result<(String, Vec<crate::db::DbValue>), String> {
    use crate::db::DbValue;
    let sql = crate::jobs::copy_string(scope, sql, lease).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Ok(arr) = v8::Local::<v8::Array>::try_from(params) {
        let count = arr.length() as usize;
        lease
            .input_grow(count.saturating_mul(32))
            .map_err(|e| e.to_string())?;
        out.reserve_exact(count);
        for i in 0..arr.length() {
            let v = arr
                .get_index(scope, i)
                .unwrap_or_else(|| v8::undefined(scope).into());
            let v = if v.is_null_or_undefined() {
                DbValue::Null
            } else if v.is_boolean() {
                DbValue::Int(i64::from(v.boolean_value(scope)))
            } else if v.is_number() {
                let n = v.number_value(scope).unwrap_or(0.0);
                if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
                    DbValue::Int(n as i64)
                } else {
                    DbValue::Real(n)
                }
            } else {
                DbValue::Text(crate::jobs::copy_string(scope, v, lease).map_err(|e| e.to_string())?)
            };
            out.push(v);
        }
    }
    Ok((sql, out))
}

/// `DbValue` -> a JS value in `scope`'s current context. `Int`/`Real` -> `Number` (a value beyond
/// 2^53 loses precision — documented; 64-bit ids should be stored/read as `Text`). `Text` ->
/// `String`. `Null` -> `null`.
fn db_value_to_v8<'s>(scope: &mut v8::PinScope<'s, '_>, v: &crate::db::DbValue) -> v8::Local<'s, v8::Value> {
    use crate::db::DbValue;
    match v {
        DbValue::Null => v8::null(scope).into(),
        DbValue::Int(i) => v8::Number::new(scope, *i as f64).into(),
        DbValue::Real(f) => v8::Number::new(scope, *f).into(),
        // A value that exceeds V8's max string length yields None — fall back to "" (empty always
        // succeeds) rather than panicking into `undefined` (an absurd-size TEXT edge; no crash).
        DbValue::Text(s) => v8::String::new(scope, s)
            .unwrap_or_else(|| v8::String::new(scope, "").unwrap())
            .into(),
    }
}

/// Resolve the s2script data directory via the `db_data_dir` engine op, or `None` if the op table
/// / the function pointer is absent (degrade path — `open` then rejects "db not available").
fn db_data_dir() -> Option<String> {
    ENGINE_OPS.with(|o| o.get())
        .and_then(|ops| ops.db_data_dir)
        .map(|f| unsafe { std::ffi::CStr::from_ptr(f()) }.to_string_lossy().into_owned())
}

/// Native `__s2_sqlite_open(name: string) -> Promise<number>`. Opens (or creates)
/// `<data_dir>/<name>.sqlite` and resolves the opaque connection handle; ledgers it against the
/// CALLING plugin. Rejects on an invalid name, an unavailable data dir (no engine op), or an
/// open failure.
fn s2_sqlite_open(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner_tag = resolver_owner_tag(scope);
        let owner = current_plugin(scope).unwrap_or_default();
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let result = (|| -> Result<u64, String> {
            let lifetime = crate::async_limits::domain()
                .sqlite
                .acquire(owner_tag.clone(), 1, 0)
                .map_err(|e| e.to_string())?;
            let name = args
                .get(0)
                .to_string(scope)
                .ok_or("invalid database name")?;
            if name.utf8_length(scope) > 64 {
                return Err("AsyncPayloadTooLarge".into());
            }
            let name = name.to_rust_string_lossy(scope);
            if owner_tag
                .as_ref()
                .is_some_and(|(id, generation)| !owner_is_live(id, *generation))
            {
                return Err("AsyncCancelled".into());
            }
            let dir = db_data_dir().ok_or("db not available")?;
            crate::db::open_reserved(
                std::path::Path::new(&dir),
                &name,
                &owner,
                crate::async_limits::domain(),
                lifetime,
            )
        })();
        match result {
            Ok(handle) => {
                // Ledger the connection against the CALLING plugin (teardown authority) — a
                // non-plugin/unknown owner (the shared HOST context) is a safe no-op.
                if let Some((ref oid, generation)) = owner_tag {
                    record_resource(oid, generation, plugin::Resource::DbConn(handle));
                }
                resolver.resolve(scope, v8::Number::new(scope, handle as f64).into());
            }
            Err(e) => {
                crate::jobs::reject(scope, resolver, &e);
            }
        }
        request_microtask_drain();
        rv.set(promise.into());
    }));
}

/// Build the JS `Row[]` (array of {col: value}) from a `QueryResult`. Shared by the sync SQLite
/// path (`s2_sqlite_query`) and the async remote-resolve path (`resolve_db`). Delegates each cell
/// to `db_value_to_v8` (`Int`/`Real` -> `Number`, `Text` -> `String`, `Null` -> `null`).
fn query_result_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    q: &crate::db::QueryResult,
) -> v8::Local<'s, v8::Value> {
    let arr = v8::Array::new(scope, q.rows.len() as i32);
    for (ri, row) in q.rows.iter().enumerate() {
        let obj = v8::Object::new(scope);
        for (ci, col) in q.columns.iter().enumerate() {
            let key = v8::String::new(scope, col).unwrap();
            let val = db_value_to_v8(scope, &row[ci]);
            obj.create_data_property(scope, key.into(), val);
        }
        let index = v8::String::new(scope, &ri.to_string()).unwrap();
        arr.create_data_property(scope, index.into(), obj.into());
    }
    arr.into()
}

/// Native `__s2_sqlite_query(handle, sql, params) -> Promise<Row[]>`. Owner-checks + queues the SELECT
/// on the connection's actor thread (`db::submit_query`); the Promise resolves later via `resolve_db`
/// with the row array. An invalid handle / closed connection rejects the Promise immediately, with no
/// RESOLVERS/PENDING_JOBS/ledger entry (no pending job to track). MIRRORS `s2_db_remote_query`.
fn s2_sqlite_query(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let owner = current_plugin(scope).unwrap_or_default();
        let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        let (sql, params) = bounded_db_input(scope, args.get(1), args.get(2), &mut lease)?;
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        crate::db::submit_query_reserved(id, handle, sql, params, &owner, lease)?;
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

/// Native `__s2_sqlite_execute(handle, sql, params) -> Promise<{changes, lastInsertId}>`. Same shape
/// as `s2_sqlite_query` but queues an INSERT/UPDATE/DELETE/DDL (`db::submit_execute`); resolves later
/// via `resolve_db` with `{changes, lastInsertId}`.
fn s2_sqlite_execute(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let owner = current_plugin(scope).unwrap_or_default();
        let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        let (sql, params) = bounded_db_input(scope, args.get(1), args.get(2), &mut lease)?;
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        crate::db::submit_execute_reserved(id, handle, sql, params, &owner, lease)?;
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

/// Native `__s2_sqlite_close(handle) -> Promise<void>`. Closes the connection (a harmless no-op
/// if already closed / never open) and always resolves `undefined` — teardown may later close the
/// same handle again (idempotent), so `close()` never rejects.
fn s2_sqlite_close(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1);
        let owner = current_plugin(scope).unwrap_or_default();
        let generation = REGISTRY.with(|r| r.borrow().generation_of(&owner));
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        if handle >= 0 {
            let handle = handle as u64;
            if crate::db::close(handle, &owner) {
                if let Some(generation) = generation {
                    release_resource(&owner, generation, &plugin::Resource::DbConn(handle));
                }
            }
        }
        let undef = v8::undefined(scope);
        resolver.resolve(scope, undef.into());
        request_microtask_drain();
        rv.set(promise.into());
    }));
}

// ---------------------------------------------------------------------------
// Remote SQL driver Task 2: the `__s2_db_remote_*` natives — MySQL/Postgres over the
// process-global tokio+sqlx runtime (core/src/sqldb.rs, Task 1). `connect` is synchronous (no I/O —
// the pool connects lazily on first query); `query`/`execute` MIRROR `s2_fetch`'s
// resolver/ledger(`record_job`)/RESOLVERS/PENDING_JOBS/refresh_detour block exactly (a `Job`
// resource — teardown drops its `RESOLVERS` entry, and a completion for an unloaded/reloaded plugin
// is DROPPED by the async-liveness guard in the drain step, never resolved) — the calling
// (main/game) thread never blocks; the Promise resolves on a LATER `frame_async_drain` via
// `resolve_db`. Note: the async remote-query/execute path reuses `js_params_to_db` (Task 3's
// sqlite-params helper) rather than a separate `js_params_to_dbvalues` — both natives bind against
// the SAME shared `crate::db::DbValue` sqldb.rs consumes, so a second byte-identical mapping would
// be pure duplication.
// ---------------------------------------------------------------------------

/// Native `__s2_db_remote_connect(configJson) -> number`. Builds+registers a lazy MySQL/Postgres
/// pool (`sqldb::connect`) and returns the opaque handle as a `Number` (0 on failure, never
/// throws). Ledgers the handle against the CALLING plugin (`RemoteDbConn`) so an unclosed pool is
/// dropped at teardown. MIRRORS `s2_sqlite_open`'s ledger block (synchronous, not Promise-returning
/// — `connect` does no I/O, so there's nothing to await).
fn s2_db_remote_connect(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner_tag = resolver_owner_tag(scope);
        let owner = current_plugin(scope).unwrap_or_default();
        let result = (|| -> Result<u64, String> {
            let cfg = args
                .get(0)
                .to_string(scope)
                .ok_or("invalid database config")?;
            let bytes = cfg.utf8_length(scope);
            if bytes > crate::async_limits::policy().input_item_bytes {
                return Err("AsyncPayloadTooLarge".into());
            }
            // Cover the JSON input, parsed fields and connection-option copies for the pool lifetime.
            let retained = bytes
                .checked_mul(4)
                .and_then(|n| n.checked_add(256))
                .ok_or("AsyncPayloadTooLarge")?;
            let lifetime = std::sync::Arc::new(
                crate::async_limits::domain()
                    .pools
                    .acquire(owner_tag.clone(), 1, retained)
                    .map_err(|e| e.to_string())?,
            );
            let cfg = cfg.to_rust_string_lossy(scope);
            if owner_tag
                .as_ref()
                .is_some_and(|(id, generation)| !owner_is_live(id, *generation))
            {
                return Err("AsyncCancelled".into());
            }
            crate::sqldb::connect_reserved(&cfg, &owner, lifetime)
        })();
        match result {
            Ok(handle) => {
                // Ledger the connection against the CALLING plugin (teardown authority) — a
                // non-plugin/unknown owner (the shared HOST context) is a safe no-op.
                if let Some((ref oid, generation)) = owner_tag {
                    record_resource(oid, generation, plugin::Resource::RemoteDbConn(handle));
                }
                rv.set(v8::Number::new(scope, handle as f64).into());
            }
            Err(_e) => rv.set(v8::Number::new(scope, 0.0).into()),
        }
    }));
}

/// Native `__s2_db_remote_query(handle, sql, params) -> Promise<Row[]>`. Resolves the owner-scoped
/// pool for `handle` (a wrong/absent handle is "invalid db handle", never probeable), then spawns
/// the SELECT on the shared tokio+sqlx runtime; the Promise resolves later via `resolve_db` with the
/// row array (`query_result_to_js`). An invalid handle rejects the Promise IMMEDIATELY and
/// synchronously — no `RESOLVERS` entry / `PENDING_JOBS` increment / ledger entry is ever made for
/// that early-reject path (there is no pending job to track or tear down).
fn s2_db_remote_query(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let owner = current_plugin(scope).unwrap_or_default();
        let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        let (sql, params) = bounded_db_input(scope, args.get(1), args.get(2), &mut lease)?;
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        let pool = crate::sqldb::get_pool(handle, &owner)?;
        crate::sqldb::spawn_query(id, pool, sql, params, lease);
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

/// Native `__s2_db_remote_execute(handle, sql, params) -> Promise<{changes, lastInsertId}>`. Same
/// shape as `s2_db_remote_query` (owner-scoped pool resolve + early-reject-on-invalid-handle, then
/// the `s2_fetch`-mirrored resolver/ledger/RESOLVERS/PENDING_JOBS/refresh_detour block), but spawns
/// an INSERT/UPDATE/DELETE/DDL statement (`spawn_execute`); the Promise resolves later via
/// `resolve_db` with `{changes, lastInsertId}`.
fn s2_db_remote_execute(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let owner = current_plugin(scope).unwrap_or_default();
        let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        let (sql, params) = bounded_db_input(scope, args.get(1), args.get(2), &mut lease)?;
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        let pool = crate::sqldb::get_pool(handle, &owner)?;
        crate::sqldb::spawn_execute(id, pool, sql, params, lease);
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

/// Native `__s2_db_remote_close(handle) -> Promise<void>`. MIRRORS `s2_sqlite_close`: closes the
/// pool (a harmless no-op if already closed / never open, regardless of the `sqldb::close`
/// bool-return) and always resolves `undefined` — teardown may later close the same handle again
/// (idempotent), so `close()` never rejects.
fn s2_db_remote_close(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1);
        let owner = current_plugin(scope).unwrap_or_default();
        let generation = REGISTRY.with(|r| r.borrow().generation_of(&owner));
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        if handle >= 0 {
            let handle = handle as u64;
            if crate::sqldb::close(handle, &owner) {
                if let Some(generation) = generation {
                    release_resource(&owner, generation, &plugin::Resource::RemoteDbConn(handle));
                }
            }
        }
        let undef = v8::undefined(scope);
        resolver.resolve(scope, undef.into());
        request_microtask_drain();
        rv.set(promise.into());
    }));
}

/// Resolve (or drop, on the async-liveness guard) a completed remote DB query/execute job in its
/// OWNING plugin's context — MIRRORS `resolve_fetch`'s owner-liveness + context-clone +
/// HandleScope/ContextScope preamble exactly (the use-after-free killer: never resolve into a
/// disposed/replaced context), but resolves with the row array (`query_result_to_js`) or the
/// `{changes, lastInsertId}` object on `Ok`, or rejects with an `Error` on `Err` (a SQL/connection
/// failure surfaced by `sqldb::run_query`/`run_execute`).
fn resolve_db(
    host: &mut Host,
    entry: &crate::jobs::ResolverEntry,
    result: Result<crate::db::DbOutcome, String>,
) {
    crate::jobs::settle_if_live(
        &mut host.isolate,
        &host.context,
        entry,
        |scope, resolver| match result {
            Ok(crate::db::DbOutcome::Query(qr)) => {
                let v = query_result_to_js(scope, &qr);
                resolver.resolve(scope, v);
            }
            Ok(crate::db::DbOutcome::Exec(er)) => {
                let obj = v8::Object::new(scope);
                let k1 = v8::String::new(scope, "changes").unwrap();
                let v1 = v8::Number::new(scope, er.changes as f64);
                let k2 = v8::String::new(scope, "lastInsertId").unwrap();
                let v2 = v8::Number::new(scope, er.last_insert_id as f64);
                obj.create_data_property(scope, k1.into(), v1.into());
                obj.create_data_property(scope, k2.into(), v2.into());
                resolver.resolve(scope, obj.into());
            }
            Err(e) => {
                crate::jobs::reject(scope, resolver, &e);
            }
        },
    );
}


pub fn init(logger: LogFn) -> Result<(), String> {
    ensure_platform();
    LOGGER.with(|l| l.set(Some(logger)));
    // Slice HTTP Task 2: build the process-global tokio+reqwest engine (idempotent — a OnceLock,
    // survives a Metamod re-init just like `pool()`). Holds no V8 handles; wiring it here (rather
    // than lazily on first `__s2_fetch` call) keeps engine-generic subsystem setup in one place.
    crate::http::init();

    UPTIME_START.with(|t| if t.get().is_none() { t.set(Some(Instant::now())) });
    crate::crash::breadcrumb::clear_plugins(); // establishes the "core"/"idle" idle stamp

    let mut isolate = v8::Isolate::new(v8::CreateParams::default());

    // We own the microtask checkpoint: with Explicit policy, await/.then continuations run ONLY
    // when we call perform_microtask_checkpoint() in frame_async_drain (once per frame).
    isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

    // D-2: isolate-wide fatal-JS capture for unhandled promise rejections. The callback records
    // rejections into PENDING_REJECTS; the frame_async_drain flush reports whatever survived the
    // end-of-frame microtask checkpoint (a later .catch cancels its entry).
    isolate.set_promise_reject_callback(promise_reject_cb);

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
    // Self-register the owner-scoped teardown stores (design spec §6). Runs last so every init path
    // (including the in-isolate test harness) gets the registry; `register_builtin_stores` resets the
    // list first, so a Metamod re-init is idempotent.
    register_builtin_stores();
    register_process_singletons();
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

    if phase == Phase::Pre {
        crate::crash::breadcrumb::note_tick(
            FRAME_COUNTER.with(|c| c.get()),
            UPTIME_START.with(|t| t.get().map(|s| s.elapsed().as_secs() as u32).unwrap_or(0)),
        );
    }

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

            let _crash_guard = crate::crash::breadcrumb::enter_dispatch(
                owner,
                if phase == Phase::Pre { "OnGameFrame:pre" } else { "OnGameFrame:post" },
            );

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

            crate::crash::breadcrumb::note_js_location(
                owner,
                v8::Local::new(tc, &jh.func).get_script_line_number().map(|l| l + 1).unwrap_or(0),
            );

            let func = v8::Local::new(tc, &jh.func);
            match func.call(tc, recv, &[ctx_val]) {
                // Exception thrown (or otherwise empty): report (kind=js) then count the error.
                None => {
                    let msg = tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "uncaught exception".into());
                    let stack = tc.stack_trace()
                        .map(|s| s.to_rust_string_lossy(&*tc))
                        .unwrap_or_default();
                    crate::crash::report_js_error(
                        owner,
                        if phase == Phase::Pre { "OnGameFrame:pre" } else { "OnGameFrame:post" },
                        &msg,
                        &stack,
                    );
                    Err(())
                }
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
    // Invalidate and join the dedicated loader before any plugin/V8 state or this library can be
    // torn down. The worker owns no V8 or engine callback pointers.
    crate::loader::shutdown_worker();
    // Run per-plugin teardown (onUnload + ledger) in reverse-dependency order BEFORE any bulk clears,
    // so each plugin's onUnload fires while the isolate + other plugins are still alive.
    // The bulk clears below are the final backstop for anything not already cleaned up by unload_all.
    unload_all();

    // Everything below used to be a ~90-line hand-written cascade: one clear per thing, extended by
    // hand for every capability slice, and silently keeping stale state on the ones where that was
    // forgotten (98cf483, e40492d, 7e62119 are three shipped fixes of exactly that shape). It is now
    // two registries swept in three calls. Adding a capability slice should never add a line here:
    // register the store (owner-scoped) or the singleton (process-scoped) next to its definition.
    //
    // The ordering around `HOST.take()` is load-bearing and is what `ResetPhase` encodes: anything
    // holding a `v8::Global` must release it while the isolate is still alive.
    crate::process_singletons::reset_all(crate::process_singletons::ResetPhase::BeforeIsolateDrop);
    crate::owner_stores::sweep_reset();

    // Drop the isolate and context.  The platform is never torn down.
    HOST.with(|h| {
        let _ = h.borrow_mut().take();
    });

    crate::process_singletons::reset_all(crate::process_singletons::ResetPhase::AfterIsolateDrop);
    crate::ws::shutdown_all();
    crate::net::shutdown_all();
    crate::db::shutdown_all();
    crate::sqldb::shutdown_all();
    crate::async_limits::stop_delivery(|| {
        while pool().try_recv_completed().is_some() {}
        while crate::http::try_recv_completed().is_some() {}
        while crate::db::try_recv_completed().is_some() {}
        while crate::ws::try_recv_signal().is_some() {}
        while crate::net::try_recv_signal().is_some() {}
    });
}

/// A repeating-or-one-shot callback timer. `interval_ms` is `Some` for `Timers.every`, in which
/// case the drain re-arms it after each fire; `None` is a one-shot that is removed after firing.
struct TimerCallback {
    owner: Option<(String, u64)>,
    cb: v8::Global<v8::Function>,
    interval_ms: Option<u64>,
}

/// Fire a callback timer in its OWNER's context. Mirrors `resolve_or_drop`'s liveness guard exactly
/// — never call into a disposed/replaced context. Returns false when the owner is gone, which tells
/// the drain to drop the timer instead of re-arming it (a repeating timer whose plugin unloaded must
/// not keep the frame detour alive forever).
fn fire_timer_cb(host: &mut Host, entry: &TimerCallback) -> bool {
    let g_ctx = match &entry.owner {
        Some((id, generation)) => {
            if !REGISTRY.with(|r| r.borrow().is_live(id, *generation)) { return false; }
            match PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.context.clone())) {
                Some(g) => g,
                None => return false,
            }
        }
        None => host.context.clone(),
    };
    let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
    let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
    let hs = &mut hs;
    let ctx_local = v8::Local::new(hs, &g_ctx);
    let scope = &mut v8::ContextScope::new(hs, ctx_local);
    let mut tc_storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
    let tc = &mut tc;
    let f = v8::Local::new(tc, &entry.cb);
    let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
    if f.call(tc, recv, &[]).is_none() {
        // A throwing timer callback must not kill the timer system or the frame. Report and carry
        // on — the same per-handler containment posture the multiplexer uses.
        let msg = tc.exception()
            .map(|e| e.to_rust_string_lossy(&*tc))
            .unwrap_or_else(|| "timer callback threw".into());
        let who = entry.owner.as_ref().map(|(id, _)| id.as_str()).unwrap_or("<host>");
        log_warn(&format!("WARN: timer callback threw (plugin '{who}'): {msg}"));
    }
    true
}

fn resolve_or_drop(host: &mut Host, entry: &crate::jobs::ResolverEntry) {
    crate::jobs::resolve_undefined(&mut host.isolate, &host.context, entry);
}

/// Resolve (or drop, on the async-liveness guard) a completed `__s2_fetch` job in its OWNING
/// plugin's context — MIRRORS `resolve_or_drop`'s owner-liveness + context-clone +
/// HandleScope/ContextScope preamble exactly (the use-after-free killer: never resolve into a
/// disposed/replaced context), but builds the raw `{status, ok, statusText, headers, body}`
/// Response payload on `Ok`, or rejects with an `Error` on `Err` (a network/timeout failure),
/// instead of `resolve_or_drop`'s bare `undefined`.
fn resolve_fetch(
    host: &mut Host,
    entry: &crate::jobs::ResolverEntry,
    result: Result<crate::http::FetchResponse, String>,
) {
    crate::jobs::settle_if_live(
        &mut host.isolate,
        &host.context,
        entry,
        |scope, resolver| match result {
            Ok(r) => {
                let obj = v8::Object::new(scope);
                let status_key = v8::String::new(scope, "status").unwrap();
                obj.create_data_property(
                    scope,
                    status_key.into(),
                    v8::Number::new(scope, r.status as f64).into(),
                );
                let ok_key = v8::String::new(scope, "ok").unwrap();
                obj.create_data_property(
                    scope,
                    ok_key.into(),
                    v8::Boolean::new(scope, (200..300).contains(&r.status)).into(),
                );
                let status_text_key = v8::String::new(scope, "statusText").unwrap();
                let status_text_val = v8::String::new(scope, &r.status_text)
                    .unwrap_or_else(|| v8::String::new(scope, "").unwrap());
                obj.create_data_property(scope, status_text_key.into(), status_text_val.into());
                let hobj = v8::Object::new(scope);
                for (k, v) in &r.headers {
                    let Some(kk) = v8::String::new(scope, k) else {
                        continue;
                    };
                    let vv = v8::String::new(scope, v)
                        .unwrap_or_else(|| v8::String::new(scope, "").unwrap());
                    hobj.create_data_property(scope, kk.into(), vv.into());
                }
                let headers_key = v8::String::new(scope, "headers").unwrap();
                obj.create_data_property(scope, headers_key.into(), hobj.into());
                let body_key = v8::String::new(scope, "body").unwrap();
                let body_val = v8::String::new(scope, &r.body)
                    .unwrap_or_else(|| v8::String::new(scope, "").unwrap());
                obj.create_data_property(scope, body_key.into(), body_val.into());
                resolver.resolve(scope, obj.into());
            }
            Err(e) => {
                crate::jobs::reject(scope, resolver, &e);
            }
        },
    );
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
fn deliver_timer(host: &mut Host, id: u64) {
    let lease = TIMER_LEASES.with(|m| m.borrow_mut().remove(&id));
    // A CALLBACK timer fires its function and, when repeating, re-arms. Take the entry out
    // while firing so a callback that kills its own timer (or creates one) cannot observe a
    // half-updated map or double-borrow TIMER_CBS.
    if let Some(cb) = TIMER_CBS.with(|m| m.borrow_mut().remove(&id)) {
        // Clear any stale record for this id, fire, then ask whether the callback killed it.
        TIMER_KILLED.with(|k| {
            k.borrow_mut().remove(&id);
        });
        let owner_live = fire_timer_cb(host, &cb);
        // Re-arm only if it repeats AND the callback did not kill itself during the fire
        // AND the owner is still live. Otherwise the entry stays removed and it is done.
        let self_killed = TIMER_KILLED.with(|k| k.borrow_mut().remove(&id));
        if let (Some(iv), true, false) = (cb.interval_ms, owner_live, self_killed) {
            TIMERS.with(|t| {
                t.borrow_mut().push(
                    id,
                    TimerKind::Deadline(Instant::now() + Duration::from_millis(iv)),
                )
            });
            TIMER_CBS.with(|m| m.borrow_mut().insert(id, cb));
            if let Some(lease) = lease {
                TIMER_LEASES.with(|m| m.borrow_mut().insert(id, lease));
            }
        } else if !self_killed {
            if let Some((owner, generation)) = &cb.owner {
                release_resource(owner, *generation, &plugin::Resource::Timer(id));
            }
        }
        return;
    }
    // Remove the tagged resolver (RESOLVERS borrow released), then resolve-or-drop it in its
    // owner's context.  A None entry means the timer was already dropped (e.g. by unload).
    let Some(entry) = crate::jobs::take_resolver(id) else {
        return;
    };
    crate::jobs::release_timer(&entry, id);
    resolve_or_drop(host, &entry);
}
pub(crate) fn frame_async_drain() {
    crate::surface_leases::advance_frame();
    crate::shared_entity_switch::retry_pending();
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };

        let callbacks = crate::cookies::pending_cached()
            || crate::ws::pending_events()
            || crate::net::pending_events();
        crate::async_limits::begin_frame(callbacks);
        let frame = FRAME_COUNTER.with(|c| c.get());
        let n = crate::async_limits::policy()
            .frame_items
            .saturating_sub(DUE_TIMERS.with(|q| q.borrow().len()));
        let due = TIMERS.with(|t| t.borrow_mut().due_limited(Instant::now(), frame, n));
        DUE_TIMERS.with(|q| q.borrow_mut().extend(due));
        let mut ws_drops = Vec::new();
        let mut net_drops = Vec::new();
        let mut empty = 0;
        let mut frame_cursor = None;
        while crate::async_limits::can_deliver(0, true) && crate::async_limits::poll_frame() {
            // Rotate the first source independently of the number of polls. Advancing only
            // when this phase actually polls also avoids locking to alternate callback frames.
            let source = *frame_cursor.get_or_insert_with(|| {
                POLL_CURSOR.with(|v| {
                    let first = v.get();
                    v.set((first + 1) % 6);
                    first
                })
            });
            frame_cursor = Some((source + 1) % 6);
            let mut progress = false;
            match source {
                0 => {
                    if let Some(id) = DUE_TIMERS.with(|q| q.borrow_mut().pop_front()) {
                        crate::async_limits::deliver(0);
                        deliver_timer(host, id);
                        progress = true;
                    }
                }
                1 => {
                    if let Some((id, res, _lease)) = pool().try_recv_completed() {
                        crate::async_limits::deliver(0);
                        progress = true;
                        if let Some(entry) = crate::jobs::complete_job(id) {
                            crate::jobs::settle_if_live(
                                &mut host.isolate,
                                &host.context,
                                &entry,
                                |scope, resolver| match res {
                                    Ok(()) => {
                                        let u = v8::undefined(scope);
                                        resolver.resolve(scope, u.into());
                                    }
                                    Err(e) => crate::jobs::reject(scope, resolver, &e),
                                },
                            );
                        }
                    }
                }
                2 => {
                    let c = PARKED_HTTP
                        .with(|v| v.borrow_mut().take())
                        .or_else(crate::http::try_recv_completed);
                    if let Some(c) = c {
                        if crate::async_limits::can_deliver(c.lease.bytes(), true) {
                            crate::async_limits::deliver(c.lease.bytes());
                            progress = true;
                            if let Some(entry) = crate::jobs::complete_job(c.id) {
                                resolve_fetch(host, &entry, c.result);
                            }
                        } else {
                            PARKED_HTTP.with(|v| *v.borrow_mut() = Some(c));
                        }
                    }
                }
                3 => {
                    let c = PARKED_DB
                        .with(|v| v.borrow_mut().take())
                        .or_else(crate::db::try_recv_completed);
                    if let Some(c) = c {
                        if crate::async_limits::can_deliver(c.lease.bytes(), true) {
                            crate::async_limits::deliver(c.lease.bytes());
                            progress = true;
                            if let Some(entry) = crate::jobs::complete_job(c.id) {
                                resolve_db(host, &entry, c.result);
                            }
                        } else {
                            PARKED_DB.with(|v| *v.borrow_mut() = Some(c));
                        }
                    }
                }
                4 => {
                    if crate::async_limits::can_deliver(
                        crate::async_limits::policy().failure_bytes,
                        true,
                    ) {
                        let poll = crate::ws::poll_signals_limited(1);
                        progress = poll.polled > 0;
                        for (id, result) in poll.connects {
                            crate::async_limits::deliver(
                                crate::async_limits::policy().failure_bytes,
                            );
                            if let Some(entry) = crate::jobs::complete_job(id) {
                                resolve_ws_connect(host, &entry, id, result);
                            }
                        }
                        ws_drops.extend(poll.drops);
                    }
                }
                _ => {
                    if crate::async_limits::can_deliver(
                        crate::async_limits::policy().failure_bytes,
                        true,
                    ) {
                        let poll = crate::net::poll_signals_limited(1);
                        progress = poll.polled > 0;
                        for (id, result) in poll.connects {
                            crate::async_limits::deliver(
                                crate::async_limits::policy().failure_bytes,
                            );
                            if let Some(entry) = crate::jobs::complete_job(id) {
                                resolve_net_connect(host, &entry, id, result);
                            }
                        }
                        net_drops.extend(poll.drops);
                    }
                }
            }
            if progress {
                empty = 0;
            } else {
                empty += 1;
                if empty >= 6 {
                    break;
                }
            }
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
        MICROTASK_DRAIN_NEEDED.with(|v| v.set(false));
        scope.perform_microtask_checkpoint();

        // NOW deregister the conns whose terminal signal arrived above. Every continuation queued by
        // this drain has run, so a `.then` that subscribes to the connection it was just handed has
        // already been able to do so. Dropping earlier is what made a server dying right after the
        // handshake look like a connection that simply never spoke.
        for id in ws_drops {
            crate::ws::retire_conn(id);
        }
        for id in net_drops {
            crate::net::retire_conn(id);
        }
    });
    // HOST + scope released: a just-completed last timer may make the detour undesired, or a
    // continuation may have queued new async keeping it desired.  Reconcile now.
    refresh_detour();
    // D-2: whatever unhandled rejections survived the checkpoint are now final — report them.
    let pending: Vec<(String, String)> =
        PENDING_REJECTS.with(|m| m.borrow_mut().drain().map(|(_, v)| v).collect());
    for (message, stack) in pending {
        // Owner attribution for a rejection is best-effort: the rejecting plugin's dispatch has
        // already unwound, so attribute to the breadcrumb's ring-latest plugin.
        let bc = crate::crash::breadcrumb::snapshot();
        let last = (bc.ring_head as usize + crate::crash::breadcrumb::RING_LEN - 1)
            % crate::crash::breadcrumb::RING_LEN;
        let owner = crate::crash::breadcrumb::read_cstr(&bc.ring[last].plugin);
        crate::crash::report_js_error(
            if owner.is_empty() { "unknown" } else { &owner },
            "unhandled-rejection",
            &message,
            &stack,
        );
    }
    crate::crash::uploader::periodic_sweep();
    // L1 lifecycle v2: drive in-flight factory loads to Active/Failed. Runs HOST-free, AFTER the
    // microtask checkpoint above ran any async factory continuations (which settled their LOADING
    // entries), so an async plugin transitions on the same drain its promise resolved.
    finalize_loading_plugins();
}

/// HOST-free callback phase shares the logical delivery budget, with its own persistent source
/// cursor including cookies. Alternate busy pre/post turns reserve progress even at one item.
fn s2_async_stats(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let mut stats = crate::async_limits::metrics();
    stats["loader"] = crate::loader::metrics();
    stats["staged"] = serde_json::json!({"timers":DUE_TIMERS.with(|q|q.borrow().len()),"ws":crate::ws::pending_count(),"net":crate::net::pending_count(),"cookies":crate::cookies::pending_count(),"http":PARKED_HTTP.with(|q|usize::from(q.borrow().is_some())),"db":PARKED_DB.with(|q|usize::from(q.borrow().is_some()))});
    stats["cache"] = crate::cookies::cache_stats();
    // Private aggregate diagnostics: fixed-size output, no handles or plugin identities.
    let (watches, callbacks, attachments, disposers, pending) = interop_lifetime::counts();
    let ledger = REGISTRY.with(|r| {
        let r = r.borrow();
        r.ids().iter().filter_map(|id| r.generation_of(id)
            .and_then(|generation| r.active_resource_count(id, generation))).sum::<usize>()
    });
    stats["interop"] = serde_json::json!({
        "watches": watches, "callbacks": callbacks, "attachments": attachments,
        "disposers": disposers, "pending": pending,
        "subscriptions": IFACE_SUBS.with(|m| m.borrow().len()),
        "methods": IFACE_METHODS.with(|m| m.borrow().len()), "ledger": ledger,
    });
    stats["timerExamined"] = serde_json::json!(crate::async_rt::timer_examined());
    let json = stats.to_string();
    if let Some(v) = v8::String::new(scope, &json) {
        rv.set(v.into());
    }
}
pub(crate) fn dispatch_async_callbacks() {
    let mut empty = 0;
    for _ in 0..crate::async_limits::policy().frame_items.saturating_mul(3) {
        if !crate::async_limits::can_deliver(0, false) {
            break;
        }
        let source = CALLBACK_CURSOR.with(|v| {
            let s = v.get();
            v.set((s + 1) % 3);
            s
        });
        let done = match source {
            0 => crate::cookies::dispatch_one_cached(),
            1 => crate::ws::dispatch_one(),
            _ => crate::net::dispatch_one(),
        };
        if done {
            // HOST-free handlers run after this frame's checkpoint. Even a last terminal
            // callback may enqueue a promise continuation after releasing every socket lease.
            request_microtask_drain();
            empty = 0;
        } else {
            empty += 1;
            if empty >= 3 {
                break;
            }
        }
    }
    crate::async_limits::finish_frame();
    refresh_detour();
}

/// Register every builtin owner-scoped subscription store into the `owner_stores` registry
/// (design spec §6). Called at the end of `init()` (after `owner_stores::reset()`, which this fn
/// re-runs so a Metamod re-init is idempotent), so `unload_plugin` can sweep the registry instead of
/// a hand-maintained cascade. Each store carries its historical follow-up engine-op verbatim, and
/// registration order == the historical cascade order (which `sweep_owner` preserves 1:1). The
/// `remove_by_ids` closures are the Scope-disposal path (T3): they mirror `remove_by_owner`'s
/// follow-up for the mux stores and are a no-op for stores that are not scope surfaces.
pub(crate) fn register_builtin_stores() {
    crate::owner_stores::reset();

    // FRAME (OnGameFrame): drop the plugin's handler Globals + reconcile the detour. The ids path is
    // a no-op — frame subs dispose via their own {dispose} closure (Scope, T3).
    crate::owner_stores::register(
        "FRAME",
        Box::new(|owner| {
            let _ = FRAME.with(|f| f.borrow_mut().remove_by_owner(owner));
            refresh_detour();
        }),
        Box::new(|_ids| {}),
        Box::new(|| {
            FRAME.with(|f| *f.borrow_mut() = Descriptor::new("OnGameFrame"));
        }),
    );

    // EVENT_MUX + EVENT_MUX_PRE: registered by the feature module, which owns both muxes and
    // the asymmetric teardown they need.
    crate::events::register_stores();

    // SDKHOOKS: per-entity OnTakeDamage (and later types). The DispatchTraceAttack detour stays
    // installed for the process lifetime — no follow-up.
    crate::sdkhooks::register_stores();

    // CHAT_MSG_SUBS + CLIENT_CMD_SUBS + CONCOMMANDS: registered by the feature module.
    crate::commands::register_stores();

    // CLIENT_MUX: registered by the feature module, which owns the mux the callbacks close over.
    crate::client::register_store();
    crate::surface_leases::register_store();
    crate::shared_entity_switch::register_store();

    // MAP_MUX: the StartupServer hook stays installed for the process lifetime — no follow-up.
    crate::owner_stores::register(
        "MAP_MUX",
        Box::new(|owner| { MAP_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { MAP_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            MAP_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );

    // PRECACHE_MUX: the OnPrecacheResource hook stays installed for the process lifetime — no follow-up.
    crate::owner_stores::register(
        "PRECACHE_MUX",
        Box::new(|owner| { PRECACHE_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { PRECACHE_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            PRECACHE_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );

    // COOKIE_CACHED_MUX: pure post-frame JS dispatch — no engine hook to remove.
    crate::cookies::register_store();

    crate::ws::register_store();

    crate::net::register_store();

    // OUTPUT_MUX: the FireOutputInternal detour stays installed for the process lifetime — no follow-up.
    crate::owner_stores::register(
        "OUTPUT_MUX",
        Box::new(|owner| { OUTPUT_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { OUTPUT_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            OUTPUT_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );

    // CVAR_MUX: the global change callback stays installed for the process lifetime — no follow-up.
    crate::owner_stores::register(
        "CVAR_MUX",
        Box::new(|owner| { CVAR_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { CVAR_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            CVAR_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );

    // ENTITY_MUX: registered by the feature module, which owns the mux the callbacks close over.
    crate::entity::register_store();

    // USERCMD_MUX: the input-processing detour stays installed for the process lifetime — no follow-up.
    crate::owner_stores::register(
        "USERCMD_MUX",
        Box::new(|owner| { USERCMD_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { USERCMD_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            USERCMD_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );

    // HOOK_MUX: a SUBSCRIBER's rows. There is no engine-op follow-up on the emptied channels — a
    // declarative hook's detour is never uninstalled while the process runs (spec §6: removing a
    // live detour races the engine calling through it), so an emptied channel simply dispatches to
    // nobody. The DESCRIPTOR side of teardown is `gamedata_hooks::drop_owner`, called from
    // `unload_plugin` beside `gamedata_calls::drop_plugin`.
    crate::owner_stores::register(
        "HOOK_MUX",
        Box::new(|owner| { HOOK_MUX.with(|m| { m.borrow_mut().remove_by_owner(owner); }); }),
        Box::new(|ids| { HOOK_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            HOOK_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
            ACTIVE_HOOK.with(|a| *a.borrow_mut() = None);
            // The descriptor + SLOT tables go with it: the shim's `S2_HookResetAll()` forgets its
            // half of the install bookkeeping at Unload, and a core that kept `installed` set would
            // make the next core's first subscribe skip the patch — every hook silently dead.
            crate::gamedata_hooks::reset_all();
        }),
    );

    // USERMSG_MUX: emptied canonical names → clear the shim bitmap bit via usermsg_hook_unsub.
    // Registered by the feature module, which owns the mux the callbacks close over.
    crate::usermsg::register_store();

    // transmit (checktransmit): drop the plugin's visibility rules + re-push each affected index.
    // Not a scope surface (ids no-op).
    crate::owner_stores::register(
        "TRANSMIT",
        Box::new(|owner| { transmit_remove_owner(owner); }),
        Box::new(|_ids| {}),
        Box::new(|| {
            TRANSMIT_RULES.with(|r| r.borrow_mut().clear());
        }),
    );

    // voice hearability: drop the plugin's rules + re-push each affected sender, so a departed
    // plugin can never leave players silenced. Not a scope surface (ids no-op).
    crate::owner_stores::register(
        "VOICE",
        Box::new(|owner| { voice_remove_owner(owner); }),
        Box::new(|_ids| {}),
        Box::new(|| {
            VOICE_RULES.with(|r| r.borrow_mut().clear());
        }),
    );

    // CONFIG_SUBS: drop config-change subs + stop watching the file. The scope path drops the subs
    // only (the file watch is plugin-lifetime, not scope-lifetime).
    crate::owner_stores::register(
        "CONFIG_SUBS",
        Box::new(|owner| {
            CONFIG_SUBS.with(|m| m.borrow_mut().remove_by_owner(owner));
            crate::loader::unwatch_config_for(owner);
        }),
        Box::new(|ids| { CONFIG_SUBS.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            CONFIG_SUBS.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );

    // TOPMENU_ITEMS: drop the plugin's registered items (categories persist once created — SM parity).
    // Not scope-able (ids no-op).
    crate::owner_stores::register(
        "TOPMENU_ITEMS",
        Box::new(|owner| { TOPMENU_ITEMS.with(|m| m.borrow_mut().retain(|_, it| it.owner != owner)); }),
        Box::new(|_ids| {}),
        Box::new(|| {
            TOPMENU_ITEMS.with(|m| m.borrow_mut().clear());
        }),
    );

    // UI_POOL_CLAIMS: free the departing plugin's pooled HUD panel slots — registered by the
    // feature module, which owns the claim table.
    crate::ui_pool::register_store();
}


/// Register every PROCESS-scoped singleton into the `process_singletons` registry, so `shutdown()`
/// sweeps it instead of a hand-maintained cascade. Called from `init()` alongside
/// `register_builtin_stores` (and, like it, re-runs its own `reset()` so a Metamod re-init is
/// idempotent). Registration order == the historical cascade order, which `reset_all` preserves.
///
/// The phase argument is the ordering rule that used to live only in repeated prose. Anything
/// holding a `v8::Global` is `BeforeIsolateDrop`; pure-Rust state is `AfterIsolateDrop`.
///
/// This registry is deliberately DISJOINT from `owner_stores`: the host-global caches here
/// (`ADMIN_*`, `BAN_*`, `SCHEMA_OFFSETS`, cookies) are designed to survive any single plugin's
/// unload, so registering them owner-scoped would wipe shared admin/ban state on an unrelated
/// plugin's reload.
pub(crate) fn register_process_singletons() {
    use crate::process_singletons::ResetPhase::{AfterIsolateDrop, BeforeIsolateDrop};
    crate::process_singletons::reset();
    fn reg(
        name: &'static str,
        phase: crate::process_singletons::ResetPhase,
        f: impl Fn() + 'static,
    ) {
        crate::process_singletons::register(name, phase, Box::new(f));
    }

    // ---- BeforeIsolateDrop: holds V8 handles, or must be torn down while the isolate lives. ----

    // Async state: RESOLVERS holds Globals into the isolate, so the handles must be released here.
    reg("TIMERS", BeforeIsolateDrop, || {
        TIMERS.with(|t| *t.borrow_mut() = TimerQueue::new())
    });
    reg("RESOLVERS", BeforeIsolateDrop, crate::jobs::reset_resolvers);
    reg("TIMER_CBS", BeforeIsolateDrop, || {
        TIMER_CBS.with(|m| m.borrow_mut().clear())
    });
    reg("ASYNC_STAGING", BeforeIsolateDrop, || {
        TIMER_LEASES.with(|m| m.borrow_mut().clear());
        DUE_TIMERS.with(|q| q.borrow_mut().clear());
        PARKED_HTTP.with(|v| v.borrow_mut().take());
        PARKED_DB.with(|v| v.borrow_mut().take());
        MICROTASK_DRAIN_NEEDED.with(|v| v.set(false));
    });
    reg("TIMER_KILLED", BeforeIsolateDrop, || {
        TIMER_KILLED.with(|k| k.borrow_mut().clear())
    });
    // PENDING_REJECTS holds only Strings, so drop order vs the isolate is not load-bearing; kept in
    // this phase to preserve the historical position.
    reg("PENDING_REJECTS", BeforeIsolateDrop, || {
        PENDING_REJECTS.with(|m| m.borrow_mut().clear())
    });
    // TopMenu categories/seq/pending — the ITEMS map is an owner-scoped store, these are not
    // (categories outlive the registering plugin, SM parity).
    reg("TOPMENU_CATEGORIES", BeforeIsolateDrop, || {
        TOPMENU_CATEGORIES.with(|c| c.borrow_mut().clear())
    });
    reg("TOPMENU_SEQ", BeforeIsolateDrop, || {
        TOPMENU_SEQ.with(|c| c.set(0))
    });
    reg("TOPMENU_PENDING", BeforeIsolateDrop, || {
        TOPMENU_PENDING.with(|q| q.borrow_mut().clear())
    });
    interop_lifetime::register_resets();
    // Inter-plugin method + subscriber Globals.
    reg("IFACE_METHODS", BeforeIsolateDrop, || {
        IFACE_METHODS.with(|m| m.borrow_mut().clear())
    });
    reg("IFACE_SUBS", BeforeIsolateDrop, || {
        IFACE_SUBS.with(|m| m.borrow_mut().clear())
    });
    reg("IFACES", BeforeIsolateDrop, || {
        IFACES.with(|r| r.borrow_mut().clear())
    });
    // The publishes registries: per-plugin unload clears these per id, but a plugin that was `set`
    // and never loaded leaves an entry no unload ever walks. This is the teardown backstop.
    reg("PLUGIN_INTEROP", BeforeIsolateDrop, || {
        PLUGIN_INTEROP.with(|p| p.borrow_mut().clear())
    });
    reg("PLUGIN_PUBLISHES", BeforeIsolateDrop, || {
        PLUGIN_PUBLISHES.with(|p| p.borrow_mut().clear())
    });
    reg("UNDECLARED_PUBLISHES", BeforeIsolateDrop, || {
        UNDECLARED_PUBLISHES.with(|p| p.borrow_mut().clear())
    });
    reg("NEXT_SUB_ID", BeforeIsolateDrop, || {
        NEXT_SUB_ID.with(|c| c.set(1))
    });
    // Per-plugin contexts: each Global<Context> points into the isolate.
    reg("PLUGINS", BeforeIsolateDrop, || {
        PLUGINS.with(|p| p.borrow_mut().clear())
    });
    reg("REGISTRY", BeforeIsolateDrop, || {
        REGISTRY.with(|r| *r.borrow_mut() = plugin::Registry::new())
    });
    reg(
        "PENDING_JOBS",
        BeforeIsolateDrop,
        crate::jobs::reset_pending,
    );
    reg("DETOUR_INSTALLED", BeforeIsolateDrop, || {
        DETOUR_INSTALLED.with(|c| c.set(false))
    });

    // ---- AfterIsolateDrop: pure Rust, no V8 handles. ----

    reg("FRAME_COUNTER", AfterIsolateDrop, || {
        FRAME_COUNTER.with(|c| c.set(0))
    });
    // Pending queues drained by the muxes' post-frame dispatch — sidecars, not subscriber stores.
    crate::client::register_singletons();
    crate::cookies::register_singletons();
    crate::ws::register_singletons();
    crate::net::register_singletons();
    // usermsg name→id resolution caches (the MUX itself is an owner-scoped store). Registered by the
    // feature module — same phase, same position in the order.
    crate::usermsg::register_singletons();
    reg("PENDING_HANDOFF", AfterIsolateDrop, || {
        PENDING_HANDOFF.with(|h| h.borrow_mut().clear())
    });
    // L1 lifecycle-v2 load state.
    reg("LOADING", AfterIsolateDrop, || {
        LOADING.with(|l| l.borrow_mut().clear())
    });
    reg("FAILED_PLUGINS", AfterIsolateDrop, || {
        FAILED_PLUGINS.with(|f| f.borrow_mut().clear())
    });
    reg("MANIFEST_VERSIONS", AfterIsolateDrop, || {
        MANIFEST_VERSIONS.with(|m| m.borrow_mut().clear())
    });
    // A `-1` cached before the schema was live must not persist across an init cycle.
    reg("SCHEMA_OFFSETS", AfterIsolateDrop, || {
        SCHEMA_OFFSETS.with(|c| *c.borrow_mut() = crate::schema::OffsetCache::new())
    });
    // Host-global caches — deliberately NOT owner-scoped (see this fn's doc comment). Each feature
    // module registers its OWN slots, which is how the admin gap got closed: seven admin statics
    // existed here and only three were ever registered.
    crate::admin::register_singletons();
    crate::bans::register_singletons();
    crate::events::register_singletons();
    reg("CRASH_BREADCRUMB", AfterIsolateDrop, || {
        crate::crash::breadcrumb::clear_plugins()
    });
}



#[cfg(test)]
/// The in-isolate test harness. `pub(crate)` so a FEATURE module's own `#[cfg(test)]
#[path = "v8host/tests.rs"]
/// The in-isolate test harness. `pub(crate)` so feature-module tests can reuse it while
/// preserving the established `v8host::frame_tests::*` module and test names.
pub(crate) mod frame_tests;
