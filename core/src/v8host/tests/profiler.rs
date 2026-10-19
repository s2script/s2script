//! `sm prof` against a real isolate: plugin attribution through the real dispatch paths, async
//! continuations, GC, deferral while V8 is busy, and the inspector-driven JS sampling profile.

use super::*;
use crate::profiler::{self, Options, Request};

fn frame() {
    crate::ffi::s2script_core_dispatch_game_frame(0, 1, 0, 0);
    crate::ffi::s2script_core_dispatch_game_frame(1, 1, 0, 0);
}

fn stop() -> profiler::Summary {
    let lines = profiler::stop_and_report();
    assert!(lines.iter().any(|l| l.contains("s2script profile")), "{lines:#?}");
    profiler::last_summary().expect("a finished summary")
}

fn owner_self(s: &profiler::Summary, owner: &str) -> u64 {
    s.owners.iter().find(|o| o.owner == owner).map_or(0, |o| o.self_ns)
}

#[test]
fn a_slow_frame_handler_is_charged_to_its_plugin() {
    init(dummy_logger()).unwrap();
    load_body("@t/slow", r#"
        ctx.server.onGameFrame(function () { const t = Date.now(); while (Date.now() - t < 3) {} });
    "#, "{}");
    load_body("@t/idle", r#" ctx.server.onGameFrame(function () {}); "#, "{}");
    let lines = profiler::request(Request::Start(Options::default()));
    assert!(lines[0].contains("Profiler started"), "{lines:?}");
    assert!(profiler::active());
    for _ in 0..5 {
        frame();
    }
    let s = stop();
    assert!(!profiler::active());
    assert_eq!(s.frames, 4, "five PREs close four frames");
    let slow = owner_self(&s, "@t/slow");
    // `Date.now() - t < 3` spins for 2-3ms (the clock ticks in whole milliseconds).
    assert!(slow >= 10_000_000, "5 frames x 2-3ms, got {slow}");
    assert!(owner_self(&s, "@t/idle") < slow / 10);
    assert_eq!(s.rows[0].owner, "@t/slow");
    assert_eq!(s.rows[0].name, "OnGameFrame (pre)");
    assert_eq!(s.rows[0].stat.calls, 5);
    // The framework's own frame spans nest the plugin's, and own only their self time.
    assert!(s.rows.iter().any(|r| r.owner == profiler::FRAMEWORK && r.name == "GameFrame pre"));
    assert!(s.worst_s2s[0].top[0].0 == "@t/slow");
    shutdown();
}

#[test]
fn async_continuations_are_charged_to_the_plugin_that_awaited() {
    init(dummy_logger()).unwrap();
    load_body("@t/async", r#"
        ctx.server.onGameFrame(function () {
            Promise.resolve().then(() => { const t = Date.now(); while (Date.now() - t < 2) {} });
        });
    "#, "{}");
    let _ = profiler::request(Request::Start(Options::default()));
    for _ in 0..3 {
        frame();
    }
    let s = stop();
    let row = s
        .rows
        .iter()
        .find(|r| r.owner == "@t/async" && r.name.starts_with("async continuation"))
        .unwrap_or_else(|| panic!("{:#?}", s.rows));
    assert_eq!(row.stat.calls, 3);
    assert!(row.stat.self_ns >= 3_000_000, "3 continuations x 1-2ms: {:?}", row.stat);
    // The checkpoint itself keeps only its own overhead.
    let checkpoint = s.rows.iter().find(|r| r.name == "microtask checkpoint").unwrap();
    assert!(checkpoint.stat.self_ns < row.stat.self_ns, "{:?}", checkpoint.stat);
    shutdown();
}

#[test]
fn garbage_collection_is_its_own_owner() {
    init(dummy_logger()).unwrap();
    let _ = profiler::request(Request::Start(Options::default()));
    with_host_isolate(|isolate| isolate.low_memory_notification()).unwrap();
    let s = stop();
    assert!(
        s.rows.iter().any(|r| r.owner == profiler::GC && r.name.starts_with("GC: mark-sweep-compact")),
        "{:#?}",
        s.rows
    );
    // The hooks are gone with the session: a later GC records nothing and does not panic.
    with_host_isolate(|isolate| isolate.low_memory_notification()).unwrap();
    shutdown();
}

#[test]
fn start_and_stop_wait_for_the_next_frame_while_v8_is_busy() {
    init(dummy_logger()).unwrap();
    let lines = with_host_borrowed(|| profiler::request(Request::Start(Options::default())));
    assert_eq!(lines, vec!["[S2S] Profiler will start on the next frame.".to_string()]);
    assert!(!profiler::active() && profiler::wants_frames());
    frame();
    assert!(profiler::active());
    let lines = with_host_borrowed(|| profiler::request(Request::Stop));
    assert_eq!(lines, vec!["[S2S] Profiler will stop on the next frame.".to_string()]);
    frame();
    assert!(!profiler::active() && !profiler::wants_frames());
    assert!(profiler::last_summary().is_some());
    shutdown();
}

#[test]
fn a_timed_session_stops_itself() {
    init(dummy_logger()).unwrap();
    let options = Options { duration: Some(std::time::Duration::from_millis(5)), ..Options::default() };
    let _ = profiler::request(Request::Start(options));
    frame();
    std::thread::sleep(std::time::Duration::from_millis(6));
    frame();
    assert!(!profiler::active());
    shutdown();
}

#[test]
fn js_sampling_names_plugin_functions_by_their_bundle() {
    init(dummy_logger()).unwrap();
    load_body("@t/js", r#"
        function spinForAWhile() { const t = Date.now(); while (Date.now() - t < 20) {} }
        ctx.server.onGameFrame(function () { spinForAWhile(); });
    "#, "{}");
    profiler::js::start().expect("inspector sampling starts");
    for _ in 0..5 {
        frame();
    }
    let response = profiler::js::stop().expect("Profiler.stop answers");
    let profile = profiler::js::extract_profile(&response).expect("a profile object");
    let hot = profiler::js::hot_functions(&profile, 50);
    let url = crate::v8host::lifecycle::plugin_script_url("@t/js");
    assert!(
        hot.iter().any(|(_, f, place)| f == "spinForAWhile" && place.starts_with(&url)),
        "{hot:#?}"
    );
    assert!(profiler::js::stop().is_err(), "the inspector is gone after stop");
    shutdown();
}

#[test]
fn shutdown_mid_session_drops_the_inspector_cleanly() {
    init(dummy_logger()).unwrap();
    let lines = profiler::request(Request::Start(Options { js: true, ..Options::default() }));
    assert!(lines[0].contains("with JS sampling"), "{lines:?}");
    frame();
    shutdown();
    assert!(!profiler::active());
    init(dummy_logger()).unwrap();
    frame();
    shutdown();
}
