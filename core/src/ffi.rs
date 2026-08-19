use crate::multiplexer::Phase;
use crate::dispatch::Delivery;
use crate::v8host::{self, HookRequestFn, LogFn, S2EngineOps};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;
use std::ffi::CStr;

/// Returned by a NOTIFY dispatch entry when the JS isolate was already borrowed (a re-entrant
/// dispatch): core delivered NOTHING and the caller must queue a replay for the next `GameFrame`.
/// Any other value means the entry was handled (delivered, or no subscribers) — do not queue.
///
/// Must stay byte-identical to `S2_DISPATCH_DEFERRED` in `shim/include/s2script_core.h`
/// (`scripts/ci-native.sh` gates the pair).
///
/// **Why `-1000`.** It is outside every value any dispatch entry can return today: `HookResult` is
/// `0..=3`, the boolean entries are `0..=1`, the `catch_unwind` fallbacks are `0` (and `-99` for
/// `game_frame` alone), and the header's "unavailable" idiom is `-1`. Deliberately far outside, so
/// an off-by-one or a widened `HookResult` cannot creep into it.
///
/// **Why negative.** Shim code that reads a dispatch result is shaped `if (r >= 2) MRES_SUPERCEDE`.
/// A negative sentinel fails that test, so a call site that forgets to check for it degrades to
/// TODAY's behaviour — "engine proceeds unhooked", the documented safe direction — instead of
/// superseding something it never meant to. A large positive sentinel would fail closed the wrong
/// way. (It is still C-truthy, which is why no entry whose result is consumed as a boolean may ever
/// be made deferrable — none are.)
pub const S2_DISPATCH_DEFERRED: c_int = -1000;

/// Map a `Delivery` onto the C-ABI dispatch result. `Delivered` is `0` — the value every one of
/// these entries returned (as `void`) before the deferred-dispatch queue existed.
#[inline]
fn deferral_code(d: Delivery) -> c_int {
    match d {
        Delivery::Deferred => S2_DISPATCH_DEFERRED,
        Delivery::Delivered => 0,
    }
}

#[no_mangle]
pub extern "C" fn s2script_core_init(
    logger: Option<LogFn>,
    request_hook: Option<HookRequestFn>,
    ops: *const S2EngineOps,
) -> c_int {
    catch_unwind(|| {
        crate::crash::panic_hook::install();
        v8host::set_hook_request(request_hook);
        // Copy the engine-ops table by value: the shim passes a pointer to a stack-local struct
        // that dies when its Load() returns, so we must NOT retain the pointer.  Null → no ops
        // (every engine native degrades to a safe miss).  Stored before the logger guard so the
        // ops are in place even if init bails.
        let ops = if ops.is_null() { None } else { Some(unsafe { *ops }) };
        v8host::set_engine_ops(ops);
        let Some(logger) = logger else { return -2 };
        match v8host::init(logger) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
    .unwrap_or(-99)
}

#[no_mangle]
pub extern "C" fn s2script_core_eval(src: *const c_char) -> c_int {
    catch_unwind(|| {
        if src.is_null() {
            return -2;
        }
        let s = match unsafe { std::ffi::CStr::from_ptr(src) }.to_str() {
            Ok(s) => s,
            Err(_) => return -3,
        };
        match v8host::eval(s) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
    .unwrap_or(-99)
}

#[no_mangle]
pub extern "C" fn s2script_core_dispatch_game_frame(
    phase: c_int,
    simulating: c_int,
    first: c_int,
    last: c_int,
) -> c_int {
    catch_unwind(|| {
        let phase = if phase == 0 { Phase::Pre } else { Phase::Post };
        // E1: reconcile the entity books before any JS runs this frame (one-shot,
        // armed at map start, fires on the first simulating frame).
        if phase == Phase::Pre { v8host::entity_repair_sweep_if_armed(simulating != 0); }
        let out = v8host::dispatch_onframe(phase, simulating != 0, first != 0, last != 0);
        if phase == Phase::Post {
            v8host::frame_async_drain(); // Post: resolve async + microtask checkpoint
            crate::cookies::dispatch_pending_cached(); // Post, HOST free: fan out queued Cookies.onCached
            crate::ws::dispatch_pending_events(); // Post, HOST free: fan out queued WebSocket on* events
            crate::net::dispatch_pending_events(); // Post, HOST free: fan out queued net (TCP/UDP) events
            v8host::dispatch_pending_topmenu_select(); // Post, HOST free: fan out queued TopMenu.select
            crate::loader::poll_plugins(); // Post: scan /plugins for .s2sp changes (throttled)
        }
        out.result as c_int
    })
    .unwrap_or(-99)
}

#[no_mangle]
pub extern "C" fn s2script_core_shutdown() {
    let _ = catch_unwind(|| v8host::shutdown());
}

/// Shim → core: called by the shim's `IGameEventListener2` when an event fires (the shim has already
/// stashed the live `IGameEvent*` for the accessor engine-ops). Dispatches to the name's JS subscribers.
///
/// `catch_unwind`-wrapped; null pointer and invalid UTF-8 degrade to a no-op (never panic across
/// the FFI boundary per spec §6).
///
/// Returns `S2_DISPATCH_DEFERRED` iff there were subscribers AND the isolate was already borrowed
/// (a re-entrant dispatch): the shim must `DuplicateEvent` the live `IGameEvent` and replay via
/// `s2script_core_replay_game_event` at the next `GameFrame`. Every degrade path (null, bad UTF-8,
/// panic) returns `0` — a malformed call is not deferrable.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_game_event(name: *const c_char) -> c_int {
    catch_unwind(|| {
        if name.is_null() { return 0; }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0 };
        deferral_code(crate::events::dispatch_game_event(name_str))
    })
    .unwrap_or(0)
}

/// Shim → core: replay a game-event dispatch that was deferred one frame earlier. The JS fan-out
/// and NOTHING else — this entry runs no bookkeeping, which is what makes it safe to run late.
/// The shim has pointed `s_currentEvent` at its `DuplicateEvent` copy for the duration of the call.
///
/// A replay that itself returns `S2_DISPATCH_DEFERRED` must be DROPPED with a named log, never
/// re-queued: the drain runs with the isolate provably free, so it can only mean a bug, and
/// re-queueing would spin across frames.
#[no_mangle]
pub extern "C" fn s2script_core_replay_game_event(name: *const c_char) -> c_int {
    catch_unwind(|| {
        if name.is_null() { return 0; }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0 };
        deferral_code(crate::events::replay_game_event(name_str))
    })
    .unwrap_or(0)
}

/// Shim → core: called by the shim's six client-lifecycle SourceHooks (Clients sub-project) with the
/// lifecycle event `name` ("connect"/"putinserver"/"active"/"fullyconnect"/"disconnect"/
/// "settingschanged") + the client's `slot`. Notify-only: dispatches to the name's `Clients.on*` JS
/// subscribers. `catch_unwind`-wrapped; null pointer / invalid UTF-8 degrade to a no-op (never panic
/// across the FFI boundary per spec §6). Returns `S2_DISPATCH_DEFERRED` on a re-entrant dispatch
/// (see `s2script_core_dispatch_game_event`); the shim replays it from the scalars it already holds.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_client_event(name: *const c_char, slot: c_int) -> c_int {
    catch_unwind(|| {
        if name.is_null() { return 0; }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0 };
        deferral_code(crate::client::dispatch_client_event(name_str, slot as i32))
    })
    .unwrap_or(0)
}

/// Shim → core: replay a deferred client-lifecycle dispatch. JS fan-out ONLY — the breadcrumb
/// player count and the voice slot-reuse clear already ran, unconditionally, at dispatch time and
/// must NEVER be replayed (they are not idempotent).
#[no_mangle]
pub extern "C" fn s2script_core_replay_client_event(name: *const c_char, slot: c_int) -> c_int {
    catch_unwind(|| {
        if name.is_null() { return 0; }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0 };
        deferral_code(crate::client::replay_client_event(name_str, slot as i32))
    })
    .unwrap_or(0)
}

/// Shim → core: the INetworkServerService::StartupServer POST hook reports a map start with the
/// live map name. Notify-only: dispatches to the `Server.onMapStart` JS subscribers.
/// `catch_unwind`-wrapped; a null pointer degrades to "" (never panic across the FFI boundary).
/// Returns `S2_DISPATCH_DEFERRED` on a re-entrant dispatch (rare — StartupServer is outside a
/// frame, so the isolate is normally free — but a plugin-declared engine call can reach a
/// changelevel from JS).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_map_start(map: *const c_char) -> c_int {
    catch_unwind(|| {
        let map_str = if map.is_null() { "" } else {
            (unsafe { CStr::from_ptr(map) }).to_str().unwrap_or("")
        };
        // E1: the implicit entity epoch — clear the books UNCONDITIONALLY before the JS
        // dispatch (which early-returns when no Server.onMapStart subscribers exist),
        // and arm the one-shot repair sweep (consumed at the next simulating frame).
        crate::entity_live::clear_for_map_transition();
        deferral_code(v8host::dispatch_map_start(map_str))
    })
    .unwrap_or(0)
}

/// Shim → core: replay a deferred map-start dispatch. JS fan-out ONLY.
///
/// THE reason the split exists: replaying `entity_live::clear_for_map_transition()` would wipe the
/// entity books a frame INTO the new map and re-arm the repair sweep, killing every `EntityRef`
/// minted since map start. This entry must never touch the books.
#[no_mangle]
pub extern "C" fn s2script_core_replay_map_start(map: *const c_char) -> c_int {
    catch_unwind(|| {
        let map_str = if map.is_null() { "" } else {
            (unsafe { CStr::from_ptr(map) }).to_str().unwrap_or("")
        };
        deferral_code(v8host::replay_map_start(map_str))
    })
    .unwrap_or(0)
}

/// Shim → core: the CGameRulesGameSystem::OnPrecacheResource manual hook reports that the session
/// resource manifest is being built (Sound slice). The live IResourceManifest* is stashed
/// shim-side around this call for the `sound_precache_add` op (block-scoped — cleared when the
/// hook returns). Notify-only: dispatches to the `Sound.onPrecache` JS subscribers.
/// `catch_unwind`-wrapped (never panic across the FFI boundary).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_precache() {
    let _ = catch_unwind(|| {
        v8host::dispatch_precache();
    });
}

/// Shim → core: an IEntityListener callback (create/spawn/delete) with the entity's packed
/// CEntityHandle (ToInt()) + class name. Notify-only: dispatches to the `Entity.on{Create,Spawn,Delete}`
/// JS subscribers. `catch_unwind`-wrapped; null/invalid-UTF-8 degrade to a no-op.
///
/// Returns `S2_DISPATCH_DEFERRED` on a re-entrant dispatch (e.g. a plugin's own synchronous
/// `createEntity` from inside a handler). **The books feed below is NOT part of the replay** — the
/// shim queues `s2script_core_replay_entity_event`, which is the JS fan-out alone.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_entity_event(kind: *const c_char, class_name: *const c_char, handle: c_int) -> c_int {
    catch_unwind(|| {
        if kind.is_null() || class_name.is_null() { return 0; }
        let Ok(kind_str) = (unsafe { CStr::from_ptr(kind) }).to_str() else { return 0 };
        let Ok(class_str) = (unsafe { CStr::from_ptr(class_name) }).to_str() else { return 0 };
        // THE BOOKS FEED (north-star §3.1, critical): unconditional, BEFORE and
        // independent of the JS mux dispatch below — dispatch_entity_event early-returns
        // when no subscribers exist and skips under the HOST try_borrow_mut re-entrancy
        // guard, but a create/delete witnessed while JS is on-stack must still update
        // the books (e.g. a plugin's own synchronous createEntity).
        let decoded = if handle == -1 { None } else {
            let (idx, ser) = crate::entity::decode_handle(handle as u32);
            if idx >= 0 && ser >= 0 { Some((idx, ser)) } else { None }
        };
        if let Some((idx, ser)) = decoded {
            match kind_str {
                "create" => { crate::entity_live::on_created(idx, ser); }
                "spawn"  => { crate::entity_live::on_spawned(idx, ser); }
                _ => {}
            }
        }
        let delivery = crate::entity::dispatch_entity_event(kind_str, class_str, handle as i32);
        // Delete is booked AFTER the dispatch: an onDelete handler may still resolve
        // the dying entity (slot-validated stage 2 stays the guard); the moment this
        // FFI entry returns, the books say dead — fail-closed for any stashed ref.
        // A DEFERRED delete is booked here all the same, which is why a replayed "delete"
        // hands the handler a null EntityRef (contract §6.2, accepted).
        if let Some((idx, ser)) = decoded {
            if kind_str == "delete" { crate::entity_live::on_deleted(idx, ser); }
        }
        deferral_code(delivery)
    })
    .unwrap_or(0)
}

/// Shim → core: replay a deferred entity-lifecycle dispatch. JS fan-out ONLY.
///
/// The books feed (`entity_live::on_created`/`on_spawned`/`on_deleted`) ran unconditionally at
/// dispatch time and must NEVER run here: a replayed `"create"` would RESURRECT a since-deleted
/// entity in the books, because `on_created` is an unconditional insert. No pointer is involved —
/// `handle` is a packed `CEntityHandle` int, resolved books-first as always.
#[no_mangle]
pub extern "C" fn s2script_core_replay_entity_event(kind: *const c_char, class_name: *const c_char, handle: c_int) -> c_int {
    catch_unwind(|| {
        if kind.is_null() || class_name.is_null() { return 0; }
        let Ok(kind_str) = (unsafe { CStr::from_ptr(kind) }).to_str() else { return 0 };
        let Ok(class_str) = (unsafe { CStr::from_ptr(class_name) }).to_str() else { return 0 };
        deferral_code(crate::entity::replay_entity_event(kind_str, class_str, handle as i32))
    })
    .unwrap_or(0)
}

/// Shim → core: called by the FireEvent Pre hook (Slice 5D.3). Runs the PRE subscribers for `name`
/// (s_currentEvent is set + mutable during the call). Returns 1 to suppress the client broadcast
/// (a pre-hook returned Handled/Stop), else 0.
///
/// `catch_unwind`-wrapped; null pointer and invalid UTF-8 degrade to 0 (never panic across the
/// FFI boundary per spec §6).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_game_event_pre(name: *const c_char) -> c_int {
    if name.is_null() { return 0; }
    let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0; };
    std::panic::catch_unwind(|| crate::events::dispatch_game_event_pre(name_str)).unwrap_or(0)
}

/// Slice 6.6 Stage 2: run the Damage.onPre subscribers over the current damage info. The shim has already
/// set the current CTakeDamageInfo pointer; handlers read/modify it in place via the damage_* ops.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_damage() {
    let _ = catch_unwind(|| v8host::dispatch_damage());
}

/// Usercmd primitive Task 2/3: called by the (Task 3) shim's per-tick input-processing detour, once per player per
/// batched tick, with the firing player's `slot`. Runs the `UserCmd.onRun` subscribers SYNCHRONOUSLY
/// over the shim's current `CUserCmd` (read/modified in place via the Task-3 `usercmd_read`/`_write`
/// ops) and returns the collapsed `HookResult` (0 Continue .. 3 Stop) — the shim skips/blocks the
/// original input for that cmd when the result is >= Handled (2), mirroring
/// `s2script_core_dispatch_output`'s supersede convention.
///
/// `catch_unwind`-wrapped and FAIL-OPEN (-> 0 Continue on a panic): a core bug must never block a
/// player's input it didn't mean to (mirrors `s2script_core_dispatch_output`'s fail-open shape).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_usercmd(slot: c_int) -> c_int {
    catch_unwind(|| v8host::dispatch_usercmd(slot)).unwrap_or(0)
}

/// Shim → core: a compiled inbound-hook thunk fired. `hook_id` is the shim hook slot (a compile-time
/// constant inside that thunk, and an id core itself handed out at registration); `arg_view` is an
/// OPAQUE pointer to the thunk's own stack-frame arg view, which core reads and writes ONLY through
/// the `hook_read_*`/`hook_write_*` ops and never dereferences.
///
/// Returns the collapsed `HookResult` (0 Continue .. 3 Stop). The thunk suppresses the original
/// engine call entirely at >= Handled (2).
///
/// **NEVER `S2_DISPATCH_DEFERRED`.** This entry is not deferrable and must never be queued for
/// replay: `arg_view` is a STACK FRAME that dies when the thunk returns, so a dispatch replayed a
/// frame later would hand JS a dead frame and every accessor would fail. `dispatch_hook` routes
/// through `fan_out_collapsing`, which discards `Delivery` by construction — a re-entrant dispatch
/// returns Continue (the engine proceeds unhooked) rather than asking anyone to replay it.
///
/// `catch_unwind`-wrapped and FAIL-OPEN (→ 0 Continue on a panic): a core bug must never suppress
/// engine behaviour it did not mean to.
///
/// This symbol is declared `__attribute__((weak))` on the shim side, so a shim paired with an older
/// core degrades to Continue with a boot WARN instead of failing `dlopen` and taking the whole addon
/// down. The cost of weak is that a MISSPELLING here would degrade silently rather than failing the
/// link — which is why `scripts/check-shim-symbols.sh` asserts every weak-undefined `s2script_core_*`
/// the shim references is defined in `libs2script_core.so`.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_hook(
    hook_id: c_int,
    arg_view: *mut std::ffi::c_void,
) -> c_int {
    catch_unwind(|| v8host::dispatch_hook(hook_id, arg_view)).unwrap_or(0)
}

/// Shim → core: the Post spectator mux for a returning inbound hook. `skipped` is 1 when Pre
/// suppressed the original. Notify-only: the return is unused. Weak on the shim side.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_hook_post(
    hook_id: c_int,
    arg_view: *mut std::ffi::c_void,
    skipped: c_int,
) -> c_int {
    catch_unwind(|| v8host::dispatch_hook_post(hook_id, arg_view, skipped != 0)).unwrap_or(0);
    0
}

/// Shim → core: a cvar's value changed. Called from the shim's ONE `ICvar` global change callback.
///
/// NOTIFY-ONLY — the engine has ALREADY applied the value, so there is nothing to veto: this entry
/// returns `S2_DISPATCH_DEFERRED` on a re-entrant dispatch (never a `HookResult`), and the shim
/// replays it next frame from its own copies of the three strings. Test the result EXACTLY against
/// the sentinel; it is not a collapsed hook result and `-1000` is C-truthy.
///
/// `catch_unwind`-wrapped; a null pointer or invalid UTF-8 degrades to `0` (delivered/no-op) and is
/// never deferrable — there would be nothing meaningful to replay.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_cvar_change(
    name: *const c_char,
    new_value: *const c_char,
    old_value: *const c_char,
) -> c_int {
    catch_unwind(|| {
        if name.is_null() { return 0; }
        let Ok(n) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0 };
        let nv = if new_value.is_null() { "" } else {
            (unsafe { CStr::from_ptr(new_value) }).to_str().unwrap_or("") };
        let ov = if old_value.is_null() { "" } else {
            (unsafe { CStr::from_ptr(old_value) }).to_str().unwrap_or("") };
        deferral_code(crate::v8host::dispatch_cvar_change(n, nv, ov))
    })
    .unwrap_or(0)
}

/// Shim → core: replay a deferred cvar-change notification. JS fan-out ONLY (this path carries no
/// bookkeeping; the entry exists so the shim's drain has one uniform `replay_*` vocabulary).
#[no_mangle]
pub extern "C" fn s2script_core_replay_cvar_change(
    name: *const c_char,
    new_value: *const c_char,
    old_value: *const c_char,
) -> c_int {
    catch_unwind(|| {
        if name.is_null() { return 0; }
        let Ok(n) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0 };
        let nv = if new_value.is_null() { "" } else {
            (unsafe { CStr::from_ptr(new_value) }).to_str().unwrap_or("") };
        let ov = if old_value.is_null() { "" } else {
            (unsafe { CStr::from_ptr(old_value) }).to_str().unwrap_or("") };
        deferral_code(crate::v8host::replay_cvar_change(n, nv, ov))
    })
    .unwrap_or(0)
}

/// Shim → core: called by the `FireOutputInternal` detour (entity-I/O slice) with the firing entity's
/// classname, the output name, packed activator/caller `CEntityHandle` ints (-1 = none), the output's
/// value as a string, and the delay. Runs the matching `Entity.onOutput` subscribers SYNCHRONOUSLY and
/// returns the collapsed `HookResult` (0 Continue .. 3 Stop) — the shim supersedes (suppresses) the
/// original `FireOutputInternal` call when the result is >= Handled (2).
///
/// NOT deferrable: the engine consumes this answer synchronously, so it never returns
/// `S2_DISPATCH_DEFERRED` and a re-entrant dispatch keeps today's graceful skip (→ 0 Continue).
///
/// `catch_unwind`-wrapped and FAIL-OPEN (-> 0 Continue on a panic or invalid UTF-8): a core bug must
/// never suppress an output it didn't mean to, mirroring `s2script_core_ban_check`'s fail-open shape.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_output(
    classname: *const c_char,
    output: *const c_char,
    act_handle: c_int,
    caller_handle: c_int,
    value: *const c_char,
    delay: f32,
) -> c_int {
    catch_unwind(|| {
        if classname.is_null() || output.is_null() { return 0; }
        let Ok(classname_str) = (unsafe { CStr::from_ptr(classname) }).to_str() else { return 0; };
        let Ok(output_str) = (unsafe { CStr::from_ptr(output) }).to_str() else { return 0; };
        let value_str = if value.is_null() {
            ""
        } else {
            match (unsafe { CStr::from_ptr(value) }).to_str() { Ok(s) => s, Err(_) => "" }
        };
        v8host::dispatch_output(classname_str, output_str, act_handle as i32, caller_handle as i32, value_str, delay)
    })
    .unwrap_or(0)
}

/// UserMessage-interception: shim → core on a bitmap-hit PostEventAbstract. `name` = the intercepted
/// message's `GetUnscopedName()` (the canonical dispatch key), `id` = its `m_MessageId`. Runs the
/// matching `UserMessages.onPre` subscribers SYNCHRONOUSLY over the shim's current (block-scoped)
/// intercepted message and returns the collapsed `HookResult` (0..3); >= Handled(2) tells the shim to
/// MRES_SUPERCEDE the send. `catch_unwind`-wrapped and FAIL-OPEN (→ 0 Continue on a panic or invalid
/// UTF-8): a core bug must never suppress a message it didn't mean to (mirrors
/// `s2script_core_dispatch_output`'s fail-open shape).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_usermsg(name: *const c_char, id: c_int) -> c_int {
    catch_unwind(|| {
        if name.is_null() { return 0; }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0; };
        crate::usermsg::dispatch(name_str, id as i32)
    })
    .unwrap_or(0)
}

/// C-ABI entry point the shim's ConCommand trampoline calls when a registered command fires.
/// `name` = command name (Arg(0)), `slot` = CPlayerSlot::Get() (-1 for server console),
/// `args` = CCommand::ArgS() (everything after the name).
///
/// `catch_unwind`-wrapped; null pointer and invalid UTF-8 degrade to a no-op (never panic
/// across the FFI boundary per spec §6).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_concommand(
    name: *const c_char,
    slot: c_int,
    args: *const c_char,
) {
    let _ = catch_unwind(|| {
        if name.is_null() || args.is_null() { return; }
        let name_str = match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(s) => s,
            Err(_) => return,
        };
        let args_str = match unsafe { CStr::from_ptr(args) }.to_str() {
            Ok(s) => s,
            Err(_) => return,
        };
        crate::commands::dispatch_concommand(
            name_str,
            slot as i32,
            args_str,
            crate::commands::ReplySource::from_slot(slot as i32),
        );
    });
}

/// C-ABI entry point: dispatch a player chat line for command triggers (Slice 6.11b).  The shim's
/// Host_Say detour calls this with the speaker's `slot` + the raw message text (CCommand::Arg(1)).
///
/// Returns 1 if the caller should SUPPRESS the chat broadcast (a matched SILENT `/` trigger, OR a raw
/// `ctx.clients.onSay` subscriber that returned >= Handled), else 0 (the public `!` trigger and ordinary
/// chat with no blocking subscriber show).  `teamonly` (0/1) is threaded to the unmatched-say subscribers.
/// `catch_unwind`-wrapped; a null pointer / invalid UTF-8 degrades to 0 (no suppress, never panic
/// across the FFI boundary per spec §6).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_chat(slot: c_int, text: *const c_char, teamonly: c_int) -> c_int {
    catch_unwind(|| {
        if text.is_null() { return 0; }
        let text_str = match unsafe { CStr::from_ptr(text) }.to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        if crate::commands::dispatch_chat(slot as i32, text_str, teamonly != 0) { 1 } else { 0 }
    })
    .unwrap_or(0)
}

/// C-ABI entry point: dispatch a player's CONSOLE command (Slice 6.11c). The shim's ClientCommand hook
/// calls this with the speaker's `slot`, the command `name` (CCommand::Arg(0)) + `args` (ArgS()).
///
/// Returns 1 iff a registered s2script command matched + was dispatched (the caller then SUPERCEDEs the
/// engine's own handling). `catch_unwind`-wrapped; null / invalid UTF-8 degrades to 0 (not handled).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_client_command(slot: c_int, name: *const c_char, args: *const c_char) -> c_int {
    catch_unwind(|| {
        if name.is_null() || args.is_null() { return 0; }
        let name_str = match unsafe { CStr::from_ptr(name) }.to_str() { Ok(s) => s, Err(_) => return 0 };
        let args_str = match unsafe { CStr::from_ptr(args) }.to_str() { Ok(s) => s, Err(_) => return 0 };
        if crate::commands::dispatch_client_command(slot as i32, name_str, args_str) { 1 } else { 0 }
    })
    .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn s2script_core_dispatch_command_listeners(slot: c_int, name: *const c_char, args: *const c_char) -> c_int {
    catch_unwind(|| {
        if name.is_null() || args.is_null() { return 0; }
        let name_str = match unsafe { CStr::from_ptr(name) }.to_str() { Ok(s) => s, Err(_) => return 0 };
        let args_str = match unsafe { CStr::from_ptr(args) }.to_str() { Ok(s) => s, Err(_) => return 0 };
        if crate::commands::dispatch_command_listeners(slot as i32, name_str, args_str) { 1 } else { 0 }
    })
    .unwrap_or(0)
}

/// C-ABI entry point: is `xuid` currently banned? (Slice 6.18). The shim's `ClientConnect` hook calls
/// this with the connecting player's SteamID64 (`xuid`) and the current unix time (`now`). Returns 1 iff
/// banned (perm or unexpired); on a hit, the ban reason is bounded-copied into `out_reason` (NUL-terminated,
/// truncated to `cap-1`) for the shim's log line. Panic → 0 (FAIL-OPEN: a core bug must never wedge all
/// connections; a banned player merely connecting through beats every connection being rejected).
#[no_mangle]
pub extern "C" fn s2script_core_ban_check(xuid: u64, now: i64, out_reason: *mut c_char, cap: c_int) -> c_int {
    std::panic::catch_unwind(|| {
        match crate::bans::ban_check(xuid, now) {
            Some(reason) => {
                if !out_reason.is_null() && cap > 1 {
                    let bytes = reason.as_bytes();
                    let n = std::cmp::min(bytes.len(), (cap as usize) - 1);
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_reason as *mut u8, n);
                        *out_reason.add(n) = 0;
                    }
                }
                1
            }
            None => 0,
        }
    })
    .unwrap_or(0)
}

/// C-ABI entry point retained for shim link-compatibility.  Now a degrade-safe no-op: game JS
/// is provided to core via `s2script_core_register_package` instead (see below).
/// `catch_unwind`-wrapped (no panic may cross the FFI boundary — spec §6).
#[no_mangle]
pub extern "C" fn s2script_core_load_cs2(_path: *const c_char) {
    // No-op: the per-plugin require model (register_injected_package) supersedes this entry.
}

/// Register a game-package JS source under `name` so core can inject it per-plugin-context
/// without baking game JS into the core binary at compile time.
///
/// Called by the shim at load time (engine-generic: core never knows which game package is being
/// registered — the name and source come entirely from the caller).
///
/// # Safety
/// `name` and `js` must be valid null-terminated UTF-8 C strings.  Null pointers degrade to a
/// no-op (never crash).  `catch_unwind`-wrapped (no panic may cross the FFI boundary — spec §6).
///
/// The shim calls this at load time with ("@s2script/cs2", <packaged pawn.js>), so each plugin
/// context receives the @s2script/cs2 package via the runtime registry.
#[no_mangle]
pub extern "C" fn s2script_core_register_package(name: *const c_char, js: *const c_char) {
    let _ = catch_unwind(|| {
        if name.is_null() || js.is_null() {
            return;
        }
        let name_str = match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(s) => s,
            Err(_) => return,
        };
        let js_str = match unsafe { CStr::from_ptr(js) }.to_str() {
            Ok(s) => s,
            Err(_) => return,
        };
        v8host::register_injected_package(name_str, js_str);
    });
}

/// Register a game package's own gamedata with core: the `calls` descriptors it declares, plus the
/// `signatures` they target (A5b, spec §9.1b). The gamedata sibling of
/// `s2script_core_register_package`, and called with the same `name`.
///
/// `gamedata_json` is the shim's MERGED view for that owner (`GameConfig::mergedJson`) — the tree /
/// master / condition / `custom/` merge stays in the shim's one loader, and core consumes the
/// result through the same registry a plugin's packed `gamedata.json` goes through. Comments are
/// already gone (nlohmann parsed it), so plain `serde_json` is enough here.
///
/// Descriptors land under a RESERVED owner id derived from `name`, which no `.s2sp` can claim
/// (`loader::read_s2sp` refuses a manifest that tries), and which is exempt from the `engine:calls`
/// operator allow-list: this is first-party runtime shipped in the same zip as core, replacing
/// natives that are unconditionally callable from any plugin today.
///
/// Engine-generic: core never names a game — the package name comes entirely from the caller.
///
/// # Safety
/// `name` and `gamedata_json` must be valid null-terminated UTF-8 C strings. Null pointers degrade
/// to a no-op (never crash). `catch_unwind`-wrapped (no panic may cross the FFI boundary).
#[no_mangle]
pub extern "C" fn s2script_core_register_package_gamedata(
    name: *const c_char,
    gamedata_json: *const c_char,
) {
    let _ = catch_unwind(|| {
        if name.is_null() || gamedata_json.is_null() {
            return;
        }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return };
        let Ok(json_str) = (unsafe { CStr::from_ptr(gamedata_json) }).to_str() else { return };
        // An owner whose tree merged nothing is the normal pre-A5b state, not an error: register it
        // anyway so the owner id exists and every ask reports "not declared" rather than the
        // game-scoped natives' "no game package registered".
        crate::gamedata_calls::register_game_package(name_str, json_str);
        // The same tree's `hooks`, under the same reserved owner: a game package's declarative
        // inbound hooks (`ctx.gameRules.onTerminateRound`, …). Registering resolves and reserves a
        // shim hook slot; it patches nothing until a plugin subscribes.
        crate::gamedata_hooks::register_game_package(name_str, json_str);
    });
}

/// Set the plugins directory path for the `.s2sp` watcher (`loader::poll_plugins`).
///
/// Called by the shim at load time with the resolved `addons/s2script/plugins/` path
/// (derived via `dladdr` — see `PluginsDir()` in `s2script_mm.cpp`).  Must be called
/// before the first Post-phase frame dispatch for the watcher to activate.
///
/// # Safety
/// `path` must be a valid null-terminated UTF-8 C string.  A null pointer or
/// invalid UTF-8 degrades to a no-op (degrade-never-crash, spec §6).
#[no_mangle]
pub extern "C" fn s2script_core_set_plugins_dir(path: *const c_char) {
    let _ = catch_unwind(|| {
        if path.is_null() {
            return;
        }
        match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => crate::loader::set_plugins_dir(s),
            Err(_) => {}
        }
    });
}

/// Crash reporter: the breadcrumb POD base pointer. The shim's Breakpad callback reads
/// `s2script_core_crash_breadcrumb_size()` raw bytes from here with a single write() —
/// no JSON, no allocation (signal-safe by construction). The pointer targets static
/// memory in this cdylib (linked -z nodelete), so it stays valid for the process lifetime.
#[no_mangle]
pub extern "C" fn s2script_core_crash_breadcrumb() -> *const u8 {
    crate::crash::breadcrumb::breadcrumb_ptr()
}

#[no_mangle]
pub extern "C" fn s2script_core_crash_breadcrumb_size() -> u32 {
    crate::crash::breadcrumb::breadcrumb_size()
}

/// Crash reporter: the shim pushes the treadmill identity block + the crash-spool dir at Load
/// (after `s2script_core_init`). `gd_fail_count > 0` marks the gamedata as stale in every
/// envelope. Null pointers degrade to "" (never crash). Also records the spool dir for the
/// capture paths (Task 2) and schedules the boot sweep (Task 3).
#[no_mangle]
pub extern "C" fn s2script_core_crash_set_identity(
    fingerprint: *const c_char,
    generated_at: *const c_char,
    hl2sdk: *const c_char,
    schema_build: *const c_char,
    gd_fail_count: c_int,
    spool_dir: *const c_char,
) {
    let _ = catch_unwind(|| {
        fn s(p: *const c_char) -> String {
            if p.is_null() { return String::new(); }
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
        crate::crash::breadcrumb::set_identity(
            &s(fingerprint), &s(generated_at), &s(hl2sdk), &s(schema_build), gd_fail_count > 0,
        );
        crate::crash::set_spool_dir(&s(spool_dir));
        crate::crash::uploader::boot_sweep();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};
    use std::sync::Mutex;

    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    extern "C" fn test_logger(_level: c_int, msg: *const c_char) {
        let s = unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned();
        CAPTURED.lock().unwrap().push(s);
    }

    /// The deferred-dispatch sentinel cannot collide with anything a dispatch entry returns.
    ///
    /// The shim tests `== S2_DISPATCH_DEFERRED` EXACTLY — never `< 0`, never truthiness — but a
    /// collision would still be silent and catastrophic (a `HookResult` read as "queue a replay",
    /// or a deferral read as SUPERCEDE). `scripts/ci-native.sh` gates this constant against the
    /// shim header's copy; this pins the ranges it must avoid, on the core side, at build time.
    #[test]
    fn deferred_sentinel_cannot_collide_with_any_dispatch_result() {
        // HookResult is 0..=3 (multiplexer.rs), the boolean entries are 0..=1.
        assert!(!(0..=3).contains(&S2_DISPATCH_DEFERRED), "must be outside the HookResult range");
        // The catch_unwind fallbacks (0, and -99 for game_frame), the header's "unavailable" idiom
        // (-1), and the init/eval error codes (-1, -2, -3, -99).
        for reserved in [0, -1, -2, -3, -99] {
            assert_ne!(S2_DISPATCH_DEFERRED, reserved as c_int);
        }
        // Negative on purpose: shim code shaped `if (r >= 2) MRES_SUPERCEDE` must FAIL that test, so
        // a call site that forgets to check degrades to today's "engine proceeds unhooked".
        assert!(S2_DISPATCH_DEFERRED < 0, "a positive sentinel would fail closed the wrong way");
        // Far outside, so a widened HookResult cannot creep into it.
        assert_eq!(S2_DISPATCH_DEFERRED, -1000);
        // And the mapping the C ABI actually ships.
        assert_eq!(deferral_code(Delivery::Deferred), S2_DISPATCH_DEFERRED);
        assert_eq!(deferral_code(Delivery::Delivered), 0);
    }

    #[test]
    fn init_eval_console_log_shutdown_and_reinit() {
        CAPTURED.lock().unwrap().clear();

        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        assert_eq!(
            s2script_core_eval(
                b"console.log('hello from V8 in CS2')\0".as_ptr() as *const c_char
            ),
            0
        );
        s2script_core_shutdown();

        // platform must survive shutdown: a second cycle works without re-init of the platform
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        assert_eq!(
            s2script_core_eval(b"console.log('second cycle')\0".as_ptr() as *const c_char),
            0
        );
        s2script_core_shutdown();

        let got = CAPTURED.lock().unwrap().clone();
        assert!(
            got.iter().any(|m| m.contains("hello from V8 in CS2")),
            "got: {:?}",
            got
        );
        assert!(
            got.iter().any(|m| m.contains("second cycle")),
            "got: {:?}",
            got
        );
    }

    #[test]
    fn eval_with_js_exception_returns_nonzero_and_does_not_panic() {
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        let rc = s2script_core_eval(b"throw new Error('boom')\0".as_ptr() as *const c_char);
        assert_ne!(rc, 0);
        s2script_core_shutdown();
    }

    use std::sync::Mutex as M2;
    static HOOKS: M2<Vec<(String, i32)>> = M2::new(Vec::new());
    extern "C" fn mock_request(name: *const c_char, enable: c_int) {
        let n = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        HOOKS.lock().unwrap().push((n, enable));
    }

    #[test]
    fn subscribe_installs_dispatch_runs_unsubscribe_removes() {
        // Same behavior as Slice 1 (subscribe → install request; dispatch runs; unsubscribe →
        // remove request), reworked onto the per-plugin model: subscription now goes through a
        // plugin context's injected `OnGameFrame.subscribe`, while the C-ABI dispatch/hook-request
        // wiring is exercised unchanged via `s2script_core_dispatch_game_frame`.
        HOOKS.lock().unwrap().clear();
        assert_eq!(s2script_core_init(Some(test_logger), Some(mock_request), std::ptr::null()), 0);
        v8host::create_plugin_context("p");
        // Subscribing the first handler (via the injected API) must request install:
        v8host::eval_in_context(
            "p",
            r#"
                const { OnGameFrame } = __s2require("@s2script/frame");
                globalThis._sub = OnGameFrame.subscribe(() => {});
            "#,
        )
        .unwrap();
        assert!(HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 1));
        // dispatch (Pre=0) must not crash and returns a HookResult code:
        let rc = s2script_core_dispatch_game_frame(0, 1, 1, 0);
        assert!(rc >= 0);
        // unsubscribe the last handler must request remove:
        v8host::eval_in_context("p", "_sub.dispose();").unwrap();
        assert!(HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 0));
        s2script_core_shutdown();
    }

    #[test]
    fn breadcrumb_ffi_exports_and_dispatch_stamping() {
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        // FFI exports: non-null pointer, size matches the POD, magic readable through the pointer.
        let ptr = s2script_core_crash_breadcrumb();
        assert!(!ptr.is_null());
        assert_eq!(
            s2script_core_crash_breadcrumb_size() as usize,
            std::mem::size_of::<crate::crash::breadcrumb::CrashBreadcrumb>()
        );
        let magic = unsafe { *(ptr as *const u32) };
        assert_eq!(magic, crate::crash::breadcrumb::BREADCRUMB_MAGIC);

        // Identity push (shim-side call simulated).
        let fp = std::ffi::CString::new("fp-1").unwrap();
        let gen = std::ffi::CString::new("1752710400").unwrap();
        let sdk = std::ffi::CString::new("dota-abc123").unwrap();
        let sb = std::ffi::CString::new("schema-77").unwrap();
        let dir = std::ffi::CString::new("/tmp/spool").unwrap();
        s2script_core_crash_set_identity(fp.as_ptr(), gen.as_ptr(), sdk.as_ptr(), sb.as_ptr(), 0, dir.as_ptr());
        let s = crate::crash::breadcrumb::snapshot();
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s.gamedata_fingerprint), "fp-1");
        assert_eq!(s.gamedata_stale, 0);

        // A frame dispatch stamps plugin+dispatch and pushes a ring entry.
        v8host::create_plugin_context("bc_test");
        v8host::eval_in_context(
            "bc_test",
            r#"
                const { OnGameFrame } = __s2require("@s2script/frame");
                globalThis._bcsub = OnGameFrame.subscribe(() => {});
            "#,
        )
        .unwrap();
        let head_before = crate::crash::breadcrumb::snapshot().ring_head;
        s2script_core_dispatch_game_frame(0, 1, 1, 0);
        let s2 = crate::crash::breadcrumb::snapshot();
        assert_ne!(s2.ring_head, head_before, "dispatch must push a ring entry");
        let last = (s2.ring_head as usize + crate::crash::breadcrumb::RING_LEN - 1)
            % crate::crash::breadcrumb::RING_LEN;
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s2.ring[last].plugin), "bc_test");
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s2.ring[last].dispatch), "OnGameFrame:pre");
        // After the dispatch returns, the current stamp is restored to core/idle.
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s2.plugin), "core");

        // The cs2-package setter native is installed in plugin contexts.
        v8host::eval_in_context("bc_test", "__s2_crash_set_game('cs2', 14099);").unwrap();
        let s3 = crate::crash::breadcrumb::snapshot();
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s3.game_name), "cs2");
        assert_eq!(s3.game_build, 14099);
        s2script_core_shutdown();
    }

    #[test]
    fn throwing_frame_handler_spools_a_js_incident_once() {
        let d = std::env::temp_dir().join(format!("s2crash-js-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        crate::crash::set_spool_dir(d.to_str().unwrap());
        v8host::create_plugin_context("thrower");
        v8host::eval_in_context(
            "thrower",
            r#"
                const { OnGameFrame } = __s2require("@s2script/frame");
                globalThis._t = OnGameFrame.subscribe(() => { throw new Error("js-boom"); });
            "#,
        )
        .unwrap();
        // Two frames: the second identical throw is deduped (same signature, <60s apart).
        s2script_core_dispatch_game_frame(0, 1, 1, 0);
        s2script_core_dispatch_game_frame(0, 1, 0, 0);
        let items = crate::crash::spool::scan(&d);
        assert_eq!(items.len(), 1, "dedup: one incident for a repeated identical throw");
        let crate::crash::spool::SpoolItem::Envelope(p) = &items[0] else { panic!("expected envelope") };
        let env: crate::crash::envelope::Envelope =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(env.kind, "js");
        assert_eq!(env.breadcrumb.plugin, "thrower");
        match env.detail {
            crate::crash::envelope::Detail::Js { message, stack, .. } => {
                assert!(message.contains("js-boom"));
                assert!(stack.contains("js-boom") || !stack.is_empty());
            }
            other => panic!("wrong detail: {:?}", other),
        }
        crate::crash::set_spool_dir("");
        s2script_core_shutdown();
    }

    #[test]
    fn unhandled_rejection_spools_a_js_incident() {
        let d = std::env::temp_dir().join(format!("s2crash-rej-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        crate::crash::set_spool_dir(d.to_str().unwrap());
        v8host::create_plugin_context("rejector");
        v8host::eval_in_context("rejector", "Promise.reject(new Error('rej-boom'));").unwrap();
        // The drain performs the microtask checkpoint AND flushes pending rejections.
        s2script_core_dispatch_game_frame(1, 1, 0, 1); // Post phase → frame_async_drain
        let items = crate::crash::spool::scan(&d);
        assert_eq!(items.len(), 1);
        let crate::crash::spool::SpoolItem::Envelope(p) = &items[0] else { panic!("expected envelope") };
        let env: crate::crash::envelope::Envelope =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(env.kind, "js");
        match env.detail {
            crate::crash::envelope::Detail::Js { message, .. } => assert!(message.contains("rej-boom")),
            other => panic!("wrong detail: {:?}", other),
        }
        crate::crash::set_spool_dir("");
        s2script_core_shutdown();
    }

    #[test]
    fn handled_rejection_is_not_reported() {
        let d = std::env::temp_dir().join(format!("s2crash-rej2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        crate::crash::set_spool_dir(d.to_str().unwrap());
        v8host::create_plugin_context("handled");
        // .catch attached synchronously — kPromiseHandlerAddedAfterReject cancels the pending report.
        v8host::eval_in_context("handled", "Promise.reject(new Error('nope')).catch(() => {});").unwrap();
        s2script_core_dispatch_game_frame(1, 1, 0, 1);
        assert!(crate::crash::spool::scan(&d).is_empty(), "a handled rejection must not report");
        crate::crash::set_spool_dir("");
        s2script_core_shutdown();
    }

    #[test]
    fn crash_test_native_is_gated_by_dev_test_config() {
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        v8host::create_plugin_context("harness");
        // No ops table + no dev_test config → every kind refuses (returns false), nothing raised.
        v8host::eval_in_context(
            "harness",
            r#"
                if (__s2_crash_test("segv") !== false) throw new Error("segv must be refused");
                if (__s2_crash_test("abort") !== false) throw new Error("abort must be refused");
                if (__s2_crash_test("panic") !== false) throw new Error("panic must be refused");
                if (__s2_crash_test("bogus") !== false) throw new Error("unknown kind must be refused");
            "#,
        )
        .unwrap();
        s2script_core_shutdown();
    }

    /// The deferred-dispatch selftest native EXISTS ONLY when `S2_DEFER_SELFTEST` is set.
    ///
    /// The gate is on INSTALLATION, not on the call, and this is what pins that: in a production
    /// process the property is not on the global at all, so there is no reachable path to the
    /// synthetic re-entrancy — a plugin cannot call it, feature-detect its way into it, or find it
    /// by enumerating the global. Both directions are asserted in one test, in one process, because
    /// "absent" is only meaningful next to a demonstration that the same code path CAN install it.
    #[test]
    fn defer_selftest_native_exists_only_when_the_env_var_is_set() {
        std::env::remove_var("S2_DEFER_SELFTEST");
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        v8host::create_plugin_context("ddq_unarmed");
        v8host::eval_in_context(
            "ddq_unarmed",
            r#"
                if (typeof __s2_defer_selftest !== "undefined")
                    throw new Error("the selftest native must NOT exist without S2_DEFER_SELFTEST");
                if (Object.getOwnPropertyNames(globalThis).indexOf("__s2_defer_selftest") !== -1)
                    throw new Error("the selftest native must not be enumerable on the global either");
            "#,
        )
        .unwrap();

        // Armed — same process, a NEW context. The gate is re-read per install_natives, so arming
        // takes effect for contexts created afterwards without a restart.
        std::env::set_var("S2_DEFER_SELFTEST", "1");
        v8host::create_plugin_context("ddq_armed");
        let armed = v8host::eval_in_context(
            "ddq_armed",
            r#"
                if (typeof __s2_defer_selftest !== "function")
                    throw new Error("armed, the selftest native must exist");
                // No engine-ops table -> the op is absent and the native degrades to 0. Never a
                // crash, and never a false "1" that would let a gate pass without the shim.
                if (__s2_defer_selftest() !== 0)
                    throw new Error("with no engine ops the native must return 0");
            "#,
        );
        std::env::remove_var("S2_DEFER_SELFTEST");
        armed.unwrap();
        s2script_core_shutdown();
    }

    /// E1: the books feed lives in THIS ffi entry, unconditionally — with ZERO JS
    /// subscribers (dispatch_entity_event early-returns) the books must still update.
    #[test]
    fn entity_event_feed_updates_books_with_no_subscribers() {
        crate::entity_live::reset_for_tests();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        let create = std::ffi::CString::new("create").unwrap();
        let spawn  = std::ffi::CString::new("spawn").unwrap();
        let delete = std::ffi::CString::new("delete").unwrap();
        let cls    = std::ffi::CString::new("prop_physics").unwrap();
        let handle = ((7u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32; // (index 42, serial 7)

        s2script_core_dispatch_entity_event(create.as_ptr(), cls.as_ptr(), handle as c_int);
        let (id, ser) = crate::entity_live::lookup(42).expect("create fed the books");
        assert!(id >= 1); assert_eq!(ser, 7);

        s2script_core_dispatch_entity_event(spawn.as_ptr(), cls.as_ptr(), handle as c_int);
        assert_eq!(crate::entity_live::lookup(42), Some((id, 7)), "matching spawn keeps the id");

        // a stale delete (wrong serial) must NOT evict:
        let stale = ((9u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32;
        s2script_core_dispatch_entity_event(delete.as_ptr(), cls.as_ptr(), stale as c_int);
        assert!(crate::entity_live::lookup(42).is_some());

        s2script_core_dispatch_entity_event(delete.as_ptr(), cls.as_ptr(), handle as c_int);
        assert_eq!(crate::entity_live::lookup(42), None, "matching delete removed the entry");

        // the -1 no-entity sentinel must not touch the books (and must not panic):
        s2script_core_dispatch_entity_event(create.as_ptr(), cls.as_ptr(), -1);
        assert_eq!(crate::entity_live::len(), 0);
        s2script_core_shutdown();
    }

    /// E1: map start clears the whole table (the implicit epoch) + arms the repair sweep,
    /// unconditionally — before/independent of the Server.onMapStart JS dispatch.
    #[test]
    fn map_start_clears_books_and_arms_repair_sweep() {
        crate::entity_live::reset_for_tests();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        crate::entity_live::on_created(3, 5);
        let map = std::ffi::CString::new("de_vertigo").unwrap();
        s2script_core_dispatch_map_start(map.as_ptr());
        assert_eq!(crate::entity_live::len(), 0, "the epoch: books cleared at map start");
        assert!(crate::entity_live::take_repair_armed(), "map start arms the repair sweep");
        s2script_core_shutdown();
    }
}

/// Shim -> core: take (and clear) the recipient allow-mask a plugin set during the current game-event
/// pre-dispatch. Returns `has_mask` via the out-param and the mask itself; `has_mask == 0` means the
/// plugin expressed no opinion and the shim must not filter.
#[no_mangle]
pub extern "C" fn s2script_core_take_event_recipients(out_mask: *mut u64) -> c_int {
    catch_unwind(|| {
        if out_mask.is_null() { return 0; }
        match crate::events::take_event_recipients() {
            Some(m) => { unsafe { *out_mask = m }; 1 }
            None => 0,
        }
    })
    .unwrap_or(0)
}
