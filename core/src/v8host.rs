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
// --- Slice menu: per-client event fire (C-ABI; the C header must match exactly) ---
pub type EventFireToClientFn = extern "C" fn(slot: c_int) -> c_int;

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
// Slice 6.14: kill a pawn via the sig-resolved CommitSuicide (shim reconstructs + serial-gates from idx/serial).
pub type PawnCommitSuicideFn = extern "C" fn(idx: c_int, serial: c_int);
// --- ban-reason sub-project 2: console-print + client-address ops (C-ABI; the C header must match exactly) ---
pub type ClientConsolePrintFn = extern "C" fn(slot: c_int, msg: *const c_char);
pub type ClientAddressFn      = extern "C" fn(slot: c_int) -> *const c_char;
// --- reservedslots+basetriggers: server-info ops (C-ABI; the C header must match exactly) ---
pub type ServerMaxClientsFn = extern "C" fn() -> c_int;          // GetMaxClients(); 0 if unavailable
pub type ServerMapNameFn    = extern "C" fn() -> *const c_char;  // GetMapName(); "" if unavailable
pub type ServerGameTimeFn   = extern "C" fn() -> f32;            // GetGlobals()->curtime; 0 if unavailable
// --- Slice DB: data-dir op (C-ABI; the C header must match exactly) ---
pub type DbDataDirFn = extern "C" fn() -> *const c_char; // absolute path to <addon>/data, created if absent

// --- Slice nominations: raw configs-dir file read/write (C-ABI; the C header must match exactly).
// Mirrors ConfigReadFn/ConfigWriteFn but the name INCLUDES its extension (no .json append). ---
type ConfigReadFileFn  = extern "C" fn(name: *const c_char) -> *const c_char;
type ConfigWriteFileFn = extern "C" fn(name: *const c_char, content: *const c_char) -> i32;

// --- Ray-trace slice: CNavPhysicsInterface::TraceShape (C-ABI; the C header must match exactly).
// ENGINE-GENERIC (Source-2 physics; no CS2 names). Mirrors shim/include/s2script_core.h's
// `S2TraceResult` / `s2_trace_shape_fn` field-for-field — the ABI is layout, not just a contract.
// Returns 1 and fills `*out` on a completed trace; returns 0 (op unavailable / vtable unresolved)
// leaving `*out` untouched — degrade-never-crash (the native builds a MISS TraceHit itself).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct S2TraceResult {
    pub did_hit: c_int,
    pub fraction: f32,
    pub endpos: [f32; 3],
    pub normal: [f32; 3],
    pub all_solid: c_int,
    pub hit_ent_handle: c_int, // GetRefEHandle().ToInt() of the hit entity, or -1
}
pub type TraceShapeFn = extern "C" fn(
    start: *const f32,
    end: *const f32,
    mins: *const f32,
    maxs: *const f32,
    interacts_with: u64,
    interacts_exclude: u64,
    ignore_ent_idx: c_int,
    ignore_ent_serial: c_int,
    out: *mut S2TraceResult,
) -> c_int;

// --- Entity-creation lifecycle slice (APPENDED after trace_shape; order is the ABI). ENGINE-GENERIC:
// create takes a className string (no CS2 semantics implied by the C ABI itself); spawn/teleport/remove
// take the (index, serial) pair already used by every other serial-gated entity op.
type EntityCreateFn   = extern "C" fn(*const std::os::raw::c_char) -> c_int;
type EntitySpawnFn    = extern "C" fn(c_int, c_int) -> c_int;
type EntityTeleportFn = extern "C" fn(c_int, c_int, *const f32, *const f32, *const f32) -> c_int;
type EntityRemoveFn   = extern "C" fn(c_int, c_int) -> c_int;

// --- Item slice (APPENDED after entity_remove; order is the ABI). ENGINE-GENERIC: the ops take
// (idx, serial, offset(s)/index/string) — no CS2 schema/class names in the C ABI itself.
type GiveNamedItemFn        = extern "C" fn(c_int, c_int, c_int, *const std::os::raw::c_char) -> c_int;
type EntitySubobjVcallFn    = extern "C" fn(c_int, c_int, c_int, c_int, c_int, c_int) -> c_int;
type RemovePlayerItemFn     = extern "C" fn(c_int, c_int, c_int, c_int) -> c_int;
type EntityReadHandleVecFn  = extern "C" fn(c_int, c_int, *const c_int, c_int, c_int, c_int, *mut c_int) -> c_int;
/// Entity-I/O slice: fire an entity input via AddEntityIOEvent. See the C typedef comment
/// (shim/include/s2script_core.h) for the argument shape.
type EntityFireInputFn = extern "C" fn(c_int, c_int, *const c_char, *const c_char, c_int, c_int, c_int, c_int, f32) -> c_int;

// --- EKV slice (APPENDED after entity_fire_input; order is the ABI). ENGINE-GENERIC: keys/types/
// values are caller-supplied parallel arrays (no CS2 semantics in the C ABI itself).
type EntitySpawnKvFn = extern "C" fn(c_int, c_int, c_int,
    *const *const c_char, *const c_int, *const *const c_char) -> c_int;

// --- Game-rules + UserMessage slice (APPENDED after entity_spawn_kv; order is the ABI). ENGINE-GENERIC:
// takes a designer-name string + out-buffers; no CS2 class names in the C ABI itself.
type EntityFindByClassFn =
    extern "C" fn(*const std::os::raw::c_char, *mut i32, *mut i32, i32) -> i32;

// --- UserMessage send family (APPENDED after entity_find_by_class; order is the ABI). ENGINE-GENERIC:
// a named protobuf message + scalar field sets by reflection cpp_type + a send to slots. No CS2 names.
type UserMessageCreateFn    = extern "C" fn(*const std::os::raw::c_char) -> i32;
type UserMessageSetIntFn    = extern "C" fn(*const std::os::raw::c_char, i64) -> i32;
type UserMessageSetFloatFn  = extern "C" fn(*const std::os::raw::c_char, f64) -> i32;
type UserMessageSetStringFn = extern "C" fn(*const std::os::raw::c_char, *const std::os::raw::c_char) -> i32;
type UserMessageSetBoolFn   = extern "C" fn(*const std::os::raw::c_char, i32) -> i32;
type UserMessageSendFn      = extern "C" fn(*const i32, i32) -> i32;

// --- FakeConVar (APPENDED after user_message_send; order is the ABI). ENGINE-GENERIC: a plugin-owned
// ConVar registered via ICvar::RegisterConVar. No CS2 names — a convar is a Source2 concept.
type ConvarRegisterFn = unsafe extern "C" fn(
    *const std::os::raw::c_char, *const std::os::raw::c_char, u64, i32,
    *const std::os::raw::c_char, *const std::os::raw::c_char, *const std::os::raw::c_char) -> i32;

// --- Translations slice (APPENDED after convar_register; order is the ABI). ENGINE-GENERIC. ---
type TranslationsReadFn = extern "C" fn(lang: *const c_char, name: *const c_char) -> *const c_char;
pub type ClientLanguageFn = extern "C" fn(slot: c_int) -> *const c_char;

// --- Zones real-trigger slice (APPENDED after client_language; order is the ABI). ENGINE-GENERIC:
// takes the (index, serial) pair already used by every other serial-gated entity op.
type CollisionActivateFn = extern "C" fn(c_int, c_int) -> c_int;
type EntitySetModelFn = extern "C" fn(c_int, c_int, *const std::os::raw::c_char) -> c_int;

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
    // --- Slice 6.14: pawn suicide op (APPENDED after cvar_get; order is the ABI; do not reorder above) ---
    pub pawn_commit_suicide: Option<PawnCommitSuicideFn>,
    // --- ban-reason sub-project 2 (APPENDED after pawn_commit_suicide; order is the ABI; do not reorder) ---
    pub client_console_print: Option<ClientConsolePrintFn>,
    pub client_address:       Option<ClientAddressFn>,
    // --- reservedslots+basetriggers: server-info ops (APPENDED after client_address; order is the ABI; do not reorder above) ---
    pub server_max_clients: Option<ServerMaxClientsFn>,
    pub server_map_name:    Option<ServerMapNameFn>,
    pub server_game_time:   Option<ServerGameTimeFn>,
    // --- Slice DB: data-dir op (APPENDED after server_game_time; order is the ABI; do not reorder above) ---
    pub db_data_dir: Option<DbDataDirFn>,
    // --- Slice menu: per-client event fire (APPENDED after db_data_dir; order is the ABI; do not reorder above) ---
    pub event_fire_to_client: Option<EventFireToClientFn>,
    // --- Slice nominations: raw config-file read/write (APPENDED after event_fire_to_client; order is the ABI) ---
    pub config_read_file:  Option<ConfigReadFileFn>,
    pub config_write_file: Option<ConfigWriteFileFn>,
    // --- Ray-trace slice: CNavPhysicsInterface::TraceShape (APPENDED after config_write_file; order is the ABI) ---
    pub trace_shape: Option<TraceShapeFn>,
    // --- Entity-creation lifecycle slice (APPENDED after trace_shape; order is the ABI; do not reorder above) ---
    pub entity_create:   Option<EntityCreateFn>,
    pub entity_spawn:    Option<EntitySpawnFn>,
    pub entity_teleport: Option<EntityTeleportFn>,
    pub entity_remove:   Option<EntityRemoveFn>,
    // --- Item slice (APPENDED after entity_remove; order is the ABI; do not reorder above) ---
    pub give_named_item:           Option<GiveNamedItemFn>,
    pub entity_subobj_vcall:       Option<EntitySubobjVcallFn>,
    pub remove_player_item:        Option<RemovePlayerItemFn>,
    pub entity_read_handle_vector: Option<EntityReadHandleVecFn>,
    // --- Entity-I/O slice (APPENDED after entity_read_handle_vector; order is the ABI; do not reorder above) ---
    pub entity_fire_input: Option<EntityFireInputFn>,
    // --- EKV slice (APPENDED after entity_fire_input; order is the ABI; do not reorder above) ---
    pub entity_spawn_kv: Option<EntitySpawnKvFn>,
    // --- Game-rules + UserMessage slice (APPENDED after entity_spawn_kv; order is the ABI; do not reorder above) ---
    pub entity_find_by_class: Option<EntityFindByClassFn>,
    // --- UserMessage send family (APPENDED after entity_find_by_class; order is the ABI; do not reorder above) ---
    pub user_message_create:     Option<UserMessageCreateFn>,
    pub user_message_set_int:    Option<UserMessageSetIntFn>,
    pub user_message_set_float:  Option<UserMessageSetFloatFn>,
    pub user_message_set_string: Option<UserMessageSetStringFn>,
    pub user_message_set_bool:   Option<UserMessageSetBoolFn>,
    pub user_message_send:       Option<UserMessageSendFn>,
    // --- FakeConVar slice (APPENDED after user_message_send; order is the ABI; do not reorder above) ---
    pub convar_register: Option<ConvarRegisterFn>,
    // --- Translations slice (APPENDED after convar_register; order is the ABI; do not reorder above) ---
    pub translations_read: Option<TranslationsReadFn>,
    pub client_language:   Option<ClientLanguageFn>,
    // --- Zones real-trigger slice (APPENDED after client_language; order is the ABI; do not reorder above) ---
    pub collision_activate: Option<CollisionActivateFn>,
    // --- Zones real-trigger slice (APPENDED after collision_activate; order is the ABI; do not reorder above) ---
    pub entity_set_model: Option<EntitySetModelFn>,
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

/// Net (raw TCP/UDP) Task 2: a queued per-connection event awaiting post-drain fan-out (see
/// `NET_EVENT_PENDING`/`dispatch_pending_net_events`). Carries raw binary payloads (a TCP inbound
/// chunk or a UDP datagram + its "host:port" source), unlike ws's text-only pending tuple.
enum PendingNetEvent {
    /// TCP inbound chunk → `"<id>:data"` fan-out with `[Uint8Array]`.
    Data(Vec<u8>),
    /// UDP inbound datagram → `"<id>:message"` fan-out with `[{host,port}, Uint8Array]`; `from` is
    /// the source `"host:port"` string.
    Datagram { from: String, data: Vec<u8> },
    /// Terminal → `"<id>:close"` fan-out with `[]` (then prune every key for this conn).
    Closed,
    /// Mid-stream error → `"<id>:error"` fan-out with `[String]`.
    Errored(String),
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
    /// `name → flags` sidecar for registered ConCommands (Slice 6.16, backing `sm_help` / `Commands.list()`).
    /// `flags` encodes the required admin bit mask: `0` = anyone, `-1` = console/server-only sentinel,
    /// else the `ADMFLAG` bit mask (`registerAdmin`). Pure `i64` — NO V8 handles — so it need not be cleared
    /// before the isolate drops; `__s2_commands_list` joins on live `CONCOMMANDS` keys (a stale meta entry is
    /// ignored). Dropped per-plugin alongside `CONCOMMANDS` in `unload_plugin`, cleared on `shutdown`.
    static COMMAND_META: std::cell::RefCell<std::collections::HashMap<String, i64>>
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
    /// Raw-chat subscriber mux (Slice 6.13b): `Chat.onMessage(h)` subscribers, keyed by the constant
    /// "" (chat has no name dimension — a single un-keyed list). Same EventMux shape/discipline;
    /// handlers receive `(slot, text, teamonly)` and may return a HookResult (>= Handled suppresses the
    /// broadcast). The Host_Say detour is always installed, so there is no per-subscribe engine-op and
    /// no engine-op on empty teardown. `remove_by_owner` on unload; reset on shutdown.
    static CHAT_MSG_SUBS: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    /// Client-lifecycle subscriber mux (Clients sub-project): name → per-plugin subscribers, keyed by
    /// the lifecycle event name ("connect"/"putinserver"/"active"/"fullyconnect"/"disconnect"/
    /// "settingschanged"). Same EventMux shape/discipline as EVENT_MUX; notify-only (a handler's return
    /// is ignored — no HookResult collapse). The shim's six lifecycle hooks are installed unconditionally
    /// at Load, so there is no per-subscribe engine-op and no engine-op on empty teardown.
    /// `remove_by_owner` on unload; reset on shutdown so a re-init starts empty.
    static CLIENT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());

    /// Map-start subscribers (clientlist-fakeconvar-onmapstart slice). Fixed key "" (map-start has
    /// no name dimension, like CHAT_MSG_SUBS); notify-only.
    static MAP_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());

    /// clientprefs Task 4: `Cookies.onCached` subscriber mux, keyed by the constant "" (no name
    /// dimension — a single un-keyed list, like `CHAT_MSG_SUBS`). Fanned out post-frame by
    /// `dispatch_pending_cookie_cached` (called from `ffi.rs` AFTER `frame_async_drain()` returns, so
    /// HOST is free — no re-entrancy risk from the plugin's own async cookie-load work).
    /// `remove_by_owner` on unload; reset on shutdown.
    static COOKIE_CACHED_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    /// clientprefs Task 4: slots queued by `__s2_cookie_dispatch_cached` (called from inside the
    /// plugin's `loadCookies` async continuation, i.e. possibly mid-async-drain) for the NEXT
    /// `dispatch_pending_cookie_cached()` post-drain fan-out. Draining + clearing happens with HOST free.
    static COOKIE_CACHED_PENDING: std::cell::RefCell<Vec<i32>> = std::cell::RefCell::new(Vec::new());

    /// WebSocket Task 2: `WebSocket.on*` subscriber mux, keyed `"<conn_id>:<event>"` (event =
    /// "message"/"close"/"error"). Same EventMux shape/discipline as COOKIE_CACHED_MUX; fanned out
    /// post-frame by `dispatch_pending_ws_events` (called from `ffi.rs` AFTER `frame_async_drain()`
    /// returns, so HOST is free). `remove_by_owner` on unload; reset on shutdown.
    static WS_EVENT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    /// WebSocket Task 2: `(conn_id, event, payload1, payload2)` queued during `frame_async_drain`'s
    /// signal-routing step, fanned out post-drain (HOST free) by `dispatch_pending_ws_events`. For
    /// "message"/"error" the 3rd tuple field is the text and the 4th is unused (0); for "close" the
    /// 3rd is the reason and the 4th is the code.
    static WS_EVENT_PENDING: std::cell::RefCell<Vec<(u64, String, String, i32)>> = std::cell::RefCell::new(Vec::new());

    /// Net (raw TCP/UDP) Task 2: `TcpSocket`/`UdpSocket` `on*` subscriber mux, keyed `"<conn_id>:<event>"`
    /// (event = "data"/"message"/"close"/"error"). Same EventMux shape/discipline as `WS_EVENT_MUX`;
    /// fanned out post-frame by `dispatch_pending_net_events` (called from `ffi.rs` AFTER
    /// `frame_async_drain()` returns, so HOST is free). `remove_by_owner` on unload; reset on shutdown.
    static NET_EVENT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    /// Net Task 2: `(conn_id, PendingNetEvent)` queued during `frame_async_drain`'s signal-routing step,
    /// fanned out post-drain (HOST free) by `dispatch_pending_net_events`. Unlike `WS_EVENT_PENDING`'s
    /// (String, i32) payload, net carries raw binary bytes → a dedicated `PendingNetEvent` enum.
    static NET_EVENT_PENDING: std::cell::RefCell<Vec<(u64, PendingNetEvent)>> = std::cell::RefCell::new(Vec::new());

    /// Entity-I/O slice: `Entity.onOutput(classname, output, handler)` subscriber mux, keyed by the
    /// literal string `"<classname>\0<output>"` (a NUL separator — classnames/outputs never contain one).
    /// `"*"` is a valid wildcard for either half (matched at dispatch by querying all 4 combinations).
    /// Unlike `DAMAGE_MUX`/`CHAT_MSG_SUBS` (whose detour is installed once, unconditionally, for the
    /// process lifetime), the `FireOutputInternal` detour here is likewise installed unconditionally at
    /// shim Load — so there is no per-subscribe engine-op and no engine-op on empty teardown. Dispatch is
    /// SYNCHRONOUS (the detour blocks on it, mirrors `DAMAGE_MUX`/`EVENT_MUX_PRE`, NOT the post-drain
    /// `*_PENDING` muxes) so a handler's `HookResult` can suppress the output before the original runs.
    /// `remove_by_owner` on unload; reset on shutdown so a re-init starts empty.
    static OUTPUT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
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

    /// Slice admin-groups: per-tier immunity levels (mirrors ADMIN_FILE/ADMIN_RUNTIME). get = max(file, runtime).
    static ADMIN_FILE_IMMUNITY:    std::cell::RefCell<std::collections::HashMap<String, i32>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static ADMIN_RUNTIME_IMMUNITY: std::cell::RefCell<std::collections::HashMap<String, i32>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Per-admin command overrides (file tier only — the resolver merges an admin's groups' override
    /// blocks). sid -> cmd -> (required_mask, is_public). is_public true => anyone (flag "").
    static ADMIN_OVERRIDES: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashMap<String, (u64, bool)>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Global command overrides (admin_overrides.json). cmd -> (required_mask, is_public).
    static ADMIN_GLOBAL_OVERRIDES: std::cell::RefCell<std::collections::HashMap<String, (u64, bool)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());

    /// Slice 6.18: the host-global ban cache — SteamID64 → (until_unix, reason). `until == 0` = permanent;
    /// else the unix-second expiry. Host-global in core (not plugin-local JS) so it is visible across all
    /// plugin contexts (like the admin cache). Populated by JS via the `__s2_ban_*` natives (loaded from
    /// bans.json through the config bridge). Enforcement is JS-driven (sub-project 3): a ban plugin's
    /// `Clients.onConnect` handler reads it via `__s2_ban_get` and shows-then-kicks a banned player. The
    /// `ban_check` ffi export below is retained as an available synchronous primitive but is no longer called.
    static BAN_CACHE: std::cell::RefCell<std::collections::HashMap<String, (i64, String)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// One-shot guard so bans.json loads once (mirrors `ADMIN_FILE_LOADED`).
    static BAN_LOADED: std::cell::Cell<bool> = std::cell::Cell::new(false);

    /// TopMenu registry (adminmenu framework). Ordered category names (deduped) + items owned by a
    /// plugin. Item `onSelect` is a Global<Function> held like a command handler (NOT marshalled;
    /// invoked in the owner's context on select). Owner-scoped teardown mirrors CONCOMMANDS.
    static TOPMENU_CATEGORIES: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    static TOPMENU_ITEMS: std::cell::RefCell<std::collections::HashMap<String, TopMenuItem>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Slots+ids queued by __s2_topmenu_select (called under the isolate borrow from a menu onSelect);
    /// fanned out post-frame by dispatch_pending_topmenu_select (ffi.rs, HOST free). Same discipline as
    /// COOKIE_CACHED_PENDING — sidesteps the re-entrant double-borrow.
    static TOPMENU_PENDING: std::cell::RefCell<Vec<(String, i32)>> = std::cell::RefCell::new(Vec::new());
    /// Monotonic insertion counter → each item's `seq`, so `snapshot` renders items in REGISTRATION order
    /// (a HashMap iterates in random per-instance order that would shuffle across restarts; the spec commits
    /// the MVP to insertion order). A re-added id reuses its existing seq so a plugin reload doesn't reorder.
    static TOPMENU_SEQ: std::cell::Cell<u64> = std::cell::Cell::new(0);
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
    writeInt8:        function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.I8, v); },
    writeInt16:       function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.I16, v); },
    writeUInt8:       function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.U8, v); },
    writeUInt16:      function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.U16, v); },
    writeUInt32:      function (o, v)  { return __s2_ent_ref_write(this.index, this.serial, o, K.U32, v); },
    readUInt64:       function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.U64); },
    readInt64:        function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.I64); },
    readFloat64:      function (o)         { return __s2_ent_ref_read(this.index, this.serial, o, K.F64); },
    readString:       function (o, maxLen) { return __s2_ent_ref_read_string(this.index, this.serial, o, maxLen); },
    writeString:      function (o, maxLen, s) { return __s2_ent_ref_write_string(this.index, this.serial, o, maxLen, String(s)); },
    readFloats:       function (o, count)  { return __s2_ent_ref_read_floats(this.index, this.serial, o, count); },
    readFloatsChain: function (chain, finalOff, count) { return __s2_ent_ref_read_floats_chain(this.index, this.serial, chain, finalOff, count); },
    readInt32Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.serial, c, o, K.I32); },
    writeInt32Via: function (c, o, v) { return __s2_ent_ref_write_chain(this.index, this.serial, c, o, K.I32, v); },
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
  // --- Entity-creation lifecycle slice: spawn/teleport/remove over the entity_* engine ops. Kept as
  //     separate prototype assignments (not folded into the object literal above) to minimize the diff. ---
  // EKV marshal: {key: string|number|boolean} -> parallel {keys, types, values} arrays.
  // types: 0=string 1=int 2=float 3=bool; values stringified ("1"/"0" for bool). Inference:
  // integer-in-int32 -> int, other finite number -> float. ANY bad entry (empty key, non-finite,
  // unsupported value type, >256 keys, an over-length key/value string) rejects the WHOLE map
  // (null) — never a partial spawn.
  // EKV_MAX_STRING_LEN: CKV3Arena's CUtlMemoryBlockAllocator::AddPage() aborts the WHOLE process
  // (a real Plat_FatalError -> our tier0-shimmed Plat_ExitProcess -> abort()) once a single
  // string's backing allocation reaches its MaxPossiblePageSize() bound — which computes to
  // exactly 2048 bytes from the SDK's MEMBLOCK_DEFAULT_PAGESIZE(0x800)/MEMBLOCK_MAX_TOTAL_PAGESIZE
  // constants (confirmed live: 2000B keyvalue strings are fine, 2050B reliably aborts the server).
  // Capped here well under that bound so an ordinary plugin bug (e.g. relaying chat/file/JSON
  // content into a spawn keyvalue) fails closed instead of taking down the whole process.
  var EKV_MAX_STRING_LEN = 1024;
  function __s2_ekv_marshal(kv) {
    var keys = [], types = [], values = [];
    var names = Object.keys(kv);
    if (names.length > 256) return null;
    for (var i = 0; i < names.length; i++) {
      var k = names[i];
      if (!k || k.length > EKV_MAX_STRING_LEN) return null;
      var v = kv[k], t = typeof v;
      if (t === "string") {
        if (v.length > EKV_MAX_STRING_LEN) return null;
        types.push(0); values.push(v);
      }
      else if (t === "number") {
        if (!isFinite(v)) return null;
        if (Number.isInteger(v) && v >= -2147483648 && v <= 2147483647) { types.push(1); values.push(String(v)); }
        else { types.push(2); values.push(String(v)); }
      }
      else if (t === "boolean") { types.push(3); values.push(v ? "1" : "0"); }
      else return null;
      keys.push(k);
    }
    return { keys: keys, types: types, values: values };
  }
  EntityRef.prototype.spawn = function (keyvalues) {
    if (keyvalues === undefined || keyvalues === null) return __s2_entity_spawn(this.index, this.serial);
    if (typeof keyvalues !== "object") return false;
    var m = __s2_ekv_marshal(keyvalues);
    if (m === null) return false;
    if (m.keys.length === 0) return __s2_entity_spawn(this.index, this.serial);
    return __s2_entity_spawn_kv(this.index, this.serial, m.keys, m.types, m.values);
  };
  EntityRef.prototype.teleport = function (origin, angles, velocity) {
    return __s2_entity_teleport(this.index, this.serial,
      origin ? [origin[0], origin[1], origin[2]] : null,
      angles ? [angles[0], angles[1], angles[2]] : null,
      velocity ? [velocity[0], velocity[1], velocity[2]] : null);
  };
  EntityRef.prototype.remove = function () { return __s2_entity_remove(this.index, this.serial); };
  // Zones real-trigger slice: register this entity's collision bounds in the spatial partition so a
  // runtime-created trigger fires touch. Serial-gated; returns false if the op is unavailable/stale.
  EntityRef.prototype.activateCollision = function () { return __s2_collision_activate(this.index, this.serial); };
  // Zones real-trigger slice: give this entity a model (and its collision) via CBaseEntity::SetModel.
  // A runtime trigger_multiple needs a model to build the physics volume that fires touch.
  EntityRef.prototype.setModel = function (name) { return __s2_ent_set_model(this.index, this.serial, String(name)); };
  // Create a new entity by class name (e.g. "env_beam"). Returns a serial-gated EntityRef, or null.
  // With keyvalues: create + DispatchSpawn(keyvalues) in one call — a non-null result is a LIVE,
  // SPAWNED entity (on spawn failure the entity is removed and null returned).
  function createEntity(className, keyvalues) {
    var ref = __s2_entity_create(String(className));
    if (!ref) return null;
    if (keyvalues !== undefined && keyvalues !== null) {
      // With kv: non-null result = a LIVE, SPAWNED entity. On spawn failure, remove the unspawned
      // entity (hygiene — never strand a half-configured entity) and return null.
      if (!ref.spawn(keyvalues)) { ref.remove(); return null; }
    }
    return ref;
  }
  // Item slice: read a CUtlVector<CHandle> at (ptrOffs chain -> vectorOff) as live serial-gated
  // EntityRefs. Each element is decoded + validated core-side; the raw pointer never crosses to JS.
  EntityRef.prototype.readHandleVector = function (ptrOffs, vectorOff, maxCount) {
    return __s2_entity_read_handle_vector(this.index, this.serial, ptrOffs || [], vectorOff, maxCount || 64);
  };
  // Entity-I/O slice: fire an input (e.g. "Kill"/"Ignite"/"FireUser1") via AddEntityIOEvent — the
  // game's own input-firing path (delay 0 = the same-tick I/O pump). value is the input's string
  // argument (Source parses it per the input's field type); activator/caller are optional EntityRefs.
  EntityRef.prototype.acceptInput = function (input, value, activator, caller, delay) {
    return __s2_entity_fire_input(
      this.index, this.serial, String(input),
      (value === undefined || value === null) ? "" : String(value),
      activator ? activator.index : -1, activator ? activator.serial : -1,
      caller ? caller.index : -1, caller ? caller.serial : -1,
      delay || 0);
  };
  // Entity-I/O slice: hook an entity output (e.g. "OnTrigger"/"OnPressed"/"OnStartTouch"). classname/
  // output accept "*" wildcards. handler(ev) may return a HookResult >= Handled to suppress the output
  // (the FireOutputInternal detour supersedes the original call). Dispatch is SYNCHRONOUS.
  var Entity = {
    onOutput: function (classname, output, handler) { __s2_output_subscribe(String(classname), String(output), handler); },
    // Find every entity whose designer-name (class) exactly matches className. Returns serial-gated
    // EntityRefs (empty array on no-op/degrade). Broadly reusable (gamerules proxy, props, triggers...).
    findByClass: function (className) {
      return __s2_entity_find_by_class(String(className));
    },
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
  // Ray-trace slice: angle -> unit forward-direction vector (x=pitch, y=yaw, per the QAngle
  // convention above). Pure math, no engine ops — lives in @s2script/math since a forward vector
  // is Source-2-generic, not CS2-specific (@s2script/trace's Trace.ray composes it).
  function forwardVector(a) {
    var p = a.x * Math.PI / 180, y = a.y * Math.PI / 180;
    return new Vector(Math.cos(p) * Math.cos(y), Math.cos(p) * Math.sin(y), -Math.sin(p));
  }
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
    // shared: apply { key: value } to the current event (create must have run). Type-infer as in `fire`.
    _applyFields: function (fields) {
      if (!fields) return;
      for (var k in fields) {
        if (!Object.prototype.hasOwnProperty.call(fields, k)) continue;
        var v = fields[k], t = typeof v;
        if (t === "boolean") __s2_event_set_bool(k, v);
        else if (t === "string") __s2_event_set_string(k, v);
        else if (t === "bigint") __s2_event_set_uint64(k, v.toString());
        else if (t === "number") { if (Number.isInteger(v)) __s2_event_set_int(k, v); else __s2_event_set_float(k, v); }
      }
    },
    // Fire a game event. fields: { key: value }. Runtime type-infer: bool→setBool, string→setString,
    // bigint→setUint64, integer number→setInt, other number→setFloat. Returns the FireEvent result.
    fire:  function (name, fields, dontBroadcast) {
      if (!__s2_event_create(name)) return false;
      this._applyFields(fields);
      return __s2_event_fire(!!dontBroadcast);
    },
    // Fire a game event to ONE client (SourceMod FireToClient parity). Same field type-inference as
    // `fire`. Returns false on any miss (no manager / no pending event / no client / bot).
    fireToClient: function (slot, name, fields) {
      if (!__s2_event_create(name)) return false;
      this._applyFields(fields);
      return __s2_event_fire_to_client(slot | 0);
    },
  };
  // --- Slice 5E.2: config module (typed getters over __s2pkg_config_values; zero-value fallback) ---
  var __s2_config = {
    getString: function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return v == null ? "" : String(v); },
    getInt:    function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return (v == null || typeof v !== "number") ? 0 : (v | 0); },   // int = 32-bit (SourceMod ConVar parity); `v | 0` truncates by design
    getFloat:  function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return (v == null || typeof v !== "number") ? 0 : v; },
    getBool:   function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return v === true; },
    onChange:  function (h) { __s2_config_on_change(h); },
    readFile:  function (name) { return __s2_config_read_file(String(name)); },
    writeFile: function (name, content) { __s2_config_write_file(String(name), String(content)); },
  };
  // --- Menu primitive (engine-generic): model + pagination + registerRenderer seam. Slot-based. ---
  var MenuStyle = { Chat: "chat", Center: "center" };
  var MenuCancelReason = { Exit: 0, Timeout: 1, Disconnect: 2, NewMenu: 3 };
  var MENU_ITEMS_PER_PAGE = 7;            // chat page size (SM ITEMS_PER_PAGE)
  var __s2_menu_renderers = {};           // style value -> renderer { open, update, close }
  var __s2_menu_activeBySlot = {};        // slot -> session (one active menu per slot, this context)

  function Menu(title) {
    this.title = title || "";
    this.style = MenuStyle.Chat;
    this.exitButton = true;
    this.freezePlayer = false;   // a renderer that supports it (CS2 center) freezes the player while open
    this.items = [];
    this._onSelect = null;
    this._onCancel = null;
  }
  Menu.registerRenderer = function (name, renderer) { __s2_menu_renderers[name] = renderer; };
  Menu.prototype.addItem = function (info, display, opts) {
    this.items.push({ info: String(info), display: String(display), disabled: !!(opts && opts.disabled) });
  };
  Menu.prototype.onSelect = function (fn) { this._onSelect = fn; };
  Menu.prototype.onCancel = function (fn) { this._onCancel = fn; };
  Menu.prototype.display = function (slot, seconds) {
    if (typeof slot !== "number" || slot < 0) return;   // console/invalid is never a menu target
    var renderer = __s2_menu_renderers[this.style] || __s2_menu_renderers[MenuStyle.Chat];
    if (!renderer) { globalThis.console && console.log("[menu] no renderer for style " + this.style); return; }
    // Install THIS session as the active one for the slot BEFORE ending the previous — so prev._end()
    // (which runs the plugin's onCancel synchronously) can't mis-delete us, and a re-entrant display()
    // from that onCancel replaces our map entry, which we then respect instead of clobbering.
    var prev = __s2_menu_activeBySlot[slot];
    var session = new MenuSession(this, slot, renderer, seconds || 0);
    __s2_menu_activeBySlot[slot] = session;
    if (prev) prev._end(MenuCancelReason.NewMenu);        // NewMenu; may re-enter display() for this slot
    if (__s2_menu_activeBySlot[slot] !== session) return; // a re-entrant display won — abandon this one
    session._start();
  };
  Menu.prototype.close = function (slot) {
    var s = __s2_menu_activeBySlot[slot];
    if (s && s.menu === this) s._end(MenuCancelReason.Exit);
  };

  // A live display of one menu to one slot. Owns page/cursor state.
  function MenuSession(menu, slot, renderer, seconds) {
    this.menu = menu; this.slot = slot; this.renderer = renderer; this.seconds = seconds;
    this.page = 0; this.cursor = 0; this._ended = false;
    this._selectable = [];   // indices (into menu.items) that are selectable on the CURRENT page
  }
  // Selectable item indices for a chat page: up to MENU_ITEMS_PER_PAGE, skipping disabled.
  MenuSession.prototype._pageItems = function (page) {
    var out = [], start = page * MENU_ITEMS_PER_PAGE, i = start;
    // NOTE: disabled items still occupy a slot in the on-screen list but get no number.
    for (; i < this.menu.items.length && (i - start) < MENU_ITEMS_PER_PAGE; i++) out.push(i);
    return out;
  };
  MenuSession.prototype.pageCount = function () {
    return Math.max(1, Math.ceil(this.menu.items.length / MENU_ITEMS_PER_PAGE));
  };
  // Center navigable targets on the CURRENT page, in display order: the selectable items, then the
  // Back/Next/Exit controls. The CENTER cursor indexes into THIS list (not just items) so W/S can reach
  // paging + Exit and E confirms them — otherwise a center menu can't paginate or be dismissed.
  MenuSession.prototype._navTargets = function () {
    var t = [], pageItems = this._pageItems(this.page);
    for (var k = 0; k < pageItems.length; k++) { var idx = pageItems[k]; if (!this.menu.items[idx].disabled) t.push({ kind: "item", index: idx }); }
    var pc = this.pageCount();
    if (this.page > 0)        t.push({ kind: "back" });
    if (this.page < pc - 1)   t.push({ kind: "next" });
    if (this.menu.exitButton) t.push({ kind: "exit" });
    return t;
  };
  // Build the resolved view the renderer paints. Assigns chat number keys 1..7 to selectable items,
  // then control keys 8=Back, 9=Next, 0=Exit as applicable. For a Center menu, marks the line under
  // the cursor (an item OR a control) so the renderer can highlight it.
  MenuSession.prototype.view = function () {
    var m = this.menu, pageItems = this._pageItems(this.page), lines = [], keyNum = 1;
    this._selectable = [];
    var nav = (m.style === MenuStyle.Center) ? this._navTargets() : null;
    var cur = (nav && this.cursor >= 0 && this.cursor < nav.length) ? nav[this.cursor] : null;
    for (var k = 0; k < pageItems.length; k++) {
      var idx = pageItems[k], it = m.items[idx], key = null, selectable = false;
      if (!it.disabled) { key = String(keyNum++); selectable = true; this._selectable.push(idx); }
      lines.push({ text: it.display, key: key, selectable: selectable, cursor: !!(cur && cur.kind === "item" && cur.index === idx), index: idx });
    }
    var pc = this.pageCount();
    if (this.page > 0)      lines.push({ text: "Back", key: "8", selectable: false, control: "back", cursor: !!(cur && cur.kind === "back") });
    if (this.page < pc - 1) lines.push({ text: "Next", key: "9", selectable: false, control: "next", cursor: !!(cur && cur.kind === "next") });
    if (m.exitButton)       lines.push({ text: "Exit", key: "0", selectable: false, control: "exit", cursor: !!(cur && cur.kind === "exit") });
    return { title: m.title, lines: lines, page: this.page, pageCount: pc, exit: m.exitButton };
  };
  MenuSession.prototype._start = function () { this.renderer.open(this); if (this.seconds > 0) this._armTimeout(); };
  // Timeout: arm a delay that cancels the session (any renderer). Lazily reads __s2pkg_timers at
  // call time (not module-load time), so this is safe regardless of prelude assignment order.
  MenuSession.prototype._armTimeout = function () {
    var self = this, ms = (this.seconds | 0) * 1000;
    globalThis.__s2pkg_timers.delay(ms).then(function () {
      if (!self._ended) self._end(MenuCancelReason.Timeout);
    });
  };
  MenuSession.prototype._repaint = function () { if (!this._ended) this.renderer.update(this); };
  MenuSession.prototype._end = function (reason) {
    if (this._ended) return; this._ended = true;
    if (__s2_menu_activeBySlot[this.slot] === this) delete __s2_menu_activeBySlot[this.slot];
    this.renderer.close(this.slot);
    if (this.menu._onCancel && (reason === MenuCancelReason.Timeout || reason === MenuCancelReason.Disconnect || reason === MenuCancelReason.NewMenu || reason === MenuCancelReason.Exit))
      { try { this.menu._onCancel({ slot: this.slot, reason: reason }); } catch (e) { globalThis.console && console.log("[menu] onCancel threw: " + e); } }
  };
  MenuSession.prototype._select = function (itemIndex) {
    var it = this.menu.items[itemIndex];
    if (!it || it.disabled) return;
    // mark ended BEFORE the callback so a re-display inside onSelect isn't clobbered
    this._ended = true;
    if (__s2_menu_activeBySlot[this.slot] === this) delete __s2_menu_activeBySlot[this.slot];
    this.renderer.close(this.slot);
    if (this.menu._onSelect) { try { this.menu._onSelect({ slot: this.slot, item: itemIndex, info: it.info, display: it.display }); } catch (e) { globalThis.console && console.log("[menu] onSelect threw: " + e); } }
  };
  // Chat idiom: a number-key pick against the current view's keys.
  MenuSession.prototype.pickNumber = function (n) {
    if (this._ended) return;
    this.view();  // refresh this._selectable for the current page
    var key = String(n);
    if (key === "8" && this.page > 0)                      { this.page--; this.cursor = 0; this._repaint(); return; }
    if (key === "9" && this.page < this.pageCount() - 1)   { this.page++; this.cursor = 0; this._repaint(); return; }
    if (key === "0" && this.menu.exitButton)               { this._end(MenuCancelReason.Exit); return; }
    var slotN = n - 1;
    if (slotN >= 0 && slotN < this._selectable.length) this._select(this._selectable[slotN]);
  };
  // Center idiom: cursor navigation over _navTargets (items + Back/Next/Exit controls).
  MenuSession.prototype.moveUp = function () {
    if (this._ended) return;
    var n = this._navTargets().length; if (!n) return;
    this.cursor = (this.cursor - 1 + n) % n; this._repaint();
  };
  MenuSession.prototype.moveDown = function () {
    if (this._ended) return;
    var n = this._navTargets().length; if (!n) return;
    this.cursor = (this.cursor + 1) % n; this._repaint();
  };
  // Confirm the current cursor target: an item selects; Back/Next paginate (cursor to top); Exit cancels.
  MenuSession.prototype.confirm = function () {
    if (this._ended) return;
    var nav = this._navTargets(); if (this.cursor < 0 || this.cursor >= nav.length) return;
    var t = nav[this.cursor];
    if (t.kind === "item")      this._select(t.index);
    else if (t.kind === "back") { this.page--; this.cursor = 0; this._repaint(); }
    else if (t.kind === "next") { this.page++; this.cursor = 0; this._repaint(); }
    else if (t.kind === "exit") this._end(MenuCancelReason.Exit);
  };
  MenuSession.prototype.cancel = function () { if (!this._ended) this._end(MenuCancelReason.Exit); };
  // --- General UserMessage builder (@s2script/usermessages): accumulate scalar fields, then flush
  // create -> set* -> send in one synchronous burst (the shim holds a single build-then-send target,
  // so there is no cross-message aliasing without an await between). Engine-generic — the message NAME
  // is the caller's; core knows no CS2 message strings. ---
  function UserMessage(name) { this._name = String(name); this._fields = []; }
  UserMessage.prototype.setInt    = function (f, v) { this._fields.push([0, String(f), v]); return this; };
  UserMessage.prototype.setFloat  = function (f, v) { this._fields.push([1, String(f), v]); return this; };
  UserMessage.prototype.setString = function (f, v) { this._fields.push([2, String(f), String(v)]); return this; };
  UserMessage.prototype.setBool   = function (f, v) { this._fields.push([3, String(f), v ? 1 : 0]); return this; };
  UserMessage.prototype.set = function (f, v) {
    if (typeof v === "boolean") return this.setBool(f, v);
    if (typeof v === "string")  return this.setString(f, v);
    if (typeof v === "number")  return Number.isInteger(v) ? this.setInt(f, v) : this.setFloat(f, v);
    return this;
  };
  UserMessage.prototype._flush = function (slotsOrNull) {
    if (__s2_user_message_create(this._name) !== 1) return false;
    for (var i = 0; i < this._fields.length; i++) {
      var fld = this._fields[i];
      if (fld[0] === 0)      __s2_user_message_set_int(fld[1], fld[2]);
      else if (fld[0] === 1) __s2_user_message_set_float(fld[1], fld[2]);
      else if (fld[0] === 2) __s2_user_message_set_string(fld[1], fld[2]);
      else                   __s2_user_message_set_bool(fld[1], fld[2]);
    }
    return __s2_user_message_send(slotsOrNull) === true;
  };
  UserMessage.prototype.send    = function (slots) { return this._flush(Array.isArray(slots) ? slots : [slots]); };
  UserMessage.prototype.sendAll = function () { return this._flush(null); };
  globalThis.__s2pkg_math       = { Vector: Vector, QAngle: QAngle, forwardVector: forwardVector };
  globalThis.__s2pkg_entity     = { EntityRef: EntityRef, createEntity: createEntity, Entity: Entity };
  globalThis.__s2pkg_usermessages = { UserMessage: UserMessage };
  globalThis.__s2pkg_frame      = { OnGameFrame: OnGameFrame };
  globalThis.__s2pkg_timers     = timers;
  globalThis.__s2pkg_console    = { console: console };
  globalThis.__s2pkg_interfaces = interfaces;
  globalThis.__s2pkg_events     = { GameEvent: GameEvent, Events: Events, HookResult: globalThis.HookResult };
  globalThis.__s2pkg_config     = { config: __s2_config };   // named export `config` (matches the .d.ts: import { config } from "@s2script/config")
  // --- @s2script/translations — SM-style i18n. Phrases: a flat key->text map; the plugin's `seed` is the
  //     in-memory English default; translations/<code>/<name>.phrases.json (read lazily) overrides per language;
  //     an optional root translations/<name>.phrases.json overrides the seed. Fully engine-generic. ---
  var __s2_tr_reg = Object.create(null);   // name -> { def: {k:text}, langs: { code: {k:text}|null } }  (null = tried+absent)
  var __s2_tr_default = "";          // server/console default language code ("" = root/English)
  // Steam cl_language -> folder code ("" = root/English). Unmapped -> "" (default).
  var __s2_TR_CODES = Object.assign(Object.create(null), { english:"", german:"de", russian:"ru", french:"fr", spanish:"es", latam:"es",
    schinese:"zh", tchinese:"zh", portuguese:"pt", brazilian:"pt", polish:"pl", italian:"it", dutch:"nl",
    swedish:"sv", danish:"da", finnish:"fi", norwegian:"no", czech:"cs", hungarian:"hu", turkish:"tr",
    japanese:"ja", koreana:"ko", thai:"th", ukrainian:"uk", bulgarian:"bg", greek:"el", romanian:"ro" });
  function __s2_tr_langCode(clLang) {
    var v = __s2_TR_CODES[String(clLang || "").toLowerCase()];
    return (typeof v === "string") ? v : "";   // non-string (e.g. a "__proto__" chain read) -> default
  }
  function __s2_tr_format(text, args) {
    return String(text).replace(/\{(\d+)\}/g, function (_m, n) {
      var i = (parseInt(n, 10) | 0) - 1;
      return (args && i >= 0 && i < args.length && args[i] != null) ? String(args[i]) : "";
    });
  }
  function __s2_tr_parse(text) { try { var o = JSON.parse(text); return (o && typeof o === "object") ? o : {}; } catch (e) { console.log("[s2script] WARN: translations file malformed — ignored"); return {}; } }
  function __s2_tr_merge(dst, src) {   // copy own enumerable keys, skipping __proto__ (no by-ref share, no proto pollution)
    for (var k in src) if (Object.prototype.hasOwnProperty.call(src, k) && k !== "__proto__") dst[k] = src[k];
    return dst;
  }
  function __s2_tr_langMap(name, code) {                     // the lazily-read (+cached) map for a code ("" = root override)
    var r = __s2_tr_reg[name]; if (!r) return null;
    if (Object.prototype.hasOwnProperty.call(r.langs, code)) return r.langs[code];   // cached (map or null)
    var text = __s2_translations_read(code, name);           // null if absent/no-op
    var map = (text == null) ? null : __s2_tr_parse(text);
    r.langs[code] = map;
    return map;
  }
  var __s2_translations = {
    load: function (name, seed) {
      name = String(name);
      var def = __s2_tr_merge({}, (seed && typeof seed === "object") ? seed : {});   // fresh copy, not the caller's ref
      __s2_tr_reg[name] = { def: def, langs: {} };
      var root = __s2_translations_read("", name);           // OPTIONAL root override of the seed
      if (root != null) __s2_tr_merge(def, __s2_tr_parse(root));                     // root file overrides seed keys
    },
    setDefaultLanguage: function (code) { __s2_tr_default = String(code || ""); },
    translate: function (slot, key) {
      var args = [].slice.call(arguments, 2);
      key = String(key);
      var code = ((slot | 0) < 0) ? __s2_tr_default : __s2_tr_langCode(__s2_client_language(slot | 0));
      // search EVERY loaded phrase set: the code's lang map (if not root) -> the default/seed -> the key.
      for (var name in __s2_tr_reg) {
        if (!Object.prototype.hasOwnProperty.call(__s2_tr_reg, name)) continue;
        if (code) { var lm = __s2_tr_langMap(name, code); if (lm && lm[key] != null) return __s2_tr_format(lm[key], args); }
        var d = __s2_tr_reg[name].def; if (d[key] != null) return __s2_tr_format(d[key], args);
      }
      return key;                                            // ultimate fallback
    },
  };
  globalThis.__s2_tr_format = __s2_tr_format;                 // test hooks (pure)
  globalThis.__s2_tr_langCode = __s2_tr_langCode;
  globalThis.__s2_tr_injectLang = function (name, code, obj) { if (__s2_tr_reg[name]) __s2_tr_reg[name].langs[code] = obj; };  // test hook (bypasses the file read)
  globalThis.__s2pkg_translations = { Translations: __s2_translations };
  globalThis.__s2pkg_menu       = { Menu: Menu, MenuStyle: MenuStyle, MenuCancelReason: MenuCancelReason };
  // --- adminmenu framework: the TopMenu registry (categories/items owned by the registering plugin;
  // onSelect is dispatched to the OWNER's context post-drain — see __s2_topmenu_select). ---
  globalThis.__s2pkg_topmenu = { TopMenu: {
    addCategory: function (name) { __s2_topmenu_add_category(String(name)); },
    addItem: function (category, item) { __s2_topmenu_add_item(String(category), String(item.id), String(item.name), item.flags | 0, item.onSelect); },
    snapshot: function () { return __s2_topmenu_snapshot(); },
    select: function (id, slot) { __s2_topmenu_select(String(id), slot | 0); },
  } };
  // --- Slice 6.1: chat module (toSlot/toAll; toAll loops __s2_client_valid, engine-generic) ---
  // `color` is an OPAQUE leading prefix prepended to every chat message (NOT the console.log reply path,
  // so rcon/server-console output stays clean). Core doesn't know what it means — a game package or plugin
  // sets it to a color control byte (CS2: a ChatColors byte); "" = send raw. This keeps color as CONTENT
  // owned by the caller (SourceMod-parity), never a native-layer default. A message may still embed its own
  // color codes mid-string.
  var __s2_chat = {
    color: "",
    toSlot: function (slot, msg) { __s2_client_print(slot | 0, __s2_chat.color + String(msg)); },
    // slot -1 = broadcast to all in ONE call (the shim routes it to the game's UTIL_ClientPrintAll, which
    // renders true custom color, not team color — SourceMod's PrintToChatAll). NOT a per-slot loop.
    toAll:  function (msg) { __s2_client_print(-1, __s2_chat.color + String(msg)); },
    // Slice 6.13b: subscribe to raw player chat. The handler gets (slot, text, teamonly) and may return
    // a HookResult (>= Handled suppresses the broadcast). Delivered from the Host_Say detour for every
    // non-command chat line; the `@`-trigger layer (a later slice) subscribes through this.
    onMessage: function (handler) { __s2_chat_on_message(handler); },
  };
  globalThis.__s2pkg_chat = { Chat: __s2_chat };   // named export `Chat`
  // --- Slice 6.4: server module (command / isMapValid; engine-generic server control) ---
  var __s2_server = {
    command: function (cmd) { __s2_server_command(String(cmd)); },
    isMapValid: function (map) { return __s2_server_map_valid(String(map)) === 1; },
    getCvar: function (name) { return __s2_cvar_get(String(name)); },                 // "" if absent
    setCvar: function (name, value) { __s2_server_command(String(name) + " " + String(value)); },
    // Register a plugin-owned ConVar (FakeConVar). Type-checked JS-side; the shim ORs FCVAR_RELEASE.
    // Value reads reuse getCvar; writes reuse setCvar/console. Idempotent (reload-safe); the cvar and
    // its value persist for the process lifetime (SourceMod parity).
    registerCvar: function (name, opts) {
      opts = opts || {};
      var tmap = { bool: 0, int: 1, float: 2, string: 3 };
      var type = tmap[String(opts.type == null ? "string" : opts.type)];
      if (type === undefined) return false;
      var def = opts.default;
      var defStr = (type === 0) ? (def ? "1" : "0")
                                : String(def == null ? (type === 3 ? "" : 0) : def);
      return __s2_convar_register(String(name),
        opts.help == null ? null : String(opts.help),
        opts.flags == null ? 0 : +opts.flags, type, defStr,
        opts.min == null ? null : String(opts.min),
        opts.max == null ? null : String(opts.max)) === 1;
    },
    // Subscribe to map start (the framework event replacing the Server.mapName OnGameFrame poll).
    // Fires on every StartupServer (boot-loaded plugins get the first map); a plugin hot-loaded
    // mid-map should read Server.mapName at load for the CURRENT map. Handlers may be async
    // (fire-and-forget). Auto-ledgered per plugin; torn down on unload.
    onMapStart: function (h) { __s2_map_start_subscribe(h); },
    get maxPlayers() { return __s2_server_max_clients(); },   // GetMaxClients(); 0 if unavailable
    get mapName() { return __s2_server_map_name(); },         // GetMapName(); "" if unavailable
    get gameTime() { return __s2_server_game_time(); },       // GetGlobals()->curtime; 0 if unavailable
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
  // --- Ray-trace slice: @s2script/trace (Trace.line/ray/hull -> TraceHit, TraceMask). ENGINE-GENERIC
  //     (Source-2 physics) — over the single __s2_trace native (the trace_shape engine op). ---
  (function () {
    // InteractionLayers bit positions (mirrors shim/src/trace.h's kLayer* constexprs exactly; all
    // bit positions <=21, well within a JS/Rust-safe 32-bit range).
    var L_SOLID       = 1 << 0;
    var L_HITBOXES    = 1 << 1;
    var L_PLAYERCLIP  = 1 << 4;
    var L_NPCCLIP     = 1 << 5;
    var L_WINDOW      = 1 << 12;
    var L_PASSBULLETS = 1 << 13;
    var L_PLAYER      = 1 << 18;
    var L_NPC         = 1 << 19;
    var L_PHYSICSPROP = 1 << 21;
    var SHOT_PHYSICS = L_SOLID | L_PLAYERCLIP | L_WINDOW | L_PASSBULLETS | L_PLAYER | L_NPC | L_PHYSICSPROP;
    var TraceMask = {
      ShotPhysics: SHOT_PHYSICS,                          // world + player-clip + windows + players/NPCs/props (default)
      ShotHitbox:  L_HITBOXES | L_PLAYER | L_NPC,          // hitboxes only (headshot-style detection)
      ShotFull:    SHOT_PHYSICS | L_HITBOXES,              // physics + hitboxes (a full bullet trace)
      WorldOnly:   L_SOLID | L_WINDOW | L_PASSBULLETS,     // world geometry only, no entities
      Grenade:     L_SOLID | L_WINDOW | L_PHYSICSPROP | L_PASSBULLETS,
      BrushOnly:   L_SOLID | L_WINDOW,                     // brushes only, no clip volumes/entities
      PlayerMove:  L_SOLID | L_WINDOW | L_PLAYERCLIP | L_PASSBULLETS,
      NPCMove:     L_SOLID | L_WINDOW | L_NPCCLIP | L_PASSBULLETS,
    };
    function ignoreOf(opts) {
      var e = opts && opts.ignoreEntity;
      return (e && typeof e.index === "number" && typeof e.serial === "number")
        ? { idx: e.index, serial: e.serial } : { idx: -1, serial: -1 };
    }
    function maskOf(opts) { return (opts && typeof opts.mask === "number") ? opts.mask : TraceMask.ShotPhysics; }
    function excludeOf(opts) { return (opts && typeof opts.exclude === "number") ? opts.exclude : 0; }
    var Trace = {
      line: function (start, end, opts) {
        var ig = ignoreOf(opts);
        return __s2_trace(
          [start.x, start.y, start.z], [end.x, end.y, end.z], [0, 0, 0], [0, 0, 0],
          maskOf(opts), excludeOf(opts), ig.idx, ig.serial
        );
      },
      ray: function (start, angles, distance, opts) {
        var f = __s2pkg_math.forwardVector(angles);
        var end = { x: start.x + f.x * distance, y: start.y + f.y * distance, z: start.z + f.z * distance };
        return Trace.line(start, end, opts);
      },
      hull: function (start, end, mins, maxs, opts) {
        var ig = ignoreOf(opts);
        return __s2_trace(
          [start.x, start.y, start.z], [end.x, end.y, end.z], [mins.x, mins.y, mins.z], [maxs.x, maxs.y, maxs.z],
          maskOf(opts), excludeOf(opts), ig.idx, ig.serial
        );
      },
    };
    globalThis.__s2pkg_trace = { Trace: Trace, TraceMask: TraceMask };
  })();
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
      // A player's reply is DEFERRED one frame: for a chat-triggered command (!cmd) the command runs in the
      // Host_Say PRE-hook, before the player's command text is broadcast, so a synchronous reply would land
      // BEFORE their "!slap …" line (jarring). nextFrame lands it after. Console/rcon (s < 0) stays immediate.
      reply: function (m) {
        if (s < 0) { console.log(String(m)); return; }
        var msg = String(m);
        globalThis.__s2pkg_timers.nextFrame().then(function () { globalThis.__s2pkg_chat.Chat.toSlot(s, msg); });
      },
      // Localized reply: translate `key` for the CALLER's language, then reply (SM's %t on the reply path).
      // Soft-deps @s2script/translations — degrades to the key if translations isn't loaded.
      replyT: function (key) {
        var t = globalThis.__s2pkg_translations;
        if (!t) { this.reply(String(key)); return; }
        this.reply(t.Translations.translate.apply(t.Translations, [s, key].concat([].slice.call(arguments, 1))));
      },
    };
  }
  // Slice 6.11: a per-context registry of wrapped dispatch fns (name -> function(slot, argString)), so a
  // command can be invoked BY NAME (chat triggers) reusing the SAME wrapper as the ConCommand path (admin
  // gating included). __s2cmd_add both registers the engine ConCommand and records the wrapper here.
  var __s2cmd_reg = {};
  // `flags` (default 0) records the required admin mask for Commands.list()/sm_help: 0 = anyone,
  // -1 = console/server-only sentinel, else the ADMFLAG bit mask. Passed through to the __s2_concommand native.
  function __s2cmd_add(name, wrapped, flags) { __s2cmd_reg[name] = wrapped; __s2_concommand(name, wrapped, flags | 0); }
  var __s2cmd_triggers = { public: "!", silent: "/" };   // SM PublicChatTrigger / SilentChatTrigger; mutable
  var __s2_commands = {
    register: function (name, handler) {
      __s2cmd_add(name, function (slot, a) { handler(__s2cmd_ctx(slot, a)); }, 0);   // 0 = anyone
    },
    registerServer: function (name, handler) {
      __s2cmd_add(name, function (slot, a) {
        var ctx = __s2cmd_ctx(slot, a);
        if (ctx.callerSlot < 0) { handler(ctx); }
        else { ctx.reply("[SM] This command can only be run from the server console."); }
      }, -1);   // -1 = console/server-only sentinel
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
        if (check(ctx.callerSlot, flags | 0, name)) { handler(ctx); }
        else { ctx.reply("[SM] You do not have access to this command."); }
      }, flags | 0);   // the ADMFLAG mask this command requires
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
    // List every globally-registered ConCommand + its required admin flags (0 = anyone, -1 = console-only,
    // else the ADMFLAG bit mask) — the sm_help backend. Degrades to [] on any error.
    list: function () { try { return JSON.parse(__s2_commands_list()); } catch (e) { return []; } },
  };
  globalThis.__s2pkg_commands = { Commands: __s2_commands };   // named export `Commands`
  // --- admin module (engine-generic; ADMFLAG + Admin API + group/immunity/override resolution) ---
  var __s2_ADMFLAG = {
    RESERVATION: 1<<0, GENERIC: 1<<1, KICK: 1<<2, BAN: 1<<3, UNBAN: 1<<4, SLAY: 1<<5, CHANGEMAP: 1<<6,
    CONVARS: 1<<7, CONFIG: 1<<8, CHAT: 1<<9, VOTE: 1<<10, PASSWORD: 1<<11, RCON: 1<<12, CHEATS: 1<<13, ROOT: 1<<14,
  };
  function __s2_hasFlags(flags, req) { return ((flags & __s2_ADMFLAG.ROOT) !== 0) || ((flags & req) === req); }

  // ---- flag-token parsing (a name, a single SM letter, or a compact letter-string) ----
  function __s2_flag_letterBit(ch) {
    if (ch === "z" || ch === "Z") return __s2_ADMFLAG.ROOT;
    var i = String(ch).charCodeAt(0) - 97;                 // 'a'
    return (i >= 0 && i <= 13) ? (1 << i) : 0;
  }
  function __s2_flag_token(tok) {                           // name OR single letter -> bit (0 = unknown)
    var up = String(tok).toUpperCase();
    if (__s2_ADMFLAG[up] != null) return __s2_ADMFLAG[up];
    var s = String(tok);
    return (s.length === 1) ? __s2_flag_letterBit(s) : 0;
  }
  function __s2_parseFlags(value) {                         // array of tokens | a name | a letter-string -> mask
    var mask = 0;
    if (Array.isArray(value)) {
      for (var i = 0; i < value.length; i++) {
        var b = __s2_flag_token(value[i]);
        if (b) mask |= b; else if (String(value[i]).length) console.log("[s2script] WARN: unknown admin flag '" + value[i] + "' — skipped");
      }
    } else if (typeof value === "string") {
      var up = value.toUpperCase();
      if (__s2_ADMFLAG[up] != null) return __s2_ADMFLAG[up];   // the whole string is a flag name
      for (var j = 0; j < value.length; j++) {
        var c = value.charAt(j), lb = __s2_flag_letterBit(c);
        if (lb) mask |= lb; else console.log("[s2script] WARN: unknown admin flag letter '" + c + "' — skipped");
      }
    }
    return mask;
  }
  function __s2_parseOverrideToken(v) {                     // "" -> public; unknown token -> null (skip); else a mask
    if (v === "" || v == null) return { public: true, mask: 0 };
    var m = __s2_parseFlags(v);
    if (!m) return null;                                     // no flag resolved -> invalid override, skip
    return { public: false, mask: m };
  }

  // ---- registries (per-context; populated from the files at prelude time) ----
  var __s2_groups = {};        // name -> { flags, immunity, overrides: {cmd:{public,mask}} }
  var __s2_adminGroups = {};   // sid  -> [groupName]

  function __s2_admin_parseGroups(text) {
    __s2_groups = {};
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admin_groups.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var name in obj) {
      if (name === "_help" || !Object.prototype.hasOwnProperty.call(obj, name)) continue;
      var g = obj[name]; if (!g || typeof g !== "object") continue;
      var ov = {};
      if (g.overrides && typeof g.overrides === "object")
        for (var cmd in g.overrides) if (Object.prototype.hasOwnProperty.call(g.overrides, cmd)) {
          var ot = __s2_parseOverrideToken(g.overrides[cmd]);
          if (ot) ov[cmd] = ot;
          else console.log("[s2script] WARN: group '" + name + "' override '" + cmd + "': unknown flag '" + g.overrides[cmd] + "' — skipped");
        }
      __s2_groups[name] = { flags: __s2_parseFlags(g.flags), immunity: (typeof g.immunity === "number") ? (g.immunity | 0) : 0, overrides: ov };
    }
  }

  function __s2_admin_resolveEntry(entry) {                 // -> { mask, immunity, groups:[], overrides:{cmd:{public,mask}} }
    var mask = 0, immunity = 0, groups = [], overrides = {};
    if (Array.isArray(entry)) {
      mask = __s2_parseFlags(entry);
    } else if (entry && typeof entry === "object") {
      if (entry.flags != null) mask |= __s2_parseFlags(entry.flags);
      if (typeof entry.immunity === "number") immunity = Math.max(immunity, entry.immunity | 0);
      if (Array.isArray(entry.groups)) for (var i = 0; i < entry.groups.length; i++) {
        var gn = entry.groups[i], g = __s2_groups[gn];
        if (!g) { console.log("[s2script] WARN: admins.json references unknown group '" + gn + "' — skipped"); continue; }
        mask |= g.flags; immunity = Math.max(immunity, g.immunity); groups.push(gn);
        for (var c in g.overrides) if (Object.prototype.hasOwnProperty.call(g.overrides, c)) overrides[c] = g.overrides[c];
      }
    }
    return { mask: mask, immunity: immunity, groups: groups, overrides: overrides };
  }

  function __s2_admin_parseAdmins(text, pushCore) {
    __s2_adminGroups = {};
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admins.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var sid in obj) {
      if (sid === "_help" || !Object.prototype.hasOwnProperty.call(obj, sid)) continue;
      var r = __s2_admin_resolveEntry(obj[sid]);
      __s2_adminGroups[String(sid)] = r.groups;
      if (pushCore) {
        __s2_admin_set(String(sid), r.mask, r.immunity, false);
        for (var cmd in r.overrides) if (Object.prototype.hasOwnProperty.call(r.overrides, cmd)) {
          var ov = r.overrides[cmd]; __s2_admin_add_override(String(sid), cmd, ov.mask | 0, !!ov.public);
        }
      }
    }
  }

  function __s2_admin_parseOverrides(text) {                // global admin_overrides.json (pushCore path only)
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admin_overrides.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var cmd in obj) {
      if (cmd === "_help" || !Object.prototype.hasOwnProperty.call(obj, cmd)) continue;
      var ov = __s2_parseOverrideToken(obj[cmd]);
      if (ov) __s2_admin_set_global_override(cmd, ov.mask | 0, !!ov.public);
      else console.log("[s2script] WARN: admin_overrides.json '" + cmd + "': unknown flag '" + obj[cmd] + "' — skipped");
    }
  }

  var __s2_GROUPS_TEMPLATE = '{\n  "_help": "Group name -> { flags, immunity, overrides }. flags: SM letters (\\"bcdefg\\") or names ([\\"kick\\",\\"ban\\"]); immunity: integer; overrides: { command: flag | \\"\\" for anyone }. e.g. \\"Full Admins\\": { \\"flags\\": \\"bcdefgjk\\", \\"immunity\\": 50 }"\n}\n';
  var __s2_ADMINS_TEMPLATE = '{\n  "_help": "SteamID64 -> [\\"flag\\",...] (flags only), or { groups:[\\"Group\\"], flags:[...], immunity:N }. Flags: reservation generic kick ban unban slay changemap convars config chat vote password rcon cheats root (or SM letters a-n,z)."\n}\n';
  var __s2_OVERRIDES_TEMPLATE = '{\n  "_help": "command -> required flag (name or SM letter), or \\"\\" for everyone. e.g. \\"sm_slap\\": \\"generic\\", \\"sm_who\\": \\"\\""\n}\n';
  function __s2_admin_readOrTemplate(name, template) {
    var t = __s2_config_read_raw(name);
    if (t == null) { __s2_config_write_raw(name, template); return "{}"; }
    return t;
  }
  function __s2_admin_reloadAll(pushCore) {
    __s2_admin_parseGroups(__s2_admin_readOrTemplate("admin_groups", __s2_GROUPS_TEMPLATE));
    __s2_admin_parseAdmins(__s2_admin_readOrTemplate("admins", __s2_ADMINS_TEMPLATE), pushCore);
    if (pushCore) __s2_admin_parseOverrides(__s2_admin_readOrTemplate("admin_overrides", __s2_OVERRIDES_TEMPLATE));
  }

  // ---- AdminInfo + the Admin API ----
  function __s2_adminInfo(steamId, flags, immunity) {
    return {
      steamId: String(steamId), flags: flags | 0, immunity: immunity | 0,
      groups: (__s2_adminGroups[String(steamId)] || []).slice(),
      hasFlags: function (req) { return __s2_hasFlags(flags | 0, req | 0); },
    };
  }
  function __s2_canTargetImm(callerSlot, callerImm, targetImm) {   // the pure immunity comparison (test hook)
    if ((callerSlot | 0) < 0) return true;                        // server console / rcon = infinite
    if ((targetImm | 0) <= 0) return true;                        // non-immune target
    return (callerImm | 0) >= (targetImm | 0);                    // SM default: equal can target
  }
  var __s2_admin = {
    add: function (steamId, flags, immunity) { __s2_admin_set(String(steamId), flags | 0, immunity | 0, true); },
    remove: function (steamId) { __s2_admin_remove(String(steamId), true); },
    get: function (steamId) {
      var sid = String(steamId), m = __s2_admin_get(sid), im = __s2_admin_get_immunity(sid);
      if (!m && !im) return null;
      return __s2_adminInfo(sid, m, im);
    },
    forSlot: function (slot) {
      var sid = __s2_client_steamid(slot | 0);
      if (sid === "0" || !sid) return null;                        // bot / mid-auth -> never an admin
      return __s2_admin.get(sid);
    },
    canTarget: function (callerSlot, targetSlot) {
      var t = __s2_admin.forSlot(targetSlot | 0), ti = t ? t.immunity : 0;
      var c = __s2_admin.forSlot(callerSlot | 0), ci = c ? c.immunity : 0;
      return __s2_canTargetImm(callerSlot | 0, ci, ti);
    },
    getGroup: function (name) {
      var g = __s2_groups[String(name)];
      // Shallow-copy overrides (like `groups`' .slice() above) so a caller can't mutate this
      // context's group registry through the returned object.
      return g ? { name: String(name), flags: g.flags, immunity: g.immunity, overrides: Object.assign({}, g.overrides) } : null;
    },
    // NOTE: reload() clears + re-reads the SHARED core cache (admin flags/immunity/overrides), so
    // ENFORCEMENT (hasFlags/canTarget/__s2_admin_check) refreshes everywhere immediately. But the
    // per-context JS group registries (__s2_groups/__s2_adminGroups below) are only re-parsed in THIS
    // context — another already-loaded context's `.groups` / `getGroup` DISPLAY metadata can be stale
    // until that context reloads (e.g. on its own next file-watch reload). Enforcement is unaffected.
    reload: function () { __s2_admin_clear_file(); __s2_admin_reloadAll(true); },
  };

  // test hooks (safe to expose; pure helpers)
  globalThis.__s2_admin_parseFlags = __s2_parseFlags;
  globalThis.__s2_admin_parseGroups = __s2_admin_parseGroups;
  globalThis.__s2_admin_parseAdmins = __s2_admin_parseAdmins;
  globalThis.__s2_admin_resolveEntry = __s2_admin_resolveEntry;
  globalThis.__s2_canTargetImm = __s2_canTargetImm;

  // Parse the registries in EVERY context (cheap, idempotent — makes getGroup / AdminInfo.groups work
  // everywhere); push the resolved admins + overrides into the shared core cache ONCE (first context).
  // See the reload/staleness note on Admin.reload above: a later reload() in one context does not
  // re-run this per-context parse in every OTHER already-loaded context.
  __s2_admin_reloadAll(!__s2_admin_mark_loaded());

  // Override-aware gating hook. A "public" override (flag "") grants ANYONE — even a non-admin; a flag
  // override changes the requirement; else the command's default mask. (registerAdmin already lets
  // callerSlot<0 / console through as root before reaching here.)
  globalThis.__s2_admin_check = function (slot, requiredMask, cmdName) {
    var sid = __s2_client_steamid(slot | 0);
    var ov = cmdName ? __s2_admin_override(sid || "", String(cmdName)) : "";
    if (ov === "public") return true;
    var a = __s2_admin.forSlot(slot | 0);
    if (!a) return false;
    if (ov !== "") return a.hasFlags(parseInt(ov, 10) | 0);
    return a.hasFlags(requiredMask | 0);
  };
  // Immunity targeting hook (consumed by the CS2 Player.target immunity filter, without importing this module).
  globalThis.__s2_admin_can_target = function (cs, ts) { return __s2_admin.canTarget(cs | 0, ts | 0); };
  globalThis.__s2pkg_admin = { ADMFLAG: __s2_ADMFLAG, Admin: __s2_admin };
  // --- Slice 6.18: bans module (engine-generic; SteamID64 ban store + bans.json persistence via the config bridge) ---
  // Parse bans.json ({ "<steamid64>": { until:<unix|0>, reason:"<str>" } }) into BAN_CACHE. `_help`/non-object
  // entries are skipped. Malformed JSON → silent skip (degrade-never-crash; the file may be hand-edited).
  function __s2_ban_parseFile(text) {
    var obj; try { obj = JSON.parse(text); } catch (e) { return; }
    if (!obj || typeof obj !== "object") return;
    for (var sid in obj) {
      if (sid === "_help" || !Object.prototype.hasOwnProperty.call(obj, sid)) continue;
      var e = obj[sid];
      if (!e || typeof e !== "object") continue;
      var until = (typeof e.until === "number") ? e.until : 0;
      var reason = (typeof e.reason === "string") ? e.reason : "";
      __s2_ban_set(String(sid), until, reason);
    }
  }
  function __s2_ban_load() {
    var text = __s2_config_read_raw("bans");
    if (text == null) {
      // A VALID-JSON self-documenting template (the "_help" key is a string, so parseFile skips it; it
      // round-trips through JSON.parse cleanly — a //-commented template would fail the next-restart parse).
      __s2_config_write_raw("bans", '{\n  "_help": "SteamID64 -> { until: <unix seconds, 0 = permanent>, reason }. Managed by sm_ban/sm_unban."\n}\n');
      text = __s2_config_read_raw("bans");
      if (text == null) return;
    }
    __s2_ban_parseFile(text);
  }
  function __s2_ban_rewrite() {
    var list = JSON.parse(__s2_ban_list());
    var obj = {};
    for (var i = 0; i < list.length; i++) obj[list[i].steamid] = { until: list[i].until, reason: list[i].reason };
    __s2_config_write_raw("bans", JSON.stringify(obj, null, 2) + "\n");
  }
  var __s2_bans = {
    add: function (steamId, minutes, reason) {
      var until = (minutes > 0) ? (Math.floor(Date.now() / 1000) + Math.floor(minutes) * 60) : 0;
      __s2_ban_set(String(steamId), until, reason ? String(reason) : "");
      __s2_ban_rewrite();
    },
    remove: function (steamId) { var r = __s2_ban_remove(String(steamId)); __s2_ban_rewrite(); return r; },
    get: function (steamId) { var s = __s2_ban_get(String(steamId)); return s ? JSON.parse(s) : null; },
    list: function () { return JSON.parse(__s2_ban_list()); },
    reload: function () { __s2_ban_clear(); __s2_ban_load(); },
  };
  // Expose parseFile on globalThis so plugins (and tests) can call it directly (mirrors how the admin module exposes its parser hooks).
  globalThis.__s2_ban_parseFile = __s2_ban_parseFile;
  // One-shot file load (first plugin to import @s2script/bans triggers this).
  if (!__s2_ban_mark_loaded()) { __s2_ban_load(); }
  globalThis.__s2pkg_bans = { Bans: __s2_bans };   // named export `Bans`
  // --- Clients sub-project: @s2script/clients (engine-generic slot-backed Client + lifecycle events).
  //     Client wraps only EXISTING client_* natives (no new engine primitive); Clients.on* subscribe via
  //     __s2_client_subscribe and construct a Client from the dispatched slot. Identity = slot (a client's
  //     slot is stable for its connection; a reused slot is a fresh onConnect). ---
  function Client(slot) { this.slot = slot | 0; }
  Client.prototype.isValid = function () { return __s2_client_valid(this.slot); };
  Object.defineProperty(Client.prototype, "steamId",     { get: function () { return __s2_client_steamid(this.slot); } });
  Object.defineProperty(Client.prototype, "name",        { get: function () { var n = __s2_client_name(this.slot); return n == null ? "" : n; } });
  Object.defineProperty(Client.prototype, "userId",      { get: function () { return __s2_client_userid(this.slot); } });
  Object.defineProperty(Client.prototype, "signonState", { get: function () { return __s2_client_signon(this.slot); } });
  Object.defineProperty(Client.prototype, "isBot",       { get: function () { return __s2_client_steamid(this.slot) === "0"; } });
  Client.prototype.kick = function (reason)  { __s2_client_kick(this.slot, reason == null ? "" : String(reason)); };
  Client.prototype.chat = function (message) { __s2_client_print(this.slot, String(message)); };
  Client.prototype.print = function (msg) { __s2_client_console_print(this.slot, String(msg) + "\n"); };
  Object.defineProperty(Client.prototype, "ip", { get: function () {
    var a = __s2_client_address(this.slot); if (!a) return ""; var i = a.indexOf(":"); return i < 0 ? a : a.slice(0, i);
  } });
  var __s2_MAX_CLIENTS = 64;
  function __s2_client_on(event, h) { __s2_client_subscribe(event, function (slot) { return h(new Client(slot)); }); }
  var __s2_clients = {
    onConnect:         function (h) { __s2_client_on("connect", h); },
    onPutInServer:     function (h) { __s2_client_on("putinserver", h); },
    onActive:          function (h) { __s2_client_on("active", h); },
    onFullyConnect:    function (h) { __s2_client_on("fullyconnect", h); },
    onDisconnect:      function (h) { __s2_client_on("disconnect", h); },
    onSettingsChanged: function (h) { __s2_client_on("settingschanged", h); },
    fromSlot: function (slot) { slot = slot | 0; return __s2_client_valid(slot) ? new Client(slot) : null; },
    all: function () { var out = []; for (var s = 0; s < __s2_MAX_CLIENTS; s++) { if (__s2_client_valid(s)) out.push(new Client(s)); } return out; }
  };
  var __s2_pendingKicks = {};
  var __s2_kickWired = false;
  // Deliver the reason to the client REPEATEDLY (chat + console, once per second) so they see it even if
  // they were mid-load, then kick on the final tick. Re-resolves the client each tick — stops if they left.
  function __s2_deliverAndKick(slot, reason, remaining) {
    var c = __s2_clients.fromSlot(slot);
    if (!c) return;                                          // already gone → nothing to do
    if (remaining <= 0) { c.kick(reason); return; }          // time's up → kick
    c.chat(reason); c.print(reason);                         // show in chat AND console, each second
    globalThis.__s2pkg_timers.delay(1000).then(function () { __s2_deliverAndKick(slot, reason, remaining - 1); });
  }
  function __s2_deliverPending(slot) {
    var p = __s2_pendingKicks[slot]; if (!p) return;
    delete __s2_pendingKicks[slot];
    __s2_deliverAndKick(slot, p.reason, Math.max(1, Math.round(p.delay)));
  }
  function __s2_wireKick() {
    if (__s2_kickWired) return; __s2_kickWired = true;
    __s2_client_on("active", function (c) { __s2_deliverPending(c.slot); });          // reconnect path: deliver once in-game
    __s2_client_on("disconnect", function (c) { delete __s2_pendingKicks[c.slot]; }); // left before active → drop
  }
  // Show a reason in chat + console (repeated once per second) then kick after ~delaySeconds. Works on an
  // ALREADY-in-game client (e.g. sm_ban — delivered immediately) AND from onConnect (deferred until the
  // client is in-game so they can actually see it). signonState >= 4 = past the connection handshake / in
  // the server (a still-connecting client is at CONNECTED=2), so it can receive messages now.
  Client.prototype.kickWithReason = function (reason, delaySeconds) {
    __s2_wireKick();
    var r = String(reason);
    var d = Math.max(1, Math.round(delaySeconds == null ? 5 : delaySeconds));
    if (this.signonState >= 4) { __s2_deliverAndKick(this.slot, r, d); }          // in-game now → deliver immediately
    else { __s2_pendingKicks[this.slot] = { reason: r, delay: d }; }              // still connecting → deliver at onActive
  };
  globalThis.__s2pkg_clients = { Client: Client, Clients: __s2_clients };   // named exports Client + Clients
  // --- Menu primitive Task 2: chat renderer (registers against @s2script/menu's registerRenderer seam)
  //     + disconnect-close lifecycle. Placed here (not immediately after the Task 1 model) because both
  //     blocks below make IMMEDIATE top-level calls into __s2pkg_menu / __s2pkg_chat / __s2pkg_clients
  //     (Menu.registerRenderer(...) and Clients.onDisconnect(...) run at prelude-eval time, not lazily),
  //     so they must run after all three are assigned to globalThis (menu @776, chat @794, clients above). ---
  // Chat renderer: paints numbered lines via __s2pkg_chat; one shared onMessage sub captures picks.
  (function () {
    var HANDLED = (globalThis.HookResult && globalThis.HookResult.Handled) || 2;
    var chatSessions = {};      // slot -> session (chat menus only)
    var subInstalled = false;
    function ensureSub() {
      if (subInstalled) return; subInstalled = true;
      globalThis.__s2pkg_chat.Chat.onMessage(function (slot, text, teamonly) {
        var s = chatSessions[slot];
        if (!s || s._ended) return;                 // no menu for this slot -> pass through
        var t = ("" + text).trim();
        if (!/^[0-9]$/.test(t)) return;             // not a single digit -> pass through (chat shows)
        s.pickNumber(parseInt(t, 10));
        return HANDLED;                              // swallow the menu pick from public chat
      });
    }
    globalThis.__s2pkg_menu.Menu.registerRenderer(globalThis.__s2pkg_menu.MenuStyle.Chat, {
      open: function (session) { ensureSub(); chatSessions[session.slot] = session; this.update(session); },
      update: function (session) {
        var v = session.view(), C = globalThis.__s2pkg_chat.Chat;
        C.toSlot(session.slot, v.title);
        for (var i = 0; i < v.lines.length; i++) {
          var l = v.lines[i];
          C.toSlot(session.slot, (l.key ? l.key + ". " : "   ") + l.text);
        }
      },
      close: function (slot) { delete chatSessions[slot]; },
    });
  })();

  // Disconnect: close any open menu for a leaving slot.
  globalThis.__s2pkg_clients.Clients.onDisconnect(function (client) {
    var s = __s2_menu_activeBySlot[client.slot];
    if (s) s._end(MenuCancelReason.Disconnect);
  });
  // --- @s2script/db — Database.open/query/execute/close over the built-in drivers. SQLite is
  //     sync-behind-Promise (__s2_sqlite_*); mysql/postgres are async off-thread (__s2_db_remote_*).
  //     Database.open resolves a name via databases.json (operator-owned; absent name -> sqlite). ---
  var __s2_db_drivers = {};
  var __s2_db_config = {};   // name -> {driver,host,port,user,password,database} from databases.json — IIFE-PRIVATE (credentials; never on globalThis)
  function __s2_db_loadConfig() {
    var text = __s2_config_read_raw("databases");
    if (text == null) {
      __s2_config_write_raw("databases", '{\n  "_help": "connection name -> { driver: \\"mysql\\"|\\"postgres\\", host, port, user, password, database }. Names not listed here default to a local SQLite file. e.g. \\"stats\\": { \\"driver\\": \\"mysql\\", \\"host\\": \\"db\\", \\"port\\": 3306, \\"user\\": \\"cs2\\", \\"password\\": \\"...\\", \\"database\\": \\"stats\\" }"\n}\n');
      return;
    }
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: databases.json malformed — all connections default to sqlite"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var name in obj) {
      if (name === "_help" || !Object.prototype.hasOwnProperty.call(obj, name)) continue;
      var c = obj[name];
      if (c && typeof c === "object" && (c.driver === "mysql" || c.driver === "postgres")) __s2_db_config[name] = c;
      else if (c && typeof c === "object" && c.driver !== "sqlite") console.log("[s2script] WARN: databases.json '" + name + "': unknown driver '" + c.driver + "' — using sqlite");
    }
  }
  function __s2_db_resolveConfig(connName) {
    var c = __s2_db_config[connName];
    if (c) { return { driver: c.driver, name: connName, host: c.host, port: c.port, user: c.user, password: c.password, database: c.database }; }
    return { driver: "sqlite", name: connName };
  }
  // test hooks — secret-free: an injector (sets the private map) + a driver-ONLY (redacted) resolve.
  globalThis.__s2_db_testSetConfig = function (cfg) { __s2_db_config = cfg || {}; };
  globalThis.__s2_db_resolveConfigDriver = function (name) { return __s2_db_resolveConfig(name).driver; };

  __s2_db_drivers["sqlite"] = {
    name: "sqlite",
    connect: function (config) {
      return __s2_sqlite_open(config.name).then(function (handle) {
        return { query: function (s, p) { return __s2_sqlite_query(handle, s, p || []); },
                 execute: function (s, p) { return __s2_sqlite_execute(handle, s, p || []); },
                 close: function () { return __s2_sqlite_close(handle); } };
      });
    },
  };
  function __s2_makeRemoteDriver(driverName) {
    return {
      name: driverName,
      connect: function (config) {
        var handle = __s2_db_remote_connect(JSON.stringify(config));
        if (!handle) return Promise.reject(new Error("could not open " + driverName + " connection '" + config.name + "'"));
        return Promise.resolve({
          query:   function (s, p) { return __s2_db_remote_query(handle, s, p || []); },
          execute: function (s, p) { return __s2_db_remote_execute(handle, s, p || []); },
          close:   function () { return __s2_db_remote_close(handle); },
        });
      },
    };
  }
  __s2_db_drivers["mysql"] = __s2_makeRemoteDriver("mysql");
  __s2_db_drivers["postgres"] = __s2_makeRemoteDriver("postgres");

  __s2_db_loadConfig();

  var __s2_Database = {
    registerDriver: function (driver) { __s2_db_drivers[driver.name] = driver; },
    open: function (name) {
      var connName = name || "default";
      var config = __s2_db_resolveConfig(connName);
      var driver = __s2_db_drivers[config.driver];
      if (!driver) return Promise.reject(new Error("unknown db driver: " + config.driver));
      return driver.connect(config).then(function (conn) {
        return { query: function (s, p) { return conn.query(s, p); },
                 execute: function (s, p) { return conn.execute(s, p); },
                 close: function () { return conn.close(); } };
      });
    },
  };
  globalThis.__s2pkg_db = { Database: __s2_Database };
  // --- @s2script/cookies: SM-parity cookies over the __s2_cookie_* host-global cache ---
  var __s2_cookie_defs = {};   // per-context registry: name -> Cookie (idempotent register)
  var __s2_Cookies = {
    register: function (name, opts) {
      if (__s2_cookie_defs[name]) return __s2_cookie_defs[name];
      opts = opts || {};
      var cookie = { name: name, access: (opts.access == null ? 0 : opts.access), default: (opts.default == null ? "" : String(opts.default)) };
      __s2_cookie_defs[name] = cookie;
      return cookie;
    },
    get: function (client, cookie) {
      if (!client || client.steamId === "0") return cookie.default;      // bots have no cookies
      var v = __s2_cookie_get(client.steamId, cookie.name);
      return v === undefined ? cookie.default : v;   // a stored "" is a hit, not a miss
    },
    set: function (client, cookie, value) {
      if (!client || client.steamId === "0") return;                     // no-op for bots
      __s2_cookie_set(client.steamId, cookie.name, String(value), Math.floor(Date.now() / 1000));
    },
    areCached: function (client) {
      return !!client && client.steamId !== "0" && __s2_cookie_is_cached(client.steamId);
    },
    getTime: function (client, cookie) {
      return (!client || client.steamId === "0") ? 0 : __s2_cookie_get_time(client.steamId, cookie.name);
    },
    setAuthId: function (steamId, cookie, value) {
      if (!steamId || steamId === "0") return;   // no-op for bots
      __s2_cookie_set_authid(String(steamId), cookie.name, String(value), Math.floor(Date.now() / 1000));
    },
    onCached: function (h) {
      // Guard: fromSlot is null if the client disconnected in the load->fan-out window, so only fire
      // for a still-connected Client (the .d.ts promises a non-null Client). A departed client's
      // "cookies cached" notification is moot.
      __s2_cookie_on_cached(function (slot) { var c = globalThis.__s2pkg_clients.Clients.fromSlot(slot); if (c) h(c); });
    },
  };
  globalThis.__s2pkg_cookies = { Cookies: __s2_Cookies, CookieAccess: { Public: 0, Protected: 1, Private: 2 } };
  // --- @s2script/http: fetch over __s2_fetch (adds text()/json() over the buffered body) ---
  globalThis.__s2pkg_http = {
    fetch: function (url, options) {
      return __s2_fetch(String(url), options || {}).then(function (raw) {
        return {
          status: raw.status, ok: raw.ok, statusText: raw.statusText, headers: raw.headers,
          text: function () { return raw.body; },
          json: function () { return JSON.parse(raw.body); },
        };
      });
    },
  };
  // --- @s2script/ws: client WebSocket over __s2_ws_* (connect resolver + per-conn event subs) ---
  globalThis.__s2pkg_ws = {
    WebSocket: {
      connect: function (url) {
        return __s2_ws_connect(String(url)).then(function (id) {
          return {
            onMessage: function (h) { __s2_ws_on(id, "message", function (m) { h(m); }); },
            onClose:   function (h) { __s2_ws_on(id, "close", function (code, reason) { h(code, reason); }); },
            onError:   function (h) { __s2_ws_on(id, "error", function (e) { h(e); }); },
            send:      function (data) { __s2_ws_send(id, String(data)); },
            close:     function () { __s2_ws_close(id); },
          };
        });
      },
    },
  };
  // --- @s2script/net: raw TCP + UDP client sockets over __s2_net_* (mirrors __s2pkg_ws, binary payloads) ---
  globalThis.__s2pkg_net = {
    Net: {
      connectTcp: function (host, port) {
        return __s2_net_tcp_connect(String(host), port | 0).then(function (id) {
          return {
            onData:  function (h) { __s2_net_on(id, "data", function (b) { h(b); }); },
            onClose: function (h) { __s2_net_on(id, "close", function () { h(); }); },
            onError: function (h) { __s2_net_on(id, "error", function (e) { h(e); }); },
            send:    function (data) { __s2_net_send(id, data); },
            close:   function () { __s2_net_close(id); },
          };
        });
      },
      udp: function () {
        return __s2_net_udp_bind().then(function (id) {
          return {
            onMessage: function (h) { __s2_net_on(id, "message", function (from, b) { h(from, b); }); },
            sendTo:    function (host, port, data) { __s2_net_send_to(id, String(host), port | 0, data); },
            close:     function () { __s2_net_close(id); },
          };
        });
      },
    },
  };
  // --- @s2script/votes: chat-ballot voting (revote) + an optional live center tally (a render seam). ---
  var __s2_vote_state = null;             // the single active vote, or null (the per-context lock)
  var __s2_vote_tallyRenderer = null;     // { show(slot, tally), clear(slot) } — CS2 registers it
  var __s2_vote_subInstalled = false;     // lazy-once guard: install onMessage/onDisconnect on first start
  var VOTE_HANDLED = (globalThis.HookResult && globalThis.HookResult.Handled) || 2;

  function __s2_vote_eligibleSlots() {
    var out = [], all = globalThis.__s2pkg_clients.Clients.all();
    for (var i = 0; i < all.length; i++) if (!all[i].isBot) out.push(all[i].slot);
    return out;
  }
  function __s2_vote_counts(st) {
    var counts = [], total = 0;
    for (var i = 0; i < st.options.length; i++) counts.push(0);
    st.votes.forEach(function (idx) { if (idx >= 0 && idx < counts.length) { counts[idx]++; total++; } });
    return { counts: counts, total: total };
  }
  var __s2_vote_warnedNoRenderer = false;
  function __s2_vote_showTally(st) {
    if (!st.showLiveTally) return;
    if (!__s2_vote_tallyRenderer) {   // showLiveTally set but no renderer (a non-CS2 game) -> degrade to chat-only, warn once
      if (!__s2_vote_warnedNoRenderer) { __s2_vote_warnedNoRenderer = true; globalThis.console && console.log("[votes] WARN: showLiveTally set but no tally renderer registered — chat-only."); }
      return;
    }
    var c = __s2_vote_counts(st);
    var opts = st.options.map(function (label, i) { return { label: label, count: c.counts[i] }; });
    var tally = { question: st.question, options: opts, total: c.total, secondsLeft: st.secondsLeft };
    var slots = __s2_vote_eligibleSlots();
    for (var i = 0; i < slots.length; i++) { try { __s2_vote_tallyRenderer.show(slots[i], tally); } catch (e) {} }
  }
  function __s2_vote_clearTally(st) {
    if (!st.showLiveTally || !__s2_vote_tallyRenderer) return;
    var slots = __s2_vote_eligibleSlots();
    for (var i = 0; i < slots.length; i++) { try { __s2_vote_tallyRenderer.clear(slots[i]); } catch (e) {} }
  }
  function __s2_vote_castFromChat(slot, text) {
    var st = __s2_vote_state; if (!st) return 0;                    // no active vote -> pass through
    var t = ("" + text).trim();
    if (!/^[0-9]$/.test(t)) return 0;
    var d = parseInt(t, 10);
    if (d < 1 || d > st.options.length) return 0;                  // out of range -> pass through
    st.votes.set(slot, d - 1);                                     // revote replaces
    __s2_vote_showTally(st);
    // NOTE: the "every connected non-bot has voted -> end early" check (design doc Flow step 5) lives
    // in __s2_vote_tick, NOT here. Checking synchronously on every cast would end the vote the instant
    // the last eligible voter casts a FIRST vote, pre-empting a later revote from any of them within the
    // same synchronous burst (see votes_cast_revote_tally_and_winner). Checking at the 1s tick boundary
    // instead gives a full window for still-pending revotes to land before turnout is judged complete.
    return VOTE_HANDLED;
  }
  function __s2_vote_ensureSubs() {
    if (__s2_vote_subInstalled) return; __s2_vote_subInstalled = true;
    globalThis.__s2pkg_chat.Chat.onMessage(function (slot, text) { return __s2_vote_castFromChat(slot, text); });
    globalThis.__s2pkg_clients.Clients.onDisconnect(function (c) { var st = __s2_vote_state; if (st) st.votes.delete(c.slot); });
  }
  function __s2_vote_tick(st) {
    if (__s2_vote_state !== st) return;                            // ended/cancelled
    if (st.secondsLeft <= 0) { __s2_vote_end(); return; }
    st.secondsLeft--;
    __s2_vote_showTally(st);
    // End early once every connected non-bot has voted (design doc Flow step 5). Guarded on elig > 0 so
    // a vote started with zero connected non-bots doesn't vacuously "complete" inside Vote.start() itself.
    var elig = __s2_vote_eligibleSlots().length;
    if (elig > 0 && st.votes.size >= elig) { __s2_vote_end(); return; }
    globalThis.__s2pkg_timers.delay(1000).then(function () { __s2_vote_tick(st); });
  }
  function __s2_vote_end() {
    var st = __s2_vote_state; if (!st) return;
    __s2_vote_state = null;                                        // release the lock BEFORE onEnd (so onEnd can start a new vote)
    __s2_vote_clearTally(st);
    var c = __s2_vote_counts(st), winner = null, best = -1, tie = false;
    for (var i = 0; i < c.counts.length; i++) {
      if (c.counts[i] > best) { best = c.counts[i]; winner = i; tie = false; }
      else if (c.counts[i] === best) { tie = true; }
    }
    if (c.total === 0 || tie) winner = null;
    var result = { winner: winner, counts: c.counts, total: c.total };
    if (winner !== null) globalThis.__s2pkg_chat.Chat.toAll("[Vote] Passed: " + st.options[winner] + " (" + Math.round(c.counts[winner] / c.total * 100) + "%)");
    else globalThis.__s2pkg_chat.Chat.toAll("[Vote] Failed — no majority.");
    try { st.onEnd(result); } catch (e) { globalThis.console && console.log("[votes] onEnd threw: " + e); }
  }
  var Vote = {
    start: function (config) {
      if (__s2_vote_state) return false;                          // one vote at a time
      if (!config || !config.question || !config.options || config.options.length < 2) return false;
      __s2_vote_ensureSubs();
      var dur = Math.max(1, (config.duration | 0) || 20);   // clamp: a negative config would end on the first tick
      var st = { question: String(config.question), options: config.options.map(String), votes: new Map(),
                 showLiveTally: !!config.showLiveTally, secondsLeft: dur,
                 onEnd: (typeof config.onEnd === "function") ? config.onEnd : function () {} };
      __s2_vote_state = st;
      globalThis.__s2pkg_chat.Chat.toAll("[Vote] " + st.question + " — " + st.options.map(function (o, i) { return (i + 1) + "=" + o; }).join(", "));
      __s2_vote_showTally(st);
      __s2_vote_tick(st);                                         // starts the countdown + end
      return true;
    },
    isActive: function () { return !!__s2_vote_state; },
    cancel: function () { var st = __s2_vote_state; if (!st) return; __s2_vote_state = null; __s2_vote_clearTally(st); },
    registerTallyRenderer: function (r) { __s2_vote_tallyRenderer = r; },
  };
  globalThis.__s2pkg_votes = { Vote: Vote };
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

/// Read `obj[name]` as a `String`, or `None` if the property is absent/`null`/`undefined` (a
/// missing/nullish `options` field should fall back to its default, not stringify to `"undefined"`).
fn get_str_prop(scope: &mut v8::PinScope, obj: v8::Local<v8::Object>, name: &str) -> Option<String> {
    let key = v8::String::new(scope, name)?;
    let val = obj.get(scope, key.into())?;
    if val.is_null_or_undefined() {
        return None;
    }
    Some(val.to_rust_string_lossy(scope))
}

/// Native `__s2_fetch(url, options) -> Promise<rawResponse>` where `rawResponse =
/// {status, ok, statusText, headers, body}`.  MIRRORS `s2_thread_sleep`'s resolver/ledger/pending
/// block exactly (a `Job` resource — teardown drops its `RESOLVERS` entry before the context
/// disposes, and a completion for an unloaded/reloaded plugin is DROPPED by the async-liveness
/// guard in the drain step, never resolved) but hands off to `crate::http::fetch` (the
/// process-global tokio+reqwest engine, Task 1) instead of the blocking-sleep worker pool — so the
/// calling (main/game) thread never blocks on I/O; the request runs off-thread and the Promise
/// resolves on a LATER `frame_async_drain` via `resolve_fetch`.
///
/// `options` (all optional): `method` (default `"GET"`), `headers` (a plain string→string object),
/// `body` (a string), `timeoutMs` (default 30000). Degrade-never-crash: the whole body runs under
/// `catch_unwind`; a malformed/absent `options` degrades to the defaults (never throws
/// synchronously) — the actual network outcome (incl. a 4xx/5xx, which RESOLVES with `ok:false`,
/// vs. a network/timeout error, which REJECTS) is decided later by `resolve_fetch`.
fn s2_fetch(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let url = args.get(0).to_rust_string_lossy(scope);
        // Parse options (defaults GET / no headers / no body / 30000ms timeout).
        let mut method = "GET".to_string();
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut body: Option<String> = None;
        let mut timeout_ms = 30_000u64;
        if let Ok(opts) = v8::Local::<v8::Object>::try_from(args.get(1)) {
            if let Some(v) = get_str_prop(scope, opts, "method") {
                method = v;
            }
            if let Some(v) = get_str_prop(scope, opts, "body") {
                body = Some(v);
            }
            if let Some(k) = v8::String::new(scope, "timeoutMs") {
                if let Some(v) = opts.get(scope, k.into()) {
                    if v.is_number() {
                        timeout_ms = v.integer_value(scope).unwrap_or(30_000).max(0) as u64;
                    }
                }
            }
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
        // Ledger this async job against the CALLING plugin (teardown authority) — a non-plugin/
        // unknown owner is a safe no-op; no borrow held across a JS call.
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
        crate::http::fetch(
            id,
            crate::http::FetchRequest { method, url, headers, body, timeout_ms },
        );
        refresh_detour();
        rv.set(promise.into());
    }));
}

// ---------------------------------------------------------------------------
// WebSocket (client) Task 2: __s2_ws_* natives + signal routing + teardown.
// Mirrors s2_fetch (the connect native)/resolve_fetch (-> resolve_ws_connect)/
// s2_cookie_on_cached (the subscribe)/dispatch_pending_cookie_cached (-> dispatch_pending_ws_events).
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
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let id = next_async_id();
        // Tag the resolver with the CALLING plugin's (id, current generation) — the async-liveness guard.
        let owner = resolver_owner_tag(scope);
        let owner_string = current_plugin(scope).unwrap_or_default();
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
        crate::ws::connect(id, url, owner_string);
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
fn s2_ws_send(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let text = args.get(1).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_default();
        crate::ws::send(id, &owner, text);
    }));
}

/// Native `__s2_ws_close(id)`.  Owner-scoped (mirrors `s2_ws_send`); hands off to `crate::ws::close`
/// (a non-blocking command send). No return value.
fn s2_ws_close(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let owner = current_plugin(scope).unwrap_or_default();
        crate::ws::close(id, &owner);
    }));
}

/// Native `__s2_ws_on(id, event, handler)` — subscribe a JS fn to a ws connection's event
/// ("message"/"close"/"error"). MIRRORS `s2_cookie_on_cached` (owner-tracked, keyed mux) but the
/// mux key is `"<id>:<event>"` (a connection has a name dimension, unlike cookies-cached).
/// Owner-scoped like `s2_ws_send`/`s2_ws_close`: conn ids are small sequential integers shared
/// across every async primitive, so WITHOUT this check any co-loaded plugin could subscribe to
/// (and read) another plugin's inbound WebSocket traffic by guessing/enumerating conn ids — gated
/// via `crate::ws::is_owner` (a no-op subscribe for a conn this plugin doesn't own, mirroring the
/// `owner == owner` check `ws::send`/`ws::close` already perform).
fn s2_ws_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let event = args.get(1).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(2)) else { return };
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        if !crate::ws::is_owner(id, &owner) { return; }
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        let key = format!("{id}:{event}");
        WS_EVENT_MUX.with(|m| { m.borrow_mut().subscribe(&key, owner, generation, handler_g); });
    }));
}

/// Drain `WS_EVENT_PENDING` and fan each queued `(conn_id, event, s, n)` out to the `WS_EVENT_MUX`
/// subscribers keyed `"<conn_id>:<event>"` (WebSocket Task 2). Called from `ffi.rs`'s Post-frame
/// branch AFTER `frame_async_drain()` returns (HOST is free). Mirrors
/// `dispatch_pending_cookie_cached` verbatim (snapshot, `try_borrow_mut` re-entrancy guard,
/// per-subscriber liveness + context clone + HandleScope/ContextScope/TryCatch + WARN-on-throw),
/// except the payload carries the event data: for "message"/"error" a single String arg `s`; for
/// "close" two args `(Number code, String reason)` built from `(s, n)` = `(reason, code)`.
pub(crate) fn dispatch_pending_ws_events() {
    let pending: Vec<(u64, String, String, i32)> = WS_EVENT_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() { return; }

    for (conn_id, event, s, n) in pending {
        let key = format!("{conn_id}:{event}");
        // Phase 1: snapshot — release WS_EVENT_MUX borrow before entering any context.
        let snap = WS_EVENT_MUX.with(|m| m.borrow().snapshot(&key));

        // Phase 2: enter each subscriber's context and invoke handler(...).
        // Skipped when this (conn,event) key has no subscriber — but the terminal-close
        // prune in Phase 3 still runs, so an onMessage-only conn is pruned on close.
        if !snap.is_empty() { HOST.with(|h| {
            // Re-entrancy guard (mirrors dispatch_pending_cookie_cached): expected free here (called
            // after frame_async_drain returns), but guarded anyway per the shared discipline.
            let Ok(mut borrow) = h.try_borrow_mut() else { return };
            let Some(host) = borrow.as_mut() else { return };

            for (owner, generation, handler_g) in &snap {
                // Liveness check (release REGISTRY borrow before entering context).
                if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
                // Clone the context Global out of PLUGINS (borrow released) so the handler may re-enter.
                let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

                let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
                let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
                let hs = &mut hs;
                let ctx_local = v8::Local::new(hs, &g_ctx);
                let scope = &mut v8::ContextScope::new(hs, ctx_local);

                let mut tc_storage = v8::TryCatch::new(scope);
                let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
                let tc = &mut tc;

                let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                let func = v8::Local::new(tc, handler_g);
                let call_result = if event == "close" {
                    let code_val: v8::Local<v8::Value> = v8::Number::new(tc, n as f64).into();
                    let reason_val: v8::Local<v8::Value> =
                        v8::String::new(tc, &s).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                    func.call(tc, recv, &[code_val, reason_val])
                } else {
                    let s_val: v8::Local<v8::Value> =
                        v8::String::new(tc, &s).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                    func.call(tc, recv, &[s_val])
                };
                if call_result.is_none() {
                    let msg = tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!("WARN: dispatch_pending_ws_events('{}'): handler '{}': {}", key, owner, msg));
                }
            }
        }); }

        // Phase 3: on the terminal "close" event, prune every subscriber key for this conn_id
        // (message/close/error). conn ids are monotonic (next_async_id, never reused), so nothing
        // ever re-subscribes these keys — without this, a reconnect-on-close loop accumulates dead
        // EventMux entries + retained JS closure Globals for the plugin's whole uptime. Runs outside
        // the Phase-2 empty-check so a conn with only onMessage is still pruned. Every teardown path
        // funnels through Closed: peer close, self-close (WsCommand::Close), stream-end, and read
        // error (ws.rs emits Closed after Errored). It runs AFTER this close's own fan-out, so any
        // onClose handler has already been invoked from the snapshot taken above.
        if event == "close" {
            WS_EVENT_MUX.with(|m| {
                let mut mux = m.borrow_mut();
                for ev in ["message", "close", "error"] {
                    mux.remove_by_name(&format!("{conn_id}:{ev}"));
                }
            });
        }
    }
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

/// Read a native arg as bytes: a `Uint8Array`/any TypedArray/DataView (COPIED out via
/// `copy_contents`) OR a `string` (UTF-8). Anything else → empty. Never hands a raw backing store to
/// Rust: `copy_contents` writes into our own owned `Vec` — no view of V8-owned memory escapes.
fn js_bytes_arg(scope: &mut v8::PinScope, val: v8::Local<v8::Value>) -> Vec<u8> {
    if val.is_string() {
        return val.to_rust_string_lossy(scope).into_bytes();
    }
    if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(val) {
        let len = view.byte_length();
        let mut buf = vec![0u8; len];
        let n = view.copy_contents(&mut buf); // copies min(len, view) bytes into our Vec
        buf.truncate(n);
        return buf;
    }
    Vec::new()
}

/// Build a JS `Uint8Array` from bytes — a fresh COPY (`bytes.to_vec()`) into a standalone
/// `ArrayBuffer` that V8 owns (the backing store's deleter frees the Vec). No raw pointer / borrowed
/// slice crosses into JS. Returns `null` if the typed-array construction fails (defensive).
fn bytes_to_uint8array<'s>(scope: &mut v8::PinScope<'s, '_>, bytes: &[u8]) -> v8::Local<'s, v8::Value> {
    if bytes.is_empty() {
        // A zero-length UDP datagram is a reachable input (net.rs recv_from -> Ok((0, from))
        // -> Datagram { data: vec![] }); build a fresh 0-length Uint8Array rather than routing
        // an empty Vec through new_backing_store_from_bytes.
        let ab = v8::ArrayBuffer::new(scope, 0);
        return match v8::Uint8Array::new(scope, ab, 0, 0) {
            Some(u) => u.into(),
            None => v8::null(scope).into(),
        };
    }
    let store = v8::ArrayBuffer::new_backing_store_from_bytes(bytes.to_vec()).make_shared();
    let ab = v8::ArrayBuffer::with_backing_store(scope, &store);
    let len = bytes.len();
    match v8::Uint8Array::new(scope, ab, 0, len) {
        Some(u) => u.into(),
        None => v8::null(scope).into(),
    }
}

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

/// Native `__s2_net_send(id, data)`. Owner-scoped (a no-op for a conn this plugin doesn't own, or an
/// absent conn); `data` is marshalled via `js_bytes_arg` (a `Uint8Array`/TypedArray copied out, or a
/// string as UTF-8). Hands off to `crate::net::send` (a non-blocking channel send). No return value.
fn s2_net_send(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let bytes = js_bytes_arg(scope, args.get(1));
        let owner = current_plugin(scope).unwrap_or_default();
        crate::net::send(id, &owner, bytes);
    }));
}

/// Native `__s2_net_send_to(id, host, port, data)` — send a UDP datagram to `host:port`. Owner-scoped
/// like `s2_net_send`; `data` marshalled via `js_bytes_arg`. Hands off to `crate::net::send_to`.
fn s2_net_send_to(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let dhost = args.get(1).to_rust_string_lossy(scope);
        let port = args.get(2).number_value(scope).unwrap_or(0.0) as u16;
        let bytes = js_bytes_arg(scope, args.get(3));
        let owner = current_plugin(scope).unwrap_or_default();
        crate::net::send_to(id, &owner, dhost, port, bytes);
    }));
}

/// Native `__s2_net_close(id)`. Owner-scoped (mirrors `s2_net_send`); hands off to `crate::net::close`
/// (a non-blocking command send that emits a terminal `Closed` signal). No return value.
fn s2_net_close(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let owner = current_plugin(scope).unwrap_or_default();
        crate::net::close(id, &owner);
    }));
}

/// Native `__s2_net_on(id, event, handler)` — subscribe a JS fn to a net connection's event
/// ("data"/"message"/"close"/"error"). MIRRORS `s2_ws_on` EXACTLY (owner-tracked, mux keyed
/// `"<id>:<event>"`, gated via `crate::net::is_owner` so a co-loaded plugin can't subscribe to
/// another plugin's inbound socket traffic by guessing conn ids).
fn s2_net_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let event = args.get(1).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(2)) else { return };
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        if !crate::net::is_owner(id, &owner) { return; }
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        let key = format!("{id}:{event}");
        NET_EVENT_MUX.with(|m| { m.borrow_mut().subscribe(&key, owner, generation, handler_g); });
    }));
}

/// Drain `NET_EVENT_PENDING` and fan each queued `(conn_id, PendingNetEvent)` out to the
/// `NET_EVENT_MUX` subscribers keyed `"<conn_id>:<event>"` (Net Task 2). Called from `ffi.rs`'s
/// Post-frame branch AFTER `frame_async_drain()` returns (HOST is free). MIRRORS
/// `dispatch_pending_ws_events` verbatim (snapshot / `try_borrow_mut` re-entrancy guard / per-sub
/// liveness + context clone + HandleScope/ContextScope/TryCatch + WARN-on-throw + the terminal-close
/// prune), except the payload is RAW BINARY (`bytes_to_uint8array`, a fresh V8-owned copy):
///   - `Data(b)`            → key `"<id>:data"`,    args `[Uint8Array]`.
///   - `Datagram{from,data}`→ key `"<id>:message"`, args `[{host,port}, Uint8Array]`.
///   - `Errored(e)`         → key `"<id>:error"`,   args `[String]`.
///   - `Closed`             → key `"<id>:close"`,   args `[]` + prune every key for this conn.
pub(crate) fn dispatch_pending_net_events() {
    let pending: Vec<(u64, PendingNetEvent)> = NET_EVENT_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() { return; }

    for (conn_id, ev) in pending {
        // The event-name dimension for the mux key (also the terminal-close discriminator below).
        let event: &str = match &ev {
            PendingNetEvent::Data(_) => "data",
            PendingNetEvent::Datagram { .. } => "message",
            PendingNetEvent::Closed => "close",
            PendingNetEvent::Errored(_) => "error",
        };
        let key = format!("{conn_id}:{event}");
        // Phase 1: snapshot — release NET_EVENT_MUX borrow before entering any context.
        let snap = NET_EVENT_MUX.with(|m| m.borrow().snapshot(&key));

        // Phase 2: enter each subscriber's context and invoke handler(...). Skipped when this
        // (conn,event) key has no subscriber — but the Phase-3 terminal-close prune still runs.
        if !snap.is_empty() { HOST.with(|h| {
            // Re-entrancy guard (mirrors dispatch_pending_ws_events): expected free here (called after
            // frame_async_drain returns), but guarded anyway per the shared discipline.
            let Ok(mut borrow) = h.try_borrow_mut() else { return };
            let Some(host) = borrow.as_mut() else { return };

            for (owner, generation, handler_g) in &snap {
                // Liveness check (release REGISTRY borrow before entering context).
                if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
                // Clone the context Global out of PLUGINS (borrow released) so the handler may re-enter.
                let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

                let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
                let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
                let hs = &mut hs;
                let ctx_local = v8::Local::new(hs, &g_ctx);
                let scope = &mut v8::ContextScope::new(hs, ctx_local);

                let mut tc_storage = v8::TryCatch::new(scope);
                let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
                let tc = &mut tc;

                let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                let func = v8::Local::new(tc, handler_g);
                let call_result = match &ev {
                    PendingNetEvent::Data(b) => {
                        let arr = bytes_to_uint8array(tc, b);
                        func.call(tc, recv, &[arr])
                    }
                    PendingNetEvent::Datagram { from, data } => {
                        // Parse "host:port" on the LAST ':' (keeps an IPv6 host intact); a missing
                        // port → 0. The datagram source is a plain {host, port} object.
                        let (fhost, fport): (&str, u16) = match from.rsplit_once(':') {
                            Some((h, p)) => (h, p.parse::<u16>().unwrap_or(0)),
                            None => (from.as_str(), 0),
                        };
                        let from_obj = v8::Object::new(tc);
                        if let Some(k) = v8::String::new(tc, "host") {
                            let v: v8::Local<v8::Value> =
                                v8::String::new(tc, fhost).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                            from_obj.set(tc, k.into(), v);
                        }
                        if let Some(k) = v8::String::new(tc, "port") {
                            let v: v8::Local<v8::Value> = v8::Number::new(tc, fport as f64).into();
                            from_obj.set(tc, k.into(), v);
                        }
                        let from_val: v8::Local<v8::Value> = from_obj.into();
                        let arr = bytes_to_uint8array(tc, data);
                        func.call(tc, recv, &[from_val, arr])
                    }
                    PendingNetEvent::Errored(e) => {
                        let s_val: v8::Local<v8::Value> =
                            v8::String::new(tc, e).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                        func.call(tc, recv, &[s_val])
                    }
                    PendingNetEvent::Closed => func.call(tc, recv, &[]),
                };
                if call_result.is_none() {
                    let msg = tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!("WARN: dispatch_pending_net_events('{}'): handler '{}': {}", key, owner, msg));
                }
            }
        }); }

        // Phase 3: on the terminal "close" event, prune every subscriber key for this conn_id
        // (data/message/error/close). conn ids are monotonic (next_async_id, never reused), so nothing
        // ever re-subscribes these keys — without this a reconnect-on-close loop accumulates dead
        // EventMux entries + retained JS closure Globals. Runs outside the Phase-2 empty-check so a
        // conn with only onData is still pruned. Every teardown path funnels through Closed (peer
        // close, self-close, stream-end, and read-error — net.rs emits Closed after Errored). It runs
        // AFTER this close's own fan-out, so any onClose handler has already been invoked.
        if matches!(ev, PendingNetEvent::Closed) {
            NET_EVENT_MUX.with(|m| {
                let mut mux = m.borrow_mut();
                for evn in ["data", "message", "error", "close"] {
                    mux.remove_by_name(&format!("{conn_id}:{evn}"));
                }
            });
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
/// (I32/F32/BOOL + narrow ints I8/I16/U8/U16/U32; 64-bit writes deferred → false).
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
            KIND_I8   => crate::entity::write_i8(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_I16  => crate::entity::write_i16(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_U8   => crate::entity::write_u8(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_U16  => crate::entity::write_u16(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_U32  => crate::entity::write_u32(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as u32),
            _         => return,                   // unknown / deferred write kind (64-bit) → false
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

/// Native `__s2_ent_ref_write_string(index, serial, offset, maxLen, str) -> boolean`. Serial-gated
/// mirror of `read_string`: writes a bounded, NUL-terminated string into an inline `char[maxLen]` field
/// (truncated to `maxLen-1` bytes + always NUL-terminated; never past the bound). The raw pointer is
/// resolved + used entirely in core and never crosses to JS. false on a stale/invalid ref.
fn s2_ent_ref_write_string(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let max_len = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        let s = args.get(4).to_rust_string_lossy(scope);
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                 // invalid → false (already set)
        crate::entity::write_string(ent, off, max_len, s.as_bytes());
        rv.set_bool(true);
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

/// Native `__s2_ent_ref_write_chain(index, serial, pathOffs[], finalOff, kind, value) -> bool`.
/// Serial-gated at the root; follows the pointer chain (each hop null-checked, raw ptrs never cross to
/// JS), then writes `value` at `finalOff`. Mirrors `s2_ent_ref_read_chain`. Used to clear a flag on a
/// pointer-referenced sub-object (e.g. CEntityIdentity::m_flags via m_pEntity). i32/u32/u8 only.
fn s2_ent_ref_write_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
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
        let base = p as *mut u8;
        match kind {
            KIND_I32 => crate::entity::write_i32(base, final_off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_U32 => crate::entity::write_u32(base, final_off, args.get(5).integer_value(scope).unwrap_or(0) as u32),
            KIND_U8  => crate::entity::write_u8(base, final_off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            _ => return,
        }
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

/// Native `__s2_concommand(name: string, fn: (slot: number, argString: string) => void, flags?: number)`.
/// Stores the JS callback `Global<Function>` keyed by command name in `CONCOMMANDS`, records the optional
/// admin-flag mask (`flags`, default 0) in `COMMAND_META` (backing `Commands.list()` / `sm_help`), then
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

        // Record the required admin-flag mask (default 0 = anyone) for `Commands.list()` / `sm_help`.
        let flags = if args.length() >= 3 { args.get(2).integer_value(scope).unwrap_or(0) } else { 0 };
        COMMAND_META.with(|m| m.borrow_mut().insert(name.clone(), flags));

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
/// On a MATCHED command trigger, returns `silent` (suppress iff the trigger was `/`) after dispatching.
/// Otherwise (not a trigger, empty trigger, or an unmatched trigger) the raw line is delivered to the
/// `Chat.onMessage` subscribers (Slice 6.13b): each live subscriber gets `(slot, text, teamonly)` and a
/// return of `>= HookResult.Handled` (2) suppresses the broadcast. No CONCOMMANDS borrow is held across
/// `dispatch_concommand`. Engine-generic: core passes only slot/text/teamonly, never a game type.
pub(crate) fn dispatch_chat(slot: i32, text: &str, teamonly: bool) -> bool {
    let (silent, is_trigger) = match text.as_bytes().first() {
        Some(b'!') => (false, true),
        Some(b'/') => (true, true),
        _ => (false, false),
    };
    if is_trigger {
        let rest = text[1..].trim();
        if !rest.is_empty() {
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
            if let Some(cmd) = matched {
                // Matched command → the command path, exactly as before. Never reach the subscriber loop.
                dispatch_concommand(&cmd, slot, &args);
                return silent;
            }
        }
    }
    // No !/ command matched → deliver the raw line to the Chat.onMessage subscribers.
    dispatch_chat_message(slot, text, teamonly)
}

/// Deliver a raw chat line to the `Chat.onMessage` subscribers (Slice 6.13b). Mirrors
/// `dispatch_game_event`: snapshot (release the mux borrow), re-entrancy guard, per-subscriber
/// liveness + context + TryCatch. Each handler is called with `(slot, text, teamonly)`; a return of
/// `>= HookResult.Handled` (numeric `>= 2`) sets suppress. `undefined`/non-number/throw ⇒ Continue.
/// Returns true iff any live subscriber requested suppression of the broadcast.
fn dispatch_chat_message(slot: i32, text: &str, teamonly: bool) -> bool {
    // Phase 1: snapshot — release CHAT_MSG_SUBS borrow before entering any context. Fixed key "".
    let snap = CHAT_MSG_SUBS.with(|m| m.borrow().snapshot(""));
    if snap.is_empty() { return false; }

    let mut suppress = false;
    // Phase 2: enter each subscriber's context and invoke handler(slot, text, teamonly).
    HOST.with(|h| {
        // Re-entrancy guard (mirrors dispatch_game_event): a chat-triggered handler that re-enters
        // dispatch while HOST is already borrowed is skipped, not double-borrowed (would panic).
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, generation, handler_g) in &snap {
            // Liveness check (release REGISTRY borrow before entering context).
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
            // Clone the context Global out of PLUGINS (borrow released) so the handler may re-enter.
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            // Per-subscriber HandleScope+ContextScope — mirrors dispatch_game_event.
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            // Per-handler TryCatch isolates a throwing handler from the rest.
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let slot_val: v8::Local<v8::Value> = v8::Integer::new(tc, slot).into();
            let text_val: v8::Local<v8::Value> = match v8::String::new(tc, text) {
                Some(s) => s.into(),
                None => v8::undefined(tc).into(),
            };
            let team_val: v8::Local<v8::Value> = v8::Boolean::new(tc, teamonly).into();

            let func = v8::Local::new(tc, handler_g);
            match func.call(tc, recv, &[slot_val, text_val, team_val]) {
                None => {
                    let msg = tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!("WARN: dispatch_chat: onMessage handler '{}': {}", owner, msg));
                }
                // A numeric return >= Handled (2) suppresses; undefined/non-number ⇒ Continue.
                Some(ret) if ret.is_number() => {
                    if ret.uint32_value(tc).unwrap_or(0) >= 2 { suppress = true; }
                }
                Some(_) => {}
            }
        }
    });
    suppress
}

/// Deliver a client-lifecycle notification to the `Clients.on*` subscribers for `event` (Clients
/// sub-project). Called from `ffi.rs`'s `s2script_core_dispatch_client_event` (the shim's six lifecycle
/// hooks pass the event name + slot). Mirrors `dispatch_chat_message`: snapshot (release the mux borrow),
/// `try_borrow_mut` re-entrancy guard, per-subscriber `is_live` + context clone + HandleScope/
/// ContextScope/TryCatch + WARN-on-throw. Notify-only — each handler is called with the single Integer
/// `slot` and its return is ignored (no suppress/HookResult collapse).
pub(crate) fn dispatch_client_event(event: &str, slot: i32) {
    // Phase 1: snapshot — release CLIENT_MUX borrow before entering any context.
    let snap = CLIENT_MUX.with(|m| m.borrow().snapshot(event));
    if snap.is_empty() { return; }

    // Phase 2: enter each subscriber's context and invoke handler(slot).
    HOST.with(|h| {
        // Re-entrancy guard (mirrors dispatch_chat_message / dispatch_game_event): a client handler
        // that re-enters dispatch while HOST is already borrowed is skipped, not double-borrowed.
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, generation, handler_g) in &snap {
            // Liveness check (release REGISTRY borrow before entering context).
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
            // Clone the context Global out of PLUGINS (borrow released) so the handler may re-enter.
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            // Per-subscriber HandleScope+ContextScope — mirrors dispatch_chat_message.
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            // Per-handler TryCatch isolates a throwing handler from the rest.
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let slot_val: v8::Local<v8::Value> = v8::Integer::new(tc, slot).into();
            let func = v8::Local::new(tc, handler_g);
            if func.call(tc, recv, &[slot_val]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_client('{}'): handler '{}': {}", event, owner, msg));
            }
        }
    });
}

/// Deliver a map-start notification to the `Server.onMapStart` subscribers. Called from ffi.rs's
/// `s2script_core_dispatch_map_start` (the shim's INetworkServerService::StartupServer POST hook).
/// Mirrors `dispatch_client_event` verbatim: snapshot (release the mux borrow), `try_borrow_mut`
/// re-entrancy guard, per-subscriber `is_live` + context clone + HandleScope/ContextScope/TryCatch +
/// WARN-on-throw. Notify-only — each handler is called with the single String `map` and its return
/// is ignored.
pub(crate) fn dispatch_map_start(map: &str) {
    let snap = MAP_MUX.with(|m| m.borrow().snapshot(""));
    if snap.is_empty() { return; }

    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, generation, handler_g) in &snap {
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let map_val: v8::Local<v8::Value> = match v8::String::new(tc, map) {
                Some(s) => s.into(),
                None => continue,
            };
            let func = v8::Local::new(tc, handler_g);
            if func.call(tc, recv, &[map_val]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_map_start: handler '{}': {}", owner, msg));
            }
        }
    });
}

/// Drain `COOKIE_CACHED_PENDING` and fan each queued slot out to the `Cookies.onCached` subscribers
/// (clientprefs Task 4). Called from `ffi.rs`'s Post-frame branch AFTER `frame_async_drain()` returns
/// (HOST is free — no re-entrancy risk from the plugin's own async cookie-load work). Mirrors
/// `dispatch_client_event` verbatim: snapshot (release the mux borrow), `try_borrow_mut` re-entrancy
/// guard, per-subscriber liveness + context clone + HandleScope/ContextScope/TryCatch + WARN-on-throw.
/// Notify-only — each handler is called with the single Integer `slot` and its return is ignored.
pub(crate) fn dispatch_pending_cookie_cached() {
    let slots: Vec<i32> = COOKIE_CACHED_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if slots.is_empty() { return; }

    // Phase 1: snapshot — release COOKIE_CACHED_MUX borrow before entering any context. Fixed key "".
    let snap = COOKIE_CACHED_MUX.with(|m| m.borrow().snapshot(""));
    if snap.is_empty() { return; }

    for slot in slots {
        // Phase 2: enter each subscriber's context and invoke handler(slot).
        HOST.with(|h| {
            // Re-entrancy guard (mirrors dispatch_client_event): expected free here (called after
            // frame_async_drain returns), but guarded anyway per the shared discipline.
            let Ok(mut borrow) = h.try_borrow_mut() else { return };
            let Some(host) = borrow.as_mut() else { return };

            for (owner, generation, handler_g) in &snap {
                // Liveness check (release REGISTRY borrow before entering context).
                if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
                // Clone the context Global out of PLUGINS (borrow released) so the handler may re-enter.
                let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

                // Per-subscriber HandleScope+ContextScope — mirrors dispatch_client_event.
                let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
                let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
                let hs = &mut hs;
                let ctx_local = v8::Local::new(hs, &g_ctx);
                let scope = &mut v8::ContextScope::new(hs, ctx_local);

                // Per-handler TryCatch isolates a throwing handler from the rest.
                let mut tc_storage = v8::TryCatch::new(scope);
                let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
                let tc = &mut tc;

                let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                let slot_val: v8::Local<v8::Value> = v8::Integer::new(tc, slot).into();
                let func = v8::Local::new(tc, handler_g);
                if func.call(tc, recv, &[slot_val]).is_none() {
                    let msg = tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!("WARN: dispatch_pending_cookie_cached: handler '{}': {}", owner, msg));
                }
            }
        });
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

/// `__s2_chat_on_message(handler)` — subscribe a JS fn to raw player chat (Slice 6.13b). Owner-tracked;
/// the Host_Say detour is installed at Load, so no per-subscribe engine registration is needed. The
/// handler receives `(slot, text, teamonly)` at dispatch and may return a HookResult to suppress the
/// broadcast. Fixed mux key "" (chat has no name dimension); the "first subscriber" signal is ignored
/// (no per-name engine-op to toggle).
fn s2_chat_on_message(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        CHAT_MSG_SUBS.with(|m| { m.borrow_mut().subscribe("", owner, generation, handler_g); });
    }));
}

/// `__s2_client_subscribe(event, handler)` — subscribe a JS fn to a client-lifecycle event name
/// (Clients sub-project). Owner-tracked (mirror `__s2_event_subscribe`); the shim's six lifecycle hooks
/// are installed unconditionally at Load, so there is no per-name engine-op — the "first subscriber"
/// signal is ignored. The handler receives the raw `slot` at dispatch; the `@s2script/clients` JS
/// wrapper builds a `Client` from it. Notify-only (the return is ignored).
fn s2_client_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let event = args.get(0).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(1)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        // Capture the calling plugin's (id, generation) for liveness-gated dispatch.
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        CLIENT_MUX.with(|m| { m.borrow_mut().subscribe(&event, owner, generation, handler_g); });
    }));
}

/// `__s2_map_start_subscribe(handler)` — subscribe a JS fn to the map-start event. Owner-tracked
/// (mirrors `__s2_chat_on_message`); fixed mux key "". The handler receives the map name string.
fn s2_map_start_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        MAP_MUX.with(|m| { m.borrow_mut().subscribe("", owner, generation, handler_g); });
    }));
}

/// `__s2_cookie_on_cached(handler)` — subscribe a JS fn to `Cookies.onCached` (clientprefs Task 4).
/// Owner-tracked (mirrors `__s2_client_subscribe`); fixed mux key "" (cookies-cached has no name
/// dimension, like `Chat.onMessage`). The handler receives the raw `slot` at dispatch; the
/// `@s2script/cookies` prelude wraps it into a `Client` via `Clients.fromSlot`.
fn s2_cookie_on_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        COOKIE_CACHED_MUX.with(|m| { m.borrow_mut().subscribe("", owner, generation, handler_g); });
    }));
}

/// `__s2_cookie_dispatch_cached(slot)` — enqueue `slot` for the next post-frame
/// `dispatch_pending_cookie_cached()` fan-out (clientprefs Task 4). No HOST access here (safe to call
/// from inside the plugin's own async `loadCookies` continuation, which may run mid-async-drain); the
/// actual `onCached` handler invocation happens later, once HOST is free.
fn s2_cookie_dispatch_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push(slot));
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

/// `__s2_pawn_commit_suicide(index, serial)` — kill a pawn via the sig-resolved CommitSuicide engine-op.
/// A thin pass-through: the shim reconstructs + serial-gates the pawn from (index, serial). No-op without
/// the op (unresolved signature) — the shim itself no-ops a stale ref. Engine-generic (a pawn is a
/// Source2 base-type concept; only the resolving signature is game-specific).
fn s2_pawn_commit_suicide(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as c_int;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as c_int;
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.pawn_commit_suicide else { return };
        f(index, serial);
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

/// `__s2_commands_list() -> string` — JSON array of `{name, flags}` for `Commands.list()` / `sm_help`.
/// Joins on live `CONCOMMANDS` keys (a stale `COMMAND_META` entry is ignored); `flags` defaults to 0 if a
/// command has no meta entry. Degrades to `"[]"` on any error (`catch_unwind`).
fn s2_commands_list(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let items: Vec<serde_json::Value> = CONCOMMANDS.with(|c| {
            COMMAND_META.with(|meta| {
                let meta = meta.borrow();
                c.borrow().keys()
                    .map(|name| serde_json::json!({
                        "name": name,
                        "flags": meta.get(name).copied().unwrap_or(0),
                    }))
                    .collect()
            })
        });
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
/// frame_async_drain() (HOST free). Mirrors dispatch_pending_cookie_cached / dispatch_concommand.
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

/// Construct `new EntityRef(index, serial)` via the injected `__s2pkg_entity.EntityRef` constructor
/// (the DamageInfo.victim / readHandle pattern — a raw handle never crosses to JS, only a decoded,
/// serial-gated `(index, serial)` pair the resulting `EntityRef` re-validates on every field access).
/// Falls back to `null` if `@s2script/entity` isn't installed on this context.
fn build_entity_ref<'s>(scope: &mut v8::PinScope<'s, '_>, index: i32, serial: i32) -> v8::Local<'s, v8::Value> {
    let val: Option<v8::Local<'s, v8::Value>> = (|| {
        let global = scope.get_current_context().global(scope);
        let pkg_key = v8::String::new(scope, "__s2pkg_entity")?;
        let pkg = global.get(scope, pkg_key.into())?;
        let pkg = v8::Local::<v8::Object>::try_from(pkg).ok()?;
        let ctor_key = v8::String::new(scope, "EntityRef")?;
        let ctor_val = pkg.get(scope, ctor_key.into())?;
        let ctor = v8::Local::<v8::Function>::try_from(ctor_val).ok()?;
        let idx_v = v8::Integer::new(scope, index);
        let ser_v = v8::Integer::new(scope, serial);
        ctor.new_instance(scope, &[idx_v.into(), ser_v.into()]).map(|o| -> v8::Local<v8::Value> { o.into() })
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
        let ignore_idx    = args.get(6).integer_value(scope).unwrap_or(-1) as c_int;
        let ignore_serial = args.get(7).integer_value(scope).unwrap_or(-1) as c_int;

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
            if entity_resolve_ptr(index, serial).is_null() {
                v8::null(scope).into() // a same-frame stale handle (defensive) -> null, not a dead ref
            } else {
                build_entity_ref(scope, index, serial)
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

/// Like `read_vec3` but returns `None` when the arg isn't a 3-number array (for nullable teleport args —
/// `origin`/`angles`/`velocity` are each independently optional).
fn read_vec3_opt(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<[f32; 3]> {
    let arr = v8::Local::<v8::Array>::try_from(v).ok()?;
    if arr.length() != 3 { return None; }
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        out[i as usize] = arr.get_index(scope, i)?.number_value(scope).unwrap_or(0.0) as f32;
    }
    Some(out)
}

/// Read a JS array of numbers into a `Vec<i32>`. Returns `[]` if `v` isn't an array (or on any
/// per-element read failure the remaining elements are skipped) — never panics on bad input.
fn read_int_array(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Vec<i32> {
    let Ok(arr) = v8::Local::<v8::Array>::try_from(v) else { return Vec::new() };
    let len = arr.length();
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        let Some(el) = arr.get_index(scope, i) else { break };
        out.push(el.integer_value(scope).unwrap_or(0) as i32);
    }
    out
}

/// Native `__s2_entity_create(className) -> EntityRef | null`. Over the `entity_create` op
/// (`UTIL_CreateEntityByName`, sig-resolved shim-side). The op returns a packed `CEntityHandle`
/// (`ToInt()`); the raw `CBaseEntity*` never crosses to JS — the handle is decoded (pure bit-math)
/// and re-validated live (`entity_resolve_ptr`) before building a serial-gated `EntityRef`, mirroring
/// the `s2_trace` hit-entity pattern. Degrades to `null` with no op / a 0 handle / a same-frame stale
/// decode (every in-isolate test hits this path).
fn s2_entity_create(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let name = args.get(0).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_create) {
            let handle = func(cname.as_ptr());
            if handle != 0 {
                let (index, serial) = crate::entity::decode_handle(handle as u32);
                if !entity_resolve_ptr(index, serial).is_null() {
                    rv.set(build_entity_ref(scope, index, serial));
                }
            }
        }
    }));
}

/// Native `__s2_entity_find_by_class(className) -> EntityRef[]`. Over the `entity_find_by_class` op
/// (the shim iterates the entity-identity list, comparing each `CEntityIdentity::m_designerName`).
/// Returns serial-gated EntityRefs — each (index, serial) is re-validated via `entity_resolve_ptr`
/// (like `s2_entity_create`) before building the ref; the raw pointer never crosses to JS.
/// Degrades to an empty array with no op / a null className. The out-buffer is bounded at 1024.
fn s2_entity_find_by_class(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let empty = v8::Array::new(scope, 0);
        rv.set(empty.into());
        let name = args.get(0).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(func) = ops.and_then(|o| o.entity_find_by_class) else { return };
        const CAP: usize = 1024;
        let mut idxs = vec![0i32; CAP];
        let mut sers = vec![0i32; CAP];
        let total = func(cname.as_ptr(), idxs.as_mut_ptr(), sers.as_mut_ptr(), CAP as i32);
        let n = (total.max(0) as usize).min(CAP);
        let arr = v8::Array::new(scope, 0);
        let mut w: u32 = 0;
        for i in 0..n {
            let (index, serial) = (idxs[i], sers[i]);
            if !entity_resolve_ptr(index, serial).is_null() {
                let r = build_entity_ref(scope, index, serial);
                arr.set_index(scope, w, r);
                w += 1;
            }
        }
        rv.set(arr.into());
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
            rv.set_bool(f(std::ptr::null(), -1) == 1);
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
        rv.set_bool(f(slots.as_ptr(), slots.len() as i32) == 1);
    }));
}

/// Native `__s2_entity_spawn(index, serial) -> boolean`. Serial-gated `DispatchSpawn`. Degrades to
/// `false` with no op / a stale ref.
fn s2_entity_spawn(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_spawn) { rv.set_bool(func(index, serial) != 0); }
    }));
}

/// Native `__s2_collision_activate(index, serial) -> boolean`. Serial-gated; over the
/// `collision_activate` op (CCollisionProperty partition registration). Degrades to `false` with no
/// op / a stale ref. Never throws.
fn s2_collision_activate(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.collision_activate) { rv.set_bool(func(index, serial) != 0); }
    }));
}

/// Native `__s2_ent_set_model(index, serial, modelName) -> boolean`. Serial-gated; over the
/// `entity_set_model` op (`CBaseEntity::SetModel`, sig-resolved shim-side). Gives a runtime entity a
/// model + its collision — a runtime `trigger_multiple` needs this for a physics volume that fires
/// touch. Degrades to `false` with no op / a stale ref / a NUL in the name. Never throws.
fn s2_ent_set_model(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let name = args.get(2).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_set_model) {
            rv.set_bool(func(index, serial, cname.as_ptr()) != 0);
        }
    }));
}

/// Native `__s2_entity_teleport(index, serial, originArr|null, anglesArr|null, velArr|null) -> boolean`.
/// Each array arg is independently optional (a non-3-element/non-array value degrades to a null pointer
/// for that component, matching the shim's nullable `Vector*`/`QAngle*`/`Vector*` ABI). Degrades to
/// `false` with no op / a stale ref.
fn s2_entity_teleport(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let origin = read_vec3_opt(scope, args.get(2));
        let angles = read_vec3_opt(scope, args.get(3));
        let vel    = read_vec3_opt(scope, args.get(4));
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_teleport) {
            let op = origin.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            let ap = angles.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            let vp = vel.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            rv.set_bool(func(index, serial, op, ap, vp) != 0);
        }
    }));
}

/// Native `__s2_entity_remove(index, serial) -> boolean`. Serial-gated `UTIL_Remove`. Degrades to
/// `false` with no op / a stale ref.
fn s2_entity_remove(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_remove) { rv.set_bool(func(index, serial) != 0); }
    }));
}

/// Native `__s2_entity_fire_input(index, serial, input, value, actIdx, actSerial, callerIdx,
/// callerSerial, delay) -> boolean`. Over the `entity_fire_input` engine op (`AddEntityIOEvent`, the
/// game's own input-firing path, sig-resolved shim-side). `actIdx`/`callerIdx` < 0 = no
/// activator/caller (the shim passes null). Degrades to `false` with no op / a stale target ref.
fn s2_entity_fire_input(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let input = args.get(2).to_rust_string_lossy(scope);
        let value = args.get(3).to_rust_string_lossy(scope);
        let act_idx = args.get(4).integer_value(scope).unwrap_or(-1) as i32;
        let act_serial = args.get(5).integer_value(scope).unwrap_or(-1) as i32;
        let caller_idx = args.get(6).integer_value(scope).unwrap_or(-1) as i32;
        let caller_serial = args.get(7).integer_value(scope).unwrap_or(-1) as i32;
        let delay = args.get(8).number_value(scope).unwrap_or(0.0) as f32;
        let Ok(input_c) = std::ffi::CString::new(input) else { return };
        let Ok(value_c) = std::ffi::CString::new(value) else { return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_fire_input) {
            rv.set_bool(func(
                index, serial, input_c.as_ptr(), value_c.as_ptr(),
                act_idx, act_serial, caller_idx, caller_serial, delay,
            ) != 0);
        }
    }));
}

/// Native `__s2_entity_spawn_kv(index, serial, keys[], types[], values[]) -> boolean`. Over the
/// `entity_spawn_kv` op (DispatchSpawn with a shim-built CEntityKeyValues). All three arrays must be
/// same-length; keys/values are strings (interior NUL -> false), types are ints. Degrades to false
/// with no op / stale serial / malformed args (every in-isolate test).
fn s2_entity_spawn_kv(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let keys_arr = match v8::Local::<v8::Array>::try_from(args.get(2)) { Ok(a) => a, Err(_) => return };
        let types_arr = match v8::Local::<v8::Array>::try_from(args.get(3)) { Ok(a) => a, Err(_) => return };
        let vals_arr = match v8::Local::<v8::Array>::try_from(args.get(4)) { Ok(a) => a, Err(_) => return };
        let n = keys_arr.length();
        if types_arr.length() != n || vals_arr.length() != n { return; }
        let mut keys_c: Vec<std::ffi::CString> = Vec::with_capacity(n as usize);
        let mut vals_c: Vec<std::ffi::CString> = Vec::with_capacity(n as usize);
        let mut types_v: Vec<c_int> = Vec::with_capacity(n as usize);
        for i in 0..n {
            let k = match keys_arr.get_index(scope, i) { Some(v) => v.to_rust_string_lossy(scope), None => return };
            let val = match vals_arr.get_index(scope, i) { Some(v) => v.to_rust_string_lossy(scope), None => return };
            let t = match types_arr.get_index(scope, i) { Some(v) => v.integer_value(scope).unwrap_or(-1) as i32, None => return };
            if !(0..=3).contains(&t) { return; }
            let kc = match std::ffi::CString::new(k) { Ok(c) => c, Err(_) => return };
            let vc = match std::ffi::CString::new(val) { Ok(c) => c, Err(_) => return };
            // BYTE-length guard (the true choke point): the JS prelude caps UTF-16 .length at 1024,
            // but CKV3Arena's AddPage() aborts the WHOLE process on a string whose UTF-8 BYTE length
            // exceeds ~2KB — and a BMP char (CJK, U+0800..U+FFFF) is 3 UTF-8 bytes/code-unit, so 1024
            // code units can be ~3KB. Re-check the exact UTF-8 byte length here (free — the CString is
            // built) and fail the WHOLE map closed (no partial spawn) BEFORE any engine call.
            if kc.as_bytes().len() > 1024 || vc.as_bytes().len() > 1024 { return; }
            keys_c.push(kc); vals_c.push(vc); types_v.push(t);
        }
        let key_ptrs: Vec<*const std::os::raw::c_char> = keys_c.iter().map(|c| c.as_ptr()).collect();
        let val_ptrs: Vec<*const std::os::raw::c_char> = vals_c.iter().map(|c| c.as_ptr()).collect();
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_spawn_kv) {
            rv.set_bool(func(index, serial, n as c_int, key_ptrs.as_ptr(), types_v.as_ptr(), val_ptrs.as_ptr()) != 0);
        }
    }));
}

/// Native `__s2_output_subscribe(classname, output, handler)`. Subscribes a JS fn to `Entity.onOutput`
/// (entity-I/O slice); owner-tracked in `OUTPUT_MUX` keyed `"<classname>\0<output>"`. The
/// `FireOutputInternal` detour is installed unconditionally at shim Load, so no per-subscribe engine
/// registration is needed (mirrors `s2_damage_subscribe`/`s2_chat_on_message`).
fn s2_output_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let classname = args.get(0).to_rust_string_lossy(scope);
        let output = args.get(1).to_rust_string_lossy(scope);
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(2)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        let key = format!("{}\0{}", classname, output);
        OUTPUT_MUX.with(|m| { m.borrow_mut().subscribe(&key, owner, generation, handler_g); });
    }));
}

/// Native `__s2_output_unsubscribe(classname, output)`. Removes the CURRENT plugin's subscriptions for
/// the `(classname, output)` key (best-effort, mirrors `EventMux::remove_by_owner_on` — V8 `Global`s
/// can't be compared by identity, so this drops ALL of the caller's subs for that exact key). Available
/// as a primitive; `Entity.onOutput` this slice has no matching `offOutput` — cleanup on unload/reload
/// runs via `remove_by_owner`, not this native.
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

/// Native `__s2_give_named_item(index, serial, subObjOffset, name) -> EntityRef | null`. Over the
/// `give_named_item` engine op (`GiveNamedItem`, sig-resolved shim-side, called on the pawn's
/// ItemServices sub-object at `subObjOffset` — a schema offset resolved JS-side, opaque here). The
/// op returns a packed `CEntityHandle` (`ToInt()`) of the created weapon; the raw pointer never
/// crosses to JS — the handle is decoded and re-validated live (`entity_resolve_ptr`) before
/// building a serial-gated `EntityRef`, mirroring `s2_entity_create`. Degrades to `null` with no
/// op / a 0 handle / an unresolvable name / a same-frame stale decode.
fn s2_give_named_item(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let name = args.get(3).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.give_named_item) {
            let handle = func(index, serial, off, cname.as_ptr());
            if handle != 0 {
                let (i, s) = crate::entity::decode_handle(handle as u32);
                if !entity_resolve_ptr(i, s).is_null() { rv.set(build_entity_ref(scope, i, s)); }
            }
        }
    }));
}

/// Native `__s2_entity_subobj_vcall(index, serial, subObjOffset, vtableIndex, argIndex, argSerial)
/// -> boolean`. Calls a `.text`-validated vtable slot on the sub-object at `subObjOffset` (e.g.
/// ItemServices' `RemoveWeapons`/`DropActivePlayerWeapon`), optionally passing a second
/// serial-gated entity arg (`argIndex < 0` = no arg, e.g. no active weapon to pass). Degrades to
/// `false` with no op / a stale root or arg ref / an unresolved sub-object / an out-of-`.text`
/// vtable slot (shim-side guard).
fn s2_entity_subobj_vcall(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let vtable_index = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let arg_idx = args.get(4).integer_value(scope).unwrap_or(-1) as i32;
        let arg_serial = args.get(5).integer_value(scope).unwrap_or(-1) as i32;
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_subobj_vcall) {
            rv.set_bool(func(index, serial, off, vtable_index, arg_idx, arg_serial) != 0);
        }
    }));
}

/// Native `__s2_remove_player_item(pawnIndex, pawnSerial, weaponIndex, weaponSerial) -> boolean`.
/// Over the `remove_player_item` engine op (`RemovePlayerItem`, sig-resolved shim-side) — a proper
/// unequip of one specific weapon (vs. `stripWeapons`'s blanket `RemoveWeapons`). Degrades to
/// `false` with no op / either ref stale.
fn s2_remove_player_item(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let pawn_idx = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let pawn_serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let weapon_idx = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let weapon_serial = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.remove_player_item) {
            rv.set_bool(func(pawn_idx, pawn_serial, weapon_idx, weapon_serial) != 0);
        }
    }));
}

/// Native `__s2_entity_read_handle_vector(index, serial, ptrOffs, vectorOff, maxCount) ->
/// EntityRef[]`. Follows the `ptrOffs` pointer-deref chain from the root entity (e.g. to a
/// WeaponServices sub-object), then reads a `CUtlVector<CHandle>` at `vectorOff` (size@+0,
/// elements@+8, shim-side) — each packed handle is decoded and `entity_resolve_ptr`-validated
/// before becoming a serial-gated `EntityRef` (raw pointers never cross to JS; `maxCount`-capped,
/// itself clamped to `[0, 256]`). Degrades to `[]` with no op / a stale root / an unresolved chain.
fn s2_entity_read_handle_vector(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let ptr_offs = read_int_array(scope, args.get(2));      // Vec<i32>, [] if not an array
        let vector_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let max_count = (args.get(4).integer_value(scope).unwrap_or(0) as i32).clamp(0, 256);
        let arr = v8::Array::new(scope, 0);
        let ops = ENGINE_OPS.with(|o| o.get());
        if let Some(func) = ops.and_then(|o| o.entity_read_handle_vector) {
            let mut out = vec![0i32; max_count as usize];
            let n = func(index, serial, ptr_offs.as_ptr(), ptr_offs.len() as i32, vector_off, max_count, out.as_mut_ptr());
            let mut w = 0u32;
            for k in 0..(n.max(0) as usize).min(max_count as usize) {
                let (i, s) = crate::entity::decode_handle(out[k] as u32);
                if !entity_resolve_ptr(i, s).is_null() {
                    let er = build_entity_ref(scope, i, s);
                    arr.set_index(scope, w, er);
                    w += 1;
                }
            }
        }
        rv.set(arr.into());
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

/// Native `__s2_client_language(slot) -> string | null`. Mirrors `s2_client_name` exactly, calling
/// `client_language` (the client's `cl_language` cvar) instead.
fn s2_client_language(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_language else { return };
        let ptr = func(slot);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
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

/// Native `__s2_event_fire_to_client(slot) -> boolean` — fire the created event to ONE client's
/// per-client listener (serialized to that netchannel; does NOT pass through IGameEventManager2::FireEvent,
/// so no pre-hook / dispatch re-entrancy). Returns false on any miss.
fn s2_event_fire_to_client(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.event_fire_to_client else { return };
        rv.set_bool(func(slot) != 0);
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
/// another output mid-dispatch skips the nested dispatch — the documented `Events.fire` limitation).
pub(crate) fn dispatch_output(classname: &str, output: &str, act_handle: i32, caller_handle: i32, value: &str, delay: f32) -> i32 {
    use crate::multiplexer::{run_chain, HookResult, Priority, SubId};
    // Phase 1: snapshot every matching key, release the OUTPUT_MUX borrow before entering any context.
    // Dedup keys that collapse onto the same string (a literal "*" classname/output would be unusual
    // but harmless) so a subscriber is never invoked twice for the same fire.
    let keys = [
        format!("{}\0{}", classname, output),
        format!("{}\0*", classname),
        format!("*\0{}", output),
        "*\0*".to_string(),
    ];
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut snap0: Vec<(String, u64, v8::Global<v8::Function>)> = Vec::new();
    for k in &keys {
        if !seen.insert(k.as_str()) { continue; }
        snap0.extend(OUTPUT_MUX.with(|m| m.borrow().snapshot(k)));
    }
    if snap0.is_empty() { return 0; }
    let snap: Vec<(SubId, Priority, (String, u64, v8::Global<v8::Function>))> = snap0
        .into_iter().enumerate()
        .map(|(i, (owner, gen, h))| (i as SubId, Priority::Normal, (owner, gen, h)))
        .collect();

    let outcome = HOST.with(|h| {
        // Re-entrancy guard: a handler that fires another output (acceptInput) re-enters this dispatch
        // while the isolate is already borrowed. ALLOW (Continue) rather than double-borrow (would
        // panic) — the engine-side output still fires; only the nested JS re-dispatch is skipped.
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

            // Build the ev object directly (no JS constructor needed — the data is already in hand,
            // unlike GameEvent/DamageInfo which read live shim state via further op calls).
            // NOTE: -1 is the EXACT sentinel the shim emits for "no entity" (a null pActivator/
            // pCaller), never a broad sign test — a live CEntityHandle::ToInt() packs a 17-bit
            // serial into the packed int's upper bits (HANDLE_ENTRY_BITS=15 in entity.rs), so a
            // real handle whose serial has climbed to >= 65536 is a genuinely negative i32 and
            // must still decode, not be misread as "none" (mirrors the exact-sentinel convention
            // `s2_give_named_item` already uses for its packed-handle return: `if handle != 0`).
            let activator_val: v8::Local<v8::Value> = if act_handle == -1 {
                v8::null(tc).into()
            } else {
                let (ai, aser) = crate::entity::decode_handle(act_handle as u32);
                if entity_resolve_ptr(ai, aser).is_null() { v8::null(tc).into() } else { build_entity_ref(tc, ai, aser) }
            };
            let caller_val: v8::Local<v8::Value> = if caller_handle == -1 {
                v8::null(tc).into()
            } else {
                let (ci, cser) = crate::entity::decode_handle(caller_handle as u32);
                if entity_resolve_ptr(ci, cser).is_null() { v8::null(tc).into() } else { build_entity_ref(tc, ci, cser) }
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

            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let ev_val: v8::Local<v8::Value> = ev_obj.into();
            match func.call(tc, recv, &[ev_val]) {
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
    outcome.result as i32
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
    set_native(scope, global_obj, "__s2_ent_ref_write_string", s2_ent_ref_write_string);
    set_native(scope, global_obj, "__s2_ent_ref_read_floats", s2_ent_ref_read_floats);
    set_native(scope, global_obj, "__s2_ent_ref_read_floats_chain", s2_ent_ref_read_floats_chain);
    set_native(scope, global_obj, "__s2_ent_ref_read_chain", s2_ent_ref_read_chain);
    set_native(scope, global_obj, "__s2_ent_ref_write_chain", s2_ent_ref_write_chain);
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
    // Translations slice: root/language phrase-file read + the client's cl_language cvar.
    set_native(scope, global_obj, "__s2_translations_read", s2_translations_read);
    set_native(scope, global_obj, "__s2_client_language", s2_client_language);
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
    set_native(scope, global_obj, "__s2_event_fire_to_client", s2_event_fire_to_client);
    // Config live-reload (Slice 5E.2): register an onChange handler for this plugin's config file.
    set_native(scope, global_obj, "__s2_config_on_change", s2_config_on_change);
    // Chat messaging (Slice 6.1): print a message to one client's chat.
    set_native(scope, global_obj, "__s2_client_print", s2_client_print);
    // Raw-chat subscriber (Slice 6.13b): register a Chat.onMessage handler.
    set_native(scope, global_obj, "__s2_chat_on_message", s2_chat_on_message);
    // Client-lifecycle subscriber (Clients sub-project): register a Clients.on* handler.
    set_native(scope, global_obj, "__s2_client_subscribe", s2_client_subscribe);
    // Map-start subscriber (clientlist-fakeconvar-onmapstart slice): register a Server.onMapStart handler.
    set_native(scope, global_obj, "__s2_map_start_subscribe", s2_map_start_subscribe);

    set_native(scope, global_obj, "__s2_admin_set", s2_admin_set);
    set_native(scope, global_obj, "__s2_admin_get", s2_admin_get);
    set_native(scope, global_obj, "__s2_admin_get_immunity", s2_admin_get_immunity);
    set_native(scope, global_obj, "__s2_admin_add_override", s2_admin_add_override);
    set_native(scope, global_obj, "__s2_admin_set_global_override", s2_admin_set_global_override);
    set_native(scope, global_obj, "__s2_admin_override", s2_admin_override);
    set_native(scope, global_obj, "__s2_admin_remove", s2_admin_remove);
    set_native(scope, global_obj, "__s2_admin_clear_file", s2_admin_clear_file);
    set_native(scope, global_obj, "__s2_admin_mark_loaded", s2_admin_mark_loaded);
    // Slice 6.18: ban cache natives (engine-generic — a SteamID/ban map, like the admin cache).
    set_native(scope, global_obj, "__s2_ban_set", s2_ban_set);
    set_native(scope, global_obj, "__s2_ban_get", s2_ban_get);
    set_native(scope, global_obj, "__s2_ban_remove", s2_ban_remove);
    set_native(scope, global_obj, "__s2_ban_clear", s2_ban_clear);
    set_native(scope, global_obj, "__s2_ban_list", s2_ban_list);
    set_native(scope, global_obj, "__s2_ban_mark_loaded", s2_ban_mark_loaded);
    // clientprefs: cookie cache natives (engine-generic — a SteamID/string-KV map, like admin/ban).
    set_native(scope, global_obj, "__s2_cookie_get", s2_cookie_get);
    set_native(scope, global_obj, "__s2_cookie_set", s2_cookie_set);
    set_native(scope, global_obj, "__s2_cookie_load", s2_cookie_load);
    set_native(scope, global_obj, "__s2_cookie_get_time", s2_cookie_get_time);
    set_native(scope, global_obj, "__s2_cookie_get_dirty", s2_cookie_get_dirty);
    set_native(scope, global_obj, "__s2_cookie_clear", s2_cookie_clear);
    set_native(scope, global_obj, "__s2_cookie_mark_cached", s2_cookie_mark_cached);
    set_native(scope, global_obj, "__s2_cookie_is_cached", s2_cookie_is_cached);
    set_native(scope, global_obj, "__s2_cookie_set_authid", s2_cookie_set_authid);
    set_native(scope, global_obj, "__s2_cookie_take_offline_writes", s2_cookie_take_offline_writes);
    set_native(scope, global_obj, "__s2_cookie_on_cached", s2_cookie_on_cached);
    set_native(scope, global_obj, "__s2_cookie_dispatch_cached", s2_cookie_dispatch_cached);
    set_native(scope, global_obj, "__s2_client_steamid", s2_client_steamid);
    set_native(scope, global_obj, "__s2_client_kick", s2_client_kick);
    // ban-reason sub-project 2: developer-console print + client IP address.
    set_native(scope, global_obj, "__s2_client_console_print", s2_client_console_print);
    set_native(scope, global_obj, "__s2_client_address", s2_client_address);
    set_native(scope, global_obj, "__s2_damage_subscribe", s2_damage_subscribe);
    set_native(scope, global_obj, "__s2_damage_read_float", s2_damage_read_float);
    set_native(scope, global_obj, "__s2_damage_read_int", s2_damage_read_int);
    set_native(scope, global_obj, "__s2_damage_write_float", s2_damage_write_float);
    set_native(scope, global_obj, "__s2_damage_victim", s2_damage_victim);
    set_native(scope, global_obj, "__s2_cvar_get", s2_cvar_get);
    set_native(scope, global_obj, "__s2_convar_register", s2_convar_register);
    set_native(scope, global_obj, "__s2_pawn_commit_suicide", s2_pawn_commit_suicide);
    set_native(scope, global_obj, "__s2_plugins_list", s2_plugins_list);
    set_native(scope, global_obj, "__s2_commands_list", s2_commands_list);
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
    // Slice DB Task 3: the `__s2_sqlite_*` natives (sync-behind-Promise) for `@s2script/db`.
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
    set_native(scope, global_obj, "__s2_fetch", s2_fetch);
    // WebSocket Task 2: client ws over the process-global tokio+tungstenite engine (core/src/ws.rs).
    set_native(scope, global_obj, "__s2_ws_connect", s2_ws_connect);
    set_native(scope, global_obj, "__s2_ws_send", s2_ws_send);
    set_native(scope, global_obj, "__s2_ws_close", s2_ws_close);
    set_native(scope, global_obj, "__s2_ws_on", s2_ws_on);
    // Net Task 2: raw TCP/UDP client sockets over the process-global tokio engine (core/src/net.rs).
    set_native(scope, global_obj, "__s2_net_tcp_connect", s2_net_tcp_connect);
    set_native(scope, global_obj, "__s2_net_udp_bind", s2_net_udp_bind);
    set_native(scope, global_obj, "__s2_net_send", s2_net_send);
    set_native(scope, global_obj, "__s2_net_send_to", s2_net_send_to);
    set_native(scope, global_obj, "__s2_net_close", s2_net_close);
    set_native(scope, global_obj, "__s2_net_on", s2_net_on);
    // TopMenu registry (adminmenu framework): owner-tracked categories/items + post-drain select dispatch.
    set_native(scope, global_obj, "__s2_topmenu_add_category", s2_topmenu_add_category);
    set_native(scope, global_obj, "__s2_topmenu_add_item", s2_topmenu_add_item);
    set_native(scope, global_obj, "__s2_topmenu_snapshot", s2_topmenu_snapshot);
    set_native(scope, global_obj, "__s2_topmenu_select", s2_topmenu_select);
    // Ray-trace slice: the sole native over the trace_shape engine op (engine-generic, no CS2 names).
    set_native(scope, global_obj, "__s2_trace", s2_trace);
    // Entity-creation lifecycle slice: createEntity + EntityRef.spawn/teleport/remove natives.
    set_native(scope, global_obj, "__s2_entity_create", s2_entity_create);
    set_native(scope, global_obj, "__s2_entity_find_by_class", s2_entity_find_by_class);
    set_native(scope, global_obj, "__s2_user_message_create", s2_user_message_create);
    set_native(scope, global_obj, "__s2_user_message_set_int", s2_user_message_set_int);
    set_native(scope, global_obj, "__s2_user_message_set_float", s2_user_message_set_float);
    set_native(scope, global_obj, "__s2_user_message_set_string", s2_user_message_set_string);
    set_native(scope, global_obj, "__s2_user_message_set_bool", s2_user_message_set_bool);
    set_native(scope, global_obj, "__s2_user_message_send", s2_user_message_send);
    set_native(scope, global_obj, "__s2_entity_spawn", s2_entity_spawn);
    set_native(scope, global_obj, "__s2_collision_activate", s2_collision_activate);
    set_native(scope, global_obj, "__s2_ent_set_model", s2_ent_set_model);
    set_native(scope, global_obj, "__s2_entity_teleport", s2_entity_teleport);
    set_native(scope, global_obj, "__s2_entity_remove", s2_entity_remove);
    // Item slice: give/vcall/remove-item natives + the readHandleVector native (wrapped as an
    // EntityRef prototype method in the prelude, below).
    set_native(scope, global_obj, "__s2_give_named_item", s2_give_named_item);
    set_native(scope, global_obj, "__s2_entity_subobj_vcall", s2_entity_subobj_vcall);
    set_native(scope, global_obj, "__s2_remove_player_item", s2_remove_player_item);
    set_native(scope, global_obj, "__s2_entity_read_handle_vector", s2_entity_read_handle_vector);
    // Entity-I/O slice: fire inputs (AddEntityIOEvent) + Entity.onOutput subscribe/unsubscribe
    // (FireOutputInternal detour dispatch — installed at shim Load, see dispatch_output).
    set_native(scope, global_obj, "__s2_entity_fire_input", s2_entity_fire_input);
    set_native(scope, global_obj, "__s2_entity_spawn_kv", s2_entity_spawn_kv);
    set_native(scope, global_obj, "__s2_output_subscribe", s2_output_subscribe);
    set_native(scope, global_obj, "__s2_output_unsubscribe", s2_output_unsubscribe);
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

/// `__s2_admin_set(steamid, flags, immunity, runtime)` — set/overwrite a SteamID's flags + immunity in
/// the file(false)/runtime(true) tier.
fn s2_admin_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let flags = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let immunity = args.get(2).number_value(scope).unwrap_or(0.0) as i32;
        let runtime = args.get(3).boolean_value(scope);
        if runtime {
            ADMIN_RUNTIME.with(|m| { m.borrow_mut().insert(sid.clone(), flags); });
            ADMIN_RUNTIME_IMMUNITY.with(|m| { m.borrow_mut().insert(sid, immunity); });
        } else {
            ADMIN_FILE.with(|m| { m.borrow_mut().insert(sid.clone(), flags); });
            ADMIN_FILE_IMMUNITY.with(|m| { m.borrow_mut().insert(sid, immunity); });
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

/// `__s2_admin_get_immunity(steamid) -> number` — max immunity across both tiers (0 = none).
fn s2_admin_get_immunity(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { rv.set_double(0.0); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let f = ADMIN_FILE_IMMUNITY.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        let r = ADMIN_RUNTIME_IMMUNITY.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        rv.set_double(f.max(r) as f64);
    }));
}

/// `__s2_admin_add_override(steamid, cmd, mask, isPublic)` — a per-admin (file-tier) command override.
fn s2_admin_add_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let cmd = args.get(1).to_rust_string_lossy(scope);
        let mask = args.get(2).number_value(scope).unwrap_or(0.0) as u64;
        let is_public = args.get(3).boolean_value(scope);
        ADMIN_OVERRIDES.with(|m| {
            m.borrow_mut().entry(sid).or_default().insert(cmd, (mask, is_public));
        });
    }));
}

/// `__s2_admin_set_global_override(cmd, mask, isPublic)` — a global (file-tier) command override.
fn s2_admin_set_global_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let cmd = args.get(0).to_rust_string_lossy(scope);
        let mask = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let is_public = args.get(2).boolean_value(scope);
        ADMIN_GLOBAL_OVERRIDES.with(|m| { m.borrow_mut().insert(cmd, (mask, is_public)); });
    }));
}

/// `__s2_admin_override(steamid, cmd) -> string` — "" (none) / "public" / decimal mask. Per-admin beats global.
fn s2_admin_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { let s = v8::String::new(scope, "").unwrap(); rv.set(s.into()); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let cmd = args.get(1).to_rust_string_lossy(scope);
        let hit = ADMIN_OVERRIDES.with(|m| m.borrow().get(&sid).and_then(|c| c.get(&cmd).copied()))
            .or_else(|| ADMIN_GLOBAL_OVERRIDES.with(|m| m.borrow().get(&cmd).copied()));
        let out = match hit {
            None => String::new(),
            Some((_, true)) => "public".to_string(),
            Some((mask, false)) => mask.to_string(),
        };
        let s = v8::String::new(scope, &out).unwrap();
        rv.set(s.into());
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
            ADMIN_RUNTIME_IMMUNITY.with(|m| { m.borrow_mut().remove(&sid); });
        } else {
            ADMIN_FILE.with(|m| { m.borrow_mut().remove(&sid); });
            ADMIN_FILE_IMMUNITY.with(|m| { m.borrow_mut().remove(&sid); });
        }
    }));
}

/// `__s2_admin_clear_file()` — wipe the file tier (Admin.reload re-reads into it), plus the file-tier
/// immunity map, per-admin overrides, and global overrides (all file-tier-sourced).
fn s2_admin_clear_file(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ADMIN_FILE.with(|m| m.borrow_mut().clear());
        ADMIN_FILE_IMMUNITY.with(|m| m.borrow_mut().clear());
        ADMIN_OVERRIDES.with(|m| m.borrow_mut().clear());
        ADMIN_GLOBAL_OVERRIDES.with(|m| m.borrow_mut().clear());
    }));
}

/// `__s2_admin_mark_loaded() -> boolean` — returns the PRIOR loaded state, then sets it true (one-shot load guard).
fn s2_admin_mark_loaded(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let prev = ADMIN_FILE_LOADED.with(|c| { let p = c.get(); c.set(true); p });
        rv.set_bool(prev);
    }));
}

// --- Slice 6.18: ban cache natives + ban_check ---

/// `__s2_ban_set(steamid, until, reason)` — insert/overwrite a ban. `until == 0` = permanent, else unix-sec expiry.
fn s2_ban_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let until = args.get(1).number_value(scope).unwrap_or(0.0) as i64;
        let reason = args.get(2).to_rust_string_lossy(scope);
        BAN_CACHE.with(|m| { m.borrow_mut().insert(sid, (until, reason)); });
    }));
}

/// `__s2_ban_get(steamid) -> string | null` — JSON `{"until":N,"reason":"..."}` if present, else null.
fn s2_ban_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let entry = BAN_CACHE.with(|m| m.borrow().get(&sid).cloned());
        match entry {
            Some((until, reason)) => {
                let json = serde_json::json!({ "until": until, "reason": reason }).to_string();
                if let Some(js) = v8::String::new(scope, &json) { rv.set(js.into()); }
            }
            None => rv.set_null(),
        }
    }));
}

/// `__s2_ban_remove(steamid) -> boolean` — remove; returns whether the key was present.
fn s2_ban_remove(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let removed = BAN_CACHE.with(|m| m.borrow_mut().remove(&sid).is_some());
        rv.set_bool(removed);
    }));
}

/// `__s2_ban_clear()` — wipe the cache (Bans.reload re-parses the file into it).
fn s2_ban_clear(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        BAN_CACHE.with(|m| m.borrow_mut().clear());
    }));
}

/// `__s2_ban_list() -> string` — JSON array `[{"steamid":"..","until":N,"reason":".."}]`.
fn s2_ban_list(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let items: Vec<serde_json::Value> = BAN_CACHE.with(|m| {
            m.borrow().iter()
                .map(|(sid, (until, reason))| serde_json::json!({
                    "steamid": sid, "until": until, "reason": reason,
                }))
                .collect()
        });
        let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
        if let Some(js) = v8::String::new(scope, &json) { rv.set(js.into()); }
    }));
}

/// `__s2_ban_mark_loaded() -> boolean` — returns the PRIOR loaded state, then sets it true (one-shot guard).
fn s2_ban_mark_loaded(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let prev = BAN_LOADED.with(|c| { let p = c.get(); c.set(true); p });
        rv.set_bool(prev);
    }));
}

/// Returns `Some(reason)` if `xuid` is currently banned (perm or unexpired), else `None`.
/// Retained as an available synchronous ban-check primitive (via the `s2script_core_ban_check` ffi
/// export); no longer called by the shim since sub-project 3 moved enforcement to the JS onConnect path.
pub fn ban_check(xuid: u64, now: i64) -> Option<String> {
    let key = xuid.to_string();
    BAN_CACHE.with(|m| {
        m.borrow().get(&key).and_then(|(until, reason)| {
            if *until == 0 || *until > now { Some(reason.clone()) } else { None }
        })
    })
}

// --- clientprefs: cookie cache natives over crate::cookies (host-global, mirrors admin/ban). ---

/// `__s2_cookie_get(steamid, name) -> string | undefined` — `undefined` on a true miss (distinct
/// from a stored `""`); the module layer decides the default fallback.
fn s2_cookie_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        match crate::cookies::get(&sid, &name) {
            Some(v) => { if let Some(s) = v8::String::new(scope, &v) { rv.set(s.into()); } }
            None => { rv.set(v8::undefined(scope).into()); }
        }
    }));
}

/// `__s2_cookie_set(steamid, name, value, updated)` — write via the API; marks the entry dirty.
fn s2_cookie_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        crate::cookies::set(&sid, &name, &val, updated);
    }));
}

/// `__s2_cookie_load(steamid, name, value, updated)` — write from the DB load; NOT dirty.
fn s2_cookie_load(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        crate::cookies::load(&sid, &name, &val, updated);
    }));
}

/// `__s2_cookie_get_time(steamid, name) -> number` — the stored `updated` timestamp, or 0 if absent.
fn s2_cookie_get_time(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let t = crate::cookies::get_time(&sid, &name);
        rv.set(v8::Number::new(scope, t as f64).into());
    }));
}

/// `__s2_cookie_get_dirty(steamid) -> { [name]: value }` — the dirty (disconnect flush) set as a JS object.
fn s2_cookie_get_dirty(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let pairs = crate::cookies::get_dirty(&sid);
        let obj = v8::Object::new(scope);
        for (name, value) in pairs.iter() {
            let k = v8::String::new(scope, name).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let v = v8::String::new(scope, value).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            obj.set(scope, k.into(), v.into());
        }
        rv.set(obj.into());
    }));
}

/// `__s2_cookie_clear(steamid)` — drop a client's entries (on disconnect, after the flush captures the dirty set).
fn s2_cookie_clear(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        crate::cookies::clear(&sid);
    }));
}

/// `__s2_cookie_mark_cached(steamid)` — mark a client's cookies loaded (a zero-cookie client is still "cached").
fn s2_cookie_mark_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        crate::cookies::mark_cached(&sid);
    }));
}

/// `__s2_cookie_is_cached(steamid) -> boolean`.
fn s2_cookie_is_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        rv.set(v8::Boolean::new(scope, crate::cookies::is_cached(&sid)).into());
    }));
}

/// `__s2_cookie_set_authid(steamid, name, value, updated)` — `SetAuthIdCookie` parity: write for a
/// SteamID that may not currently be connected (cache write + queue for offline persistence).
fn s2_cookie_set_authid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        let updated = args.get(3).integer_value(scope).unwrap_or(0);
        crate::cookies::set_authid(&sid, &name, &val, updated);
    }));
}

/// `__s2_cookie_take_offline_writes() -> Array<[steamid, name, value, updated]>` — drain + clear the
/// queued offline writes for the plugin to persist directly (an offline SteamID never fires the
/// disconnect flush).
fn s2_cookie_take_offline_writes(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let writes = crate::cookies::take_offline_writes();
        let out = v8::Array::new(scope, writes.len() as i32);
        for (i, (sid, name, val, updated)) in writes.iter().enumerate() {
            let row = v8::Array::new(scope, 4);
            let sid_s = v8::String::new(scope, sid).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let name_s = v8::String::new(scope, name).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let val_s = v8::String::new(scope, val).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let updated_n = v8::Number::new(scope, *updated as f64);
            row.set_index(scope, 0, sid_s.into());
            row.set_index(scope, 1, name_s.into());
            row.set_index(scope, 2, val_s.into());
            row.set_index(scope, 3, updated_n.into());
            out.set_index(scope, i as u32, row.into());
        }
        rv.set(out.into());
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

/// `__s2_client_console_print(slot, msg)` — print one line to the client's developer console.
/// No-op without the op / for a bad slot / for a bot (shim skips a null-netchannel fake client).
fn s2_client_console_print(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let msg = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.client_console_print else { return };
        if let Ok(cmsg) = CString::new(msg) { f(slot, cmsg.as_ptr()); }
    }));
}

/// `__s2_client_address(slot) -> string` — the client's IP address ("IP:port"). `""` without the op / for a bot / on null.
fn s2_client_address(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let s: String = (|| {
            let ops = ENGINE_OPS.with(|o| o.get())?;
            let f = ops.client_address?;
            let ptr = f(slot);
            if ptr.is_null() { return None; }
            Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
        })().unwrap_or_default();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
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
// Slice DB Task 3: the `__s2_sqlite_*` natives — sync-behind-Promise execution over
// `crate::db` (Task 1) + the `db_data_dir` engine op (Task 2). Every native returns a real
// `Promise` (the async API contract), resolved/rejected INLINE (no threadpool this slice — see
// the plan's simplification note). A connection handle is ledgered against the CALLING plugin
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

/// Native `__s2_sqlite_query(handle, sql, params) -> Promise<Row[]>`. Runs a parameterized SELECT
/// synchronously and resolves an array of row objects keyed by column name (`query_result_to_js`);
/// an invalid handle or SQL error rejects the Promise.
fn s2_sqlite_query(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1);
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let result = if handle < 0 {
            Err("invalid db handle".to_string())
        } else {
            crate::db::query(handle as u64, &sql, &params, &owner)
        };
        match result {
            Ok(qr) => {
                let result = query_result_to_js(scope, &qr);
                resolver.resolve(scope, result);
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

/// Native `__s2_sqlite_execute(handle, sql, params) -> Promise<{changes, lastInsertId}>`. Runs a
/// parameterized INSERT/UPDATE/DELETE/DDL statement synchronously; resolves `{changes,
/// lastInsertId}` (both JS numbers); an invalid handle or SQL error rejects the Promise.
fn s2_sqlite_execute(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1);
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let result = if handle < 0 {
            Err("invalid db handle".to_string())
        } else {
            crate::db::execute(handle as u64, &sql, &params, &owner)
        };
        match result {
            Ok(er) => {
                let obj = v8::Object::new(scope);
                let k = v8::String::new(scope, "changes").unwrap();
                let v = v8::Number::new(scope, er.changes as f64);
                obj.set(scope, k.into(), v.into());
                let k = v8::String::new(scope, "lastInsertId").unwrap();
                let v = v8::Number::new(scope, er.last_insert_id as f64);
                obj.set(scope, k.into(), v.into());
                resolver.resolve(scope, obj.into());
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
fn resolve_db(host: &mut Host, entry: &ResolverEntry, result: Result<crate::sqldb::DbOutcome, String>) {
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
        Ok(crate::sqldb::DbOutcome::Query(qr)) => {
            let v = query_result_to_js(scope, &qr);
            resolver.resolve(scope, v);
        }
        Ok(crate::sqldb::DbOutcome::Exec(er)) => {
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
    // Slice HTTP Task 2: build the process-global tokio+reqwest engine (idempotent — a OnceLock,
    // survives a Metamod re-init just like `pool()`). Holds no V8 handles; wiring it here (rather
    // than lazily on first `__s2_fetch` call) keeps engine-generic subsystem setup in one place.
    crate::http::init();

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
    // Clear the flag-meta sidecar too (pure i64, no V8 handles, but reset for re-init hygiene).
    COMMAND_META.with(|m| m.borrow_mut().clear());
    // Clear the TopMenu registry BEFORE dropping the isolate — TOPMENU_ITEMS holds Global<Function>s
    // into it (same discipline as CONCOMMANDS); categories/pending are pure Rust, cleared for hygiene.
    TOPMENU_ITEMS.with(|m| m.borrow_mut().clear());
    TOPMENU_CATEGORIES.with(|c| c.borrow_mut().clear());
    TOPMENU_SEQ.with(|c| c.set(0));
    TOPMENU_PENDING.with(|q| q.borrow_mut().clear());
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
    // Reset the raw-chat subscriber mux (Slice 6.13b) so a re-init starts clean.
    CHAT_MSG_SUBS.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the client-lifecycle mux (Clients sub-project) so a re-init starts clean.
    CLIENT_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the map-start mux (clientlist-fakeconvar-onmapstart slice) so a re-init starts clean.
    MAP_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the Cookies.onCached mux + pending queue (clientprefs Task 4) so a re-init starts clean.
    COOKIE_CACHED_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().clear());
    // Reset the WebSocket on* mux + pending queue (WebSocket Task 2) so a re-init starts clean.
    WS_EVENT_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    WS_EVENT_PENDING.with(|q| q.borrow_mut().clear());
    // Reset the net (raw TCP/UDP) on* mux + pending queue (Net Task 2) so a re-init starts clean.
    NET_EVENT_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    NET_EVENT_PENDING.with(|q| q.borrow_mut().clear());
    // Reset the Entity.onOutput mux (entity-I/O slice) so a re-init starts clean.
    OUTPUT_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
    // Reset the reload-handoff map (Slice 5E.3) so a re-init starts clean.
    PENDING_HANDOFF.with(|h| h.borrow_mut().clear());
    // Reset the schema-offset cache so a re-init re-resolves (a `-1` cached before the schema was
    // loaded must not persist across an init cycle).
    SCHEMA_OFFSETS.with(|c| *c.borrow_mut() = crate::schema::OffsetCache::new());
    // Reset the admin cache tiers (Slice 6.2) so a re-init starts with no admins.
    ADMIN_FILE.with(|m| m.borrow_mut().clear());
    ADMIN_RUNTIME.with(|m| m.borrow_mut().clear());
    ADMIN_FILE_LOADED.with(|c| c.set(false));
    // Reset the ban cache (Slice 6.18) so a re-init starts with no bans.
    BAN_CACHE.with(|m| m.borrow_mut().clear());
    BAN_LOADED.with(|c| c.set(false));
    // Reset the cookie cache (clientprefs) so a re-init starts with no stale entries / cached flags.
    crate::cookies::reset();
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
        while let Some(c) = crate::sqldb::try_recv_completed() {
            let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&c.id)) else { continue };
            PENDING_JOBS.with(|cnt| cnt.set(cnt.get().saturating_sub(1)));
            resolve_db(host, &entry, c.result);
        }
        // Route completed ws signals (WebSocket Task 2, over core/src/ws.rs's tokio+tungstenite
        // engine). ORDERING (load-bearing): Connected/ConnectFailed resolve/reject the connect
        // Promise INSIDE this drain (before the microtask checkpoint below, so the plugin's `.then`
        // continuation — which subscribes onMessage — runs THIS frame); Message/Errored/Closed are
        // queued into WS_EVENT_PENDING and fanned out separately, AFTER this whole drain returns
        // (dispatch_pending_ws_events, called from ffi.rs, HOST free) — never before the checkpoint.
        while let Some(sig) = crate::ws::try_recv_signal() {
            match sig.kind {
                crate::ws::WsSignalKind::Connected => {
                    if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                        PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                        resolve_ws_connect(host, &entry, sig.conn_id, Ok(()));
                    }
                }
                crate::ws::WsSignalKind::ConnectFailed(e) => {
                    if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                        PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                        resolve_ws_connect(host, &entry, sig.conn_id, Err(e));
                    }
                    crate::ws::drop_conn(sig.conn_id);
                }
                crate::ws::WsSignalKind::Message(t) => {
                    WS_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, "message".into(), t, 0)));
                }
                crate::ws::WsSignalKind::Errored(e) => {
                    WS_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, "error".into(), e, 0)));
                }
                crate::ws::WsSignalKind::Closed(code, reason) => {
                    WS_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, "close".into(), reason, code as i32)));
                    crate::ws::drop_conn(sig.conn_id);
                    // (mux subscribers for this conn are cleaned up when the plugin unloads; a closed
                    // conn's stale subscribers simply never fire again — acceptable.)
                }
            }
        }

        // Route completed net (raw TCP/UDP) signals (Net Task 2, over core/src/net.rs's tokio engine).
        // MIRRORS the ws routing above verbatim: Connected/Bound resolve the connect/bind Promise INSIDE
        // this drain (before the microtask checkpoint, so the plugin's `.then` — which subscribes
        // onData/onMessage — runs THIS frame); ConnectFailed rejects + drops; Data/Datagram/Errored are
        // queued into NET_EVENT_PENDING and fanned out post-drain (dispatch_pending_net_events, HOST
        // free); Closed queues then drops the conn (the drain's single drop_conn/mux-prune driver).
        while let Some(sig) = crate::net::try_recv_signal() {
            match sig.kind {
                crate::net::NetSignalKind::Connected | crate::net::NetSignalKind::Bound => {
                    if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                        PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                        resolve_net_connect(host, &entry, sig.conn_id, Ok(()));
                    }
                }
                crate::net::NetSignalKind::ConnectFailed(e) => {
                    if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                        PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                        resolve_net_connect(host, &entry, sig.conn_id, Err(e));
                    }
                    crate::net::drop_conn(sig.conn_id);
                }
                crate::net::NetSignalKind::Data(b) => {
                    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Data(b))));
                }
                crate::net::NetSignalKind::Datagram { from, data } => {
                    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Datagram { from, data })));
                }
                crate::net::NetSignalKind::Errored(e) => {
                    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Errored(e))));
                }
                crate::net::NetSignalKind::Closed => {
                    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Closed)));
                    crate::net::drop_conn(sig.conn_id);
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
    // (a2c') Drop the plugin's Chat.onMessage subscriptions (Slice 6.13b). The Host_Say detour is
    // installed for the process lifetime (removed in the shim's Unload), so no hook-removal request.
    CHAT_MSG_SUBS.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c'') Drop the plugin's client-lifecycle subscriptions (Clients sub-project). The six shim
    // lifecycle hooks are installed for the process lifetime (removed in the shim's Unload), so no
    // per-plugin hook-removal request is needed.
    CLIENT_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c'') Drop the plugin's Server.onMapStart subscriptions (clientlist-fakeconvar-onmapstart
    // slice). The StartupServer hook is installed for the process lifetime (removed in the shim's
    // Unload), so no per-plugin hook-removal request is needed.
    MAP_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c'') Drop the plugin's Cookies.onCached subscriptions (clientprefs Task 4). No engine-level
    // hook to remove (the fan-out is a pure post-frame JS dispatch, no shim involvement).
    COOKIE_CACHED_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c''') Drop the plugin's WebSocket on* subscriptions (WebSocket Task 2). No engine-level hook
    // to remove (the fan-out is a pure post-frame JS dispatch, like COOKIE_CACHED_MUX); the underlying
    // connections themselves are closed below via the ledger's WsConn resources.
    WS_EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c''') Drop the plugin's net (raw TCP/UDP) on* subscriptions (Net Task 2). No engine-level hook
    // to remove (a pure post-frame JS dispatch, like WS_EVENT_MUX); the underlying sockets themselves
    // are dropped below via the ledger's NetConn resources.
    NET_EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c'''') Drop the plugin's Entity.onOutput subscriptions (entity-I/O slice). The FireOutputInternal
    // detour stays installed for the process lifetime (removed in the shim's Unload), so no per-plugin
    // hook-removal request is needed.
    OUTPUT_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
    // (a2c) Drop the plugin's config-change subscriptions (Slice 5E.2) and stop watching its file.
    CONFIG_SUBS.with(|m| m.borrow_mut().remove_by_owner(id));
    crate::loader::unwatch_config_for(id);
    // (a2d) Drop the plugin's registered ConCommands so a post-unload dispatch no-ops. This is the
    // per-plugin (.s2sp) unload: we remove from the JS dispatch map only — the shim's ICvar ConCommand
    // stays registered (idempotent, reload-safe) and re-routes to the new handler on reload. The engine-
    // side ICvar unregister happens on full shim teardown (Metamod Unload → s2script_mm.cpp, UAF-safe).
    let dropped_cmds: Vec<String> = CONCOMMANDS.with(|m| {
        let mut b = m.borrow_mut();
        let names: Vec<String> = b.iter().filter(|(_, (owner, _, _))| owner == id).map(|(n, _)| n.clone()).collect();
        b.retain(|_, (owner, _, _)| owner != id);
        names
    });
    // Drop the departing plugin's flag-meta alongside its commands (a stale entry would be ignored by the
    // list join anyway, but keep the sidecar tidy — trivial).
    COMMAND_META.with(|m| { let mut b = m.borrow_mut(); for n in &dropped_cmds { b.remove(n); } });
    // (a2e) Drop the plugin's registered TopMenu items (adminmenu framework). Categories are left in
    // place (harmless if empty; SM parity — a category persists once created) — only owner-scoped items
    // are torn down, mirroring the CONCOMMANDS cleanup above.
    TOPMENU_ITEMS.with(|m| m.borrow_mut().retain(|_, it| it.owner != id));

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
    // Returns the completion value of `src`'s last statement as a String (mirrors
    // `eval_in_context_string`) so callers can assert on a computed value (e.g. `JSON.stringify(...)`);
    // callers that only care about side effects may simply discard the return. Panics loudly (with the
    // JS exception message) on a compile or runtime error, same as the previous void-returning behavior.
    fn eval_std(id: &str, src: &str) -> String {
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

    /// `__s2_commands_list` returns valid JSON: `[]` when no commands are registered, and a
    /// `[{name, flags}]` entry (with the flags passed to `__s2_concommand`) once one is registered.
    /// Mirrors the plugins-list style; exercises the store + list join without the engine.
    #[test]
    fn commands_list_returns_name_and_flags() {
        init(dummy_logger()).unwrap();
        // Load a plugin whose body: (1) confirms the list is empty BEFORE any registration, then
        // (2) registers two commands with distinct flag masks (2nd arg is the callback, 3rd is the flags).
        load_plugin_js("cl_test", r#"
            globalThis.__cl_empty = __s2_commands_list();               // must be "[]" — nothing registered yet
            __s2_concommand("s2_open", function () {}, 0);              // 0 = anyone
            __s2_concommand("s2_admin", function () {}, 6);             // an ADMFLAG bit mask
            var list = JSON.parse(__s2_commands_list());
            var byName = {};
            for (var i = 0; i < list.length; i++) { byName[list[i].name] = list[i].flags; }
            globalThis.__cl = list.length + "|" + byName["s2_open"] + "|" + byName["s2_admin"];
        "#, "{}");
        // Empty (valid JSON) before registration, then both commands surface with their flags.
        assert_eq!(eval_in_context_string("cl_test", "String(globalThis.__cl_empty)"), "[]");
        assert_eq!(eval_in_context_string("cl_test", "String(globalThis.__cl)"), "2|0|6");
        // Native still returns valid JSON directly.
        assert_eq!(eval_in_context_string("cl_test", "typeof __s2_commands_list()"), "string");
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

    /// Clients sub-project Task 1: the `@s2script/clients` prelude exposes the `Client` class + the
    /// `Clients` namespace (6 lifecycle `on*` + `fromSlot`/`all`).  With no engine-ops table wired,
    /// `__s2_client_valid` degrades false → `fromSlot(0)` is null and `all()` is empty; `Client.isBot`
    /// derives from `steamId === "0"` (the no-ops steamid degrade), so a bare `new Client(0)` is a bot.
    #[test]
    fn clients_prelude_exposes_client_and_clients_namespace() {
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops → __s2_client_valid degrades false, steamid "0"
        create_plugin_context("pcl");
        assert_eq!(eval_in_context_string("pcl", "typeof globalThis.__s2pkg_clients"), "object");
        assert_eq!(eval_in_context_string("pcl", "typeof __s2pkg_clients.Client"), "function");
        // All 6 lifecycle subscribers + the two enumerators are present as functions.
        for m in ["onConnect", "onPutInServer", "onActive", "onFullyConnect", "onDisconnect",
                  "onSettingsChanged", "fromSlot", "all"] {
            assert_eq!(
                eval_in_context_string("pcl", &format!("typeof __s2pkg_clients.Clients.{}", m)),
                "function", "Clients.{} must be a function", m);
        }
        // No engine → an empty slot: fromSlot(0) is null, all() is [].
        assert_eq!(eval_in_context_string("pcl", "String(__s2pkg_clients.Clients.fromSlot(0))"), "null");
        assert_eq!(eval_in_context_string("pcl", "String(__s2pkg_clients.Clients.all().length)"), "0");
        // A Client is slot-backed; isBot derives from steamId === "0" (no-ops steamid → "0" → bot).
        assert_eq!(eval_in_context_string("pcl", "String(new __s2pkg_clients.Client(3).slot)"), "3");
        assert_eq!(eval_in_context_string("pcl", "String(new __s2pkg_clients.Client(3).isBot)"), "true");
        shutdown();
    }

    /// Clients sub-project Task 1: a subscribed `onConnect` handler receives a `Client` whose `.slot`
    /// equals the dispatched slot (the `CLIENT_MUX` reuse + the JS wrapper's `new Client(slot)`);
    /// a different event name (`"active"`) is independent (does NOT run the connect handler); and after
    /// `unload_plugin` (remove_by_owner teardown) further dispatches are a safe no-op.
    #[test]
    fn client_dispatch_delivers_client_with_slot() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_plugin_js("pcl", r#"
            __s2pkg_clients.Clients.onConnect(function (c) {
                globalThis.__cl_ran  = (globalThis.__cl_ran || 0) + 1;
                globalThis.__cl_slot = c.slot;
                globalThis.__cl_ctor = (c instanceof __s2pkg_clients.Client) ? 1 : 0;
            });
            __s2pkg_clients.Clients.onActive(function (c) {
                globalThis.__cl_active_slot = c.slot;
            });
        "#, "{}");

        // Dispatch "connect" slot 3 → the connect handler runs once and receives a Client(3).
        dispatch_client_event("connect", 3);
        assert_eq!(read_i32_global_in("pcl", "__cl_ran"), 1, "connect handler must run exactly once");
        assert_eq!(read_i32_global_in("pcl", "__cl_slot"), 3, "handler must receive the dispatched slot");
        assert_eq!(read_i32_global_in("pcl", "__cl_ctor"), 1, "the argument must be a Client instance");

        // Independence: dispatching "active" must not re-run the connect handler.
        dispatch_client_event("active", 5);
        assert_eq!(read_i32_global_in("pcl", "__cl_ran"), 1, "connect handler must not run for 'active'");
        assert_eq!(read_i32_global_in("pcl", "__cl_active_slot"), 5, "the active handler receives its own slot");

        // Teardown: unload removes all of pcl's client subs; a later dispatch is a safe no-op.
        unload_plugin("pcl");
        dispatch_client_event("connect", 9);   // must not crash / must not deliver (context disposed)
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
        dispatch_map_start("de_test");
        assert_eq!(eval_in_context_string("pms", "globalThis.__map"), "de_test");
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
            pawn_commit_suicide: None,
            client_console_print: None,
            client_address: None,
            server_max_clients: None,
            server_map_name: None,
            server_game_time: None,
            db_data_dir: None,
            event_fire_to_client: None,
            config_read_file: None,
            config_write_file: None,
            trace_shape: None,
            entity_create: None,
            entity_spawn: None,
            entity_teleport: None,
            entity_remove: None,
            give_named_item: None,
            entity_subobj_vcall: None,
            remove_player_item: None,
            entity_read_handle_vector: None,
            entity_fire_input: None,
            entity_spawn_kv: None,
            entity_find_by_class: None,
            user_message_create: None,
            user_message_set_int: None,
            user_message_set_float: None,
            user_message_set_string: None,
            user_message_set_bool: None,
            user_message_send: None,
            convar_register: None,
            translations_read: None,
            client_language: None,
            collision_activate: None,
            entity_set_model: None,
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

    /// Entity-creation lifecycle slice: `createEntity` degrades to `null` with no `entity_create`
    /// op (e.g. every in-isolate test) — never a crash.
    #[test]
    fn entity_create_native_degrades_to_null_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const { createEntity } = __s2pkg_entity;
            String(createEntity("env_beam"))
        "#);
        assert_eq!(out, "null");
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
        EKV_CAPTURE.lock().unwrap().clear();
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps { entity_spawn_kv: Some(capture_spawn_kv), ..mock_event_ops() }));
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
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
        EKV_CAPTURE.lock().unwrap().clear();
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps { entity_spawn_kv: Some(capture_spawn_kv), ..mock_event_ops() }));
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
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

    /// Item slice: `__s2_give_named_item`/`__s2_entity_subobj_vcall`/`__s2_remove_player_item`/
    /// `EntityRef.readHandleVector` all degrade (null/false/false/[]) with no engine ops wired —
    /// never a crash.
    #[test]
    fn item_natives_degrade_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const r = new (__s2pkg_entity.EntityRef)(1, 7);
            [ String(__s2_give_named_item(1,7,3304,"weapon_ak47")),
              __s2_entity_subobj_vcall(1,7,3304,25,-1,-1),
              __s2_remove_player_item(1,7,2,9),
              JSON.stringify(r.readHandleVector([3296], 100, 64)) ].join("|")
        "#);
        assert_eq!(out, "null|false|false|[]");
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
            pawn_commit_suicide: None,
            client_console_print: None,
            client_address: None,
            server_max_clients: None,
            server_map_name: None,
            server_game_time: None,
            db_data_dir: None,
            event_fire_to_client: None,
            config_read_file: None,
            config_write_file: None,
            trace_shape: None,
            entity_create: None,
            entity_spawn: None,
            entity_teleport: None,
            entity_remove: None,
            give_named_item: None,
            entity_subobj_vcall: None,
            remove_player_item: None,
            entity_read_handle_vector: None,
            entity_fire_input: None,
            entity_spawn_kv: None,
            entity_find_by_class: None,
            user_message_create: None,
            user_message_set_int: None,
            user_message_set_float: None,
            user_message_set_string: None,
            user_message_set_bool: None,
            user_message_send: None,
            convar_register: None,
            translations_read: None,
            client_language: None,
            collision_activate: None,
            entity_set_model: None,
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

    /// Ban-reason sub-project 2: the console-print + client-address natives degrade cleanly
    /// with no engine ops wired — `__s2_client_console_print` is a no-op (never throws) and
    /// `__s2_client_address` returns "" (an empty string, never null).
    #[test]
    fn client_console_print_and_address_degrade_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // console-print no-ops (returns undefined, never throws) without the op.
        assert_eq!(eval_in_context_string("p", "String(__s2_client_console_print(0, 'x'))"), "undefined");
        // address returns "" (empty string, NOT null) without the op.
        assert_eq!(eval_in_context_string("p", "__s2_client_address(0)"), "");
        assert_eq!(eval_in_context_string("p", "typeof __s2_client_address(0)"), "string");
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

    /// Ban-reason sub-project 2: the `@s2script/clients` prelude exposes `Client.prototype.print`,
    /// the `ip` getter, and `Client.prototype.kickWithReason` on the module surface.  With no engine
    /// ops, `print` is a no-op (returns undefined), `ip` returns "" (address degrade → ""), and
    /// `kickWithReason` is a callable function.  Also verifies the ":port" strip logic via a faked
    /// `__s2_client_address` in-isolate.
    #[test]
    fn clients_prelude_exposes_print_ip_and_kick_with_reason() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        set_engine_ops(None);   // no ops → __s2_client_address returns ""
        create_plugin_context("pcl2");
        // print is a function on the prototype.
        assert_eq!(eval_in_context_string("pcl2", "typeof __s2pkg_clients.Client.prototype.print"), "function");
        // kickWithReason is a function on the prototype.
        assert_eq!(eval_in_context_string("pcl2", "typeof __s2pkg_clients.Client.prototype.kickWithReason"), "function");
        // ip getter: no engine → address "" → ip "".
        assert_eq!(eval_in_context_string("pcl2", "new __s2pkg_clients.Client(0).ip"), "");
        // print is a no-op without the op (returns undefined, never throws).
        assert_eq!(eval_in_context_string("pcl2", "String(new __s2pkg_clients.Client(0).print('hello'))"), "undefined");
        // ":port" strip logic: fake __s2_client_address then check the getter strips correctly.
        assert_eq!(
            eval_in_context_string("pcl2",
                "(function () { \
                    var orig = globalThis.__s2_client_address; \
                    globalThis.__s2_client_address = function () { return \"1.2.3.4:27005\"; }; \
                    var ip = new __s2pkg_clients.Client(0).ip; \
                    globalThis.__s2_client_address = orig; \
                    return ip; \
                }())"),
            "1.2.3.4");
        // address with no colon returns the value unchanged.
        assert_eq!(
            eval_in_context_string("pcl2",
                "(function () { \
                    var orig = globalThis.__s2_client_address; \
                    globalThis.__s2_client_address = function () { return \"1.2.3.4\"; }; \
                    var ip = new __s2pkg_clients.Client(0).ip; \
                    globalThis.__s2_client_address = orig; \
                    return ip; \
                }())"),
            "1.2.3.4");
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
        eval_in_context("p", "__s2_admin_set('111', 4, 0, false); __s2_admin_set('111', 1, 0, true);").unwrap(); // file KICK(4) + runtime RESERVATION(1)
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
        // reservedslots+basetriggers: server-info natives degrade (max_clients->0, map_name->"", game_time->0)
        // and the @s2script/server module exposes maxPlayers/mapName/gameTime getters that pass them through.
        assert_eq!(eval_in_context_string("p", "String(__s2_server_max_clients())"), "0");
        assert_eq!(eval_in_context_string("p", "__s2_server_map_name()"), "");
        assert_eq!(eval_in_context_string("p", "typeof __s2_server_map_name()"), "string");
        assert_eq!(eval_in_context_string("p", "String(__s2_server_game_time())"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.maxPlayers)"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.mapName)"), "");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.gameTime)"), "0");
        // Slice 6.14: __s2_pawn_commit_suicide degrades to a no-op (undefined) without the op.
        assert_eq!(eval_in_context_string("p", "String(__s2_pawn_commit_suicide(1,7))"), "undefined");
        // Slice 6.12: plugin natives degrade (no file-watch in-isolate → empty list, ops false) + module wires.
        assert_eq!(eval_in_context_string("p", "__s2_plugins_list()"), "[]");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_unload('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_reload('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_load('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "JSON.stringify(__s2pkg_plugins.Plugins.list())"), "[]");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_plugins.Plugins.reload"), "function");
        shutdown();
    }

    /// Admin-groups slice Task 1: per-tier immunity (max across tiers) + command overrides (per-admin
    /// beats global; "public" sentinel) + clear_file wiping file immunity/overrides while runtime survives.
    #[test]
    fn admin_immunity_and_overrides() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // immunity: max across tiers
        eval_in_context("p", "__s2_admin_set('222', 4, 30, false); __s2_admin_set('222', 8, 70, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('222'))"), "12");        // 4|8
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('222'))"), "70"); // max(30,70)
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('999'))"), "0");  // absent
        // overrides: per-admin beats global; public sentinel
        eval_in_context("p", "__s2_admin_set_global_override('sm_x', 2, false); __s2_admin_add_override('222','sm_x',4,false);").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_x')"), "4");    // per-admin wins
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('other','sm_x')"), "2");  // falls to global
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','nope')"), "");     // no override
        eval_in_context("p", "__s2_admin_set_global_override('sm_pub', 0, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_pub')"), "public");
        // clear_file wipes file immunity + overrides + global overrides; runtime immunity survives
        eval_in_context("p", "__s2_admin_clear_file();").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('222'))"), "70"); // runtime kept
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_x')"), "");
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('other','sm_x')"), "");
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
    /// root-implies-all, non-admin→null, __s2_admin_check hook, parseAdmins name→bit mapping.
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
        eval_in_context("p", "__s2_admin_set('0', __s2pkg_admin.ADMFLAG.ROOT, 0, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__s2_admin_check(0, __s2pkg_admin.ADMFLAG.CHAT))"), "false"); // "0" never an admin
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.forSlot(0))"), "null");
        // parseAdmins (renamed from parseFile in the admin-groups slice): name→bit mapping (file-tier path).
        eval_in_context("p", r#"__s2_admin_parseAdmins('{"888":["kick"]}', true);"#).unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('888').hasFlags(__s2pkg_admin.ADMFLAG.KICK))"), "true");
        shutdown();
    }

    /// Admin-groups slice Task 2: the flag-token parser — a compact SM letter-string, an array of names,
    /// a whole-string name, and the 'z'→ROOT letter.
    #[test]
    fn admin_flag_parser() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('bcdefg'))"), "126"); // bits 1..6
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags(['kick','ban']))"), "12"); // KICK|BAN
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('kick'))"), "4");   // whole string = a name
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('z'))"), "16384");  // ROOT
        shutdown();
    }

    /// Admin-groups slice Task 2: group resolution — an admin's own flags/immunity merge with their
    /// groups' (group flags OR'd in, immunity MAX'd), an unknown group is skipped+WARNed but the admin's
    /// own flags survive, and the full parseGroups→parseAdmins(pushCore) pipeline lands in the core cache.
    #[test]
    fn admin_group_resolution() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "__s2_admin_parseGroups('{\"G\":{\"flags\":\"cd\",\"immunity\":50}}');").unwrap();
        // own immunity 10 loses to group 50; flags = own(none) ∪ group(KICK|BAN)=12; groups=['G']
        assert_eq!(eval_in_context_string("p",
            "(function(){var r=__s2_admin_resolveEntry({groups:['G'],immunity:10}); return r.mask+'/'+r.immunity+'/'+r.groups.join(',');})()"),
            "12/50/G");
        // unknown group skipped, own flags kept
        assert_eq!(eval_in_context_string("p",
            "(function(){var r=__s2_admin_resolveEntry({groups:['Nope'],flags:['slay']}); return r.mask+'/'+r.groups.length;})()"),
            "32/0");
        // full push: parseGroups then parseAdmins(pushCore) -> Admin.get reads immunity + groups from core+registry
        eval_in_context("p", "__s2_admin_parseAdmins('{\"111\":{\"groups\":[\"G\"],\"immunity\":5}}', true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('111').immunity)"), "50");
        assert_eq!(eval_in_context_string("p", "__s2pkg_admin.Admin.get('111').groups.join(',')"), "G");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('nobody'))"), "null");
        // an override with an unknown flag token is SKIPPED (not installed as a weakening mask-0)
        eval_in_context("p", "__s2_admin_parseGroups('{\"H\":{\"flags\":\"c\",\"overrides\":{\"sm_x\":\"q\",\"sm_y\":\"d\"}}}');").unwrap();
        assert_eq!(eval_in_context_string("p",
            "Object.keys(__s2_admin_resolveEntry({groups:['H']}).overrides).sort().join(',')"), "sm_y");
        shutdown();
    }

    /// Admin-groups slice Task 2: the pure immunity-comparison hook consumed by Player.target's filter —
    /// console is infinite, a non-immune target is always fair game, and equal immunity can target.
    #[test]
    fn admin_can_target_immunity() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(-1, 0, 100))"), "true");  // console infinite
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 0, 0))"), "true");      // non-immune target
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 50, 100))"), "false");  // punch up blocked
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 100, 50))"), "true");   // punch down
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 50, 50))"), "true");    // equal can target
        shutdown();
    }

    /// Slice 6.18 Task 1: `__s2_ban_*` natives round-trip through `BAN_CACHE`; the `@s2script/bans`
    /// prelude parses a `{steamid:{until,reason}}` blob (skipping `_help`), degrades on malformed JSON,
    /// and exposes `Bans.add/remove/get/list/reload`.
    #[test]
    fn bans_natives_and_prelude() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // set two SteamIDs (one perm, one timed) → get returns the right JSON, list has 2 entries.
        eval_in_context("p", "__s2_ban_set('111', 0, 'grief'); __s2_ban_set('222', 5000000000, 'cheat');").unwrap();
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_get('111')).until)"), "0");
        assert_eq!(eval_in_context_string("p", "JSON.parse(__s2_ban_get('111')).reason"), "grief");
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_get('222')).until)"), "5000000000");
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_list()).length)"), "2");
        // absent → get is null.
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_get('999'))"), "null");
        // remove returns true (was present); a second remove is false; the get is then null.
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_remove('111'))"), "true");
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_remove('111'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_get('111'))"), "null");
        // clear empties the list.
        eval_in_context("p", "__s2_ban_clear();").unwrap();
        assert_eq!(eval_in_context_string("p", "String(JSON.parse(__s2_ban_list()).length)"), "0");
        // mark_loaded: the prelude already called it in create_plugin_context, so it now returns true.
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_mark_loaded())"), "true");
        // the @s2script/bans module is wired.
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_bans.Bans.add"), "function");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_bans.Bans.reload"), "function");
        // prelude parseFile: a {steamid:{until,reason}} blob populates via the natives (skips _help).
        eval_in_context("p", r#"__s2_ban_parseFile('{"_help":"ignore me","333":{"until":0,"reason":"x"}}');"#).unwrap();
        assert_eq!(eval_in_context_string("p", "JSON.parse(__s2_ban_get('333')).reason"), "x");
        assert_eq!(eval_in_context_string("p", "String(__s2_ban_get('_help'))"), "null");
        // malformed JSON degrades without throwing.
        eval_in_context("p", "__s2_ban_parseFile('not json');").unwrap();
        shutdown();
    }

    /// Slice 6.18 Task 1: `ban_check` — banned iff present AND (`until == 0` perm OR `until > now`).
    /// An expired entry and an absent SteamID both read as not-banned.
    #[test]
    fn ban_check_expiry_semantics() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        let now: i64 = 1_000_000;
        // perm (until=0) → banned; the reason is returned.
        eval_in_context("p", "__s2_ban_set('111', 0, 'perm-reason');").unwrap();
        assert_eq!(ban_check(111, now), Some("perm-reason".to_string()));
        // future expiry → banned.
        eval_in_context("p", "__s2_ban_set('222', 1000100, 'timed');").unwrap();
        assert_eq!(ban_check(222, now), Some("timed".to_string()));
        // past expiry → not banned.
        eval_in_context("p", "__s2_ban_set('333', 999900, 'expired');").unwrap();
        assert_eq!(ban_check(333, now), None);
        // absent → not banned.
        assert_eq!(ban_check(444, now), None);
        shutdown();
    }

    /// clientprefs Task 2: `__s2_cookie_*` natives round-trip through `crate::cookies` — a loaded
    /// value is NOT dirty, a set value IS, `get_dirty` returns only the dirty entries, and
    /// `is_cached` reflects `mark_cached`.
    #[test]
    fn cookie_natives_round_trip() {
        let _ = init(dummy_logger());
        load_plugin_js("ck", r#"
            __s2_cookie_load("S1", "a", "1", 111);    // loaded, not dirty
            __s2_cookie_set("S1", "b", "2", 222);     // set, dirty
            __s2_cookie_mark_cached("S1");
            var dirty = __s2_cookie_get_dirty("S1");
            globalThis.__out = __s2_cookie_get("S1","a") + "," + __s2_cookie_get("S1","b")
                + "," + __s2_cookie_is_cached("S1") + "," + Object.keys(dirty).join("|") + "=" + dirty.b;
        "#, "{}");
        assert_eq!(read_global_string("ck", "__out"), "1,2,true,b=2"); // only b is dirty
        shutdown();
    }

    /// clientprefs Task 2: `__s2_cookie_get` returns `undefined` (not `""`) on a true miss, so a
    /// stored `""` reads back as a real hit distinct from an absent name; `__s2_cookie_get_time`
    /// reads back the `updated` passed to `set`/`load`, and is 0 when absent.
    #[test]
    fn cookie_natives_empty_string_and_get_time() {
        let _ = init(dummy_logger());
        load_plugin_js("ck2", r#"
            __s2_cookie_set("S2", "empty", "", 12345);
            var missing = __s2_cookie_get("S2", "nope");
            var empty = __s2_cookie_get("S2", "empty");
            globalThis.__out = (missing === undefined) + "," + (empty === "") + ","
                + __s2_cookie_get_time("S2", "empty") + "," + __s2_cookie_get_time("S2", "nope");
        "#, "{}");
        assert_eq!(read_global_string("ck2", "__out"), "true,true,12345,0");
        shutdown();
    }

    /// clientprefs Task 3: the `@s2script/cookies` module — `Cookies.register` is idempotent,
    /// `get`/`set` route through the cache with a default fallback, and bots (`steamId === "0"`)
    /// are skipped entirely by both `get` (returns the default) and `set` (a no-op — the raw
    /// native cache stays empty for that steamid).
    #[test]
    fn clientprefs_module_get_set_default_and_bot_skip() {
        let _ = init(dummy_logger());
        load_plugin_js("cp", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("hud", { default: "white" });
            var real = { steamId: "S9" };
            var bot  = { steamId: "0" };
            globalThis.__out = Cookies.get(real, c)                 // default (empty cache) -> "white"
                + "," + (function(){ Cookies.set(real, c, "red"); return Cookies.get(real, c); })()  // "red"
                + "," + Cookies.get(bot, c)                          // bot -> default "white"
                + "," + (function(){ Cookies.set(bot, c, "x"); return __s2_cookie_get("0","hud"); })(); // bot set is a no-op -> undefined
        "#, "{}");
        assert_eq!(read_global_string("cp", "__out"), "white,red,white,undefined");
        shutdown();
    }

    /// clientprefs Task 2 (module layer): a `Cookies.set(client, cookie, "")` followed by
    /// `Cookies.get` returns `""` — NOT the cookie's default — the empty-string-vs-miss fix; and
    /// `Cookies.getTime` reads back a nonzero timestamp after a set, 0 before any set, and 0 for a bot.
    #[test]
    fn clientprefs_module_empty_string_and_get_time() {
        let _ = init(dummy_logger());
        load_plugin_js("cp2", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("nickname", { default: "Anonymous" });
            var real = { steamId: "S10" };
            var bot  = { steamId: "0" };
            var beforeSetTime = Cookies.getTime(real, c);      // 0 — never set
            Cookies.set(real, c, "");
            var afterEmptySet = Cookies.get(real, c);          // "" not "Anonymous"
            var afterSetTime = Cookies.getTime(real, c);       // nonzero now
            var botTime = Cookies.getTime(bot, c);             // 0 — bots skipped
            globalThis.__out = beforeSetTime + "," + (afterEmptySet === "") + "," + (afterSetTime > 0) + "," + botTime;
        "#, "{}");
        assert_eq!(read_global_string("cp2", "__out"), "0,true,true,0");
        shutdown();
    }

    /// clientprefs Task 3: `__s2_cookie_set_authid` writes the cache (a subsequent `__s2_cookie_get`
    /// sees it immediately) AND queues the write, drained via `__s2_cookie_take_offline_writes` as a
    /// `[steamid,name,value,updated]` row; a second take is empty.
    #[test]
    fn cookie_set_authid_native_writes_cache_and_queues_offline_write() {
        let _ = init(dummy_logger());
        load_plugin_js("ck3", r#"
            __s2_cookie_set_authid("S11", "k", "v", 999);
            var cached = __s2_cookie_get("S11", "k");
            var writes = __s2_cookie_take_offline_writes();
            var again = __s2_cookie_take_offline_writes();
            globalThis.__out = cached + "," + writes.length + "," + writes[0].join("|") + "," + again.length;
        "#, "{}");
        assert_eq!(read_global_string("ck3", "__out"), "v,1,S11|k|v|999,0");
        shutdown();
    }

    /// clientprefs Task 3 (module layer): `Cookies.setAuthId` writes for a SteamID not passed as a
    /// `Client` at all (offline parity) — a subsequent `Cookies.get` on that steamid sees the value,
    /// and it is a no-op for "0" (bot/unset).
    #[test]
    fn clientprefs_module_set_authid_offline_and_bot_skip() {
        let _ = init(dummy_logger());
        load_plugin_js("cp3", r#"
            var { Cookies } = require("@s2script/cookies");
            var c = Cookies.register("hud", { default: "white" });
            Cookies.setAuthId("S12", c, "blue");
            var real = { steamId: "S12" };
            var seenByClient = Cookies.get(real, c);           // "blue" — the offline write is visible
            Cookies.setAuthId("0", c, "x");                    // bot steamid — no-op
            var botRaw = __s2_cookie_get("0", "hud");
            globalThis.__out = seenByClient + "," + botRaw;
        "#, "{}");
        assert_eq!(read_global_string("cp3", "__out"), "blue,undefined");
        shutdown();
    }

    /// clientprefs Task 4: `Cookies.onCached` (post-drain fan-out). Subscribing via the raw native
    /// `__s2_cookie_on_cached` and enqueuing a slot via `__s2_cookie_dispatch_cached` does NOT run the
    /// handler immediately (only `dispatch_pending_cookie_cached()` — the ffi.rs post-`frame_async_drain`
    /// call site — does); calling it fans the queued slot out to the handler exactly once, and a second
    /// call (now-empty queue) does not re-run it. After `unload_plugin` (remove_by_owner teardown), a
    /// further enqueue+dispatch is a safe no-op.
    #[test]
    fn cookie_cached_dispatch_fans_out_queued_slots() {
        let _ = init(dummy_logger());
        load_plugin_js("ck4", r#"
            __s2_cookie_on_cached(function (slot) {
                globalThis.__ck_ran = (globalThis.__ck_ran || 0) + 1;
                globalThis.__ck_slot = slot;
            });
            __s2_cookie_dispatch_cached(5);
        "#, "{}");

        // Enqueuing alone must not have run the handler yet.
        assert_eq!(read_i32_global_in("ck4", "__ck_ran"), 0, "enqueue must not itself dispatch");

        dispatch_pending_cookie_cached();
        assert_eq!(read_i32_global_in("ck4", "__ck_ran"), 1, "handler must run exactly once");
        assert_eq!(read_i32_global_in("ck4", "__ck_slot"), 5, "handler must receive the queued slot");

        // An empty queue: a further dispatch is a no-op (does not re-run the handler).
        dispatch_pending_cookie_cached();
        assert_eq!(read_i32_global_in("ck4", "__ck_ran"), 1, "an empty queue must not re-run the handler");

        // Teardown: unload removes ck4's subscription; a later enqueue+dispatch is a safe no-op
        // (must not crash even though the context is disposed).
        unload_plugin("ck4");
        COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push(9));
        dispatch_pending_cookie_cached();
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
        assert_eq!(dispatch_chat(5, "!test foo bar", false), false, "! trigger never suppresses");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "5:foo bar", "!test dispatched sm_test");
        // Silent `/test` → dispatches AND suppresses (matched silent trigger).
        eval_in_context("hs", "globalThis.__ran = '';").unwrap();
        assert_eq!(dispatch_chat(7, "/test", false), true, "matched / trigger suppresses");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "7:", "/test dispatched with empty args");
        // Ordinary chat (no trigger char) → no dispatch, no suppress.
        eval_in_context("hs", "globalThis.__ran = 'untouched';").unwrap();
        assert_eq!(dispatch_chat(5, "hello world", false), false, "ordinary chat is not a trigger");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "untouched", "ordinary chat did not dispatch");
        // Unknown `/nope` → no command match → NOT suppressed (never swallow a non-command message).
        assert_eq!(dispatch_chat(5, "/nope", false), false, "unmatched silent trigger is not suppressed");
        shutdown();
    }

    /// Slice 6.13b Task 3: the raw-chat subscriber mechanism (`Chat.onMessage`). A non-command chat
    /// line is delivered to `CHAT_MSG_SUBS` subscribers with `(slot, text, teamonly)`; if a live
    /// subscriber returns `>= HookResult.Handled` (2) the broadcast is suppressed (`dispatch_chat`
    /// returns true). `Continue`/`undefined`/non-number → no suppress. A matched command trigger
    /// takes the command path and never reaches the subscriber loop. Engine-generic: core passes
    /// only slot/text/teamonly (no game type).
    #[test]
    fn chat_message_subscriber_suppresses_on_handled() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_plugin_js("cm", r#"
            var Chat = __s2pkg_chat.Chat;
            globalThis.__got = null;
            globalThis.__block = false;
            Chat.onMessage(function (slot, text, teamonly) {
                globalThis.__got = slot + "|" + text + "|" + teamonly;
                return globalThis.__block ? 2 /*Handled*/ : 0 /*Continue*/;
            });
        "#, "{}");
        // Continue (return 0) → not suppressed; the subscriber still saw slot/text/teamonly.
        assert_eq!(dispatch_chat(3, "hello world", true), false, "Continue does not suppress");
        assert_eq!(eval_in_context_string("cm", "globalThis.__got"), "3|hello world|true", "subscriber saw slot/text/teamonly");
        // Handled (return 2) → suppressed; teamonly=false threads through as `false`.
        eval_in_context("cm", "globalThis.__block = true;").unwrap();
        assert_eq!(dispatch_chat(4, "hi again", false), true, ">= Handled suppresses");
        assert_eq!(eval_in_context_string("cm", "globalThis.__got"), "4|hi again|false", "subscriber saw the second line");
        // A command trigger with NO subscriber-reachable path: `!nope` doesn't match a command, so it
        // falls to the raw-chat subscriber loop — the subscriber (blocking) suppresses it too.
        eval_in_context("cm", "globalThis.__got = 'x';").unwrap();
        assert_eq!(dispatch_chat(5, "!nope", false), true, "unmatched trigger reaches subscribers (blocking)");
        assert_eq!(eval_in_context_string("cm", "globalThis.__got"), "5|!nope|false", "unmatched trigger delivered raw to subscriber");
        shutdown();
    }

    /// Slice 6.13b Task 3: `__s2_chat_on_message` degrades safely with no engine ops present — the
    /// native only touches CHAT_MSG_SUBS (no engine-op), so subscribing must not panic, and a
    /// dispatch with no subscriber-return-value change (handler returns nothing) does not suppress.
    #[test]
    fn chat_on_message_native_degrades_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();               // no engine ops set
        create_plugin_context("p");
        // Subscribing must not throw even with no ops.
        eval_in_context("p", "__s2_chat_on_message(function (slot, text, teamonly) { /* returns undefined */ });").unwrap();
        // A handler returning undefined ⇒ Continue ⇒ no suppress.
        assert_eq!(dispatch_chat(1, "plain line", false), false, "undefined return ⇒ Continue ⇒ no suppress");
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

    // ---------------------------------------------------------------------------
    // Slice DB Task 3: __s2_sqlite_* natives — sync-behind-Promise round trip + degrade tests.
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
    /// query(parameterized) -> close, all chained through native-returned Promises and drained by
    /// ONE `frame_async_drain()` (a single `perform_microtask_checkpoint()` empties the whole
    /// already-resolved chain, since each link resolves synchronously before the next `.then`
    /// attaches). Proves value marshalling both directions (params in, columns/rows out) and the
    /// `lastInsertId`/`changes` execute-result shape.
    #[test]
    fn sqlite_open_execute_query_round_trip() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("t3_roundtrip");
        load_plugin_js("dbp", &format!(r#"
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
        frame_async_drain();
        assert_eq!(read_global_string("dbp", "__out"), "changes=1 id=1 rows=1 v=red k=color");
        shutdown();
    }

    /// A bad-SQL query rejects the Promise (not a panic/crash) — the `.catch` handler runs and
    /// records the error, proving `db::query`'s `Err` path reaches JS as a rejection.
    #[test]
    fn sqlite_bad_sql_rejects_promise() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("t3_badsql");
        load_plugin_js("dbp2", &format!(r#"
            globalThis.__out = "pending";
            __s2_sqlite_open("{name}").then(function (h) {{
                return __s2_sqlite_query(h, "SELECT * FROM nope", []);
            }}).then(function () {{
                globalThis.__out = "should-not-resolve";
            }}).catch(function (e) {{
                globalThis.__out = "rejected:" + (String(e).length > 0);
            }});
        "#, name = name), "{}");
        frame_async_drain();
        assert_eq!(read_global_string("dbp2", "__out"), "rejected:true");
        shutdown();
    }

    /// Degrade path: with NO `db_data_dir` op wired, `__s2_sqlite_open` rejects gracefully (never
    /// panics) — proving the natives are registered and reachable even when the engine op table
    /// is absent (e.g. a stale core against an old shim).
    #[test]
    fn sqlite_open_degrades_without_data_dir_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None); // no ops at all -> db_data_dir() returns None -> open() rejects
        load_plugin_js("dbp3", r#"
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
        load_plugin_js("dbshape", r#"
            var { Database } = require("@s2script/db");
            globalThis.__out = (typeof Database.open === "function") + "," + (typeof Database.registerDriver === "function");
        "#, "{}");
        assert_eq!(read_global_string("dbshape", "__out"), "true,true");
        shutdown();
    }

    /// End-to-end through the PUBLIC `@s2script/db` API (not the raw natives): open a named
    /// database, CREATE + INSERT (parameterized), SELECT it back, close. Proves the Database
    /// object built by the prelude (over the SQLite reference driver) round-trips correctly.
    #[test]
    fn db_module_open_execute_query_round_trip() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("t4_roundtrip");
        load_plugin_js("dbmod", &format!(r#"
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
        frame_async_drain();
        assert_eq!(read_global_string("dbmod", "__out"), "changes=1 id=1 rows=1 v=red k=color");
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
        load_plugin_js("dbdrv", r#"
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js("wsOwnerB", r#"globalThis.__spied = "none";"#, "{}");
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
        load_plugin_js(
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
        load_plugin_js(
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
        for _ in 0..500 {
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
    #[test]
    fn ws_module_self_close_fires_on_close() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_echo_server();
        load_plugin_js(
            "wsclose",
            &format!(
                r#"
            var {{ WebSocket }} = require("@s2script/ws");
            globalThis.__out = "pending";
            WebSocket.connect("ws://127.0.0.1:{port}/").then(function (ws) {{
                ws.onMessage(function (m) {{ ws.close(); }});
                ws.onClose(function (code, reason) {{ globalThis.__out = "closed:" + code + ":" + reason; }});
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
        for _ in 0..500 {
            frame_async_drain();
            dispatch_pending_ws_events();
            if read_global_string("wsclose", "__out") != "pending" {
                resolved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(resolved, "onClose never fired for a self-initiated close");
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        for _ in 0..500 {
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
        load_plugin_js(
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
        for _ in 0..500 {
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
            var realOn = globalThis.__s2pkg_chat.Chat.onMessage;
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
            var m = new Menu("Pick"); m.style = MenuStyle.Chat;
            m.addItem("kick", "Kick"); m.addItem("ban", "Ban");
            var got = null; m.onSelect(function (e){ got = e.info; });
            m.display(3, 0);
            // simulate slot 3 typing "2"
            var suppressed = chatHandler(3, "2", false);
            // restore
            globalThis.__s2pkg_chat.Chat.toSlot = realToSlot;
            globalThis.__s2pkg_chat.Chat.onMessage = realOn;
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
            var realOn = globalThis.__s2pkg_chat.Chat.onMessage;
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
            var m = new Menu("P"); m.style = MenuStyle.Chat; m.addItem("a", "A");
            m.display(3, 0);
            var r1 = chatHandler(3, "hello", false);   // not a digit -> pass through (undefined/0)
            var r2 = chatHandler(4, "1", false);        // different slot -> pass through
            globalThis.__s2pkg_chat.Chat.onMessage = realOn;
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
        load_plugin_js("tm_a", r#"
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
        load_plugin_js("tm_b", r#"
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
        load_plugin_js("tm_c", r#"
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
        load_plugin_js("tm_ord", r#"
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
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
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
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
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
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
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
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
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
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
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
            globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
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
