use crate::multiplexer::Phase;
use crate::v8host::{self, HookRequestFn, LogFn, S2EngineOps};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;
use std::ffi::CStr;

#[no_mangle]
pub extern "C" fn s2script_core_init(
    logger: Option<LogFn>,
    request_hook: Option<HookRequestFn>,
    ops: *const S2EngineOps,
) -> c_int {
    catch_unwind(|| {
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
        let out = v8host::dispatch_onframe(phase, simulating != 0, first != 0, last != 0);
        if phase == Phase::Post {
            v8host::frame_async_drain(); // Post: resolve async + microtask checkpoint
            v8host::dispatch_pending_cookie_cached(); // Post, HOST free: fan out queued Cookies.onCached
            v8host::dispatch_pending_ws_events(); // Post, HOST free: fan out queued WebSocket on* events
            v8host::dispatch_pending_net_events(); // Post, HOST free: fan out queued net (TCP/UDP) events
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
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_game_event(name: *const c_char) {
    let _ = catch_unwind(|| {
        if name.is_null() { return; }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return };
        v8host::dispatch_game_event(name_str);
    });
}

/// Shim → core: called by the shim's six client-lifecycle SourceHooks (Clients sub-project) with the
/// lifecycle event `name` ("connect"/"putinserver"/"active"/"fullyconnect"/"disconnect"/
/// "settingschanged") + the client's `slot`. Notify-only: dispatches to the name's `Clients.on*` JS
/// subscribers. `catch_unwind`-wrapped; null pointer / invalid UTF-8 degrade to a no-op (never panic
/// across the FFI boundary per spec §6).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_client_event(name: *const c_char, slot: c_int) {
    let _ = catch_unwind(|| {
        if name.is_null() { return; }
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return };
        v8host::dispatch_client_event(name_str, slot as i32);
    });
}

/// Shim → core: the INetworkServerService::StartupServer POST hook reports a map start with the
/// live map name. Notify-only: dispatches to the `Server.onMapStart` JS subscribers.
/// `catch_unwind`-wrapped; a null pointer degrades to "" (never panic across the FFI boundary).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_map_start(map: *const c_char) {
    let _ = catch_unwind(|| {
        let map_str = if map.is_null() { "" } else {
            (unsafe { CStr::from_ptr(map) }).to_str().unwrap_or("")
        };
        v8host::dispatch_map_start(map_str);
    });
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
    std::panic::catch_unwind(|| v8host::dispatch_game_event_pre(name_str)).unwrap_or(0)
}

/// Slice 6.6 Stage 2: run the Damage.onPre subscribers over the current damage info. The shim has already
/// set the current CTakeDamageInfo pointer; handlers read/modify it in place via the damage_* ops.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_damage() {
    let _ = catch_unwind(|| v8host::dispatch_damage());
}

/// Shim → core: called by the `FireOutputInternal` detour (entity-I/O slice) with the firing entity's
/// classname, the output name, packed activator/caller `CEntityHandle` ints (-1 = none), the output's
/// value as a string, and the delay. Runs the matching `Entity.onOutput` subscribers SYNCHRONOUSLY and
/// returns the collapsed `HookResult` (0 Continue .. 3 Stop) — the shim supersedes (suppresses) the
/// original `FireOutputInternal` call when the result is >= Handled (2).
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
        v8host::dispatch_concommand(name_str, slot as i32, args_str);
    });
}

/// C-ABI entry point: dispatch a player chat line for command triggers (Slice 6.11b).  The shim's
/// Host_Say detour calls this with the speaker's `slot` + the raw message text (CCommand::Arg(1)).
///
/// Returns 1 if the caller should SUPPRESS the chat broadcast (a matched SILENT `/` trigger, OR a raw
/// `Chat.onMessage` subscriber that returned >= Handled), else 0 (the public `!` trigger and ordinary
/// chat with no blocking subscriber show).  `teamonly` (0/1) is threaded to the raw-chat subscribers.
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
        if v8host::dispatch_chat(slot as i32, text_str, teamonly != 0) { 1 } else { 0 }
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
        if v8host::dispatch_client_command(slot as i32, name_str, args_str) { 1 } else { 0 }
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
        match v8host::ban_check(xuid, now) {
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
}
