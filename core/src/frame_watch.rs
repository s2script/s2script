//! The slow-frame watchdog: an always-on account of how much of each server frame s2script used.
//!
//! `sm prof` answers "what was slow" in detail, but only for frames recorded while it ran, and it
//! measures the Rust fan-out instead of the JS multiplexer. This is the cheap counterpart that never
//! stops: every entry from the engine into the core is timed (two clock reads), garbage collections
//! are timed through V8's callbacks, and a frame whose total reaches the threshold prints one line
//! naming where the time went. It tells an operator whether a slow server frame was ours.
//!
//! A frame, here, is everything from the end of one `GameFrame` POST to the end of the next: the
//! engine also calls in between frames (network events, commands), and that time belongs somewhere.
//!
//! Engine-generic: the parts are the core's own entry points, never a game concept.
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// Where an entry from the engine spends its time. One per kind of entry point, not per plugin:
/// `sm prof` is the tool for which plugin.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum Part {
    FramePre = 0,
    FramePost,
    AsyncDrain,
    Callbacks,
    Loader,
    Event,
    Usercmd,
    Hook,
    Entity,
    Command,
    Client,
    Other,
}
const PARTS: usize = 12;
const LABELS: [&str; PARTS] = [
    "OnGameFrame pre",
    "OnGameFrame post",
    "timers and async results",
    "socket and cookie callbacks",
    "plugin loader",
    "game events",
    "usercmd hooks",
    "entity and engine hooks",
    "entity lifecycle",
    "commands and chat",
    "client events",
    "other dispatch",
];

/// The default threshold: a frame is reported when s2script used at least this much of it.
pub(crate) const DEFAULT_THRESHOLD_MS: f64 = 4.0;
/// At most one line per this interval; frames in between are counted and summarised.
const LOG_INTERVAL: Duration = Duration::from_secs(1);
/// A part smaller than this is left out of the line.
const SHOW_NS: u64 = 100_000;

#[derive(Default)]
struct Totals {
    frames: u64,
    slow: u64,
    worst_ns: u64,
    scavenges: u64,
    fulls: u64,
    scavenge_worst_ns: u64,
    full_worst_ns: u64,
    gc_total_ns: u64,
}

struct State {
    parts: [u64; PARTS],
    scavenge_ns: u64,
    scavenge_n: u32,
    full_ns: u64,
    full_n: u32,
    last_log: Option<Instant>,
    suppressed: u32,
    totals: Totals,
}

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// Nanoseconds; 0 turns reporting off (the accounting stays on, for `sm frames`).
    static THRESHOLD_NS: Cell<u64> = const { Cell::new((DEFAULT_THRESHOLD_MS * 1e6) as u64) };
    static GC_START: Cell<Option<Instant>> = const { Cell::new(None) };
    static STATE: RefCell<State> = const { RefCell::new(State {
        parts: [0; PARTS], scavenge_ns: 0, scavenge_n: 0, full_ns: 0, full_n: 0,
        last_log: None, suppressed: 0,
        totals: Totals { frames: 0, slow: 0, worst_ns: 0, scavenges: 0, fulls: 0,
            scavenge_worst_ns: 0, full_worst_ns: 0, gc_total_ns: 0 },
    }) };
}

/// Times one entry from the engine. Only the outermost entry on the stack counts, so a dispatch a
/// handler triggers (an event fired from a command) is charged once, to the entry that started it.
#[must_use]
pub(crate) struct Guard(Option<(Part, Instant)>);

pub(crate) fn enter(part: Part) -> Guard {
    let depth = DEPTH.with(|d| {
        let n = d.get();
        d.set(n + 1);
        n
    });
    Guard((depth == 0).then(|| (part, Instant::now())))
}

impl Drop for Guard {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        if let Some((part, start)) = self.0.take() {
            let ns = start.elapsed().as_nanos() as u64;
            STATE.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut() {
                    s.parts[part as usize] += ns;
                }
            });
        }
    }
}

/// A section inside an entry that is already being timed (the phases of the frame POST): closes the
/// time so far under `from` and continues under `to`.
pub(crate) fn switch(guard: &mut Guard, to: Part) {
    if let Some((from, start)) = guard.0.as_mut() {
        let now = Instant::now();
        let ns = now.duration_since(*start).as_nanos() as u64;
        let part = *from;
        STATE.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.parts[part as usize] += ns;
            }
        });
        *from = to;
        *start = now;
    }
}

/// V8 GC prologue. Signal-free and allocation-free: one clock read.
pub(crate) fn gc_begin() {
    GC_START.with(|g| g.set(Some(Instant::now())));
}

/// V8 GC epilogue. `full` is a mark-compact; anything else counts as a young-generation collection.
/// Incremental-marking and weak-callback notifications are not pauses of their own and are skipped
/// by the caller.
pub(crate) fn gc_end(full: bool) {
    let Some(start) = GC_START.with(Cell::take) else { return };
    let ns = start.elapsed().as_nanos() as u64;
    STATE.with(|s| {
        let Ok(mut s) = s.try_borrow_mut() else { return };
        s.totals.gc_total_ns += ns;
        if full {
            s.full_ns += ns;
            s.full_n += 1;
            s.totals.fulls += 1;
            s.totals.full_worst_ns = s.totals.full_worst_ns.max(ns);
        } else {
            s.scavenge_ns += ns;
            s.scavenge_n += 1;
            s.totals.scavenges += 1;
            s.totals.scavenge_worst_ns = s.totals.scavenge_worst_ns.max(ns);
        }
    });
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

/// Close the frame: report it if it was slow, then start the next. Returns the line it decided to
/// print (the caller logs it, outside this module's borrow).
pub(crate) fn end_frame() -> Option<String> {
    let threshold = THRESHOLD_NS.with(Cell::get);
    STATE.with(|s| {
        let Ok(mut s) = s.try_borrow_mut() else { return None };
        let total: u64 = s.parts.iter().sum();
        s.totals.frames += 1;
        s.totals.worst_ns = s.totals.worst_ns.max(total);
        let mut line = None;
        if threshold > 0 && total >= threshold {
            s.totals.slow += 1;
            let now = Instant::now();
            if s.last_log.is_none_or(|t| now.duration_since(t) >= LOG_INTERVAL) {
                line = Some(describe(&s, total));
                s.last_log = Some(now);
                s.suppressed = 0;
            } else {
                s.suppressed += 1;
            }
        }
        s.parts = [0; PARTS];
        s.scavenge_ns = 0;
        s.scavenge_n = 0;
        s.full_ns = 0;
        s.full_n = 0;
        line
    })
}

fn describe(s: &State, total: u64) -> String {
    let mut out = format!("[s2script] slow frame: s2script used {:.1} ms", ms(total));
    let mut detail: Vec<String> = Vec::new();
    let gc = s.scavenge_ns + s.full_ns;
    if gc >= SHOW_NS {
        let mut kinds = Vec::new();
        if s.full_n > 0 {
            kinds.push(format!("{} full", s.full_n));
        }
        if s.scavenge_n > 0 {
            kinds.push(format!("{} young", s.scavenge_n));
        }
        detail.push(format!("of which garbage collection {:.1} ms, {}", ms(gc), kinds.join(" + ")));
    }
    let mut parts: Vec<(usize, u64)> = s.parts.iter().copied().enumerate().filter(|(_, ns)| *ns >= SHOW_NS).collect();
    parts.sort_by(|a, b| b.1.cmp(&a.1));
    let shown: Vec<String> = parts.iter().map(|(i, ns)| format!("{} {:.1} ms", LABELS[*i], ms(*ns))).collect();
    if !shown.is_empty() {
        detail.push(shown.join(", "));
    }
    if !detail.is_empty() {
        out.push_str(&format!(" ({})", detail.join("; ")));
    }
    if s.suppressed > 0 {
        out.push_str(&format!(" [+{} slow frame(s) since the last line]", s.suppressed));
    }
    out
}

/// `sm frames` with no argument: the running totals since the last reset.
pub(crate) fn status() -> Vec<String> {
    let threshold = THRESHOLD_NS.with(Cell::get);
    STATE.with(|s| {
        let s = s.borrow();
        let t = &s.totals;
        vec![
            if threshold == 0 {
                "[S2S] Slow-frame reporting is off (sm frames <ms> turns it on).".to_string()
            } else {
                format!("[S2S] Reporting frames where s2script uses {:.1} ms or more.", ms(threshold))
            },
            format!("  frames seen {}, over the threshold {}, most s2script time in one frame {:.2} ms", t.frames, t.slow, ms(t.worst_ns)),
            format!(
                "  garbage collection: {} young (longest {:.2} ms), {} full (longest {:.2} ms), {:.0} ms in total",
                t.scavenges, ms(t.scavenge_worst_ns), t.fulls, ms(t.full_worst_ns), ms(t.gc_total_ns)
            ),
        ]
    })
}

/// `sm frames <ms|off|reset>`.
pub(crate) fn command(arg: &str) -> Vec<String> {
    match arg.to_ascii_lowercase().as_str() {
        "off" | "0" => {
            THRESHOLD_NS.with(|t| t.set(0));
            status()
        }
        "reset" => {
            STATE.with(|s| s.borrow_mut().totals = Totals::default());
            status()
        }
        n => match n.parse::<f64>() {
            Ok(v) if v > 0.0 && v <= 1000.0 => {
                THRESHOLD_NS.with(|t| t.set((v * 1e6) as u64));
                status()
            }
            _ => vec![USAGE.to_string()],
        },
    }
}

pub(crate) const USAGE: &str = "Usage: sm frames [<ms> | off | reset]";

#[cfg(test)]
pub(crate) fn test_reset() {
    DEPTH.with(|d| d.set(0));
    THRESHOLD_NS.with(|t| t.set((DEFAULT_THRESHOLD_MS * 1e6) as u64));
    GC_START.with(|g| g.set(None));
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.parts = [0; PARTS];
        s.scavenge_ns = 0;
        s.scavenge_n = 0;
        s.full_ns = 0;
        s.full_n = 0;
        s.last_log = None;
        s.suppressed = 0;
        s.totals = Totals::default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spend(part: Part, d: Duration) {
        let _g = enter(part);
        std::thread::sleep(d);
    }

    #[test]
    fn a_quick_frame_prints_nothing_and_a_slow_one_names_its_parts() {
        test_reset();
        spend(Part::Event, Duration::from_micros(50));
        assert_eq!(end_frame(), None);

        spend(Part::FramePost, Duration::from_millis(5));
        spend(Part::Usercmd, Duration::from_millis(1));
        let line = end_frame().expect("a 4 ms frame is over the 2 ms default");
        assert!(line.starts_with("[s2script] slow frame: s2script used "), "{line}");
        let post = line.find("OnGameFrame post").expect("names the frame post");
        let cmd = line.find("usercmd hooks").expect("names the usercmd hooks");
        assert!(post < cmd, "largest part first: {line}");
        assert!(!line.contains("game events"), "a part under 0.1 ms is left out: {line}");
    }

    #[test]
    fn only_the_outermost_entry_is_charged() {
        test_reset();
        {
            let _outer = enter(Part::Command);
            let _inner = enter(Part::Event);
            std::thread::sleep(Duration::from_millis(5));
        }
        let line = end_frame().unwrap();
        assert!(line.contains("commands and chat"), "{line}");
        assert!(!line.contains("game events"), "{line}");
    }

    #[test]
    fn switch_splits_one_entry_into_sections() {
        test_reset();
        {
            let mut g = enter(Part::FramePost);
            std::thread::sleep(Duration::from_millis(3));
            switch(&mut g, Part::Loader);
            std::thread::sleep(Duration::from_millis(3));
        }
        let line = end_frame().unwrap();
        assert!(line.contains("OnGameFrame post") && line.contains("plugin loader"), "{line}");
    }

    #[test]
    fn a_collection_is_named_with_its_kind() {
        test_reset();
        {
            let _g = enter(Part::FramePost);
            gc_begin();
            std::thread::sleep(Duration::from_millis(5));
            gc_end(true);
        }
        let line = end_frame().unwrap();
        assert!(line.contains("of which garbage collection") && line.contains("1 full"), "{line}");
        let status = status().join("\n");
        assert!(status.contains("1 full"), "{status}");
    }

    #[test]
    fn slow_frames_inside_the_interval_are_counted_not_printed() {
        test_reset();
        spend(Part::FramePost, Duration::from_millis(5));
        assert!(end_frame().is_some());
        spend(Part::FramePost, Duration::from_millis(5));
        assert_eq!(end_frame(), None, "second line inside one second is held back");
        STATE.with(|s| s.borrow_mut().last_log = Some(Instant::now() - Duration::from_secs(2)));
        spend(Part::FramePost, Duration::from_millis(5));
        let line = end_frame().unwrap();
        assert!(line.contains("[+1 slow frame(s) since the last line]"), "{line}");
    }

    /// End to end: a collection a frame handler forces is timed by the isolate's GC callbacks and
    /// charged to the frame that ran it.
    #[test]
    fn a_collection_inside_a_frame_handler_is_reported() {
        use crate::v8host::frame_tests::{dummy_logger, eval_std};
        let _ = crate::v8host::init(dummy_logger());
        test_reset();
        command("0.01");
        eval_std("gc", "OnGameFrame.subscribe(() => { __s2_v8_gc(); });");
        crate::ffi::s2script_core_dispatch_game_frame(0, 1, 0, 0);
        crate::ffi::s2script_core_dispatch_game_frame(1, 1, 0, 0);
        let status = status().join("\n");
        assert!(!status.contains(" 0 full"), "the forced collection was timed: {status}");
        assert!(status.contains("frames seen 1"), "{status}");
        crate::v8host::shutdown();
        test_reset();
    }

    #[test]
    fn the_threshold_is_set_and_turned_off_by_command() {
        test_reset();
        assert_eq!(command("nonsense"), vec![USAGE.to_string()]);
        assert!(command("10")[0].contains("10.0 ms"));
        spend(Part::FramePost, Duration::from_millis(5));
        assert_eq!(end_frame(), None, "5 ms is under a 10 ms threshold");
        assert!(command("off")[0].contains("off"));
        spend(Part::FramePost, Duration::from_millis(5));
        assert_eq!(end_frame(), None);
        assert!(status()[1].contains("frames seen 2"), "{:?}", status());
        assert!(command("reset")[1].contains("frames seen 0"));
        test_reset();
    }
}
