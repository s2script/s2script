    use super::*;
    // Game-event dispatch moved to `crate::events`; the deferred-queue and lifecycle tests below
    // still drive it as their vehicle.
    use crate::events::{dispatch_game_event, dispatch_game_event_pre, replay_game_event};
    // The client-lifecycle dispatch moved to `crate::client`; the fan_out and voice tests
    // below still drive it as their vehicle.
    use crate::client::dispatch_client_event;
    use crate::commands::{dispatch_chat, dispatch_concommand, ReplySource};
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

    fn active_resources(id: &str) -> usize {
        REGISTRY.with(|r| {
            let registry = r.borrow();
            let generation = registry.generation_of(id).expect("plugin is live");
            registry.active_resource_count(id, generation).expect("generation is current")
        })
    }

    fn seed_injected_socket_connect(owner: &str, ws_socket: bool) -> (u64, u64) {
        let generation = REGISTRY.with(|r| r.borrow().generation_of(owner).expect("plugin live"));
        let g_ctx = PLUGINS.with(|p| p.borrow().get(owner).unwrap().context.clone());
        let id = HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().unwrap();
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx);
            let (id, promise) = crate::jobs::begin_job(scope);
            let key = v8::String::new(scope, "__injected_connect").unwrap();
            ctx.global(scope).set(scope, key.into(), promise.into());

            fn owned(
                scope: &mut v8::PinScope,
                args: v8::FunctionCallbackArguments,
                mut rv: v8::ReturnValue,
            ) {
                let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
                let owner = current_plugin(scope).unwrap_or_default();
                let ws_socket = args.get(1).boolean_value(scope);
                rv.set(v8::Boolean::new(
                    scope,
                    if ws_socket { crate::ws::is_owner(id, &owner) } else { crate::net::is_owner(id, &owner) },
                ).into());
            }
            let global = ctx.global(scope);
            set_native(scope, global, "__test_socket_owned", owned);
            id
        });
        let resource = if ws_socket { plugin::Resource::WsConn(id) } else { plugin::Resource::NetConn(id) };
        assert!(record_resource(owner, generation, resource));
        if ws_socket {
            crate::ws::test_insert_conn(id, owner.into(), generation);
        } else {
            crate::net::test_insert_conn(id, owner.into(), generation);
        }
        (id, generation)
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
        assert_eq!(active_resources("p"), 1, "a live repeater retains one timer resource");
        assert_eq!(eval_in_context_string("p", "String(__t.kill())"), "true");
        assert_eq!(active_resources("p"), 0, "kill releases the repeater resource");
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
        assert_eq!(active_resources("p"), 0, "self-kill releases the timer ledger entry");
        shutdown();
    }

    #[test]
    fn one_shot_timer_completion_releases_its_active_ledger_entry() {
        init(dummy_logger()).unwrap();
        eval_std("p", "__s2pkg_timers.after(0, function () {});");
        assert_eq!(active_resources("p"), 1);
        frame_async_drain();
        assert_eq!(active_resources("p"), 0);
        shutdown();
    }

    #[test]
    fn explicit_hook_disposal_releases_before_unload() {
        init(dummy_logger()).unwrap();
        eval_std("p", "globalThis.__h = OnGameFrame.subscribe(function () {});");
        assert_eq!(active_resources("p"), 1);
        eval_in_context_string("p", "__h.dispose(); 'disposed'");
        assert_eq!(active_resources("p"), 0);
        unload_plugin("p");
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
            crate::jobs::pending(),
            0,
            "baseline: PENDING_JOBS should be 0 after draining strays"
        );

        // Submit a real in-flight job with a long sleep so it stays pending throughout.
        eval_std("p", "threadSleep(1000).then(()=>{});");
        assert_eq!(crate::jobs::pending(), 1, "PENDING_JOBS should be 1 after submitting real job");

        // Inject a STALE completion for an id that has no resolver (mimics a prior isolate's leftover).
        // This does NOT touch PENDING_JOBS and stores no resolver.
        pool().submit(9_999_999, Box::new(|| Ok(())));

        // Wait briefly for the trivial stale job to land on the completion channel.
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Drain — the stale completion surfaces here; the 1000ms real job is still pending.
        frame_async_drain();

        assert_eq!(
            crate::jobs::pending(),
            1,
            "stale completion must not undercount PENDING_JOBS"
        );

        shutdown();
    }

    /// Jobs spine: missing complete of a live in-flight job must not decrement, and a second
    /// complete of an already-taken id must not undercount.
    #[test]
    fn missing_and_double_job_completion_do_not_undercount() {
        init(dummy_logger()).unwrap();
        while pool().try_recv_completed().is_some() {}
        eval_std("p", "threadSleep(1000).then(()=>{});");
        assert_eq!(crate::jobs::pending(), 1);
        let ids = crate::jobs::resolver_ids();
        assert_eq!(ids.len(), 1, "exactly one in-flight job resolver");
        let id = ids[0];

        assert!(crate::jobs::complete_job(9_999_996).is_none());
        assert_eq!(crate::jobs::pending(), 1, "missing complete must not decrement");

        assert!(crate::jobs::complete_job(id).is_some());
        assert_eq!(crate::jobs::pending(), 0);
        assert!(crate::jobs::complete_job(id).is_none());
        assert_eq!(crate::jobs::pending(), 0, "double complete must not undercount");
        shutdown();
    }

    #[test]
    fn real_job_completion_releases_its_active_ledger_entry() {
        init(dummy_logger()).unwrap();
        while pool().try_recv_completed().is_some() {}
        eval_std("p", "threadSleep(0).then(function () { globalThis.__done = true; });");
        assert_eq!(active_resources("p"), 1);
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if crate::jobs::pending() == 0 { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(crate::jobs::pending(), 0);
        assert_eq!(active_resources("p"), 0);
        shutdown();
    }

    /// Unload a plugin with a generic in-flight job (`threadSleep`). Teardown drops the resolver
    /// and decrements pending. The original 1000ms sleep cannot be observed on a short wait, so
    /// this injects a zero-work completion for the *same captured id* through the process-global
    /// pool, then drains that late completion: pending stays 0 and the disposed context is not
    /// re-entered.
    #[test]
    fn generic_job_unload_mid_flight_drops_and_late_complete_is_noop() {
        init(dummy_logger()).unwrap();
        while pool().try_recv_completed().is_some() {}
        load_body(
            "jobul",
            r#"
            const { threadSleep } = require("@s2script/timers");
            threadSleep(1000).then(function () { globalThis.__resumed = true; });
        "#,
            "{}",
        );
        assert_eq!(crate::jobs::pending(), 1, "job is in-flight before unload");
        let ids = crate::jobs::resolver_ids();
        assert_eq!(ids.len(), 1, "exactly one in-flight job resolver");
        let id = ids[0];
        unload_plugin("jobul");
        assert!(
            !PLUGINS.with(|p| p.borrow().contains_key("jobul")),
            "context disposed"
        );
        assert_eq!(
            crate::jobs::pending(),
            0,
            "teardown decrements pending only once"
        );
        assert!(
            crate::jobs::resolver_is_empty(),
            "resolver removed on Job teardown"
        );

        // Guaranteed stale completion for the captured id. Zero-work so it lands immediately;
        // waiting 40ms for the original 1000ms threadSleep cannot prove the late-complete path.
        pool().submit(id, Box::new(|| Ok(())));
        let mut landed = false;
        for _ in 0..ASYNC_POLL_TICKS {
            if let Some((cid, _, _lease)) = pool().try_recv_completed() {
                assert_eq!(
                    cid, id,
                    "injected completion must carry the unloaded job id"
                );
                landed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(landed, "injected zero-work stale completion never arrived");
        // Re-inject so `frame_async_drain` consumes a completion for this id
        // (`complete_job` → None). Same 30ms floor as
        // `stale_job_completion_does_not_undercount_pending`.
        pool().submit(id, Box::new(|| Ok(())));
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        frame_async_drain();
        assert_eq!(
            crate::jobs::pending(),
            0,
            "late completion must not decrement again"
        );
        assert!(crate::jobs::resolver_is_empty());
        assert!(
            crate::jobs::complete_job(id).is_none(),
            "drain consumed the late id; a second complete is still a no-op"
        );
        assert!(
            !PLUGINS.with(|p| p.borrow().contains_key("jobul")),
            "late completion must not resurrect a disposed context"
        );
        assert!(
            eval_in_context("jobul", "globalThis.__resumed").is_err(),
            "continuation must not run: disposed context is not enterable"
        );
        shutdown();
    }

    /// Unload during an in-flight WebSocket connect against the existing local echo server
    /// (a completing handshake, not a never-accepted listener / 10s timeout). Unload before the
    /// first drain; then poll until `poll_signals` consumes the late Connected/Closed (or
    /// ConnectFailed) and prove the Jobs late-complete path is a no-op.
    #[test]
    fn ws_connect_unload_mid_flight_drops_and_late_complete_is_noop() {
        init(dummy_logger()).unwrap();
        let port = spawn_local_ws_echo_server();
        load_body(
            "wsul",
            &format!(
                r#"
            globalThis.__out = "pending";
            __s2_ws_connect("ws://127.0.0.1:{port}/").then(function () {{
                globalThis.__out = "resolved";
            }}).catch(function () {{
                globalThis.__out = "rejected";
            }});
        "#,
                port = port
            ),
            "{}",
        );
        assert_eq!(crate::jobs::pending(), 1, "ws connect job is in-flight before unload");
        assert!(!crate::jobs::resolver_is_empty());
        assert_eq!(
            crate::jobs::resolver_ids().len(),
            1,
            "exactly one in-flight ws connect resolver"
        );
        // Unload BEFORE the first drain so the handshake cannot settle into a live context.
        unload_plugin("wsul");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("wsul")), "context disposed");
        assert_eq!(crate::jobs::pending(), 0, "Job teardown decrements once");
        assert!(crate::jobs::resolver_is_empty());

        // Teardown is separately wakeable from data/connect completion: the worker exits promptly
        // and silently, rather than producing a completion into the unloaded generation.
        for _ in 0..200 {
            if crate::ws::active_worker_count() == 0 { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(crate::ws::active_worker_count(), 0, "cancelled handshake worker must exit");
        assert_eq!(crate::ws::active_conn_count(), 0, "teardown removes the registry row");
        let poll = crate::ws::poll_signals();
        assert!(poll.connects.is_empty() && poll.drops.is_empty(), "owner shutdown is silent");
        frame_async_drain();
        dispatch_pending_ws_events();
        assert_eq!(crate::jobs::pending(), 0, "late ws signal must not decrement again");
        assert!(crate::jobs::resolver_is_empty());
        assert!(
            !PLUGINS.with(|p| p.borrow().contains_key("wsul")),
            "late ws signal must not resurrect a disposed context"
        );
        assert!(
            eval_in_context("wsul", "globalThis.__out").is_err(),
            "continuation must not run: disposed context is not enterable"
        );
        shutdown();
    }

    /// SQLite query unload mid-flight.
    ///
    /// The in-process actor can finish the SQL in microseconds, so racing "query still on the
    /// actor" is inherently nondeterministic. The Jobs-visible in-flight window is deterministic:
    /// `commit_job` has run (resolver in the map, pending == 1) and we unload *before the next
    /// drain* applies any completion. That is the same protocol a live mid-query unload hits.
    #[test]
    fn sqlite_query_unload_mid_flight_drops_before_drain() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("job_unload");
        load_body(
            "sqlul",
            &format!(
                r#"
            globalThis.__h = 0;
            __s2_sqlite_open("{name}").then(function (h) {{ globalThis.__h = h; }});
        "#,
                name = name
            ),
            "{}",
        );
        // Open resolves synchronously; one drain runs the .then so we have a handle.
        frame_async_drain();
        let handle = read_i32_global_in("sqlul", "__h");
        assert!(handle > 0, "sqlite open must yield a handle, got {handle}");
        eval_in_context(
            "sqlul",
            &format!(r#"__s2_sqlite_query({handle}, "SELECT 1 AS n", []);"#, handle = handle),
        )
        .expect("sqlite query eval");
        assert_eq!(crate::jobs::pending(), 1, "query committed before drain");
        assert!(!crate::jobs::resolver_is_empty());
        unload_plugin("sqlul");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("sqlul")));
        assert_eq!(crate::jobs::pending(), 0, "Job teardown decrements once");
        assert!(crate::jobs::resolver_is_empty());
        for _ in 0..20 {
            frame_async_drain();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(crate::jobs::pending(), 0, "late sqlite completion must not decrement again");
        set_engine_ops(None);
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
        load_body(
            "demo",
            r#"ctx.server.onGameFrame(function(){globalThis.__n=(globalThis.__n||0)+1;});"#,
            "{}",
        );
        dispatch_game_frame_pre_post();
        // The subscribe (the only subscriber) requested the detour INSTALL.
        assert!(
            HOOKS
                .lock()
                .unwrap()
                .iter()
                .any(|(n, e)| n == "OnGameFrame" && *e == 1),
            "the only subscriber must have requested the detour install"
        );
        assert_eq!(
            read_i32_global_in("demo", "__n"),
            1,
            "handler ran once before unload"
        );

        unload_plugin("demo");
        dispatch_game_frame_pre_post(); // demo's handler must NOT run now (context disposed)
        assert!(!FRAME.with(|f| f
            .borrow()
            .snapshot(Phase::Pre)
            .iter()
            .any(|(_, _, o, _)| o == "demo")));
        assert!(
            !PLUGINS.with(|p| p.borrow().contains_key("demo")),
            "context disposed"
        );
        // Earlier isolate tests may leave an uncancellable worker alive. That process-owned
        // obligation intentionally survives shutdown and must finish before detour removal.
        for _ in 0..ASYNC_POLL_TICKS {
            if async_pending() == 0 {
                break;
            }
            frame_async_drain();
            std::thread::sleep(Duration::from_millis(10));
        }
        refresh_detour();
        // No subscriber or surviving producer remains: the detour is removed.
        assert!(
            HOOKS
                .lock()
                .unwrap()
                .iter()
                .any(|(n, e)| n == "OnGameFrame" && *e == 0),
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
        let old_gen = REGISTRY
            .with(|r| r.borrow().generation_of("demo").expect("demo loaded"));

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
        let new_gen = REGISTRY
            .with(|r| r.borrow().generation_of("demo").expect("demo still loaded"));
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

    /// Free `command()` (bound to the factory ctx via `__s2_load_ctx`) registers during the factory
    /// and throws after settle. The JS wrapper returns the handler's HookResult (engine SUPERCEDE
    /// is not this slice).
    #[test]
    fn command_api_registers_during_factory_and_throws_after() {
        init(dummy_logger()).unwrap();
        load_body(
            "cmdapi",
            r#"
            var command = globalThis.__s2_require("@s2script/sdk/commands").command;
            command("sm_x", function (cmd) {
                globalThis.__slot = cmd.callerSlot;
                return HookResult.Handled;
            });
            globalThis.__command = command;
            globalThis.__hook = globalThis.__s2_require("@s2script/sdk/plugin").hook;
        "#,
            "{}",
        );
        assert_eq!(plugin_phase("cmdapi"), Some(crate::plugin::Phase::Active));
        dispatch_concommand("sm_x", -1, "", ReplySource::from_slot(-1));
        assert_eq!(eval_in_context_string("cmdapi", "String(globalThis.__slot)"), "-1");
        let threw = eval_in_context_string(
            "cmdapi",
            r#"
            (function () {
                try { globalThis.__command("sm_late", function () {}); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
        "#,
        );
        assert!(
            threw.contains("outside the load window"),
            "command() after settle must throw, got: {}",
            threw
        );
        let hook_threw = eval_in_context_string(
            "cmdapi",
            r#"
            (function () {
                try { globalThis.__hook.on("x", function () {}); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
        "#,
        );
        assert!(
            hook_threw.contains("outside the load window"),
            "hook.on() after settle must throw, got: {}",
            hook_threw
        );
        shutdown();
    }

    /// `export function OnPluginStart` (no plugin() default) is a valid artifact. Load-window
    /// `command()` registers during OnPluginStart and throws after settle.
    #[test]
    fn on_plugin_start_public_is_a_valid_artifact() {
        init(dummy_logger()).unwrap();
        load_plugin_js(
            "pubstart",
            r#"
            module.exports.OnPluginStart = function () {
                var command = globalThis.__s2_require("@s2script/sdk/commands").command;
                command("sm_x", function (cmd) {
                    globalThis.__slot = cmd.callerSlot;
                    return HookResult.Handled;
                });
                globalThis.__command = command;
            };
            "#,
            "{}",
        );
        assert_eq!(plugin_phase("pubstart"), Some(crate::plugin::Phase::Active));
        dispatch_concommand("sm_x", -1, "", ReplySource::from_slot(-1));
        assert_eq!(eval_in_context_string("pubstart", "String(globalThis.__slot)"), "-1");
        let threw = eval_in_context_string(
            "pubstart",
            r#"
            (function () {
                try { globalThis.__command("sm_late", function () {}); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
        "#,
        );
        assert!(
            threw.contains("outside the load window"),
            "command() after OnPluginStart settle must throw, got: {}",
            threw
        );
        shutdown();
    }

    /// SM-named client/map/say publics subscribe during OnPluginStart's sibling export scan.
    #[test]
    fn sm_publics_client_map_say_and_all_plugins_loaded() {
        init(dummy_logger()).unwrap();
        load_plugin_js(
            "smpub",
            r#"
            globalThis.__log = [];
            module.exports.OnPluginStart = function () {};
            module.exports.OnAllPluginsLoaded = function () { globalThis.__log.push("all"); };
            module.exports.OnClientPutInServer = function (c) { globalThis.__log.push("put:"+c.slot); };
            module.exports.OnClientPostAdminCheck = function (c) { globalThis.__log.push("admin:"+c.slot); };
            module.exports.OnClientDisconnect = function (c) { globalThis.__log.push("disc:"+c.slot); };
            module.exports.OnClientSayCommand = function (slot, text, teamonly) {
                globalThis.__log.push("say:"+slot+":"+text+":"+teamonly);
                return 2;
            };
            module.exports.OnMapStart = function (m) { globalThis.__log.push("start:"+m); };
            module.exports.OnMapEnd = function () { globalThis.__log.push("end"); };
            module.exports.OnConfigsExecuted = function () { globalThis.__log.push("cfg"); };
            "#,
            "{}",
        );
        assert_eq!(plugin_phase("smpub"), Some(crate::plugin::Phase::Active));
        assert_eq!(
            eval_in_context_string("smpub", "globalThis.__log.join(',')"),
            "all",
            "OnAllPluginsLoaded fires once the plugin is Active and the set is quiet"
        );
        let _ = dispatch_client_event("putinserver", 3);
        let _ = dispatch_client_event("fullyconnect", 3);
        let _ = dispatch_client_event("disconnect", 3);
        assert!(dispatch_chat(3, "hello", true), "OnClientSayCommand Handled suppresses");
        let _ = dispatch_map_start("de_a");
        let _ = dispatch_map_start("de_b");
        assert_eq!(
            eval_in_context_string("smpub", "globalThis.__log.join(',')"),
            "all,put:3,admin:3,disc:3,say:3:hello:true,cfg,start:de_a,end,cfg,start:de_b"
        );
        shutdown();
    }

    /// hook.on / topmenu / createScope register during OnPluginStart and throw after settle.
    #[test]
    fn sm_publics_hook_event_and_create_scope() {
        init(dummy_logger()).unwrap();
        load_plugin_js(
            "smhook",
            r#"
            module.exports.OnPluginStart = function () {
                var p = globalThis.__s2_require("@s2script/sdk/plugin");
                p.hook.on("round_start", function () { globalThis.__hits = (globalThis.__hits|0)+1; });
                p.topmenu.addCategory("Server Commands");
                globalThis.__scope = p.createScope();
                globalThis.__hook = p.hook;
                globalThis.__topmenu = p.topmenu;
                globalThis.__createScope = p.createScope;
                globalThis.__onOutput = p.onOutput;
            };
            "#,
            "{}",
        );
        assert_eq!(plugin_phase("smhook"), Some(crate::plugin::Phase::Active));
        let _ = dispatch_game_event("round_start");
        assert_eq!(eval_in_context_string("smhook", "String(globalThis.__hits|0)"), "1");
        let threw = eval_in_context_string(
            "smhook",
            r#"
            (function () {
                try { globalThis.__hook.on("x", function () {}); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
            "#,
        );
        assert!(
            threw.contains("outside the load window"),
            "hook.on after settle must throw, got: {}",
            threw
        );
        let pre_threw = eval_in_context_string(
            "smhook",
            r#"
            (function () {
                try { globalThis.__hook.onPre("x", function () {}); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
            "#,
        );
        assert!(
            pre_threw.contains("outside the load window"),
            "hook.onPre after settle must throw, got: {}",
            pre_threw
        );
        let top_threw = eval_in_context_string(
            "smhook",
            r#"
            (function () {
                try { globalThis.__topmenu.addCategory("x"); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
            "#,
        );
        assert!(
            top_threw.contains("outside the load window"),
            "topmenu after settle must throw, got: {}",
            top_threw
        );
        let scope_threw = eval_in_context_string(
            "smhook",
            r#"
            (function () {
                try { globalThis.__createScope(); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
            "#,
        );
        assert!(
            scope_threw.contains("outside the load window"),
            "createScope after settle must throw, got: {}",
            scope_threw
        );
        let output_threw = eval_in_context_string(
            "smhook",
            r#"
            (function () {
                try { globalThis.__onOutput("*", "*", function () {}); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
            "#,
        );
        assert!(
            output_threw.contains("outside the load window"),
            "onOutput after settle must throw, got: {}",
            output_threw
        );
        shutdown();
    }

    /// Neither plugin() nor OnPluginStart → refused, reason names OnPluginStart.
    #[test]
    fn missing_plugin_and_on_plugin_start_refused() {
        init(dummy_logger()).unwrap();
        load_plugin_js("nopub", "module.exports.foo = 1;", "{}");
        assert!(
            FAILED_PLUGINS.with(|f| f.borrow().get("nopub").map(|r| r.contains("OnPluginStart")).unwrap_or(false)),
            "missing both must refuse naming OnPluginStart, got {:?}",
            FAILED_PLUGINS.with(|f| f.borrow().get("nopub").cloned())
        );
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("nopub")));
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
        assert!(!crate::jobs::resolver_is_empty(), "the delay timer's resolver is ledgered");
        unload_plugin("ul");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("ul")), "context disposed");
        assert!(crate::jobs::resolver_is_empty(), "partial-ledger walk dropped the delay resolver");
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
        // Bare `@s2script/sdk` (no capability) → the authoring barrel (`__s2pkg_sdk`).
        assert!(eval_in_context_bool("dualpfx",
            r#"typeof __s2require("@s2script/sdk").command === "function""#),
            "bare @s2script/sdk must expose command");
        assert!(eval_in_context_bool("dualpfx",
            r#"__s2require("@s2script/sdk").hook === __s2require("@s2script/sdk/plugin").hook"#),
            "barrel hook must be the same object as @s2script/sdk/plugin.hook");
        assert!(eval_in_context_bool("dualpfx",
            r#"__s2require("@s2script/sdk").HookResult.Handled === 2"#),
            "barrel must re-export HookResult");
        // A non-s2script specifier is still null (handled by the JS interop shim).
        assert!(eval_in_context_bool("dualpfx",
            r#"__s2require("@other/x") === null"#));
        shutdown();
    }

    /// `require("@s2script/sdk")` (the CJS spelling esbuild emits for the root barrel) registers
    /// `command` during OnPluginStart.
    #[test]
    fn sdk_barrel_command_registers_during_on_plugin_start() {
        init(dummy_logger()).unwrap();
        load_plugin_js(
            "sdkbar",
            r#"
            module.exports.OnPluginStart = function () {
                var sdk = globalThis.__s2_require("@s2script/sdk");
                sdk.command("sm_x", function (cmd) {
                    globalThis.__slot = cmd.callerSlot;
                    return sdk.HookResult.Handled;
                });
            };
            "#,
            "{}",
        );
        assert_eq!(plugin_phase("sdkbar"), Some(crate::plugin::Phase::Active));
        dispatch_concommand("sm_x", -1, "", ReplySource::from_slot(-1));
        assert_eq!(eval_in_context_string("sdkbar", "String(globalThis.__slot)"), "-1");
        shutdown();
    }

    /// `previous()` + `OnPluginState` round-trip a value across a same-id reload (no plugin() factory).
    #[test]
    fn on_plugin_state_and_previous_roundtrip() {
        init(dummy_logger()).unwrap();
        load_plugin_js(
            "prevpub",
            r#"
            module.exports.OnPluginStart = function () {
                var p = globalThis.__s2_require("@s2script/sdk/plugin");
                var prev = p.previous();
                globalThis.__seen = prev ? prev.n : -1;
                globalThis.__id = p.pluginId();
                globalThis.__previous = p.previous;
            };
            module.exports.OnPluginState = function () { return { n: 7 }; };
            "#,
            "{}",
        );
        assert_eq!(plugin_phase("prevpub"), Some(crate::plugin::Phase::Active));
        assert_eq!(eval_in_context_string("prevpub", "String(globalThis.__seen)"), "-1", "first load: no previous");
        assert_eq!(eval_in_context_string("prevpub", "String(globalThis.__id)"), "prevpub");
        unload_plugin("prevpub");
        load_plugin_js(
            "prevpub",
            r#"
            module.exports.OnPluginStart = function () {
                var p = globalThis.__s2_require("@s2script/sdk/plugin");
                var prev = p.previous();
                globalThis.__seen = prev ? prev.n : -1;
            };
            module.exports.OnPluginState = function () { return { n: 7 }; };
            "#,
            "{}",
        );
        assert_eq!(eval_in_context_string("prevpub", "String(globalThis.__seen)"), "7", "reload sees OnPluginState via previous()");
        shutdown();
    }

    /// Named publics OnGameFrame subscribe at load. SDKHook after settle (not a named public).
    /// Post-simulation frame is createScope().server.onGameFrame({ phase: "post" }), not a SM public.
    /// hook.on still throws after settle.
    extern "C" fn named_publics_damage_victim() -> c_int {
        let bits = crate::entity::HANDLE_ENTRY_BITS;
        ((1u32 << bits) | 5) as c_int
    }
    #[test]
    fn named_publics_frame_and_hook_on_throw_after_settle() {
        init(dummy_logger()).unwrap();
        crate::entity_live::reset_for_tests();
        let id = crate::entity_live::on_created(5, 1);
        set_engine_ops(Some(S2EngineOps {
            damage_victim: Some(named_publics_damage_victim),
            ..mock_event_ops()
        }));
        load_plugin_js(
            "hookmore",
            r#"
            module.exports.OnPluginStart = function () {
                var p = globalThis.__s2_require("@s2script/sdk/plugin");
                globalThis.__hook = p.hook;
                p.createScope().server.onGameFrame(function () { globalThis.__post = (globalThis.__post|0)+1; }, { phase: "post" });
            };
            module.exports.OnGameFrame = function () { globalThis.__frames = (globalThis.__frames|0)+1; };
            "#,
            "{}",
        );
        assert_eq!(plugin_phase("hookmore"), Some(crate::plugin::Phase::Active));
        dispatch_game_frame_pre_post();
        assert_eq!(eval_in_context_string("hookmore", "String(globalThis.__frames|0)"), "1");
        assert_eq!(eval_in_context_string("hookmore", "String(globalThis.__post|0)"), "1");
        assert_eq!(
            eval_in_context_string(
                "hookmore",
                &format!(
                    r#"
                    globalThis.__dmg = 0;
                    String(__s2pkg_sdkhooks.SDKHook({{index:5,id:{id}}}, "OnTakeDamage", function () {{
                        globalThis.__dmg = (globalThis.__dmg|0)+1;
                    }}))
                    "#
                ),
            ),
            "true",
            "SDKHook after settle must succeed"
        );
        dispatch_damage();
        assert_eq!(eval_in_context_string("hookmore", "String(globalThis.__dmg|0)"), "1");
        let threw = eval_in_context_string(
            "hookmore",
            r#"
            (function () {
                try { globalThis.__hook.on("x", function () {}); return "no"; }
                catch (e) { return String(e && e.message || e); }
            })()
            "#,
        );
        assert!(
            threw.contains("outside the load window"),
            "hook.on after settle must throw, got: {}",
            threw
        );
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
        assert!(names.contains(&("CLIENT_CONNECTIONS", ResetPhase::AfterIsolateDrop)));

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

    fn protocol2_setup() {
        let _ = init(dummy_logger());
        let metadata = serde_json::json!({"version":1,"methods":{"getCount":{"args":[],"result":{"kind":"number"}}},"forwards":{"OnCountChanged":{"kind":"notification","payload":{"kind":"object","fields":{"count":{"schema":{"kind":"number"},"optional":false}}}}}});
        use sha2::{Digest, Sha256};
        let sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&metadata).unwrap())
        );
        let contract: crate::interop::Contract =
            serde_json::from_value(serde_json::json!({"metadata":metadata,"sha256":sha256}))
                .unwrap();
        set_plugin_publishes(
            "prod",
            [(
                "@x/counter".into(),
                crate::loader::PublishDecl {
                    version: "1.0.0".into(),
                    types_sha256: "a".repeat(64),
                    contract: Some(contract.clone()),
                },
            )]
            .into_iter()
            .collect(),
        );
        create_plugin_context("prod");
        for consumer in ["cons", "cons2"] {
            set_plugin_imports(
                consumer,
                vec![crate::interfaces::ImportSpec {
                    name: "@x/counter".into(),
                    range: "^1.0.0".into(),
                    kind: crate::interfaces::Kind::Hard,
                    compiled_types_sha256: Some("a".repeat(64)),
                }],
            );
            set_plugin_interop(
                consumer,
                [("@x/counter".into(), contract.clone())]
                    .into_iter()
                    .collect(),
            );
            create_plugin_context(consumer);
        }
        eval_in_context(
            "prod",
            r#"__s2_iface_publish("@x/counter",{getCount:function(){return 1;}})"#,
        )
        .unwrap();
    }

    fn protocol2_decisions_setup() {
        protocol2_setup();
        let mut value = serde_json::to_value(published_contract("@x/counter").unwrap()).unwrap();
        let payload = serde_json::json!({"kind":"object","fields":{
            "identity":{"schema":{"kind":"string"},"optional":false},
            "text":{"schema":{"kind":"string"},"optional":false}}});
        value["metadata"]["forwards"]["OnRequest"] =
            serde_json::json!({"kind":"hook","payload":payload});
        value["metadata"]["forwards"]["OnFormat"] =
            serde_json::json!({"kind":"transform","payload":payload,"writable":["text"]});
        use sha2::{Digest, Sha256};
        value["sha256"] = format!(
            "{:x}",
            Sha256::digest(serde_json_canonicalizer::to_vec(&value["metadata"]).unwrap())
        )
        .into();
        let contract: crate::interop::Contract = serde_json::from_value(value).unwrap();
        assert_eq!(contract.validate(), Ok(()));
        PLUGIN_PUBLISHES.with(|p| {
            p.borrow_mut()
                .get_mut("prod")
                .unwrap()
                .get_mut("@x/counter")
                .unwrap()
                .contract = Some(contract.clone())
        });
        for consumer in ["cons", "cons2"] {
            set_plugin_interop(
                consumer,
                [("@x/counter".into(), contract.clone())]
                    .into_iter()
                    .collect(),
            );
        }
    }

    // Test-only internal registry teardown; this creates no late public registration path.
    fn protocol2_install_teardown_probe() {
        let g_ctx = PLUGINS.with(|p| p.borrow().get("cons").unwrap().context.clone());
        HOST.with(|h| {
            let mut borrow = h.borrow_mut();
            let host = borrow.as_mut().unwrap();
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let ctx = v8::Local::new(&hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(&mut hs, ctx);
            fn teardown(
                scope: &mut v8::PinScope,
                args: v8::FunctionCallbackArguments,
                _: v8::ReturnValue,
            ) {
                if args.get(0).is_number() {
                    REGISTRY.with(|r| {
                        r.borrow_mut().remove("cons");
                    });
                } else if args.get(0).boolean_value(scope) {
                    REGISTRY.with(|r| {
                        r.borrow_mut().remove("prod");
                    });
                    IFACES.with(|r| {
                        r.borrow_mut().remove_by_producer("prod");
                    });
                } else {
                    let dropped =
                        IFACES.with(|r| r.borrow_mut().remove_subscribers_by_consumer("cons2"));
                    IFACE_SUBS.with(|r| {
                        let mut r = r.borrow_mut();
                        for (_, id) in dropped {
                            r.remove(&id);
                        }
                    });
                }
            }
            let global = ctx.global(scope);
            set_native(scope, global, "__test_interop_teardown", teardown);
        });
    }
    #[test]
    fn protocol2_decisions_internal_removal_skips_snapshot_and_nested_calls_work() {
        protocol2_decisions_setup();
        protocol2_install_teardown_probe();
        eval_in_context("cons",r#"globalThis.nested=0;
          __s2_iface_on('@x/counter','OnRequest',()=>{nested=__s2_iface_call('@x/counter','getCount',[]);__test_interop_teardown(false);return 1;});"#).unwrap();
        eval_in_context("cons2",r#"globalThis.called=false;__s2_iface_on('@x/counter','OnRequest',()=>{called=true;return 3;});"#).unwrap();
        assert_eq!(eval_in_context_string("prod","String(__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'}))"),"1");
        assert_eq!(eval_in_context_string("cons", "String(nested)"), "1");
        assert!(!eval_in_context_bool("cons2", "called"));
        shutdown();
    }
    #[test]
    fn protocol2_decisions_provider_removal_during_listener_throws_and_stops() {
        protocol2_decisions_setup();
        protocol2_install_teardown_probe();
        eval_in_context("cons",r#"__s2_iface_on('@x/counter','OnFormat',()=>{__test_interop_teardown(true);return {result:1,patch:{text:'changed'}}});"#).unwrap();
        eval_in_context("cons2",r#"globalThis.called=false;__s2_iface_on('@x/counter','OnFormat',()=>{called=true;return {result:3};});"#).unwrap();
        assert!(eval_in_context_bool(
            "prod",
            r#"(()=>{try{__s2_iface_dispatch('@x/counter','OnFormat',{identity:'a',text:'before'});return false}catch(e){return e.message.includes('InterfaceUnavailable')}})()"#
        ));
        assert!(!eval_in_context_bool("cons2", "called"));
        shutdown();
    }
    #[test]
    fn protocol2_decisions_transform_stop_preserves_earlier_patch() {
        protocol2_decisions_setup();
        eval_in_context("cons",r#"globalThis.called=false;
          __s2_iface_on('@x/counter','OnFormat',()=>({result:1,patch:{text:'changed'}}));
          __s2_iface_on('@x/counter','OnFormat',()=>({result:3}));
          __s2_iface_on('@x/counter','OnFormat',()=>{called=true;return {result:1,patch:{text:'wrong'}}});"#).unwrap();
        assert!(eval_in_context_bool(
            "prod",
            r#"(()=>{const r=__s2_iface_dispatch('@x/counter','OnFormat',{identity:'a',text:'before'});return r.result===3&&r.payload.text==='changed'})()"#
        ));
        assert!(!eval_in_context_bool("cons", "called"));
        shutdown();
    }

    #[test]
    fn protocol2_decisions_nested_recursion_limit_unwinds() {
        protocol2_decisions_setup();
        eval_in_context("prod",r#"__s2_iface_publish('@x/counter',{getCount:()=>__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'})});"#).unwrap();
        eval_in_context("cons",r#"globalThis.limit=false;globalThis.recurse=true;
          __s2_iface_on('@x/counter','OnRequest',()=>{if(!recurse)return 1;try{return __s2_iface_call('@x/counter','getCount',[])}catch(e){limit=e.message.includes('InterfaceRecursionLimit');return 0}});"#).unwrap();
        assert_eq!(eval_in_context_string("prod","String(__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'}))"),"0");
        assert!(eval_in_context_bool("cons", "limit"));
        eval_in_context("cons", "recurse=false").unwrap();
        assert_eq!(eval_in_context_string("prod","String(__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'}))"),"1");
        shutdown();
    }

    #[test]
    fn protocol2_decisions_stale_listener_response_cannot_patch_or_stop() {
        protocol2_decisions_setup();
        protocol2_install_teardown_probe();
        eval_in_context("cons",r#"__s2_iface_on('@x/counter','OnFormat',()=>{__test_interop_teardown(2);return {result:1,patch:{text:'stale'}}});"#).unwrap();
        eval_in_context("cons2",r#"globalThis.seen='';__s2_iface_on('@x/counter','OnFormat',p=>{seen=p.text;return {result:2};});"#).unwrap();
        assert!(eval_in_context_bool(
            "prod",
            r#"(()=>{const r=__s2_iface_dispatch('@x/counter','OnFormat',{identity:'a',text:'before'});return r.result===2&&r.payload.text==='before'})()"#
        ));
        assert_eq!(eval_in_context_string("cons2", "seen"), "before");
        shutdown();
    }

    #[test]
    fn protocol2_decisions_nested_patch_is_copied_and_replaces_whole_field() {
        protocol2_decisions_setup();
        let mut value = serde_json::to_value(published_contract("@x/counter").unwrap()).unwrap();
        value["metadata"]["forwards"]["OnFormat"]["payload"]["fields"]["detail"] = serde_json::json!({"optional":true,"schema":{"kind":"object","fields":{
            "label":{"optional":false,"schema":{"kind":"string"}},"other":{"optional":true,"schema":{"kind":"string"}}}}});
        value["metadata"]["forwards"]["OnFormat"]["writable"] =
            serde_json::json!(["detail", "text"]);
        use sha2::{Digest, Sha256};
        value["sha256"] = format!(
            "{:x}",
            Sha256::digest(serde_json_canonicalizer::to_vec(&value["metadata"]).unwrap())
        )
        .into();
        let contract: crate::interop::Contract = serde_json::from_value(value).unwrap();
        assert_eq!(contract.validate(), Ok(()));
        PLUGIN_PUBLISHES.with(|p| {
            p.borrow_mut()
                .get_mut("prod")
                .unwrap()
                .get_mut("@x/counter")
                .unwrap()
                .contract = Some(contract.clone())
        });
        set_plugin_interop(
            "cons",
            [("@x/counter".into(), contract)].into_iter().collect(),
        );
        eval_in_context("cons",r#"globalThis.patch={detail:{label:'accepted'}};
          __s2_iface_on('@x/counter','OnFormat',()=>({result:1,patch}));
          __s2_iface_on('@x/counter','OnFormat',p=>{patch.detail.label='source mutation';p.detail.label='listener mutation';return {result:0}});"#).unwrap();
        assert!(eval_in_context_bool(
            "prod",
            r#"(()=>{const input={identity:'a',text:'before',detail:{label:'before',other:'remove'}};const r=__s2_iface_dispatch('@x/counter','OnFormat',input);return r.result===1&&r.payload.detail.label==='accepted'&&!('other' in r.payload.detail)&&input.detail.label==='before'&&input.detail.other==='remove'})()"#
        ));
        shutdown();
    }
    #[test]
    fn protocol2_decisions_empty_results_and_modes() {
        protocol2_decisions_setup();
        assert_eq!(
            eval_in_context_string(
                "prod",
                r#"String(__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'}))"#
            ),
            "0"
        );
        assert!(eval_in_context_bool(
            "prod",
            r#"(()=>{const r=__s2_iface_dispatch('@x/counter','OnFormat',{identity:'a',text:'before'});return r.result===0 && r.payload.text==='before' && r.payload.identity==='a';})()"#
        ));
        for expr in [
            "__s2_iface_emit('@x/counter','OnRequest',{identity:'a',text:'before'})",
            "__s2_iface_dispatch('@x/counter','OnCountChanged',{count:1})",
            "__s2_iface_dispatch('@x/counter','missing',{})",
            "__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:4})",
        ] {
            assert!(eval_in_context("prod", expr).is_err(), "{expr}");
        }
        assert!(eval_in_context(
            "cons",
            "__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'})"
        )
        .is_err());
        shutdown();
    }
    #[test]
    fn protocol2_decisions_collapse_max_handled_continues_stop_breaks() {
        protocol2_decisions_setup();
        eval_in_context("cons",r#"globalThis.actions=[0,1,2];globalThis.seen=[];globalThis.copied=true;
          for(let i=0;i<3;i++) __s2_iface_on('@x/counter','OnRequest',p=>{seen.push(i);copied=copied&&p.text==='before';p.text='mutated';return actions[i];});"#).unwrap();
        for (actions, result, seen) in [
            ("[0,0,0]", "0", "0,1,2"),
            ("[1,0,0]", "1", "0,1,2"),
            ("[2,1,0]", "2", "0,1,2"),
            ("[0,3,2]", "3", "0,1"),
        ] {
            eval_in_context("cons", &format!("actions={actions};seen=[];")).unwrap();
            assert_eq!(eval_in_context_string("prod", "String(__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'}))"),result);
            assert_eq!(eval_in_context_string("cons", "seen.join(',')"), seen);
            assert!(eval_in_context_bool("cons", "copied"));
        }
        shutdown();
    }
    #[test]
    fn protocol2_decisions_transform_copies_and_validates_whole_response() {
        protocol2_decisions_setup();
        eval_in_context("cons",r#"globalThis.patch={text:'accepted'};globalThis.seen=[];
         __s2_iface_on('@x/counter','OnFormat',p=>{p.identity='mutated';return {result:1,patch};});
         __s2_iface_on('@x/counter','OnFormat',p=>{seen.push(p.identity+':'+p.text);patch.text='later';return {result:1,patch:{text:'rejected',identity:'forged'}};});
         __s2_iface_on('@x/counter','OnFormat',p=>{seen.push(p.identity+':'+p.text);return {result:2};});
         __s2_iface_on('@x/counter','OnFormat',p=>{seen.push(p.identity+':'+p.text);return {result:0};});"#).unwrap();
        assert!(eval_in_context_bool(
            "prod",
            r#"(()=>{const input={identity:'a',text:'before'};const r=__s2_iface_dispatch('@x/counter','OnFormat',input);return r.result===2 && r.payload.text==='accepted' && r.payload.identity==='a' && input.text==='before';})()"#
        ));
        assert_eq!(
            eval_in_context_string("cons", "seen.join(',')"),
            "a:accepted,a:accepted,a:accepted"
        );
        shutdown();
    }
    #[test]
    fn protocol2_decisions_invalid_responses_contribute_continue_and_no_patch() {
        protocol2_decisions_setup();
        eval_in_context(
            "cons",
            r#"globalThis.reply=()=>0;
         __s2_iface_on('@x/counter','OnRequest',p=>reply());
         __s2_iface_on('@x/counter','OnFormat',p=>reply());"#,
        )
        .unwrap();
        eval_in_context("cons2",r#"globalThis.after=0;__s2_iface_on('@x/counter','OnRequest',()=>{after++;return 0;});__s2_iface_on('@x/counter','OnFormat',()=>{after++;return {result:0};});"#).unwrap();
        let mut attempts = 0;
        for reply in [
            "()=>-1",
            "()=>4",
            "()=>1.5",
            "()=>NaN",
            "()=>Infinity",
            "()=>'3'",
            "()=>undefined",
            "()=>{throw Error('failure')}",
            "async()=>{throw Error('rejection')}",
            "()=>({then(_ok,fail){fail(Error('rejection'))}})",
        ] {
            attempts += 1;
            eval_in_context("cons", &format!("reply={reply}")).unwrap();
            assert_eq!(eval_in_context_string("prod", "String(__s2_iface_dispatch('@x/counter','OnRequest',{identity:'a',text:'before'}))"),"0","{reply}");
        }
        for reply in [
            "()=>({result:4})",
            "()=>({result:1,extra:1})",
            "()=>({result:2,patch:{text:'invalid'}})",
            "()=>({result:1,patch:{text:4}})",
            "()=>({result:1,patch:{text:'x',identity:'forged'}})",
            "()=>({result:1,patch:{text:undefined}})",
            "()=>({result:1,patch:{get text(){throw Error('getter')}}})",
            "async()=>{throw Error('rejection')}",
            "()=>{throw Error('failure')}",
        ] {
            attempts += 1;
            eval_in_context("cons", &format!("reply={reply}")).unwrap();
            assert!(eval_in_context_bool("prod", "(()=>{const r=__s2_iface_dispatch('@x/counter','OnFormat',{identity:'a',text:'before'});return r.result===0&&r.payload.text==='before'&&r.payload.identity==='a'})()"),"{reply}");
        }
        assert_eq!(
            eval_in_context_string("cons2", "String(after)"),
            attempts.to_string()
        );
        assert!(PENDING_REJECTS.with(|r| r.borrow().is_empty()));
        shutdown();
    }
    #[test]
    fn protocol2_notifications_validate_before_any_listener_and_enforce_identity() {
        protocol2_setup();
        eval_in_context("cons",r#"globalThis.received=0; __s2_iface_on("@x/counter","OnCountChanged",function(p){received++;p.count=9;});"#).unwrap();
        eval_in_context("cons2",r#"globalThis.count=0; __s2_iface_on("@x/counter","OnCountChanged",function(p){count=p.count;});"#).unwrap();
        assert!(eval_in_context(
            "prod",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:"bad"})"#
        )
        .is_err());
        assert_eq!(eval_in_context_string("cons", "String(received)"), "0");
        assert!(eval_in_context(
            "cons",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:1})"#
        )
        .is_err());
        assert!(
            eval_in_context("cons", r#"__s2_iface_on("@x/counter","typo",function(){})"#).is_err()
        );
        eval_in_context(
            "prod",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:1})"#,
        )
        .unwrap();
        assert_eq!(eval_in_context_string("cons2", "String(count)"), "1");
        shutdown();
    }

    #[test]
    fn protocol2_rejects_unsupported_values_without_getters_or_partial_delivery() {
        protocol2_setup();
        eval_in_context("cons",r#"globalThis.received=0;__s2_iface_on("@x/counter","OnCountChanged",function(){received++})"#).unwrap();
        for payload in [
            "{count:NaN}",
            "{count:Infinity}",
            "{count:undefined}",
            "{count:1n}",
            "{count:Symbol()}",
            "{count:1,extra:undefined}",
            "{count:1,extra:function(){}}",
            "Object.assign(new Date(),{count:1})",
            "({get count(){globalThis.getterRan=true;return 1;}})",
            "{count:1,[Symbol()]:2}",
            "new Proxy({count:1},{})",
        ] {
            assert!(
                eval_in_context(
                    "prod",
                    &format!("__s2_iface_emit('@x/counter','OnCountChanged',{})", payload)
                )
                .is_err(),
                "{payload}"
            );
        }
        assert_eq!(eval_in_context_string("cons", "String(received)"), "0");
        assert_eq!(
            eval_in_context_string("prod", "String(globalThis.getterRan)"),
            "undefined"
        );
        shutdown();
    }
    #[test]
    fn protocol2_subscriptions_require_declared_dependency_and_matching_hash() {
        protocol2_setup();
        create_plugin_context("rogue");
        assert!(eval_in_context(
            "rogue",
            r#"__s2_iface_on("@x/counter","OnCountChanged",function(){})"#
        )
        .is_err());
        set_plugin_imports(
            "cons",
            vec![crate::interfaces::ImportSpec {
                name: "@x/counter".into(),
                range: "^1.0.0".into(),
                kind: crate::interfaces::Kind::Hard,
                compiled_types_sha256: Some("b".repeat(64)),
            }],
        );
        assert!(eval_in_context(
            "cons",
            r#"__s2_iface_on("@x/counter","OnCountChanged",function(){})"#
        )
        .is_err());
        shutdown();
    }
    #[test]
    fn protocol2_listener_errors_and_thenables_do_not_interrupt_later_notifications() {
        protocol2_setup();
        eval_in_context("cons",r#"__s2_iface_on("@x/counter","OnCountChanged",function(){throw Error('listener failure')}); __s2_iface_on("@x/counter","OnCountChanged",async function(){throw Error('async failure')});"#).unwrap();
        eval_in_context("cons2",r#"globalThis.received=0;__s2_iface_on("@x/counter","OnCountChanged",function(){received++})"#).unwrap();
        for _ in 0..2 {
            eval_in_context(
                "prod",
                r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:1})"#,
            )
            .unwrap();
        }
        assert_eq!(eval_in_context_string("cons2", "String(received)"), "2");
        assert!(
            PENDING_REJECTS.with(|r| r.borrow().is_empty()),
            "thenable rejection must be observed"
        );
        shutdown();
    }
    #[test]
    fn protocol2_method_results_are_validated_and_recursion_recovers() {
        protocol2_setup();
        for result in ["'wrong'", "undefined", "NaN", "Promise.resolve(1)"] {
            eval_in_context(
                "prod",
                &format!(
                    "__s2_iface_publish('@x/counter',{{getCount:function(){{return {};}}}})",
                    result
                ),
            )
            .unwrap();
            assert!(
                eval_in_context("cons", r#"__s2_iface_call("@x/counter","getCount",[])"#).is_err()
            );
        }
        let contract = published_contract("@x/counter").unwrap();
        set_plugin_imports(
            "prod",
            vec![crate::interfaces::ImportSpec {
                name: "@x/counter".into(),
                range: "^1.0.0".into(),
                kind: crate::interfaces::Kind::Hard,
                compiled_types_sha256: Some("a".repeat(64)),
            }],
        );
        set_plugin_interop(
            "prod",
            [("@x/counter".into(), contract)].into_iter().collect(),
        );
        eval_in_context("prod",r#"globalThis.left=31;__s2_iface_publish("@x/counter",{getCount:function(){return left-->0?__s2_iface_call("@x/counter","getCount",[]):7;}});"#).unwrap();
        assert_eq!(
            eval_in_context_string(
                "cons",
                r#"String(__s2_iface_call("@x/counter","getCount",[]))"#
            ),
            "7"
        );
        eval_in_context("prod", "left=32").unwrap();
        assert!(
            eval_in_context("cons", r#"__s2_iface_call("@x/counter","getCount",[])"#)
                .unwrap_err()
                .contains("InterfaceRecursionLimit")
        );
        eval_in_context("prod", "left=0").unwrap();
        assert_eq!(
            eval_in_context_string(
                "cons",
                r#"String(__s2_iface_call("@x/counter","getCount",[]))"#
            ),
            "7"
        );
        shutdown();
    }
    #[test]
    fn protocol2_stale_context_cannot_emit_or_subscribe() {
        protocol2_setup();
        REGISTRY.with(|r| r.borrow_mut().insert("prod"));
        assert!(eval_in_context(
            "prod",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:1})"#
        )
        .is_err());
        REGISTRY.with(|r| r.borrow_mut().insert("cons"));
        assert!(eval_in_context(
            "cons",
            r#"__s2_iface_on("@x/counter","OnCountChanged",function(){})"#
        )
        .is_err());
        shutdown();
    }

    #[test]
    fn protocol2_nested_values_methods_and_interface_names_are_isolated() {
        protocol2_setup();
        let manifest: crate::loader_worker::Manifest = serde_json::from_str(include_str!(
            "../../../packages/sdk/test/fixtures/interop/manifest.json"
        ))
        .unwrap();
        let decl = manifest.publishes["@demo/counter"].clone();
        set_plugin_publishes(
            "second",
            [("@demo/counter".into(), decl.clone())]
                .into_iter()
                .collect(),
        );
        create_plugin_context("second");
        set_plugin_imports(
            "cons2",
            vec![crate::interfaces::ImportSpec {
                name: "@demo/counter".into(),
                range: "^1.0.0".into(),
                kind: crate::interfaces::Kind::Hard,
                compiled_types_sha256: Some(decl.types_sha256),
            }],
        );
        set_plugin_interop(
            "cons2",
            [("@demo/counter".into(), decl.contract.unwrap())]
                .into_iter()
                .collect(),
        );
        eval_in_context("second",r#"globalThis.count=0;__s2_iface_publish("@demo/counter",{getCount:function(){return count;},setCount:function(n){count=n;}})"#).unwrap();
        eval_in_context("cons",r#"globalThis.received=0;__s2_iface_on("@x/counter","OnCountChanged",function(){received++})"#).unwrap();
        eval_in_context("cons2",r#"globalThis.received=0;__s2_iface_on("@demo/counter","OnCountChanged",function(){received++})"#).unwrap();
        assert!(eval_in_context(
            "second",
            r#"__s2_iface_emit("@demo/counter","OnCountChanged",{count:1,detail:{label:3}})"#
        )
        .is_err());
        assert_eq!(eval_in_context_string("cons2", "String(received)"), "0");
        assert!(eval_in_context(
            "cons2",
            r#"__s2_iface_call("@demo/counter","setCount",["wrong"])"#
        )
        .is_err());
        assert_eq!(eval_in_context_string("second", "String(count)"), "0");
        eval_in_context(
            "cons2",
            r#"__s2_iface_call("@demo/counter","setCount",[2])"#,
        )
        .unwrap();
        assert_eq!(eval_in_context_string("second", "String(count)"), "2");
        eval_in_context(
            "second",
            r#"__s2_iface_emit("@demo/counter","OnCountChanged",{count:1,detail:{label:"ok"}})"#,
        )
        .unwrap();
        assert_eq!(eval_in_context_string("cons", "String(received)"), "0");
        assert_eq!(eval_in_context_string("cons2", "String(received)"), "1");
        shutdown();
    }

    fn protocol2_ref_setup() {
        protocol2_setup();
        let mut contract = published_contract("@x/counter").unwrap();
        contract
            .metadata
            .forwards
            .get_mut("OnCountChanged")
            .unwrap()
            .payload = crate::interop::Schema::EntityRef;
        use sha2::{Digest, Sha256};
        contract.sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&serde_json::to_value(&contract.metadata).unwrap()).unwrap()
            )
        );
        set_plugin_publishes(
            "prod",
            [(
                "@x/counter".into(),
                crate::loader::PublishDecl {
                    version: "1.0.0".into(),
                    types_sha256: "a".repeat(64),
                    contract: Some(contract.clone()),
                },
            )]
            .into_iter()
            .collect(),
        );
        for consumer in ["cons", "cons2"] {
            set_plugin_interop(
                consumer,
                [("@x/counter".into(), contract.clone())]
                    .into_iter()
                    .collect(),
            );
        }
        eval_in_context(
            "prod",
            r#"__s2_iface_publish("@x/counter",{getCount:function(){return 1;}})"#,
        )
        .unwrap();
    }

    #[test]
    fn protocol2_entity_refs_keep_the_existing_copy_and_revival_encoding() {
        protocol2_ref_setup();
        eval_in_context(
            "cons",
            r#"__s2_iface_on("@x/counter","OnCountChanged",function(p){p.index=99;})"#,
        )
        .unwrap();
        eval_in_context("cons2",r#"globalThis.result='';__s2_iface_on("@x/counter","OnCountChanged",function(p){result=String(p instanceof __s2pkg_entity.EntityRef)+':'+p.index+':'+p.id;})"#).unwrap();
        eval_in_context(
            "prod",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",new __s2pkg_entity.EntityRef(7,17))"#,
        )
        .unwrap();
        assert_eq!(eval_in_context_string("cons2", "result"), "true:7:17");
        shutdown();
    }

    #[test]
    fn protocol2_entity_refs_reject_coercion_extras_and_spoofed_constructor() {
        protocol2_ref_setup();
        eval_in_context("cons", r#"globalThis.received=0; __s2_iface_on("@x/counter","OnCountChanged",function(){received++;});"#).unwrap();
        eval_in_context("prod", "globalThis.coerced=0;").unwrap();
        assert_eq!(
            eval_in_context_string(
                "prod",
                "JSON.stringify(Reflect.ownKeys(new __s2pkg_entity.EntityRef(7,17)))"
            ),
            r#"["index","id"]"#
        );
        for mutation in [
            "ref.index='7'",
            "ref.id='17'",
            "ref.index={valueOf(){coerced++;return 7}}",
            "ref.id={valueOf(){coerced++;return 17}}",
            "ref.extra=function(){}",
            "ref.extra=undefined",
            "ref.extra=Symbol('x')",
            "ref[Symbol('extra')]=1",
            "Object.defineProperty(ref,'hidden',{value:1})",
            "ref=new (class EntityRef {constructor(){this.index=7;this.id=17}})()",
        ] {
            let source = format!(
                r#"(()=>{{let ref=new __s2pkg_entity.EntityRef(7,17);{mutation};__s2_iface_emit("@x/counter","OnCountChanged",ref);}})()"#
            );
            assert!(
                eval_in_context("prod", &source).is_err(),
                "accepted {mutation}"
            );
            assert_eq!(
                eval_in_context_string("prod", "String(coerced)"),
                "0",
                "coerced {mutation}"
            );
            assert_eq!(
                eval_in_context_string("cons", "String(received)"),
                "0",
                "delivered {mutation}"
            );
        }
        shutdown();
    }

    #[test]
    fn protocol2_stale_consumer_call_and_off_cannot_touch_replacement() {
        protocol2_setup();
        let old = PLUGINS.with(|p| p.borrow().get("cons").unwrap().context.clone());
        let generation = create_plugin_context("cons");
        eval_in_context("cons", r#"globalThis.received=0;__s2_iface_on("@x/counter","OnCountChanged",function(){received++});"#).unwrap();
        let fresh = PLUGINS
            .with(|p| std::mem::replace(&mut p.borrow_mut().get_mut("cons").unwrap().context, old));
        let before = REGISTRY.with(|r| r.borrow().active_resource_count("cons", generation));
        let call = eval_in_context("cons", r#"__s2_iface_call("@x/counter","getCount",[])"#);
        let off = eval_in_context("cons", r#"__s2_iface_off("@x/counter","OnCountChanged")"#);
        PLUGINS.with(|p| p.borrow_mut().get_mut("cons").unwrap().context = fresh);
        assert!(call.is_err());
        assert!(off.is_err());
        assert_eq!(
            REGISTRY.with(|r| r.borrow().active_resource_count("cons", generation)),
            before
        );
        eval_in_context(
            "prod",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:1})"#,
        )
        .unwrap();
        assert_eq!(eval_in_context_string("cons", "String(received)"), "1");
        shutdown();
    }
    #[test]
    fn protocol2_snapshot_skips_disposed_rows_and_defers_new_rows_during_reentrant_calls() {
        protocol2_setup();
        eval_in_context(
            "cons",
            r#"globalThis.result=0;
          __s2_iface_on("@x/counter","OnCountChanged",function(){
            result=__s2_iface_call("@x/counter","getCount",[]);
            __s2_iface_off("@x/counter","OnCountChanged");
            __s2_iface_on("@x/counter","OnCountChanged",function(){result+=10;});
          });
          __s2_iface_on("@x/counter","OnCountChanged",function(){result=999;});"#,
        )
        .unwrap();
        eval_in_context(
            "prod",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:1})"#,
        )
        .unwrap();
        assert_eq!(eval_in_context_string("cons", "String(result)"), "1");
        eval_in_context(
            "prod",
            r#"__s2_iface_emit("@x/counter","OnCountChanged",{count:1})"#,
        )
        .unwrap();
        assert_eq!(eval_in_context_string("cons", "String(result)"), "11");
        shutdown();
    }

    #[test]
    fn protocol2_stale_provider_is_unavailable_to_fresh_consumers() {
        protocol2_setup();
        REGISTRY.with(|r| r.borrow_mut().insert("prod"));
        assert!(!eval_in_context_bool(
            "cons",
            r#"__s2_iface_is_published("@x/counter")"#
        ));
        assert!(eval_in_context(
            "cons",
            r#"__s2_iface_on("@x/counter","OnCountChanged",function(){})"#
        )
        .is_err());
        shutdown();
    }

    #[test]
    fn iface_publish_records_methods_and_dep_kind() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "2.5.0".into(), types_sha256: "abc".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "h".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "h".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "h".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "h".into() },
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
        let decl = crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "h".into() };
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
    fn republishing_an_interface_keeps_one_active_ledger_entry() {
        let _ = init(dummy_logger());
        set_plugin_publishes("prod", [(
            "@x/hot".to_string(),
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "h".into() },
        )].into_iter().collect());
        create_plugin_context("prod");
        eval_in_context("prod", r#"__s2_iface_publish("@x/hot", { a:function(){return 1;} });"#)
            .expect("first publish");
        eval_in_context("prod", r#"__s2_iface_publish("@x/hot", { a:function(){return 2;} });"#)
            .expect("hot republish");
        assert_eq!(active_resources("prod"), 1);
        unload_plugin("prod");
        shutdown();
    }

    #[test]
    fn publish_interface_of_a_name_owned_by_another_producer_is_refused() {
        let _ = init(dummy_logger());
        let decl = crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "h".into() };
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
            crate::loader::PublishDecl { contract: None, version: "1.4.0".into(), types_sha256: "abc".into() },
        )].into_iter().collect());
        eval_setup("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/greeter", { greet: function (n) { return "hi " + n.who; } });
        "#);
        let v = IFACES.with(|r| r.borrow().lookup("@x/greeter").map(|e| e.version.clone()));
        assert_eq!(v, Some("1.4.0".to_string()));
        shutdown();
    }

    #[test]
    fn interface_subscription_off_releases_its_active_ledger_entry() {
        let _ = init(dummy_logger());
        set_plugin_publishes("prod", [(
            "@x/events".to_string(),
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
        load_body("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/events", {});
        "#, "{}");
        eval_std("cons", r#"
            globalThis.__handler = function () {};
            __s2_iface_on("@x/events", "changed", globalThis.__handler);
        "#);
        assert_eq!(active_resources("cons"), 1);
        eval_in_context_string("cons", "__s2_iface_off('@x/events', 'changed', __handler); 'off'");
        assert_eq!(active_resources("cons"), 0);
        unload_plugin("cons");
        unload_plugin("prod");
        shutdown();
    }

    #[test]
    fn replacing_manifest_imports_releases_old_import_entries() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new(
            "@x/dep", "^1.0.0", crate::interfaces::Kind::Hard,
        )]);
        create_plugin_context("cons");
        assert_eq!(active_resources("cons"), 1);
        set_plugin_imports("cons", Vec::new());
        assert_eq!(active_resources("cons"), 0);
        unload_plugin("cons");
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "aaa111".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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

    #[test]
    fn producer_reload_releases_surviving_optional_consumer_subscriptions() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new(
            "@x/events", "^1.0.0", crate::interfaces::Kind::Optional,
        )]);
        let publish_decl = || [(
            "@x/events".to_string(),
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect();
        let publish_body = r#"const {publishInterface}=require("@s2script/interfaces");
            publishInterface("@x/events", {});"#;

        set_plugin_publishes("prod", publish_decl());
        load_body("prod", publish_body, "{}");
        load_body("cons", r#"globalThis.__events=require("@x/events");
            globalThis.__handler=function(){};
            __events.on("changed", __handler);"#, "{}");
        assert_eq!(active_resources("cons"), 2, "one import and one event subscription");
        assert_eq!(IFACE_SUBS.with(|m| m.borrow().len()), 1);

        for _ in 0..2 {
            unload_plugin("prod");
            assert_eq!(active_resources("cons"), 1, "producer removal releases the dead subscription");
            assert!(IFACE_SUBS.with(|m| m.borrow().is_empty()), "producer removal drops callback Globals");
            eval_in_context("cons", r#"__events.off("changed", __handler);"#)
                .expect("off after producer removal is harmless");
            assert_eq!(active_resources("cons"), 1);

            set_plugin_publishes("prod", publish_decl());
            load_body("prod", publish_body, "{}");
            eval_in_context("cons", r#"__events.on("changed", __handler);"#)
                .expect("optional consumer resubscribes after producer reload");
            assert_eq!(active_resources("cons"), 2);
            assert_eq!(IFACE_SUBS.with(|m| m.borrow().len()), 1);
        }

        unload_plugin("cons");
        assert!(IFACE_SUBS.with(|m| m.borrow().is_empty()));
        assert_eq!(IFACES.with(|r| r.borrow().lookup("@x/events").unwrap().subscribers.len()), 0);
        unload_plugin("prod");
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
        crate::client::begin(5);
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
        crate::client::begin(4);
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

    /// P0-1: a subscription made at PRELUDE-eval time (before the plugin's `PluginInstance` lands
    /// in PLUGINS) must stamp the plugin's REAL generation and dispatch — for EVERY plugin, not
    /// just the process's first. The old code stamped such subscriptions with the `unwrap_or(0)`
    /// fallback, and the registry's first minted generation was ALSO 0, so the first plugin's
    /// prelude subscriptions accidentally fired while every later plugin's were silently dropped
    /// (this is why the cs2 prelude's HUD click hook and ui.js's onMapStart worked for exactly one
    /// plugin on a live server). Synthetic prelude — engine-generic, no CS2 names.
    #[test]
    fn prelude_time_subscription_is_live_for_every_plugin_not_only_the_first() {
        let _ = init(dummy_logger());
        register_injected_package(
            "@s2script/cs2",
            // Subscribe DURING prelude eval — the exact window where the generation stamp used to
            // read 0 because the PluginInstance is not in PLUGINS yet.
            r#"__s2_event_subscribe("round_start", function () {
                   globalThis.__prelude_hits = (globalThis.__prelude_hits || 0) + 1;
               });
               globalThis.__s2pkg_cs2 = {};"#,
        );
        load_body("first", "", "{}");
        load_body("second", "", "{}");
        let _ = crate::events::dispatch_game_event("round_start");
        assert_eq!(read_i32_global_in("first", "__prelude_hits"), 1,
            "the first plugin's prelude subscription must dispatch (and exactly once)");
        assert_eq!(read_i32_global_in("second", "__prelude_hits"), 1,
            "a prelude subscription from a plugin that is NOT the process's first must dispatch too \
             — it must never be stamped with the never-live generation 0 and silently dropped");
        // And reload must still invalidate: the second plugin's reload re-runs the prelude, whose
        // NEW subscription (new generation) fires while the OLD row is dead — one hit, not two.
        load_body("second", "", "{}");
        let _ = crate::events::dispatch_game_event("round_start");
        assert_eq!(read_i32_global_in("second", "__prelude_hits"), 1,
            "after a reload only the new instance's prelude subscription is live");
        // Registered packages survive shutdown (a process-lifetime registry): overwrite the
        // subscribing prelude with a no-op so it cannot leak a round_start handler into every
        // later test on this thread.
        register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
            crate::loader::PublishDecl { contract: None, version: "1.0.0".into(), types_sha256: "test".into() },
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
        crate::client::begin(0);
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
        crate::client::begin(0);
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
        crate::client::ensure(5);
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
        crate::client::ensure(5);
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

    // Damage OnTakeDamage SDKHook fan-out, Handled/Stop collapse, and Handled-zeroes-live-damage
    // live in `crate::sdkhooks` isolate tests (`sdkhook_*`). A global Damage.onPre mux no longer exists.

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

    #[test]
    fn sqlite_materialization_retains_bytes_through_reentrant_then_lookup() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("retained_materialization");
        load_body(
            "retained",
            &format!(
                r#"
            globalThis.__out = "pending";
            globalThis.__timer = false;
            __s2_sqlite_open("{name}").then(function (h) {{
                Object.defineProperty(Object.prototype, "resultcol", {{configurable:true, set:function () {{ throw Error("column setter"); }}}});
                Object.defineProperty(Array.prototype, "0", {{configurable:true, set:function () {{ throw Error("index setter"); }}}});
                Object.defineProperty(Array.prototype, "then", {{configurable:true, get:function () {{
                    var stats = JSON.parse(__s2_async_stats());
                    globalThis.__retained = stats.completion.items > 0 && stats.completion.bytes > 0;
                    __s2_next_frame().then(function () {{ globalThis.__timer = true; }});
                    return undefined;
                }}}});
                __s2_sqlite_query(h, "SELECT 'value' AS resultcol", []).then(function (rows) {{
                    delete Object.prototype.resultcol;
                    delete Array.prototype[0];
                    delete Array.prototype.then;
                    globalThis.__out = String(globalThis.__retained) + ":" + rows[0].resultcol;
                    __s2_sqlite_close(h);
                }}).catch(function(e) {{ globalThis.__out = "ERROR:" + String(e); }});
            }});
        "#
            ),
            "{}",
        );
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if read_global_string("retained", "__out") != "pending"
                && read_global_string("retained", "__timer") == "true"
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(read_global_string("retained", "__out"), "true:value");
        assert_eq!(read_global_string("retained", "__timer"), "true");
        shutdown();
    }

    #[test]
    fn explicit_sqlite_close_releases_connection_before_unload() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("ledger_close");
        load_body("dbledger", &format!(r#"
            globalThis.__state = "pending";
            __s2_sqlite_open("{name}").then(function (h) {{
                globalThis.__handle = h;
                globalThis.__state = "open";
            }});
        "#), "{}");
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            if read_global_string("dbledger", "__state") == "open" { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(read_global_string("dbledger", "__state"), "open");
        assert_eq!(active_resources("dbledger"), 1, "only the open connection remains active");
        eval_in_context("dbledger", r#"
            __s2_sqlite_close(globalThis.__handle).then(function () {
                globalThis.__state = "closed";
            });
        "#).unwrap();
        frame_async_drain();
        assert_eq!(read_global_string("dbledger", "__state"), "closed");
        assert_eq!(active_resources("dbledger"), 0);
        unload_plugin("dbledger");
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
    /// `ws::retire_conn` cleanup of the now-dead registry entry).
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
        assert_eq!(active_resources("wsbad"), 0, "failed connect releases job and connection");
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
                ws.onMessage(function (m) {{ ws.close(); globalThis.__out = String(globalThis.__accepted) + ":" + String(ws.send("late")) + ":" + m; }});
                globalThis.__accepted = ws.send("hi");
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
        assert_eq!(read_global_string("wsmod", "__out"), "true:false:hi");
        shutdown();
    }

    /// Regression: a plugin that calls `ws.close()` from inside its OWN `onMessage` handler —
    /// exactly `plugins/ws-demo`'s pattern (log the echo, then close) — must still see `onClose`
    /// fire. A self-initiated close used to be a silent `write.send(Close) + break` with NO
    /// `WsSignal` emitted, so `onClose` (and the ledger's `ws::retire_conn` cleanup, which
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
            globalThis.__trace = [];
            WebSocket.connect("ws://127.0.0.1:{port}/").then(function (ws) {{
                globalThis.__trace.push("connected");
                ws.onClose(function (code, reason) {{
                    globalThis.__trace.push("closed");
                    globalThis.__out = globalThis.__trace.join(",") + ":" + code;
                }});
            }}).catch(function (e) {{ globalThis.__out = "rejected:" + String(e); }});
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
        assert!(settled, "neither onClose nor catch ever ran: the close was delivered to nobody");
        let out = read_global_string("wsdead", "__out");
        assert!(
            out.starts_with("connected,closed:"),
            "the connect continuation must subscribe before the same-batch close dispatch; got {out:?}"
        );
        shutdown();
    }

    #[test]
    fn ws_same_batch_connect_message_error_close_keeps_subscription_checkpoint() {
        init(dummy_logger()).unwrap();
        load_body("wsbatch", "", "{}");
        let (id, _) = seed_injected_socket_connect("wsbatch", true);
        eval_in_context("wsbatch", r#"
            globalThis.__trace = [];
            globalThis.__injected_connect.then(function (id) {
                globalThis.__trace.push("connect:" + __test_socket_owned(id, true));
                __s2_ws_on(id, "message", function () { globalThis.__trace.push("message:" + __test_socket_owned(id, true)); });
                __s2_ws_on(id, "error", function () { globalThis.__trace.push("error:" + __test_socket_owned(id, true)); });
                __s2_ws_on(id, "close", function () { globalThis.__trace.push("close:" + __test_socket_owned(id, true)); });
            });
        "#).unwrap();
        crate::ws::test_inject_batch(id, "payload", "boom");
        assert!(crate::ws::is_owner(id, "wsbatch"));

        frame_async_drain();
        assert_eq!(eval_in_context_string("wsbatch", "globalThis.__trace.join(',')"), "connect:true");
        assert!(crate::ws::is_owner(id, "wsbatch"), "terminal-pending row survives the checkpoint");
        dispatch_pending_ws_events();
        assert_eq!(eval_in_context_string("wsbatch", "globalThis.__trace.join(',')"), "connect:true,message:true,error:true,close:true");
        assert!(!crate::ws::is_owner(id, "wsbatch"));
        assert_eq!(crate::ws::test_mux_count(id), 0);
        assert_eq!(active_resources("wsbatch"), 0);

        crate::ws::test_inject_batch(id, "late", "late");
        frame_async_drain();
        dispatch_pending_ws_events();
        assert_eq!(eval_in_context_string("wsbatch", "globalThis.__trace.join(',')"), "connect:true,message:true,error:true,close:true");
        assert_eq!(active_resources("wsbatch"), 0);
        shutdown();
    }

    #[test]
    fn net_same_batch_connect_data_error_close_keeps_subscription_checkpoint() {
        init(dummy_logger()).unwrap();
        load_body("netbatch", "", "{}");
        let (id, _) = seed_injected_socket_connect("netbatch", false);
        eval_in_context("netbatch", r#"
            globalThis.__trace = [];
            globalThis.__injected_connect.then(function (id) {
                globalThis.__trace.push("connect:" + __test_socket_owned(id, false));
                __s2_net_on(id, "data", function () { globalThis.__trace.push("data:" + __test_socket_owned(id, false)); });
                __s2_net_on(id, "error", function () { globalThis.__trace.push("error:" + __test_socket_owned(id, false)); });
                __s2_net_on(id, "close", function () { globalThis.__trace.push("close:" + __test_socket_owned(id, false)); });
            });
        "#).unwrap();
        crate::net::test_inject_batch(id, b"payload", "boom");
        assert!(crate::net::is_owner(id, "netbatch"));

        frame_async_drain();
        assert_eq!(eval_in_context_string("netbatch", "globalThis.__trace.join(',')"), "connect:true");
        assert!(crate::net::is_owner(id, "netbatch"), "terminal-pending row survives the checkpoint");
        dispatch_pending_net_events();
        assert_eq!(eval_in_context_string("netbatch", "globalThis.__trace.join(',')"), "connect:true,data:true,error:true,close:true");
        assert!(!crate::net::is_owner(id, "netbatch"));
        assert_eq!(crate::net::test_mux_count(id), 0);
        assert_eq!(active_resources("netbatch"), 0);

        crate::net::test_inject_batch(id, b"late", "late");
        frame_async_drain();
        dispatch_pending_net_events();
        assert_eq!(eval_in_context_string("netbatch", "globalThis.__trace.join(',')"), "connect:true,data:true,error:true,close:true");
        assert_eq!(active_resources("netbatch"), 0);
        shutdown();
    }

    #[test]
    fn final_socket_callback_keeps_detour_for_its_promise_continuation() {
        init(dummy_logger()).unwrap();
        load_body("lastcallback", "", "{}");
        let (id, _) = seed_injected_socket_connect("lastcallback", false);
        eval_in_context(
            "lastcallback",
            r#"
            globalThis.__continued = false;
            __injected_connect.then(function (id) {
                __s2_net_on(id, "close", function () {
                    Promise.resolve().then(function () { globalThis.__continued = true; });
                });
            });
        "#,
        )
        .unwrap();
        crate::net::test_inject_batch(id, b"payload", "terminal");
        for _ in 0..16 {
            frame_async_drain();
            dispatch_async_callbacks();
            if !crate::net::is_owner(id, "lastcallback") {
                break;
            }
        }
        assert!(!crate::net::is_owner(id, "lastcallback"));
        assert_eq!(read_global_string("lastcallback", "__continued"), "false");
        assert!(
            MICROTASK_DRAIN_NEEDED.with(|v| v.get()),
            "last callback leaves a checkpoint obligation"
        );
        assert!(DETOUR_INSTALLED.with(|v| v.get()));
        frame_async_drain();
        dispatch_async_callbacks();
        assert_eq!(read_global_string("lastcallback", "__continued"), "true");
        assert!(!MICROTASK_DRAIN_NEEDED.with(|v| v.get()));
        shutdown();
    }

    #[test]
    fn ws_thousand_production_workers_plateau_every_lifecycle_store() {
        init(dummy_logger()).unwrap();
        load_body("wsstress", "", "{}");
        let generation = REGISTRY.with(|r| r.borrow().generation_of("wsstress").unwrap());
        let worker_baseline = crate::ws::active_worker_count();
        let conn_baseline = crate::ws::active_conn_count();
        let pending_baseline = crate::ws::test_pending_count();
        let ledger_baseline = active_resources("wsstress");
        for cycle in 0..1000u64 {
            let id = 600_000 + cycle;
            let resource = plugin::Resource::WsConn(id);
            assert!(record_resource("wsstress", generation, resource.clone()));
            assert!(!release_resource("wsstress", generation + 1, &resource), "stale generation released cycle {cycle}");
            if cycle % 2 == 0 {
                crate::ws::test_spawn_terminal_worker(id, "wsstress".into(), generation);
                eval_in_context("wsstress", &format!("__s2_ws_on({id}, 'close', function () {{}});" )).unwrap();
                assert_eq!(crate::ws::test_mux_count(id), 1);
                for _ in 0..200 {
                    let _ = crate::ws::poll_signals();
                    if crate::ws::test_pending_count() >= 3 { break; }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert!(crate::ws::test_pending_count() >= 3, "worker terminal missing at cycle {cycle}");
                dispatch_pending_ws_events();
            } else {
                crate::ws::test_spawn_failed_worker(id, "wsstress".into(), generation);
                let mut retired = false;
                for _ in 0..200 {
                    let poll = crate::ws::poll_signals();
                    if poll.drops.contains(&id) {
                        crate::ws::retire_conn(id);
                        retired = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert!(retired, "worker connect failure missing at cycle {cycle}");
            }
            crate::ws::retire_conn(id);
            crate::ws::shutdown_conn(id);
            for _ in 0..200 {
                if crate::ws::active_worker_count() == worker_baseline { break; }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(crate::ws::active_worker_count(), worker_baseline);
            assert_eq!(crate::ws::active_conn_count(), conn_baseline);
            assert_eq!(crate::ws::test_pending_count(), pending_baseline);
            assert_eq!(crate::ws::test_mux_count(id), 0);
            assert_eq!(active_resources("wsstress"), ledger_baseline);
        }
        assert!(crate::ws::try_recv_signal().is_none());
        shutdown();
    }

    #[test]
    fn net_thousand_production_workers_plateau_every_lifecycle_store() {
        init(dummy_logger()).unwrap();
        load_body("netstress", "", "{}");
        let generation = REGISTRY.with(|r| r.borrow().generation_of("netstress").unwrap());
        let worker_baseline = crate::net::active_worker_count();
        let conn_baseline = crate::net::active_conn_count();
        let pending_baseline = crate::net::test_pending_count();
        let ledger_baseline = active_resources("netstress");
        for cycle in 0..1000u64 {
            let id = 700_000 + cycle;
            let resource = plugin::Resource::NetConn(id);
            assert!(record_resource("netstress", generation, resource.clone()));
            assert!(!release_resource("netstress", generation + 1, &resource), "stale generation released cycle {cycle}");
            if cycle % 2 == 0 {
                crate::net::test_spawn_terminal_worker(id, "netstress".into(), generation);
                eval_in_context("netstress", &format!("__s2_net_on({id}, 'close', function () {{}});" )).unwrap();
                assert_eq!(crate::net::test_mux_count(id), 1);
                for _ in 0..200 {
                    let _ = crate::net::poll_signals();
                    if crate::net::test_pending_count() >= 1 { break; }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert!(crate::net::test_pending_count() >= 1, "worker terminal missing at cycle {cycle}");
                dispatch_pending_net_events();
            } else {
                crate::net::test_spawn_failed_worker(id, "netstress".into(), generation);
                let mut retired = false;
                for _ in 0..200 {
                    let poll = crate::net::poll_signals();
                    if poll.drops.contains(&id) {
                        crate::net::retire_conn(id);
                        retired = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert!(retired, "worker connect failure missing at cycle {cycle}");
            }
            crate::net::retire_conn(id);
            crate::net::shutdown_conn(id);
            for _ in 0..200 {
                if crate::net::active_worker_count() == worker_baseline { break; }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(crate::net::active_worker_count(), worker_baseline);
            assert_eq!(crate::net::active_conn_count(), conn_baseline);
            assert_eq!(crate::net::test_pending_count(), pending_baseline);
            assert_eq!(crate::net::test_mux_count(id), 0);
            assert_eq!(active_resources("netstress"), ledger_baseline);
        }
        assert!(crate::net::try_recv_signal().is_none());
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
        assert_eq!(active_resources("wsclose"), 0, "explicit close releases job and connection");
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
                    __s2_net_close(id);
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
        for _ in 0..ASYNC_POLL_TICKS {
            frame_async_drain();
            dispatch_pending_net_events();
            if active_resources("netp") == 0 { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(active_resources("netp"), 0, "explicit close releases job and connection");
        shutdown();
    }

    /// A TCP connect failure (connection refused — port 1) REJECTS the connect Promise (the `.catch`
    /// runs) rather than resolving or panicking — proves `resolve_net_connect`'s `Err` branch + the
    /// drain's `ConnectFailed` routing (incl. `net::retire_conn` cleanup of the dead registry entry).
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
        assert_eq!(active_resources("netbad"), 0, "failed connect releases job and connection");
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
    fn menu_renderer_can_delegate_to_the_replaced_presenter() {
        init(dummy_logger()).unwrap();
        let out = eval_std("menu_delegate", r#"
            var Menu = globalThis.__s2pkg_menu.Menu;
            var calls = [];
            Menu.registerRenderer("delegate", {
                open: function (s) { calls.push("open:" + s.slot); },
                update: function () {},
                close: function (slot) { calls.push("close:" + slot); }
            });
            var previous = Menu.registerRenderer("delegate", {
                open: function (s) { previous.open(s); },
                update: function (s) { previous.update(s); },
                close: function (slot) { previous.close(slot); }
            });
            var m = new Menu("T");
            m.style = "delegate";
            m.addItem("one", "One");
            m.display(3, 0);
            m.close(3);
            JSON.stringify(calls);
        "#);
        assert_eq!(out, r#"["open:3","close:3"]"#);
        shutdown();
    }

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
    fn menu_activation_defaults_and_coerces() {
        init(dummy_logger()).unwrap();
        let out = eval_std("ma", r#"
            var m = new globalThis.__s2pkg_menu.Menu("T");
            var def = m.activation;
            m.activation = "tab";
            var tab = m.activation;
            m.activation = "nope";
            var nope = m.activation;
            JSON.stringify({ def: def, tab: tab, nope: nope });
        "#);
        assert_eq!(out, r#"{"def":"immediate","tab":"tab","nope":"immediate"}"#);
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

    #[test]
    fn topmenu_snapshot_sheets_defaults_and_validates() {
        init(dummy_logger()).unwrap();
        let out = eval_std("tm_sheets", r#"
            var T = globalThis.__s2pkg_topmenu.TopMenu;
            T.addItem("Player Commands", { id: "sheet:default", name: "Default", flags: 1, onSelect: function(){} });
            T.addItem("Player Commands", { id: "sheet:menu", name: "Menu", flags: 2, sheets: ["menu"], onSelect: function(){} });
            T.addItem("Player Commands", { id: "sheet:both", name: "Both", flags: 4, sheets: ["admin", "menu"], onSelect: function(){} });
            var invalid = false, message = "";
            try {
                T.addItem("Player Commands", { id: "sheet:invalid", name: "Invalid", flags: 8, sheets: ["bogus"], onSelect: function(){} });
            } catch (e) {
                invalid = true;
                message = String(e);
            }
            var items = T.snapshot().items;
            function sheets(id) {
                return items.filter(function (item) { return item.id === id; })[0].sheets;
            }
            JSON.stringify({
                defaultSheets: sheets("sheet:default"),
                menuSheets: sheets("sheet:menu"),
                bothSheets: sheets("sheet:both"),
                invalid: invalid,
                messageHasName: message.indexOf("TopMenuInvalidSheet") !== -1,
                messageHasSheet: message.indexOf("bogus") !== -1
            });
        "#);
        assert_eq!(
            out,
            r#"{"defaultSheets":["admin"],"menuSheets":["menu"],"bothSheets":["admin","menu"],"invalid":true,"messageHasName":true,"messageHasSheet":true}"#
        );
        shutdown();
    }

    #[test]
    fn topmenu_add_tab_sets_title_and_keeps_order() {
        init(dummy_logger()).unwrap();
        let out = eval_std("tm_tabs", r#"
            var T = globalThis.__s2pkg_topmenu.TopMenu;
            T.addCategory("Player Commands");
            T.addTab({ id: "playercommands", title: "Players" });
            T.addItem("playercommands", { id: "pc:slap", name: "Slap", flags: 16, onSelect: function(){} });
            T.addTab({ id: "playercommands", title: "Player" });
            var s = T.snapshot();
            JSON.stringify({
                cats: s.categories,
                tabs: s.tabs,
                itemCat: s.items[0].category
            });
        "#);
        assert_eq!(
            out,
            r#"{"cats":["Player Commands","playercommands"],"tabs":[{"id":"Player Commands","title":"Player Commands"},{"id":"playercommands","title":"Player"}],"itemCat":"playercommands"}"#
        );
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
    fn votes_registered_renderer_paints_without_show_live_tally() {
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
            V.start({ question:"Q", options:["A","B"], duration:1, onEnd:function(){} });
            chatHandler(0, "1");
            while (delayed.length) delayed.shift()();
            String(calls > 0);
        "#);
        assert_eq!(out, "true");
        shutdown();
    }

    #[test]
    fn votes_tally_choice_is_per_slot() {
        init(dummy_logger()).unwrap();
        let out = eval_std("vt4b", r#"
            var chatHandler = null, delayed = [], choices = [];
            globalThis.__s2pkg_chat.Chat.toAll = function () {};
            globalThis.__s2_chat_on_message = function (fn) { chatHandler = fn; };
            globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false},{slot:1,isBot:false}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
            var V = globalThis.__s2pkg_votes.Vote;
            V.registerTallyRenderer({
                show: function (slot, t) { choices.push(slot + ":" + t.choice); },
                clear: function () {}
            });
            V.start({ question:"Q", options:["A","B"], duration:2, onEnd:function(){} });
            chatHandler(0, "1");
            globalThis.__s2_vote_cast(1, 1);
            JSON.stringify({ start0: choices[0], start1: choices[1], after: choices.slice(-2) });
        "#);
        assert_eq!(out, r#"{"start0":"0:null","start1":"1:null","after":["0:0","1:1"]}"#);
        shutdown();
    }

    #[test]
    fn votes_chat_is_one_line() {
        init(dummy_logger()).unwrap();
        let out = eval_std("vt4c", r#"
            var sent = [];
            globalThis.__s2pkg_chat.Chat.toAll = function (m) { sent.push(m); };
            globalThis.__s2_chat_on_message = function () {};
            globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
            globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false}]; };
            globalThis.__s2pkg_timers.delay = function () { return { then: function () { return { then: function () {} }; } }; };
            globalThis.__s2pkg_votes.Vote.start({ question:"Kick Rex?", options:["Yes","No"], duration:2, onEnd:function(){} });
            JSON.stringify(sent);
        "#);
        assert_eq!(out, r#"["[Vote] Kick Rex? — Tab, or type 1–2"]"#);
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
    #[test]
    #[ignore = "requires the isolated six-poll S2SCRIPT_ASYNC_LIMITS_JSON policy"]
    fn oversized_completions_progress_with_a_full_poll_round_and_due_timer() {
        assert_eq!(crate::async_limits::policy().frame_poll_items, 6);
        assert_eq!(crate::async_limits::policy().frame_items, 2);
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(db_ops()));
        let name = unique_db_name("oversize_round");
        let port = spawn_local_http_server("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        load_body(
            "round",
            &format!(
                r#"
            globalThis.__http = false; globalThis.__db = false; globalThis.__ticks = 0;
            __s2_sqlite_open("{name}").then(function(h) {{
                __s2_sqlite_query(h, "SELECT 1 AS x", []).then(function() {{ __db = true; }});
                __s2_fetch("http://127.0.0.1:{port}/", {{}}).then(function() {{ __http = true; }});
            }});
        "#
            ),
            "{}",
        );
        frame_async_drain();
        let deadline = Instant::now() + Duration::from_secs(2);
        while crate::async_limits::queued_metrics()["http"] != 1
            || crate::async_limits::queued_metrics()["db"] != 1
        {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        eval_in_context(
            "round",
            "__s2_timer_create(1, function(){ __ticks++; }, true);",
        )
        .unwrap();
        POLL_CURSOR.with(|v| v.set(0));
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(2));
            frame_async_drain();
            dispatch_async_callbacks();
        }
        assert_eq!(
            read_global_string("round", "__http"),
            "true",
            "HTTP must obtain an empty frame"
        );
        assert_eq!(
            read_global_string("round", "__db"),
            "true",
            "DB must obtain an empty frame"
        );
        assert!(eval_in_context_string("round", "String(__ticks > 0)") == "true");
        shutdown();
    }

    /// Run by scripts/test-async-pressure.sh in a fresh process with an explicitly tiny policy.
    #[test]
    #[ignore = "requires the isolated tiny S2SCRIPT_ASYNC_LIMITS_JSON policy"]
    fn async_tiny_policy_admission_reinit_and_frame_progress() {
        use std::sync::{Arc, Barrier};
        assert_eq!(crate::async_limits::policy().jobs_global, 2);
        assert_eq!(crate::async_limits::policy().frame_items, 1);
        init(dummy_logger()).unwrap();
        load_body("pressure", "", "{}");
        let entered = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(3));
        for _ in 0..2 {
            let entered = entered.clone();
            let release = release.clone();
            let ctx = PLUGINS.with(|p| p.borrow().get("pressure").unwrap().context.clone());
            HOST.with(|h| {
                let mut host = h.borrow_mut();
                let host = host.as_mut().unwrap();
                let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
                let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
                let ctx = v8::Local::new(&mut hs, &ctx);
                let scope = &mut v8::ContextScope::new(&mut hs, ctx);
                let lease = crate::jobs::reserve(scope, 128).unwrap();
                let cancel = lease.cancel.clone();
                let id = crate::jobs::next_id();
                let resolver = v8::PromiseResolver::new(scope).unwrap();
                pool()
                    .try_submit(
                        id,
                        Box::new(move || {
                            entered.wait();
                            release.wait();
                            Ok(())
                        }),
                        lease,
                    )
                    .unwrap();
                crate::jobs::commit_reserved(scope, id, resolver, cancel);
            });
        }
        entered.wait();
        assert_eq!(crate::async_limits::domain().jobs.snapshot().items, 2);
        let resources = active_resources("pressure");
        eval_in_context(
            "pressure",
            r#"globalThis.__rejected='pending';__s2_thread_sleep(0).catch(e=>__rejected=e.name);"#,
        )
        .unwrap();
        assert_eq!(active_resources("pressure"), resources);
        assert_eq!(crate::jobs::pending(), 2);
        frame_async_drain();
        dispatch_async_callbacks();
        assert_eq!(
            read_global_string("pressure", "__rejected"),
            "AsyncQueueFull"
        );
        unload_plugin("pressure");
        assert_eq!(crate::jobs::pending(), 0);
        assert!(
            async_pending() > 0,
            "producer keeps detour obligation after resolver unload"
        );
        shutdown();
        init(dummy_logger()).unwrap();
        load_body("replacement", "", "{}");
        eval_in_context(
            "replacement",
            r#"globalThis.__rejected='pending';__s2_thread_sleep(0).catch(e=>__rejected=e.name);"#,
        )
        .unwrap();
        frame_async_drain();
        dispatch_async_callbacks();
        assert_eq!(
            read_global_string("replacement", "__rejected"),
            "AsyncQueueFull"
        );
        assert_eq!(
            crate::async_limits::domain().jobs.snapshot().bytes,
            256,
            "old copied inputs are still charged after reinit"
        );
        release.wait();
        let end = Instant::now() + Duration::from_secs(2);
        while crate::async_limits::domain().jobs.snapshot().items != 0 {
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
        assert_eq!(
            crate::jobs::pending(),
            0,
            "late cancelled completions never settle replacement context"
        );
        eval_in_context(
            "replacement",
            r#"
            globalThis.__progress=0;__s2_thread_sleep(0).then(()=>__progress++);
            __s2_delay(100000).catch(()=>{});__s2_delay(100000).catch(()=>{});
            globalThis.__timerError='pending';__s2_delay(0).catch(e=>__timerError=e.name);
        "#,
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while crate::async_limits::queued_metrics()["worker"] == 0 {
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
        for _ in 0..12 {
            frame_async_drain();
            dispatch_async_callbacks();
            assert!(
                crate::async_limits::metrics()["frame"]["items"]
                    .as_u64()
                    .unwrap()
                    <= 1
            );
        }
        assert_eq!(
            read_i32_global_in("replacement", "__progress"),
            1,
            "far-future timers cannot prevent completion progress"
        );
        assert_eq!(
            read_global_string("replacement", "__timerError"),
            "AsyncQueueFull"
        );
        unload_plugin("replacement");
        assert_eq!(crate::async_limits::domain().timers.snapshot().items, 0);
        load_body("cold", "", "{}");
        let (id, _) = seed_injected_socket_connect("cold", false);
        eval_in_context("cold",r#"
            globalThis.__trace=[];__injected_connect.then(id=>{
                __trace.push('connect');__s2_net_on(id,'data',()=>__trace.push('data'));
                __s2_net_on(id,'error',()=>__trace.push('error'));__s2_net_on(id,'close',()=>__trace.push('close'));
            });
        "#).unwrap();
        crate::net::test_inject_batch(id, b"cold", "terminal");
        for _ in 0..24 {
            crate::cookies::test_queue_notification();
            frame_async_drain();
            dispatch_async_callbacks();
            let stats = crate::async_limits::metrics();
            assert!(stats["frame"]["items"].as_u64().unwrap() <= 1);
            assert!(stats["frame"]["polls"].as_u64().unwrap() <= 8);
        }
        assert_eq!(
            eval_in_context_string("cold", "__trace.join(',')"),
            "connect,data,error,close",
            "hot cookies cannot starve a cold socket callback"
        );
        assert!(!crate::net::is_owner(id, "cold"));
        shutdown();
    }


    #[test]
    fn async_request_header_capacity_is_covered_by_input_reservation() {
        let _ = init(dummy_logger());
        create_plugin_context("headers-review");
        eval_in_context(
            "headers-review",
            r#"
            var headers = Object.fromEntries(Array.from({length:513},(_,i)=>['h'+i,'']));
            __s2_fetch('invalid', {headers}).catch(()=>{});
        "#,
        )
        .unwrap();
        let http = crate::http::TEST_REQUEST_HEADER_CAPACITY
            .lock()
            .unwrap()
            .unwrap();
        shutdown();
        // New process-stable jobs may finish asynchronously; wait only for the invalid-URL worker.
        let deadline = Instant::now() + Duration::from_secs(2);
        while crate::async_limits::domain().jobs.snapshot().items != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let _ = init(dummy_logger());
        create_plugin_context("headers-review");
        eval_in_context(
            "headers-review",
            r#"
            var headers = Object.fromEntries(Array.from({length:513},(_,i)=>['h'+i,'']));
            __s2_ws_connect('invalid', {headers}).catch(()=>{});
        "#,
        )
        .unwrap();
        let ws = super::TEST_WS_REQUEST_HEADER_CAPACITY
            .lock()
            .unwrap()
            .unwrap();
        shutdown();
        eprintln!("actual native capacity / input charge: HTTP={http:?}, WS={ws:?}");
        assert!(http.0 <= http.1 && ws.0 <= ws.1,
            "actual native header Vec/String capacity vs total input charge: HTTP={http:?}, WS={ws:?}");
        assert_eq!(
            (http.1, ws.1),
            (34813, 34813),
            "tuple slots use the existing string allowance"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while crate::async_limits::domain().jobs.snapshot().items != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    #[ignore = "requires the isolated 40000-byte S2SCRIPT_ASYNC_LIMITS_JSON policy"]
    fn async_request_headers_tiny_policy_capacity_and_named_overload() {
        assert_eq!(crate::async_limits::policy().input_item_bytes, 40000);
        async_request_header_capacity_is_covered_by_input_reservation();
        init(dummy_logger()).unwrap();
        create_plugin_context("headers-overload");
        *crate::http::TEST_REQUEST_HEADER_CAPACITY.lock().unwrap() = None;
        *super::TEST_WS_REQUEST_HEADER_CAPACITY.lock().unwrap() = None;
        eval_in_context(
            "headers-overload",
            r#"
            var headers = Object.fromEntries(Array.from({length:601},(_,i)=>['h'+i,'']));
            globalThis.errors = [];
            __s2_fetch('invalid', {headers}).catch(e=>errors.push(e.name));
            __s2_ws_connect('invalid', {headers}).catch(e=>errors.push(e.name));
        "#,
        )
        .unwrap();
        frame_async_drain();
        dispatch_async_callbacks();
        assert_eq!(
            eval_in_context_string("headers-overload", "JSON.stringify(errors)"),
            r#"["AsyncPayloadTooLarge","AsyncPayloadTooLarge"]"#
        );
        assert!(crate::http::TEST_REQUEST_HEADER_CAPACITY
            .lock()
            .unwrap()
            .is_none());
        assert!(super::TEST_WS_REQUEST_HEADER_CAPACITY
            .lock()
            .unwrap()
            .is_none());
        assert_eq!(crate::jobs::pending(), 0);
        assert_eq!(crate::async_limits::domain().jobs.snapshot().items, 0);
        assert_eq!(crate::async_limits::domain().jobs.snapshot().bytes, 0);
        shutdown();
    }

