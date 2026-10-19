use super::*;
use std::thread::sleep;

const MS: u64 = 1_000_000;

fn fresh(options: Options) {
    shutdown();
    begin(options);
}

fn finished() -> Finished {
    end().expect("a session was running")
}

fn row<'a>(s: &'a Summary, owner: &str, name: &str) -> &'a report::Row {
    s.rows
        .iter()
        .find(|r| r.owner == owner && r.name == name)
        .unwrap_or_else(|| panic!("no row {owner} :: {name} in {:?}", s.rows))
}

#[test]
fn spans_are_inert_while_off() {
    shutdown();
    assert!(!active());
    let s = span("p", "x");
    assert_eq!(s.session, 0);
    drop(s);
    assert!(end().is_none());
}

#[test]
fn self_time_excludes_children_and_charges_each_owner() {
    fresh(Options::default());
    {
        let _outer = framework("dispatch_game_event");
        sleep(Duration::from_millis(2));
        {
            let _p = span("@a/plugin", "event player_death");
            sleep(Duration::from_millis(4));
            {
                // A GC landing inside the handler is charged to V8, not to the plugin.
                gc_begin("GC: scavenge");
                sleep(Duration::from_millis(3));
                gc_end();
            }
        }
    }
    let f = finished();
    let s = &f.summary;
    let outer = row(s, FRAMEWORK, "dispatch_game_event");
    let plugin = row(s, "@a/plugin", "event player_death");
    let gc = row(s, GC, "GC: scavenge");
    assert_eq!((outer.stat.calls, plugin.stat.calls, gc.stat.calls), (1, 1, 1));
    assert!(outer.stat.total >= 9 * MS, "{:?}", outer.stat);
    assert!(outer.stat.self_ns >= 2 * MS && outer.stat.self_ns < 4 * MS, "{:?}", outer.stat);
    assert!(plugin.stat.self_ns >= 4 * MS && plugin.stat.self_ns < 6 * MS, "{:?}", plugin.stat);
    assert!(gc.stat.self_ns >= 3 * MS, "{:?}", gc.stat);
    // Only the depth-0 span counts toward s2script's share of the wall clock.
    assert_eq!(s.s2s_total, outer.stat.total);
    assert_eq!(f.trace.timeline.len(), 3);
    assert_eq!(s.owners[0].owner, "@a/plugin");
}

#[test]
fn a_span_open_across_stop_is_inert_in_the_next_session() {
    fresh(Options::default());
    let leftover = span("p", "the sm prof stop command");
    let _ = finished();
    begin(Options::default());
    {
        let _a = span("p", "next session");
    }
    drop(leftover); // must not close (or corrupt) the new session's stack
    let f = finished();
    assert_eq!(f.summary.rows.len(), 1);
    assert_eq!(f.summary.rows[0].name, "next session");
}

#[test]
fn hook_closers_unwind_spans_left_open_above_them() {
    fresh(Options::default());
    {
        let _outer = framework("microtask checkpoint");
        async_begin("@a/plugin");
        // A span the continuation opened and never closed (a guard leaked by a throw path).
        std::mem::forget(span("@a/plugin", "leaked"));
        async_end();
        // The guard for `outer` still closes cleanly afterwards.
    }
    let f = finished();
    assert_eq!(f.summary.rows.len(), 3);
    assert!(f.summary.rows.iter().all(|r| r.stat.calls == 1));
    // gc_end without a matching prologue (session began mid-GC) is ignored.
    begin(Options::default());
    gc_end();
    assert!(finished().summary.rows.is_empty());
}

#[test]
fn frames_split_at_pre_and_the_verdict_rules_s2script_out() {
    fresh(Options { long_frame_ms: 20, ..Options::default() });
    // Lead-in before the first PRE is not a frame.
    sleep(Duration::from_millis(1));
    for i in 0..4 {
        frame_pre();
        {
            let _p = span("@a/plugin", "OnGameFrame (pre)");
            sleep(Duration::from_millis(1));
        }
        frame_post();
        // The engine's own time between our hooks: frame 3 is a 30 ms hitch with ~1 ms of us.
        sleep(Duration::from_millis(if i == 2 { 30 } else { 2 }));
    }
    frame_pre();
    let f = finished();
    let s = &f.summary;
    assert_eq!(s.frames, 4);
    assert_eq!(s.long_frames, 1);
    let worst = &s.worst_interval[0];
    assert_eq!(worst.seq, 3);
    assert!(worst.interval >= 30 * MS);
    assert!(worst.s2s >= MS && worst.s2s < 10 * MS, "{worst:?}");
    assert!(worst.game_frame.is_some());
    assert_eq!(worst.top[0].0, "@a/plugin");
    let lines = report::console_lines(s, 10);
    let verdict = lines.iter().find(|l| l.contains("Verdict")).expect("verdict line");
    assert!(verdict.contains("NOT s2script"), "{verdict}");
    assert_eq!(f.trace.frames.len(), 4);
}

#[test]
fn the_verdict_names_the_plugin_when_it_is_the_hitch() {
    fresh(Options::default());
    frame_pre();
    {
        let _p = span("@a/slow", "timer: tick (line 3)");
        sleep(Duration::from_millis(20));
    }
    frame_pre();
    let s = finished().summary;
    let lines = report::console_lines(&s, 10);
    let verdict = lines.iter().find(|l| l.contains("Verdict")).expect("verdict line");
    assert!(verdict.contains("@a/slow"), "{verdict}");
    assert!(lines.iter().any(|l| l.contains("@a/slow :: timer: tick (line 3)")), "{lines:#?}");
}

#[test]
fn a_full_timeline_still_keeps_the_worst_frames_in_full() {
    fresh(Options { timeline_cap: 4, ..Options::default() });
    for i in 0..6 {
        frame_pre();
        for _ in 0..3 {
            let _p = span("@a/plugin", "event");
        }
        if i == 4 {
            let _p = span("@a/plugin", "the slow one");
            sleep(Duration::from_millis(5));
        }
    }
    frame_pre();
    let f = finished();
    assert!(f.summary.dropped > 0);
    assert!(f.trace.timeline.len() <= 4 + 10 * 4, "{}", f.trace.timeline.len());
    let slow = f.trace.names.iter().position(|n| n == "the slow one").unwrap() as u32;
    assert!(f.trace.timeline.iter().any(|e| e.name == slow), "the worst frame's spans are kept");
    // ...but only once, even though it tops both worst lists.
    assert_eq!(f.trace.timeline.iter().filter(|e| e.name == slow).count(), 1);
    assert_eq!(row(&f.summary, "@a/plugin", "event").stat.calls, 18, "totals keep counting");
}

#[test]
fn a_session_without_frames_stays_bounded() {
    fresh(Options { timeline_cap: 10, ..Options::default() });
    for _ in 0..100 {
        let _p = span("@a/plugin", "event");
    }
    let f = finished();
    assert_eq!(f.trace.timeline.len(), 10);
    assert_eq!(f.summary.dropped, 90);
    assert_eq!(f.summary.frames, 0);
    assert_eq!(row(&f.summary, "@a/plugin", "event").stat.calls, 100);
    assert!(report::console_lines(&f.summary, 5).iter().any(|l| l.contains("No frames ran")));
}

#[test]
fn the_trace_is_valid_chrome_trace_json() {
    fresh(Options { long_frame_ms: 1, ..Options::default() });
    frame_pre();
    {
        let _a = framework("GameFrame pre");
        let _b = span("@a/\"quoted\"", "dispatch_game_event('x')");
        sleep(Duration::from_millis(2));
    }
    frame_pre();
    let f = finished();
    let mut buf = Vec::new();
    report::write_trace(&mut buf, &f.trace).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&buf).expect("valid JSON");
    let events = v["traceEvents"].as_array().unwrap();
    let complete: Vec<_> = events.iter().filter(|e| e["ph"] == "X").collect();
    assert_eq!(complete.len(), 2);
    let plugin = complete.iter().find(|e| e["cat"] == "@a/\"quoted\"").unwrap();
    assert_eq!(plugin["name"], "@a/\"quoted\" \u{b7} dispatch_game_event('x')");
    assert!(plugin["dur"].as_f64().unwrap() >= 2000.0);
    assert!(events.iter().any(|e| e["ph"] == "C" && e["name"] == "frame interval (ms)"));
    assert!(events.iter().any(|e| e["ph"] == "i"), "a long-frame marker");
}

#[test]
fn civil_dates_and_stamps() {
    assert_eq!(report::civil_for_test(0), (1970, 1, 1));
    assert_eq!(report::civil_for_test(20_745), (2026, 10, 19));
    assert_eq!(report::civil_for_test(11_016), (2000, 2, 29));
    let t = SystemTime::UNIX_EPOCH + Duration::from_millis(1_792_411_994_567);
    assert_eq!(report::stamp(t), "20261019-121314");
    assert_eq!(report::clock(t), "12:13:14.567Z");
}

#[test]
fn hot_functions_spread_samples_over_the_profile() {
    let profile = serde_json::json!({
        "startTime": 0, "endTime": 4000,
        "samples": [1, 2, 2, 2],
        "nodes": [
            { "id": 1, "hitCount": 1, "callFrame": { "functionName": "", "url": "", "lineNumber": -1 } },
            { "id": 2, "hitCount": 3, "callFrame": { "functionName": "spin", "url": "s2s://@a/b/plugin.js", "lineNumber": 9 } },
        ]
    });
    let hot = js::hot_functions(&profile, 10);
    assert_eq!(hot[0], (3 * MS, "spin".to_string(), "s2s://@a/b/plugin.js:10".to_string()));
    assert_eq!(hot[1], (MS, "(anonymous)".to_string(), String::new()));
    let response = serde_json::json!({ "id": 3, "result": { "profile": profile } }).to_string();
    assert_eq!(js::extract_profile(&response).unwrap()["endTime"], 4000);
}

#[test]
fn control_without_an_engine() {
    shutdown();
    assert_eq!(request(Request::Stop), vec!["[S2S] Profiler is not running. Use \"sm prof start\".".to_string()]);
    assert!(dump(10)[0].contains("No finished profile"));
    assert!(status()[0].contains("not running"));
}
