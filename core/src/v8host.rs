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
    /// The plugin's REGISTRY-assigned generation (set together with the REGISTRY entry at
    /// `create_plugin_context`).  Read when a native creates an async resolver so the resolver is
    /// tagged with `(id, generation)`; `frame_async_drain` later checks `REGISTRY.is_live` against
    /// this to DROP a continuation whose plugin unloaded or reloaded.
    generation: u64,
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
    /// Boot instant for the breadcrumb's uptime field (set once in `init`).
    static UPTIME_START: std::cell::Cell<Option<Instant>> = std::cell::Cell::new(None);
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
    /// Callback timers (`Timers.after`/`Timers.every`) — distinct from RESOLVERS, which holds
    /// one-shot Promise resolvers. A callback timer's function must SURVIVE firing when it repeats,
    /// so it cannot live in a map the drain removes from unconditionally. Keyed by the same
    /// async-id space as TIMERS/RESOLVERS so ledger teardown reaches all three by one id.
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
    /// Damage pre-hook multiplexer (Slice 6.6): `Damage.onPre(h)` subscribers, keyed by the constant
    /// "onPre" (damage has no name dimension). Same EventMux shape/discipline; handlers read/modify the
    /// current CTakeDamageInfo in place. remove_by_owner on unload; reset on shutdown.
    // EVENT_MUX / EVENT_MUX_PRE / EVENT_RECIPIENTS moved to `crate::events`.
    static DAMAGE_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
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
    /// Unlike `DAMAGE_MUX`/`CHAT_MSG_SUBS` (whose detour is installed once, unconditionally, for the
    /// process lifetime), the `FireOutputInternal` detour here is likewise installed unconditionally at
    /// shim Load — so there is no per-subscribe engine-op and no engine-op on empty teardown. Dispatch is
    /// SYNCHRONOUS (the detour blocks on it, mirrors `DAMAGE_MUX`/`EVENT_MUX_PRE`, NOT the post-drain
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
    /// (usercmd has no name dimension, like `DAMAGE_MUX`'s "onPre"). Dispatch is SYNCHRONOUS (the
    /// Task-3 per-tick input-processing detour blocks on it, mirrors `DAMAGE_MUX`/`OUTPUT_MUX`) so a handler's
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

    /// TopMenu registry (adminmenu framework). Ordered category names (deduped) + items owned by a
    /// plugin. Item `onSelect` is a Global<Function> held like a command handler (NOT marshalled;
    /// invoked in the owner's context on select). Owner-scoped teardown mirrors CONCOMMANDS.
    static TOPMENU_CATEGORIES: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
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

/// A registered TopMenu item. `on_select` is invoked in `owner`'s context (liveness-gated by `generation`).
/// `seq` is a monotonic insertion index — `snapshot` sorts by it for stable, registration-order rendering.
struct TopMenuItem {
    category: String,
    name: String,
    flags: i64,
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
    let generation = PLUGINS
        .with(|p| p.borrow().get(&owner).map(|pi| pi.generation))
        .unwrap_or(0);
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

/// Allocate the next async id (timers + Task-5 jobs share this space).
/// Monotonic async-id allocator (1-based; 0 is reserved as "none").
///
/// PROCESS-GLOBAL, not thread_local — and that is the whole point. These ids key `RESOLVERS`, but
/// they are ALSO handed to engines that live for the process: the threadpool, http/fetch, and the
/// ws/net registries. A per-thread counter feeding process-wide registries means two threads mint
/// the SAME id for unrelated work.
///
/// That is not theoretical. libtest runs every `#[test]` on its own thread, so a thread_local counter
/// restarted at 1 for each test while the threadpool's completion channel carried on across all of
/// them. A completion from an earlier test arriving during a later one found the later test's
/// resolver under the same id, removed it, and resolved it with `undefined` — so
/// `WebSocket.connect(...).then(id => ...)` handed JS `undefined`, the socket wrapper was built with
/// `id = 0`, and every subsequent native refused it as "does not own ws conn 0". Silent, and only
/// under load, because it needs a stale completion to land inside another test's poll window.
///
/// The comment at the threadpool completion loop ("a stale id from a prior isolate has no entry and
/// skips this") states the intended rule; a resettable counter is what made it false.
static NEXT_ASYNC_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_async_id() -> u64 {
    NEXT_ASYNC_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

/// Shared helper for payload-carrying job natives (`__s2_fetch`, and any later cousin): create a
/// `PromiseResolver`, tag it with the calling plugin, ledger it as a `Job`, stash it under a fresh
/// async id, bump `PENDING_JOBS`, reconcile the detour, and return `(id, promise)`.
///
/// The caller submits work keyed by `id`; `frame_async_drain` later pops the resolver. RESOLVERS
/// stay here — modules must not touch the map. A non-plugin / unknown owner is a safe no-op.
pub(crate) fn begin_job_promise<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> (u64, v8::Local<'s, v8::Value>) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let id = next_async_id();
    let owner = resolver_owner_tag(scope);
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
    refresh_detour();
    (id, promise.into())
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

/// Native `__s2_timer_create(ms, fn, repeat) -> id`. A CALLBACK timer (SourceMod `CreateTimer`),
/// as opposed to `__s2_delay`'s one-shot Promise. Returns 0 when the arguments are unusable, so JS
/// can report failure instead of handing back a handle that will never fire.
fn s2_timer_create(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        if args.length() < 2 { return; }
        let ms = args.get(0).integer_value(scope).unwrap_or(-1);
        if ms < 0 { return; }
        let ms = ms as u64;
        let Ok(f) = v8::Local::<v8::Function>::try_from(args.get(1)) else { return };
        let repeat = args.get(2).boolean_value(scope);
        // A zero-interval REPEATING timer would re-arm itself every drain forever with no way for
        // the frame to make progress on anything else. Refuse it rather than ship a footgun.
        if repeat && ms == 0 { return; }

        let id = next_async_id();
        let owner = resolver_owner_tag(scope);
        if let Some((ref oid, _)) = owner {
            REGISTRY.with(|r| {
                if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_timer(id); }
            });
        }
        TIMER_CBS.with(|m| m.borrow_mut().insert(id, TimerCallback {
            owner,
            cb: v8::Global::new(scope.as_ref(), f),
            interval_ms: if repeat { Some(ms) } else { None },
        }));
        TIMERS.with(|t| t.borrow_mut().push(id, TimerKind::Deadline(Instant::now() + Duration::from_millis(ms))));
        refresh_detour();
        rv.set_double(id as f64);
    }));
}

/// Native `__s2_timer_kill(id) -> bool`. Idempotent: killing an already-dead or never-existing
/// timer is `false`, not an error. Removes from BOTH the queue and the callback map — leaving the
/// callback behind would keep a Global<Function> alive for the isolate's lifetime.
fn s2_timer_kill(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = args.get(0).integer_value(scope).unwrap_or(0) as u64;
        let had_cb = TIMER_CBS.with(|m| m.borrow_mut().remove(&id)).is_some();
        let had_q = TIMERS.with(|t| t.borrow_mut().remove(id));
        // Record ONLY the self-kill case, so this set stays bounded. If the timer was still in
        // TIMER_CBS or the queue we removed it above and the drain will never see it; the only way
        // both are absent for a live id is that the drain is holding it mid-fire — i.e. the
        // callback is killing itself. (An id that never existed also lands here; the drain removes
        // whatever it looks up, and shutdown clears the rest.)
        if !had_cb && !had_q { TIMER_KILLED.with(|k| { k.borrow_mut().insert(id); }); }
        rv.set_bool(had_cb || had_q);
    }));
}

/// Native `__s2_timer_alive(id) -> bool`.
fn s2_timer_alive(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = args.get(0).integer_value(scope).unwrap_or(0) as u64;
        // Killed-mid-fire counts as dead even though the drain is holding the entry.
        if TIMER_KILLED.with(|k| k.borrow().contains(&id)) { rv.set_bool(false); return; }
        rv.set_bool(TIMER_CBS.with(|m| m.borrow().contains_key(&id)));
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
fn s2_ws_connect(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let url = args.get(0).to_rust_string_lossy(scope);
        // Optional `{ headers }` init, read exactly like s2_fetch's. A header the
        // handshake owns is refused in ws::build_request, not here — the rejection
        // has to reach JS through ConnectFailed like every other connect failure,
        // and this native has no promise to reject with yet.
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Ok(opts) = v8::Local::<v8::Object>::try_from(args.get(1)) {
            if let Some(k) = v8::String::new(scope, "headers") {
                if let Some(hv) = opts.get(scope, k.into()) {
                    if let Ok(ho) = v8::Local::<v8::Object>::try_from(hv) {
                        if let Some(names) = ho.get_own_property_names(scope, Default::default()) {
                            for i in 0..names.length() {
                                let Some(key) = names.get_index(scope, i) else { continue };
                                let Some(val) = ho.get(scope, key) else { continue };
                                headers.push((
                                    key.to_rust_string_lossy(scope),
                                    val.to_rust_string_lossy(scope),
                                ));
                            }
                        }
                    }
                }
            }
        }
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let id = next_async_id();
        // Tag the resolver with the CALLING plugin's (id, current generation) — the async-liveness guard.
        let owner = resolver_owner_tag(scope);
        // The SAME derivation send/close/on use to CHECK this owner — see `ws_owner`.
        let owner_string = ws_owner(scope);
        // Ledger this async job (as a Job, for RESOLVERS/PENDING_JOBS cleanup) AND the connection
        // itself (as a WsConn, so an unclosed connection is closed at teardown) against the CALLING
        // plugin — a non-plugin/unknown owner is a safe no-op; no borrow held across a JS call.
        if let Some((ref oid, _)) = owner {
            REGISTRY.with(|r| {
                if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                    l.record_job(id);
                    l.record_ws_conn(id);
                }
            });
        }
        RESOLVERS.with(|m| {
            m.borrow_mut()
                .insert(id, ResolverEntry { owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
        });
        PENDING_JOBS.with(|c| c.set(c.get() + 1));
        crate::ws::connect(id, url, owner_string, headers);
        refresh_detour();
        rv.set(promise.into());
    }));
}

/// Resolve (or drop, on the async-liveness guard) a completed `__s2_ws_connect` job in its OWNING
/// plugin's context — MIRRORS `resolve_fetch`'s owner-liveness + context-clone +
/// HandleScope/ContextScope preamble exactly, but resolves with the conn-id `Number` on `Ok`
/// (the plugin's `WebSocket.connect` prelude then wraps it into a handle), or rejects with an
/// `Error` on `Err` (a connect failure — bad host/port/handshake).
fn resolve_ws_connect(host: &mut Host, entry: &ResolverEntry, id: u64, result: Result<(), String>) {
    let g_ctx = match &entry.owner {
        Some((oid, generation)) => {
            if !REGISTRY.with(|r| r.borrow().is_live(oid, *generation)) {
                return; // plugin unloaded or reloaded → DROP (do not resolve into a dead context)
            }
            match PLUGINS.with(|p| p.borrow().get(oid).map(|pi| pi.context.clone())) {
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

    match result {
        Ok(()) => {
            let id_val = v8::Number::new(scope, id as f64);
            resolver.resolve(scope, id_val.into());
        }
        Err(e) => {
            let msg = v8::String::new(scope, &e).unwrap_or_else(|| v8::String::new(scope, "ws connect error").unwrap());
            let ex = v8::Exception::error(scope, msg);
            resolver.reject(scope, ex);
        }
    }
}

/// Native `__s2_ws_send(id, text)`.  Owner-scoped (a no-op for a conn this plugin doesn't own, or an
/// absent conn); hands off to `crate::ws::send` (a non-blocking unbounded-channel send — never
/// blocks the calling thread). No return value.
/// The owner string every ws native uses — connect to REGISTER it, send/close/on to CHECK it.
///
/// One function because four copies of `current_plugin(scope).unwrap_or_*()` are four chances to
/// disagree, and they did: `__s2_ws_on` fell back to `"legacy"` where the others fell back to `""`,
/// so whenever `current_plugin` could not name a plugin the socket went half-mute — `send` matched
/// the registered owner and reached the wire, `is_owner` did not and dropped the subscription on the
/// floor. A test can assert these agree; only a single definition makes disagreeing impossible.
fn ws_owner(scope: &mut v8::PinScope) -> String {
    current_plugin(scope).unwrap_or_default()
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
fn s2_net_tcp_connect(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let host = args.get(0).to_rust_string_lossy(scope);
        let port = args.get(1).number_value(scope).unwrap_or(0.0) as u16;
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let id = next_async_id();
        let owner = resolver_owner_tag(scope);
        let owner_string = current_plugin(scope).unwrap_or_default();
        if let Some((ref oid, _)) = owner {
            REGISTRY.with(|r| {
                if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                    l.record_job(id);
                    l.record_net_conn(id);
                }
            });
        }
        RESOLVERS.with(|m| {
            m.borrow_mut()
                .insert(id, ResolverEntry { owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
        });
        PENDING_JOBS.with(|c| c.set(c.get() + 1));
        crate::net::connect_tcp(id, host, port, owner_string);
        refresh_detour();
        rv.set(promise.into());
    }));
}

/// Native `__s2_net_udp_bind() -> Promise<connId>`. Same block as `s2_net_tcp_connect`, hand-off
/// `crate::net::bind_udp` (a UDP socket bound to an ephemeral local port; the Promise resolves once
/// the socket is bound, or rejects on a bind failure).
fn s2_net_udp_bind(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let id = next_async_id();
        let owner = resolver_owner_tag(scope);
        let owner_string = current_plugin(scope).unwrap_or_default();
        if let Some((ref oid, _)) = owner {
            REGISTRY.with(|r| {
                if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                    l.record_job(id);
                    l.record_net_conn(id);
                }
            });
        }
        RESOLVERS.with(|m| {
            m.borrow_mut()
                .insert(id, ResolverEntry { owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
        });
        PENDING_JOBS.with(|c| c.set(c.get() + 1));
        crate::net::bind_udp(id, owner_string);
        refresh_detour();
        rv.set(promise.into());
    }));
}

/// Resolve (or drop, on the async-liveness guard) a completed `__s2_net_tcp_connect`/`_udp_bind` job
/// in its OWNING plugin's context — a verbatim copy of `resolve_ws_connect` (resolves with the
/// conn-id `Number` on `Ok`, rejects with an `Error` on `Err` = a connect/bind failure; the
/// owner-liveness DROP preamble is identical — never resolve into a dead/replaced context).
fn resolve_net_connect(host: &mut Host, entry: &ResolverEntry, id: u64, result: Result<(), String>) {
    let g_ctx = match &entry.owner {
        Some((oid, generation)) => {
            if !REGISTRY.with(|r| r.borrow().is_live(oid, *generation)) {
                return; // plugin unloaded or reloaded → DROP (do not resolve into a dead context)
            }
            match PLUGINS.with(|p| p.borrow().get(oid).map(|pi| pi.context.clone())) {
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

    match result {
        Ok(()) => {
            let id_val = v8::Number::new(scope, id as f64);
            resolver.resolve(scope, id_val.into());
        }
        Err(e) => {
            let msg = v8::String::new(scope, &e).unwrap_or_else(|| v8::String::new(scope, "net connect error").unwrap());
            let ex = v8::Exception::error(scope, msg);
            resolver.reject(scope, ex);
        }
    }
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
/// ORDER IS LOAD-BEARING: `@s2script/sdk/` is stripped BEFORE the shorter `@s2script/`, which also
/// matches `@s2script/sdk/<cap>` and would strip to the garbage cap `sdk/<cap>`. Non-`@s2script/`
/// specifiers → `null` (the JS `__s2_require` shim resolves those as inter-plugin deps).  A
/// retired/unknown name (global undefined) → `null`. Engine-generic: no module list hardcoded;
/// `@s2script/cs2` maps to `__s2pkg_cs2` via the plain `@s2script/` strip.
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
        // which never exists → null (the flat barrel is rejected; the typecheck gate, not
        // s2require, enforces the namespace split). Still generic — no module list hardcoded;
        // `@s2script/cs2` keeps riding the plain `@s2script/` strip.
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
    IFACES.with(|r| r.borrow_mut().set_imports(id, decls));
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

/// Drop a plugin's publishes map (teardown).
pub fn clear_plugin_publishes(plugin_id: &str) {
    PLUGIN_PUBLISHES.with(|p| { p.borrow_mut().remove(plugin_id); });
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
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(impl_obj) = v8::Local::<v8::Object>::try_from(args.get(1)) else {
            log_warn(&format!("WARN: iface_publish('{}'): impl is not an object", name));
            return;
        };
        let Some(owner) = current_plugin(scope) else {
            log_warn("WARN: iface_publish: no current plugin");
            return;
        };

        // The manifest is the sole source of the version. An undeclared name never registers.
        let Some(decl) = PLUGIN_PUBLISHES.with(|p| {
            p.borrow().get(&owner).and_then(|m| m.get(&name)).cloned()
        }) else {
            log_warn(&format!(
                "WARN: iface_publish('{}'): plugin '{}' did not declare this interface in its \
                 manifest `publishes` — refusing",
                name, owner
            ));
            // Remember it so `reconcile_publishes` fails this plugin's LOAD. The declared→owned
            // check cannot see this case: a plugin that declares nothing has nothing to reconcile,
            // so without this it would run on with its interface silently unpublished.
            UNDECLARED_PUBLISHES.with(|p| {
                p.borrow_mut().entry(owner.clone()).or_default().push(name.clone());
            });
            return;
        };

        let generation = REGISTRY.with(|r| r.borrow().generation_of(&owner)).unwrap_or(0);

        // Enumerate own function properties → method names + capture Globals.
        let mut method_names: Vec<String> = Vec::new();
        let mut captured: Vec<(String, v8::Global<v8::Function>)> = Vec::new();
        if let Some(prop_names) = impl_obj.get_own_property_names(scope, Default::default()) {
            for i in 0..prop_names.length() {
                let Some(key) = prop_names.get_index(scope, i) else { continue };
                let Some(val) = impl_obj.get(scope, key) else { continue };
                if let Ok(f) = v8::Local::<v8::Function>::try_from(val) {
                    let m = key.to_rust_string_lossy(scope);
                    method_names.push(m.clone());
                    captured.push((m, v8::Global::new(scope.as_ref(), f)));
                }
            }
        }

        // Register FIRST: a REJECTED publish must not leave method Globals behind (a rejected
        // second producer's functions would otherwise shadow the incumbent's in IFACE_METHODS,
        // which is keyed by name).
        if let Err(e) = IFACES.with(|r| {
            r.borrow_mut().publish(&name, &decl.version, &decl.types_sha256, &owner, generation, method_names)
        }) {
            log_warn(&format!("WARN: iface_publish('{}'): {}", name, e));
            return;
        }
        for (m, g) in captured {
            IFACE_METHODS.with(|mm| { mm.borrow_mut().insert((name.clone(), m), g); });
        }
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
            crate::interfaces::CallTarget::TypesMismatch => { throw_named(scope, "InterfaceTypesMismatch", &name); return; }
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





/// `__s2_damage_subscribe(handler)` — subscribe a JS fn to `Damage.onPre` (Slice 6.6). Owner-tracked;
/// the shim detour is installed at Load, so no per-subscribe engine registration is needed.
fn s2_damage_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        // The DispatchTraceAttack detour stays installed for the process lifetime — no follow-up.
        let Some((sub_id, _)) = subscribe_into(scope, &args, &DAMAGE_MUX, "onPre", 0) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_usercmd_subscribe(handler)` — subscribe a JS fn to `UserCmd.onRun` (usercmd primitive Task 2).
/// Owner-tracked, fixed mux key "onRun" (usercmd has no name dimension, like `s2_damage_subscribe`'s
/// "onPre"). On the FIRST-EVER subscribe (the mux was empty), calls the (Task 3) `usercmd_hook_install`
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


/// `__s2_topmenu_add_category(name)` — append a category if absent (order = insertion; deduped).
fn s2_topmenu_add_category(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        TOPMENU_CATEGORIES.with(|c| { let mut b = c.borrow_mut(); if !b.contains(&name) { b.push(name); } });
    }));
}

/// `__s2_topmenu_add_item(category, id, name, flags, onSelectFn)` — register/replace an item owned by
/// current_plugin. Auto-creates the category (order hint). Mirrors s2_concommand's owner+gen+Global store.
fn s2_topmenu_add_item(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 5 { return; }
        let category = args.get(0).to_rust_string_lossy(scope);
        let id = args.get(1).to_rust_string_lossy(scope);
        let name = args.get(2).to_rust_string_lossy(scope);
        let flags = args.get(3).integer_value(scope).unwrap_or(0);
        let func_local = match v8::Local::<v8::Function>::try_from(args.get(4)) { Ok(f) => f, Err(_) => return };
        let on_select = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        TOPMENU_CATEGORIES.with(|c| { let mut b = c.borrow_mut(); if !b.contains(&category) { b.push(category.clone()); } });
        // Reuse the existing seq on a re-add (reload) so positions stay stable; else take the next counter.
        let seq = TOPMENU_ITEMS.with(|m| m.borrow().get(&id).map(|it| it.seq))
            .unwrap_or_else(|| TOPMENU_SEQ.with(|c| { let s = c.get(); c.set(s + 1); s }));
        TOPMENU_ITEMS.with(|m| m.borrow_mut().insert(id, TopMenuItem { category, name, flags, owner, generation, seq, on_select }));
    }));
}

/// `__s2_topmenu_snapshot() -> { categories: string[], items: [{id, category, name, flags}] }` (metadata only).
fn s2_topmenu_snapshot(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cats: Vec<String> = TOPMENU_CATEGORIES.with(|c| c.borrow().clone());
        // Sort by seq → items render in registration order (stable across restarts), not random HashMap order.
        let items: Vec<serde_json::Value> = TOPMENU_ITEMS.with(|m| {
            let b = m.borrow();
            let mut entries: Vec<(&String, &TopMenuItem)> = b.iter().collect();
            entries.sort_by_key(|(_, it)| it.seq);
            entries.into_iter().map(|(id, it)| {
                serde_json::json!({ "id": id, "category": it.category, "name": it.name, "flags": it.flags })
            }).collect()
        });
        let obj = serde_json::json!({ "categories": cats, "items": items });
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
/// registration is needed (mirrors `s2_damage_subscribe`/`s2_chat_on_message`).
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
                "string" => {
                    let s = v.to_rust_string_lossy(scope);
                    // An interior NUL would silently truncate into a DIFFERENT string — no-op instead.
                    let Ok(c) = CString::new(s) else { return };
                    strs_owned.push(c);
                    gp.push((strs_owned.len() - 1) as u64);
                    gp_kind.push(crate::gamedata_calls::GP_STRING);
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













/// Slice 6.6 Stage 2: run the `Damage.onPre` subscribers over the current CTakeDamageInfo (set by the
/// shim detour). Mirrors `dispatch_game_event`: snapshot (release the mux borrow), re-entrancy guard,
/// per-subscriber liveness + context + TryCatch. Each handler gets `new DamageInfo()` (a block-scoped
/// accessor over the current damage) and reads/modifies it in place; blocking = the handler setting
/// damage to 0.
/// Zero the live CTakeDamageInfo damage — the block power behind a `ctx.entities.onDamage` handler
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
    let snap = DAMAGE_MUX.with(|m| m.borrow().snapshot("onPre"));
    let result = fan_out_collapsing(
        &snap,
        "dispatch_damage",
        Instrument::breadcrumb("damage:onPre"),
        StopAt::Stop,
        |tc| {
            // Construct new DamageInfo(): globalThis.__s2pkg_damage.DamageInfo. A failure yields
            // `undefined` rather than skipping the subscriber (pre-fan_out behaviour).
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
    // Block power: a handler returning >= Handled zeroes the live damage. Applied AFTER the collapse
    // rather than mid-loop.
    //
    // This is the B2 fix. The old loop `break`ed at >= Handled, so ONE plugin's Handled silently
    // denied every other plugin's onDamage handler its dispatch for that hit. That contradicts
    // ARCHITECTURE.md:78 — "`Stop` short-circuits. `Handled` does NOT short-circuit (a later
    // observer may still want the event)" — and multiplexer.rs's own `handled_does_not_short_circuit`
    // test. The block is a decision about the DAMAGE, not a veto over other observers; a handler that
    // genuinely wants to end the chain returns `Stop`, which `StopAt::Stop` honours.
    if result >= HookResult::Handled {
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
fn hook_param_read(view: *mut std::ffi::c_void, idx: i32) -> Option<(f64, bool)> {
    let ops = ENGINE_OPS.with(|o| o.get())?;
    if let Some(f) = ops.hook_read_f32 {
        let mut out: f32 = 0.0;
        if f(view, idx, &mut out) == 0 {
            return Some((out as f64, true));
        }
    }
    if let Some(f) = ops.hook_read_i32 {
        let mut out: i32 = 0;
        if f(view, idx, &mut out) == 0 {
            return Some((out as f64, false));
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
            Some((v, _)) => rv.set_double(v),
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
            Some((_, true)) if !value.is_finite() || value.abs() > f32::MAX as f64 => None,
            Some((_, false)) if !(value >= i32::MIN as f64 && value <= i32::MAX as f64) => None,
            Some((_, true)) => ops.and_then(|o| o.hook_write_f32).map(|f| f(view, idx, value as f32)),
            Some((_, false)) => ops.and_then(|o| o.hook_write_i32).map(|f| f(view, idx, value as i32)),
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
        let result = hook_param_read(s.view, 1).map(|(v, _)| v as i32).unwrap_or(0);
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
    set_native(scope, global_obj, "__s2_timer_create", s2_timer_create);
    set_native(scope, global_obj, "__s2_timer_kill", s2_timer_kill);
    set_native(scope, global_obj, "__s2_timer_alive", s2_timer_alive);
    set_native(scope, global_obj, "__s2_next_tick", s2_next_tick);
    set_native(scope, global_obj, "__s2_next_frame", s2_next_frame);
    set_native(scope, global_obj, "__s2_thread_sleep", s2_thread_sleep);
    // Schema + entity system.
    set_native(scope, global_obj, "__s2_schema_offset", s2_schema_offset);
    // Perf instrumentation: monotonic ns clock + isolate heap size + force-GC (dev/benchmark).
    set_native(scope, global_obj, "__s2_v8_heap_used", s2_v8_heap_used);
    set_native(scope, global_obj, "__s2_v8_gc", s2_v8_gc);
    set_native(scope, global_obj, "__s2_hrtime_ns", s2_hrtime_ns);
    // Slice 5A: (index, serial) entity natives — serial-gated read/write/valid/decode.
    // The five Slice-3 raw-pointer natives (entity-by-index, deref-handle, ent-read/write-i32,
    // ent-state-changed) were retired in Task 4; callers now use the __s2_ent_ref_* path.
    // __s2_ent_ref_read_i32/__s2_ent_ref_write_i32 (introduced in Slice 5A) were retired in 5B.2 → generic __s2_ent_ref_read/write.
    crate::entity::install_natives(scope, global_obj);
    set_native(scope, global_obj, "__s2_handle_decode", s2_handle_decode);
    set_native(scope, global_obj, "__s2_handle_adopt", s2_handle_adopt);
    crate::commands::install_natives(scope, global_obj);
    // Schema dump (5B.1): drives the shim's schema_enumerate op into a Catalog and writes JSON.
    set_native(scope, global_obj, "__s2_schema_dump", s2_schema_dump);
    // Per-context identity probe + the CJS require shim.
    set_native(scope, global_obj, "__s2_current_plugin", s2_current_plugin);
    // L1 lifecycle v2: awaited-factory settle + reload-handoff consume (design spec §5).
    set_native(scope, global_obj, "__s2_load_settled", s2_load_settled);
    set_native(scope, global_obj, "__s2_load_failed", s2_load_failed);
    set_native(scope, global_obj, "__s2_handoff_take", s2_handoff_take);
    set_native(scope, global_obj, "__s2_scope_dispose", s2_scope_dispose);
    set_native(scope, global_obj, "__s2require", s2require);
    set_native(scope, global_obj, "__s2_crash_set_game", s2_crash_set_game);
    set_native(scope, global_obj, "__s2_server_build", s2_server_build);
    set_native(scope, global_obj, "__s2_crash_test", s2_crash_test);
    // Deferred-dispatch selftest — INSTALLED ONLY when S2_DEFER_SELFTEST is set in the process
    // environment. Gating the INSTALL (not the call) is the point: in a production process the
    // property does not exist on any global, so `typeof __s2_defer_selftest === "undefined"` and
    // there is nothing for a plugin to reach. See `s2_defer_selftest` for why the path it exercises
    // has no natural trigger before A5b.
    if defer_selftest_armed() {
        set_native(scope, global_obj, "__s2_defer_selftest", s2_defer_selftest);
    }
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
    crate::events::install_natives(scope, global_obj);
    // Engine-identity client-list natives (Slice 5D.2).
    crate::client::install_natives(scope, global_obj);
    // Translations slice: root/language phrase-file read + the client's cl_language cvar.
    set_native(scope, global_obj, "__s2_translations_read", s2_translations_read);
    // Event write/fire (Slice 5D.3): pre-subscribe/unsubscribe + setters + create/fire.
    // Config live-reload (Slice 5E.2): register an onChange handler for this plugin's config file.
    set_native(scope, global_obj, "__s2_config_on_change", s2_config_on_change);
    // Client-lifecycle subscriber (Clients sub-project): register a Clients.on* handler.
    // Map-start subscriber (clientlist-fakeconvar-onmapstart slice): register a Server.onMapStart handler.
    set_native(scope, global_obj, "__s2_map_start_subscribe", s2_map_start_subscribe);
    // Precache subscriber (Sound slice): register a Sound.onPrecache handler.
    set_native(scope, global_obj, "__s2_precache_subscribe", s2_precache_subscribe);

    crate::admin::install_natives(scope, global_obj);
    // Slice 6.18: ban cache natives (engine-generic — a SteamID/ban map, like the admin cache).
    crate::bans::install_natives(scope, global_obj);
    // clientprefs: cookie cache natives (engine-generic — a SteamID/string-KV map, like admin/ban).
    crate::cookies::install_natives(scope, global_obj);
    // Voice-control slice: per-slot voice mute set/get (shim-side flag consulted by the
    // SetClientListening rewrite hook; JS never sits in that hot path).
    set_native(scope, global_obj, "__s2_voice_set_muted", s2_voice_set_muted);
    set_native(scope, global_obj, "__s2_voice_get_muted", s2_voice_get_muted);
    // ban-reason sub-project 2: developer-console print + client IP address.
    set_native(scope, global_obj, "__s2_damage_subscribe", s2_damage_subscribe);
    set_native(scope, global_obj, "__s2_damage_read_float", s2_damage_read_float);
    set_native(scope, global_obj, "__s2_damage_read_int", s2_damage_read_int);
    set_native(scope, global_obj, "__s2_damage_write_float", s2_damage_write_float);
    set_native(scope, global_obj, "__s2_damage_victim", s2_damage_victim);
    set_native(scope, global_obj, "__s2_cvar_get", s2_cvar_get);
    set_native(scope, global_obj, "__s2_cvar_set", s2_cvar_set);
    set_native(scope, global_obj, "__s2_convar_register", s2_convar_register);
    // Usercmd primitive Task 2: raw subscribe native (block/read/write natives are Task 3/4).
    set_native(scope, global_obj, "__s2_usercmd_subscribe", s2_usercmd_subscribe);
    // Usercmd primitive Task 3: field read/write + buttons + subtick-clear natives (Task 4 wraps these
    // in the prelude's singleton Cmd accessor object).
    set_native(scope, global_obj, "__s2_usercmd_read", s2_usercmd_read);
    set_native(scope, global_obj, "__s2_usercmd_write", s2_usercmd_write);
    set_native(scope, global_obj, "__s2_usercmd_read_buttons", s2_usercmd_read_buttons);
    set_native(scope, global_obj, "__s2_usercmd_write_buttons", s2_usercmd_write_buttons);
    set_native(scope, global_obj, "__s2_usercmd_clear_subtick", s2_usercmd_clear_subtick);
    set_native(scope, global_obj, "__s2_plugins_list", s2_plugins_list);
    set_native(scope, global_obj, "__s2_plugin_unload", s2_plugin_unload);
    set_native(scope, global_obj, "__s2_plugin_reload", s2_plugin_reload);
    set_native(scope, global_obj, "__s2_plugin_load", s2_plugin_load);
    set_native(scope, global_obj, "__s2_server_command", s2_server_command);
    set_native(scope, global_obj, "__s2_server_map_valid", s2_server_map_valid);
    // reservedslots+basetriggers: server-info natives (max clients / map name / game time).
    set_native(scope, global_obj, "__s2_server_max_clients", s2_server_max_clients);
    set_native(scope, global_obj, "__s2_server_map_name", s2_server_map_name);
    set_native(scope, global_obj, "__s2_server_game_time", s2_server_game_time);
    // Slice 6.2 Task 2: config-bridge natives for the admin module (file load/write).
    set_native(scope, global_obj, "__s2_config_read_raw", s2_config_read_raw);
    set_native(scope, global_obj, "__s2_config_write_raw", s2_config_write_raw);
    // Slice nominations Task 1: raw configs-dir file read/write for @s2script/config.
    set_native(scope, global_obj, "__s2_config_read_file", s2_config_read_file);
    set_native(scope, global_obj, "__s2_config_write_file", s2_config_write_file);
    // Slice DB Task 3: the `__s2_sqlite_*` natives (query/execute now actor-backed, off-thread) for `@s2script/db`.
    set_native(scope, global_obj, "__s2_sqlite_open", s2_sqlite_open);
    set_native(scope, global_obj, "__s2_sqlite_query", s2_sqlite_query);
    set_native(scope, global_obj, "__s2_sqlite_execute", s2_sqlite_execute);
    set_native(scope, global_obj, "__s2_sqlite_close", s2_sqlite_close);
    // Remote SQL driver Task 2: the `__s2_db_remote_*` natives (MySQL/Postgres over sqldb.rs).
    set_native(scope, global_obj, "__s2_db_remote_connect", s2_db_remote_connect);
    set_native(scope, global_obj, "__s2_db_remote_query", s2_db_remote_query);
    set_native(scope, global_obj, "__s2_db_remote_execute", s2_db_remote_execute);
    set_native(scope, global_obj, "__s2_db_remote_close", s2_db_remote_close);
    // Slice HTTP Task 2: async fetch over the process-global tokio+reqwest engine (core/src/http.rs).
    crate::http::install_natives(scope, global_obj);
    // WebSocket Task 2: client ws over the process-global tokio+tungstenite engine (core/src/ws.rs).
    set_native(scope, global_obj, "__s2_ws_connect", s2_ws_connect);
    crate::ws::install_natives(scope, global_obj);
    // Net Task 2: raw TCP/UDP client sockets over the process-global tokio engine (core/src/net.rs).
    set_native(scope, global_obj, "__s2_net_tcp_connect", s2_net_tcp_connect);
    set_native(scope, global_obj, "__s2_net_udp_bind", s2_net_udp_bind);
    crate::net::install_natives(scope, global_obj);
    // TopMenu registry (adminmenu framework): owner-tracked categories/items + post-drain select dispatch.
    set_native(scope, global_obj, "__s2_topmenu_add_category", s2_topmenu_add_category);
    set_native(scope, global_obj, "__s2_topmenu_add_item", s2_topmenu_add_item);
    set_native(scope, global_obj, "__s2_topmenu_snapshot", s2_topmenu_snapshot);
    set_native(scope, global_obj, "__s2_topmenu_select", s2_topmenu_select);
    // Ray-trace slice: the sole native over the trace_shape engine op (engine-generic, no CS2 names).
    set_native(scope, global_obj, "__s2_trace", s2_trace);
    // Entity-creation lifecycle slice: createEntity + EntityRef.spawn/teleport/remove natives.
    set_native(scope, global_obj, "__s2_user_message_create", s2_user_message_create);
    set_native(scope, global_obj, "__s2_user_message_set_int", s2_user_message_set_int);
    set_native(scope, global_obj, "__s2_user_message_set_float", s2_user_message_set_float);
    set_native(scope, global_obj, "__s2_user_message_set_string", s2_user_message_set_string);
    set_native(scope, global_obj, "__s2_user_message_set_bool", s2_user_message_set_bool);
    set_native(scope, global_obj, "__s2_user_message_send", s2_user_message_send);
    set_native(scope, global_obj, "__s2_collision_activate", s2_collision_activate);
    set_native(scope, global_obj, "__s2_sound_emit", s2_sound_emit);
    set_native(scope, global_obj, "__s2_sound_precache_add", s2_sound_precache_add);
    // Item slice: the sub-object vcall native + the readHandleVector native (wrapped as an
    // EntityRef prototype method in the prelude, below). A5b retired give/remove-item to
    // gamedata/cs2 `calls` descriptors.
    // Entity-I/O slice: fire inputs (AddEntityIOEvent) + Entity.onOutput subscribe/unsubscribe
    // (FireOutputInternal detour dispatch — installed at shim Load, see dispatch_output).
    set_native(scope, global_obj, "__s2_output_subscribe", s2_output_subscribe);
    set_native(scope, global_obj, "__s2_cvar_on_change", s2_cvar_on_change);
    set_native(scope, global_obj, "__s2_cvar_off_change", s2_cvar_off_change);
    set_native(scope, global_obj, "__s2_output_unsubscribe", s2_output_unsubscribe);
    // Entity lifecycle listeners slice: Entity.onCreate/onSpawn/onDelete subscribe/unsubscribe (the
    // IEntityListener is lazily installed shim-side on the first subscribe via entity_listener_install).
    // entity_name slice: EntityRef.name reads CEntityIdentity::m_name (sibling of entity_find_by_class's
    // m_designerName read on the same identity).
    // entity_target slice: EntityRef.target reads CBaseEntity::m_target via the schema-offset cache
    // (the field lives on the instance itself, not the identity — see s2_entity_target's doc comment).
    // E1 entity-liveness slice: identity-slot flags (books-gated) for pawn.isValid's staging check.
    // checktransmit slice: declarative per-client entity visibility rules (@s2script/transmit).
    set_native(scope, global_obj, "__s2_transmit_set", s2_transmit_set);
    set_native(scope, global_obj, "__s2_transmit_reset", s2_transmit_reset);
    set_native(scope, global_obj, "__s2_transmit_reset_all", s2_transmit_reset_all);
    set_native(scope, global_obj, "__s2_transmit_stats", s2_transmit_stats);
    // Voice-hearability slice: declarative per-(receiver, sender) rules (@s2script/sdk/voice). The
    // shim evaluates them on the SetClientListening hot path; no JS runs per pair.
    set_native(scope, global_obj, "__s2_voice_audible_set", s2_voice_audible_set);
    set_native(scope, global_obj, "__s2_voice_audible_clear", s2_voice_audible_clear);
    set_native(scope, global_obj, "__s2_voice_reset_all", s2_voice_reset_all);
    set_native(scope, global_obj, "__s2_voice_audible_stats", s2_voice_audible_stats);
    // UserMessage-interception slice: UserMessages.onPre subscribe/unsubscribe + the block-scoped view
    // read natives (route through the usermsg_hook_* ops; the shim's PostEventAbstract hook installs
    // lazily on the first subscribe).
    crate::usermsg::install_natives(scope, global_obj);
    // Plugin-declared engine calls (`@s2script/sdk/unsafe`): ask-by-name only — there is deliberately
    // NO registration native (core registers descriptors itself from the packed gamedata.json).
    set_native(scope, global_obj, "__s2_engine_call_ready", s2_engine_call_ready);
    set_native(scope, global_obj, "__s2_engine_call_receiverless", s2_engine_call_receiverless);
    set_native(scope, global_obj, "__s2_engine_call_status", s2_engine_call_status);
    set_native(scope, global_obj, "__s2_engine_call_invoke", s2_engine_call_invoke);
    // The GAME-PACKAGE-scoped four (A5b): same natives, keyed on core's reserved owner id for the
    // registered game package instead of the calling context's plugin id. The game package's
    // prelude runs in the raw context scope and has no plugin identity of its own, so it cannot use
    // the four above; and an owner id is never taken from JS, so these cannot be aimed elsewhere.
    set_native(scope, global_obj, "__s2_game_call_ready", s2_game_call_ready);
    set_native(scope, global_obj, "__s2_game_call_receiverless", s2_game_call_receiverless);
    set_native(scope, global_obj, "__s2_game_call_status", s2_game_call_status);
    set_native(scope, global_obj, "__s2_game_call_invoke", s2_game_call_invoke);
    // Declarative inbound hooks: `__s2_hook_on` is the game-package subscribe (owner is the first
    // argument, remapped to the reserved owner id). `__s2_engine_hook_*` is the plugin path —
    // owner is the calling context, never an argument — so `Engine.hook` cannot name another plugin.
    // There is no registration native: core registers hook descriptors itself from the packed
    // gamedata, so JS can never declare a detour, only subscribe to a declared one.
    set_native(scope, global_obj, "__s2_hook_on", s2_hook_on);
    set_native(scope, global_obj, "__s2_engine_hook_ready", s2_engine_hook_ready);
    set_native(scope, global_obj, "__s2_engine_hook_status", s2_engine_hook_status);
    set_native(scope, global_obj, "__s2_engine_hook_on", s2_engine_hook_on);
    set_native(scope, global_obj, "__s2_hook_on_post", s2_hook_on_post);
    set_native(scope, global_obj, "__s2_hook_q_u16", s2_hook_q_u16);
    set_native(scope, global_obj, "__s2_hook_self_matches", s2_hook_self_matches);
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

/// The REGISTERING plugin's generation, for owner-tracked stores that live outside this module.
/// `0` when `id` is not a live plugin context (the shared HOST / `"legacy"` path).
pub(crate) fn plugin_generation(id: &str) -> u64 {
    PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.generation)).unwrap_or(0)
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

/// `__s2_load_settled(hooks?)` — the plugin factory settled OK (design spec §5). Stores the returned
/// `PluginHooks` object (if any) on the `PluginInstance` (via `exports`) and marks the load `Settled`;
/// `finalize_loading_plugins` then arms + reconciles + moves it to `Active`. A second call for the same
/// id (state no longer `InFlight`) is ignored.
fn s2_load_settled(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        if args.length() >= 1 {
            if let Ok(obj) = v8::Local::<v8::Object>::try_from(args.get(0)) {
                let g = v8::Global::new(scope.as_ref(), obj);
                PLUGINS.with(|p| {
                    if let Some(pi) = p.borrow_mut().get_mut(&id) {
                        pi.exports = Some(g);
                    }
                });
            }
        }
        LOADING.with(|l| {
            if let Some(e) = l.borrow_mut().get_mut(&id) {
                if matches!(e.state, SettleState::InFlight) {
                    e.state = SettleState::Settled;
                }
            }
        });
    }));
}

/// `__s2_load_failed(message)` — the plugin factory threw or its promise rejected (design spec §5).
/// Marks the load `Failed(message)`; `finalize_loading_plugins` then tears it down (never runs it).
fn s2_load_failed(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        let msg = if args.length() >= 1 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            "factory failed".into()
        };
        LOADING.with(|l| {
            if let Some(e) = l.borrow_mut().get_mut(&id) {
                if matches!(e.state, SettleState::InFlight) {
                    e.state = SettleState::Failed(msg);
                }
            }
        });
    }));
}

/// `__s2_handoff_take() -> unknown` — consume this plugin's reload-handoff blob (if a prior unload
/// captured one via `state()`) and revive it in THIS (new) context via `iface_from_json` (JSON.parse +
/// the EntityRef reviver). Consume-once. Backs `ctx.previous` (the 5E.3 mechanics moved off
/// `onLoad(prev)`). No blob → `undefined`.
fn s2_handoff_take(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        let Some(blob) = PENDING_HANDOFF.with(|h| h.borrow_mut().remove(&id)) else { return };
        if let Some(v) = iface_from_json(scope, &blob) {
            rv.set(v);
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
            // Framework config templates (__s2_TEMPLATES) must exist before the engine prelude, whose
            // admin/db loaders read them to write the operator's file on first boot.
            run_prelude(scope, "config-templates", &config_templates_prelude());
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
        // A fresh load clears any prior FAILED reason for this id (spec §5).
        FAILED_PLUGINS.with(|f| { f.borrow_mut().remove(id); });
        PLUGINS.with(|p| {
            p.borrow_mut().insert(
                id.to_string(),
                PluginInstance {
                    exports: None,
                    context: g_ctx,
                    generation,
                    config_decls: std::collections::HashMap::new(),
                    phase: crate::plugin::Phase::Loading,
                },
            )
        });
        generation
    })
}

/// The current lifecycle phase of `id`, or `None` if unknown (never loaded / disposed). Used by
/// tests and `unload_plugin`'s phase-aware entry.
pub(crate) fn plugin_phase(id: &str) -> Option<crate::plugin::Phase> {
    PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.phase))
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
pub(crate) fn materialize_for_load(id: &str, decls: &std::collections::HashMap<String, crate::config::ConfigEntry>) -> String {
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
pub(crate) fn store_config_decls(id: &str, decls: std::collections::HashMap<String, crate::config::ConfigEntry>) {
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

/// Crash reporter: read an arbitrary configs/<id>.json via the config_read op (the same shim
/// path plugins' configs use). pub(crate) so crash::config can reach it without touching ops.
pub(crate) fn read_engine_config(id: &str) -> Option<String> {
    config_file_content(id)
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
        // `owner` is needed AFTER subscribing, to seed the file watch, so it is resolved here and the
        // helper resolves it again. Only reached once a row was actually stored.
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        if subscribe_into(scope, &args, &CONFIG_SUBS, "config", 0).is_none() { return; }
        crate::loader::watch_config_for(&owner);  // idempotent; seeds baseline if not yet watched
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

/// JS array (params) -> `Vec<DbValue>`. `bool` -> `Int(0|1)`; an integral `number` -> `Int`, else
/// `Real`; `string` -> `Text`; `null`/`undefined` -> `Null`. A non-array `val` (e.g. omitted arg)
/// yields an empty params vec (degrade, not a crash).
fn js_params_to_db(scope: &mut v8::PinScope, val: v8::Local<v8::Value>) -> Vec<crate::db::DbValue> {
    use crate::db::DbValue;
    let mut out = Vec::new();
    if let Ok(arr) = v8::Local::<v8::Array>::try_from(val) {
        for i in 0..arr.length() {
            let Some(el) = arr.get_index(scope, i) else { out.push(DbValue::Null); continue; };
            let dv = if el.is_null_or_undefined() {
                DbValue::Null
            } else if el.is_boolean() {
                DbValue::Int(if el.boolean_value(scope) { 1 } else { 0 })
            } else if el.is_string() {
                DbValue::Text(el.to_rust_string_lossy(scope))
            } else if el.is_number() {
                let n = el.number_value(scope).unwrap_or(0.0);
                // 2^53 — beyond it a JS number can't represent every integer, so keep it a Real
                // (64-bit ids are passed as strings per the contract).
                if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 { DbValue::Int(n as i64) } else { DbValue::Real(n) }
            } else {
                DbValue::Text(el.to_rust_string_lossy(scope))
            };
            out.push(dv);
        }
    }
    out
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
fn s2_sqlite_open(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_default();
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let result = match db_data_dir() {
            Some(dir) => crate::db::open(std::path::Path::new(&dir), &name, &owner),
            None => Err("db not available".to_string()),
        };
        match result {
            Ok(handle) => {
                // Ledger the connection against the CALLING plugin (teardown authority) — a
                // non-plugin/unknown owner (the shared HOST context) is a safe no-op.
                if let Some((ref oid, _)) = resolver_owner_tag(scope) {
                    REGISTRY.with(|r| {
                        if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                            l.record_db_conn(handle);
                        }
                    });
                }
                resolver.resolve(scope, v8::Number::new(scope, handle as f64).into());
            }
            Err(e) => {
                let msg = v8::String::new(scope, &e).unwrap();
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
            }
        }
        rv.set(promise.into());
    }));
}

/// Build the JS `Row[]` (array of {col: value}) from a `QueryResult`. Shared by the sync SQLite
/// path (`s2_sqlite_query`) and the async remote-resolve path (`resolve_db`). Delegates each cell
/// to `db_value_to_v8` (`Int`/`Real` -> `Number`, `Text` -> `String`, `Null` -> `null`).
fn query_result_to_js<'s>(scope: &mut v8::PinScope<'s, '_>, q: &crate::db::QueryResult) -> v8::Local<'s, v8::Value> {
    let arr = v8::Array::new(scope, q.rows.len() as i32);
    for (ri, row) in q.rows.iter().enumerate() {
        let obj = v8::Object::new(scope);
        for (ci, col) in q.columns.iter().enumerate() {
            let key = v8::String::new(scope, col).unwrap();
            let val = db_value_to_v8(scope, &row[ci]);
            obj.set(scope, key.into(), val);
        }
        arr.set_index(scope, ri as u32, obj.into());
    }
    arr.into()
}

/// Native `__s2_sqlite_query(handle, sql, params) -> Promise<Row[]>`. Owner-checks + queues the SELECT
/// on the connection's actor thread (`db::submit_query`); the Promise resolves later via `resolve_db`
/// with the row array. An invalid handle / closed connection rejects the Promise immediately, with no
/// RESOLVERS/PENDING_JOBS/ledger entry (no pending job to track). MIRRORS `s2_db_remote_query`.
fn s2_sqlite_query(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();

        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);

        let id = next_async_id();
        match crate::db::submit_query(id, handle, sql, params, &owner) {
            Ok(()) => {
                let job_owner = resolver_owner_tag(scope);
                if let Some((ref oid, _)) = job_owner {
                    REGISTRY.with(|r| {
                        if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_job(id); }
                    });
                }
                RESOLVERS.with(|m| {
                    m.borrow_mut()
                        .insert(id, ResolverEntry { owner: job_owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
                });
                PENDING_JOBS.with(|c| c.set(c.get() + 1));
                refresh_detour();
            }
            Err(e) => {
                let msg = v8::String::new(scope, &e).unwrap();
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
            }
        }
        rv.set(promise.into());
    }));
}

/// Native `__s2_sqlite_execute(handle, sql, params) -> Promise<{changes, lastInsertId}>`. Same shape
/// as `s2_sqlite_query` but queues an INSERT/UPDATE/DELETE/DDL (`db::submit_execute`); resolves later
/// via `resolve_db` with `{changes, lastInsertId}`.
fn s2_sqlite_execute(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();

        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);

        let id = next_async_id();
        match crate::db::submit_execute(id, handle, sql, params, &owner) {
            Ok(()) => {
                let job_owner = resolver_owner_tag(scope);
                if let Some((ref oid, _)) = job_owner {
                    REGISTRY.with(|r| {
                        if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_job(id); }
                    });
                }
                RESOLVERS.with(|m| {
                    m.borrow_mut()
                        .insert(id, ResolverEntry { owner: job_owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
                });
                PENDING_JOBS.with(|c| c.set(c.get() + 1));
                refresh_detour();
            }
            Err(e) => {
                let msg = v8::String::new(scope, &e).unwrap();
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
            }
        }
        rv.set(promise.into());
    }));
}

/// Native `__s2_sqlite_close(handle) -> Promise<void>`. Closes the connection (a harmless no-op
/// if already closed / never open) and always resolves `undefined` — teardown may later close the
/// same handle again (idempotent), so `close()` never rejects.
fn s2_sqlite_close(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1);
        let owner = current_plugin(scope).unwrap_or_default();
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        if handle >= 0 {
            crate::db::close(handle as u64, &owner);
        }
        let undef = v8::undefined(scope);
        resolver.resolve(scope, undef.into());
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
fn s2_db_remote_connect(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cfg = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_default();
        match crate::sqldb::connect(&cfg, &owner) {
            Ok(handle) => {
                // Ledger the connection against the CALLING plugin (teardown authority) — a
                // non-plugin/unknown owner (the shared HOST context) is a safe no-op.
                if let Some((ref oid, _)) = resolver_owner_tag(scope) {
                    REGISTRY.with(|r| {
                        if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                            l.record_remote_db_conn(handle);
                        }
                    });
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
fn s2_db_remote_query(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();

        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);

        let pool = match crate::sqldb::get_pool(handle, &owner) {
            Ok(p) => p,
            Err(e) => {
                let msg = v8::String::new(scope, &e).unwrap();
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
                rv.set(promise.into());
                return;
            }
        };

        let id = next_async_id();
        // Tag the resolver with the CALLING plugin's (id, current generation) — the async-liveness guard.
        let job_owner = resolver_owner_tag(scope);
        // Ledger this async job against the CALLING plugin (teardown authority) — a non-plugin/
        // unknown owner is a safe no-op; no borrow held across a JS call.
        if let Some((ref oid, _)) = job_owner {
            REGISTRY.with(|r| {
                if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                    l.record_job(id);
                }
            });
        }
        RESOLVERS.with(|m| {
            m.borrow_mut()
                .insert(id, ResolverEntry { owner: job_owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
        });
        PENDING_JOBS.with(|c| c.set(c.get() + 1));
        crate::sqldb::spawn_query(id, pool, sql, params);
        refresh_detour();
        rv.set(promise.into());
    }));
}

/// Native `__s2_db_remote_execute(handle, sql, params) -> Promise<{changes, lastInsertId}>`. Same
/// shape as `s2_db_remote_query` (owner-scoped pool resolve + early-reject-on-invalid-handle, then
/// the `s2_fetch`-mirrored resolver/ledger/RESOLVERS/PENDING_JOBS/refresh_detour block), but spawns
/// an INSERT/UPDATE/DELETE/DDL statement (`spawn_execute`); the Promise resolves later via
/// `resolve_db` with `{changes, lastInsertId}`.
fn s2_db_remote_execute(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();

        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);

        let pool = match crate::sqldb::get_pool(handle, &owner) {
            Ok(p) => p,
            Err(e) => {
                let msg = v8::String::new(scope, &e).unwrap();
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
                rv.set(promise.into());
                return;
            }
        };

        let id = next_async_id();
        // Tag the resolver with the CALLING plugin's (id, current generation) — the async-liveness guard.
        let job_owner = resolver_owner_tag(scope);
        // Ledger this async job against the CALLING plugin (teardown authority) — a non-plugin/
        // unknown owner is a safe no-op; no borrow held across a JS call.
        if let Some((ref oid, _)) = job_owner {
            REGISTRY.with(|r| {
                if let Some(l) = r.borrow_mut().ledger_mut(oid) {
                    l.record_job(id);
                }
            });
        }
        RESOLVERS.with(|m| {
            m.borrow_mut()
                .insert(id, ResolverEntry { owner: job_owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
        });
        PENDING_JOBS.with(|c| c.set(c.get() + 1));
        crate::sqldb::spawn_execute(id, pool, sql, params);
        refresh_detour();
        rv.set(promise.into());
    }));
}

/// Native `__s2_db_remote_close(handle) -> Promise<void>`. MIRRORS `s2_sqlite_close`: closes the
/// pool (a harmless no-op if already closed / never open, regardless of the `sqldb::close`
/// bool-return) and always resolves `undefined` — teardown may later close the same handle again
/// (idempotent), so `close()` never rejects.
fn s2_db_remote_close(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1);
        let owner = current_plugin(scope).unwrap_or_default();
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        if handle >= 0 {
            crate::sqldb::close(handle as u64, &owner);
        }
        let undef = v8::undefined(scope);
        resolver.resolve(scope, undef.into());
        rv.set(promise.into());
    }));
}

/// Resolve (or drop, on the async-liveness guard) a completed remote DB query/execute job in its
/// OWNING plugin's context — MIRRORS `resolve_fetch`'s owner-liveness + context-clone +
/// HandleScope/ContextScope preamble exactly (the use-after-free killer: never resolve into a
/// disposed/replaced context), but resolves with the row array (`query_result_to_js`) or the
/// `{changes, lastInsertId}` object on `Ok`, or rejects with an `Error` on `Err` (a SQL/connection
/// failure surfaced by `sqldb::run_query`/`run_execute`).
fn resolve_db(host: &mut Host, entry: &ResolverEntry, result: Result<crate::db::DbOutcome, String>) {
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

    match result {
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
            obj.set(scope, k1.into(), v1.into());
            obj.set(scope, k2.into(), v2.into());
            resolver.resolve(scope, obj.into());
        }
        Err(e) => {
            let msg = v8::String::new(scope, &e).unwrap_or_else(|| v8::String::new(scope, "db error").unwrap());
            let ex = v8::Exception::error(scope, msg);
            resolver.reject(scope, ex);
        }
    }
}

/// The result of STARTING a plugin load (L1 lifecycle v2). The load's TRANSITION (arm → Active, or
/// teardown → Failed) happens later, in `finalize_loading_plugins` (the sync fast-path runs it inline
/// at the tail of `load_plugin_js`; an async factory settles on a later `frame_async_drain`).
enum LoadStart {
    /// The artifact was valid; the factory was driven and an in-flight `LOADING` entry registered.
    Started,
    /// The bundle is not a `plugin()` artifact (legacy `onLoad` shape or a malformed default export)
    /// — fail loud, tear down the fresh context (never run it).
    Refused(String),
    /// Host not initialized / context missing — nothing to do.
    Aborted,
}

/// Load a built plugin bundle `plugin_js` under plugin id `id` (the L1 lifecycle-v2 artifact path).
///
/// Steps: (1) `create_plugin_context(id)` — a fresh per-plugin context with the full injected API
/// (`__s2require` + the engine-generic prelude + any registered game preludes); (2) evaluate the CJS
/// wrapper `(function(require,module,exports){…})(require, module, module.exports)` in that context
/// and CAPTURE the RETURNED `module.exports`; (3) require `module.exports.default` to be a `plugin()`
/// definition (`{ __s2plugin: 1, factory }`) — fail LOUD otherwise (locked decision #5); (4) register
/// an in-flight `LOADING` entry and drive `globalThis.__s2_run_factory(def)`. The factory runs against
/// a load-scoped `ctx`; its settle (`__s2_load_settled`/`__s2_load_failed`) marks the `LOADING` state,
/// and `finalize_loading_plugins` performs the actual arm-at-Active / teardown-on-Failed transition —
/// inline here for a synchronous factory (the whole base suite), or on a later drain for an async one.
///
/// Degrade-never-crash: a compile/run error logs a named WARN and tears down; no exception propagates
/// (the whole JS run is under a `TryCatch`).
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
    // in the factory see it). @s2script/config's getters read globalThis.__s2pkg_config_values.
    let _ = eval_in_context(id, &format!("globalThis.__s2pkg_config_values = {};", config_values_json));

    // The spike's PROVEN wrapper — the outer arrow-IIFE returns `module.exports` so `script.run`
    // hands it straight back to Rust.  `{PLUGIN_JS}` is spliced verbatim.
    let wrapper = format!(
        "(() => {{\n  const module = {{ exports: {{}} }};\n  const require = globalThis.__s2_require;\n  (function (require, module, exports) {{\n{}\n}})(require, module, module.exports);\n  return module.exports;\n}})()",
        plugin_js
    );

    let start = HOST.with(|h| -> LoadStart {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else {
            log_warn("WARN: load_plugin_js called before init");
            return LoadStart::Aborted;
        };

        // Clone the plugin's Global<Context> out of PLUGINS (cheap refcount bump); release the
        // borrow before opening the HandleScope on HOST.isolate.
        let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.context.clone())) else {
            log_warn(&format!("WARN: load_plugin_js('{}'): context missing after create", id));
            return LoadStart::Aborted;
        };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Compile+run the wrapper, detect the plugin() artifact, drive the factory — all under one
        // TryCatch so a throwing bundle can't leak a pending exception into later frames.
        let start: LoadStart = 'blk: {
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let Some(code) = v8::String::new(tc, &wrapper) else {
                break 'blk LoadStart::Refused("failed to intern source".into());
            };
            let ret = match v8::Script::compile(tc, code, None).and_then(|s| s.run(tc)) {
                Some(r) => r,
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "unknown error".into());
                    log_warn(&format!("WARN: load_plugin_js('{}'): eval error: {}", id, msg));
                    crate::crash::report_js_error(id, "load", &msg, "");
                    break 'blk LoadStart::Refused(format!("eval error: {}", msg));
                }
            };
            // The wrapper returns `module.exports` — must be an object.
            let Ok(exports) = v8::Local::<v8::Object>::try_from(ret) else {
                break 'blk LoadStart::Refused("module.exports is not an object".into());
            };

            // (3) The artifact: module.exports.default must be a plugin() definition (spec §1.1).
            let def_obj: Option<v8::Local<v8::Object>> = v8::String::new(tc, "default")
                .and_then(|k| exports.get(tc, k.into()))
                .and_then(|v| v8::Local::<v8::Object>::try_from(v).ok());
            let is_plugin = def_obj
                .map(|o| {
                    let tag_ok = v8::String::new(tc, "__s2plugin")
                        .and_then(|k| o.get(tc, k.into()))
                        .and_then(|t| t.int32_value(tc))
                        .map(|n| n == 1)
                        .unwrap_or(false);
                    let factory_ok = v8::String::new(tc, "factory")
                        .and_then(|k| o.get(tc, k.into()))
                        .map(|f| f.is_function())
                        .unwrap_or(false);
                    tag_ok && factory_ok
                })
                .unwrap_or(false);

            if !is_plugin {
                // Fail loud (locked decision #5): a legacy onLoad shape or a malformed default export.
                let has_legacy = v8::String::new(tc, "onLoad")
                    .and_then(|k| exports.get(tc, k.into()))
                    .map(|v| v.is_function())
                    .unwrap_or(false);
                let reason = if has_legacy {
                    "legacy plugin shape (export onLoad) - rebuild with @s2script/sdk >= 0.2: export default plugin(factory)"
                } else {
                    "default export is not a plugin() definition"
                };
                log_warn(&format!("WARN: load('{}'): {}", id, reason));
                crate::crash::report_js_error(id, "load", reason, "");
                break 'blk LoadStart::Refused(reason.to_string());
            }
            let def = def_obj.expect("is_plugin implies def_obj is Some");

            // Register the in-flight load BEFORE running the factory (a SYNC settle mutates this entry).
            LOADING.with(|l| {
                l.borrow_mut().insert(
                    id.to_string(),
                    LoadingEntry {
                        started_frame: FRAME_COUNTER.with(|c| c.get()),
                        state: SettleState::InFlight,
                        pending_reload: false,
                    },
                )
            });

            // Drive the factory: globalThis.__s2_run_factory(def). It builds the load-scoped ctx,
            // calls def.factory(ctx), and settles via __s2_load_settled / __s2_load_failed.
            let global = tc.get_current_context().global(tc);
            let run_ok = v8::String::new(tc, "__s2_run_factory")
                .and_then(|k| global.get(tc, k.into()))
                .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok())
                .map(|run_f| {
                    let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                    run_f.call(tc, recv, &[def.into()]).is_some()
                })
                .unwrap_or(false);
            if !run_ok {
                let msg = tc
                    .exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "factory driver threw".into());
                LOADING.with(|l| {
                    if let Some(e) = l.borrow_mut().get_mut(id) {
                        e.state = SettleState::Failed(msg);
                    }
                });
            }
            LoadStart::Started
        };
        start
    });

    match start {
        // Sync fast-path: a synchronous factory is already Settled, so this arms + reconciles + goes
        // Active within this same call (preserving today's synchronous-load semantics). An async
        // factory stays InFlight and finalizes on a later frame_async_drain.
        LoadStart::Started => finalize_loading_plugins(),
        // Fail loud: record the reason + tear down the fresh (never-Active) context (HOST released).
        LoadStart::Refused(reason) => {
            FAILED_PLUGINS.with(|f| { f.borrow_mut().insert(id.to_string(), reason); });
            unload_partial(id);
        }
        LoadStart::Aborted => {}
    }
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

/// Resolve (or drop, on the async-liveness guard) a completed `__s2_fetch` job in its OWNING
/// plugin's context — MIRRORS `resolve_or_drop`'s owner-liveness + context-clone +
/// HandleScope/ContextScope preamble exactly (the use-after-free killer: never resolve into a
/// disposed/replaced context), but builds the raw `{status, ok, statusText, headers, body}`
/// Response payload on `Ok`, or rejects with an `Error` on `Err` (a network/timeout failure),
/// instead of `resolve_or_drop`'s bare `undefined`.
fn resolve_fetch(
    host: &mut Host,
    entry: &ResolverEntry,
    result: Result<crate::http::FetchResponse, String>,
) {
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

    match result {
        Ok(r) => {
            let obj = v8::Object::new(scope);
            let status_key = v8::String::new(scope, "status").unwrap();
            obj.set(scope, status_key.into(), v8::Number::new(scope, r.status as f64).into());
            let ok_key = v8::String::new(scope, "ok").unwrap();
            obj.set(scope, ok_key.into(), v8::Boolean::new(scope, (200..300).contains(&r.status)).into());
            let status_text_key = v8::String::new(scope, "statusText").unwrap();
            let status_text_val = v8::String::new(scope, &r.status_text)
                .unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            obj.set(scope, status_text_key.into(), status_text_val.into());
            let hobj = v8::Object::new(scope);
            for (k, v) in &r.headers {
                let Some(kk) = v8::String::new(scope, k) else { continue };
                let vv = v8::String::new(scope, v).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
                hobj.set(scope, kk.into(), vv.into());
            }
            let headers_key = v8::String::new(scope, "headers").unwrap();
            obj.set(scope, headers_key.into(), hobj.into());
            let body_key = v8::String::new(scope, "body").unwrap();
            let body_val = v8::String::new(scope, &r.body)
                .unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            obj.set(scope, body_key.into(), body_val.into());
            resolver.resolve(scope, obj.into());
        }
        Err(e) => {
            let msg = v8::String::new(scope, &e).unwrap_or_else(|| v8::String::new(scope, "fetch error").unwrap());
            let ex = v8::Exception::error(scope, msg);
            resolver.reject(scope, ex);
        }
    }
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
            // A CALLBACK timer fires its function and, when repeating, re-arms. Take the entry out
            // while firing so a callback that kills its own timer (or creates one) cannot observe a
            // half-updated map or double-borrow TIMER_CBS.
            if let Some(cb) = TIMER_CBS.with(|m| m.borrow_mut().remove(&id)) {
                // Clear any stale record for this id, fire, then ask whether the callback killed it.
                TIMER_KILLED.with(|k| { k.borrow_mut().remove(&id); });
                let owner_live = fire_timer_cb(host, &cb);
                // Re-arm only if it repeats AND the callback did not kill itself during the fire
                // AND the owner is still live. Otherwise the entry stays removed and it is done.
                let self_killed = TIMER_KILLED.with(|k| k.borrow_mut().remove(&id));
                if let (Some(iv), true, false) = (cb.interval_ms, owner_live, self_killed) {
                    TIMERS.with(|t| t.borrow_mut()
                        .push(id, TimerKind::Deadline(Instant::now() + Duration::from_millis(iv))));
                    TIMER_CBS.with(|m| m.borrow_mut().insert(id, cb));
                }
                continue;
            }
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
        // Resolve completed fetch requests (payload-carrying, from the tokio+reqwest engine in
        // core/src/http.rs; Slice HTTP Task 2). Mirrors the pool-completion loop above exactly,
        // except the payload is a built Response object (or a rejection) via `resolve_fetch`.
        while let Some(c) = crate::http::try_recv_completed() {
            let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&c.id)) else { continue };
            PENDING_JOBS.with(|cnt| cnt.set(cnt.get().saturating_sub(1)));
            resolve_fetch(host, &entry, c.result);
        }
        // Remote SQL completions (core/src/sqldb.rs). Mirrors the http loop: pop a completion, remove
        // its RESOLVERS entry, decrement PENDING_JOBS, resolve/reject (or DROP on the liveness guard).
        while let Some(c) = crate::db::try_recv_completed() {
            let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&c.id)) else { continue };
            PENDING_JOBS.with(|cnt| cnt.set(cnt.get().saturating_sub(1)));
            resolve_db(host, &entry, c.result);
        }
        // Poll ws/net. The tick only polls: each module matches its own signal kinds, queues
        // events internally, and hands back connect results + deferred drops.
        //
        // ORDERING (load-bearing): Connected/ConnectFailed resolve/reject the connect Promise
        // INSIDE this drain (before the microtask checkpoint below, so the plugin's `.then` —
        // which subscribes onMessage — runs THIS frame). Events fan out AFTER this drain returns
        // (`dispatch_pending_*`, HOST free). Deregistering a conn is DEFERRED past the checkpoint.
        // `Connected` resolves the connect Promise here, but the plugin's `.then` does not run
        // until the checkpoint. If a terminal signal for the SAME conn is in this batch (a server
        // that dies right after the handshake sends Connected then Closed(1006)), dropping it here
        // removes it from `conns` before that continuation runs, so its subscribe fails the
        // ownership gate and its close event fans out to nobody.
        let ws = crate::ws::poll_signals();
        for (id, result) in ws.connects {
            if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) {
                PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                resolve_ws_connect(host, &entry, id, result);
            }
        }
        let net = crate::net::poll_signals();
        for (id, result) in net.connects {
            if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) {
                PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                resolve_net_connect(host, &entry, id, result);
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
        scope.perform_microtask_checkpoint();

        // NOW deregister the conns whose terminal signal arrived above. Every continuation queued by
        // this drain has run, so a `.then` that subscribes to the connection it was just handed has
        // already been able to do so. Dropping earlier is what made a server dying right after the
        // handshake look like a connection that simply never spoke.
        for id in ws.drops { crate::ws::drop_conn(id); }
        for id in net.drops { crate::net::drop_conn(id); }
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

    // DAMAGE_MUX: the DispatchTraceAttack detour stays installed for the process lifetime — no follow-up.
    crate::owner_stores::register(
        "DAMAGE_MUX",
        Box::new(|owner| { DAMAGE_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { DAMAGE_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            DAMAGE_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );

    // CHAT_MSG_SUBS + CLIENT_CMD_SUBS + CONCOMMANDS: registered by the feature module.
    crate::commands::register_stores();

    // CLIENT_MUX: registered by the feature module, which owns the mux the callbacks close over.
    crate::client::register_store();

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
    fn reg(name: &'static str, phase: crate::process_singletons::ResetPhase, f: impl Fn() + 'static) {
        crate::process_singletons::register(name, phase, Box::new(f));
    }

    // ---- BeforeIsolateDrop: holds V8 handles, or must be torn down while the isolate lives. ----

    // Async state: RESOLVERS holds Globals into the isolate, so the handles must be released here.
    reg("TIMERS", BeforeIsolateDrop, || TIMERS.with(|t| *t.borrow_mut() = TimerQueue::new()));
    reg("RESOLVERS", BeforeIsolateDrop, || RESOLVERS.with(|m| m.borrow_mut().clear()));
    reg("TIMER_CBS", BeforeIsolateDrop, || TIMER_CBS.with(|m| m.borrow_mut().clear()));
    reg("TIMER_KILLED", BeforeIsolateDrop, || TIMER_KILLED.with(|k| k.borrow_mut().clear()));
    // PENDING_REJECTS holds only Strings, so drop order vs the isolate is not load-bearing; kept in
    // this phase to preserve the historical position.
    reg("PENDING_REJECTS", BeforeIsolateDrop, || PENDING_REJECTS.with(|m| m.borrow_mut().clear()));
    // TopMenu categories/seq/pending — the ITEMS map is an owner-scoped store, these are not
    // (categories outlive the registering plugin, SM parity).
    reg("TOPMENU_CATEGORIES", BeforeIsolateDrop, || TOPMENU_CATEGORIES.with(|c| c.borrow_mut().clear()));
    reg("TOPMENU_SEQ", BeforeIsolateDrop, || TOPMENU_SEQ.with(|c| c.set(0)));
    reg("TOPMENU_PENDING", BeforeIsolateDrop, || TOPMENU_PENDING.with(|q| q.borrow_mut().clear()));
    // Inter-plugin method + subscriber Globals.
    reg("IFACE_METHODS", BeforeIsolateDrop, || IFACE_METHODS.with(|m| m.borrow_mut().clear()));
    reg("IFACE_SUBS", BeforeIsolateDrop, || IFACE_SUBS.with(|m| m.borrow_mut().clear()));
    reg("IFACES", BeforeIsolateDrop, || IFACES.with(|r| r.borrow_mut().clear()));
    // The publishes registries: per-plugin unload clears these per id, but a plugin that was `set`
    // and never loaded leaves an entry no unload ever walks. This is the teardown backstop.
    reg("PLUGIN_PUBLISHES", BeforeIsolateDrop, || PLUGIN_PUBLISHES.with(|p| p.borrow_mut().clear()));
    reg("UNDECLARED_PUBLISHES", BeforeIsolateDrop, || UNDECLARED_PUBLISHES.with(|p| p.borrow_mut().clear()));
    reg("NEXT_SUB_ID", BeforeIsolateDrop, || NEXT_SUB_ID.with(|c| c.set(1)));
    // Per-plugin contexts: each Global<Context> points into the isolate.
    reg("PLUGINS", BeforeIsolateDrop, || PLUGINS.with(|p| p.borrow_mut().clear()));
    reg("REGISTRY", BeforeIsolateDrop, || REGISTRY.with(|r| *r.borrow_mut() = plugin::Registry::new()));
    reg("PENDING_JOBS", BeforeIsolateDrop, || PENDING_JOBS.with(|c| c.set(0)));
    reg("DETOUR_INSTALLED", BeforeIsolateDrop, || DETOUR_INSTALLED.with(|c| c.set(false)));

    // ---- AfterIsolateDrop: pure Rust, no V8 handles. ----

    reg("FRAME_COUNTER", AfterIsolateDrop, || FRAME_COUNTER.with(|c| c.set(0)));
    // Pending queues drained by the muxes' post-frame dispatch — sidecars, not subscriber stores.
    crate::cookies::register_singletons();
    crate::ws::register_singletons();
    crate::net::register_singletons();
    // usermsg name→id resolution caches (the MUX itself is an owner-scoped store). Registered by the
    // feature module — same phase, same position in the order.
    crate::usermsg::register_singletons();
    reg("PENDING_HANDOFF", AfterIsolateDrop, || PENDING_HANDOFF.with(|h| h.borrow_mut().clear()));
    // L1 lifecycle-v2 load state.
    reg("LOADING", AfterIsolateDrop, || LOADING.with(|l| l.borrow_mut().clear()));
    reg("FAILED_PLUGINS", AfterIsolateDrop, || FAILED_PLUGINS.with(|f| f.borrow_mut().clear()));
    reg("MANIFEST_VERSIONS", AfterIsolateDrop, || MANIFEST_VERSIONS.with(|m| m.borrow_mut().clear()));
    // A `-1` cached before the schema was live must not persist across an init cycle.
    reg("SCHEMA_OFFSETS", AfterIsolateDrop, || SCHEMA_OFFSETS.with(|c| *c.borrow_mut() = crate::schema::OffsetCache::new()));
    // Host-global caches — deliberately NOT owner-scoped (see this fn's doc comment). Each feature
    // module registers its OWN slots, which is how the admin gap got closed: seven admin statics
    // existed here and only three were ever registered.
    crate::admin::register_singletons();
    crate::bans::register_singletons();
    crate::events::register_singletons();
    reg("CRASH_BREADCRUMB", AfterIsolateDrop, || crate::crash::breadcrumb::clear_plugins());
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
    crate::crash::breadcrumb::plugin_unloaded(id);

    // Phase-aware entry (design spec §5.4): a plugin still LOADING never reached Active — it has no
    // hooks to run and its subs are still buffered (nothing to sweep). Seal its ctx, drop the LOADING
    // entry, and walk the PARTIAL ledger (any DB conn / timer / import acquired before it was unloaded).
    if matches!(plugin_phase(id), Some(crate::plugin::Phase::Loading)) {
        let _ = eval_in_context(id, "globalThis.__s2_ctx_seal && globalThis.__s2_ctx_seal();");
        LOADING.with(|l| { l.borrow_mut().remove(id); });
        unload_partial(id);
        return;
    }

    // Active/Unloading: mark Unloading, then the full teardown.
    PLUGINS.with(|p| {
        if let Some(pi) = p.borrow_mut().get_mut(id) {
            pi.phase = crate::plugin::Phase::Unloading;
        }
    });

    // (a) Sweep every owner-scoped subscription store in registration order (design spec §6). This
    // replaces the hand-maintained cascade: each store's `remove_by_owner` closure (registered in
    // `register_builtin_stores`) drops the plugin's handler Globals / rules and runs its own follow-up
    // engine-op (event_unsubscribe, the PRE-hook GameEvent removal, usermsg_hook_unsub, transmit
    // re-push, config unwatch, ConCommand/flag-meta drop, TopMenu-item drop). The FRAME store also
    // reconciles the detour; the trailing refresh_detour here re-applies the combined predicate as the
    // source of truth (idempotent).
    crate::owner_stores::sweep_owner(id);
    refresh_detour();

    // (b) state() handoff + best-effort onUnload in the plugin's OWN context.
    capture_state_and_run_onunload(id);

    // (c)–(e) ledger reverse-walk + iface cleanup + exports/context drop.
    teardown_ledger_and_dispose(id);
}

/// Teardown for a never-Active plugin (a Failed load, or an unload while still Loading — design spec
/// §5.4). Sweeps the owner-scoped stores (a no-op for still-buffered subs), then walks the PARTIAL
/// ledger + disposes the context. Skips `state()`/`onUnload` — the plugin was never Active.
fn unload_partial(id: &str) {
    crate::owner_stores::sweep_owner(id);
    refresh_detour();
    teardown_ledger_and_dispose(id);
}

/// Capture the reload-handoff via the plugin's `state()` hook, then run its `onUnload()` — both off
/// the settled `PluginHooks` object (`pi.exports`), in the plugin's OWN context. Clone the context +
/// hooks out of PLUGINS (borrow released) so the hooks may re-enter PLUGINS/FRAME/etc. without a
/// double borrow. `state()` runs FIRST (serialize via `iface_to_json` → `PENDING_HANDOFF`, WARN on a
/// non-serializable return); `onUnload()`'s return is IGNORED (WARN once if it returns non-undefined —
/// use `state()` for the handoff).
fn capture_state_and_run_onunload(id: &str) {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };
        let Some((g_ctx, Some(hooks))) =
            PLUGINS.with(|p| p.borrow().get(id).map(|pi| (pi.context.clone(), pi.exports.clone())))
        else {
            return; // no context or no settled hooks → nothing to call
        };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

        let hooks_local = v8::Local::new(tc, &hooks);
        let recv: v8::Local<v8::Value> = v8::undefined(tc).into();

        // (1) state() FIRST — its serializable return becomes the reload-handoff blob.
        if let Some(f) = v8::String::new(tc, "state")
            .and_then(|k| hooks_local.get(tc, k.into()))
            .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok())
        {
            match f.call(tc, recv, &[]) {
                Some(ret) => {
                    if !ret.is_undefined() && !ret.is_null() {
                        match iface_to_json(tc, ret) {
                            Some(blob) => PENDING_HANDOFF.with(|h| {
                                h.borrow_mut().insert(id.to_string(), blob);
                            }),
                            None => log_warn(&format!(
                                "WARN: unload_plugin('{}'): state() return not serializable — no state handoff",
                                id
                            )),
                        }
                    }
                }
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "state() threw".into());
                    log_warn(&format!("WARN: unload_plugin('{}'): state() error: {}", id, msg));
                }
            }
        }

        // (2) onUnload() — return IGNORED (use state() for the handoff).
        if let Some(f) = v8::String::new(tc, "onUnload")
            .and_then(|k| hooks_local.get(tc, k.into()))
            .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok())
        {
            match f.call(tc, recv, &[]) {
                Some(ret) => {
                    if !ret.is_undefined() {
                        log_warn(&format!(
                            "WARN: unload_plugin('{}'): onUnload return is ignored - use state() for the reload handoff",
                            id
                        ));
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
    });
}

/// The ledger reverse-walk + iface cleanup + exports/context drop shared by `unload_plugin` (Active)
/// and `unload_partial` (never-Active). `REGISTRY.remove` yields the entry (also making `is_live` false
/// for any lingering resolver of this generation).
fn teardown_ledger_and_dispose(id: &str) {
    // (c) Ledger reverse-walk: the teardown authority.  REGISTRY.remove yields the entry (also makes
    // is_live false for any lingering resolver of this generation).
    if let Some(entry) = REGISTRY.with(|r| r.borrow_mut().remove(id)) {
        for res in entry.ledger.teardown_order() {
            match res {
                plugin::Resource::Timer(tid) => {
                    TIMERS.with(|t| { t.borrow_mut().remove(tid); });
                    RESOLVERS.with(|m| { m.borrow_mut().remove(&tid); });
                    // A repeating callback timer re-arms itself, so failing to drop it here would
                    // leave it firing into a dead context forever — the ledger is the teardown
                    // authority precisely so this does not depend on the plugin's own cleanup.
                    TIMER_CBS.with(|m| { m.borrow_mut().remove(&tid); });
                    TIMER_KILLED.with(|k| { k.borrow_mut().remove(&tid); });
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
                    // Prunes IFACE_METHODS by interface NAME. Safe by construction since the
                    // contract-grammar slice: InterfaceRegistry::publish rejects a second live
                    // producer of a name (spec §4.8), so at most one producer can ever hold the
                    // methods being pruned here. (Retires the slice-5 TODO, which asked for a
                    // (producer_id, name) key against a case that can no longer occur.)
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
                plugin::Resource::DbConn(h) => {
                    // A late/never `close()` — teardown closes the connection now, passing the
                    // unloading plugin's OWN id as the owner (it owns every handle in its ledger).
                    // Idempotent (an already-closed handle is a harmless no-op inside db::close).
                    crate::db::close(h, id);
                }
                plugin::Resource::WsConn(conn_id) => {
                    // A late/never `close()` — teardown closes the ws connection now regardless of
                    // owner (the ledger owns the id; `drop_conn` mirrors `db::close`'s idempotence —
                    // an already-removed conn_id is a harmless no-op inside ws::drop_conn). This also
                    // covers the ConnectFailed case (the drain step already called drop_conn once).
                    crate::ws::drop_conn(conn_id);
                }
                plugin::Resource::NetConn(conn_id) => {
                    // A late/never `close()` — teardown drops the raw socket now regardless of owner
                    // (the ledger owns the id; `net::drop_conn` is idempotent — an already-removed
                    // conn_id is a harmless no-op). Also covers the ConnectFailed/Closed cases (the
                    // drain step already called drop_conn once).
                    crate::net::drop_conn(conn_id);
                }
                plugin::Resource::RemoteDbConn(h) => {
                    // Late/never close() — teardown drops the pool now (idempotent; a wrong/absent
                    // handle is a harmless no-op inside sqldb::close). Passes the unloading plugin's
                    // own id (it owns every handle in its ledger).
                    crate::sqldb::close(h, id);
                }
            }
        }
    }
    // Drop any subscriber rows this plugin (as a consumer) still holds, and its import declarations.
    // Idempotent with the per-resource drops above (remove() is a no-op on missing keys).
    let orphaned = IFACES.with(|r| r.borrow_mut().remove_subscribers_by_consumer(id));
    IFACE_SUBS.with(|m| { let mut mm = m.borrow_mut(); for (_iface, sid) in orphaned { mm.remove(&sid); } });
    IFACES.with(|r| r.borrow_mut().clear_imports(id));
    clear_plugin_publishes(id);
    // Plugin-declared engine calls: drop this plugin's descriptor table (spec §12 "Unload" row). On
    // BOTH teardown paths (Active and never-Active), so a reload always re-resolves from scratch
    // rather than inheriting a stale call id.
    crate::gamedata_calls::drop_plugin(id);
    // Declarative inbound hooks: drop the descriptors this plugin DECLARED. Its subscriptions to
    // other owners' hooks are swept by the `HOOK_MUX` owner-store above. Nothing is uninstalled —
    // an installed detour outlives its declaring plugin by design (spec §6), and its slot is kept so
    // a reload re-uses it instead of burning a second one.
    crate::gamedata_hooks::drop_owner(id);
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

/// A snapshot of a `LOADING` entry's settle state (owned — `SettleState::Failed(String)` is not
/// `Clone`, so `finalize_loading_plugins` snapshots into this before releasing the LOADING borrow).
enum SettleSnapshot {
    Settled,
    Failed(String),
    InFlight,
}

/// Drive every in-flight load to its transition (design spec §5). Called (1) at the tail of
/// `frame_async_drain` — after the microtask checkpoint that runs factory continuations — and (2)
/// inline at the tail of `load_plugin_js` for the SYNC fast-path (a synchronous factory is already
/// `Settled`, so it arms + reconciles + goes Active within the same call). Must be called HOST-free.
pub(crate) fn finalize_loading_plugins() {
    let frame = FRAME_COUNTER.with(|c| c.get());
    let snapshot: Vec<(String, SettleSnapshot, bool, u64)> = LOADING.with(|l| {
        l.borrow()
            .iter()
            .map(|(id, e)| {
                let s = match &e.state {
                    SettleState::Settled => SettleSnapshot::Settled,
                    SettleState::Failed(m) => SettleSnapshot::Failed(m.clone()),
                    SettleState::InFlight => SettleSnapshot::InFlight,
                };
                (id.clone(), s, e.pending_reload, e.started_frame)
            })
            .collect()
    });

    for (id, state, pending_reload, started) in snapshot {
        match state {
            SettleSnapshot::Settled => {
                // (1) arm: replay buffered registrations + seal the ctx. A registration that throws
                // while arming aborts the arm (host TryCatch → eval_in_context Err) → Failed.
                let arm_ok = eval_in_context(&id, "globalThis.__s2_ctx_arm && globalThis.__s2_ctx_arm();").is_ok();
                if !arm_ok {
                    fail_load(&id, "a registration failed while arming at Active");
                    continue_or_reload(&id, pending_reload);
                    continue;
                }
                // (2) publishes reconciliation MOVES here from the loader (design spec §4).
                if let Err(e) = reconcile_publishes(&id) {
                    fail_load(&id, &format!("publishes: {}", e));
                    continue_or_reload(&id, pending_reload);
                    continue;
                }
                // (3) Active.
                PLUGINS.with(|p| {
                    if let Some(pi) = p.borrow_mut().get_mut(&id) {
                        pi.phase = crate::plugin::Phase::Active;
                    }
                });
                LOADING.with(|l| { l.borrow_mut().remove(&id); });
                if let Some(version) = plugin_manifest_version(&id) {
                    crate::crash::breadcrumb::plugin_loaded(&id, &version);
                }
                log_warn(&format!("[plugins] '{}' Active", id));
                if pending_reload {
                    crate::loader::request_reload(&id);
                }
            }
            SettleSnapshot::Failed(msg) => {
                fail_load(&id, &msg);
                continue_or_reload(&id, pending_reload);
            }
            SettleSnapshot::InFlight if frame.saturating_sub(started) > LOAD_TIMEOUT_FRAMES => {
                let _ = eval_in_context(&id, "globalThis.__s2_ctx_seal && globalThis.__s2_ctx_seal();");
                fail_load(&id, "factory did not settle within ~30s (LOAD_TIMEOUT_FRAMES)");
                continue_or_reload(&id, pending_reload);
            }
            _ => {}
        }
    }

    crate::loader::start_unblocked_waiters(); // T4 provides the real body; a no-op stub until then.
}

/// Fail a never-Active load: WARN + report + record the reason + drop the LOADING entry + tear down
/// the fresh (never-Active) context. The plugin is NOT running.
fn fail_load(id: &str, reason: &str) {
    log_warn(&format!(
        "WARN: load('{}') FAILED: {} - tearing down (the plugin is NOT running)",
        id, reason
    ));
    crate::crash::report_js_error(id, "factory", reason, "");
    FAILED_PLUGINS.with(|f| { f.borrow_mut().insert(id.to_string(), reason.to_string()); });
    LOADING.with(|l| { l.borrow_mut().remove(id); });
    unload_partial(id);
}

/// After a failed/timed-out load, if a reload was queued while it was loading, request it now. The
/// `pending_reload` flag is only ever set by T4's waiting-loads machinery, so this is a no-op today.
fn continue_or_reload(id: &str, pending_reload: bool) {
    if pending_reload {
        crate::loader::request_reload(id);
    }
}

/// The manifest version for `id` (set by the loader before load), for the `Active` breadcrumb.
fn plugin_manifest_version(id: &str) -> Option<String> {
    MANIFEST_VERSIONS.with(|m| m.borrow().get(id).cloned())
}

/// Record a plugin's manifest version (called by the loader before `load_plugin_js`), so the
/// `Active`-transition breadcrumb in `finalize_loading_plugins` can carry it without the manifest.
pub(crate) fn set_plugin_version(id: &str, version: &str) {
    MANIFEST_VERSIONS.with(|m| { m.borrow_mut().insert(id.to_string(), version.to_string()); });
}

/// The current frame count (loader-facing getter over `FRAME_COUNTER`). Used by the loader's WAITING
/// window (`start_unblocked_waiters`) and topo batch to bound the hard-dependency wait.
pub(crate) fn current_frame() -> u64 {
    FRAME_COUNTER.with(|c| c.get())
}

/// True while `id` has an in-flight factory load (between `create_plugin_context` and its
/// `Active`/`Failed` transition). The loader uses this to coalesce a reload-while-Loading into a
/// queued `pending_reload` rather than an unload+load race (design spec §5.3).
pub(crate) fn is_loading(id: &str) -> bool {
    LOADING.with(|l| l.borrow().contains_key(id))
}

/// Queue a reload for a plugin that is still Loading: the reload fires from `continue_or_reload` once
/// the in-flight load reaches its transition, instead of tearing down a half-loaded context.
pub(crate) fn queue_pending_reload(id: &str) {
    LOADING.with(|l| { if let Some(e) = l.borrow_mut().get_mut(id) { e.pending_reload = true; } });
}

/// True if `id`'s load FAILED (context disposed) — backs the `failed` state in `sm plugins list`.
pub(crate) fn is_failed(id: &str) -> bool {
    FAILED_PLUGINS.with(|f| f.borrow().contains_key(id))
}

/// Mark a plugin FAILED without it ever loading (loader refusals: apiVersion major, and — B1 —
/// a `compiledAgainst` typesSha256 mismatch). Shows as `failed` in `sm plugins list`; cleared by
/// the next successful load (load_plugin_js clears on fresh load) or by `clear_failed`.
pub(crate) fn set_failed(id: &str, reason: &str) {
    FAILED_PLUGINS.with(|f| { f.borrow_mut().insert(id.to_string(), reason.to_string()); });
}

/// Drop a FAILED entry (loader: the file vanished — a removed plugin is not `failed`, it is gone).
pub(crate) fn clear_failed(id: &str) {
    FAILED_PLUGINS.with(|f| { f.borrow_mut().remove(id); });
}

/// Every plugin id whose load FAILED (for `sm plugins list`'s `failed` state).
pub(crate) fn failed_plugin_ids() -> Vec<String> {
    FAILED_PLUGINS.with(|f| f.borrow().keys().cloned().collect())
}

/// True if an interface `name` currently has a producer (published AND live), regardless of any
/// consumer's version range. The loader's topo/WAITING gate uses this to decide whether a hard
/// dependency is satisfied before starting a consumer's load.
pub(crate) fn iface_published(name: &str) -> bool {
    IFACES.with(|r| r.borrow().producer_of(name).is_some())
}

/// The published contract hash for `name` (empty string = producer ships none), None when
/// unpublished. The loader's `verify_compiled_against` (B1) fail-fast gate reads this.
pub(crate) fn iface_published_types_sha256(name: &str) -> Option<String> {
    IFACES.with(|r| r.borrow().lookup(name).map(|e| e.types_sha256.clone()))
}

/// Slice 5E.3: drop any pending reload-handoff blob for `id` WITHOUT consuming it — called by the
/// loader on a FINAL removal (Vanished) so a deleted plugin's captured state is discarded rather than
/// handed to a future re-add of the same id.
pub(crate) fn clear_pending_handoff(id: &str) {
    PENDING_HANDOFF.with(|h| { h.borrow_mut().remove(id); });
}

#[cfg(test)]
/// The in-isolate test harness. `pub(crate)` so a FEATURE module's own `#[cfg(test)] mod tests`
/// can drive the shared isolate (`init`/`create_plugin_context`/`eval_in_context`/`shutdown`) instead
/// of its tests having to live here. Extracting `usermsg` proved this out: the harness is genuinely
/// shared infrastructure, the per-feature assertions are not.
pub(crate) mod frame_tests {
    use super::*;
    // Game-event dispatch moved to `crate::events`; the deferred-queue and lifecycle tests below
    // still drive it as their vehicle.
    use crate::events::{dispatch_game_event, dispatch_game_event_pre, replay_game_event};
    // The client-lifecycle dispatch moved to `crate::client`; the fan_out and voice tests
    // below still drive it as their vehicle.
    use crate::client::dispatch_client_event;
    use crate::commands::{dispatch_concommand, ReplySource};
    use crate::ws::dispatch_pending_events as dispatch_pending_ws_events;
    use crate::net::dispatch_pending_events as dispatch_pending_net_events;
    use crate::multiplexer::{Phase, HookResult};
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};
    use std::sync::Mutex;

    /// How many poll iterations an async test drives before declaring the work never completed.
    ///
    /// Every one of these loops does real V8 work per iteration (`frame_async_drain` enters the
    /// isolate), then sleeps 2-10ms, so the loop COMPETES FOR CPU with the very tokio runtime it is
    /// waiting on. That is fine on a dev box — the round trips here complete in ~40ms — but on a
    /// 2-vCPU CI runner also carrying 4 tokio workers, the db actors' dedicated OS threads and a V8
    /// isolate, a multi-second scheduling stall is reachable, and the old budget of 500 ticks was as
    /// little as 1s (the 2ms loops).
    ///
    /// That is what made the `ws_module_*` tests fail intermittently in CI and never locally, on a
    /// DIFFERENT test each run — whichever async test happened to hit the stall. Three consecutive
    /// runs failed across two branches, including a re-run of a previously-green branch with no code
    /// change, which is what ruled out any particular slice as the cause.
    ///
    /// A passing test breaks out on the first iteration that observes its condition, so a large
    /// bound costs nothing when things work — it only buys headroom when the box is contended.
    const ASYNC_POLL_TICKS: usize = 3000;

    pub(crate) static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
    pub(crate) extern "C" fn logger(_l: c_int, m: *const c_char) {
        LOG.lock().unwrap().push(unsafe { CStr::from_ptr(m) }.to_string_lossy().into_owned());
    }

    // A no-op logger for tests that don't care about log output.
    extern "C" fn dummy_log_fn(_l: c_int, _m: *const c_char) {}
    /// Run `f` while the HOST borrow is held, simulating an outer dispatch already on the stack.
    ///
    /// Exists so a FEATURE module's re-entrancy test can exercise the `try_borrow_mut` graceful-skip
    /// without `HOST` itself becoming reachable outside `v8host` — the isolate handle is the one
    /// thing the extraction program deliberately never exposes.
    pub(crate) fn with_host_borrowed<R>(f: impl FnOnce() -> R) -> R {
        HOST.with(|h| { let _b = h.borrow_mut(); f() })
    }

    pub(crate) fn dummy_logger() -> LogFn { dummy_log_fn }

    /// L1 lifecycle v2: wrap `body` as a `plugin()` artifact whose (synchronous) factory body is
    /// `body`. `ctx` is in scope inside `body`. This is the new-shape bundle every test loads.
    fn def_js(body: &str) -> String {
        format!(
            "module.exports.default = {{ __s2plugin: 1, factory: function (ctx) {{ {} }} }};",
            body
        )
    }

    /// Load a plugin whose factory body is `body` (the common test path — a synchronous factory that
    /// reaches Active within the single `load_plugin_js` call via the sync fast-path).
    pub(crate) fn load_body(id: &str, body: &str, cfg: &str) {
        load_plugin_js(id, &def_js(body), cfg);
    }

    /// Set up a plugin context and run `body` in it directly (NO factory / no arm / no finalize) —
    /// for unit tests that exercise a native's side effects (e.g. `__s2_iface_publish` via
    /// `publishInterface`) and then assert on core state (`reconcile_publishes`, IFACES) decoupled
    /// from the load→arm→reconcile transition. The plugin stays in the `Loading` phase.
    fn eval_setup(id: &str, body: &str) {
        create_plugin_context(id);
        // Raw eval (unlike the CJS wrapper) has no `require` binding — inject one so bodies may
        // `require("@s2script/interfaces")` exactly as they would inside a plugin bundle.
        let full = format!("const require = globalThis.__s2_require;\n{}", body);
        eval_in_context(id, &full).expect("eval_setup body ran");
    }

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
    pub(crate) fn read_global_string(id: &str, name: &str) -> String {
        read_string_global_in(id, name)
    }

    // Read `globalThis[name]` as an i32 from a specific PLUGIN context (mirrors read_string_global_in).
    pub(crate) fn read_i32_global_in(id: &str, name: &str) -> i32 {
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
    pub(crate) fn read_bool_global_in(id: &str, name: &str) -> bool {
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
    // Returns the completion value of `src`'s last statement as a String (mirrors
    // `eval_in_context_string`) so callers can assert on a computed value (e.g. `JSON.stringify(...)`);
    // callers that only care about side effects may simply discard the return. Panics loudly (with the
    // JS exception message) on a compile or runtime error, same as the previous void-returning behavior.
    pub(crate) fn eval_std(id: &str, src: &str) -> String {
        create_plugin_context(id);
        let full = format!(
            "const {{ OnGameFrame }} = __s2require(\"@s2script/frame\");\nconst {{ delay, nextTick, nextFrame, threadSleep }} = __s2require(\"@s2script/timers\");\n{}",
            src
        );
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().expect("eval_std: no host");
            let g_ctx = PLUGINS
                .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
                .unwrap_or_else(|| panic!("eval_std: no context for '{}'", id));
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            let code = v8::String::new(tc, &full).expect("failed to intern");
            let script = match v8::Script::compile(tc, code, None) {
                Some(s) => s,
                None => panic!(
                    "eval_std compile failed: {}",
                    tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "unknown JavaScript error (compile)".into())
                ),
            };
            match script.run(tc) {
                Some(v) => v.to_rust_string_lossy(tc),
                None => panic!(
                    "eval_std run failed: {}",
                    tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "unknown JavaScript error (run)".into())
                ),
            }
        })
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

    /// A one-shot callback timer fires exactly once and then reports dead.
    #[test]
    fn timer_after_fires_once_then_is_dead() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__n = 0; globalThis.__t = __s2pkg_timers.after(20, () => { globalThis.__n++; });");
        frame_async_drain();
        assert_eq!(read_i32_global_in("p", "__n"), 0, "must not fire before its deadline");
        assert_eq!(eval_in_context_string("p", "String(__t.alive)"), "true");
        std::thread::sleep(std::time::Duration::from_millis(30));
        frame_async_drain();
        assert_eq!(read_i32_global_in("p", "__n"), 1);
        assert_eq!(eval_in_context_string("p", "String(__t.alive)"), "false", "one-shot is dead after firing");
        std::thread::sleep(std::time::Duration::from_millis(30));
        frame_async_drain();
        assert_eq!(read_i32_global_in("p", "__n"), 1, "a one-shot must never fire twice");
        shutdown();
    }

    /// The repeat re-arm: `every` keeps firing across drains. This is the invariant that separates
    /// it from `after` — without the re-arm in the drain it would fire once and silently stop.
    #[test]
    fn timer_every_rearms_across_drains() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__n = 0; globalThis.__t = __s2pkg_timers.every(10, () => { globalThis.__n++; });");
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame_async_drain();
        }
        assert!(read_i32_global_in("p", "__n") >= 3,
            "expected >= 3 fires, got {}", read_i32_global_in("p", "__n"));
        assert_eq!(eval_in_context_string("p", "String(__t.alive)"), "true", "a repeater stays alive");
        shutdown();
    }

    /// kill() stops a repeater, and is idempotent rather than an error.
    #[test]
    fn timer_kill_stops_a_repeater_and_is_idempotent() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__n = 0; globalThis.__t = __s2pkg_timers.every(10, () => { globalThis.__n++; });");
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame_async_drain();
        let after_one = read_i32_global_in("p", "__n");
        assert!(after_one >= 1);
        assert_eq!(eval_in_context_string("p", "String(__t.kill())"), "true");
        assert_eq!(eval_in_context_string("p", "String(__t.kill())"), "false", "second kill is false, not an error");
        assert_eq!(eval_in_context_string("p", "String(__t.alive)"), "false");
        std::thread::sleep(std::time::Duration::from_millis(30));
        frame_async_drain();
        assert_eq!(read_i32_global_in("p", "__n"), after_one, "no fires after kill");
        shutdown();
    }

    /// A callback that kills its OWN timer must not be re-armed. The drain takes the entry out
    /// while firing, so the self-kill has to be detected rather than overwritten by the re-arm.
    #[test]
    fn timer_callback_can_kill_itself() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__n = 0; globalThis.__t = __s2pkg_timers.every(10, () => { globalThis.__n++; __t.kill(); });");
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame_async_drain();
        }
        assert_eq!(read_i32_global_in("p", "__n"), 1, "self-kill must prevent the re-arm");
        shutdown();
    }

    /// THE teardown invariant: a repeating timer whose plugin unloads must stop. Without the
    /// ledger dropping TIMER_CBS it would re-arm forever and fire into a dead context.
    #[test]
    fn unload_kills_a_repeating_timer() {
        init(dummy_logger()).unwrap();
        eval_std("demo", "globalThis.__n = 0; __s2pkg_timers.every(10, () => { globalThis.__n++; });");
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame_async_drain();
        assert!(read_i32_global_in("demo", "__n") >= 1, "sanity: it fired at least once while loaded");
        unload_plugin("demo");
        // Nothing to assert in JS (the context is gone) — assert on the host books instead: no
        // callback and no queue entry may survive the unload.
        assert_eq!(TIMER_CBS.with(|m| m.borrow().len()), 0, "unload must drop the callback");
        assert_eq!(TIMERS.with(|t| t.borrow().len()), 0, "unload must drop the queue entry");
        shutdown();
    }

    /// A throwing callback is contained: it does not kill the timer system, and a repeater keeps
    /// going rather than silently dying on the first exception.
    #[test]
    fn throwing_timer_callback_is_contained() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__n = 0; __s2pkg_timers.every(10, () => { globalThis.__n++; throw new Error('boom'); });");
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame_async_drain();
        }
        assert!(read_i32_global_in("p", "__n") >= 3,
            "a throwing repeater must keep firing, got {}", read_i32_global_in("p", "__n"));
        shutdown();
    }

    /// A 0ms REPEAT would re-arm every drain and starve the frame, so it is refused loudly.
    /// A 0ms one-shot is fine (fire on the next drain).
    #[test]
    fn zero_interval_repeat_is_refused_but_zero_oneshot_is_fine() {
        init(dummy_logger()).unwrap();
        // eval_std creates the context; eval_in_context_string alone would panic with "no context".
        eval_std("p", "globalThis.__z = 0;");
        assert_eq!(eval_in_context_string("p",
            "(function(){ try { __s2pkg_timers.every(0, function(){}); return 'no-throw'; } catch (e) { return e.constructor.name; } })()"),
            "RangeError");
        eval_std("p", "__s2pkg_timers.after(0, () => { globalThis.__z = 1; });");
        frame_async_drain();
        assert_eq!(read_i32_global_in("p", "__z"), 1, "a 0ms one-shot fires on the next drain");
        shutdown();
    }

    /// A subscriber gets (name, new, old) for its own cvar, and a "*" subscriber sees every cvar.
    #[test]
    fn cvar_change_fans_out_to_exact_name_and_wildcard() {
        init(dummy_logger()).unwrap();
        eval_std("p", r#"
            globalThis.__seen = [];
            __s2pkg_server.Server.onCvarChange("mp_friendlyfire", (n, nv, ov) => { __seen.push("exact:"+n+":"+nv+":"+ov); });
            __s2pkg_server.Server.onCvarChange("*",               (n, nv, ov) => { __seen.push("star:"+n+":"+nv+":"+ov); });
        "#);
        let _ = dispatch_cvar_change("mp_friendlyfire", "1", "0");
        let _ = dispatch_cvar_change("sv_gravity", "600", "800");
        assert_eq!(eval_in_context_string("p", "__seen.join('|')"),
            "exact:mp_friendlyfire:1:0|star:mp_friendlyfire:1:0|star:sv_gravity:600:800");
        shutdown();
    }

    /// A handler that throws is contained: the other subscribers for the same change still run.
    #[test]
    fn throwing_cvar_handler_does_not_stop_the_others() {
        init(dummy_logger()).unwrap();
        eval_std("p", r#"
            globalThis.__n = 0;
            __s2pkg_server.Server.onCvarChange("*", () => { throw new Error("boom"); });
            __s2pkg_server.Server.onCvarChange("*", () => { globalThis.__n++; });
        "#);
        let _ = dispatch_cvar_change("sv_cheats", "1", "0");
        assert_eq!(read_i32_global_in("p", "__n"), 1);
        shutdown();
    }

    /// dispose() drops this plugin's subscriptions for that name.
    #[test]
    fn cvar_change_dispose_stops_delivery() {
        init(dummy_logger()).unwrap();
        eval_std("p", r#"
            globalThis.__n = 0;
            globalThis.__h = __s2pkg_server.Server.onCvarChange("sv_cheats", () => { globalThis.__n++; });
        "#);
        let _ = dispatch_cvar_change("sv_cheats", "1", "0");
        assert_eq!(read_i32_global_in("p", "__n"), 1);
        eval_in_context_string("p", "__h.dispose(); ''");
        let _ = dispatch_cvar_change("sv_cheats", "0", "1");
        assert_eq!(read_i32_global_in("p", "__n"), 1, "no delivery after dispose");
        shutdown();
    }

    /// THE teardown invariant: unload must drop the subscription, so a later change cannot
    /// dispatch into a dead context. The ledger is the authority, not the plugin's own cleanup.
    #[test]
    fn unload_drops_cvar_subscriptions() {
        init(dummy_logger()).unwrap();
        eval_std("demo", r#"__s2pkg_server.Server.onCvarChange("*", () => {});"#);
        assert!(CVAR_MUX.with(|m| !m.borrow().snapshot("*").is_empty()), "sanity: subscribed");
        unload_plugin("demo");
        assert!(CVAR_MUX.with(|m| m.borrow().snapshot("*").is_empty()),
            "unload must drop the subscription");
        let _ = dispatch_cvar_change("sv_cheats", "1", "0");   // must not panic into a dead context
        shutdown();
    }

    /// A non-function handler is refused loudly rather than silently never firing.
    #[test]
    fn cvar_change_rejects_a_non_function_handler() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__x = 0;");
        assert_eq!(eval_in_context_string("p",
            "(function(){ try { __s2pkg_server.Server.onCvarChange('a', 42); return 'no-throw'; } catch (e) { return e.constructor.name; } })()"),
            "TypeError");
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
        for _ in 0..ASYNC_POLL_TICKS {
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
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if read_bool_global_in("p", "__t") { resolved = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(resolved, "threadSleep promise never resolved on a drain");
        shutdown();
    }



    /// `load_plugin_js` creates the plugin context (full injected API), wraps the bundle in the CJS
    /// `require`/`module` wrapper, and runs the module body.  This replaces the Slice-3 `load_cs2_file`
    /// path (removed): the same "a loaded bundle's top-level code runs and its globals are visible"
    /// behavior, now under the per-plugin loader.  The body sets `globalThis.__loaded = 42`.
    #[test]
    fn load_plugin_js_runs_module_body() {
        init(dummy_logger()).unwrap();
        load_body("probe", "globalThis.__loaded = 41 + 1;", "{}");
        assert_eq!(read_i32_global_in("probe", "__loaded"), 42);
        shutdown();
    }

    /// The brief's acceptance test: a CJS bundle requires the injected API, subscribes in `onLoad`,
    /// and its handler runs once per frame — tagged to the CALLING plugin ("demo") in the ledger +
    /// the multiplexer owner.
    #[test]
    fn load_plugin_js_runs_onload_and_tags_subscription() {
        init(dummy_logger()).unwrap();
        // L1 lifecycle v2: the factory subscribes via ctx.server.onGameFrame (buffered until Active,
        // replayed at arm on the sync fast-path). Its handler runs once per frame, tagged to "demo".
        load_body("demo", r#"
            ctx.server.onGameFrame(function () { globalThis.__ticks = (globalThis.__ticks||0)+1; });
        "#, "{}");
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
        load_body("demo", r#"ctx.server.onGameFrame(function(){globalThis.__n=(globalThis.__n||0)+1;});"#, "{}");
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

    /// L1 lifecycle v2: unload_plugin captures a serializable state() return into PENDING_HANDOFF; a
    /// non-serializable return is dropped with a WARN (no entry); a throwing state() leaves no entry.
    #[test]
    fn unload_captures_state_return_as_handoff_blob() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // (a) serializable return → captured
        load_body("cap", r#"return { state: function(){ return { count: 7, name: "hi" }; } };"#, "{}");
        unload_plugin("cap");
        let blob = PENDING_HANDOFF.with(|h| h.borrow().get("cap").cloned());
        let blob = blob.expect("handoff blob captured");
        assert!(blob.contains("\"count\":7"), "blob has the state: {blob}");

        // (b) non-serializable return (a function) → no entry
        load_body("nos", r#"return { state: function(){ return function(){}; } };"#, "{}");
        unload_plugin("nos");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("nos").is_none()), "non-serializable → no blob");

        // (c) throwing state() → no entry
        load_body("thr", r#"return { state: function(){ throw new Error("boom"); } };"#, "{}");
        unload_plugin("thr");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("thr").is_none()), "throwing state() → no blob");
        shutdown();
    }

    /// L1 lifecycle v2: a same-id reload carries state — state()'s return revives into ctx.previous.
    /// Covers the primitive/nested round-trip, first-load undefined, and consume-once.
    #[test]
    fn reload_hands_off_state_to_ctx_previous() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // A plugin that seeds a counter from ctx.previous on load and bumps it in state() on unload.
        const JS: &str = r#"
            var count = 0;
            if (ctx.previous) { count = ctx.previous.count; }
            globalThis.__count = count;
            globalThis.__hadPrev = (ctx.previous !== undefined);
            return { state: function(){ return { count: count + 1 }; } };
        "#;
        // First load → ctx.previous undefined
        load_body("rh", JS, "{}");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__hadPrev)"), "false", "first load: no prev");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "0");
        // Reload: unload (captures {count:1}) then load again (consumes → ctx.previous)
        unload_plugin("rh");
        load_body("rh", JS, "{}");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__hadPrev)"), "true", "reload: prev present");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "1", "count carried across the reload");
        // Consume-once: the blob is gone, so a fresh load with no new unload sees undefined again.
        unload_plugin("rh");                                   // captures {count:2}
        load_body("rh", JS, "{}");                             // consumes → count=2
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "2");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("rh").is_none()), "blob consumed");
        shutdown();
    }

    /// L1 lifecycle v2: an EntityRef in the handoff state revives into a live, serial-gated EntityRef
    /// bound to the NEW context (reusing the inter-plugin reviver).
    #[test]
    fn reload_revives_entityref_in_state() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        const JS: &str = r#"
            globalThis.__revived = ctx.previous && ctx.previous.ref;
            return { state: function(){ return { ref: new (__s2pkg_entity.EntityRef)(1, 7) }; } };
        "#;
        load_body("er", JS, "{}");
        unload_plugin("er");                                   // captures { ref: <tagged EntityRef> }
        load_body("er", JS, "{}");                             // revives → live EntityRef
        assert_eq!(eval_in_context_string("er", "String(globalThis.__revived instanceof __s2pkg_entity.EntityRef)"), "true");
        assert_eq!(eval_in_context_string("er", "globalThis.__revived.index + ',' + globalThis.__revived.id"), "1,7");
        shutdown();
    }

    /// L1 lifecycle v2: a factory that throws when it sees ctx.previous → Failed (fail loud), but the
    /// handoff blob was already consumed by ctx.previous (__s2_handoff_take) before the throw.
    #[test]
    fn reload_factory_throw_consumes_handoff_no_crash() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        const JS: &str = r#"
            if (ctx.previous) throw new Error("boom");
            return { state: function(){ return { x: 1 }; } };
        "#;
        load_body("ot", JS, "{}");                             // first load: no prev → Active, state set
        unload_plugin("ot");                                   // captures {x:1}
        load_body("ot", JS, "{}");                             // ctx.previous={x:1} consumed, then throws → Failed
        // No panic; the blob was consumed despite the throw; the plugin failed (not running).
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("ot").is_none()), "blob consumed even though factory threw");
        assert!(FAILED_PLUGINS.with(|f| f.borrow().contains_key("ot")), "throwing reload factory → Failed");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("ot")), "failed load's context disposed");
        shutdown();
    }

    /// Brief test: a `delay` continuation whose plugin is UNLOADED before the deadline must be
    /// DROPPED — `frame_async_drain` must NOT run the continuation into a disposed context (no
    /// panic; the resolver was dropped by the ledger teardown).
    #[test]
    fn delay_continuation_for_unloaded_plugin_is_dropped() {
        init(dummy_logger()).unwrap();
        load_body("demo", r#"const {delay}=require("@s2script/timers");
            (async()=>{ await delay(30); globalThis.__resumed=true; })();"#, "{}");
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
    /// `load_body("demo", v2_js)` is called while "demo" is still in PLUGINS, it detects
    /// the existing instance, calls `unload_plugin("demo")` first (teardown: removes the v1
    /// handler, disposes the context), then loads v2 in a fresh context.
    #[test]
    fn reload_tears_down_old_and_runs_new_handler() {
        init(dummy_logger()).unwrap();

        // v1: subscribes an OnGameFrame handler that writes "v1" to a global.
        let v1_js = r#"ctx.server.onGameFrame(function () { globalThis.__v = "v1"; });"#;
        load_body("demo", v1_js, "{}");
        dispatch_game_frame_pre_post();
        assert_eq!(read_string_global_in("demo", "__v"), "v1", "v1 handler ran before reload");

        // Capture the v1 generation so we can assert it becomes stale after reload.
        let old_gen = PLUGINS
            .with(|p| p.borrow().get("demo").expect("demo loaded").generation);

        // RELOAD: call load_body with the same id — the defensive guard fires. v2 writes "v2".
        let v2_js = r#"ctx.server.onGameFrame(function () { globalThis.__v = "v2"; });"#;
        load_body("demo", v2_js, "{}");

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

    // === L1 lifecycle v2: the phase machine + typed-artifact load path (design spec §5) ===

    /// A synchronous factory reaches Active within the single `load_plugin_js` call (the sync
    /// fast-path), and its (buffered) registration is armed by then.
    #[test]
    fn sync_factory_reaches_active_in_one_call() {
        init(dummy_logger()).unwrap();
        load_body("s", r#"ctx.events.on('round_start', function(){});"#, "{}");
        assert_eq!(plugin_phase("s"), Some(crate::plugin::Phase::Active));
        assert_eq!(crate::events::subscriber_count("round_start"), 1);
        shutdown();
    }

    /// An ASYNC factory stays Loading until its promise settles; its ctx.events.on is BUFFERED (not
    /// armed) until the Active transition on a later drain.
    #[test]
    fn buffered_registration_does_not_arm_before_active() {
        init(dummy_logger()).unwrap();
        load_body("ap", r#"
            ctx.events.on('round_start', function(){});
            return new Promise(function(res){ globalThis.__resolve = res; });
        "#, "{}");
        // Still Loading; the buffered registration has NOT reached EVENT_MUX yet.
        assert_eq!(plugin_phase("ap"), Some(crate::plugin::Phase::Loading));
        assert_eq!(crate::events::subscriber_count("round_start"), 0);
        // Resolve the factory promise, then drain: the .then runs (__s2_load_settled), finalize arms.
        let _ = eval_in_context("ap", "globalThis.__resolve();");
        frame_async_drain();
        assert_eq!(plugin_phase("ap"), Some(crate::plugin::Phase::Active));
        assert_eq!(crate::events::subscriber_count("round_start"), 1);
        shutdown();
    }

    /// A factory that throws fails LOUD — no zombie: reason recorded in FAILED_PLUGINS, no PLUGINS
    /// entry (context disposed), and the buffered sub never armed (EVENT_MUX empty).
    #[test]
    fn throwing_factory_fails_loud_no_zombie() {
        init(dummy_logger()).unwrap();
        load_body("tf", r#"
            ctx.events.on('round_start', function(){});
            throw new Error("boom-factory");
        "#, "{}");
        assert!(FAILED_PLUGINS.with(|f| f.borrow().get("tf").map(|r| r.contains("boom-factory")).unwrap_or(false)),
            "failed reason recorded");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("tf")), "context disposed — no zombie");
        assert_eq!(crate::events::subscriber_count("round_start"), 0, "buffered sub never armed");
        assert!(plugin_phase("tf").is_none());
        shutdown();
    }

    /// A factory whose promise rejects → Failed, reason carries the rejection message.
    #[test]
    fn async_rejection_fails() {
        init(dummy_logger()).unwrap();
        load_body("ar", r#"return Promise.reject(new Error("nope-async"));"#, "{}");
        assert_eq!(plugin_phase("ar"), Some(crate::plugin::Phase::Loading));
        frame_async_drain();
        assert!(FAILED_PLUGINS.with(|f| f.borrow().get("ar").map(|r| r.contains("nope-async")).unwrap_or(false)),
            "rejection reason recorded");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("ar")));
        shutdown();
    }

    /// A legacy `export onLoad` bundle (no plugin() default) is REFUSED with a named reason.
    #[test]
    fn legacy_shape_refused() {
        init(dummy_logger()).unwrap();
        load_plugin_js("legacy", "module.exports.onLoad=()=>{};", "{}");
        assert!(FAILED_PLUGINS.with(|f| f.borrow().get("legacy").map(|r| r.contains("legacy plugin shape")).unwrap_or(false)),
            "legacy shape refused with a named reason");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("legacy")));
        shutdown();
    }

    /// After Active the ctx is SEALED: a leaked reference used to register outside the load window
    /// throws (never silently registers).
    #[test]
    fn sealed_ctx_throws() {
        init(dummy_logger()).unwrap();
        load_body("seal", r#"globalThis.LEAK = ctx;"#, "{}");
        assert_eq!(plugin_phase("seal"), Some(crate::plugin::Phase::Active));
        let _ = eval_in_context("seal",
            "try { LEAK.events.on('x', function(){}); globalThis.T='no' } catch(e) { globalThis.T='threw' }");
        assert_eq!(eval_in_context_string("seal", "String(globalThis.T)"), "threw");
        shutdown();
    }

    /// A factory whose promise never settles → Failed once past LOAD_TIMEOUT_FRAMES.
    #[test]
    fn load_timeout_fails() {
        init(dummy_logger()).unwrap();
        load_body("to", r#"return new Promise(function(){});"#, "{}");
        assert_eq!(plugin_phase("to"), Some(crate::plugin::Phase::Loading));
        // Advance the frame counter past the timeout, then finalize.
        FRAME_COUNTER.with(|c| c.set(LOAD_TIMEOUT_FRAMES + 5));
        finalize_loading_plugins();
        assert!(FAILED_PLUGINS.with(|f| f.borrow().get("to").map(|r| r.contains("did not settle")).unwrap_or(false)),
            "timeout reason recorded");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("to")));
        shutdown();
    }

    /// state() → ctx.previous round-trips a value across a same-id reload.
    #[test]
    fn state_and_previous_roundtrip() {
        init(dummy_logger()).unwrap();
        load_body("rt", r#"
            globalThis.__seen = ctx.previous ? ctx.previous.n : -1;
            return { state: function(){ return { n: 7 }; } };
        "#, "{}");
        assert_eq!(eval_in_context_string("rt", "String(globalThis.__seen)"), "-1", "first load: no previous");
        unload_plugin("rt");
        load_body("rt", r#"
            globalThis.__seen = ctx.previous ? ctx.previous.n : -1;
            return { state: function(){ return { n: 7 }; } };
        "#, "{}");
        assert_eq!(eval_in_context_string("rt", "String(globalThis.__seen)"), "7", "reload sees state()'s n via ctx.previous");
        shutdown();
    }

    /// Unloading a plugin still Loading seals its ctx, drops the LOADING entry, and walks its PARTIAL
    /// ledger (the delay timer it acquired) — no state() capture (it was never Active).
    #[test]
    fn unload_while_loading_seals_and_walks_partial_ledger() {
        init(dummy_logger()).unwrap();
        load_body("ul", r#"
            const {delay}=require("@s2script/timers");
            delay(10);
            return new Promise(function(){});
        "#, "{}");
        assert_eq!(plugin_phase("ul"), Some(crate::plugin::Phase::Loading));
        assert!(!RESOLVERS.with(|m| m.borrow().is_empty()), "the delay timer's resolver is ledgered");
        unload_plugin("ul");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("ul")), "context disposed");
        assert!(RESOLVERS.with(|m| m.borrow().is_empty()), "partial-ledger walk dropped the delay resolver");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("ul").is_none()), "never Active → no state() capture");
        assert!(LOADING.with(|l| l.borrow().get("ul").is_none()), "LOADING entry dropped");
        shutdown();
    }

    /// L1 Task 3: `scope.clear()` disposes ONLY the scope's subscriptions (via the sub ids the
    /// subscribe natives now return), leaving the plugin-lifetime `ctx` subs intact. Both a ctx sub
    /// and a scope sub fire before `clear()`; after `clear()` only the ctx sub fires and EVENT_MUX
    /// keeps exactly the one ctx row.
    #[test]
    fn scope_clear_removes_only_scope_subs() {
        init(dummy_logger()).unwrap();
        load_body("sc", r#"
            ctx.events.on('round_start', function () { globalThis.PLUGIN_HITS = (globalThis.PLUGIN_HITS|0) + 1; });
            var s = ctx.createScope();
            s.events.on('round_start', function () { globalThis.SCOPE_HITS = (globalThis.SCOPE_HITS|0) + 1; });
            globalThis.S = s;
        "#, "{}");
        assert_eq!(plugin_phase("sc"), Some(crate::plugin::Phase::Active));
        assert_eq!(crate::events::subscriber_count("round_start"), 2, "ctx + scope subs both registered");

        let _ = dispatch_game_event("round_start");
        assert_eq!(eval_in_context_string("sc", "String(globalThis.PLUGIN_HITS|0)"), "1");
        assert_eq!(eval_in_context_string("sc", "String(globalThis.SCOPE_HITS|0)"), "1");

        // Dispose the scope's subs by id; the ctx sub survives.
        let _ = eval_in_context("sc", "globalThis.S.clear();");
        assert_eq!(crate::events::subscriber_count("round_start"), 1, "only the ctx sub remains");

        let _ = dispatch_game_event("round_start");
        assert_eq!(eval_in_context_string("sc", "String(globalThis.PLUGIN_HITS|0)"), "2", "ctx sub still fires");
        assert_eq!(eval_in_context_string("sc", "String(globalThis.SCOPE_HITS|0)"), "1", "scope sub gone after clear()");
        shutdown();
    }

    // Evaluate `src` in a named plugin context and return the result via `coerce`.
    // Mirrors the borrow discipline of `load_plugin_js`: clone the Global<Context> out of PLUGINS
    // before opening the HandleScope on HOST.isolate, run under a TryCatch.
    pub(crate) fn eval_in_context_string(id: &str, src: &str) -> String {
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
    fn s2require_dual_resolves_sdk_and_legacy_prefixes() {
        let _ = init(dummy_logger());
        create_plugin_context("dualpfx");
        // Both prefixes resolve the SAME capability global.
        assert!(eval_in_context_bool("dualpfx",
            r#"__s2require("@s2script/sdk/math") === __s2require("@s2script/math")"#),
            "@s2script/sdk/math must resolve to the same object as @s2script/math");
        assert!(eval_in_context_bool("dualpfx",
            r#"typeof __s2require("@s2script/sdk/entity").EntityRef === "function""#),
            "@s2script/sdk/entity must expose EntityRef");
        // Bare `@s2script/sdk` (no capability — the rejected flat barrel) → null at runtime:
        // it falls through to the plain `@s2script/` strip → `__s2pkg_sdk`, which never exists.
        assert!(eval_in_context_bool("dualpfx",
            r#"__s2require("@s2script/sdk") === null"#),
            "bare @s2script/sdk must resolve to null");
        // A non-s2script specifier is still null (handled by the JS interop shim).
        assert!(eval_in_context_bool("dualpfx",
            r#"__s2require("@other/x") === null"#));
        shutdown();
    }

    /// Plugin-declared engine calls, JS half: the `@s2script/sdk/unsafe` prelude compiles, resolves
    /// under both specifier spellings, and its three natives are registered. An UNDECLARED call is
    /// the default state for every plugin, so `call()` must be `null` (never a throw, never a
    /// callable that would reach the engine) and `status()` must NAME why.
    #[test]
    fn unsafe_module_exposes_engine_call_status_and_degrades_to_null() {
        let _ = init(dummy_logger());
        create_plugin_context("unsafepfx");
        assert!(eval_in_context_bool("unsafepfx",
            r#"typeof __s2require("@s2script/sdk/unsafe").Engine.call === "function""#),
            "@s2script/sdk/unsafe must expose Engine.call (the prelude compiled)");
        assert!(eval_in_context_bool("unsafepfx",
            r#"__s2require("@s2script/sdk/unsafe").Engine.call("nope") === null"#),
            "an undeclared call must yield null, not a callable");
        assert_eq!(
            eval_in_context_string("unsafepfx",
                r#"__s2require("@s2script/sdk/unsafe").Engine.status("nope")"#),
            "not declared in this plugin's gamedata");
        // The natives themselves: ready is false and invoke no-ops to null for an unknown descriptor.
        // They take the call name ONLY — the plugin id is the calling context's.
        assert!(eval_in_context_bool("unsafepfx",
            r#"__s2_engine_call_ready("nope") === false"#));
        assert!(eval_in_context_bool("unsafepfx",
            r#"__s2_engine_call_invoke("nope", 1, 1, []) === null"#));
        shutdown();
    }

    /// A plugin must never be able to act AS another plugin. `gamedata_calls` gates the
    /// `engine:calls` permission once, at registration (`prepare`), so a descriptor's mere
    /// existence in the registry IS its authorization — which means the plugin id these natives
    /// key on decides who may drive an operator-allow-listed engine call. It must therefore come
    /// from the CALLING CONTEXT (`current_plugin`), never from a string the caller chose: the raw
    /// `__s2_engine_call_*` natives sit on every plugin's global object.
    #[test]
    fn engine_call_natives_ignore_a_caller_supplied_plugin_id() {
        let _ = init(dummy_logger());
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["gd_victim"]}"#).unwrap();
        create_plugin_context("gd_victim");
        create_plugin_context("gd_attacker");
        // The victim declares a call. With no engine ops under test it registers Degraded — still a
        // REGISTERED descriptor, which is all this test needs to tell "someone else's" apart from
        // "not declared".
        crate::gamedata_calls::register_plugin(
            "gd_victim",
            r#"{"calls":{"SecretCall":{"receiver":{"kind":"entity"},
                "target":{"kind":"signature","name":"Ig","module":"libserver.so",
                          "pattern":"55 48","resolve":"direct"},
                "args":["float"],"returns":"void"}}}"#,
        );
        // Precondition, asserted Rust-side so it does not depend on the natives' arity: the
        // descriptor really is registered. Without this the attack assertion could pass vacuously.
        assert_ne!(
            crate::gamedata_calls::status("gd_victim", "SecretCall"),
            "not declared in this plugin's gamedata",
            "precondition: the victim's descriptor must be registered"
        );
        // The attack: the attacker names the victim as the plugin id. The natives take the call
        // name ONLY, so this reads as a call named "gd_victim" in the ATTACKER's own registry —
        // which does not exist.
        assert_eq!(
            eval_in_context_string(
                "gd_attacker",
                r#"__s2_engine_call_status("gd_victim", "SecretCall")"#
            ),
            "not declared in this plugin's gamedata",
            "a caller-supplied plugin id must not select another plugin's descriptor"
        );
        shutdown();
    }

    thread_local! {
        /// (call_id, first FP arg) recorded by `fake_call_invoke`.
        static LAST_ENGINE_CALL: std::cell::Cell<(c_int, f64)> =
            const { std::cell::Cell::new((-1, 0.0)) };
    }

    extern "C" fn fake_call_resolve(
        _kind: *const c_char, _module: *const c_char, _pattern: *const c_char,
        _resolve: *const c_char, _class_name: *const c_char, _vtable_index: c_int,
        _validate_json: *const c_char, _reason_out: *mut c_char, _reason_cap: c_int,
    ) -> c_int { 7 }

    extern "C" fn fake_call_invoke(
        call_id: c_int, _ent_index: c_int, _ent_serial: c_int, _subobj_off: c_int,
        _gp: *const u64, _gp_kind: *const u8, _gp_count: c_int,
        fp: *const f64, fp_count: c_int,
        _strs: *const *const c_char, _vecs: *const f32,
        _ret_kind: c_int, _ret_out: *mut u64,
    ) -> c_int {
        let f0 = if fp_count > 0 && !fp.is_null() { unsafe { *fp } } else { 0.0 };
        LAST_ENGINE_CALL.with(|c| c.set((call_id, f0)));
        1
    }

    /// The legitimate path, which dropping the leading `pluginId` argument could have broken
    /// silently: an AUTHORIZED plugin asking for its OWN descriptor still gets a working callable,
    /// and the receiver/arg slots still land where the shim expects after every following argument
    /// shifted down one. A RECEIVERLESS descriptor is used so no live entity is needed — the arg
    /// array is the slot that moved (`args.get(4)` -> `args.get(3)`).
    ///
    /// Without an `engine_call_resolve` op no descriptor ever reaches `Ready`, so nothing else in
    /// the suite covers a successful `Engine.call` at all.
    #[test]
    fn engine_call_still_invokes_for_the_owning_plugin() {
        let _ = init(dummy_logger());
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["gd_owner"]}"#).unwrap();
        set_engine_ops(Some(S2EngineOps {
            engine_call_resolve: Some(fake_call_resolve),
            engine_call_invoke: Some(fake_call_invoke),
            ..mock_event_ops()
        }));
        create_plugin_context("gd_owner");
        crate::gamedata_calls::register_plugin(
            "gd_owner",
            r#"{"calls":{"Boom":{"receiver":{"kind":"none"},
                "target":{"kind":"signature","name":"Bo","module":"libserver.so",
                          "pattern":"55 48","resolve":"direct"},
                "args":["float"],"returns":"void"}}}"#,
        );
        assert_eq!(
            crate::gamedata_calls::status("gd_owner", "Boom"), "available",
            "precondition: the descriptor must resolve through the fake op"
        );
        // The owning plugin gets a callable — and never names itself to get one.
        assert!(eval_in_context_bool("gd_owner",
            r#"typeof __s2require("@s2script/sdk/unsafe").Engine.call("Boom") === "function""#),
            "an authorized plugin's own declared call must still be callable");
        LAST_ENGINE_CALL.with(|c| c.set((-1, 0.0)));
        assert!(eval_in_context_bool("gd_owner",
            r#"(__s2require("@s2script/sdk/unsafe").Engine.call("Boom")(2.5), true)"#));
        let (call_id, fp0) = LAST_ENGINE_CALL.with(|c| c.get());
        assert_eq!(call_id, 7, "the resolved call id must reach the engine op");
        assert_eq!(fp0, 2.5, "the declared float arg must survive the dropped pluginId slot");
        set_engine_ops(None);
        shutdown();
    }

    /// A5b: the GAME PACKAGE's descriptor path, end to end through the natives pawn.js uses.
    ///
    /// The shim hands core the merged gamedata for the `cs2` owner (here: the same JSON text
    /// `GameConfig::mergedJson` produces, in the shape `gamedata/cs2/game.cs2.jsonc` will carry);
    /// core registers it under the reserved owner id; and every plugin context's
    /// `__s2_game_call_*` natives report `ready` / `status` for it — WITHOUT the calling plugin
    /// appearing in the `engine:calls` allow-list, and without the plugin's own registry being
    /// touched.
    #[test]
    fn game_package_declared_calls_are_ready_through_the_game_scoped_natives() {
        let _ = init(dummy_logger());
        // Nobody is allow-listed. The game package is runtime, not a plugin — it must not need one.
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        set_engine_ops(Some(S2EngineOps {
            engine_call_resolve: Some(fake_call_resolve),
            engine_call_invoke: Some(fake_call_invoke),
            ..mock_event_ops()
        }));
        crate::gamedata_calls::register_game_package(
            "@demo/gamepkg",
            r#"{"signatures":{"DoThing":{"linuxsteamrt64":{"module":"libserver.so",
                 "pattern":"55 48","resolve":"direct"}}},
                "calls":{"doThing":{"receiver":{"kind":"none"},
                 "target":{"kind":"signature","name":"DoThing"},
                 "args":["float"],"returns":"void"}}}"#,
        );
        create_plugin_context("gd_consumer");
        assert!(
            eval_in_context_bool("gd_consumer", r#"__s2_game_call_ready("doThing") === true"#),
            "a game-package descriptor must be ready in any plugin context"
        );
        assert_eq!(
            eval_in_context_string("gd_consumer", r#"__s2_game_call_status("doThing")"#),
            "available",
            "and report its status through the game-scoped native"
        );
        // The owner really is separate: the SAME name asked through the plugin-scoped native is
        // not declared, so the game package's descriptors never merge into a plugin's namespace.
        assert_eq!(
            eval_in_context_string("gd_consumer", r#"__s2_engine_call_status("doThing")"#),
            "not declared in this plugin's gamedata",
            "the game package's descriptors are its own, not the calling plugin's"
        );
        // And the call actually reaches the engine op through the game-scoped invoke.
        LAST_ENGINE_CALL.with(|c| c.set((-1, 0.0)));
        assert!(eval_in_context_bool(
            "gd_consumer",
            r#"(__s2_game_call_invoke("doThing", 0, 0, [1.5]), true)"#
        ));
        let (call_id, fp0) = LAST_ENGINE_CALL.with(|c| c.get());
        assert_eq!(call_id, 7, "the resolved call id must reach the engine op");
        assert_eq!(fp0, 1.5, "the declared float arg must survive the game-scoped marshaller");
        crate::gamedata_calls::drop_plugin(&crate::gamedata_calls::reserved_owner_id("@demo/gamepkg"));
        set_engine_ops(None);
        shutdown();
    }

    /// `shutdown()` must tear every registered owner-scoped store down by SWEEPING the registry, not
    /// by a hand-written line per store. The cascade this replaces had to be extended by hand for
    /// every new capability slice, and silently kept stale state on the ones where that was
    /// forgotten — three shipped fixes of exactly that shape (98cf483, e40492d, 7e62119). A store is
    /// now torn down because it is REGISTERED, not because someone remembered to add a line.
    ///
    /// The probe registers after `init()` (which calls `register_builtin_stores`, itself starting
    /// with `owner_stores::reset()`), so it is still in the registry when `shutdown()` runs.
    #[test]
    fn shutdown_sweeps_every_registered_owner_store() {
        thread_local! {
            static PROBE_RESET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        let _ = init(dummy_logger());
        PROBE_RESET.with(|c| c.set(false));
        crate::owner_stores::register(
            "TEST_PROBE",
            Box::new(|_| {}),
            Box::new(|_| {}),
            Box::new(|| PROBE_RESET.with(|c| c.set(true))),
        );
        shutdown();
        assert!(
            PROBE_RESET.with(|c| c.get()),
            "shutdown() must run every registered store's reset closure"
        );
    }

    /// The phase contract, asserted directly: `BeforeIsolateDrop` resets run while the isolate is
    /// still alive (which is the entire reason the phase exists — a `v8::Global` must be released
    /// before its isolate goes away), and `AfterIsolateDrop` resets run once it is gone.
    ///
    /// Each probe records `HOST.is_some()` at the moment it runs, so this fails if either the
    /// `reset_all` calls move to the wrong side of `HOST.take()` or a phase is dropped entirely.
    #[test]
    fn shutdown_resets_process_singletons_on_both_sides_of_the_isolate_drop() {
        thread_local! {
            static SEEN: std::cell::RefCell<Vec<(&'static str, bool)>> =
                const { std::cell::RefCell::new(Vec::new()) };
        }
        let _ = init(dummy_logger());
        SEEN.with(|s| s.borrow_mut().clear());
        // Registered after init() (which calls register_process_singletons, itself starting with
        // process_singletons::reset()), so these probes survive to shutdown.
        crate::process_singletons::register(
            "TEST_BEFORE",
            crate::process_singletons::ResetPhase::BeforeIsolateDrop,
            Box::new(|| {
                let isolate_alive = HOST.with(|h| h.borrow().is_some());
                SEEN.with(|s| s.borrow_mut().push(("before", isolate_alive)));
            }),
        );
        crate::process_singletons::register(
            "TEST_AFTER",
            crate::process_singletons::ResetPhase::AfterIsolateDrop,
            Box::new(|| {
                let isolate_alive = HOST.with(|h| h.borrow().is_some());
                SEEN.with(|s| s.borrow_mut().push(("after", isolate_alive)));
            }),
        );
        shutdown();
        SEEN.with(|s| {
            assert_eq!(
                *s.borrow(),
                vec![("before", true), ("after", false)],
                "Before must run with the isolate alive; After must run once it is gone"
            )
        });
    }

    /// `register_process_singletons` is 36 near-identical hand-written lines, and the mistake that
    /// shape invites is a copy-paste that registers one static twice and its sibling not at all
    /// (`ADMIN_FILE` twice, `ADMIN_RUNTIME` never) — which reads fine and silently drops a clear,
    /// the exact bug class this registry exists to end. A duplicate NAME is that mistake's
    /// fingerprint. Also asserts both phases are populated, so deleting a whole phase's worth of
    /// registrations cannot pass quietly.
    #[test]
    fn process_singleton_registrations_are_unique_and_cover_both_phases() {
        use crate::process_singletons::ResetPhase;
        let _ = init(dummy_logger());
        let names = crate::process_singletons::registered_names();

        let mut seen = std::collections::HashSet::new();
        let dupes: Vec<&str> = names
            .iter()
            .filter(|(n, _)| !seen.insert(*n))
            .map(|(n, _)| *n)
            .collect();
        assert!(dupes.is_empty(), "duplicate singleton registrations: {dupes:?}");

        for phase in [ResetPhase::BeforeIsolateDrop, ResetPhase::AfterIsolateDrop] {
            assert!(
                names.iter().any(|(_, p)| *p == phase),
                "no singletons registered for {phase:?}"
            );
        }
        shutdown();
    }

    #[test]
    fn iface_publish_records_methods_and_dep_kind() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        create_plugin_context("prod");
        create_plugin_context("cons");

        // Producer publishes.
        eval_in_context("prod", r#"__s2_iface_publish("@x/greeter",{ greet:function(n){return "hi "+n;} });"#).expect("publish");
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

    #[test]
    fn publish_interface_takes_its_version_from_the_manifest() {
        let _ = init(dummy_logger());
        // The manifest declares the contract; the plugin never types a version.
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "2.5.0".into(), types_sha256: "abc".into() },
        )].into_iter().collect());
        create_plugin_context("prod");
        eval_in_context("prod", r#"__s2_iface_publish("@x/greeter",{ greet:function(){return "hi";} });"#)
            .expect("publish");
        let v = IFACES.with(|r| r.borrow().lookup("@x/greeter").map(|e| e.version.clone()));
        assert_eq!(v, Some("2.5.0".to_string()), "version must come from the manifest, not JS");
        shutdown();
    }

    #[test]
    fn publish_interface_of_an_undeclared_name_is_refused() {
        let _ = init(dummy_logger());
        set_plugin_publishes("prod", std::collections::HashMap::new());
        create_plugin_context("prod");
        // Publishing a name absent from the manifest must NOT register anything.
        let _ = eval_in_context("prod", r#"__s2_iface_publish("@x/undeclared",{ a:function(){} });"#);
        let found = IFACES.with(|r| r.borrow().lookup("@x/undeclared").is_some());
        assert!(!found, "an undeclared interface must never reach the registry");
        shutdown();
    }

    // --- Post-load publishes reconciliation (design spec §4.3). ---
    // An undeclared publish is refused at publish time (above), but that alone lets a TYPO load
    // green: the manifest declares "@x/greeter", the code publishes "@x/greetr", nothing registers,
    // and consumers just see InterfaceUnavailable. Reconciling AFTER the load catches it, and the
    // loader turns a mismatch into a real teardown — the spec's "fails the load".

    #[test]
    fn reconcile_publishes_ok_when_every_declared_interface_was_published() {
        let _ = init(dummy_logger());
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() },
        )].into_iter().collect());
        eval_setup("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/greeter", { greet: function () { return "hi"; } });
        "#);
        assert!(reconcile_publishes("prod").is_ok());
        shutdown();
    }

    #[test]
    fn reconcile_publishes_reports_a_declared_interface_the_plugin_never_published() {
        let _ = init(dummy_logger());
        // The typo case: manifest says @x/greeter, the code publishes @x/greetr.
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() },
        )].into_iter().collect());
        eval_setup("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/greetr", { greet: function () { return "hi"; } });
        "#);
        let err = reconcile_publishes("prod").expect_err("a declared-but-unpublished name must fail");
        // A typo trips BOTH directions, and the pair is the diagnosis: you typed @x/greetr,
        // you declared @x/greeter. The message must name both, not just whichever is checked first.
        assert!(err.contains("@x/greetr"), "error names what was published: {}", err);
        assert!(err.contains("@x/greeter"), "error names what was declared: {}", err);
        shutdown();
    }

    #[test]
    fn reconcile_publishes_ok_for_a_plugin_that_declares_nothing() {
        let _ = init(dummy_logger());
        set_plugin_publishes("plain", std::collections::HashMap::new());
        eval_setup("plain", "");
        assert!(reconcile_publishes("plain").is_ok(), "publishing nothing is not a mismatch");
        shutdown();
    }

    #[test]
    fn reconcile_publishes_fails_a_plugin_that_declares_nothing_but_publishes_anyway() {
        let _ = init(dummy_logger());
        // The forgot-the-manifest case. Nothing is declared, so the declared→owned check has
        // nothing to say — without the undeclared-attempt record this plugin would run on with
        // its interface silently unpublished, and consumers would meet InterfaceUnavailable.
        set_plugin_publishes("forgetful", std::collections::HashMap::new());
        eval_setup("forgetful", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/forgotten", { a: function () { return 1; } });
        "#);
        let err = reconcile_publishes("forgetful").expect_err("an undeclared publish must fail the load");
        assert!(err.contains("@x/forgotten"), "error names the interface: {}", err);
        assert!(!IFACES.with(|r| r.borrow().lookup("@x/forgotten").is_some()));
        shutdown();
    }

    #[test]
    fn undeclared_publish_record_is_cleared_on_unload() {
        let _ = init(dummy_logger());
        set_plugin_publishes("retry", std::collections::HashMap::new());
        eval_setup("retry", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/oops", { a: function () { return 1; } });
        "#);
        assert!(reconcile_publishes("retry").is_err());
        unload_plugin("retry");
        // A fixed reload must not inherit the previous attempt's failure.
        set_plugin_publishes("retry", [(
            "@x/oops".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() },
        )].into_iter().collect());
        eval_setup("retry", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/oops", { a: function () { return 1; } });
        "#);
        assert!(reconcile_publishes("retry").is_ok(), "the stale undeclared record must not persist");
        shutdown();
    }

    #[test]
    fn shutdown_clears_the_publishes_registries() {
        let _ = init(dummy_logger());
        // Populate PLUGIN_PUBLISHES via a `set` with no matching load (so no per-plugin unload ever
        // clears it), and UNDECLARED_PUBLISHES via a plugin that publishes an interface it never
        // declared. Both thread_locals must be non-empty going into shutdown.
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() },
        )].into_iter().collect());
        set_plugin_publishes("forgetful", std::collections::HashMap::new());
        eval_setup("forgetful", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/undeclared", { a: function () { return 1; } });
        "#);
        assert!(!PLUGIN_PUBLISHES.with(|p| p.borrow().is_empty()),
            "precondition: PLUGIN_PUBLISHES populated");
        assert!(!UNDECLARED_PUBLISHES.with(|p| p.borrow().is_empty()),
            "precondition: UNDECLARED_PUBLISHES populated");

        shutdown();

        assert!(PLUGIN_PUBLISHES.with(|p| p.borrow().is_empty()),
            "shutdown must clear PLUGIN_PUBLISHES");
        assert!(UNDECLARED_PUBLISHES.with(|p| p.borrow().is_empty()),
            "shutdown must clear UNDECLARED_PUBLISHES");
    }

    #[test]
    fn reconcile_publishes_rejects_a_name_published_by_a_DIFFERENT_producer() {
        let _ = init(dummy_logger());
        let decl = crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() };
        set_plugin_publishes("first", [("@x/dup".to_string(), decl.clone())].into_iter().collect());
        set_plugin_publishes("second", [("@x/dup".to_string(), decl)].into_iter().collect());
        eval_setup("first", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/dup", { a: function () { return 1; } });
        "#);
        // `second`'s publish is refused (the incumbent holds the name), so reconciliation must
        // fail it rather than let it run as a live plugin whose declared interface isn't its own.
        eval_setup("second", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/dup", { a: function () { return 2; } });
        "#);
        assert!(reconcile_publishes("first").is_ok(), "the incumbent is consistent");
        let err = reconcile_publishes("second").expect_err("the loser must fail its load");
        assert!(err.contains("@x/dup"), "error names the interface: {}", err);
        shutdown();
    }

    #[test]
    fn publish_interface_of_a_name_owned_by_another_producer_is_refused() {
        let _ = init(dummy_logger());
        let decl = crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() };
        set_plugin_publishes("first", [("@x/dup".to_string(), decl.clone())].into_iter().collect());
        set_plugin_publishes("second", [("@x/dup".to_string(), decl)].into_iter().collect());
        create_plugin_context("first");
        create_plugin_context("second");
        eval_in_context("first", r#"__s2_iface_publish("@x/dup",{ a:function(){return 1;} });"#).expect("first");
        let _ = eval_in_context("second", r#"__s2_iface_publish("@x/dup",{ a:function(){return 2;} });"#);
        let owner = IFACES.with(|r| r.borrow().lookup("@x/dup").map(|e| e.producer_id.clone()));
        assert_eq!(owner, Some("first".to_string()), "the incumbent producer must keep the name");
        shutdown();
    }

    #[test]
    fn prelude_publish_interface_takes_two_args() {
        let _ = init(dummy_logger());
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.4.0".into(), types_sha256: "abc".into() },
        )].into_iter().collect());
        eval_setup("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/greeter", { greet: function (n) { return "hi " + n.who; } });
        "#);
        let v = IFACES.with(|r| r.borrow().lookup("@x/greeter").map(|e| e.version.clone()));
        assert_eq!(v, Some("1.4.0".to_string()));
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
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        // Producer publishes via the plugin path so the prelude publishInterface is exercised.
        load_body("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/greeter",{ greet:function(n){ return "hi "+n.who; } });
        "#, "{}");
        // Consumer resolves a hard proxy and calls across (arg + return structured-copied).
        load_body("cons", r#"
            const g = require("@x/greeter");
            globalThis.__test_out = g.greet({ who: "world" });
        "#, "{}");
        assert_eq!(read_global_string("cons", "__test_out"), "hi world");

        // Producer-absent hard dep → InterfaceUnavailable (caught by the wrapper TryCatch → WARN).
        set_plugin_imports("lonely", vec![crate::interfaces::ImportSpec::new("@missing", "^1.0.0", crate::interfaces::Kind::Hard)]);
        load_body("lonely", r#"
            try { require("@missing").foo(); globalThis.__err = "no throw"; }
            catch (e) { globalThis.__err = String(e); }
        "#, "{}");
        assert!(read_global_string("lonely", "__err").contains("InterfaceUnavailable"));

        // Optional dep, not published → require returns null.
        set_plugin_imports("optc", vec![crate::interfaces::ImportSpec::new("@absent", "^1.0.0", crate::interfaces::Kind::Optional)]);
        load_body("optc", r#"globalThis.__opt = (require("@absent") === null) ? "null" : "proxy";"#, "{}");
        assert_eq!(read_global_string("optc", "__opt"), "null");

        // Non-serializable (cyclic) arg → InterfaceValueNotSerializable (JSON.stringify throws → None → throw).
        set_plugin_imports("cyc", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        load_body("cyc", r#"
            const g = require("@x/greeter");
            const a = {}; a.self = a;
            try { g.greet(a); globalThis.__e2 = "no throw"; }
            catch (e) { globalThis.__e2 = String(e); }
        "#, "{}");
        assert!(read_global_string("cyc", "__e2").contains("InterfaceValueNotSerializable"));

        // Producer method THROWS → consumer sees InterfaceCallError carrying the producer message
        // (not a crash, not a mislabeled InterfaceValueNotSerializable).
        set_plugin_publishes("prodBoom", [(
            "@x/boom".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prodBoom", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/boom", { boom: function(){ throw new Error("kaboom"); } });
        "#, "{}");
        set_plugin_imports("consBoom", vec![crate::interfaces::ImportSpec::new("@x/boom", "^1.0.0", crate::interfaces::Kind::Hard)]);
        load_body("consBoom", r#"
            const g = require("@x/boom");
            try { g.boom(); globalThis.__boom = "no throw"; } catch (e) { globalThis.__boom = String(e); }
        "#, "{}");
        let boom = read_global_string("consBoom", "__boom");
        assert!(boom.contains("InterfaceCallError"), "producer throw → InterfaceCallError, got: {}", boom);
        assert!(boom.contains("kaboom"), "producer message surfaced, got: {}", boom);

        // Producer method returns undefined (void) → consumer receives undefined, NOT a throw.
        set_plugin_publishes("prodVoid", [(
            "@x/void".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prodVoid", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/void", { poke: function(){ /* returns undefined */ } });
        "#, "{}");
        set_plugin_imports("consVoid", vec![crate::interfaces::ImportSpec::new("@x/void", "^1.0.0", crate::interfaces::Kind::Hard)]);
        load_body("consVoid", r#"
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
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            globalThis.__h = publishInterface("@x/greeter",{ greet:function(){return "";} });
        "#, "{}");
        load_body("cons", r#"
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
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prod", r#"const {publishInterface}=require("@s2script/interfaces");
            publishInterface("@x/greeter",{greet:function(){return "ok";}});"#, "{}");
        load_body("cons", r#"const g=require("@x/greeter");
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

    /// B1: consumer compiled against hash "bbb…" but the producer publishes "aaa…" — every call
    /// throws InterfaceTypesMismatch (the late-producer backstop; load-time refusal is loader-side).
    #[test]
    fn iface_call_throws_types_mismatch_when_compiled_hash_differs() {
        let _ = init(dummy_logger());
        set_plugin_publishes("tm_prod", [(
            "@x/tm".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "aaa111".into() },
        )].into_iter().collect());
        load_body("tm_prod",
            r#"const {publishInterface}=require("@s2script/interfaces");
               publishInterface("@x/tm",{ping:function(){return 1;}});"#, "{}");
        set_plugin_imports("tm_cons", vec![crate::interfaces::ImportSpec {
            name: "@x/tm".into(), range: "^1.0.0".into(), kind: crate::interfaces::Kind::Hard,
            compiled_types_sha256: Some("bbb222".into()),
        }]);
        load_body("tm_cons",
            r#"const h=require("@x/tm");
               globalThis.__tmcall=function(){ try { return String(h.ping()); } catch(e){ return String(e); } };"#, "{}");
        let out = eval_in_context_string("tm_cons", "globalThis.__tmcall()");
        assert!(out.contains("InterfaceTypesMismatch"), "got: {}", out);
        unload_plugin("tm_cons");
        unload_plugin("tm_prod");
        shutdown();
    }

    /// Task 7: consumer unload removes its subscriber rows from the producer's list and from
    /// IFACE_SUBS, so a later emit reaches nobody.
    #[test]
    fn consumer_unload_removes_subscriber() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prod", r#"const {publishInterface}=require("@s2script/interfaces");
            globalThis.__h=publishInterface("@x/greeter",{greet:function(){return "";}});"#, "{}");
        load_body("cons", r#"const g=require("@x/greeter"); g.on("greeted",function(){});"#, "{}");
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
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prod", r#"const {publishInterface}=require("@s2script/interfaces");
            publishInterface("@x/greeter",{greet:function(){return "still-here";}});"#, "{}");
        // consumer's onUnload calls the producer — must still work because producer outlives it.
        load_body("cons", r#"const g=require("@x/greeter");
            return { onUnload: function(){ globalThis.__unload_result = g.greet(); } };"#, "{}");
        unload_all();
        // If the producer had been torn down first, greet() would have thrown; the consumer's
        // onUnload observed a live producer.
        // (Assert via a log capture or a side channel; here we assert no crash + registry cleared.)
        assert!(IFACES.with(|r| r.borrow().lookup("@x/greeter").is_none()));
        assert!(PLUGINS.with(|p| p.borrow().is_empty()));
        shutdown();
    }


    /// E1: the two minting natives — index-minting via the books id, handle-minting via
    /// adoption. A dangling/mismatched handle can never mint; an absent index mints 0.
    #[test]
    fn minting_natives_are_books_backed() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        set_engine_ops(None);                                   // books-only paths: no ops needed
        let id = crate::entity_live::on_created(42, 7);
        create_plugin_context("mint");
        assert_eq!(eval_in_context_string("mint", "String(__s2_ent_id_for_index(42))"), id.to_string());
        assert_eq!(eval_in_context_string("mint", "String(__s2_ent_id_for_index(43))"), "0");
        let good = ((7u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32;
        let stale = ((9u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32;
        assert_eq!(
            eval_in_context_string("mint", &format!("var a=__s2_handle_adopt({good}); a ? a[0]+','+a[1] : 'null'")),
            format!("42,{id}"));
        assert_eq!(
            eval_in_context_string("mint", &format!("String(__s2_handle_adopt({stale}))")),
            "null", "a stale handle field can never mint a live ref");
        shutdown();
    }



    // ============================================================================================
    // Deferred-dispatch queue — the CORE half.
    //
    // Core holds `HOST.borrow_mut()` across ALL JS. Plugin-originated outbound (`Events.fire`,
    // `Engine.call`, …) publishes a nest token so inbound `fan_out_inner` uses CallbackScope
    // and does not take HOST. True engine inbound still arrives through the C ABI with no
    // handle (`#63`): that path reports `Delivery::Deferred` instead of a silent drop.
    //
    // Core detects the failed borrow but owns no dispatch payload: every dispatch ORIGINATES in the
    // shim, which still has its arguments on the stack, and a game event's data lives in an
    // engine-owned `IGameEvent` valid only for the duration of the call. So the SHIM owns the queue
    // and the replay. There is no shim in this process, which is why these tests stand a minimal one
    // up: `reentrant_event_fire` is the engine op a plugin's `Events.fire()` reaches, it observes
    // the `Delivery` core hands back, and `drain_deferred()` is the next frame's
    // `Hook_GameFramePre`. That is faithful — in the real system the shim is exactly the thing
    // behind that op — and it is why `v8host::dispatch_*` (not just the FFI wrapper) returns the
    // delivery status.
    //
    // `#63` closed reconstructing `&mut Isolate` from a raw pointer when the C-ABI inbound has
    // no live V8 handle. Outbound natives publish their `FunctionCallbackInfo` for that window;
    // only the no-handle case still defers.
    // ============================================================================================

    thread_local! {
        /// The mock shim's FIFO of deferred game-event replays (by name).
        static DEFERRED_Q: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        /// The mock shim's named-drop log — the lines `META_CONPRINTF`s in the real one.
        static DEFERRED_DROPS: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        /// What a re-entrant PRE hook answered (see `reentrant_pre_hook_is_skipped_not_deferred`).
        static PRE_REENTRANT_RESULT: std::cell::Cell<i32> = std::cell::Cell::new(-1);
        /// Did a re-entrant dispatch with NO subscribers report `Deferred`?
        static NOSUB_WAS_DEFERRED: std::cell::Cell<bool> = std::cell::Cell::new(false);
    }

    /// The mock shim's queue bound, standing in for `kDeferredQueueMax` (`shim/src/s2script_mm.cpp`).
    ///
    /// Deliberately 3 rather than the shim's 256: what is under test is the BEHAVIOUR at the
    /// boundary — drop the newest, name it, do not grow — not the number, which is a shim-side
    /// tuning knob. Driving 257 real JS dispatches to reach the shipped cap would test the same
    /// three assertions much more slowly.
    const MOCK_DEFER_MAX: usize = 3;

    fn deferred_len() -> usize { DEFERRED_Q.with(|q| q.borrow().len()) }
    fn deferred_names() -> Vec<String> { DEFERRED_Q.with(|q| q.borrow().clone()) }
    fn deferred_drops() -> Vec<String> { DEFERRED_DROPS.with(|d| d.borrow().clone()) }
    fn deferred_clear() {
        DEFERRED_Q.with(|q| q.borrow_mut().clear());
        DEFERRED_DROPS.with(|d| d.borrow_mut().clear());
    }

    /// The mock shim's bounded push, mirroring `S2_DeferGameEvent`: an unbounded queue would turn a
    /// plugin bug (a handler that re-fires its own event forever) into an OOM, so past the cap the
    /// NEWEST is dropped — and SAID, because a silent drop is the exact failure mode this slice
    /// exists to end. The wording matches the shim's log line so both are greppable as one string.
    fn defer_push(name: &str) {
        if deferred_len() >= MOCK_DEFER_MAX {
            DEFERRED_DROPS.with(|d| d.borrow_mut().push(format!(
                "deferred-dispatch: queue full ({}) — dropped game_event '{}' (newest)",
                MOCK_DEFER_MAX, name
            )));
            return;
        }
        DEFERRED_Q.with(|q| q.borrow_mut().push(name.to_string()));
    }

    /// The mock shim's drain, standing in for the top of `Hook_GameFramePre`. Returns how many
    /// entries it replayed.
    ///
    /// `mem::take` IS the spec's double buffer: a dispatch deferred BY a deferred handler lands in
    /// the NEXT drain rather than extending the current one, so a handler that re-fires its own
    /// event cannot spin the frame forever.
    ///
    /// **This mock covers CORE'S half of the contract only** — that a re-entrant dispatch reports
    /// `Deferred` and that its replay delivers. It is NOT coverage of the shipped drain, and must
    /// not be read as such: the real one lives in `shim/src/defer_queue.cpp` over two file-scope
    /// buffers plus a `swap`, and a `mem::take` into a local `Vec` is structurally incapable of
    /// reproducing that shape (a flush re-entered from inside a replay clearing the buffer being
    /// walked shipped past this file for exactly that reason). The drain itself is tested by
    /// `shim/tests/defer_queue_test.cpp` — `scripts/test-defer-queue.sh` in `ci-native.sh`.
    fn drain_deferred() -> usize {
        let batch: Vec<String> = DEFERRED_Q.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for name in &batch {
            // A replay that itself re-defers is dropped with a named reason, NEVER re-queued: the
            // drain runs with HOST provably free, so it can only mean a bug, and re-queueing would
            // spin across frames.
            assert_ne!(
                replay_game_event(name), Delivery::Deferred,
                "replay of '{}' re-deferred — the drain must run with HOST free", name
            );
        }
        batch.len()
    }

    /// The engine op behind a plugin's `Events.fire()`: the engine fires the event and
    /// synchronously dispatches it back into core. With a nest token this delivers now.
    extern "C" fn reentrant_event_fire(_dont: c_int) -> c_int {
        if dispatch_game_event("inner") == Delivery::Deferred {
            defer_push("inner");
        }
        1
    }

    /// `Events.fire` from a handler nests: the other plugin's listener runs before `fire()` returns.
    /// SourceMod `FireEvent` shape. DDQ is not this path — see `no_handle_reentry_is_delivered_next_drain`.
    #[test]
    fn events_fire_from_a_handler_nests() {
        let _ = init(dummy_logger());
        deferred_clear();
        set_engine_ops(Some(S2EngineOps {
            event_fire: Some(reentrant_event_fire),
            ..mock_event_ops()
        }));
        load_body("firer", r#"
            __s2pkg_events.Events.on("outer", function () {
                globalThis.__ran = 1;
                __s2_event_fire(false);
            });
        "#, "{}");
        load_body("listener", r#"
            __s2pkg_events.Events.on("inner", function () {
                globalThis.__ran = (globalThis.__ran || 0) + 1;
            });
        "#, "{}");

        let _ = dispatch_game_event("outer");
        assert_eq!(read_i32_global_in("firer", "__ran"), 1, "outer handler must run");
        assert_eq!(
            read_i32_global_in("listener", "__ran"), 1,
            "Events.fire must run other plugins' handlers before it returns"
        );
        assert_eq!(deferred_len(), 0, "a nested Events.fire is not a DDQ defer");

        set_engine_ops(None);
        deferred_clear();
        shutdown();
    }

    /// `#63` no-handle inbound while HOST is held still defers notify and delivers next drain.
    #[test]
    fn no_handle_reentry_is_delivered_next_drain() {
        let _ = init(dummy_logger());
        deferred_clear();
        set_engine_ops(Some(mock_event_ops()));
        load_body("listener", r#"
            __s2pkg_events.Events.on("inner", function () {
                globalThis.__ran = (globalThis.__ran || 0) + 1;
            });
        "#, "{}");

        with_host_borrowed(|| {
            if dispatch_game_event("inner") == Delivery::Deferred {
                defer_push("inner");
            }
        });
        assert_eq!(
            read_i32_global_in("listener", "__ran"), 0,
            "C-ABI inbound with no nest token cannot take HOST"
        );
        assert_eq!(deferred_len(), 1);

        assert_eq!(drain_deferred(), 1);
        assert_eq!(read_i32_global_in("listener", "__ran"), 1);
        assert_eq!(deferred_len(), 0);

        set_engine_ops(None);
        deferred_clear();
        shutdown();
    }

    /// `mem::take` isolation: a push that happens after the drain has taken the batch is the
    /// NEXT drain, not an extension of this one. This is the `#63` double-buffer, no longer
    /// driven by `Events.fire` (that nests).
    #[test]
    fn nested_defer_lands_in_the_next_drain_not_the_current_one() {
        let _ = init(dummy_logger());
        deferred_clear();
        defer_push("inner");
        let batch: Vec<String> = DEFERRED_Q.with(|q| std::mem::take(&mut *q.borrow_mut()));
        assert_eq!(batch, vec!["inner".to_string()]);
        defer_push("inner");
        assert_eq!(deferred_len(), 1, "a push after take is the next batch");
        assert_eq!(batch.len(), 1, "the drain must not grow while it is running");
        deferred_clear();
        shutdown();
    }

    /// Two dispatches deferred in ONE frame replay in PUSH order.
    ///
    /// The spec's reason for ONE FIFO rather than a queue per payload type: a deferred `player_death`
    /// and a deferred client-disconnect must keep their relative order, which a split queue leaves
    /// undefined. The op below pushes `innerB` BEFORE `innerA` precisely so the assertion can tell
    /// push order apart from the orders a broken drain would produce — sorted or LIFO both read
    /// `"AB"`, only a FIFO reads `"BA"`.
    #[test]
    fn deferred_dispatches_replay_in_push_order() {
        let _ = init(dummy_logger());
        deferred_clear();
        set_engine_ops(Some(mock_event_ops()));
        load_body("listener", r#"
            __s2pkg_events.Events.on("innerA", function () {
                globalThis.__order = (globalThis.__order || "") + "A";
            });
            __s2pkg_events.Events.on("innerB", function () {
                globalThis.__order = (globalThis.__order || "") + "B";
            });
        "#, "{}");

        // `#63` no-handle: two inbound dispatches while HOST is held, no nest token.
        with_host_borrowed(|| {
            for name in ["innerB", "innerA"] {
                if dispatch_game_event(name) == Delivery::Deferred {
                    defer_push(name);
                }
            }
        });
        assert_eq!(
            deferred_names(), vec!["innerB".to_string(), "innerA".to_string()],
            "both re-entrant dispatches must land in the ONE queue, in the order they were pushed"
        );
        assert_eq!(
            read_string_global_in("listener", "__order"), "undefined",
            "neither ran inside the held borrow"
        );

        assert_eq!(drain_deferred(), 2, "one drain replays the whole batch");
        assert_eq!(
            read_string_global_in("listener", "__order"), "BA",
            "replayed in PUSH order; sorted or LIFO order would read 'AB'"
        );
        assert_eq!(deferred_len(), 0);

        set_engine_ops(None);
        deferred_clear();
        shutdown();
    }

    /// The queue is BOUNDED: pushing past the cap drops the newest, SAYS SO BY NAME, and the queue
    /// does not grow.
    ///
    /// An unbounded queue turns a plugin bug — a handler that re-fires its own event every time —
    /// into an OOM, so the bound is not optional. What makes the bound safe rather than a
    /// re-introduction of the silent drop is that overflow is NAMED: the log says which dispatch was
    /// dropped and that it was the newest. This asserts all three (cap held, drop counted, drop
    /// named) against the mock shim's `defer_push`; the shipped bound is the shim's
    /// `kDeferredQueueMax` and its `META_CONPRINTF` (`shim/src/s2script_mm.cpp`), which this mirrors
    /// line for line at a smaller cap.
    #[test]
    fn deferred_queue_is_bounded_and_names_what_it_drops() {
        const BURST: usize = MOCK_DEFER_MAX + 2;
        let _ = init(dummy_logger());
        deferred_clear();
        set_engine_ops(Some(mock_event_ops()));
        load_body("listener", r#"
            __s2pkg_events.Events.on("inner", function () {
                globalThis.__ran = (globalThis.__ran || 0) + 1;
            });
        "#, "{}");

        with_host_borrowed(|| {
            for _ in 0..BURST {
                if dispatch_game_event("inner") == Delivery::Deferred {
                    defer_push("inner");
                }
            }
        });

        assert_eq!(deferred_len(), MOCK_DEFER_MAX, "the queue must not grow past its cap");
        let drops = deferred_drops();
        assert_eq!(
            drops.len(), BURST - MOCK_DEFER_MAX,
            "every push past the cap is dropped — and counted, not swallowed"
        );
        for d in &drops {
            assert!(
                d.contains("queue full") && d.contains("game_event 'inner'") && d.contains("(newest)"),
                "an overflow drop must NAME the dispatch it dropped, not vanish: {}", d
            );
        }

        // The entries that DID fit are still delivered — overflow degrades this dispatch, not the queue.
        assert_eq!(drain_deferred(), MOCK_DEFER_MAX);
        assert_eq!(read_i32_global_in("listener", "__ran"), MOCK_DEFER_MAX as i32);
        assert_eq!(deferred_len(), 0);

        set_engine_ops(None);
        deferred_clear();
        shutdown();
    }

    /// The engine op for the PRE-hook contrast: re-enter `dispatch_game_event_pre` under the borrow.
    extern "C" fn reentrant_event_fire_pre(_dont: c_int) -> c_int {
        // No `Delivery` to observe and nothing to queue — `dispatch_game_event_pre` returns a plain
        // suppress/allow int, BY CONSTRUCTION. That is the point of the test.
        PRE_REENTRANT_RESULT.with(|c| c.set(dispatch_game_event_pre("innerpre")));
        1
    }

    /// `Events.fire` nests `onPre` too: the subscriber runs and its HookResult is what the engine sees.
    #[test]
    fn events_fire_nests_pre_hooks() {
        let _ = init(dummy_logger());
        deferred_clear();
        PRE_REENTRANT_RESULT.with(|c| c.set(-1));
        set_engine_ops(Some(S2EngineOps {
            event_fire: Some(reentrant_event_fire_pre),
            ..mock_event_ops()
        }));
        load_body("firer", r#"
            __s2pkg_events.Events.on("outer", function () {
                globalThis.__ran = 1;
                __s2_event_fire(false);
            });
        "#, "{}");
        load_body("prelistener", r#"
            __s2_event_subscribe_pre("innerpre", function () {
                globalThis.__ran = 1;
                return 2;
            });
        "#, "{}");

        let _ = dispatch_game_event("outer");
        assert_eq!(read_i32_global_in("firer", "__ran"), 1, "outer handler must run");
        assert_eq!(
            read_i32_global_in("prelistener", "__ran"), 1,
            "Events.fire must run onPre before it returns"
        );
        assert_eq!(
            PRE_REENTRANT_RESULT.with(|c| c.get()), 1,
            "Handled (2) collapses to suppress (1) — the engine sees the subscriber's answer"
        );
        assert_eq!(deferred_len(), 0, "a nested onPre is not queued");

        set_engine_ops(None);
        deferred_clear();
        shutdown();
    }

    /// `#63` no-handle PRE inbound is still skip + fail-open (pre-hooks cannot replay).
    #[test]
    fn no_handle_pre_hook_is_skipped_not_deferred() {
        let _ = init(dummy_logger());
        deferred_clear();
        set_engine_ops(Some(mock_event_ops()));
        load_body("prelistener", r#"
            __s2_event_subscribe_pre("innerpre", function () {
                globalThis.__ran = 1;
                return 2;
            });
        "#, "{}");

        let pre = with_host_borrowed(|| dispatch_game_event_pre("innerpre"));
        assert_eq!(
            read_i32_global_in("prelistener", "__ran"), 0,
            "C-ABI onPre with no nest token is skipped"
        );
        assert_eq!(pre, 0, "fail-open ALLOW, not the Handled the subscriber wanted");
        assert_eq!(deferred_len(), 0, "pre-hooks are not deferrable");

        set_engine_ops(None);
        deferred_clear();
        shutdown();
    }

    /// The engine op for the empty-snapshot case: re-enter a dispatch NOBODY subscribes to.
    extern "C" fn reentrant_fire_no_subscribers(_dont: c_int) -> c_int {
        NOSUB_WAS_DEFERRED.with(|c| c.set(dispatch_game_event("nobody_listens") == Delivery::Deferred));
        1
    }

    /// An EMPTY subscriber snapshot never defers, even under a held borrow.
    ///
    /// Core reports `Deferred` iff the snapshot is non-empty AND `try_borrow_mut` failed. Reporting
    /// it for an unsubscribed event would make the shim `DuplicateEvent` (and hold, and free) every
    /// event fired on a server with nobody listening — a per-event allocation for a replay that
    /// would reach no one.
    #[test]
    fn empty_snapshot_never_defers() {
        let _ = init(dummy_logger());
        deferred_clear();
        NOSUB_WAS_DEFERRED.with(|c| c.set(true));   // must be cleared BY the dispatch, not by default
        set_engine_ops(Some(S2EngineOps {
            event_fire: Some(reentrant_fire_no_subscribers),
            ..mock_event_ops()
        }));
        load_body("firer", r#"
            __s2pkg_events.Events.on("outer", function () {
                globalThis.__ran = 1;
                __s2_event_fire(false);
            });
        "#, "{}");

        let _ = dispatch_game_event("outer");
        assert_eq!(read_i32_global_in("firer", "__ran"), 1, "outer handler must run");
        assert!(
            !NOSUB_WAS_DEFERRED.with(|c| c.get()),
            "a re-entrant dispatch with no subscribers must report Delivered, not Deferred"
        );

        set_engine_ops(None);
        deferred_clear();
        shutdown();
    }

    /// `fan_out`'s throw-isolation guarantee, asserted on a converted path.
    ///
    /// The per-handler `TryCatch` is the part of the preamble a deduplication most easily loses —
    /// hoisting it out of the loop, or dropping it because "the caller has one", would make ONE
    /// plugin's throwing handler silently deny every later subscriber its dispatch. That failure is
    /// invisible in a diff and invisible at runtime except as "my plugin stopped getting events",
    /// so it gets its own test rather than riding on the existing per-capability ones.
    ///
    /// Two plugins subscribe to the same event; the first throws. The second must still run, and a
    /// later dispatch must still reach both.
    #[test]
    fn fan_out_isolates_a_throwing_handler_from_the_rest() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_body("thrower", r#"
            __s2pkg_clients.Clients.onConnect(function () {
                globalThis.__ran = (globalThis.__ran || 0) + 1;
                throw new Error("boom");
            });
        "#, "{}");
        load_body("survivor", r#"
            __s2pkg_clients.Clients.onConnect(function (c) {
                globalThis.__ran  = (globalThis.__ran || 0) + 1;
                globalThis.__slot = c.slot;
            });
        "#, "{}");

        let _ = dispatch_client_event("connect", 3);
        assert_eq!(read_i32_global_in("thrower", "__ran"), 1, "the throwing handler must run");
        assert_eq!(
            read_i32_global_in("survivor", "__ran"), 1,
            "a handler that throws must not deny later subscribers their dispatch"
        );
        assert_eq!(read_i32_global_in("survivor", "__slot"), 3, "and its arguments must be intact");

        // The throw must not have poisoned the subscription either — both run again next dispatch.
        let _ = dispatch_client_event("connect", 4);
        assert_eq!(read_i32_global_in("thrower", "__ran"), 2, "throwing must not disable the sub");
        assert_eq!(read_i32_global_in("survivor", "__ran"), 2);
        assert_eq!(read_i32_global_in("survivor", "__slot"), 4);
        shutdown();
    }


    thread_local! { static VOICE_MUTED_CAPTURE: std::cell::RefCell<[i32; 64]> = std::cell::RefCell::new([0; 64]); }
    extern "C" fn capture_voice_set_muted(slot: c_int, muted: c_int) -> c_int {
        if !(0..64).contains(&slot) { return 0; }
        VOICE_MUTED_CAPTURE.with(|a| a.borrow_mut()[slot as usize] = if muted != 0 { 1 } else { 0 });
        1
    }
    extern "C" fn capture_voice_get_muted(slot: c_int) -> c_int {
        if !(0..64).contains(&slot) { return -1; }
        VOICE_MUTED_CAPTURE.with(|a| a.borrow()[slot as usize])
    }

    /// Voice-control: Client.voiceMuted round-trips through the voice_set_muted/voice_get_muted ops
    /// (set writes the shim-side flag; get maps 1 -> true, 0 -> false).
    #[test]
    fn voice_muted_property_round_trips_through_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps {
            voice_set_muted: Some(capture_voice_set_muted),
            voice_get_muted: Some(capture_voice_get_muted),
            ..mock_event_ops()
        }));
        VOICE_MUTED_CAPTURE.with(|a| *a.borrow_mut() = [0; 64]);
        create_plugin_context("pvm");
        assert_eq!(eval_in_context_string("pvm",
            "var c = new __s2pkg_clients.Client(5); c.voiceMuted = true; String(c.voiceMuted)"), "true");
        assert_eq!(VOICE_MUTED_CAPTURE.with(|a| a.borrow()[5]), 1, "op received (5, 1)");
        assert_eq!(eval_in_context_string("pvm", "c.voiceMuted = false; String(c.voiceMuted)"), "false");
        assert_eq!(VOICE_MUTED_CAPTURE.with(|a| a.borrow()[5]), 0, "op received (5, 0)");
        shutdown();
    }

    /// Voice-control degrade: with no engine ops the setter is a silent no-op and reads are false
    /// (get_muted degrades to -1, which must NOT read as muted).
    #[test]
    fn voice_muted_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pvd");
        assert_eq!(eval_in_context_string("pvd",
            "var c = new __s2pkg_clients.Client(2); c.voiceMuted = true; String(c.voiceMuted)"), "false");
        shutdown();
    }

    /// Voice-control: Clients.onVoice subscribes on the existing CLIENT_MUX under the "voice" name —
    /// a dispatched "voice" event delivers a Client with the slot; other names don't cross-fire.
    #[test]
    fn voice_client_event_dispatches_to_on_voice() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_body("pvv", r#"
            __s2pkg_clients.Clients.onVoice(function (c) {
                globalThis.__v_ran  = (globalThis.__v_ran || 0) + 1;
                globalThis.__v_slot = c.slot;
            });
        "#, "{}");
        let _ = dispatch_client_event("voice", 4);
        assert_eq!(read_i32_global_in("pvv", "__v_ran"), 1, "onVoice handler runs once");
        assert_eq!(read_i32_global_in("pvv", "__v_slot"), 4, "handler receives the dispatched slot");
        let _ = dispatch_client_event("settingschanged", 4);   // a different name must not re-run it
        assert_eq!(read_i32_global_in("pvv", "__v_ran"), 1);
        shutdown();
    }

    /// dispatch_map_start delivers the map name to a Server.onMapStart subscriber (the MAP_MUX reuse +
    /// the string-arg dispatch); mirrors client_event_dispatch_reaches_subscriber.
    #[test]
    fn map_start_dispatch_delivers_map_name() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pms");
        eval_in_context_string("pms", r#"
            globalThis.__map = "";
            __s2pkg_server.Server.onMapStart(function (m) { globalThis.__map = m; });
            "ok"
        "#);
        let _ = dispatch_map_start("de_test");
        assert_eq!(eval_in_context_string("pms", "globalThis.__map"), "de_test");
        shutdown();
    }

    /// dispatch_precache runs a Sound.onPrecache-level subscriber (raw __s2_precache_subscribe —
    /// the module wrapper is Task-4-tested); the block-scoped add degrades false with no op.
    #[test]
    fn precache_dispatch_runs_subscriber() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("ppc");
        eval_in_context_string("ppc", r#"
            globalThis.__fired = 0; globalThis.__addResult = null;
            __s2_precache_subscribe(function () {
                globalThis.__fired++;
                globalThis.__addResult = __s2_sound_precache_add("soundevents/x.vsndevts");
            });
            "ok"
        "#);
        dispatch_precache();
        assert_eq!(eval_in_context_string("ppc", "String(globalThis.__fired)"), "1");
        assert_eq!(eval_in_context_string("ppc", "String(globalThis.__addResult)"), "false");
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Entity lifecycle listeners slice: Entity.onCreate/onSpawn/onDelete
    // ---------------------------------------------------------------------------




    thread_local! { static SNAPSHOT_PAIRS: std::cell::RefCell<Vec<(i32, i32)>> = std::cell::RefCell::new(Vec::new()); }
    extern "C" fn fake_ent_snapshot(oi: *mut c_int, os: *mut c_int, cap: c_int) -> c_int {
        SNAPSHOT_PAIRS.with(|p| {
            let p = p.borrow();
            let n = p.len().min(cap as usize);
            for i in 0..n { unsafe { *oi.add(i) = p[i].0; *os.add(i) = p[i].1; } }
            p.len() as c_int
        })
    }

    /// E1: the map-start-armed repair sweep reconciles the books from the identity-chunk
    /// snapshot at the FIRST SIMULATING frame — and only then (a non-simulating frame
    /// leaves it armed).
    #[test]
    fn repair_sweep_runs_once_on_first_simulating_frame() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps { ent_snapshot: Some(fake_ent_snapshot), ..mock_event_ops() }));
        SNAPSHOT_PAIRS.with(|p| *p.borrow_mut() = vec![(1, 11), (64, 3)]);
        crate::entity_live::clear_for_map_transition();          // arm (what map start does)
        entity_repair_sweep_if_armed(false);                     // NOT simulating → stays armed
        assert_eq!(crate::entity_live::len(), 0);
        entity_repair_sweep_if_armed(true);                      // simulating → reconcile
        assert!(crate::entity_live::lookup(1).is_some() && crate::entity_live::lookup(64).is_some());
        SNAPSHOT_PAIRS.with(|p| p.borrow_mut().clear());
        entity_repair_sweep_if_armed(true);                      // disarmed → no second sweep
        assert_eq!(crate::entity_live::len(), 2, "sweep is one-shot per arming");
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
        load_body("er2", r#"
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
        // the string WRITE native degrades to false (stale/unresolved ref → no write):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_string(1,7,8,128,'x'))"), "false");
        // EntityRef methods degrade (proving they're wired) — use `__s2require` (the native, available in a
        // create_plugin_context raw scope, as `eval_std` uses), NOT the CJS `require` (only in load_plugin_js):
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readUInt64(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readInt64(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readFloat64(8))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readString(8,128))"#), "null");
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).writeString(8,128,'x'))"#), "false");
        shutdown();
    }

    /// `Sound.stop` is a SPELLING of `EntityRef.stopSound`, not a reimplementation: it must reach the
    /// same native with byte-identical arguments, and must short-circuit to `false` without touching
    /// the native when no entity is supplied.
    ///
    /// Spies on `__s2_ent_stop_sound` rather than asserting the degraded return value, because
    /// without engine-ops EVERY path returns `false` — including a `Sound.stop` that silently did
    /// nothing at all. A return-value test here would pass on a completely broken forward.
    #[test]
    fn sound_stop_forwards_identically_to_entityref_stop_sound() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        eval_in_context("p", r#"
            globalThis.__calls = [];
            // A bare identifier in the prelude resolves against the global at CALL time, so replacing
            // the native here intercepts both the direct and the Sound.stop path.
            globalThis.__s2_ent_stop_sound = function (index, id, name) {
                globalThis.__calls.push(index + "," + id + "," + name + "," + typeof name);
                return true;
            };
            var { EntityRef } = __s2require("@s2script/entity");
            var { Sound } = __s2require("@s2script/sound");
            var ref = new EntityRef(3, 11);
            globalThis.__direct   = String(ref.stopSound("Weapon.Fire"));
            globalThis.__viaSound = String(Sound.stop("Weapon.Fire", { entity: ref }));
            globalThis.__reached  = String(globalThis.__calls.length);
            // no entity / no opts at all: false WITHOUT reaching the native
            globalThis.__noEntity  = String(Sound.stop("Weapon.Fire", {}));
            globalThis.__noOpts    = String(Sound.stop("Weapon.Fire"));
            globalThis.__nullEnt   = String(Sound.stop("Weapon.Fire", { entity: null }));
            globalThis.__afterMiss = String(globalThis.__calls.length);
        "#).unwrap();
        // both spellings reached the native, and both returned what it returned
        assert_eq!(eval_in_context_string("p", "__direct"), "true");
        assert_eq!(eval_in_context_string("p", "__viaSound"), "true");
        assert_eq!(eval_in_context_string("p", "__reached"), "2");
        // ...with identical arguments, including the String() coercion of the name
        assert_eq!(
            eval_in_context_string("p", "__calls.join('|')"),
            "3,11,Weapon.Fire,string|3,11,Weapon.Fire,string",
        );
        // the three no-entity forms degrade to false and never call the native (count stays 2)
        assert_eq!(eval_in_context_string("p", "__noEntity"), "false");
        assert_eq!(eval_in_context_string("p", "__noOpts"), "false");
        assert_eq!(eval_in_context_string("p", "__nullEnt"), "false");
        assert_eq!(eval_in_context_string("p", "__afterMiss"), "2");
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
        load_body("p", r#"
            const cs2 = require("@s2script/cs2");
            globalThis.__ok = String(cs2 !== null && cs2.hasEntityRef === true && cs2.noRequire === true);
        "#, "{}");
        assert_eq!(read_global_string("p", "__ok"), "true");
        shutdown();
    }

    /// Declarative inbound hooks, Task 4 fix round 1: a game package's `__s2pkg_game_ctx` must
    /// never silently clobber a built-in `ctx` member. A package that declares (by typo or by
    /// design) a namespace named `events` is REFUSED — the built-in `ctx.events` survives intact —
    /// while a non-colliding namespace (`gameRules`) still merges normally.
    #[test]
    fn game_ctx_namespace_cannot_clobber_a_builtin() {
        let _ = init(dummy_logger());
        register_injected_package(
            "@s2script/cs2",
            r#"globalThis.__s2pkg_game_ctx = {
                 events: function (reg, viaId) { return { bogus: true }; },
                 gameRules: function (reg, viaId) { return { ok: true }; },
               };"#,
        );
        load_body("gctx", r#"
            globalThis.__eventsOnIsFn = String(typeof ctx.events.on === "function");
            globalThis.__eventsBogus  = String(ctx.events.bogus);
            globalThis.__gameRulesOk  = String(ctx.gameRules.ok);
        "#, "{}");
        assert_eq!(read_global_string("gctx", "__eventsOnIsFn"), "true");     // built-in survives
        assert_eq!(read_global_string("gctx", "__eventsBogus"), "undefined"); // the collision never landed
        assert_eq!(read_global_string("gctx", "__gameRulesOk"), "true");      // a non-colliding name still merges
        shutdown();
    }

    // --- Slice 4.5 Task 1: EntityRef replacer/reviver wire round-trip ---

    #[test]
    fn iface_call_return_rehydrates_entityref() {
        let _ = init(dummy_logger());
        set_engine_ops(None); // degrade path: a real EntityRef -> isValid()==false, readInt32()==null
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/ent", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/ent".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        // Producer returns an EntityRef from a method.
        load_body("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            const { EntityRef } = require("@s2script/entity");
            publishInterface("@x/ent", { getRef: function(){ return new EntityRef(1, 7); } });
        "#, "{}");
        // Consumer receives it: must be a LIVE EntityRef (methods present), not plain data.
        load_body("cons", r#"
            const { EntityRef } = require("@s2script/entity");
            const r = require("@x/ent").getRef();
            globalThis.__isRef  = String(r instanceof EntityRef);        // "true" — rehydrated
            globalThis.__idx    = String(r.index) + "," + String(r.id); // "1,7" — data crossed
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
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/ent", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/ent".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            const { EntityRef } = require("@s2script/entity");
            globalThis.__h = publishInterface("@x/ent", { noop: function(){} });
        "#, "{}");
        load_body("cons", r#"
            const { EntityRef } = require("@s2script/entity");
            const g = require("@x/ent");
            globalThis.__seen = "none";
            g.on("spawned", function (r) {
                globalThis.__seen = (r instanceof EntityRef) ? (r.index + "," + r.id) : "plain";
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
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/data", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/data".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/data", { echo: function(){ return { a: 1, b: "hi", c: [1,2,3] }; } });
        "#, "{}");
        load_body("cons", r#"
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
    extern "C" fn stub_enumerate(ctx: *mut c_void, ec: EmitClassFn, ef: EmitFieldFn, ee: EmitEnumFn) -> c_int {
        ec(ctx, b"CTest\0".as_ptr() as *const c_char, b"CBase\0".as_ptr() as *const c_char);
        ef(ctx, b"CTest\0".as_ptr() as *const c_char, b"m_x\0".as_ptr() as *const c_char, 8,
           b"atomic\0".as_ptr() as *const c_char, b"int32\0".as_ptr() as *const c_char, std::ptr::null(), 0);
        ef(ctx, b"CTest\0".as_ptr() as *const c_char, b"m_h\0".as_ptr() as *const c_char, 12,
           b"handle\0".as_ptr() as *const c_char, std::ptr::null(), b"CThing\0".as_ptr() as *const c_char, 0);
        // Two enumerators of one enum, so the dump's enum table has something to serialize.
        ee(ctx, b"MoveType_t\0".as_ptr() as *const c_char, 1, b"MOVETYPE_NONE\0".as_ptr() as *const c_char, 0);
        ee(ctx, b"MoveType_t\0".as_ptr() as *const c_char, 1, b"MOVETYPE_FLY\0".as_ptr() as *const c_char, 5);
        1
    }

    /// Full core path: stub enumerate → callbacks → Catalog → JSON → file. No real shim needed.
    #[test]
    fn schema_dump_writes_catalog_via_stub_enumerate() {
        let _ = init(dummy_logger());
        // Wire an ops table whose schema_enumerate is the stub (all other fields None).
        set_engine_ops(Some(S2EngineOps {
            schema_enumerate: Some(stub_enumerate),
            ..S2EngineOps::none()
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
        load_body("mods", r#"
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

    /// Ray-trace slice: `@s2script/math`'s `forwardVector` — a known-angle sanity check
    /// (yaw=0,pitch=0 -> forward (1,0,0); yaw=90,pitch=0 -> forward ~(0,1,0)). Pure math, no ops.
    #[test]
    fn forward_vector_known_angles() {
        let _ = init(dummy_logger());
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", r#"
                var m = __s2require("@s2script/math");
                var f = m.forwardVector(new m.QAngle(0, 0, 0));
                f.x.toFixed(3) + "," + f.y.toFixed(3) + "," + f.z.toFixed(3)
            "#),
            "1.000,0.000,0.000"
        );
        assert_eq!(
            eval_in_context_string("p", r#"
                var m = __s2require("@s2script/math");
                var f = m.forwardVector(new m.QAngle(0, 90, 0));
                f.x.toFixed(3) + "," + f.y.toFixed(3) + "," + f.z.toFixed(3)
            "#),
            "0.000,1.000,0.000"
        );
        shutdown();
    }

    /// Ray-trace slice: `__s2_trace` degrades to a MISS `TraceHit` when there's no `trace_shape`
    /// op (e.g. every in-isolate test, which never wires the shim): `didHit:false, fraction:1,
    /// allSolid:false, entity:null`, and `endPos` defaults to the requested `end` (not a zero
    /// vector) — `endPos`/`normal` are real `Vector` instances, not plain objects.
    #[test]
    fn trace_native_degrades_to_miss_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let js = r#"
            var m = __s2require("@s2script/math");
            var hit = __s2_trace([0, 0, 0], [10, 20, 30], [0, 0, 0], [0, 0, 0], 1, 0, -1, -1);
            [
                hit.didHit, hit.fraction, hit.startSolid, (hit.entity === null),
                hit.endPos instanceof m.Vector, hit.endPos.x, hit.endPos.y, hit.endPos.z,
                hit.normal instanceof m.Vector, hit.normal.x, hit.normal.y, hit.normal.z,
            ].join(",")
        "#;
        // NOTE: `entity` is asserted `=== null` explicitly — Array.join renders a bare `null` as an
        // empty field, which would silently pass for `undefined` too.
        assert_eq!(
            eval_in_context_string("p", js),
            "false,1,false,true,true,10,20,30,true,0,0,0"
        );
        shutdown();
    }

    /// Ray-trace slice: `TraceMask.ShotPhysics` matches the reference project's own
    /// `static_assert(MASK_SHOT_PHYSICS == 0x2c3011, ...)` value (shim/src/trace.h) — the JS
    /// composite mirrors the C++ constexpr bit-for-bit.
    #[test]
    fn trace_mask_shot_physics_matches_reference_value() {
        let _ = init(dummy_logger());
        create_plugin_context("p");
        assert_eq!(
            eval_in_context_string("p", r#"String(__s2require("@s2script/trace").TraceMask.ShotPhysics === 0x2c3011)"#),
            "true"
        );
        assert_eq!(
            eval_in_context_string("p", r#"String(__s2require("@s2script/trace").TraceMask.ShotPhysics)"#),
            "2895889"
        );
        shutdown();
    }

    /// Ray-trace slice: `Trace.line`/`ray`/`hull` compose cleanly end-to-end through the public
    /// `@s2script/trace` module (ignore-entity/mask/exclude defaulting, `forwardVector` composition
    /// in `ray`) and degrade to a MISS (no `trace_shape` op in-isolate) without throwing.
    #[test]
    fn trace_module_line_ray_hull_degrade_cleanly() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let js = r#"
            var t = __s2require("@s2script/trace").Trace;
            var m = __s2require("@s2script/math");
            var start = new m.Vector(0, 0, 0);
            var end = new m.Vector(100, 0, 0);
            var hitLine = t.line(start, end);
            var hitRay = t.ray(start, new m.QAngle(0, 0, 0), 100);
            var hitHull = t.hull(start, end, new m.Vector(-16, -16, -16), new m.Vector(16, 16, 16));
            [hitLine.didHit, hitRay.didHit, hitHull.didHit, hitRay.endPos.x.toFixed(0)].join(",")
        "#;
        assert_eq!(eval_in_context_string("p", js), "false,false,false,100");
        shutdown();
    }


    /// Game-rules slice: `Entity.findByClass` degrades to an empty array with no `entity_find_by_class`
    /// op (e.g. every in-isolate test) — never a crash.
    #[test]
    fn find_by_class_degrades_to_empty_array_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const refs = __s2pkg_entity.Entity.findByClass("some_class");
            String(Array.isArray(refs) && refs.length === 0)
        "#);
        assert_eq!(out, "true");
        shutdown();
    }



    /// entity_origin slice: `EntityRef.origin` (`CGameSceneNode::m_vecAbsOrigin`, reached via the
    /// `CBaseEntity::m_CBodyComponent` -> `CBodyComponent::m_pSceneNode` chain) degrades to `null` with
    /// no ops (e.g. every in-isolate test) — never a crash. Unlike `target`, there's no dedicated
    /// native: the getter is a prelude.js composition of the already-native `__s2_schema_offset` (x3,
    /// schema-resolved, never baked) and `EntityRef.readFloatsChain`, so this exercises that
    /// composition end-to-end — every offset lookup misses (-1) AND the root ref fails to resolve.
    #[test]
    fn entity_origin_degrades_to_null_without_ops() {
        init(dummy_logger()).unwrap();
        let out = eval_std("eo1", r#"
            var EntityRef = globalThis.__s2pkg_entity.EntityRef;
            var offMiss = __s2_schema_offset("CBaseEntity", "m_CBodyComponent");
            var viaRef = new EntityRef(5, 7).origin;
            JSON.stringify({ offMiss: offMiss, viaRef: viaRef });
        "#);
        assert_eq!(out, r#"{"offMiss":-1,"viaRef":null}"#);
        shutdown();
    }

    /// UserMessage slice: the `UserMessage` builder degrades with no engine ops — `create` returns 0
    /// so `send`/`sendAll` return `false`, the `set*` chain never throws, no crash.
    #[test]
    fn user_message_degrades_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const m = new __s2pkg_usermessages.UserMessage("CUserMessageFade");
            m.setInt("duration", 1024).set("flags", 18).set("amplitude", 1.5);
            // no ops installed -> create returns 0 -> send returns false, no throw
            String(m.send([0]) === false && m.sendAll() === false)
        "#);
        assert_eq!(out, "true");
        shutdown();
    }

    // -----------------------------------------------------------------------
    // Declarative inbound hooks — the DISPATCH path, in-isolate.
    //
    // The registry's own rules are unit-tested in `gamedata_hooks`; what can only be proven with a
    // live isolate is the part that faces a plugin: the block-scoped view object, the write-back of
    // a `mutable` param, the read-only-ness of the others, the collapse, and the bypass latch
    // bracketing an outbound invoke that DEGRADES.
    //
    // These mocks stand in for the shim's arg view and honour its liveness discipline: an accessor
    // accepts ONLY the exact pointer the dispatch was handed, so a view retained past its dispatch
    // fails here exactly as it would on a real frame.
    // -----------------------------------------------------------------------

    /// The stand-in for a thunk's stack frame. Any non-null token would do; the value is only ever
    /// compared, never dereferenced — which is precisely core's contract with the real thing.
    const HOOK_VIEW_TOKEN: usize = 0xF00D_BEEF;
    static HOOK_F32: Mutex<[f32; 1]> = Mutex::new([0.0]);
    static HOOK_I32: Mutex<[i32; 3]> = Mutex::new([0; 3]);
    static HOOK_ARMED: Mutex<Vec<i32>> = Mutex::new(Vec::new());
    static HOOK_DISARMED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

    /// `this_f32_i32_i32_i32`: param 0 is the float, params 1..=3 are the ints. Anything else is the
    /// shim's -1 — a stale binding, or a shape with no such param.
    extern "C" fn mock_hook_read_f32(view: *mut std::ffi::c_void, idx: c_int, out: *mut f32) -> c_int {
        if view as usize != HOOK_VIEW_TOKEN || idx != 0 { return -1; }
        unsafe { *out = HOOK_F32.lock().unwrap()[0] };
        0
    }
    extern "C" fn mock_hook_read_i32(view: *mut std::ffi::c_void, idx: c_int, out: *mut i32) -> c_int {
        if view as usize != HOOK_VIEW_TOKEN || !(1..=3).contains(&idx) { return -1; }
        unsafe { *out = HOOK_I32.lock().unwrap()[(idx - 1) as usize] };
        0
    }
    extern "C" fn mock_hook_write_f32(view: *mut std::ffi::c_void, idx: c_int, v: f32) -> c_int {
        if view as usize != HOOK_VIEW_TOKEN || idx != 0 { return -1; }
        HOOK_F32.lock().unwrap()[0] = v;
        0
    }
    extern "C" fn mock_hook_write_i32(view: *mut std::ffi::c_void, idx: c_int, v: i32) -> c_int {
        if view as usize != HOOK_VIEW_TOKEN || !(1..=3).contains(&idx) { return -1; }
        HOOK_I32.lock().unwrap()[(idx - 1) as usize] = v;
        0
    }
    /// The receiver the shim would hand back: `-1` = "this `this` is not an entity" (the common
    /// case — a rules/services singleton), otherwise a packed CEntityHandle. Settable, because BOTH
    /// answers have to be exercised: the null one, and the one that actually mints an object.
    static HOOK_RECEIVER: Mutex<i64> = Mutex::new(-1);

    /// RAII reset for a process-global test static. Restores the sentinel on Drop, so a panicking
    /// assertion between set and clear cannot leave the value armed for the NEXT test — the suite is
    /// forced single-threaded, so a leaked value is inherited, not merely untidy.
    struct ResetOnDrop<'a, T: Copy>(&'a Mutex<T>, T);
    impl<T: Copy> Drop for ResetOnDrop<'_, T> {
        fn drop(&mut self) {
            // On a POISONED lock this declines to act — it does not restore, and every later reader
            // here `.unwrap()`s, so they will panic on the PoisonError. That is deliberate but it is
            // NOT "surviving" poisoning: re-panicking inside Drop aborts the process, so declining
            // is the only safe option, and a poisoned lock already means a test panicked while
            // holding it. Not reachable from current code — nothing holds these across an assertion.
            if let Ok(mut g) = self.0.lock() { *g = self.1; }
        }
    }

    extern "C" fn mock_hook_receiver(v: *mut std::ffi::c_void, out: *mut u32) -> c_int {
        if v as usize != HOOK_VIEW_TOKEN { return -1; }
        let h = *HOOK_RECEIVER.lock().unwrap();
        if h < 0 { return -1; }
        unsafe { *out = h as u32 };
        0
    }
    extern "C" fn mock_hook_install(_id: c_int, _shape: c_int, _addr: i64, _r: *mut c_char, _c: c_int) -> c_int { 0 }
    extern "C" fn mock_hook_arm(id: c_int) { HOOK_ARMED.lock().unwrap().push(id); }
    extern "C" fn mock_hook_disarm(id: c_int) { HOOK_DISARMED.lock().unwrap().push(id); }
    #[allow(clippy::too_many_arguments)]
    extern "C" fn mock_call_resolve(
        _k: *const c_char, _m: *const c_char, _p: *const c_char, _r: *const c_char,
        _c: *const c_char, _i: c_int, _v: *const c_char, _out: *mut c_char, _cap: c_int,
    ) -> c_int { 7 }
    extern "C" fn mock_call_address(_id: c_int) -> i64 { 0x0000_7f00_0040_0000 }
    /// The invoke DEGRADES (0 = stale receiver / absent sub-object): the case where the hooked
    /// function is never reached, and therefore the case the latch would leak on.
    #[allow(clippy::too_many_arguments)]
    extern "C" fn mock_call_invoke_degrades(
        _id: c_int, _ei: c_int, _es: c_int, _so: c_int,
        _gp: *const u64, _gk: *const u8, _gc: c_int,
        _fp: *const f64, _fc: c_int,
        _s: *const *const c_char, _v: *const f32,
        _rk: c_int, _ro: *mut u64,
    ) -> c_int { 0 }

    fn hook_test_ops() -> S2EngineOps {
        S2EngineOps {
            engine_call_resolve:  Some(mock_call_resolve),
            engine_call_address:  Some(mock_call_address),
            engine_call_invoke:   Some(mock_call_invoke_degrades),
            hook_install:         Some(mock_hook_install),
            hook_arm_bypass:      Some(mock_hook_arm),
            hook_disarm_bypass:   Some(mock_hook_disarm),
            hook_read_f32:        Some(mock_hook_read_f32),
            hook_read_i32:        Some(mock_hook_read_i32),
            hook_write_f32:       Some(mock_hook_write_f32),
            hook_write_i32:       Some(mock_hook_write_i32),
            hook_receiver_handle: Some(mock_hook_receiver),
            ..mock_event_ops()
        }
    }

    /// One `hooks` entry on the 4-param shape, plus a receiverless `calls` entry it bypasses with.
    /// `u5` is declared past the shape's params ON PURPOSE — that is the stale-binding case whose
    /// read must degrade BY NAME rather than hand a handler a plausible-looking 0.
    fn hook_gamedata() -> &'static str {
        r#"{"signatures":{"Sig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48","resolve":"direct",
                          "validate":{"prologue":"55"}}}},
            "calls":{"doThing":{"receiver":{"kind":"none"},
                     "target":{"kind":"signature","name":"Sig"},"args":[],"returns":"void"}},
            "hooks":{"onX":{"target":{"kind":"signature","name":"Sig"},
                     "shape":"this_f32_i32_i32_i32",
                     "params":["delay","reason","u3","u4","u5"],"mutable":["delay","reason"],
                     "bypassWith":"doThing","expose":{"ctx":"g"}}}}"#
    }

    fn hook_test_setup(plugin: &str) -> i32 {
        crate::loader::load_permissions_from_str(&format!(
            r#"{{"engine:calls":["{p}"],"engine:hooks":["{p}"]}}"#, p = plugin
        )).expect("parses");
        HOOK_ARMED.lock().unwrap().clear();
        HOOK_DISARMED.lock().unwrap().clear();
        *HOOK_F32.lock().unwrap() = [1.5];
        *HOOK_I32.lock().unwrap() = [7, 8, 9];
        crate::gamedata_calls::register_plugin(plugin, hook_gamedata());
        crate::gamedata_hooks::register_plugin(plugin, hook_gamedata());
        assert_eq!(crate::gamedata_hooks::status(plugin, "onX"), "available",
            "{}", crate::gamedata_hooks::status(plugin, "onX"));
        crate::gamedata_hooks::plan(plugin, "onX").expect("ready").hook_id
    }

    /// `Engine.hook` is the plugin-facing subscribe factory: owner is the calling context (never
    /// an argument), null when the descriptor is missing, and a successful subscribe actually
    /// fires on dispatch.
    #[test]
    fn engine_hook_factory_uses_the_calling_plugin() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk_eng");
        create_plugin_context("hk_eng");

        eval_in_context("hk_eng", r#"
            var Engine = __s2require("@s2script/sdk/unsafe").Engine;
            globalThis.__ready = Engine.hook("onX") !== null;
            globalThis.__status = Engine.hookStatus("onX");
            globalThis.__missing = Engine.hook("nope") === null;
            globalThis.__missingStatus = Engine.hookStatus("nope");
            globalThis.__hit = null;
            var onX = Engine.hook("onX");
            onX(function (v) { globalThis.__hit = v.reason; return HookResult.Continue; });
        "#).unwrap();
        assert!(eval_in_context_bool("hk_eng", "globalThis.__ready === true"),
            "Engine.hook('onX') must return a subscribe function when the descriptor is ready");
        assert_eq!(eval_in_context_string("hk_eng", "String(globalThis.__status)"), "available");
        assert!(eval_in_context_bool("hk_eng", "globalThis.__missing === true"),
            "Engine.hook('nope') must be null — an undeclared name is not a callable");
        assert_eq!(
            eval_in_context_string("hk_eng", "String(globalThis.__missingStatus)"),
            "not declared in this owner's gamedata"
        );

        assert_eq!(dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void), 0);
        assert!(eval_in_context_bool("hk_eng", "globalThis.__hit === 7"),
            "Engine.hook subscribe must actually fire (reason mock is 7)");
    }

    /// The view is LIVE: reads hit the frame, a `mutable` write reaches the engine's copy, a
    /// read-only param does not, and the handlers' results collapse the standard way.
    #[test]
    fn hook_dispatch_delivers_a_live_view_and_collapses() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk1");
        create_plugin_context("hk1");

        // No subscribers yet: the thunk must be told to proceed, and nothing may be dispatched.
        assert_eq!(dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void), 0);

        eval_in_context("hk1", r#"
            globalThis.__seen = null;
            __s2_hook_on("hk1", "onX", function (v) {
                globalThis.__seen = [v.delay, v.reason, v.u3, v.u4];
                v.reason = 42;          // mutable -> must reach the engine
                v.delay = 0.25;         // mutable
                try { v.u3 = 999; } catch (e) { globalThis.__threw = true; }  // read-only
                return HookResult.Handled;
            });
        "#).unwrap();

        // Through the C-ABI entry, not `dispatch_hook` directly: the invariant that matters is what
        // crosses BACK to the thunk. It must be a plain collapsed HookResult and never the
        // deferred-dispatch sentinel — `argView` is a stack frame, so a replayed dispatch would hand
        // JS a dead one. This assertion is what would catch a future refactor swapping
        // `fan_out_collapsing` (which discards `Delivery`) for a deferrable fan-out.
        let r = crate::ffi::s2script_core_dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);
        assert_ne!(r, crate::ffi::S2_DISPATCH_DEFERRED, "a hook dispatch is NEVER deferrable");
        assert!((0..=3).contains(&r), "the thunk only understands a collapsed HookResult, got {r}");
        assert_eq!(r, 2, "Handled collapses to 2 — the thunk suppresses the original call");
        assert_eq!(eval_in_context_string("hk1", "JSON.stringify(globalThis.__seen)"),
            "[1.5,7,8,9]", "every declared param reads through the arg view, by name and by class");
        assert_eq!(HOOK_I32.lock().unwrap()[0], 42, "a mutable param is written back to the frame");
        assert!((HOOK_F32.lock().unwrap()[0] - 0.25).abs() < 1e-6, "float class is written as f32");
        assert_eq!(HOOK_I32.lock().unwrap()[1], 8, "a read-only param never reaches the engine");
        shutdown();
    }

    /// (a) A param the shape does not have reads as `undefined` and a NAMED degrade — never 0.
    /// (b) The view dies with the dispatch: a handler that stashes it reads `undefined` afterwards,
    ///     which is what keeps a dead stack frame from being read as data.
    #[test]
    fn hook_view_failures_are_named_and_the_view_is_block_scoped() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk2");
        create_plugin_context("hk2");
        eval_in_context("hk2", r#"
            globalThis.__stash = null;
            __s2_hook_on("hk2", "onX", function (v) {
                globalThis.__stash = v;
                globalThis.__u5 = v.u5;      // index 4: past this shape's params
                return HookResult.Continue;
            });
        "#).unwrap();
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);

        assert_eq!(eval_in_context_string("hk2", "String(globalThis.__u5)"), "undefined",
            "a failed read must be undefined — a 0 would be indistinguishable from a real zero");
        assert!(crate::gamedata_hooks::status("hk2", "onX").contains("param #4"),
            "and it must name the failure: {}", crate::gamedata_hooks::status("hk2", "onX"));

        assert_eq!(eval_in_context_string("hk2", "String(globalThis.__stash.delay)"), "undefined",
            "the view is block-scoped: outside its dispatch every accessor is dead");
        shutdown();
    }

    /// A SECOND hook on the same shape, so a view stashed out of one dispatch has somewhere to be
    /// misused. `onY` declares NO `mutable` params at all — every one of its args is read-only.
    fn two_hook_gamedata() -> &'static str {
        r#"{"signatures":{"Sig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48","resolve":"direct",
                          "validate":{"prologue":"55"}}},
                          "Sig2":{"linuxsteamrt64":{"module":"m.so","pattern":"55 49","resolve":"direct",
                          "validate":{"prologue":"55"}}}},
            "hooks":{"onX":{"target":{"kind":"signature","name":"Sig"},
                     "shape":"this_f32_i32_i32_i32",
                     "params":["delay","reason","u3","u4"],"mutable":["delay","reason"],
                     "expose":{"ctx":"g"}},
                     "onY":{"target":{"kind":"signature","name":"Sig2"},
                     "shape":"this_f32_i32_i32_i32",
                     "params":["delay","reason","u3","u4"],
                     "expose":{"ctx":"g"}}}}"#
    }

    /// THE VIEW IS BOUND TO ITS DISPATCH, not merely to "some dispatch".
    ///
    /// A plugin subscribes to two hooks on the same shape and stashes the view its FIRST handler
    /// received. During the SECOND hook's dispatch it writes through that stale view. Without the
    /// per-dispatch epoch the write passes the shim's bounds-and-class check against the second
    /// hook's live frame — `onY` declares no `mutable` params at all, so it would be a write past
    /// that hook's own allow-list, with no degrade and no warning. Reads confuse the same way.
    #[test]
    fn a_view_stashed_from_one_dispatch_cannot_touch_another() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        crate::loader::load_permissions_from_str(r#"{"engine:hooks":["hk4"]}"#).expect("parses");
        *HOOK_F32.lock().unwrap() = [1.5];
        *HOOK_I32.lock().unwrap() = [7, 8, 9];
        crate::gamedata_hooks::register_plugin("hk4", two_hook_gamedata());
        let x_id = crate::gamedata_hooks::plan("hk4", "onX").expect("onX ready").hook_id;
        let y_id = crate::gamedata_hooks::plan("hk4", "onY").expect("onY ready").hook_id;
        assert_ne!(x_id, y_id, "two hooks, two slots");

        create_plugin_context("hk4");
        eval_in_context("hk4", r#"
            globalThis.__stash = null;
            globalThis.__crossRead = "unset";
            __s2_hook_on("hk4", "onX", function (v) { globalThis.__stash = v; });
            __s2_hook_on("hk4", "onY", function () {
                // The stashed view belongs to onX's FINISHED dispatch. onY's frame is the live one.
                globalThis.__crossRead = String(globalThis.__stash.delay);
                globalThis.__stash.reason = 5;
            });
        "#).unwrap();

        dispatch_hook(x_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);
        assert!(eval_in_context_bool("hk4", "globalThis.__stash !== null"), "onX ran and stashed");

        // A value onY's frame carries, distinct from anything onX saw.
        *HOOK_I32.lock().unwrap() = [77, 8, 9];
        dispatch_hook(y_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);

        assert_eq!(HOOK_I32.lock().unwrap()[0], 77,
            "a view from a finished dispatch must NOT write the live frame — that is a write past \
             onY's own 'mutable' list");
        assert_eq!(eval_in_context_string("hk4", "globalThis.__crossRead"), "undefined",
            "and it must not read it either");
        let st = crate::gamedata_hooks::status("hk4", "onY");
        assert!(st.contains("FINISHED dispatch") && st.contains("REFUSED"),
            "the refusal must be NAMED against the hook whose frame was aimed at, got: {st}");
        shutdown();
    }

    /// A hook that SURFACES its receiver. `this_void` so nothing but the receiver is in play.
    fn receiver_hook_gamedata() -> &'static str {
        r#"{"signatures":{"Sig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48","resolve":"direct",
                          "validate":{"prologue":"55"}}}},
            "hooks":{"onR":{"target":{"kind":"signature","name":"Sig"},"shape":"this_void",
                     "receiver":{"kind":"entity","as":"player"},"expose":{"ctx":"g"}}}}"#
    }

    /// A surfaced receiver is a REAL `EntityRef`, not an `[index, id]` array that merely holds the
    /// same two numbers.
    ///
    /// The loud half of getting this wrong is `v.player.isValid()` throwing. The SILENT half is the
    /// one that matters: `pack_entity_arg` — the packer every `EntityRef`-typed native argument goes
    /// through — reads the NAMED `.index` and `.id`. On a bare array those live at numeric indices,
    /// so both read `undefined`, the packer computes "no entity", and a live, just-respawned player
    /// reaches an engine call looking absent with no error anywhere. Hence both assertions: the
    /// prototype (methods exist) AND the property NAMES (the packer's actual dependency).
    #[test]
    fn a_surfaced_receiver_is_a_real_entity_ref() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        crate::loader::load_permissions_from_str(r#"{"engine:hooks":["hk6"]}"#).expect("parses");
        // Seed the host's books, then hand back the handle that decodes to exactly that entity.
        let id = crate::entity_live::on_created(42, 7);
        let _receiver_reset = ResetOnDrop(&HOOK_RECEIVER, -1);
        *HOOK_RECEIVER.lock().unwrap() =
            (((7u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32) as i64;
        crate::gamedata_hooks::register_plugin("hk6", receiver_hook_gamedata());
        let hook_id = crate::gamedata_hooks::plan("hk6", "onR").expect("onR ready").hook_id;
        create_plugin_context("hk6");
        eval_in_context("hk6", r#"
            globalThis.__r = {};
            __s2_hook_on("hk6", "onR", function (v) {
                // The NAMED reads FIRST and a guarded call last, so a throwing method cannot
                // mask the silent half: a bare array reads `undefined` here without throwing.
                globalThis.__r = {
                    index:     v.player.index,          // NAMED — what pack_entity_arg reads
                    id:        String(v.player.id),     // NAMED
                    isRef:     v.player instanceof __s2pkg_entity.EntityRef,
                    hasMethod: typeof v.player.isValid === "function",
                    callable:  false,
                };
                try { globalThis.__r.callable = typeof v.player.isValid() === "boolean"; }
                catch (e) { globalThis.__r.callable = "threw: " + e.message; }
            });
        "#).unwrap();
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);

        // The SILENT half first: these two are what `pack_entity_arg` reads, and a bare array
        // reads `undefined` from both without throwing anything.
        assert_eq!(eval_in_context_string("hk6", "String(globalThis.__r.index)"), "42",
            "the NAMED .index is what every EntityRef-typed native argument is packed from");
        assert_eq!(eval_in_context_string("hk6", "globalThis.__r.id"), id.to_string(),
            "and the NAMED .id — a bare array reads `undefined` here and packs as NO_ENTITY");
        // The loud half.
        assert_eq!(eval_in_context_string("hk6", "String(globalThis.__r.isRef)"), "true",
            "the receiver must BE an EntityRef, not an array shaped like one");
        assert_eq!(eval_in_context_string("hk6", "String(globalThis.__r.hasMethod)"), "true");
        assert_eq!(eval_in_context_string("hk6", "String(globalThis.__r.callable)"), "true",
            "and its methods must actually run against the books");

        // The other answer: the shim says "not an entity" and the receiver is a plain `null`, which
        // is what `EntityRef | null` promises and what `?.` in a handler expects.
        *HOOK_RECEIVER.lock().unwrap() = -1;
        eval_in_context("hk6", r#"globalThis.__r = { isRef: "unset" };"#).unwrap();
        eval_in_context("hk6", r#"
            __s2_hook_on("hk6", "onR", function (v) { globalThis.__r = { isNull: v.player === null }; });
        "#).unwrap();
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);
        assert_eq!(eval_in_context_string("hk6", "String(globalThis.__r.isNull)"), "true");
        shutdown();
    }

    /// A value the param's class cannot represent is REFUSED, not coerced: `NaN as i32` saturates to
    /// 0, so a coerced write would hand the engine a plausible-looking zero and report success.
    #[test]
    fn a_non_representable_write_is_refused_and_named() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk5");
        create_plugin_context("hk5");
        eval_in_context("hk5", r#"
            __s2_hook_on("hk5", "onX", function (v) { v.reason = "abc"; });
        "#).unwrap();
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);
        assert_eq!(HOOK_I32.lock().unwrap()[0], 7, "NaN must not become 0 in the engine's args");
        assert!(crate::gamedata_hooks::status("hk5", "onX").contains("REFUSED, never coerced"),
            "{}", crate::gamedata_hooks::status("hk5", "onX"));
        shutdown();
    }

    /// The FLOAT half of the same rule, and the one that bites hardest because it looks like it
    /// worked. `1e300 as f32` is not a saturation, it is `f32::INFINITY`: the write "succeeds", the
    /// shim returns 0, no degrade is recorded, and the engine is handed `+inf` as a round-restart
    /// delay. A handler computing `view.delay = scale * base` and overflowing gets a silently wrong
    /// value REPORTED AS A SUCCESSFUL WRITE, which is the one failure mode this project ranks below
    /// a crash. Finiteness alone does not cover it — 1e300 is perfectly finite as an f64.
    #[test]
    fn a_float_write_outside_f32_range_is_refused_not_silently_infinite() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk7");
        create_plugin_context("hk7");
        eval_in_context("hk7", r#"
            __s2_hook_on("hk7", "onX", function (v) { v.delay = 1e300; });
        "#).unwrap();
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);

        let got = HOOK_F32.lock().unwrap()[0];
        assert!(got.is_finite(), "an out-of-f32-range write reached the engine as {got}");
        assert!((got - 1.5).abs() < 1e-6, "the engine must still see the ORIGINAL delay, got {got}");
        assert!(crate::gamedata_hooks::status("hk7", "onX").contains("REFUSED, never coerced"),
            "and the refusal must be NAMED: {}", crate::gamedata_hooks::status("hk7", "onX"));
        shutdown();
    }

    /// A write through a view that belongs to NO dispatch is a LOST WRITE, and a lost write must be
    /// loud. It cannot be a `note_miss` — the view is dead, so the hook it belonged to cannot be
    /// named from here — which is exactly why the WARN is the only signal there is. Without this
    /// test, deleting that `log_warn` leaves the suite green and makes the write silent: unlike a
    /// read (which returns a visible `undefined`), an assignment that goes nowhere looks identical
    /// to one that worked.
    #[test]
    fn a_write_through_a_view_outside_any_dispatch_is_ignored_and_warns() {
        LOG.lock().unwrap().clear();
        let _ = init(logger);
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk8");
        create_plugin_context("hk8");
        eval_in_context("hk8", r#"
            globalThis.__stash = null;
            __s2_hook_on("hk8", "onX", function (v) { globalThis.__stash = v; });
        "#).unwrap();
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);

        LOG.lock().unwrap().clear();
        // Outside the dispatch entirely: no ACTIVE_HOOK at all, which is the `Dead` arm (the
        // `Rebound` arm — a stale view during ANOTHER dispatch — is a different test).
        eval_in_context("hk8", r#"globalThis.__stash.delay = 9.5;"#).unwrap();

        assert!((HOOK_F32.lock().unwrap()[0] - 1.5).abs() < 1e-6,
            "a dead view must not write the frame it used to point at");
        let got = LOG.lock().unwrap().clone();
        assert!(got.iter().any(|m| m.contains("written outside its dispatch")),
            "the lost write must be reported — it is the only signal a caller gets: {:?}", got);
        shutdown();
    }

    /// THE CASE THE WHOLE EPOCH ARGUMENT RESTS ON: two invocations of the *same* hook.
    ///
    /// `a_view_stashed_from_one_dispatch_cannot_touch_another` proves the CROSS-hook case, which a
    /// hook id alone would also catch. This one cannot be caught by a hook id — it matches — so it
    /// is the case that says the binding token has to be per-DISPATCH. A view stashed from
    /// invocation #1 is aimed at invocation #2's live frame; both are `onX`, both are `mutable`
    /// `delay`/`reason`, and the shim's bounds-and-class check passes. Only the epoch refuses it.
    #[test]
    fn a_view_stashed_from_an_earlier_invocation_of_the_same_hook_cannot_touch_this_one() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk9");
        create_plugin_context("hk9");
        eval_in_context("hk9", r#"
            globalThis.__n = 0;
            globalThis.__stash = null;
            globalThis.__crossRead = "unset";
            __s2_hook_on("hk9", "onX", function (v) {
                globalThis.__n++;
                if (globalThis.__n === 1) { globalThis.__stash = v; return; }
                // Invocation #2. The stashed view is the SAME hook's — same slot id, same shape,
                // same `mutable` list — just a dispatch that has already finished.
                globalThis.__crossRead = String(globalThis.__stash.delay);
                globalThis.__stash.reason = 5;
            });
        "#).unwrap();

        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);
        assert!(eval_in_context_bool("hk9", "globalThis.__stash !== null"), "invocation #1 stashed");

        // A value only invocation #2's frame carries.
        *HOOK_I32.lock().unwrap() = [77, 8, 9];
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);

        assert_eq!(eval_in_context_string("hk9", "String(globalThis.__n)"), "2", "it ran twice");
        assert_eq!(HOOK_I32.lock().unwrap()[0], 77,
            "a view from invocation #1 must NOT write invocation #2's frame — a hook id would match");
        assert_eq!(eval_in_context_string("hk9", "globalThis.__crossRead"), "undefined",
            "and it must not read it either");
        let st = crate::gamedata_hooks::status("hk9", "onX");
        assert!(st.contains("FINISHED dispatch") && st.contains("REFUSED"),
            "the refusal must be NAMED: {st}");
        shutdown();
    }

    /// A `calls` descriptor and a hook on the SAME address with NO `bypassWith` between them — the
    /// case the bypass latch does NOT cover, because `bypass_ids_for_call` is scoped to (owner,
    /// call name) while SourceMod's `g_pIgnoreTerminateDetour` is global.
    fn unlatched_reentrancy_gamedata() -> &'static str {
        r#"{"signatures":{"Sig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48","resolve":"direct",
                          "validate":{"prologue":"55"}}}},
            "calls":{"aCallNoHookNames":{"receiver":{"kind":"none"},
                     "target":{"kind":"signature","name":"Sig"},"args":[],"returns":"void"}},
            "hooks":{"onX":{"target":{"kind":"signature","name":"Sig"},
                     "shape":"this_f32_i32_i32_i32",
                     "params":["delay","reason","u3","u4"],"mutable":["delay","reason"],
                     "expose":{"ctx":"g"}}}}"#
    }

    /// The hook id an invoke should re-enter, or -1. Set only for the window of one test.
    static REENTER_HOOK_ID: Mutex<i32> = Mutex::new(-1);

    /// An invoke that actually REACHES the hooked function, so the detour fires from inside JS —
    /// i.e. while core holds the isolate borrow. `mock_call_invoke_degrades` cannot model this: it
    /// returns without calling anything.
    #[allow(clippy::too_many_arguments)]
    extern "C" fn mock_call_invoke_reenters_a_hook(
        _id: c_int, _ei: c_int, _es: c_int, _so: c_int,
        _gp: *const u64, _gk: *const u8, _gc: c_int,
        _fp: *const f64, _fc: c_int,
        _s: *const *const c_char, _v: *const f32,
        _rk: c_int, _ro: *mut u64,
    ) -> c_int {
        let id = *REENTER_HOOK_ID.lock().unwrap();
        if id >= 0 {
            dispatch_hook(id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);
        }
        1
    }

    /// A hook that fires from inside a JS `Engine.call` runs — the outbound native published a
    /// nest token, so `fan_out_inner` uses CallbackScope and does not take HOST.
    #[test]
    fn a_reentrant_hook_dispatch_from_engine_call_runs() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps {
            engine_call_invoke: Some(mock_call_invoke_reenters_a_hook),
            ..hook_test_ops()
        }));
        crate::loader::load_permissions_from_str(
            r#"{"engine:calls":["hk10"],"engine:hooks":["hk10"]}"#).expect("parses");
        *HOOK_F32.lock().unwrap() = [1.5];
        *HOOK_I32.lock().unwrap() = [7, 8, 9];
        crate::gamedata_calls::register_plugin("hk10", unlatched_reentrancy_gamedata());
        crate::gamedata_hooks::register_plugin("hk10", unlatched_reentrancy_gamedata());
        let hook_id = crate::gamedata_hooks::plan("hk10", "onX").expect("ready").hook_id;
        create_plugin_context("hk10");
        eval_in_context("hk10", r#"
            globalThis.__ran = 0;
            __s2_hook_on("hk10", "onX", function () { globalThis.__ran++; });
        "#).unwrap();
        assert_eq!(crate::gamedata_hooks::status("hk10", "onX"), "available",
            "nothing has gone wrong yet");

        let _reenter_reset = ResetOnDrop(&REENTER_HOOK_ID, -1);
        *REENTER_HOOK_ID.lock().unwrap() = hook_id;
        eval_in_context("hk10", r#"__s2_engine_call_invoke("aCallNoHookNames", -1, 0, []);"#).unwrap();
        *REENTER_HOOK_ID.lock().unwrap() = -1;

        assert_eq!(eval_in_context_string("hk10", "String(globalThis.__ran)"), "1",
            "JS Engine.call must run other plugins' hooks before it returns");
        shutdown();
    }

    /// Same hook already on the stack (give from onCanAcquire) is skip-and-named, not nested.
    #[test]
    fn same_hook_reentry_is_skipped_and_named() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps {
            engine_call_invoke: Some(mock_call_invoke_reenters_a_hook),
            ..hook_test_ops()
        }));
        crate::loader::load_permissions_from_str(
            r#"{"engine:calls":["hk11"],"engine:hooks":["hk11"]}"#).expect("parses");
        *HOOK_F32.lock().unwrap() = [1.5];
        *HOOK_I32.lock().unwrap() = [7, 8, 9];
        crate::gamedata_calls::register_plugin("hk11", unlatched_reentrancy_gamedata());
        crate::gamedata_hooks::register_plugin("hk11", unlatched_reentrancy_gamedata());
        let hook_id = crate::gamedata_hooks::plan("hk11", "onX").expect("ready").hook_id;
        create_plugin_context("hk11");
        eval_in_context("hk11", r#"
            globalThis.__ran = 0;
            __s2_hook_on("hk11", "onX", function () {
                globalThis.__ran++;
                __s2_engine_call_invoke("aCallNoHookNames", -1, 0, []);
            });
        "#).unwrap();

        let _reenter_reset = ResetOnDrop(&REENTER_HOOK_ID, -1);
        *REENTER_HOOK_ID.lock().unwrap() = hook_id;
        // Engine-originated inbound (no nest token): HOST is free, handler runs, then the
        // inner Engine.call re-enters the SAME hook and must skip.
        dispatch_hook(hook_id, HOOK_VIEW_TOKEN as *mut std::ffi::c_void);
        *REENTER_HOOK_ID.lock().unwrap() = -1;

        assert_eq!(eval_in_context_string("hk11", "String(globalThis.__ran)"), "1",
            "outer dispatch runs once; inner same-hook give is skipped");
        let st = crate::gamedata_hooks::status("hk11", "onX");
        assert!(st.contains("re-entrant") && st.contains("UNHOOKED"),
            "same-hook skip must be NAMED: {st}");
        shutdown();
    }

    /// The bypass latch brackets the outbound invoke and is DISARMED even when that invoke never
    /// reaches the hooked function — the leak that would otherwise swallow the next genuine
    /// engine-driven call (spec §10).
    #[test]
    fn the_bypass_latch_is_armed_and_disarmed_around_a_degrading_invoke() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(hook_test_ops()));
        let hook_id = hook_test_setup("hk3");
        create_plugin_context("hk3");

        // Not installed yet -> nothing to arm: an uninstalled slot has no thunk to take the latch.
        eval_in_context("hk3", r#"__s2_engine_call_invoke("doThing", -1, 0, []);"#).unwrap();
        assert!(HOOK_ARMED.lock().unwrap().is_empty(), "no subscriber, no detour, no latch");

        eval_in_context("hk3", r#"__s2_hook_on("hk3", "onX", function () {});"#).unwrap();
        eval_in_context("hk3", r#"__s2_engine_call_invoke("doThing", -1, 0, []);"#).unwrap();

        assert_eq!(*HOOK_ARMED.lock().unwrap(), vec![hook_id], "our own call arms its hook's latch");
        assert_eq!(*HOOK_DISARMED.lock().unwrap(), vec![hook_id],
            "and disarms it even though the invoke DEGRADED — the thunk never ran to take it");

        // An ops table with arm but NO disarm (an older shim) must not arm at all. Arming without a
        // disarm is strictly worse than not arming: losing the "our own call does not fire our own
        // hook" semantic costs one spurious dispatch, a stuck latch silently swallows a genuine one.
        set_engine_ops(Some(S2EngineOps { hook_disarm_bypass: None, ..hook_test_ops() }));
        HOOK_ARMED.lock().unwrap().clear();
        eval_in_context("hk3", r#"__s2_engine_call_invoke("doThing", -1, 0, []);"#).unwrap();
        assert!(HOOK_ARMED.lock().unwrap().is_empty(),
            "with no way to disarm, the latch must not be armed");
        shutdown();
    }

    /// Entity-creation lifecycle slice: `spawn`/`teleport`/`remove` on a synthetic `EntityRef` all
    /// degrade to `false` with no engine ops wired.
    #[test]
    fn entity_lifecycle_methods_degrade_to_false_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
            [r.spawn(), r.teleport([0,0,0]), r.teleport([0,0,0],null,null), r.remove()].join(",")
        "#);
        assert_eq!(out, "false,false,false,false");
        shutdown();
    }

    /// EKV slice: `spawn(kv)` degrades to `false` with no `entity_spawn_kv` op; `createEntity(cls, kv)`
    /// degrades to `null` with no `entity_create` op.
    #[test]
    fn entity_spawn_kv_degrades_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
            const a = r.spawn({ health: 42 });                       // no op -> false
            const b = __s2pkg_entity.createEntity("x", { a: 1 });    // no entity_create op -> null
            [String(a), String(b)].join("|")
        "#);
        assert_eq!(out, "false|null");
        shutdown();
    }

    /// EKV slice: marshal rejections return false BEFORE any op call (bad value type, empty key,
    /// non-finite number); {} and omitted kv take the plain entity_spawn path.
    #[test]
    fn entity_spawn_kv_marshal_rejects_bad_input() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
            [
                String(r.spawn({ o: {} })),          // object value -> false
                String(r.spawn({ "": 1 })),          // empty key -> false
                String(r.spawn({ n: NaN })),         // non-finite -> false
                String(r.spawn({ n: Infinity })),    // non-finite -> false
                String(r.spawn({})),                 // empty map -> plain spawn path (no op -> false)
                String(r.spawn())                    // omitted -> plain spawn path (no op -> false)
            ].join(",")
        "#);
        assert_eq!(out, "false,false,false,false,false,false");
        shutdown();
    }

    // Test-only capture buffer for the entity_spawn_kv marshal-capture test below (shared across
    // this one test; safe because RUST_TEST_THREADS=1).
    static EKV_CAPTURE: Mutex<Vec<String>> = Mutex::new(Vec::new());

    // Fake entity_spawn_kv op: records "key:type:value" triples (joined "|") into EKV_CAPTURE and
    // returns 1 (success) — proves the JS marshal produces the exact parallel arrays the shim expects.
    extern "C" fn capture_spawn_kv(_index: c_int, _serial: c_int, count: c_int,
        keys: *const *const c_char, types: *const c_int, values: *const *const c_char) -> c_int {
        let n = count as usize;
        let mut parts: Vec<String> = Vec::with_capacity(n);
        unsafe {
            for i in 0..n {
                let k = CStr::from_ptr(*keys.add(i)).to_string_lossy().into_owned();
                let t = *types.add(i);
                let v = CStr::from_ptr(*values.add(i)).to_string_lossy().into_owned();
                parts.push(format!("{}:{}:{}", k, t, v));
            }
        }
        EKV_CAPTURE.lock().unwrap().push(parts.join("|"));
        1
    }

    /// EKV slice: `{name:"bob", health:42, scale:1.5, enabled:true, big:3000000000}` crosses as types
    /// `[string,int,float,bool,float]` with values `["bob","42","1.5","1","3000000000"]` (int32
    /// overflow -> float tag), and the native returns `true` (fake op returns 1). Key ORDER is
    /// `Object.keys` insertion order, deterministic.
    #[test]
    fn entity_spawn_kv_marshal_capture_matches_expected_arrays() {
        crate::entity_live::reset_for_tests();
        EKV_CAPTURE.lock().unwrap().clear();
        let _ = init(dummy_logger());
        crate::entity_live::on_created(1, 7);          // seed the books so (1, id) resolves
        set_engine_ops(Some(S2EngineOps { entity_spawn_kv: Some(capture_spawn_kv), ..mock_event_ops() }));
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, __s2_ent_id_for_index(1));
            String(r.spawn({ name: "bob", health: 42, scale: 1.5, enabled: true, big: 3000000000 }))
        "#);
        assert_eq!(out, "true");
        assert_eq!(
            EKV_CAPTURE.lock().unwrap().last().unwrap().as_str(),
            "name:0:bob|health:1:42|scale:2:1.5|enabled:3:1|big:2:3000000000"
        );
        shutdown();
    }

    /// EKV slice (review fix): a string key OR value at/beyond EKV_MAX_STRING_LEN (1024) rejects
    /// the WHOLE map (`spawn` returns `false`, no crash) BEFORE any op call — guards the real
    /// live-confirmed abort in CKV3Arena's CUtlMemoryBlockAllocator::AddPage() at its ~2048-byte
    /// MaxPossiblePageSize() bound (2000B keyvalue strings are fine; 2050B reliably aborted the
    /// whole server process). Proven with the fake op wired: the capture buffer stays UNTOUCHED
    /// for the oversized calls (the marshal rejected before reaching the native/op at all), while
    /// a normal-length value in the same test still reaches it — isolating "rejected by marshal"
    /// from "no op wired".
    #[test]
    fn entity_spawn_kv_marshal_rejects_oversized_strings() {
        crate::entity_live::reset_for_tests();
        EKV_CAPTURE.lock().unwrap().clear();
        let _ = init(dummy_logger());
        crate::entity_live::on_created(1, 7);          // seed the books so the normal-length spawn resolves
        set_engine_ops(Some(S2EngineOps { entity_spawn_kv: Some(capture_spawn_kv), ..mock_event_ops() }));
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, __s2_ent_id_for_index(1));
            const big = "x".repeat(2050);   // beyond the real ~2048-byte engine abort bound
            const cjk = "字".repeat(500); // .length 500 (UNDER the JS .length cap) but 1500 UTF-8 bytes
            const ok  = "x".repeat(100);    // comfortably under the cap
            [
                String(r.spawn({ message: big })),   // oversized ASCII VALUE -> rejected by the JS .length cap
                String(r.spawn({ [big]: 1 })),        // oversized KEY -> rejected by the JS .length cap
                String(r.spawn({ message: cjk })),    // multibyte VALUE: passes .length cap, rejected by the NATIVE byte guard
                String(r.spawn({ message: ok }))      // normal-length value -> reaches the fake op -> true
            ].join(",")
        "#);
        assert_eq!(out, "false,false,false,true");
        assert_eq!(EKV_CAPTURE.lock().unwrap().len(), 1, "only the normal-length spawn should have reached the op");
        shutdown();
    }

    static SOUND_EMIT_CALLS: std::sync::Mutex<Vec<(String, i32, i32, Vec<i32>, f32)>> =
        std::sync::Mutex::new(Vec::new());
    extern "C" fn mock_sound_emit(name: *const c_char, ent_index: c_int, ent_serial: c_int,
                                  slots: *const c_int, slot_count: c_int, volume: f32) -> c_int {
        let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
        let s = if slots.is_null() || slot_count <= 0 { Vec::new() }
                else { unsafe { std::slice::from_raw_parts(slots, slot_count as usize) }.to_vec() };
        SOUND_EMIT_CALLS.lock().unwrap().push((n, ent_index, ent_serial, s, volume));
        7   // a fake nonzero guid
    }

    /// __s2_sound_emit marshals (name, entIndex, entSerial, slots[], volume) into the op and
    /// returns its guid (struct-update over mock_event_ops, the entity_spawn_kv capture precedent).
    #[test]
    fn sound_emit_marshals_args_to_op() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        SOUND_EMIT_CALLS.lock().unwrap().clear();
        let id = crate::entity_live::on_created(42, 99);   // books: index 42 → engine serial 99
        set_engine_ops(Some(S2EngineOps { sound_emit: Some(mock_sound_emit), ..mock_event_ops() }));
        create_plugin_context("psm");
        // arg 2 is now the host-id; the native translates it to the engine serial 99 the op captures.
        let out = eval_in_context_string("psm",
            &format!("String(__s2_sound_emit('Weapon_AK47.Single', 42, {id}, [3, 5], 0.5))"));
        assert_eq!(out, "7");
        let calls = SOUND_EMIT_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Weapon_AK47.Single");
        assert_eq!(calls[0].1, 42);
        assert_eq!(calls[0].2, 99);
        assert_eq!(calls[0].3, vec![3, 5]);
        assert!((calls[0].4 - 0.5).abs() < 1e-6);
        shutdown();
    }

    /// @s2script/sound module surface (defaults): no entity -> worldspawn (0, -1); no recipients ->
    /// the all-valid-clients enumeration (client_valid is None under mock_event_ops -> empty ->
    /// the op still receives slotCount 0); volume defaults 1.0.
    #[test]
    fn sound_module_emit_defaults() {
        let _ = init(dummy_logger());
        SOUND_EMIT_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(S2EngineOps { sound_emit: Some(mock_sound_emit), ..mock_event_ops() }));
        create_plugin_context("psd");
        let out = eval_in_context_string("psd",
            "String(__s2pkg_sound.Sound.emit('Weapon_AK47.Single'))");
        assert_eq!(out, "7");
        let calls = SOUND_EMIT_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, 0);                 // worldspawn index
        assert_eq!(calls[0].2, -1);                // the no-serial-check sentinel
        assert_eq!(calls[0].3, Vec::<i32>::new()); // no valid clients under the mock ops
        assert!((calls[0].4 - 1.0).abs() < 1e-6);  // default volume
        shutdown();
    }

    /// @s2script/sound module surface (explicit opts): entity {index,serial} -> (idx, serial);
    /// recipients passed through; volume passed through. And the module resolves via require.
    #[test]
    fn sound_module_emit_explicit_opts() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        SOUND_EMIT_CALLS.lock().unwrap().clear();
        crate::entity_live::on_created(42, 99);        // books: index 42 → engine serial 99
        set_engine_ops(Some(S2EngineOps { sound_emit: Some(mock_sound_emit), ..mock_event_ops() }));
        load_body("psx", r#"
            const { Sound } = require("@s2script/sound");
            globalThis.__g = Sound.emit("UI.PlayerPing",
                { entity: { index: 42, id: __s2_ent_id_for_index(42) }, recipients: [3, 5], volume: 0.5 });
        "#, "{}");
        assert_eq!(eval_in_context_string("psx", "String(globalThis.__g)"), "7");
        let calls = SOUND_EMIT_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "UI.PlayerPing");
        assert_eq!(calls[0].1, 42);
        assert_eq!(calls[0].2, 99);
        assert_eq!(calls[0].3, vec![3, 5]);
        assert!((calls[0].4 - 0.5).abs() < 1e-6);
        shutdown();
    }

    /// Sound.onPrecache wraps the raw subscribe: the handler receives a ctx whose add() hits the
    /// (absent) op and returns false; the ctx is freshly built per dispatch.
    #[test]
    fn sound_module_onprecache_builds_ctx() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("ppx");
        eval_in_context_string("ppx", r#"
            globalThis.__ctxAdd = null;
            __s2pkg_sound.Sound.onPrecache(function (ctx) {
                globalThis.__ctxAdd = ctx.add("soundevents/y.vsndevts");
            });
            "ok"
        "#);
        dispatch_precache();
        assert_eq!(eval_in_context_string("ppx", "String(globalThis.__ctxAdd)"), "false");
        shutdown();
    }

    // --- checktransmit slice: __s2_transmit_* natives + the per-plugin rule store ---
    static TRANSMIT_SET_CALLS: std::sync::Mutex<Vec<(i32, i32, u64)>> = std::sync::Mutex::new(Vec::new());
    static TRANSMIT_CLEAR_CALLS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
    extern "C" fn mock_transmit_set(index: c_int, serial: c_int, mask: u64) -> c_int {
        TRANSMIT_SET_CALLS.lock().unwrap().push((index, serial, mask));
        1
    }
    extern "C" fn mock_transmit_set_reject(_index: c_int, _serial: c_int, _mask: u64) -> c_int { 0 }
    extern "C" fn mock_transmit_clear(index: c_int) -> c_int {
        TRANSMIT_CLEAR_CALLS.lock().unwrap().push(index);
        1
    }
    extern "C" fn mock_transmit_stats(out: *mut u64) {
        unsafe { for i in 0..5 { *out.add(i) = (i as u64 + 1) * 10; } }
    }
    // --- client command execution (SourceMod ClientCommand / FakeClientCommand parity) ---
    pub(crate) static CLIENT_CMD_CALLS: std::sync::Mutex<Vec<(i32, String)>> = std::sync::Mutex::new(Vec::new());

    pub(crate) extern "C" fn mock_client_command(slot: c_int, cmd: *const c_char) -> c_int {
        let s = unsafe { std::ffi::CStr::from_ptr(cmd) }.to_string_lossy().into_owned();
        CLIENT_CMD_CALLS.lock().unwrap().push((slot, s));
        1
    }
    pub(crate) static FAKE_CMD_CALLS: std::sync::Mutex<Vec<(i32, String)>> = std::sync::Mutex::new(Vec::new());

    pub(crate) extern "C" fn mock_client_fake_command(slot: c_int, cmd: *const c_char) -> c_int {
        let s = unsafe { std::ffi::CStr::from_ptr(cmd) }.to_string_lossy().into_owned();
        FAKE_CMD_CALLS.lock().unwrap().push((slot, s));
        1
    }



    /// Reproduces the ENGINE ROUND TRIP: the real shim's fakeCommand reaches
    /// `ICvar::DispatchConCommand`, which calls our ConCommand trampoline, which re-enters
    /// `dispatch_concommand`. This mock does the same, so the re-entrancy behaviour is testable
    /// without a server.
    extern "C" fn mock_fake_command_roundtrip(slot: c_int, cmd: *const c_char) -> c_int {
        let s = unsafe { std::ffi::CStr::from_ptr(cmd) }.to_string_lossy().into_owned();
        FAKE_CMD_CALLS.lock().unwrap().push((slot, s.clone()));
        let name = s.split(' ').next().unwrap_or("").to_string();
        let args = s.splitn(2, ' ').nth(1).unwrap_or("").to_string();
        dispatch_concommand(&name, slot, &args, ReplySource::Console);
        1
    }
    fn roundtrip_ops() -> S2EngineOps {
        S2EngineOps { client_fake_command: Some(mock_fake_command_roundtrip), ..mock_event_ops() }
    }

    /// fakeCommand from JS publishes a nest token, so the target plugin's command handler runs.
    #[test]
    fn fake_command_runs_the_target_plugin_command() {
        let _ = init(dummy_logger());
        FAKE_CMD_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(roundtrip_ops()));
        load_body("p", r#"
            globalThis.__ran = 0;
            __s2_concommand("s2_target", function () { globalThis.__ran++; }, -1);
        "#, "{}");
        eval_in_context_string("p", r#"new __s2pkg_clients.Client(0).fakeCommand("s2_target"); ''"#);
        assert_eq!(FAKE_CMD_CALLS.lock().unwrap().len(), 1,
            "the op IS reached — the engine really is asked to dispatch");
        assert_eq!(read_i32_global_in("p", "__ran"), 1,
            "the target command handler runs before fakeCommand returns");
        shutdown();
    }

    /// fakeCommand from inside a command handler also nests (board-wide composition).
    #[test]
    fn fake_command_from_inside_a_command_handler_runs() {
        let _ = init(dummy_logger());
        FAKE_CMD_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(roundtrip_ops()));
        load_body("p", r#"
            globalThis.__ran = 0;
            __s2_concommand("s2_target", function () { globalThis.__ran++; }, -1);
            __s2_concommand("s2_outer", function () {
              new __s2pkg_clients.Client(0).fakeCommand("s2_target");
            }, -1);
        "#, "{}");
        dispatch_concommand("s2_outer", -1, "", ReplySource::Server);
        assert_eq!(FAKE_CMD_CALLS.lock().unwrap().len(), 1, "the op is still called");
        assert_eq!(read_i32_global_in("p", "__ran"), 1,
            "nested command handler runs, not skipped");
        shutdown();
    }

    /// With no op wired (an older shim) it reports false rather than pretending it dispatched.
    #[test]
    fn fake_command_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(mock_event_ops()));
        create_plugin_context("fc3");
        assert_eq!(eval_in_context_string("fc3",
            r#"String(new __s2pkg_clients.Client(0).fakeCommand("sm_help"))"#), "false");
        shutdown();
    }





    fn transmit_test_ops() -> S2EngineOps {
        S2EngineOps {
            transmit_set: Some(mock_transmit_set),
            transmit_clear: Some(mock_transmit_clear),
            transmit_stats: Some(mock_transmit_stats),
            ..mock_event_ops()
        }
    }

    /// setVisibleTo folds the viewer-slot array into a u64 mask and pushes (index, serial, mask).
    #[test]
    fn transmit_set_folds_viewer_slots_into_mask() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        crate::entity_live::on_created(7, 42);         // books: index 7 → engine serial 42
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tm1");
        let out = eval_in_context_string("tm1",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 7, id: __s2_ent_id_for_index(7)}, [0, 5, 63]))");
        assert_eq!(out, "true");
        let calls = TRANSMIT_SET_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (7, 42, 1u64 | (1u64 << 5) | (1u64 << 63)));
        drop(calls);
        shutdown();
    }

    /// Empty viewer array = hidden from everyone = mask 0.
    #[test]
    fn transmit_set_empty_array_masks_zero() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        crate::entity_live::on_created(3, 1);          // books: index 3 → engine serial 1
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tm2");
        let out = eval_in_context_string("tm2",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 3, id: __s2_ent_id_for_index(3)}, []))");
        assert_eq!(out, "true");
        assert_eq!(TRANSMIT_SET_CALLS.lock().unwrap()[0], (3, 1, 0u64));
        shutdown();
    }

    /// A slot outside [0,64) throws RangeError from the JS wrapper (programmer error, not staleness).
    #[test]
    fn transmit_set_out_of_range_slot_throws() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tm3");
        let out = eval_in_context_string("tm3",
            "(function(){ try { __s2pkg_transmit.Transmit.setVisibleTo({index:1,id:1},[64]); return 'no-throw'; } catch (e) { return e.constructor.name; } })()");
        assert_eq!(out, "RangeError");
        assert_eq!(TRANSMIT_SET_CALLS.lock().unwrap().len(), 0);
        shutdown();
    }

    /// Two plugins with rules on the same (index, serial) AND-merge: the pushed mask is the intersection.
    #[test]
    fn transmit_rules_and_merge_across_plugins() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        crate::entity_live::on_created(5, 9);          // books: index 5 → engine serial 9 (both owners share it)
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tma");
        create_plugin_context("tmb");
        eval_in_context_string("tma",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 5, id: __s2_ent_id_for_index(5)}, [0, 1]))");
        eval_in_context_string("tmb",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 5, id: __s2_ent_id_for_index(5)}, [1, 2]))");
        let calls = TRANSMIT_SET_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], (5, 9, 0b11u64));          // tma alone
        assert_eq!(calls[1], (5, 9, 0b10u64));          // tma AND tmb = bit 1 only
        drop(calls);
        shutdown();
    }

    /// reset() removes only the caller's rule; the remaining merge is re-pushed; the LAST reset clears.
    #[test]
    fn transmit_reset_recomputes_then_clears() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        TRANSMIT_CLEAR_CALLS.lock().unwrap().clear();
        crate::entity_live::on_created(5, 9);          // books: index 5 → engine serial 9
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tra");
        create_plugin_context("trb");
        eval_in_context_string("tra", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, id: __s2_ent_id_for_index(5)}, [0, 1])");
        eval_in_context_string("trb", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, id: __s2_ent_id_for_index(5)}, [1, 2])");
        let out = eval_in_context_string("tra", "String(__s2pkg_transmit.Transmit.reset({index: 5, id: __s2_ent_id_for_index(5)}))");
        assert_eq!(out, "true");
        assert_eq!(TRANSMIT_SET_CALLS.lock().unwrap().last().copied(), Some((5, 9, 0b110u64))); // trb alone
        let out = eval_in_context_string("trb", "String(__s2pkg_transmit.Transmit.reset({index: 5, id: __s2_ent_id_for_index(5)}))");
        assert_eq!(out, "true");
        assert_eq!(TRANSMIT_CLEAR_CALLS.lock().unwrap().as_slice(), &[5]);
        shutdown();
    }

    /// reset() with a serial that doesn't match the recorded rule returns false and pushes nothing.
    #[test]
    fn transmit_reset_serial_mismatch_is_false() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        TRANSMIT_CLEAR_CALLS.lock().unwrap().clear();
        let id = crate::entity_live::on_created(5, 9);
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("trm");
        eval_in_context_string("trm", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, id: __s2_ent_id_for_index(5)}, [0])");
        // reset with a STALE id (never minted) — the books say not-live, so reset is false and clears nothing.
        let out = eval_in_context_string("trm", &format!("String(__s2pkg_transmit.Transmit.reset({{index: 5, id: {}}}))", id + 1000));
        assert_eq!(out, "false");
        assert_eq!(TRANSMIT_CLEAR_CALLS.lock().unwrap().len(), 0);
        shutdown();
    }

    /// Unloading a plugin clears its rules (the ledger walk): last owner gone -> transmit_clear pushed.
    #[test]
    fn transmit_unload_clears_owner_rules() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        TRANSMIT_CLEAR_CALLS.lock().unwrap().clear();
        crate::entity_live::on_created(11, 2);         // books: index 11 → engine serial 2
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tun");
        eval_in_context_string("tun", "__s2pkg_transmit.Transmit.setVisibleTo({index: 11, id: __s2_ent_id_for_index(11)}, [0])");
        unload_plugin("tun");
        assert_eq!(TRANSMIT_CLEAR_CALLS.lock().unwrap().as_slice(), &[11]);
        shutdown();
    }

    /// A new rule with a NEWER live serial evicts other owners' stale-serial entries on the same index
    /// (the op validated the new serial is the live one, so the old one is a dead entity's rule).
    #[test]
    fn transmit_stale_serial_evicted_on_new_set() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        let ida = crate::entity_live::on_created(5, 1);      // books: index 5 → serial 1 (tsa's live entity)
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tsa");
        create_plugin_context("tsb");
        eval_in_context_string("tsa", &format!("__s2pkg_transmit.Transmit.setVisibleTo({{index: 5, id: {ida}}}, [0])"));
        let idb = crate::entity_live::on_created(5, 2);      // slot 5 reused (serial 2) — invalidates ida in the books
        eval_in_context_string("tsb", &format!("__s2pkg_transmit.Transmit.setVisibleTo({{index: 5, id: {idb}}}, [1])"));
        let calls = TRANSMIT_SET_CALLS.lock().unwrap();
        // Second push must NOT be ANDed with tsa's stale-serial mask.
        assert_eq!(calls[1], (5, 2, 1u64 << 1));
        drop(calls);
        // And tsa's stale entry is gone (evicted) + its ref is now dead: resetting it reports false.
        let out = eval_in_context_string("tsa", &format!("String(__s2pkg_transmit.Transmit.reset({{index: 5, id: {ida}}}))"));
        assert_eq!(out, "false");
        shutdown();
    }

    /// Missing ops (old shim) degrade to false — never a throw.
    #[test]
    fn transmit_set_missing_op_degrades_false() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        crate::entity_live::on_created(1, 1);      // live ref, so we reach the (absent) op
        set_engine_ops(Some(mock_event_ops()));   // no transmit ops
        create_plugin_context("tmo");
        let out = eval_in_context_string("tmo",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 1, id: __s2_ent_id_for_index(1)}, [0]))");
        assert_eq!(out, "false");
        shutdown();
    }

    /// The op rejecting (stale ref / full table / disabled) -> false, and the rule is NOT recorded.
    #[test]
    fn transmit_set_op_reject_not_recorded() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps {
            transmit_set: Some(mock_transmit_set_reject),
            transmit_clear: Some(mock_transmit_clear),
            transmit_stats: Some(mock_transmit_stats),
            ..mock_event_ops()
        }));
        crate::entity_live::reset_for_tests();
        crate::entity_live::on_created(1, 1);      // live ref, so we reach the rejecting op
        create_plugin_context("trj");
        let out = eval_in_context_string("trj",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 1, id: __s2_ent_id_for_index(1)}, [0]))");
        assert_eq!(out, "false");
        let out = eval_in_context_string("trj", "String(__s2pkg_transmit.Transmit.reset({index: 1, id: __s2_ent_id_for_index(1)}))");
        assert_eq!(out, "false");   // nothing was recorded
        shutdown();
    }

    /// stats() surfaces the op's out[5] as a plain numbers object.
    #[test]
    fn transmit_stats_surfaces_counters() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tst");
        let out = eval_in_context_string("tst", "JSON.stringify(__s2pkg_transmit.Transmit.stats())");
        assert_eq!(out, r#"{"snapshots":10,"entries":20,"bitsCleared":30,"nsLast":40,"nsMax":50}"#);
        shutdown();
    }

    /// stats() with no op -> null (typed TransmitStats | null).
    #[test]
    fn transmit_stats_missing_op_is_null() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(mock_event_ops()));
        create_plugin_context("tsn");
        let out = eval_in_context_string("tsn", "String(__s2pkg_transmit.Transmit.stats())");
        assert_eq!(out, "null");
        shutdown();
    }

    // --- voice hearability slice: the VOICE_RULES policy store + its AND merge ---

    #[test]
    fn voice_rules_and_merge_across_owners() {
        // Two owners restricting the same sender -> the shim sees the INTERSECTION.
        voice_rules_clear_for_test();
        voice_set_rule_for_test("@a/one", 3, 0b0111);
        voice_set_rule_for_test("@b/two", 3, 0b0110);
        assert_eq!(voice_merged_for_test(3), Some(0b0110));
    }

    #[test]
    fn voice_owner_teardown_recomputes() {
        voice_rules_clear_for_test();
        voice_set_rule_for_test("@a/one", 3, 0b0111);
        voice_set_rule_for_test("@b/two", 3, 0b0110);
        voice_remove_owner("@b/two");
        assert_eq!(voice_merged_for_test(3), Some(0b0111), "the survivor's rule stands alone");
        voice_remove_owner("@a/one");
        assert_eq!(voice_merged_for_test(3), None, "no owners -> no rule at all");
    }

    #[test]
    fn voice_empty_receiver_list_is_a_rule_not_an_absence() {
        // mask 0 WITH a rule = audible to nobody. Distinct from None = engine decides.
        voice_rules_clear_for_test();
        voice_set_rule_for_test("@a/one", 5, 0);
        assert_eq!(voice_merged_for_test(5), Some(0));
    }

    // --- voice hearability: the in-isolate __s2pkg_voice.Voice surface ---
    static VOICE_SET_CALLS: std::sync::Mutex<Vec<(i32, u64)>> = std::sync::Mutex::new(Vec::new());

    extern "C" fn voice_fake_set(sender: c_int, mask: u64) -> c_int {
        VOICE_SET_CALLS.lock().unwrap().push((sender, mask));
        1
    }

    /// Ops with ONLY voice_audible_set wired — everything else stays as `mock_event_ops` leaves it
    /// (None), which is also what proves the stats native reports ABSENT rather than zero.
    fn voice_test_ops() -> S2EngineOps {
        S2EngineOps {
            voice_audible_set: Some(voice_fake_set),
            ..mock_event_ops()
        }
    }

    static VOICE_CLEAR_CALLS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

    extern "C" fn voice_fake_clear(sender: c_int) -> c_int {
        VOICE_CLEAR_CALLS.lock().unwrap().push(sender);
        1
    }

    /// A set op that REJECTS everything, mirroring a degraded / hook-not-installed shim.
    extern "C" fn voice_rejecting_set(sender: c_int, mask: u64) -> c_int {
        VOICE_SET_CALLS.lock().unwrap().push((sender, mask));
        0
    }

    /// Both ops wired, so teardown and slot-clear paths can be observed.
    fn voice_test_ops_full() -> S2EngineOps {
        S2EngineOps {
            voice_audible_set: Some(voice_fake_set),
            voice_audible_clear: Some(voice_fake_clear),
            ..mock_event_ops()
        }
    }

    /// setAudibleTo folds the receiver-slot array into a u64 mask and pushes (sender, mask).
    #[test]
    fn voice_set_audible_to_folds_receiver_slots_into_mask() {
        let _ = init(dummy_logger());
        VOICE_SET_CALLS.lock().unwrap().clear();
        voice_rules_clear_for_test();
        set_engine_ops(Some(voice_test_ops()));
        create_plugin_context("vc1");
        let out = eval_in_context_string("vc1",
            "String(__s2pkg_voice.Voice.setAudibleTo(3, [0, 5, 63]))");
        assert_eq!(out, "true");
        let calls = VOICE_SET_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (3, 1u64 | (1u64 << 5) | (1u64 << 63)));
        drop(calls);
        shutdown();
    }

    /// An old shim must report ABSENT, not zero — zero would read as "working, nothing happened".
    #[test]
    fn voice_stats_is_null_without_the_op() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(mock_event_ops()));      // voice_audible_stats stays None
        create_plugin_context("vc2");
        let out = eval_in_context_string("vc2", "String(__s2pkg_voice.Voice.stats())");
        assert_eq!(out, "null");
        shutdown();
    }

    /// The JS wrapper rejects a non-array before it can reach the native.
    #[test]
    fn voice_set_audible_to_rejects_a_non_array() {
        let _ = init(dummy_logger());
        VOICE_SET_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(voice_test_ops()));
        create_plugin_context("vc3");
        let out = eval_in_context_string("vc3",
            "(function(){ try { __s2pkg_voice.Voice.setAudibleTo(0, 5); return 'no-throw'; } \
              catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()");
        assert_eq!(out, "TypeError");
        assert_eq!(VOICE_SET_CALLS.lock().unwrap().len(), 0, "the native must never be reached");
        shutdown();
    }

    /// A REJECTED push must leave VOICE_RULES untouched. This is the push-then-persist invariant the
    /// plan review caught; a mutation test proved it had zero coverage (swapping the order kept the
    /// whole suite green), so it is asserted here directly.
    #[test]
    fn voice_rejected_push_does_not_persist_the_rule() {
        let _ = init(dummy_logger());
        VOICE_SET_CALLS.lock().unwrap().clear();
        voice_rules_clear_for_test();
        set_engine_ops(Some(S2EngineOps {
            voice_audible_set: Some(voice_rejecting_set),
            ..mock_event_ops()
        }));
        create_plugin_context("vr1");
        let out = eval_in_context_string("vr1", "String(__s2pkg_voice.Voice.setAudibleTo(4, [1]))");
        assert_eq!(out, "false", "a rejecting op must surface as false, not a silent success");
        assert_eq!(VOICE_SET_CALLS.lock().unwrap().len(), 1, "the push was attempted");
        assert_eq!(voice_merged_for_test(4), None,
            "core must NOT hold a rule the shim rejected — that is the state divergence this guards");
        shutdown();
    }

    /// Two plugin contexts restricting the same sender must AND-merge. The native inlines its own
    /// merge loop separate from voice_merged(), and a mutation test showed flipping it to OR (letting
    /// one plugin WIDEN another's restriction — spec criterion 3) kept the suite green.
    #[test]
    fn voice_two_owners_and_merge_through_the_native() {
        let _ = init(dummy_logger());
        VOICE_SET_CALLS.lock().unwrap().clear();
        voice_rules_clear_for_test();
        set_engine_ops(Some(voice_test_ops()));
        create_plugin_context("vm1");
        create_plugin_context("vm2");
        eval_in_context_string("vm1", "__s2pkg_voice.Voice.setAudibleTo(6, [0, 1, 2])");
        eval_in_context_string("vm2", "__s2pkg_voice.Voice.setAudibleTo(6, [1, 2, 3])");
        let calls = VOICE_SET_CALLS.lock().unwrap().clone();
        drop(VOICE_SET_CALLS.lock());
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], (6, 0b0111), "first owner alone");
        assert_eq!(calls[1], (6, 0b0110),
            "second push must be the INTERSECTION — an owner may narrow, never widen");
        shutdown();
    }

    /// Unloading a plugin must drop its rules through the registered "VOICE" owner store and push a
    /// clear. Mirrors transmit_unload_clears_owner_rules; a mutation test showed no-op'ing the
    /// registered closure kept the suite green, so the wiring itself was unverified.
    #[test]
    fn voice_unload_clears_owner_rules() {
        let _ = init(dummy_logger());
        VOICE_CLEAR_CALLS.lock().unwrap().clear();
        voice_rules_clear_for_test();
        set_engine_ops(Some(voice_test_ops_full()));
        create_plugin_context("vun");
        eval_in_context_string("vun", "__s2pkg_voice.Voice.setAudibleTo(9, [3])");
        unload_plugin("vun");
        assert_eq!(VOICE_CLEAR_CALLS.lock().unwrap().as_slice(), &[9],
            "teardown must reach the shim, not just drop the core-side map");
        assert_eq!(voice_merged_for_test(9), None);
        shutdown();
    }

    /// An empty receiver array is a RULE (audible to nobody), not an absence. Observable only at the
    /// op boundary: it must reach the shim as set(sender, 0), never as clear(sender).
    #[test]
    fn voice_empty_array_reaches_the_shim_as_set_zero() {
        let _ = init(dummy_logger());
        VOICE_SET_CALLS.lock().unwrap().clear();
        VOICE_CLEAR_CALLS.lock().unwrap().clear();
        voice_rules_clear_for_test();
        set_engine_ops(Some(voice_test_ops_full()));
        create_plugin_context("ve1");
        let out = eval_in_context_string("ve1", "String(__s2pkg_voice.Voice.setAudibleTo(2, []))");
        assert_eq!(out, "true");
        assert_eq!(VOICE_SET_CALLS.lock().unwrap().as_slice(), &[(2, 0u64)],
            "mask 0 WITH a rule — silencing everyone");
        assert!(VOICE_CLEAR_CALLS.lock().unwrap().is_empty(),
            "must NOT be routed to clear, which would mean 'no rule, engine decides'");
        shutdown();
    }

    /// A client disconnecting drops every owner's rule for that slot. Slots are recycled, and a rule
    /// is authored about the player who left — a survivor would silence the next occupant.
    #[test]
    fn voice_disconnect_clears_the_slot_across_owners() {
        let _ = init(dummy_logger());
        VOICE_CLEAR_CALLS.lock().unwrap().clear();
        voice_rules_clear_for_test();
        set_engine_ops(Some(voice_test_ops_full()));
        voice_set_rule_for_test("@a/one", 5, 0);
        voice_set_rule_for_test("@b/two", 5, 0b11);
        // No plugin subscribes to "disconnect" here ON PURPOSE: the cleanup must run ahead of the
        // dispatcher's no-subscriber early return.
        let _ = dispatch_client_event("disconnect", 5);
        assert_eq!(voice_merged_for_test(5), None, "every owner's rule for slot 5 is gone");
        assert_eq!(VOICE_CLEAR_CALLS.lock().unwrap().as_slice(), &[5], "and the shim was told");
        shutdown();
    }

    /// Item slice: `__s2_entity_subobj_vcall` and `EntityRef.readHandleVector` degrade (false/[])
    /// with no engine ops wired — never a crash. (A5b retired give/remove-item to gamedata/cs2
    /// `calls` descriptors; `engine_call_degrades_without_ops` covers their degrade path now.)
    #[test]
    fn item_natives_degrade_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
            [ __s2_entity_subobj_vcall(1,7,3304,25,-1,-1),
              JSON.stringify(r.readHandleVector([3296], 100, 64)) ].join("|")
        "#);
        assert_eq!(out, "false|[]");
        shutdown();
    }

    /// Entity-I/O slice: `acceptInput` degrades to `false` with no `entity_fire_input` op, and
    /// `Entity.onOutput` registers without throwing (the core-side dispatch is exercised by the shim;
    /// this only asserts the subscribe path is wired). Verbatim per the plan's Step 2.
    #[test]
    fn entity_io_degrades_and_mux_subscribes() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
            const a = r.acceptInput("Kill");                 // no op -> false
            let fired = 0;
            __s2pkg_entity.Entity.onOutput("logic_relay", "OnTrigger", () => { fired++; });
            // core-side dispatch is exercised by the shim; here assert subscribe didn't throw + acceptInput degraded
            [String(a), typeof __s2pkg_entity.Entity.onOutput].join("|")
        "#);
        assert_eq!(out, "false|function");
        shutdown();
    }

    /// Entity-I/O slice: `dispatch_output` runs every subscriber whose key matches `(class,output)`,
    /// `(class,"*")`, `("*",output)`, `("*","*")` — a wildcard-class sub and an exact-key sub both fire
    /// for one dispatch, but a DIFFERENT (class,output) pair does not. `activator`/`caller` are `null`
    /// (no engine ops -> no entity system), `value`/`delay` are threaded through verbatim, and the
    /// collapsed `HookResult` (Handled from the exact sub) is returned to the caller (>= Handled -> the
    /// shim would supersede the original `FireOutputInternal`).
    #[test]
    fn output_dispatch_matches_wildcards_and_collapses_hookresult() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        eval_in_context("p", r#"
            globalThis.__wildcardRan = 0;
            globalThis.__exactRan = 0;
            __s2pkg_entity.Entity.onOutput("*", "*", function (ev) {
                globalThis.__wildcardRan++;
                globalThis.__wcValue = ev.value;
                globalThis.__wcDelay = ev.delay;
                globalThis.__wcActivator = ev.activator;
                globalThis.__wcCaller = ev.caller;
            });
            __s2pkg_entity.Entity.onOutput("logic_relay", "OnTrigger", function (ev) {
                globalThis.__exactRan++;
                globalThis.__exactOutput = ev.output;
                return HookResult.Handled;
            });
        "#).unwrap();

        let result = dispatch_output("logic_relay", "OnTrigger", -1, -1, "some-value", 0.25);
        assert_eq!(result, HookResult::Handled as i32, "collapsed HookResult must be Handled (2)");
        assert_eq!(read_i32_global_in("p", "__wildcardRan"), 1, "the (*,*) sub must run");
        assert_eq!(read_i32_global_in("p", "__exactRan"), 1, "the exact (class,output) sub must run");
        assert_eq!(read_global_string("p", "__exactOutput"), "OnTrigger");
        assert_eq!(read_global_string("p", "__wcValue"), "some-value");
        assert!(eval_in_context_string("p", "String(globalThis.__wcDelay)").starts_with("0.25"));
        assert_eq!(eval_in_context_string("p", "String(globalThis.__wcActivator)"), "null", "no engine ops -> activator null");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__wcCaller)"), "null", "no engine ops -> caller null");

        // A different (class,output) pair matches only the (*,*) wildcard, not the exact sub.
        let result2 = dispatch_output("func_button", "OnPressed", -1, -1, "", 0.0);
        assert_eq!(result2, HookResult::Continue as i32, "no exact sub for this pair -> Continue");
        assert_eq!(read_i32_global_in("p", "__wildcardRan"), 2, "the (*,*) sub runs for every output");
        assert_eq!(read_i32_global_in("p", "__exactRan"), 1, "the exact sub must NOT run for a different pair");

        // Teardown: unload_plugin removes all of "p"'s output subs; a further dispatch is a safe no-op.
        unload_plugin("p");
        let result3 = dispatch_output("logic_relay", "OnTrigger", -1, -1, "", 0.0);
        assert_eq!(result3, HookResult::Continue as i32, "no subscribers left after unload -> Continue, no panic");

        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Slice 5D.1: game-event mechanism — subscribe/accessor/dispatch/teardown
    // ---------------------------------------------------------------------------

    // Module-level statics for mock event-op tracking (shared across the 5D.1 tests).
    pub(crate) static EV_SUBSCRIBED:   Mutex<Vec<String>> = Mutex::new(Vec::new());
    pub(crate) static EV_UNSUBSCRIBED: Mutex<Vec<String>> = Mutex::new(Vec::new());

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

    /// Event accessors wired; everything else None. Adding an op does not touch this fixture —
    /// `S2EngineOps::none()` is generated Default.
    pub(crate) fn mock_event_ops() -> S2EngineOps {
        S2EngineOps {
            event_subscribe:       Some(mock_ev_subscribe),
            event_unsubscribe:     Some(mock_ev_unsubscribe),
            event_get_int:         Some(mock_ev_get_int),
            event_get_float:       Some(mock_ev_get_float),
            event_get_bool:        Some(mock_ev_get_bool),
            event_get_string:      Some(mock_ev_get_string),
            event_get_uint64:      Some(mock_ev_get_uint64),
            event_get_player_slot: Some(mock_ev_get_player_slot),
            ..S2EngineOps::none()
        }
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
        load_body("gec", r#"
            const { GameEvent } = require("@s2script/events");
            const ev = new GameEvent("round_start");
            globalThis.__ev_name = ev.name;
            globalThis.__ev_type = typeof GameEvent;
        "#, "{}");
        assert_eq!(read_global_string("gec", "__ev_name"), "round_start");
        assert_eq!(read_global_string("gec", "__ev_type"), "function");
        shutdown();
    }






    /// Slice menu: Events.fireToClient degrades to false with no engine ops (no create -> no fire).
    #[test]
    fn events_fire_to_client_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // With no engine ops, __s2_event_create returns false, so fireToClient short-circuits to false.
        assert_eq!(
            eval_in_context_string("p", r#"var {Events}=__s2pkg_events; String(Events.fireToClient(0, "x", {a:1}))"#),
            "false"
        );
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

    /// A dotted getter key walks nested section objects; a partial/missing path yields the zero-value.
    #[test]
    fn config_getters_walk_dotted_sections() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        eval_in_context("p", r#"
            globalThis.__s2pkg_config_values = { top: 1, sect: { inner: 5, deeper: { leaf: "x" } } };
        "#).unwrap();
        // Dotted keys walk into the nested section objects.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('sect.inner'))"), "5");
        assert_eq!(eval_in_context_string("p", "__s2pkg_config.config.getString('sect.deeper.leaf')"), "x");
        // A top-level plain key still works.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('top'))"), "1");
        // A section object read as a scalar → zero-value (typeof object, not number/true).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('sect'))"), "0");
        // A path that runs off the end of a leaf → zero-value (no throw).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('sect.inner.nope'))"), "0");
        assert_eq!(eval_in_context_string("p", "__s2pkg_config.config.getString('sect.missing')"), "");
        shutdown();
    }

    /// C3: the canonical framework templates are injected at globalThis.__s2_TEMPLATES, each value
    /// equals its include_str! source and parses as JSON with a string `_help`; the admin loader
    /// still resolves through the template path (reload does not throw).
    #[test]
    fn config_framework_templates_injected_and_used() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // __s2_TEMPLATES.admins equals the canonical file byte-for-byte (via include_str!).
        assert_eq!(eval_in_context_string("p", "globalThis.__s2_TEMPLATES.admins"), super::ADMINS_TEMPLATE);
        // Every template parses as JSON with a string _help.
        for name in ["admins", "admin_groups", "admin_overrides", "databases"] {
            let expr = format!("String(typeof JSON.parse(globalThis.__s2_TEMPLATES.{})._help)", name);
            assert_eq!(eval_in_context_string("p", &expr), "string", "template {} must parse with a string _help", name);
        }
        // The admin cache still resolves through the template path: reload() calls
        // __s2_admin_readOrTemplate, which reads __s2_TEMPLATES. With no ops the file is absent →
        // template written (a no-op without ops) → parse "{}" → no throw.
        assert_eq!(eval_in_context_string("p", "(function(){ __s2pkg_admin.Admin.reload(); return 'ok'; })()"), "ok");
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
        decls.insert("greeting".to_string(), crate::config::ConfigEntry::Decl(crate::config::ConfigDecl {
            r#type: "string".to_string(),
            default: serde_json::json!("hello"),
            ..Default::default()
        }));
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
        decls.insert("x".to_string(), crate::config::ConfigEntry::Decl(crate::config::ConfigDecl {
            r#type: "int".to_string(),
            default: serde_json::json!(42),
            ..Default::default()
        }));
        store_config_decls("p", decls);
        // No onChange subscribed → must not panic.
        re_materialize_config("p");
        // Values still re-injected (even with no handlers).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_config.config.getInt('x'))"), "42");
        shutdown();
    }

    /// Slice nominations Task 1: `config.readFile`/`writeFile` degrade cleanly with no engine ops
    /// wired — readFile returns null, writeFile is a no-op (never throws).
    #[test]
    fn config_read_file_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // No engine ops -> readFile returns null, writeFile is a no-op (never throws).
        assert_eq!(
            eval_in_context_string("p", r#"var {config}=__s2pkg_config; config.writeFile("x.txt","hi"); String(config.readFile("x.txt"))"#),
            "null"
        );
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

    /// `Chat.toSlot`/`toAll` prepend a leading ZERO-WIDTH SPACE (U+200B) so a Source 2 chat box renders the
    /// message's first color control byte (an index-0 colour is muted) — the author never hand-writes a
    /// prefix. Idempotent: a line already led by the ZWSP OR a (legacy) plain space is passed through
    /// unchanged. Captured by swapping the writable `__s2_client_print` global for a JS spy.
    #[test]
    fn chat_prepends_leading_zwsp_idempotently() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // Spy captures each composed line; we compare CHAR CODES (ZWSP = 8203, plain space = 32, colour
        // byte = 4 / 7) so the assertion stays plain ASCII and proves the exact leading byte.
        let seen = eval_in_context_string(
            "p",
            r#"
            var seen = [];
            var orig = globalThis.__s2_client_print;
            globalThis.__s2_client_print = function (slot, m) { seen.push(m); };
            var C = __s2pkg_chat.Chat;
            C.color = "";
            C.toSlot(0, "\x04hi");         // bare colour      -> ZWSP + \x04hi
            C.toSlot(0, "\u200B\x04hi");   // already ZWSP-led  -> unchanged (idempotent)
            C.toSlot(0, " \x04hi");        // legacy space-led  -> unchanged (compat, no double prefix)
            C.color = "\x04";
            C.toSlot(0, "hi");             // colour via prefix -> ZWSP + \x04hi
            C.color = "";
            C.toAll("\x07red");            // broadcast path    -> ZWSP + \x07red
            globalThis.__s2_client_print = orig;
            seen.map(function (s) {
              return s.split("").map(function (c) { return c.charCodeAt(0); }).join(",");
            }).join("|");
            "#,
        );
        assert_eq!(
            seen,
            // ZWSP+\x04hi | ZWSP+\x04hi | space+\x04hi | ZWSP+\x04hi | ZWSP+\x07red
            "8203,4,104,105|8203,4,104,105|32,4,104,105|8203,4,104,105|8203,7,114,101,100"
        );
        shutdown();
    }


    /// Translations slice: `__s2_translations_read`/`__s2_client_language` degrade cleanly with no
    /// engine ops wired — translations_read returns null (both a root-file and a per-language read),
    /// client_language returns null (no crash).
    #[test]
    fn translations_natives_degrade_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // no ENGINE_OPS installed in tests -> read returns null, client_language returns null/"".
        assert_eq!(eval_in_context_string("p", "String(__s2_translations_read('', 'x'))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_translations_read('de', 'x'))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_language(0))"), "null");
        shutdown();
    }

    /// H3(b): `Translations.load` must warn loudly, exactly once, naming the set — but ONLY when
    /// the caller supplied no usable seed (the `Translations.load("common")` convention) AND the
    /// root-file read comes back null. With no ENGINE_OPS installed (as above), every root read is
    /// null, so this isolates the branch on `hasSeed` alone: a SEEDLESS load with the file "missing"
    /// must warn (every key in that set would silently render as its own key text — e.g. a missing
    /// translations/common.phrases.json degrading "No matching players" to that literal, no [SM],
    /// no colour, nothing in the console); a load WITH a seed and the identical missing-file read is
    /// the normal, correct in-code-English-default degrade path and must stay silent.
    #[test]
    fn translations_load_warns_only_when_seedless_and_file_missing() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");

        // Seedless — the "common" convention — with the backing file absent: must warn, naming the set.
        eval_in_context("p", "__s2pkg_translations.Translations.load('common');").unwrap();
        let after_seedless = LOG.lock().unwrap().clone();
        assert!(
            after_seedless.iter().any(|l| l.starts_with("[s2script] WARN") && l.contains("common")),
            "a seedless Translations.load with no backing file should have logged one \
             [s2script] WARN naming the set \"common\"; got: {:?}",
            after_seedless
        );

        // Seeded — every other plugin's convention — with the identical missing-file read: must stay silent.
        LOG.lock().unwrap().clear();
        eval_in_context("p", "__s2pkg_translations.Translations.load('withseed', { Hi: 'Hi' });").unwrap();
        let after_seeded = LOG.lock().unwrap().clone();
        assert!(
            after_seeded.iter().all(|l| !l.starts_with("[s2script] WARN")),
            "a Translations.load WITH a seed degrades correctly to the in-code English default and \
             must not warn just because the root file is also missing; got: {:?}",
            after_seeded
        );
        shutdown();
    }

    /// Colour tags expand on the chat path and are deleted on the console path. The table is
    /// supplied the way a game package supplies it — at runtime, from inside the context.
    #[test]
    fn colour_tags_expand_on_chat_and_vanish_on_console() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "__s2_colors.setTable({ Green: '\\x04', White: '\\x01' });").unwrap();
        // chat: tag -> byte, and the ZWSP still leads. JSON.stringify escapes the C0
        // control byte but leaves U+200B unescaped, so the ZWSP survives literally.
        assert_eq!(
            eval_in_context_string("p", "JSON.stringify(__s2_colors.chatLine('', '{green}hi'))"),
            "\"\u{200b}\\u0004hi\""
        );
        // console: tag deleted entirely
        assert_eq!(eval_in_context_string("p", "__s2_colors.consoleLine('{green}hi')"), "hi");
        // unknown tag: deleted, never literal
        assert_eq!(eval_in_context_string("p", "__s2_colors.consoleLine('{nope}hi')"), "hi");
        shutdown();
    }

    /// Task 2 wiring proof, chat side: `colour_tags_expand_on_chat_and_vanish_on_console` above
    /// only proves `colors.js` is reachable — it calls `__s2_colors.chatLine`/`consoleLine`
    /// directly, never the two functions Task 2 actually rewired (`__s2_chatLine`,
    /// `__s2cmd_stripCtl`). This test drives the real production entry point,
    /// `__s2pkg_chat.Chat.toSlot` (which calls `__s2_chatLine` internally), and observes what
    /// `__s2_client_print` actually received — the same spy technique as
    /// `chat_prepends_leading_zwsp_idempotently` above. A tag is put in BOTH `Chat.color` (the
    /// `prefix` argument of `chatLine(prefix, msg)`) and the message body, so a transposed
    /// argument order in the `__s2_chatLine` wrapper would produce a different byte sequence
    /// than expected and fail this test — `colour_tags_expand_on_chat_and_vanish_on_console`
    /// cannot catch that class of bug because it never calls the wrapper.
    #[test]
    fn colour_tags_expand_through_chat_to_slot() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "__s2_colors.setTable({ Green: '\\x04', White: '\\x01' });").unwrap();
        let seen = eval_in_context_string(
            "p",
            r#"
            var seen = [];
            var orig = globalThis.__s2_client_print;
            globalThis.__s2_client_print = function (slot, m) { seen.push(m); };
            var C = __s2pkg_chat.Chat;
            C.color = "{white}";        // the `prefix` argument of chatLine(prefix, msg)
            C.toSlot(0, "{green}hi");   // the `msg` argument — a DIFFERENT tag, to catch transposition
            globalThis.__s2_client_print = orig;
            seen.map(function (s) {
              return s.split("").map(function (c) { return c.charCodeAt(0); }).join(",");
            }).join("|");
            "#,
        );
        // ZWSP + white(\x01) + green(\x04) + "hi" — prefix expands before msg, in that order.
        assert_eq!(seen, "8203,1,4,104,105");
        shutdown();
    }

    /// Task 2 wiring proof, console side: same gap as above but for `__s2cmd_stripCtl`. Drives
    /// the real production entry point — a registered command replying via
    /// `ctx.replyToConsole` for a server caller (slot -1), which logs through `console.log` and
    /// is captured in `LOG` (same technique as `ctx_replyt_localizes` above). The message mixes
    /// a colour TAG with a raw C0 control byte in one string: the tag must be gone (proving
    /// `__s2cmd_stripCtl` really calls the expander, not just the old regex) and the raw byte
    /// must still be gone too (proving the pre-existing strip behaviour survived the rewrite).
    #[test]
    fn colour_tags_vanish_through_reply_to_console() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "__s2_colors.setTable({ Green: '\\x04' });").unwrap();
        eval_in_context(
            "p",
            "__s2pkg_commands.Commands.register('sm_x', function (ctx) { ctx.replyToConsole('{green}hi\\x07there'); });",
        )
        .unwrap();
        eval_in_context("p", "__s2pkg_commands.Commands.dispatch('sm_x', -1, '');").unwrap();
        assert!(
            LOG.lock().unwrap().iter().any(|l| l == "hithere"),
            "replyToConsole should have logged the tag- and control-byte-stripped string, got: {:?}",
            LOG.lock().unwrap()
        );
        shutdown();
    }

    /// Translations slice: the pure formatting/lang-code test hooks (`__s2_tr_format`/`__s2_tr_langCode`).
    #[test]
    fn translations_format_and_langcode() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // positional {1}/{2}; missing {3} -> empty; no args -> literal text
        assert_eq!(eval_in_context_string("p", "__s2_tr_format('Slapped {1} for {2}', ['Bob','5'])"), "Slapped Bob for 5");
        assert_eq!(eval_in_context_string("p", "__s2_tr_format('a {3} b', ['x'])"), "a  b");
        assert_eq!(eval_in_context_string("p", "__s2_tr_format('plain', [])"), "plain");
        // cl_language -> folder code
        assert_eq!(eval_in_context_string("p", "__s2_tr_langCode('german')"), "de");
        assert_eq!(eval_in_context_string("p", "__s2_tr_langCode('english')"), "");   // root
        assert_eq!(eval_in_context_string("p", "__s2_tr_langCode('klingon')"), "");   // unknown -> default(root)
        shutdown();
    }

    /// Translations slice: the registry fallback chain — lang -> default(seed) -> key.
    #[test]
    fn translations_fallback_chain() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "\
            __s2pkg_translations.Translations.load('t', { Hi: 'Hi {1}', Only: 'Only-EN' });\
            __s2_tr_injectLang('t', 'de', { Hi: 'Hallo {1}' });\
        ").unwrap();
        // slot<0 default(root/en): seed
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Hi','Bob')"), "Hi Bob");
        // default language de -> the injected de map; a key missing in de falls back to the seed
        eval_in_context("p", "__s2pkg_translations.Translations.setDefaultLanguage('de');").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Hi','Bob')"), "Hallo Bob");
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Only')"), "Only-EN"); // de miss -> seed
        // an unknown key -> the key itself
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Nope')"), "Nope");
        shutdown();
    }

    /// D1: a translation in a LATER-loaded set must beat an English default in an earlier one.
    /// Unreachable with a single set, which is why it survived; reachable the moment a plugin
    /// loads its own set plus the shared `common` set.
    #[test]
    fn translate_prefers_any_language_hit_over_any_english_default() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "\
            __s2pkg_translations.Translations.load('own',    { Greet: 'EN own' });\
            __s2pkg_translations.Translations.load('common', { Greet: 'EN common' });\
            __s2_tr_injectLang('common', 'de', { Greet: 'DE common' });\
            __s2pkg_translations.Translations.setDefaultLanguage('de');\
        ").unwrap();
        assert_eq!(
            eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Greet')"),
            "DE common"
        );
        shutdown();
    }

    /// `ctx.translations.load(a, b)` registers in the order given, so a plugin's own phrase beats a
    /// shared one of the same key. Nothing is loaded for a plugin automatically — that is the rule
    /// SourceMod's LoadTranslations enforces, and the order is the plugin's to state.
    #[test]
    fn ctx_translations_load_registers_in_the_order_given() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("p", r#"ctx.translations.load("own", "common");"#, "{}");
        // Both sets define Greet. Injected at a real language code, not "", because translate skips
        // the language pass entirely when the code is empty and would then only see the (empty,
        // file-less) English defaults.
        eval_in_context("p", "\
            __s2_tr_injectLang('own',    'de', { Greet: 'from own' });\
            __s2_tr_injectLang('common', 'de', { Greet: 'from common' });\
            __s2pkg_translations.Translations.setDefaultLanguage('de');\
        ").unwrap();
        assert_eq!(
            eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Greet')"),
            "from own",
        );
        shutdown();
    }

    /// D2: a substituted argument must not be able to inject a colour tag. A player who renames
    /// themselves "{red}x{default}" would otherwise recolour every message that names them.
    #[test]
    fn translate_strips_braces_from_substituted_args() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p",
            "__s2pkg_translations.Translations.load('t', { Slain: '{1} was slain' });").unwrap();
        assert_eq!(
            eval_in_context_string("p",
                "__s2pkg_translations.Translations.translate(-1,'Slain','{red}evil{default}')"),
            "redevildefault was slain"
        );
        shutdown();
    }

    /// Translations slice: `ctx.replyT` (in `@s2script/commands`) translates the key for the caller's
    /// language before replying. A console caller (slot -1) replies via `console.log`, captured in `LOG`.
    #[test]
    fn ctx_replyt_localizes() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "\
            __s2pkg_translations.Translations.load('c', { Kicked: 'Kicked {1}' });\
            __s2pkg_commands.Commands.register('sm_x', function (ctx) { ctx.replyT('Kicked', 'Bob'); });\
        ").unwrap();
        // invoke the command with a console caller (slot -1) via the dispatch registry
        eval_in_context("p", "__s2pkg_commands.Commands.dispatch('sm_x', -1, '');").unwrap();
        assert!(LOG.lock().unwrap().iter().any(|l| l.contains("Kicked Bob")), "replyT should have logged the translated string");
        shutdown();
    }




    /// `__s2_cvar_set` degrades to false without the op; Server.setCvar is wired to it.
    #[test]
    fn cvar_set_degrades_false_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pcset");
        assert_eq!(eval_in_context_string("pcset", "String(__s2_cvar_set('sv_gravity', '800'))"), "false");
        assert_eq!(eval_in_context_string("pcset", "String(__s2pkg_server.Server.setCvar('sv_gravity', '800'))"), "false");
        shutdown();
    }

    /// `__s2_cvar_set` passes (name, value) to the op and returns its 1/0 as a boolean.
    #[test]
    fn cvar_set_passes_name_and_value_to_op() {
        use std::sync::Mutex;
        static LAST: Mutex<Option<(String, String)>> = Mutex::new(None);
        extern "C" fn mock_set(name: *const c_char, value: *const c_char) -> c_int {
            let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
            let v = unsafe { std::ffi::CStr::from_ptr(value) }.to_string_lossy().into_owned();
            *LAST.lock().unwrap() = Some((n, v));
            1
        }
        let _ = init(dummy_logger());
        *LAST.lock().unwrap() = None;
        set_engine_ops(Some(S2EngineOps {
            cvar_set: Some(mock_set),
            ..mock_event_ops()
        }));
        create_plugin_context("pcset2");
        assert_eq!(eval_in_context_string("pcset2", "String(__s2pkg_server.Server.setCvar('sv_gravity', '400'))"), "true");
        assert_eq!(LAST.lock().unwrap().clone(), Some(("sv_gravity".into(), "400".into())));
        set_engine_ops(None);
        shutdown();
    }

    /// FakeConVar slice: Server.registerCvar degrades to false without the convar_register op, and an
    /// unknown type string is rejected false JS-side (never reaches the op).
    #[test]
    fn register_cvar_degrades_false_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pcv");
        let out = eval_in_context_string("pcv", r#"
            var a = __s2pkg_server.Server.registerCvar("s2_test_cvar", { type: "int", default: 42, min: 0, max: 100 });
            var b = __s2pkg_server.Server.registerCvar("s2_bad", { type: "nope", default: 1 });
            String(a === false && b === false)
        "#);
        assert_eq!(out, "true");
        shutdown();
    }

    /// Both sound natives degrade with no ops table: emit -> 0, precache-add -> false. Raw-native
    /// level (the @s2script/sound module surface is Task-4-tested).
    #[test]
    fn sound_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("psnd");
        assert_eq!(eval_in_context_string("psnd",
            "String(__s2_sound_emit('Weapon_AK47.Single', 0, -1, [0, 1], 1.0))"), "0");
        assert_eq!(eval_in_context_string("psnd",
            "String(__s2_sound_precache_add('soundevents/test.vsndevts'))"), "false");
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

    // L1 Task 4 (onDamage return collapse) recording seams: a fixed m_flDamage offset + a
    // damage_write_float that records its (offset, value) so a test can assert the live damage was zeroed.
    static DMG_WRITE_REC: std::sync::Mutex<Option<(i32, f32)>> = std::sync::Mutex::new(None);
    extern "C" fn rec_damage_write_float(offset: c_int, value: f32) {
        *DMG_WRITE_REC.lock().unwrap() = Some((offset, value));
    }
    extern "C" fn fake_dmg_schema_offset(_cls: *const c_char, _field: *const c_char) -> c_int { 68 }

    /// B2 fix — this test previously asserted the OPPOSITE, and the old assertion was the bug.
    ///
    /// `Handled` must NOT short-circuit an `onDamage` chain. `ARCHITECTURE.md:78` states the collapse
    /// rule outright — "`Stop` short-circuits. `Handled` does **not** short-circuit (a later observer
    /// may still want the event)" — and `multiplexer.rs`'s own `handled_does_not_short_circuit` test
    /// says the same. `dispatch_damage` was the one path that `break`ed at `>= Handled`, so a single
    /// plugin blocking a hit silently denied every OTHER plugin's damage observer its dispatch. That
    /// is precisely the cross-plugin composition the collapse rule exists to protect.
    ///
    /// Blocking is a decision about the damage, not a veto over other observers.
    #[test]
    fn damage_onpre_handled_does_not_stop_the_chain() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0; \
            __s2pkg_damage.Damage.onPre(function(){ globalThis.__a++; return HookResult.Handled; }); \
            __s2pkg_damage.Damage.onPre(function(){ globalThis.__b++; return HookResult.Continue; });").unwrap();
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1", "first handler ran");
        assert_eq!(
            eval_in_context_string("p", "String(globalThis.__b)"), "1",
            "a later observer must still run after another plugin returned Handled"
        );
        shutdown();
    }

    /// The other half of the rule: `Stop` DOES truncate. A handler that genuinely wants to end the
    /// chain has a way to say so — which is what makes removing the `Handled` short-circuit safe
    /// rather than a loss of expressiveness.
    #[test]
    fn damage_onpre_stop_return_truncates_the_chain() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "globalThis.__a=0; globalThis.__b=0; \
            __s2pkg_damage.Damage.onPre(function(){ globalThis.__a++; return HookResult.Stop; }); \
            __s2pkg_damage.Damage.onPre(function(){ globalThis.__b++; return HookResult.Continue; });").unwrap();
        dispatch_damage();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__a)"), "1", "first handler ran");
        assert_eq!(
            eval_in_context_string("p", "String(globalThis.__b)"), "0",
            "Stop must still truncate the remainder of the chain"
        );
        shutdown();
    }

    /// L1 Task 4: an `onDamage` handler returning `>= HookResult.Handled` zeroes the live damage via
    /// the same `damage_write_float` op path the JS `info.damage = 0` setter uses.
    #[test]
    fn damage_onpre_handled_return_zeroes_live_damage() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        *DMG_WRITE_REC.lock().unwrap() = None;
        set_engine_ops(Some(S2EngineOps {
            schema_offset: Some(fake_dmg_schema_offset),
            damage_write_float: Some(rec_damage_write_float),
            ..mock_event_ops()
        }));
        create_plugin_context("p");
        eval_in_context("p", "__s2pkg_damage.Damage.onPre(function (info) { return HookResult.Handled; });").unwrap();
        dispatch_damage();
        assert_eq!(*DMG_WRITE_REC.lock().unwrap(), Some((68, 0.0)),
            "onDamage >= Handled zeroed m_flDamage (offset 68) through the damage_write_float op");
        shutdown();
    }

    /// Usercmd primitive Task 2 (MF-3): `__s2_usercmd_subscribe` registers a RAW handler into
    /// `USERCMD_MUX` under "onRun" (no `UserCmd.onRun` wrapper exists yet — that's Task 4), and
    /// `dispatch_usercmd(slot)` invokes it with `(cmd, ctx)` where `ctx.slot` is the firing slot,
    /// collapsing the handler's returned int into a `HookResult` (2 = Handled here).
    #[test]
    fn usercmd_dispatch_runs_subscriber_and_collapses_hookresult() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context(
            "p",
            "globalThis.__capturedSlot = -999; \
             __s2_usercmd_subscribe(function (cmd, ctx) { globalThis.__capturedSlot = ctx.slot; return 2; });",
        )
        .unwrap();
        assert_eq!(dispatch_usercmd(3), 2, "the handler's returned HookResult (Handled) collapses through");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__capturedSlot)"), "3", "ctx.slot === the dispatched slot");
        shutdown();
    }

    /// Usercmd primitive Task 2: with no subscribers at all, `dispatch_usercmd` returns Continue (0)
    /// and does not throw/panic.
    #[test]
    fn usercmd_dispatch_no_subs_returns_continue() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(dispatch_usercmd(5), 0, "no subscribers -> Continue");
        shutdown();
    }

    /// Usercmd primitive Task 3 (Step 6, degrade-never-crash): with NO engine ops installed at all,
    /// every accessor native degrades to a safe default rather than throwing/panicking —
    /// `__s2_usercmd_read` reads `0`, `__s2_usercmd_write`/`__s2_usercmd_write_buttons`/
    /// `__s2_usercmd_clear_subtick` are silent no-ops (return `undefined`), and
    /// `__s2_usercmd_read_buttons` reads `0n` — a `bigint`, never `undefined` (the spec's `buttons:
    /// bigint` contract holds even out of dispatch / with no op). `__s2_usercmd_subscribe` itself
    /// already registers cleanly without a `usercmd_hook_install` op present, proven by the two
    /// dispatch tests directly above (both run under this exact no-ops condition) — `UserCmd.onRun`
    /// (the Task 4 JS wrapper around this same native) has nothing more to degrade.
    #[test]
    fn usercmd_accessors_degrade_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_usercmd_read(0))"), "0", "read degrades to 0");
        assert_eq!(eval_in_context_string("p", "String(__s2_usercmd_write(0, 1.0))"), "undefined", "write no-throws");
        assert_eq!(eval_in_context_string("p", "typeof __s2_usercmd_read_buttons()"), "bigint", "buttons stays a bigint, never undefined");
        assert_eq!(eval_in_context_string("p", "String(__s2_usercmd_read_buttons())"), "0", "read_buttons degrades to 0n");
        assert_eq!(eval_in_context_string("p", "String(__s2_usercmd_write_buttons(5n))"), "undefined", "write_buttons no-throws");
        assert_eq!(eval_in_context_string("p", "String(__s2_usercmd_clear_subtick())"), "undefined", "clear_subtick no-throws");
        shutdown();
    }

    /// Usercmd primitive Task 4: the `@s2script/usercmd` prelude module wires — `__s2pkg_usercmd` exposes
    /// `UserCmd`/`HookResult`; `UserCmd.onRun` is a function that forwards straight to
    /// `__s2_usercmd_subscribe` (proven separately by `usercmd_dispatch_runs_subscriber_and_collapses_hookresult`,
    /// which subscribes via the raw native); and the SINGLETON `Cmd` object's accessors read/write
    /// through the (here op-less, degrading) natives — `forwardMove`/`sideMove`/`upMove`/`impulse` read
    /// `0` and accept a set with no throw, `buttons` reads a real `0n` bigint and accepts a bigint set,
    /// `viewAngles` reads a `QAngle`-shaped `{x:0,y:0,z:0}` (fields 3/4/5) and a set writes all three
    /// via three separate `__s2_usercmd_write` calls, and `clearSubtickMoves()` doesn't throw.
    #[test]
    fn usercmd_module_cmd_singleton_and_userrun_wiring() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_usercmd.UserCmd.onRun"), "function");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_usercmd.HookResult.Handled)"), "2");
        let cmd = "__s2pkg_usercmd.Cmd";
        assert_eq!(eval_in_context_string("p", &format!("String({cmd}.forwardMove)")), "0");
        assert_eq!(eval_in_context_string("p", &format!("String({cmd}.sideMove)")), "0");
        assert_eq!(eval_in_context_string("p", &format!("String({cmd}.upMove)")), "0");
        assert_eq!(eval_in_context_string("p", &format!("String({cmd}.impulse)")), "0");
        assert_eq!(eval_in_context_string("p", &format!("typeof {cmd}.buttons")), "bigint");
        assert_eq!(eval_in_context_string("p", &format!("String({cmd}.buttons)")), "0");
        assert_eq!(
            eval_in_context_string("p", &format!("JSON.stringify({{x:{cmd}.viewAngles.x, y:{cmd}.viewAngles.y, z:{cmd}.viewAngles.z}})")),
            "{\"x\":0,\"y\":0,\"z\":0}",
        );
        // Sets no-throw (degrade-never-crash) — a plain numeric set, a bigint set, and a viewAngles
        // object set (exercises all three underlying __s2_usercmd_write calls).
        assert_eq!(eval_in_context_string("p", &format!("(function(){{ {cmd}.forwardMove = 1; {cmd}.sideMove = -1; {cmd}.upMove = 0.5; {cmd}.impulse = 100; {cmd}.buttons = 5n; {cmd}.viewAngles = {{x:1,y:2,z:3}}; {cmd}.clearSubtickMoves(); return \"ok\"; }}())")), "ok");
        // End-to-end through UserCmd.onRun + dispatch_usercmd: the handler must receive the REAL Cmd
        // singleton object (typeof "object" with a working forwardMove/buttons property), not
        // `undefined` — this is the exact wiring a missing `Cmd` key on `__s2pkg_usercmd` would silently
        // break (dispatch_usercmd degrades to passing `undefined` when the lookup fails).
        eval_in_context(
            "p",
            "globalThis.__cmdType = null; globalThis.__cmdIsSingleton = false; \
             __s2pkg_usercmd.UserCmd.onRun(function (cmd, ctx) { \
               globalThis.__cmdType = typeof cmd; \
               globalThis.__cmdIsSingleton = (cmd === __s2pkg_usercmd.Cmd); \
               globalThis.__cmdForward = String(cmd.forwardMove); \
             });",
        )
        .unwrap();
        assert_eq!(dispatch_usercmd(9), 0, "no Handled/Stop returned -> Continue");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__cmdType)"), "object", "handler received an object, not undefined");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__cmdIsSingleton)"), "true", "handler received the exact Cmd singleton (MF-3)");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__cmdForward)"), "0", "cmd.forwardMove readable inside the handler");
        shutdown();
    }

































    // ---------------------------------------------------------------------------
    // Slice DB Task 3: __s2_sqlite_* natives — round trip (now actor-backed, off-thread) + degrade tests.
    // ---------------------------------------------------------------------------

    /// A fresh per-call SQLite connection "name" — avoids cross-test file collisions (tests run
    /// serially via `.cargo/config.toml` `RUST_TEST_THREADS=1`, but the on-disk file persists
    /// across separate `cargo test` invocations, so a fixed name could see stale state).
    fn unique_db_name(prefix: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        format!("{}_{}_{}", prefix, std::process::id(), n)
    }

    /// Mock `db_data_dir` op: a fixed OS-temp subdirectory, lazily created (mirrors the shim
    /// `s2_db_data_dir`'s static-buffer-return style).
    extern "C" fn mock_db_data_dir() -> *const c_char {
        use std::sync::OnceLock;
        static DIR: OnceLock<std::ffi::CString> = OnceLock::new();
        let c = DIR.get_or_init(|| {
            let mut p = std::env::temp_dir();
            p.push("s2script_test_db_data");
            let _ = std::fs::create_dir_all(&p);
            std::ffi::CString::new(p.to_string_lossy().into_owned()).unwrap()
        });
        c.as_ptr()
    }

    /// A full ops table with ONLY `db_data_dir` wired (reuses `mock_event_ops()`'s all-None base
    /// via struct-update syntax — every other field stays None).
    fn db_ops() -> S2EngineOps {
        S2EngineOps { db_data_dir: Some(mock_db_data_dir), ..mock_event_ops() }
    }

    /// The full happy path: open -> execute(CREATE) -> execute(INSERT, parameterized) ->
    /// query(parameterized) -> close, all chained through native-returned Promises. `query`/
    /// `execute` now run OFF-THREAD on the connection's actor (this task's behavior change), so
    /// each link needs its OWN completion to arrive on the shared channel before the next `.then`
    /// can fire — drive frames until the chain settles (bounded), mirroring
    /// `thread_sleep_runs_off_thread_and_resolves_on_a_drain`. Proves value marshalling both
    /// directions (params in, columns/rows out) and the `lastInsertId`/`changes` execute-result shape.
    #[test]
    fn sqlite_open_execute_query_round_trip() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("t3_roundtrip");
        load_body("dbp", &format!(r#"
            globalThis.__out = "pending";
            __s2_sqlite_open("{name}").then(function (h) {{
                return __s2_sqlite_execute(h, "CREATE TABLE kv (k TEXT, v TEXT)", []).then(function () {{
                    return __s2_sqlite_execute(h, "INSERT INTO kv (k, v) VALUES (?, ?)", ["color", "red"]);
                }}).then(function (er) {{
                    return __s2_sqlite_query(h, "SELECT k, v FROM kv WHERE k = ?", ["color"]).then(function (rows) {{
                        globalThis.__out = "changes=" + er.changes + " id=" + er.lastInsertId
                            + " rows=" + rows.length + " v=" + rows[0].v + " k=" + rows[0].k;
                        return __s2_sqlite_close(h);
                    }});
                }});
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#, name = name), "{}");
        let mut out = "pending".to_string();
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            out = read_global_string("dbp", "__out");
            if out != "pending" { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(out, "changes=1 id=1 rows=1 v=red k=color");
        shutdown();
    }

    /// A bad-SQL query rejects the Promise (not a panic/crash) — the `.catch` handler runs and
    /// records the error, proving the actor's `run_query` `Err` path reaches JS as a rejection via
    /// `resolve_db` on a LATER drain (the query itself now runs off-thread on the actor).
    #[test]
    fn sqlite_bad_sql_rejects_promise() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("t3_badsql");
        load_body("dbp2", &format!(r#"
            globalThis.__out = "pending";
            __s2_sqlite_open("{name}").then(function (h) {{
                return __s2_sqlite_query(h, "SELECT * FROM nope", []);
            }}).then(function () {{
                globalThis.__out = "should-not-resolve";
            }}).catch(function (e) {{
                globalThis.__out = "rejected:" + (String(e).length > 0);
            }});
        "#, name = name), "{}");
        let mut out = "pending".to_string();
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            out = read_global_string("dbp2", "__out");
            if out != "pending" { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(out, "rejected:true");
        shutdown();
    }

    /// Degrade path: with NO `db_data_dir` op wired, `__s2_sqlite_open` rejects gracefully (never
    /// panics) — proving the natives are registered and reachable even when the engine op table
    /// is absent (e.g. a stale core against an old shim).
    #[test]
    fn sqlite_open_degrades_without_data_dir_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None); // no ops at all -> db_data_dir() returns None -> open() rejects
        load_body("dbp3", r#"
            globalThis.__out = "pending";
            __s2_sqlite_open("whatever").then(function () {
                globalThis.__out = "should-not-resolve";
            }).catch(function (e) {
                globalThis.__out = "rejected:" + (String(e).length > 0);
            });
        "#, "{}");
        frame_async_drain();
        assert_eq!(read_global_string("dbp3", "__out"), "rejected:true");
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Slice DB Task 4: `@s2script/db` — the __s2pkg_db prelude runtime (Database.open/query/
    // execute/close over the __s2_sqlite_* natives, registerDriver seam).
    // ---------------------------------------------------------------------------

    /// The module resolves via `require("@s2script/db")` (the generic `s2require` rule) and
    /// exposes `Database.open`/`Database.registerDriver` as functions.
    #[test]
    fn db_module_resolves_with_expected_shape() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        load_body("dbshape", r#"
            var { Database } = require("@s2script/db");
            globalThis.__out = (typeof Database.open === "function") + "," + (typeof Database.registerDriver === "function");
        "#, "{}");
        assert_eq!(read_global_string("dbshape", "__out"), "true,true");
        shutdown();
    }

    /// End-to-end through the PUBLIC `@s2script/db` API (not the raw natives): open a named
    /// database, CREATE + INSERT (parameterized), SELECT it back, close. Proves the Database
    /// object built by the prelude (over the SQLite reference driver, now actor-backed)
    /// round-trips correctly — drive frames until the chain settles (query/execute are off-thread).
    #[test]
    fn db_module_open_execute_query_round_trip() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("t4_roundtrip");
        load_body("dbmod", &format!(r#"
            var {{ Database }} = require("@s2script/db");
            globalThis.__out = "pending";
            Database.open("{name}").then(function (db) {{
                return db.execute("CREATE TABLE kv (k TEXT, v TEXT)").then(function () {{
                    return db.execute("INSERT INTO kv (k, v) VALUES (?, ?)", ["color", "red"]);
                }}).then(function (er) {{
                    return db.query("SELECT k, v FROM kv WHERE k = ?", ["color"]).then(function (rows) {{
                        globalThis.__out = "changes=" + er.changes + " id=" + er.lastInsertId
                            + " rows=" + rows.length + " v=" + rows[0].v + " k=" + rows[0].k;
                        return db.close();
                    }});
                }});
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#, name = name), "{}");
        let mut out = "pending".to_string();
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            out = read_global_string("dbmod", "__out");
            if out != "pending" { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(out, "changes=1 id=1 rows=1 v=red k=color");
        shutdown();
    }

    /// `registerDriver` actually takes effect: `Database.open`'s config is stubbed to the
    /// `"sqlite"`-named driver this slice, so registering a fake driver UNDER THAT NAME proves the
    /// seam is live (the fake's `connect` runs instead of the real SQLite one) without needing a
    /// second name->config route.
    #[test]
    fn db_module_register_driver_seam_overrides_by_name() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        load_body("dbdrv", r#"
            var { Database } = require("@s2script/db");
            globalThis.__out = "pending";
            Database.registerDriver({
                name: "sqlite",
                connect: function (config) {
                    return Promise.resolve({
                        query: function () { return Promise.resolve([{ fake: "yes", name: config.name }]); },
                        execute: function () { return Promise.resolve({ changes: 0, lastInsertId: 0 }); },
                        close: function () { return Promise.resolve(); },
                    });
                },
            });
            Database.open("whatever-name").then(function (db) {
                return db.query("SELECT 1").then(function (rows) {
                    globalThis.__out = "fake=" + rows[0].fake + " name=" + rows[0].name;
                });
            }).catch(function (e) { globalThis.__out = "ERROR:" + String(e); });
        "#, "{}");
        frame_async_drain();
        assert_eq!(read_global_string("dbdrv", "__out"), "fake=yes name=whatever-name");
        shutdown();
    }

    /// The remote-SQL-driver slice's Task 3: `Database.open` resolves a name via `databases.json`
    /// (the config bridge) instead of always defaulting to SQLite. Seeds the IIFE-private config
    /// map via the secret-free `__s2_db_testSetConfig` hook (bypassing the config bridge, which
    /// degrades to null in tests) + registers a fake `mysql` driver to assert the configured name
    /// routes to it; also exercises the secret-free `__s2_db_resolveConfigDriver` test hook directly
    /// for the configured-vs-unconfigured cases (the full config, including `password`, is never
    /// exposed on `globalThis`).
    #[test]
    fn db_open_routes_by_config() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // seed the per-context config via the injector hook (bypass the config bridge, unavailable in tests) + a fake driver
        eval_in_context("p", "\
            __s2_db_testSetConfig({ stats: { driver:'mysql', name:'stats', host:'h' } });\
            var seen=null;\
            __s2pkg_db.Database.registerDriver({ name:'mysql', connect:function(c){ seen=c; return Promise.resolve({query:function(){},execute:function(){},close:function(){}});} });\
            __s2pkg_db.Database.open('stats');\
            globalThis.__test_seen_driver = seen ? seen.driver : 'none';\
        ").unwrap();
        assert_eq!(eval_in_context_string("p", "globalThis.__test_seen_driver"), "mysql");
        // an UNconfigured name falls back to sqlite
        assert_eq!(eval_in_context_string("p", "__s2_db_resolveConfigDriver('whatever')"), "sqlite");
        // a configured name resolves to its driver
        assert_eq!(eval_in_context_string("p", "__s2_db_resolveConfigDriver('stats')"), "mysql");
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Slice HTTP Task 2: __s2_fetch native + the async-result drain step (frame_async_drain's
    // new fetch-completion loop + resolve_fetch) — the async spine over core/src/http.rs (Task 1).
    // ---------------------------------------------------------------------------

    /// A tiny local HTTP/1.1 server on an ephemeral port; returns one canned response then exits.
    /// Duplicated from `http::tests::spawn_server` (that helper is private to `http`'s own test
    /// module) so this module can drive `__s2_fetch` end to end without any real-network egress.
    fn spawn_local_http_server(response: &'static str) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(response.as_bytes());
            }
        });
        port
    }

    /// `__s2_fetch` end-to-end against a real (local) HTTP server: the native hands off to the
    /// tokio engine and returns a pending Promise immediately (never blocking the calling thread);
    /// the Promise resolves only on a LATER `frame_async_drain()` once the background request
    /// completes — proving the whole async-result spine (RESOLVERS + PENDING_JOBS + the fetch
    /// drain step + `resolve_fetch`'s payload-building) together.
    #[test]
    fn fetch_native_resolves_on_a_later_drain_with_the_response_payload() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_http_server("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        load_body(
            "fetchp",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_fetch("http://127.0.0.1:{port}/", {{}}).then(function (r) {{
                globalThis.__out = r.status + ":" + r.ok + ":" + r.body;
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        // The response arrives async (a real background thread) — poll the drain up to ~500
        // times (bounded) rather than assuming it lands on the very next drain.
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if read_global_string("fetchp", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "fetch promise never resolved on a drain");
        assert_eq!(read_global_string("fetchp", "__out"), "200:true:hello");
        shutdown();
    }

    /// A 4xx/5xx HTTP status RESOLVES the Promise with `ok:false` (never rejects) — the
    /// degrade-never-crash contract for an application-level error vs. a network/timeout failure.
    #[test]
    fn fetch_native_404_resolves_with_ok_false() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_http_server("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        load_body(
            "fetch404",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_fetch("http://127.0.0.1:{port}/", {{}}).then(function (r) {{
                globalThis.__out = r.status + ":" + r.ok;
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if read_global_string("fetch404", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "fetch promise never resolved on a drain");
        assert_eq!(read_global_string("fetch404", "__out"), "404:false");
        shutdown();
    }

    /// A network failure (connection refused) REJECTS the Promise (the `.catch` runs) rather than
    /// resolving or panicking — the native never blocks nor crashes on an unreachable host.
    #[test]
    fn fetch_native_bad_host_rejects_the_promise() {
        init(dummy_logger()).unwrap();
        load_body(
            "fetchbad",
            r#"
            globalThis.__out = "pending";
            __s2_fetch("http://127.0.0.1:1/", { timeoutMs: 1000 }).then(function (r) {
                globalThis.__out = "should-not-resolve:" + r.status;
            }).catch(function (e) {
                globalThis.__out = "rejected:" + (String(e).length > 0);
            });
        "#,
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if read_global_string("fetchbad", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "fetch promise never settled on a drain");
        assert_eq!(read_global_string("fetchbad", "__out"), "rejected:true");
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Slice HTTP Task 3: `@s2script/http` — the __s2pkg_http prelude runtime (fetch over
    // __s2_fetch, adding text()/json() over the buffered body).
    // ---------------------------------------------------------------------------

    /// The module resolves via `require("@s2script/http")` (the generic `s2require` rule) and
    /// exposes `fetch` (the named export) as a function.
    #[test]
    fn http_module_resolves_with_expected_shape() {
        init(dummy_logger()).unwrap();
        load_body(
            "httpshape",
            r#"
            var { fetch } = require("@s2script/http");
            globalThis.__out = String(typeof fetch === "function");
        "#,
            "{}",
        );
        assert_eq!(read_global_string("httpshape", "__out"), "true");
        shutdown();
    }

    /// End-to-end through the PUBLIC `@s2script/http` API (not the raw native): `fetch` against a
    /// real local server resolves with `status`/`ok`/`statusText`/`headers` plus the `text()`/
    /// `json()` accessors over the buffered body — proving the wrapper the prelude builds over the
    /// raw `__s2_fetch` payload.
    #[test]
    fn http_module_fetch_round_trip_with_text_and_json() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_http_server(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9\r\n\r\n{\"a\":\"b\"}",
        );
        load_body(
            "httpmod",
            &format!(
                r#"
            var {{ fetch }} = require("@s2script/http");
            globalThis.__out = "pending";
            fetch("http://127.0.0.1:{port}/").then(function (r) {{
                globalThis.__out = r.status + ":" + r.ok + ":" + r.statusText + ":" + r.text() + ":" + r.json().a;
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if read_global_string("httpmod", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "http module fetch never resolved on a drain");
        assert_eq!(
            read_global_string("httpmod", "__out"),
            "200:true:OK:{\"a\":\"b\"}:b"
        );
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // WebSocket Task 2: __s2_ws_* natives + signal routing (connect resolver + event mux) — the
    // async spine over core/src/ws.rs's tokio+tungstenite engine (Task 1).
    // ---------------------------------------------------------------------------

    /// A tiny local WebSocket echo server on an ephemeral port. Duplicated from
    /// `ws::tests::echo_server_port` (that helper is private to `ws`'s own test module) so this
    /// module can drive `__s2_ws_connect`/`__s2_ws_send`/`__s2_ws_on` end to end without any
    /// real-network egress.
    /// Completes the WebSocket handshake and then immediately drops the connection.
    ///
    /// The client sees `Connected` and `Closed` land in the SAME drain batch, which is the ordering
    /// that used to be unrecoverable: the connect Promise resolved, but the conn was deregistered
    /// before the `.then` continuation ran, so the continuation's `onClose` subscribe failed the
    /// ownership gate and the close event fanned out to nobody. The plugin got a Promise that
    /// resolved onto a connection it could neither use nor be told about — indistinguishable, from
    /// JS, from a connection that simply never spoke again.
    fn spawn_local_ws_instant_close_server() -> u16 {
        crate::http::init();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        crate::http::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    drop(ws);   // handshake done, then gone
                }
            }
        });
        port
    }

    fn spawn_local_ws_echo_server() -> u16 {
        use futures_util::{SinkExt, StreamExt};
        crate::http::init();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        crate::http::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    let (mut w, mut r) = ws.split();
                    while let Some(Ok(m)) = r.next().await {
                        if m.is_close() {
                            break;
                        }
                        if w.send(m).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        port
    }

    /// A local ws server that reports back what it saw on the HANDSHAKE rather than echoing frames:
    /// its first message is the request's `Authorization` header (or `"<none>"`). That is what lets
    /// a test assert a JS-supplied header actually crossed the wire, instead of only asserting the
    /// Rust request builder set it.
    fn spawn_local_ws_header_reporting_server() -> u16 {
        use futures_util::SinkExt;
        crate::http::init();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        crate::http::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            if let Ok((stream, _)) = listener.accept().await {
                let seen = std::sync::Arc::new(std::sync::Mutex::new(String::from("<none>")));
                let captured = seen.clone();
                let cb = |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                          res: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    if let Some(v) = req.headers().get("authorization") {
                        *captured.lock().unwrap() = v.to_str().unwrap_or("<unreadable>").to_string();
                    }
                    Ok(res)
                };
                if let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, cb).await {
                    // Reply only when asked. Pushing unprompted races the client's
                    // `onMessage` subscription, which is registered in the connect
                    // promise's `.then` and so does not exist yet at handshake time.
                    use futures_util::StreamExt;
                    let (mut w, mut r) = ws.split();
                    if let Some(Ok(_)) = r.next().await {
                        let value = seen.lock().unwrap().clone();
                        let _ = w
                            .send(tokio_tungstenite::tungstenite::Message::text(value))
                            .await;
                    }
                }
            }
        });
        port
    }

    /// A header passed to `__s2_ws_connect`'s init object reaches the server's handshake.
    ///
    /// This is the whole point of the init parameter: servers that authenticate the UPGRADE (rather
    /// than the first frame) are unreachable without it. The server echoes back the `Authorization`
    /// it saw, so a pass means the value genuinely crossed the wire.
    #[test]
    fn ws_connect_sends_caller_supplied_headers_on_the_handshake() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_header_reporting_server();
        load_body(
            "wsh",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_ws_connect("ws://127.0.0.1:{port}/", {{ headers: {{ Authorization: "Bearer tok-123" }} }})
              .then(function (id) {{
                __s2_ws_on(id, "message", function (m) {{ globalThis.__out = m; }});
                // Subscribe first, then ask — the server replies only on request.
                __s2_ws_send(id, "what-did-you-see");
              }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
              }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsh", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "handshake report never arrived on a drain");
        assert_eq!(read_global_string("wsh", "__out"), "Bearer tok-123");
        shutdown();
    }

    /// A reserved header rejects the connect Promise instead of silently dropping it — a plugin
    /// that thinks it authenticated must not get an anonymous socket.
    #[test]
    fn ws_connect_reserved_header_rejects_the_promise() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_header_reporting_server();
        load_body(
            "wsr",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_ws_connect("ws://127.0.0.1:{port}/", {{ headers: {{ Host: "evil.example" }} }})
              .then(function () {{ globalThis.__out = "RESOLVED"; }})
              .catch(function (e) {{ globalThis.__out = "REJECTED:" + String(e); }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsr", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "connect never settled");
        let out = read_global_string("wsr", "__out");
        assert!(out.starts_with("REJECTED:"), "expected a rejection, got: {out}");
        assert!(out.contains("reserved"), "rejection should name the cause: {out}");
        shutdown();
    }

    /// `__s2_ws_connect` end-to-end against a local ws echo server: the native hands off to the
    /// tokio engine and returns a pending Promise immediately (never blocking the calling thread);
    /// the connect Promise resolves with the conn id on a LATER `frame_async_drain()` — and its
    /// `.then` continuation (which subscribes `__s2_ws_on(id,"message",...)` and sends "hi") runs
    /// THAT SAME drain, before the checkpoint returns (the load-bearing ordering: resolve happens
    /// inside the drain so the plugin can subscribe before any message could arrive). The echoed
    /// "message" event is then queued and fanned out by `dispatch_pending_ws_events` (post-drain,
    /// HOST free) — proving the whole natives + signal-routing + WS_EVENT_MUX spine together.
    #[test]
    fn ws_connect_send_on_message_round_trips_the_echo() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_echo_server();
        load_body(
            "wsp",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_ws_connect("ws://127.0.0.1:{port}/").then(function (id) {{
                __s2_ws_on(id, "message", function (m) {{ globalThis.__out = m; }});
                __s2_ws_send(id, "hi");
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsp", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "ws message never arrived on a drain");
        assert_eq!(read_global_string("wsp", "__out"), "hi");
        shutdown();
    }

    /// A ws connect failure (connection refused) REJECTS the connect Promise (the `.catch` runs)
    /// rather than resolving or panicking — mirrors `fetch_native_bad_host_rejects_the_promise`,
    /// proving `resolve_ws_connect`'s `Err` branch + the drain's `ConnectFailed` routing (incl. the
    /// `ws::drop_conn` cleanup of the now-dead registry entry).
    #[test]
    fn ws_connect_bad_host_rejects_the_promise() {
        init(dummy_logger()).unwrap();
        load_body(
            "wsbad",
            r#"
            globalThis.__out = "pending";
            __s2_ws_connect("ws://127.0.0.1:1/").then(function (id) {
                globalThis.__out = "should-not-resolve:" + id;
            }).catch(function (e) {
                globalThis.__out = "rejected:" + (String(e).length > 0);
            });
        "#,
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsbad", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "ws connect promise never settled on a drain");
        assert_eq!(read_global_string("wsbad", "__out"), "rejected:true");
        shutdown();
    }

    /// Regression for the owner-scoping finding: `__s2_ws_on` must verify the CALLING plugin owns
    /// the conn id, exactly like `__s2_ws_send`/`__s2_ws_close` already do — a co-loaded plugin that
    /// never opened a connection must NOT be able to subscribe to (and read) another plugin's
    /// inbound WebSocket traffic by guessing/reusing its numeric conn id.
    #[test]
    fn ws_on_wrong_owner_does_not_subscribe() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_echo_server();

        // Plugin A opens the only connection.
        load_body(
            "wsOwnerA",
            &format!(
                r#"
            globalThis.__connId = -1;
            __s2_ws_connect("ws://127.0.0.1:{port}/").then(function (id) {{
                globalThis.__connId = id;
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut a_id = -1;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            a_id = read_i32_global_in("wsOwnerA", "__connId");
            if a_id >= 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(a_id >= 0, "plugin A's connect never resolved");

        // Plugin B never opened anything — it tries to subscribe directly to A's numeric conn id.
        load_body("wsOwnerB", r#"globalThis.__spied = "none";"#, "{}");
        eval_in_context(
            "wsOwnerB",
            &format!(r#"__s2_ws_on({a_id}, "message", function (m) {{ globalThis.__spied = m; }});"#, a_id = a_id),
        )
        .expect("eval in wsOwnerB failed");

        // A sends a message on its own conn; the local echo server echoes it back as a "message" event.
        eval_in_context("wsOwnerA", &format!(r#"__s2_ws_send({a_id}, "secret-from-A");"#, a_id = a_id))
            .expect("eval in wsOwnerA failed");

        for _ in 0..200 {
            frame_async_drain();
            dispatch_pending_ws_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            read_global_string("wsOwnerB", "__spied"),
            "none",
            "a non-owning plugin must not receive another plugin's ws message"
        );
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // WebSocket Task 3: `@s2script/ws` — the __s2pkg_ws prelude runtime (the `WebSocket` handle
    // over __s2_ws_connect/send/close/on, mirroring @s2script/http's fetch wrapper).
    // ---------------------------------------------------------------------------

    /// The module resolves via `require("@s2script/ws")` (the generic `s2require` rule) and
    /// exposes `WebSocket.connect` (the named export) as a function.
    #[test]
    fn ws_module_resolves_with_expected_shape() {
        init(dummy_logger()).unwrap();
        load_body(
            "wsshape",
            r#"
            var { WebSocket } = require("@s2script/ws");
            globalThis.__out = String(typeof WebSocket.connect === "function");
        "#,
            "{}",
        );
        assert_eq!(read_global_string("wsshape", "__out"), "true");
        shutdown();
    }

    /// End-to-end through the PUBLIC `@s2script/ws` API (not the raw `__s2_ws_*` natives): connect
    /// against a local ws echo server, subscribe `onMessage`, send a message, and read the echoed
    /// reply back through the wrapper's `WebSocket` handle — proving the prelude the module builds
    /// over the raw natives (connect resolves a handle object; `onMessage`/`send` close over its
    /// conn id).
    #[test]
    fn ws_module_connect_send_on_message_round_trip() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_echo_server();
        load_body(
            "wsmod",
            &format!(
                r#"
            var {{ WebSocket }} = require("@s2script/ws");
            globalThis.__out = "pending";
            WebSocket.connect("ws://127.0.0.1:{port}/").then(function (ws) {{
                ws.onMessage(function (m) {{ globalThis.__out = m; }});
                ws.send("hi");
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsmod", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "ws module message never arrived on a drain");
        assert_eq!(read_global_string("wsmod", "__out"), "hi");
        shutdown();
    }

    /// Regression: a plugin that calls `ws.close()` from inside its OWN `onMessage` handler —
    /// exactly `plugins/ws-demo`'s pattern (log the echo, then close) — must still see `onClose`
    /// fire. A self-initiated close used to be a silent `write.send(Close) + break` with NO
    /// `WsSignal` emitted, so `onClose` (and the ledger's `ws::drop_conn` registry cleanup, which
    /// is driven off that same `Closed` signal in the drain) never ran.
    /// A connection that dies the instant it is established must still reach the plugin's onClose.
    ///
    /// This pins the drain's ordering: `Connected` resolves the connect Promise, but the `.then` that
    /// subscribes does not run until the microtask checkpoint, so the conn must NOT be deregistered
    /// until after it. Dropping it inside the signal loop — as this did — silently refused the
    /// subscribe and dropped the close event, leaving `__out` "pending" forever with the plugin
    /// holding a resolved Promise and no way to learn anything had happened.
    #[test]
    fn a_connection_that_dies_at_once_still_reaches_on_close() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_instant_close_server();
        load_body(
            "wsdead",
            &format!(
                r#"
            var {{ WebSocket }} = require("@s2script/ws");
            globalThis.__out = "pending";
            WebSocket.connect("ws://127.0.0.1:{port}/").then(function (ws) {{
                ws.onClose(function (code, reason) {{ globalThis.__out = "closed:" + code; }});
            }}).catch(function (e) {{ globalThis.__out = "rejected"; }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut settled = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsdead", "__out") != "pending" { settled = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Either outcome is legitimate — the handshake may lose the race and reject — but SILENCE is
        // not: a resolved connect whose close nobody hears is the bug.
        assert!(settled, "neither onClose nor catch ever ran: the close was delivered to nobody");
        let out = read_global_string("wsdead", "__out");
        assert!(
            out.starts_with("closed:") || out == "rejected",
            "expected a close code or a rejection, got {out:?}"
        );
        shutdown();
    }

    #[test]
    fn ws_module_self_close_fires_on_close() {
        // The CAPTURING logger, not dummy_logger(). This test has failed in CI and never locally,
        // and the natives on its path (__s2_ws_on / _send / _close) report a refused ownership gate
        // by WARN — which dummy_log_fn silently threw away, so the one diagnostic that would explain
        // the failure was guaranteed to be invisible in the only place it mattered.
        LOG.lock().unwrap().clear();
        init(logger as LogFn).unwrap();
        let port = spawn_local_ws_echo_server();
        load_body(
            "wsclose",
            &format!(
                r#"
            var {{ WebSocket }} = require("@s2script/ws");
            globalThis.__out = "pending";
            // __stage records how far the chain got. "onClose never fired" is true of a connect that
            // never settled, an echo that never came back, and a close that produced no signal — three
            // different bugs. This test has failed in CI and passed locally, where the difference
            // between those three was the entire question.
            globalThis.__stage = "loaded";
            WebSocket.connect("ws://127.0.0.1:{port}/").then(function (ws) {{
                globalThis.__stage = "connected";
                ws.onMessage(function (m) {{ globalThis.__stage = "echoed"; ws.close(); }});
                ws.onClose(function (code, reason) {{ globalThis.__out = "closed:" + code + ":" + reason; }});
                ws.send("hi");
                globalThis.__stage = "sent";
            }}).catch(function (e) {{
                globalThis.__stage = "rejected";
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsclose", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Dump what core actually SAID. "reached stage 'sent'" narrowed this to "the subscribe never
        // took", but not why; a refused ownership gate WARNs, and that line is the difference between
        // "the conn was gone" and "the socket went quiet".
        let logged = LOG.lock().unwrap().clone();
        assert!(
            resolved,
            "onClose never fired for a self-initiated close (reached stage '{}')\n  core log:\n{}",
            read_global_string("wsclose", "__stage"),
            logged.iter().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
        );
        assert_eq!(read_global_string("wsclose", "__out"), "closed:1000:");
        shutdown();
    }

    // ---------------------------------------------------------------------------
    // Net Task 2: __s2_net_* natives + Uint8Array marshalling + signal routing (connect resolver +
    // event mux) — the async spine over core/src/net.rs's tokio TCP/UDP engine (Task 1). These
    // exercise the ONE net-new mechanism (binary Uint8Array <-> Vec<u8> marshalling) end to end
    // in-isolate; the higher-level `@s2script/net` prelude (Task 3) + live gate (Task 4) build on it.
    // ---------------------------------------------------------------------------

    /// A tiny local TCP echo server on an ephemeral port (a std listener + thread — independent of the
    /// tokio runtime, which drives the CLIENT side). Reads one chunk, echoes it back verbatim.
    fn spawn_local_tcp_echo_server() -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 64];
                if let Ok(n) = s.read(&mut buf) {
                    if n > 0 { let _ = s.write_all(&buf[..n]); }
                }
            }
        });
        port
    }

    /// The full binary round-trip: `__s2_net_tcp_connect` resolves the conn Promise on a later drain;
    /// its `.then` subscribes `__s2_net_on(id,"data",...)` and sends a `Uint8Array([104,105])` ("hi").
    /// `js_bytes_arg` COPIES those bytes out of the typed array on the send path; the echo comes back
    /// and the drain's Data routing → `dispatch_pending_net_events` → `bytes_to_uint8array` hands the
    /// handler a fresh JS `Uint8Array` it can `.length`/index. Proves BOTH marshalling directions +
    /// the whole natives/signal-routing/NET_EVENT_MUX spine together (the net-new mechanism this task
    /// adds — no live socket in a real game needed to verify the copy-in/copy-out).
    #[test]
    fn net_tcp_connect_send_data_round_trips_the_echo() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_tcp_echo_server();
        load_body(
            "netp",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_net_tcp_connect("127.0.0.1", {port}).then(function (id) {{
                __s2_net_on(id, "data", function (bytes) {{
                    var s = "len=" + bytes.length + ":";
                    for (var i = 0; i < bytes.length; i++) s += bytes[i] + ",";
                    globalThis.__out = s;
                }});
                __s2_net_send(id, new Uint8Array([104, 105]));
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_net_events();
            if read_global_string("netp", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "net data event never arrived on a drain");
        // Uint8Array([104,105]) echoed back, handed to the handler as a fresh indexable Uint8Array.
        assert_eq!(read_global_string("netp", "__out"), "len=2:104,105,");
        shutdown();
    }

    /// A TCP connect failure (connection refused — port 1) REJECTS the connect Promise (the `.catch`
    /// runs) rather than resolving or panicking — proves `resolve_net_connect`'s `Err` branch + the
    /// drain's `ConnectFailed` routing (incl. the `net::drop_conn` cleanup of the dead registry entry).
    /// Mirrors `ws_connect_bad_host_rejects_the_promise`.
    #[test]
    fn net_connect_bad_port_rejects_the_promise() {
        init(dummy_logger()).unwrap();
        load_body(
            "netbad",
            r#"
            globalThis.__out = "pending";
            __s2_net_tcp_connect("127.0.0.1", 1).then(function (id) {
                globalThis.__out = "should-not-resolve:" + id;
            }).catch(function (e) {
                globalThis.__out = "rejected:" + (String(e).length > 0);
            });
        "#,
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_net_events();
            if read_global_string("netbad", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "net connect promise never settled on a drain");
        assert_eq!(read_global_string("netbad", "__out"), "rejected:true");
        shutdown();
    }

    /// A tiny local UDP echo server on an ephemeral port (mirrors `spawn_local_tcp_echo_server`, but
    /// over a `std::net::UdpSocket` independent of the tokio runtime driving the CLIENT side). Reads
    /// ONE datagram of any length (including zero) and echoes the same bytes straight back.
    fn spawn_local_udp_echo_server() -> u16 {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            if let Ok((n, from)) = socket.recv_from(&mut buf) {
                let _ = socket.send_to(&buf[..n], from);
            }
        });
        port
    }

    /// Final-review Fix 1: a zero-length UDP datagram is a REACHABLE input (`net.rs`'s `recv_from`
    /// returns `Ok((0, from))` for an empty datagram -> `Datagram { data: vec![] }`), and it is the
    /// only net-new code path `bytes_to_uint8array` didn't exercise before this fix. Sends an empty
    /// `Uint8Array` to a local UDP echo server, which echoes 0 bytes back; asserts the "message"
    /// handler receives a REAL `Uint8Array` (not null/undefined) with `.length === 0` — driving
    /// `bytes_to_uint8array(&[])`'s fresh-`ArrayBuffer::new(scope, 0)` path end to end.
    #[test]
    fn net_udp_empty_datagram_round_trips_as_zero_length_uint8array() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_udp_echo_server();
        load_body(
            "netudp",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_net_udp_bind().then(function (id) {{
                __s2_net_on(id, "message", function (from, bytes) {{
                    globalThis.__out = "isArr=" + (bytes instanceof Uint8Array) + ":len=" + bytes.length;
                }});
                __s2_net_send_to(id, "127.0.0.1", {port}, new Uint8Array(0));
            }}).catch(function (e) {{
                globalThis.__out = "ERROR:" + String(e);
            }});
        "#,
                port = port
            ),
            "{}",
        );
        let mut resolved = false;
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_net_events();
            if read_global_string("netudp", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "net udp empty-datagram message event never arrived on a drain");
        assert_eq!(read_global_string("netudp", "__out"), "isArr=true:len=0");
        shutdown();
    }

    // --- Menu primitive Task 1: the pure Menu model + pagination + registerRenderer seam. ---
    // A test-only "record renderer" captures each computed `view()` so the model is fully
    // unit-testable with NO chat/timers/clients dependency.

    #[test]
    fn menu_model_pagination_pick_cursor() {
        init(dummy_logger()).unwrap();
        // Pagination: 9 items, exitButton -> page 0 shows items 1..7 as keys "1".."7",
        // then control keys 9=Next, 0=Exit (no Back on page 0).
        let out = eval_std("mp", r#"
            var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
            var captured = [];
            Menu.registerRenderer("rec", {
                open: function (s) { captured.push(s.view()); },
                update: function (s) { captured.push(s.view()); },
                close: function () {},
            });
            var m = new Menu("T");
            m.style = "rec";
            for (var i = 0; i < 9; i++) m.addItem("info" + i, "Item " + i);
            var picked = null;
            m.onSelect(function (e) { picked = e.info + ":" + e.item; });
            m.display(3, 0);
            var v0 = captured[captured.length - 1];
            // 7 selectable item-lines on page 0
            var itemKeys = v0.lines.filter(function (l) { return l.selectable; }).map(function (l) { return l.key; });
            // control keys present: Next="9", Exit="0"; no Back
            var ctrlKeys = v0.lines.filter(function (l) { return !l.selectable && l.key; }).map(function (l) { return l.key; });
            JSON.stringify({ items: itemKeys, ctrl: ctrlKeys, pageCount: v0.pageCount });
        "#);
        assert_eq!(out, r#"{"items":["1","2","3","4","5","6","7"],"ctrl":["9","0"],"pageCount":2}"#);
        shutdown();
    }

    #[test]
    fn menu_model_next_page_and_select() {
        init(dummy_logger()).unwrap();
        let out = eval_std("mn", r#"
            var { Menu } = globalThis.__s2pkg_menu;
            var last = null;
            Menu.registerRenderer("rec2", { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
            var m = new Menu("T"); m.style = "rec2";
            for (var i = 0; i < 9; i++) m.addItem("info" + i, "Item " + i);
            var picked = null; m.onSelect(function (e){ picked = e.info + ":" + e.item; });
            m.display(3, 0);
            last.pickNumber(9);          // Next -> page 1 (items 8,9 => "info7","info8")
            last.pickNumber(1);          // first item on page 1 = index 7
            picked;
        "#);
        assert_eq!(out, "info7:7");
        shutdown();
    }

    #[test]
    fn menu_model_disabled_item_not_selectable() {
        init(dummy_logger()).unwrap();
        let out = eval_std("md", r#"
            var { Menu } = globalThis.__s2pkg_menu;
            var last = null;
            Menu.registerRenderer("rec3", { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
            var m = new Menu("T"); m.style = "rec3";
            m.addItem("a", "A", { disabled: true });
            m.addItem("b", "B");
            var picked = "none"; m.onSelect(function (e){ picked = e.info; });
            m.display(3, 0);
            // disabled "a" has no number; "b" is key "1"
            var v = last.view();
            var aLine = v.lines[0], bLine = v.lines[1];
            last.pickNumber(1);   // selects "b"
            JSON.stringify({ aKey: aLine.key, aSel: aLine.selectable, bKey: bLine.key, picked: picked });
        "#);
        assert_eq!(out, r#"{"aKey":null,"aSel":false,"bKey":"1","picked":"b"}"#);
        shutdown();
    }

    #[test]
    fn menu_model_center_cursor_and_confirm() {
        init(dummy_logger()).unwrap();
        let out = eval_std("mc", r#"
            var { Menu } = globalThis.__s2pkg_menu;
            var last = null;
            Menu.registerRenderer("rec4", { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
            var m = new Menu("T"); m.style = "rec4";
            m.addItem("x", "X"); m.addItem("y", "Y"); m.addItem("z", "Z");
            var picked = null; m.onSelect(function (e){ picked = e.info; });
            m.display(3, 0);
            last.moveDown();     // cursor 0 -> 1 (Y)
            last.confirm();      // selects Y
            picked;
        "#);
        assert_eq!(out, "y");
        shutdown();
    }

    #[test]
    fn menu_model_center_style_rendered_cursor_flag() {
        init(dummy_logger()).unwrap();
        // MenuSession must resolve `cursor` off the owning Menu's style (MenuStyle.Center), not
        // an (unset) session-local `.style` -- else every rendered line's `cursor` is always false,
        // even for a Center-style menu, silently breaking the center renderer's highlight.
        let out = eval_std("mcs", r#"
            var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
            var last = null;
            Menu.registerRenderer(MenuStyle.Center, { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
            var m = new Menu("T"); m.style = MenuStyle.Center;
            m.addItem("x", "X"); m.addItem("y", "Y"); m.addItem("z", "Z");
            m.display(3, 0);
            last.moveDown();     // cursor 0 -> 1 (Y)
            var v = last.view();
            // only the 3 item-lines carry a `cursor` flag; control lines (e.g. Exit) don't set one.
            var cursorFlags = v.lines.filter(function (l) { return l.selectable; }).map(function (l) { return l.cursor; });
            JSON.stringify({ cursorFlags: cursorFlags, highlightedText: v.lines[1].text });
        "#);
        assert_eq!(out, r#"{"cursorFlags":[false,true,false],"highlightedText":"Y"}"#);
        shutdown();
    }

    #[test]
    fn menu_model_center_paginate_and_exit() {
        init(dummy_logger()).unwrap();
        // A center menu's cursor must reach the Next/Back/Exit controls (not just items) so pages
        // beyond 1 are reachable + the menu is dismissable. 9 items + exitButton -> page-0 nav targets =
        // [item0..item6 (7), next (idx7), exit (idx8)].
        let out = eval_std("mcp", r#"
            var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
            var last = null, picked = null;
            Menu.registerRenderer(MenuStyle.Center, { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
            var m = new Menu("T"); m.style = MenuStyle.Center;
            for (var i = 0; i < 9; i++) m.addItem("info" + i, "Item " + i);
            m.onSelect(function (e){ picked = e.info; });
            m.display(3, 0);
            last.moveUp();   // wrap 0 -> idx 8 (Exit control)
            var onExit = last.view().lines.filter(function(l){return l.control==="exit";})[0].cursor;
            last.moveUp();   // -> idx 7 (Next control)
            var onNext = last.view().lines.filter(function(l){return l.control==="next";})[0].cursor;
            last.confirm();  // Next -> page 1 (items 7,8), cursor 0
            var pageAfterNext = last.page;
            var page1first = last.view().lines.filter(function(l){return l.selectable;})[0].text;
            last.confirm();  // select page-1 item 0 == info7
            JSON.stringify({ onExit: onExit, onNext: onNext, page: pageAfterNext, page1first: page1first, picked: picked });
        "#);
        assert_eq!(out, r#"{"onExit":true,"onNext":true,"page":1,"page1first":"Item 7","picked":"info7"}"#);
        shutdown();
    }

    #[test]
    fn menu_model_center_exit_cancels() {
        init(dummy_logger()).unwrap();
        // Confirming the Exit control on a center menu cancels it with reason Exit (0) -- a seconds:0
        // center menu is dismissable by the player (the review-1 gap).
        let out = eval_std("mce", r#"
            var { Menu, MenuStyle, MenuCancelReason } = globalThis.__s2pkg_menu;
            var cancelled = null;
            Menu.registerRenderer(MenuStyle.Center, { open: function (s){ last = s; }, update: function (s){ last = s; }, close: function(){} });
            var last = null;
            var m = new Menu("T"); m.style = MenuStyle.Center;
            m.addItem("a", "A"); m.onCancel(function (e){ cancelled = e.reason; });
            m.display(3, 0);
            last.moveDown();  // item(0) -> exit(1)
            last.confirm();   // Exit -> cancel
            JSON.stringify({ cancelled: cancelled, exitReason: MenuCancelReason.Exit });
        "#);
        assert_eq!(out, r#"{"cancelled":0,"exitReason":0}"#);
        shutdown();
    }

    #[test]
    fn menu_model_newmenu_replaces_and_reentrant_display_wins() {
        init(dummy_logger()).unwrap();
        // A 2nd display to a slot cancels the 1st with NewMenu (3); and if that onCancel synchronously
        // displays a re-entrant menu for the slot, the re-entrant one must WIN (not be clobbered by the
        // outer display) -- the review-2 guard.
        let out = eval_std("mnm", r#"
            var { Menu, MenuCancelReason } = globalThis.__s2pkg_menu;
            var opened = [];
            Menu.registerRenderer("recX", { open: function (s){ opened.push(s.menu.title); }, update: function(){}, close: function(){} });
            var reentrant = new Menu("REENTRANT"); reentrant.style = "recX"; reentrant.addItem("r","R");
            var first = new Menu("FIRST"); first.style = "recX"; first.addItem("a","A");
            var firstCancelReason = null;
            first.onCancel(function (e){ firstCancelReason = e.reason; reentrant.display(3, 0); });
            var second = new Menu("SECOND"); second.style = "recX"; second.addItem("b","B");
            first.display(3, 0);    // opens FIRST
            second.display(3, 0);   // cancels FIRST(NewMenu) -> onCancel opens REENTRANT -> SECOND abandoned
            JSON.stringify({ firstCancelReason: firstCancelReason, newMenu: MenuCancelReason.NewMenu, opened: opened });
        "#);
        assert_eq!(out, r#"{"firstCancelReason":3,"newMenu":3,"opened":["FIRST","REENTRANT"]}"#);
        shutdown();
    }

    #[test]
    fn menu_freeze_player_flag_default_false_and_settable() {
        init(dummy_logger()).unwrap();
        // freezePlayer is an engine-generic Menu flag (default false = movement allowed); the CS2 center
        // renderer honors it. The generic model just carries it.
        let out = eval_std("mfp", r#"
            var { Menu } = globalThis.__s2pkg_menu;
            var a = new Menu("A");
            var b = new Menu("B"); b.freezePlayer = true;
            JSON.stringify({ def: a.freezePlayer, set: b.freezePlayer });
        "#);
        assert_eq!(out, r#"{"def":false,"set":true}"#);
        shutdown();
    }

    // --- Menu primitive Task 2: the built-in chat renderer (over __s2pkg_chat) + lifecycle. ---

    #[test]
    fn menu_chat_renders_and_number_selects() {
        init(dummy_logger()).unwrap();
        let out = eval_std("mchat", r#"
            var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
            // capture chat lines sent to the slot
            var sent = [];
            var realToSlot = globalThis.__s2pkg_chat.Chat.toSlot;
            globalThis.__s2pkg_chat.Chat.toSlot = function (s, msg) { sent.push([s, msg]); };
            // capture the onMessage handler the renderer installs
            var chatHandler = null;
            var realOn = globalThis.__s2_chat_on_message;
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            var m = new Menu("Pick"); m.style = MenuStyle.Chat;
            m.addItem("kick", "Kick"); m.addItem("ban", "Ban");
            var got = null; m.onSelect(function (e){ got = e.info; });
            m.display(3, 0);
            // simulate slot 3 typing "2"
            var suppressed = chatHandler(3, "2", false);
            // restore
            globalThis.__s2pkg_chat.Chat.toSlot = realToSlot;
            globalThis.__s2_chat_on_message = realOn;
            JSON.stringify({ sentCount: sent.length > 0, picked: got, suppressed: suppressed });
        "#);
        // "2" -> second item "ban"; a matched pick suppresses the chat line (>=2)
        assert_eq!(out, r#"{"sentCount":true,"picked":"ban","suppressed":2}"#);
        shutdown();
    }

    #[test]
    fn menu_chat_nonmatching_message_passes_through() {
        init(dummy_logger()).unwrap();
        let out = eval_std("mchat2", r#"
            var { Menu, MenuStyle } = globalThis.__s2pkg_menu;
            var chatHandler = null;
            var realOn = globalThis.__s2_chat_on_message;
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            var m = new Menu("P"); m.style = MenuStyle.Chat; m.addItem("a", "A");
            m.display(3, 0);
            var r1 = chatHandler(3, "hello", false);   // not a digit -> pass through (undefined/0)
            var r2 = chatHandler(4, "1", false);        // different slot -> pass through
            globalThis.__s2_chat_on_message = realOn;
            JSON.stringify({ r1: r1 == null || r1 < 2, r2: r2 == null || r2 < 2 });
        "#);
        assert_eq!(out, r#"{"r1":true,"r2":true}"#);
        shutdown();
    }

    /// adminmenu Task 1: a plugin registers a category + two items; `snapshot()` returns them (metadata
    /// only, no functions) — reachable from a DIFFERENT plugin context (the registry is host-global,
    /// like CONCOMMANDS), proving cross-context owner-scoped visibility.
    #[test]
    fn topmenu_add_snapshot_and_owner_scoped() {
        init(dummy_logger()).unwrap();
        load_body("tm_a", r#"
            var { TopMenu } = globalThis.__s2pkg_topmenu;
            TopMenu.addCategory("Player Commands");
            TopMenu.addItem("Player Commands", { id: "a:kick", name: "Kick", flags: 8, onSelect: function(){} });
            TopMenu.addItem("Player Commands", { id: "a:slap", name: "Slap", flags: 16, onSelect: function(){} });
        "#, "{}");
        // Build a NEW plain object with an explicit key order in the test itself (rather than
        // stringifying `kick` directly) — independent of whichever key order the native's JSON
        // round-trip happens to produce (an implementation detail, not a contract).
        let out = eval_std("q1", r#"
            var s = globalThis.__s2pkg_topmenu.TopMenu.snapshot();
            var kick = s.items.filter(function(i){return i.id==="a:kick";})[0];
            JSON.stringify({ cats: s.categories, ids: s.items.map(function(i){return i.id;}).sort(),
                             kickId: kick.id, kickCategory: kick.category, kickName: kick.name, kickFlags: kick.flags });
        "#);
        assert_eq!(out, r#"{"cats":["Player Commands"],"ids":["a:kick","a:slap"],"kickId":"a:kick","kickCategory":"Player Commands","kickName":"Kick","kickFlags":8}"#);
        shutdown();
    }

    /// adminmenu Task 1: `TopMenu.select` only QUEUES (never synchronous — a menu onSelect runs under
    /// the isolate borrow, so a synchronous cross-context dispatch would double-borrow); the owner's
    /// `onSelect` fires only once `dispatch_pending_topmenu_select` runs post-drain (HOST free).
    #[test]
    fn topmenu_select_dispatches_to_owner_post_drain() {
        init(dummy_logger()).unwrap();
        load_body("tm_b", r#"
            var { TopMenu } = globalThis.__s2pkg_topmenu;
            globalThis.__tm_picked = null;
            TopMenu.addItem("Player Commands", { id: "b:kick", name: "Kick", flags: 8,
                onSelect: function(slot){ globalThis.__tm_picked = "b:kick@" + slot; } });
        "#, "{}");
        // select QUEUES; it must NOT have fired yet (synchronous would double-borrow).
        eval_std("q2", r#" globalThis.__s2pkg_topmenu.TopMenu.select("b:kick", 3); "#);
        assert_eq!(eval_in_context_string("tm_b", r#" String(globalThis.__tm_picked) "#), "null",
            "select must not dispatch synchronously");
        // fan out post-drain (HOST free) — dispatch runs the owner's onSelect.
        dispatch_pending_topmenu_select();
        let out = eval_in_context_string("tm_b", r#" String(globalThis.__tm_picked) "#);
        assert_eq!(out, "b:kick@3");
        shutdown();
    }

    /// adminmenu Task 1: unload drops the departing plugin's TopMenu items (owner-scoped teardown,
    /// mirrors the CONCOMMANDS cleanup) — a subsequent snapshot no longer lists them.
    #[test]
    fn topmenu_unload_drops_owner_items() {
        init(dummy_logger()).unwrap();
        load_body("tm_c", r#"
            var { TopMenu } = globalThis.__s2pkg_topmenu;
            TopMenu.addItem("Player Commands", { id: "c:ban", name: "Ban", flags: 2, onSelect: function(){} });
        "#, "{}");
        unload_plugin("tm_c");   // Vanished
        let out = eval_std("q3", r#" String(globalThis.__s2pkg_topmenu.TopMenu.snapshot().items.length) "#);
        assert_eq!(out, "0");   // the departed plugin's item is gone
        shutdown();
    }

    #[test]
    fn topmenu_snapshot_preserves_registration_order() {
        init(dummy_logger()).unwrap();
        // snapshot must return items in REGISTRATION order (by seq), not random HashMap order — the spec
        // commits the MVP to insertion order + stable-across-restarts. Register many so a HashMap would
        // very likely scramble them.
        load_body("tm_ord", r#"
            var { TopMenu } = globalThis.__s2pkg_topmenu;
            ["zeta","alpha","mike","bravo","yankee","charlie","delta","echo"].forEach(function (n, i) {
                TopMenu.addItem("Player Commands", { id: "ord:" + i, name: n, flags: 0, onSelect: function(){} });
            });
        "#, "{}");
        let out = eval_std("qord", r#"
            globalThis.__s2pkg_topmenu.TopMenu.snapshot().items.map(function (i) { return i.name; }).join(",")
        "#);
        assert_eq!(out, "zeta,alpha,mike,bravo,yankee,charlie,delta,echo");
        shutdown();
    }

    // --- basevotes Task 1: @s2script/votes — chat-ballot voting (revote) + an optional live tally. ---

    #[test]
    fn votes_cast_revote_tally_and_winner() {
        init(dummy_logger()).unwrap();
        let out = eval_std("vt1", r#"
            var sent = [], chatHandler = null, delayed = [];
            globalThis.__s2pkg_chat.Chat.toAll = function (m) { sent.push(m); };
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false},{slot:1,isBot:false},{slot:9,isBot:true}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
            var res = null;
            var ok = globalThis.__s2pkg_votes.Vote.start({ question:"Q", options:["A","B"], duration:2, onEnd:function(r){ res = r; } });
            var handled = chatHandler(0, "1");   // slot0 -> A
            chatHandler(1, "2");                 // slot1 -> B
            chatHandler(0, "2");                 // slot0 REVOTE -> B
            while (delayed.length) delayed.shift()();   // drain the countdown -> end
            JSON.stringify({ ok:ok, handled:handled, counts:res.counts, total:res.total, winner:res.winner });
        "#);
        // slot0 revoted to B, slot1 B -> A:0 B:2, winner index 1
        assert_eq!(out, r#"{"ok":true,"handled":2,"counts":[0,2],"total":2,"winner":1}"#);
        shutdown();
    }

    #[test]
    fn votes_tie_and_zero_are_null_winner_and_lock() {
        init(dummy_logger()).unwrap();
        let out = eval_std("vt2", r#"
            var chatHandler = null, delayed = [];
            globalThis.__s2pkg_chat.Chat.toAll = function () {};
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false},{slot:1,isBot:false}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
            var V = globalThis.__s2pkg_votes.Vote, res = null;
            V.start({ question:"Q", options:["A","B"], duration:1, onEnd:function(r){ res = r; } });
            var second = V.start({ question:"Q2", options:["A","B"], duration:1, onEnd:function(){} });  // locked out
            var activeMid = V.isActive();
            chatHandler(0, "1"); chatHandler(1, "2");   // 1-1 tie
            while (delayed.length) delayed.shift()();
            JSON.stringify({ second:second, activeMid:activeMid, winner:res.winner, activeEnd:V.isActive() });
        "#);
        assert_eq!(out, r#"{"second":false,"activeMid":true,"winner":null,"activeEnd":false}"#);
        shutdown();
    }

    #[test]
    fn votes_live_tally_renderer_show_and_clear() {
        init(dummy_logger()).unwrap();
        let out = eval_std("vt3", r#"
            var chatHandler = null, delayed = [], shows = [], clears = [];
            globalThis.__s2pkg_chat.Chat.toAll = function () {};
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
            var V = globalThis.__s2pkg_votes.Vote;
            V.registerTallyRenderer({ show:function(slot,t){ shows.push(slot + ":" + t.options[0].count); }, clear:function(slot){ clears.push(slot); } });
            V.start({ question:"Q", options:["A","B"], duration:1, showLiveTally:true, onEnd:function(){} });
            chatHandler(0, "1");   // A:1
            while (delayed.length) delayed.shift()();
            JSON.stringify({ shows: shows.length > 0 && shows[shows.length-1] === "0:1", cleared: clears.indexOf(0) !== -1 });
        "#);
        assert_eq!(out, r#"{"shows":true,"cleared":true}"#);
        shutdown();
    }

    #[test]
    fn votes_no_live_tally_never_calls_renderer() {
        init(dummy_logger()).unwrap();
        let out = eval_std("vt4", r#"
            var chatHandler = null, delayed = [], calls = 0;
            globalThis.__s2pkg_chat.Chat.toAll = function () {};
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
            var V = globalThis.__s2pkg_votes.Vote;
            V.registerTallyRenderer({ show:function(){ calls++; }, clear:function(){ calls++; } });
            V.start({ question:"Q", options:["A","B"], duration:1, onEnd:function(){} });   // showLiveTally omitted -> false
            chatHandler(0, "1");
            while (delayed.length) delayed.shift()();
            String(calls);
        "#);
        assert_eq!(out, "0");
        shutdown();
    }

    #[test]
    fn votes_ends_early_once_everyone_voted_even_with_time_left() {
        init(dummy_logger()).unwrap();
        let out = eval_std("vt5", r#"
            var chatHandler = null, delayed = [];
            globalThis.__s2pkg_chat.Chat.toAll = function () {};
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false},{slot:1,isBot:false}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
            var V = globalThis.__s2pkg_votes.Vote, res = null;
            V.start({ question:"Q", options:["A","B"], duration:10, onEnd:function(r){ res = r; } });
            chatHandler(0, "1"); chatHandler(1, "1");   // both eligible voters cast -> full turnout
            var endedBeforeDrain = !V.isActive();       // no tick has run yet -> must still be active
            delayed.shift()();                          // drain exactly ONE tick (duration=10, nowhere near 0)
            JSON.stringify({ endedBeforeDrain: endedBeforeDrain, pendingAfterOneTick: delayed.length, active: V.isActive(), winner: res && res.winner, total: res && res.total });
        "#);
        // full turnout ends the vote at the NEXT tick boundary (not synchronously mid-cast, and well
        // before the configured 10s duration elapses) — the reconciled design-doc Flow step 5 behavior.
        assert_eq!(out, r#"{"endedBeforeDrain":false,"pendingAfterOneTick":0,"active":false,"winner":0,"total":2}"#);
        shutdown();
    }

    #[test]
    fn votes_disconnect_drops_that_slots_vote() {
        init(dummy_logger()).unwrap();
        // A voter who disconnects mid-vote has their vote removed (the design doc's required case).
        let out = eval_std("vt6", r#"
            var chatHandler = null, disconnectHandler = null, delayed = [], res = null;
            globalThis.__s2pkg_chat.Chat.toAll = function () {};
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            globalThis.__s2pkg_clients.Clients.onDisconnect = function (fn) { disconnectHandler = fn; };
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false},{slot:1,isBot:false}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
            var V = globalThis.__s2pkg_votes.Vote;
            V.start({ question:"Q", options:["A","B"], duration:2, onEnd:function(r){ res = r; } });
            chatHandler(0, "1");   // slot0 -> A
            chatHandler(1, "2");   // slot1 -> B
            disconnectHandler({ slot: 0 });   // slot0 leaves -> its vote drops
            while (delayed.length) delayed.shift()();
            // A dropped, B remains -> counts [0,1], total 1, winner index 1
            JSON.stringify({ counts: res.counts, total: res.total, winner: res.winner });
        "#);
        assert_eq!(out, r#"{"counts":[0,1],"total":1,"winner":1}"#);
        shutdown();
    }
}
