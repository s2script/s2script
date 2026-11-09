//! `Server.commandEx` — run a server console command and get its console output back (SourceMod
//! `ServerCommandEx`). Engine-generic: "run a console command and read what it printed" holds on
//! every Source 2 game; the capture itself is the shim's (`server_command_capture` op), and its
//! result comes back through `s2script_core_dispatch_command_output`, drained at the top of the next
//! frame.
//!
//! Same shape as the client convar queries in `crate::client`: each pending call is one
//! subscription on `CAPTURES`, keyed by a host-minted cookie, so owner teardown drops it like any
//! other subscription. A result is delivered only to the call that minted its cookie; a call that
//! gets no result in time resolves `null`.

use crate::dispatch::{fan_out, Delivery, Instrument};
use crate::v8host::{engine_ops, set_native, subscribe_into};
use std::ffi::CString;

#[derive(Clone)]
enum CaptureOutcome { Cancelled, Output { text: String, truncated: bool } }

struct PendingCapture {
    owner: String,
    sub_id: u64,
    deadline: std::time::Instant,
    outcome: Option<CaptureOutcome>,
}

thread_local! {
    static CAPTURES: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    static PENDING: std::cell::RefCell<std::collections::HashMap<i32, PendingCapture>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static COOKIE_SEED: (std::collections::hash_map::RandomState, std::cell::Cell<u64>)
        = (std::collections::hash_map::RandomState::new(), std::cell::Cell::new(0));
}

/// The shim bounds its own queue too; this keeps core's books from growing past it.
const MAX_PENDING_CAPTURES: usize = 64;
/// Longer than the shim's own timeout (10 s), so a capture the shim finishes late still lands.
const CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn mint_cookie() -> i32 {
    use std::hash::BuildHasher;
    COOKIE_SEED.with(|(state, counter)| loop {
        let n = counter.get().wrapping_add(1);
        counter.set(n);
        // Positive and nonzero: the shim treats 0 as "no capture".
        let cookie = (state.hash_one(n) & 0x7fff_ffff) as i32;
        if cookie != 0 && !PENDING.with(|p| p.borrow().contains_key(&cookie)) { return cookie; }
    })
}

/// Pending captures keep the frame detour requested, so the shim drains results and the sweep runs.
pub(crate) fn captures_pending() -> usize {
    PENDING.with(|p| p.borrow().len())
}

/// Run the call's callback with `outcome`. The entry and its subscription go only once the callback
/// actually ran; a busy isolate leaves both, with the outcome recorded for the sweep to retry.
fn settle(cookie: i32, outcome: CaptureOutcome) {
    let key = cookie.to_string();
    let snap = CAPTURES.with(|m| m.borrow().snapshot(&key));
    let delivery = fan_out(&snap, "server_command_capture_settle", Instrument::none(), |tc| match &outcome {
        CaptureOutcome::Cancelled => Some(vec![v8::null(tc).into(), v8::Boolean::new(tc, false).into()]),
        CaptureOutcome::Output { text, truncated } => {
            let text = v8::String::new(tc, text)?;
            Some(vec![text.into(), v8::Boolean::new(tc, *truncated).into()])
        }
    });
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if delivery == Delivery::Deferred {
            if let Some(c) = p.get_mut(&cookie) { c.outcome = Some(outcome); }
            return;
        }
        if let Some(c) = p.remove(&cookie) {
            CAPTURES.with(|m| { m.borrow_mut().remove_by_ids(&[c.sub_id]); });
        }
    });
}

/// Per frame, before any JS runs: retry deliveries a busy isolate deferred, and resolve null any
/// call whose result never came.
pub(crate) fn sweep() {
    let now = std::time::Instant::now();
    let due: Vec<(i32, CaptureOutcome)> = PENDING.with(|p| p.borrow().iter().filter_map(|(c, q)| {
        match &q.outcome {
            Some(o) => Some((*c, o.clone())),
            None if now >= q.deadline => Some((*c, CaptureOutcome::Cancelled)),
            None => None,
        }
    }).collect());
    for (cookie, outcome) in due { settle(cookie, outcome); }
}

/// Deliver a finished capture to the one call that minted `cookie`.
pub(crate) fn dispatch_command_output(cookie: i32, text: &str, truncated: bool) {
    let waiting = PENDING.with(|p| p.borrow().get(&cookie).is_some_and(|c| c.outcome.is_none()));
    if !waiting { return; }   // unknown, already settled, or its plugin unloaded
    settle(cookie, CaptureOutcome::Output { text: text.to_string(), truncated });
}

/// Native `__s2_server_command_capture(cmd, callback) -> number` — queue `cmd` at the server console
/// with its output captured. Returns the cookie, or 0 when nothing was run (empty command, too many
/// captures pending, or capture unavailable on this build — the caller then runs it uncaptured).
/// `callback(text, truncated)` runs once, on the frame after the command executes; `(null, false)`
/// when no result arrives within 15 seconds.
fn s2_server_command_capture(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        if args.length() < 2 || !args.get(1).is_function() { return; }
        let cmd = args.get(0).to_rust_string_lossy(scope);
        if cmd.trim().is_empty() { return; }
        if captures_pending() >= MAX_PENDING_CAPTURES {
            crate::v8host::log_warn("WARN: Server.commandEx: too many pending captures; running uncaptured");
            return;
        }
        let Some(f) = engine_ops().and_then(|o| o.server_command_capture) else { return };
        let Ok(ccmd) = CString::new(cmd) else { return };
        let cookie = mint_cookie();
        if crate::nest::with_outbound(&args, || f(ccmd.as_ptr(), cookie)) == 0 { return; }
        let Some((sub_id, _)) = subscribe_into(scope, &args, &CAPTURES, &cookie.to_string(), 1) else { return };
        let owner = crate::v8host::current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let deadline = std::time::Instant::now() + CAPTURE_TIMEOUT;
        PENDING.with(|p| p.borrow_mut().insert(cookie, PendingCapture { owner, sub_id, deadline, outcome: None }));
        crate::v8host::refresh_detour();
        rv.set_int32(cookie);
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_server_command_capture", s2_server_command_capture);
}

/// The owner-scoped store: an unloading plugin's pending calls are dropped without a callback.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "SERVER_COMMAND_CAPTURES",
        Box::new(|owner| {
            CAPTURES.with(|m| { m.borrow_mut().remove_by_owner(owner); });
            PENDING.with(|p| p.borrow_mut().retain(|_, c| c.owner != owner));
        }),
        Box::new(|ids| {
            CAPTURES.with(|m| { m.borrow_mut().remove_by_ids(ids); });
            PENDING.with(|p| p.borrow_mut().retain(|_, c| !ids.contains(&c.sub_id)));
        }),
        Box::new(|| {
            CAPTURES.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
            PENDING.with(|p| p.borrow_mut().clear());
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string};
    use crate::v8host::{create_plugin_context, init, set_engine_ops, shutdown, unload_plugin, S2EngineOps};
    use std::os::raw::{c_char, c_int};

    thread_local! {
        static CALLS: std::cell::RefCell<Vec<(String, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
        static PLAIN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
        static ACCEPT: std::cell::Cell<i32> = const { std::cell::Cell::new(1) };
    }
    extern "C" fn mock_capture(cmd: *const c_char, cookie: c_int) -> c_int {
        let cmd = unsafe { std::ffi::CStr::from_ptr(cmd) }.to_string_lossy().into_owned();
        CALLS.with(|c| c.borrow_mut().push((cmd, cookie)));
        ACCEPT.with(|a| a.get())
    }
    extern "C" fn mock_plain(cmd: *const c_char) {
        let cmd = unsafe { std::ffi::CStr::from_ptr(cmd) }.to_string_lossy().into_owned();
        PLAIN.with(|c| c.borrow_mut().push(cmd));
    }
    fn ops() -> S2EngineOps {
        S2EngineOps { server_command_capture: Some(mock_capture), server_command: Some(mock_plain), ..S2EngineOps::none() }
    }
    fn reset() {
        CALLS.with(|c| c.borrow_mut().clear());
        PLAIN.with(|c| c.borrow_mut().clear());
        ACCEPT.with(|a| a.set(1));
    }
    fn last_cookie() -> i32 { CALLS.with(|c| c.borrow().last().map(|c| c.1).unwrap_or(0)) }
    /// Promises settle at the frame's explicit microtask checkpoint; run one before reading.
    fn js(plugin: &str, expr: &str) -> String {
        crate::v8host::with_host_isolate(|isolate| isolate.perform_microtask_checkpoint()).unwrap();
        eval_in_context_string(plugin, expr)
    }

    /// The output reaches only the call that minted the cookie, once.
    #[test]
    fn command_ex_delivers_output_to_its_caller() {
        init(dummy_logger()).unwrap();
        reset();
        set_engine_ops(Some(ops()));
        create_plugin_context("sc1");
        eval_in_context_string("sc1", r#"globalThis.r = "pending";
            __s2pkg_server.Server.commandEx("status").then(function (v) { globalThis.r = JSON.stringify(v); }); """#);
        let cookie = last_cookie();
        assert!(cookie > 0);
        assert_eq!(CALLS.with(|c| c.borrow()[0].0.clone()), "status");
        assert!(PLAIN.with(|c| c.borrow().is_empty()), "a captured command is not also run plainly");
        dispatch_command_output(cookie ^ 1, "forged", false);
        assert_eq!(js("sc1", "r"), "pending");
        dispatch_command_output(cookie, "hostname: test\n", false);
        assert_eq!(js("sc1", "r"), r#""hostname: test\n""#);
        dispatch_command_output(cookie, "again", false);
        assert_eq!(js("sc1", "r"), r#""hostname: test\n""#);
        assert_eq!(captures_pending(), 0);

        eval_in_context_string("sc1", r#"__s2pkg_server.Server.commandEx("cvarlist").then(function (v) { globalThis.r = JSON.stringify(v); }); """#);
        dispatch_command_output(last_cookie(), "first 16 KiB", true);
        assert_eq!(js("sc1", "r"), r#""first 16 KiB""#, "a cut capture still delivers what it has");
        shutdown();
    }

    /// Without the capture op (or when the shim refuses), the command still runs, uncaptured, and
    /// the promise resolves null.
    #[test]
    fn command_ex_falls_back_to_a_plain_command() {
        init(dummy_logger()).unwrap();
        reset();
        ACCEPT.with(|a| a.set(0));
        set_engine_ops(Some(ops()));
        create_plugin_context("sc2");
        eval_in_context_string("sc2", r#"globalThis.out = [];
            __s2pkg_server.Server.commandEx("mp_restartgame 1").then(function (v) { out.push(String(v)); }); """#);
        assert_eq!(js("sc2", "out.join()"), "null");
        assert_eq!(PLAIN.with(|c| c.borrow().clone()), vec!["mp_restartgame 1".to_string()]);
        assert_eq!(captures_pending(), 0);

        set_engine_ops(Some(S2EngineOps { server_command: Some(mock_plain), ..S2EngineOps::none() }));
        eval_in_context_string("sc2", r#"__s2pkg_server.Server.commandEx("say hi").then(function (v) { out.push(String(v)); }); """#);
        assert_eq!(js("sc2", "out.join()"), "null,null");
        assert_eq!(PLAIN.with(|c| c.borrow().len()), 2);
        shutdown();
    }

    /// A result that lands while the isolate is busy is kept for the sweep; one that never lands
    /// resolves null at the timeout; unload drops the owner's calls.
    #[test]
    fn command_ex_retries_times_out_and_unloads() {
        init(dummy_logger()).unwrap();
        reset();
        set_engine_ops(Some(ops()));
        create_plugin_context("sc3");
        eval_in_context_string("sc3", r#"globalThis.out = [];
            __s2pkg_server.Server.commandEx("status").then(function (v) { out.push(String(v)); }); """#);
        let cookie = last_cookie();
        crate::v8host::with_host_isolate(|_| dispatch_command_output(cookie, "late", false)).unwrap();
        assert_eq!(js("sc3", "out.join()"), "", "nothing ran inside the busy isolate");
        assert_eq!(captures_pending(), 1, "the result is kept");
        sweep();
        assert_eq!(js("sc3", "out.join()"), "late");
        assert_eq!(captures_pending(), 0);

        eval_in_context_string("sc3", r#"__s2pkg_server.Server.commandEx("status").then(function (v) { out.push(String(v)); }); """#);
        sweep();
        assert_eq!(js("sc3", "out.length"), "1", "not yet due");
        PENDING.with(|p| p.borrow_mut().values_mut().for_each(|c| c.deadline = std::time::Instant::now()));
        sweep();
        assert_eq!(js("sc3", "out[1]"), "null", "timed out");

        create_plugin_context("sc4");
        eval_in_context_string("sc4", r#"__s2pkg_server.Server.commandEx("status"); """#);
        assert_eq!(captures_pending(), 1);
        unload_plugin("sc4");
        assert_eq!(captures_pending(), 0, "unload drops the owner's captures");
        shutdown();
    }
}
