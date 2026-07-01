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
        if phase == Phase::Post { v8host::frame_async_drain(); } // Post: resolve async + microtask checkpoint
        out.result as c_int
    })
    .unwrap_or(-99)
}

#[no_mangle]
pub extern "C" fn s2script_core_shutdown() {
    let _ = catch_unwind(|| v8host::shutdown());
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

/// C-ABI entry point retained for shim link-compatibility.  As of Slice 4 the `@s2script/cs2`
/// package (`games/cs2/js/pawn.js`) is EMBEDDED per plugin context (via core's `include_str!` +
/// the injected prelude) and returned through `require("@s2script/cs2")`, so there is no longer a
/// single-context "load a cs2 JS file" step.  This entry is now a degrade-safe no-op — the `path`
/// argument is ignored.  `catch_unwind`-wrapped (no panic may cross the FFI boundary — spec §6).
///
/// TODO(shim, later slice): drop the shim's `s2script_core_load_cs2(Cs2JsPath())` call and its
/// HOST-context `cs2.*` demo; the per-plugin `require` model supersedes both.
#[no_mangle]
pub extern "C" fn s2script_core_load_cs2(_path: *const c_char) {
    // Intentionally empty: pawn.js is compiled into each plugin context (see v8host::create_plugin_context).
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
                const { OnGameFrame } = __s2require("@s2script/std");
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
