//! Garbage collection at points where a pause costs nothing, and V8's own background-friendly work
//! given a slot every frame.
//!
//! Left alone, V8 collects whenever an allocation forces it, which is in the middle of whatever
//! handler happens to allocate. A full collection with twenty plugins loaded pauses for several
//! milliseconds, enough to turn an ordinary frame into a slow one. Two things move that work:
//!
//! * **A full collection once a map has started and the plugins have settled.** The map is loading
//!   and players are connecting, so a pause there is not felt, and it clears what loading the
//!   plugins left behind before the round starts.
//! * **One V8 foreground task per frame.** V8 posts incremental-marking steps and other GC work to
//!   the platform's task queue for the embedder to run. Nothing ran that queue, so the work waited
//!   until an allocation forced it. Running one task at the end of each frame's POST lets V8 spread
//!   it out. One, not all: running the whole queue at once performs all the marking in one frame.
//!
//! Engine-generic: nothing here knows a game.
use std::cell::Cell;

/// Frames after map start to wait for the loader before collecting anyway.
const SETTLE_FRAMES: u32 = 600;

thread_local! {
    /// `Some(frames waited)` while a post-map-start collection is due.
    static DUE: Cell<Option<u32>> = const { Cell::new(None) };
    static PUMP: Cell<bool> = const { Cell::new(true) };
}

/// Map start: collect once the plugins have settled.
pub(crate) fn arm_map_start() {
    DUE.with(|d| d.set(Some(0)));
}

/// End of each frame's POST. `settled` is whether the loader has nothing left to do.
pub(crate) fn frame_post(settled: bool) {
    if let Some(waited) = DUE.with(Cell::get) {
        if settled || waited >= SETTLE_FRAMES {
            DUE.with(|d| d.set(None));
            collect("after map start");
        } else {
            DUE.with(|d| d.set(Some(waited + 1)));
        }
    }
    if PUMP.with(Cell::get) {
        let _ = crate::v8host::pump_one_v8_task();
    }
}

/// Run a full collection now and log what it took and freed.
fn collect(why: &str) {
    let t = std::time::Instant::now();
    let Ok((before, after)) = crate::v8host::with_host_isolate(|isolate| {
        let before = isolate.get_heap_statistics().used_heap_size();
        isolate.low_memory_notification();
        (before, isolate.get_heap_statistics().used_heap_size())
    }) else {
        return;
    };
    let mb = |b: usize| b as f64 / 1048576.0;
    crate::v8host::log_warn(&format!(
        "[s2script] garbage collected {why}: {:.1} ms, heap {:.1} MB -> {:.1} MB",
        t.elapsed().as_secs_f64() * 1000.0,
        mb(before),
        mb(after)
    ));
}

/// Tests: whether a post-map-start collection is still due.
#[cfg(test)]
pub(crate) fn due() -> bool {
    DUE.with(Cell::get).is_some()
}

#[cfg(test)]
pub(crate) fn test_reset() {
    DUE.with(|d| d.set(None));
    PUMP.with(|p| p.set(true));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, eval_std};

    #[test]
    fn map_start_collects_once_the_loader_settles() {
        let _ = crate::v8host::init(dummy_logger());
        test_reset();
        arm_map_start();
        frame_post(false);
        assert!(due(), "waits while the loader still has work");
        frame_post(true);
        assert!(!due(), "collected once settled");
        frame_post(true);
        assert!(!due());
        crate::v8host::shutdown();
    }

    #[test]
    fn map_start_collects_anyway_after_the_wait() {
        let _ = crate::v8host::init(dummy_logger());
        test_reset();
        arm_map_start();
        for _ in 0..SETTLE_FRAMES {
            frame_post(false);
        }
        assert!(due());
        frame_post(false);
        assert!(!due(), "a loader that never settles does not hold the collection back for ever");
        crate::v8host::shutdown();
    }

    /// The pump runs V8's foreground queue, which is where a FinalizationRegistry's cleanup is
    /// posted: before it nothing ran that queue and such callbacks never fired.
    #[test]
    fn the_per_frame_pump_runs_posted_v8_tasks() {
        let _ = crate::v8host::init(dummy_logger());
        test_reset();
        eval_std("fin", r#"
            globalThis.__cleaned = 0;
            globalThis.__reg = new FinalizationRegistry(() => { globalThis.__cleaned++; });
            (function () { __reg.register({}, 1); })();
            "ok";
        "#);
        eval_in_context_string("fin", "__s2_v8_gc(); 'ok'");
        for _ in 0..8 {
            frame_post(true);
        }
        assert_eq!(eval_in_context_string("fin", "String(__cleaned)"), "1");
        crate::v8host::shutdown();
    }
}
