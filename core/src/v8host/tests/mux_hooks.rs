//! The per-tick hooks routed through the JS multiplexer (usercmd pre/post, entity lifecycle,
//! SDKHooks, SetTransmit): the multiplexer and the Rust fan-out it replaces must deliver the same
//! thing, in the same order, with the same collapse. Each test runs its body on both paths.
use super::*;

/// Run `body` on the multiplexer, then on the Rust fan-out, each in a fresh isolate; both results.
fn both<R>(body: impl Fn() -> R) -> (R, R) {
    crate::jsmux::FORCE_FALLBACK.with(|f| f.set(false));
    let js = body();
    crate::jsmux::FORCE_FALLBACK.with(|f| f.set(true));
    let rust = body();
    crate::jsmux::FORCE_FALLBACK.with(|f| f.set(false));
    (js, rust)
}

#[test]
fn usercmd_pre_delivers_own_realm_args_and_stops_at_stop_on_both_paths() {
    let (js, rust) = both(|| {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        for p in ["a", "b", "c"] { create_plugin_context(p); }
        // a: checks that cmd and ctx come from its own realm, returns Handled.
        eval_in_context("a", r#"
            globalThis.__seen = "none";
            __s2pkg_usercmd.UserCmd.onRun(function (cmd, ctx) {
                globalThis.__seen = (cmd === __s2pkg_usercmd.Cmd) + ":" +
                    (Object.getPrototypeOf(ctx) === Object.prototype) + ":" + ctx.slot;
                return 2;
            });
        "#).unwrap();
        eval_in_context("b", "__s2pkg_usercmd.UserCmd.onRun(function () { return 3; });").unwrap();
        eval_in_context("c", "globalThis.__ran = false; __s2pkg_usercmd.UserCmd.onRun(function () { globalThis.__ran = true; });").unwrap();
        let result = dispatch_usercmd(4);
        let out = (
            result,
            eval_in_context_string("a", "__seen"),
            eval_in_context_string("c", "String(__ran)"),
        );
        shutdown();
        out
    });
    assert_eq!(js, (3, "true:true:4".to_string(), "false".to_string()), "Stop truncates; args are the subscriber's own");
    assert_eq!(js, rust);
}

#[test]
fn usercmd_pre_ctx_is_fresh_per_command_on_both_paths() {
    let (js, rust) = both(|| {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        eval_in_context("p", r#"
            globalThis.__ctxs = [];
            __s2pkg_usercmd.UserCmd.onRun(function (cmd, ctx) { __ctxs.push(ctx); });
        "#).unwrap();
        let _ = dispatch_usercmd(1);
        let _ = dispatch_usercmd(2);
        let out = eval_in_context_string("p", "__ctxs.length + ':' + (__ctxs[0] !== __ctxs[1]) + ':' + __ctxs[0].slot + ',' + __ctxs[1].slot");
        shutdown();
        out
    });
    assert_eq!(js, "2:true:1,2");
    assert_eq!(js, rust);
}

#[test]
fn entity_events_reach_exact_then_wildcard_with_class_on_both_paths() {
    let (js, rust) = both(|| {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("e1");
        create_plugin_context("e2");
        eval_in_context("e1", r#"
            globalThis.__hits = [];
            __s2pkg_entity.Entity.onSpawn("*", function (e, cls) { __hits.push("star1:" + cls + ":" + (e === null)); });
            __s2pkg_entity.Entity.onSpawn("weapon_ak47", function (e, cls) { __hits.push("exact1:" + cls); });
        "#).unwrap();
        eval_in_context("e2", r#"
            globalThis.__hits = [];
            __s2pkg_entity.Entity.onSpawn("weapon_ak47", function (e, cls) { __hits.push("exact2:" + cls); });
            __s2pkg_entity.Entity.onSpawn("*", function (e, cls) { throw new Error("boom"); });
        "#).unwrap();
        let _ = crate::entity::dispatch_entity_event("spawn", "weapon_ak47", -1);
        let _ = crate::entity::dispatch_entity_event("spawn", "prop_physics", -1);
        let out = (eval_in_context_string("e1", "__hits.join('|')"), eval_in_context_string("e2", "__hits.join('|')"));
        shutdown();
        out
    });
    assert_eq!(js.0, "exact1:weapon_ak47|star1:weapon_ak47:true|star1:prop_physics:true");
    assert_eq!(js.1, "exact2:weapon_ak47", "a throwing wildcard handler does not stop the others");
    assert_eq!(js, rust);
}

#[test]
fn usercmd_post_runs_through_the_multiplexer_on_both_paths() {
    let (js, rust) = both(|| {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        eval_in_context("p", r#"
            globalThis.__log = [];
            __s2pkg_usercmd.UserCmd.onRun(function (c, i) { __log.push(i.slot + "@" + i.tick); return 3; }, { phase: "post" });
            __s2pkg_usercmd.UserCmd.onRun(function (c, i) { __log.push("second"); }, { phase: "post" });
        "#).unwrap();
        dispatch_usercmd_post(&UsercmdSnapshot { slot: 2, tick: 77, ..Default::default() });
        let out = eval_in_context_string("p", "__log.join('|')");
        shutdown();
        out
    });
    assert_eq!(js, "2@77|second", "post returns are ignored");
    assert_eq!(js, rust);
}

/// `cargo test --release -p s2script-core mux_hooks_bench -- --ignored --nocapture`: one usercmd
/// pre dispatch to 10 plugins, Rust fan-out vs the multiplexer.
#[test]
#[ignore]
fn mux_hooks_bench_usercmd_ten_plugins() {
    let _ = init(dummy_logger());
    set_engine_ops(None);
    for i in 0..10 {
        let p = format!("u{i}");
        create_plugin_context(&p);
        eval_in_context(&p, "var n = 0; __s2pkg_usercmd.UserCmd.onRun(function (cmd, ctx) { n += ctx.slot; });").unwrap();
    }
    let run = |n: u32| {
        let t = std::time::Instant::now();
        for i in 0..n { let _ = dispatch_usercmd((i % 64) as i32); }
        t.elapsed().as_nanos() as f64 / n as f64
    };
    crate::jsmux::FORCE_FALLBACK.with(|f| f.set(true));
    run(2_000);
    let rust = run(50_000);
    crate::jsmux::FORCE_FALLBACK.with(|f| f.set(false));
    run(2_000);
    let js = run(50_000);
    println!("usercmd pre, 10 plugins: Rust fan-out {:.0} ns, JS multiplexer {:.0} ns ({:.1}x)", rust, js, rust / js);
    shutdown();
}
