//! `sm prof` — the runtime's own profiler, as SourceMod's `sm prof` was.
//!
//! The question it answers: the server hitched ("UNEXPECTED LONG FRAME", a rubber-band, a slow
//! round start) — was it s2script, and if so which plugin, which callback? Or was the engine (or the
//! host) on its own?
//!
//! **Model.** Every place the core does work on the game thread opens a [`Span`] naming an OWNER and
//! what ran: a plugin id for a plugin's callback, [`FRAMEWORK`] for the runtime's own work, [`GC`]
//! for V8 garbage collection. Spans nest. A span's SELF time is its duration minus its children's,
//! so a plugin calling another plugin's interface, or a GC landing inside a handler, is charged to
//! whoever actually ran. Async continuations (code after an `await`, a `.then`) run in one shared
//! microtask checkpoint; a promise hook installed only while profiling charges each one to the
//! plugin whose context created it.
//!
//! **Frames.** The profiler keeps the `GameFrame` hook installed and cuts time into windows at each
//! frame's PRE: a window's length is the engine's frame-to-frame interval, and the depth-0 spans
//! closed inside it are s2script's share of it. That is what rules s2script in or out of a long
//! frame — a 70 ms window with 0.1 ms of s2script in it was not s2script.
//!
//! **Cost.** Off: one thread-local bool load per span site. On: two clock reads and a few hashmap
//! updates per span. The timeline (for the trace file) is bounded by `Options::timeline_cap`; the
//! per-callback totals and the worst frames are kept whatever the cap.
//!
//! **Output** (written off-thread when a session stops, under `data/profiles/`):
//! - `<stamp>.txt` — the console report;
//! - `<stamp>.trace.json` — Chrome trace-event format: open in <https://ui.perfetto.dev> or
//!   `chrome://tracing`;
//! - `<stamp>.cpuprofile` — only with `sm prof start js`: V8's JS sampling profile, per function and
//!   line; open in Chrome DevTools (Performance → Load profile) or <https://www.speedscope.app>.
//!
//! Engine-generic: no game names, no V8 types in the recorder. The V8 hooks (GC, promise, inspector)
//! live in `v8host` and [`js`] and only call the plain functions here.

pub(crate) mod js;
mod report;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

pub(crate) use report::Summary;

/// The owner charged for the runtime's own work.
pub(crate) const FRAMEWORK: &str = "s2script";
/// The owner charged for V8 garbage collection (shared by every plugin).
pub(crate) const GC: &str = "v8 (garbage collection)";

/// Spans kept for the trace file before the timeline stops growing. 32 bytes each.
pub(crate) const DEFAULT_TIMELINE_CAP: usize = 1_000_000;
/// A frame window at least this long is a "long frame" in the report.
pub(crate) const DEFAULT_LONG_FRAME_MS: u32 = 50;
/// How many worst frames each list keeps.
const WORST_FRAMES: usize = 10;
/// Spans kept per worst frame that did not fit in the timeline.
const WORST_FRAME_EVENTS: usize = 20_000;
/// Frame intervals kept for percentiles and the trace counter track (~17 hours at 64 Hz).
const FRAME_CAP: usize = 4_000_000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Options {
    /// Stop on its own after this long. `None` = until `sm prof stop`.
    pub duration: Option<Duration>,
    /// Also run V8's JS sampling profiler (per-function detail, `.cpuprofile`).
    pub js: bool,
    pub timeline_cap: usize,
    pub long_frame_ms: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { duration: None, js: false, timeline_cap: DEFAULT_TIMELINE_CAP, long_frame_ms: DEFAULT_LONG_FRAME_MS }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Normal,
    Gc,
    Async,
}

struct Open {
    owner: u32,
    name: u32,
    start: u64,
    child: u64,
    kind: Kind,
}

/// One closed span on the timeline. Times are ns since the session started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub start: u64,
    pub dur: u64,
    pub owner: u32,
    pub name: u32,
    pub depth: u16,
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Stat {
    pub calls: u64,
    pub total: u64,
    pub self_ns: u64,
    pub max: u64,
}

#[derive(Default)]
struct Interner {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.ids.get(s) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(s.to_string());
        self.ids.insert(s.to_string(), id);
        id
    }
}

/// One frame window: from one `GameFrame` PRE to the next.
#[derive(Clone, Debug)]
pub(crate) struct FrameRecord {
    /// 1-based frame number within the session.
    pub seq: u64,
    pub start: u64,
    /// PRE to next PRE: the engine's frame-to-frame time.
    pub interval: u64,
    /// PRE to POST of this frame's `GameFrame` (the server's own simulation), when POST was seen.
    pub game_frame: Option<u64>,
    /// s2script's depth-0 time inside the window.
    pub s2s: u64,
    /// Up to five owners by self time inside the window.
    pub top: Vec<(String, u64)>,
    /// The window's spans when they did not fit in the timeline (worst frames only).
    pub events: Vec<Event>,
}

#[derive(Default)]
struct Window {
    start: u64,
    /// Set for a real frame (opened by a PRE); the lead-in window before the first PRE has none.
    pre: Option<u64>,
    game_frame: Option<u64>,
    s2s: u64,
    by_owner: HashMap<u32, u64>,
    events: Vec<Event>,
}

struct Recorder {
    session: u32,
    base: Instant,
    started_at: SystemTime,
    options: Options,
    names: Interner,
    owners: Interner,
    stack: Vec<Open>,
    stats: HashMap<(u32, u32), Stat>,
    timeline: Vec<Event>,
    dropped: u64,
    window: Window,
    seq: u64,
    /// (start, interval, s2s) per complete frame.
    frames: Vec<(u64, u64, u64)>,
    long_frames: u64,
    worst_interval: Vec<FrameRecord>,
    worst_s2s: Vec<FrameRecord>,
    /// All depth-0 time: s2script's total share of the wall clock.
    top_level: u64,
}

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static SESSION: Cell<u32> = const { Cell::new(0) };
    static REC: RefCell<Option<Recorder>> = const { RefCell::new(None) };
    /// The last finished session's summary, for `sm prof dump`.
    static LAST: RefCell<Option<Summary>> = const { RefCell::new(None) };
    /// A start/stop that could not take effect where it was asked (V8 busy) waits for the next frame.
    static PENDING: RefCell<Option<Request>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Request {
    Start(Options),
    Stop,
}

/// Is a session recording? One thread-local load — the whole cost of a span site while off.
#[inline]
pub(crate) fn active() -> bool {
    ACTIVE.with(Cell::get)
}

/// RAII span. Inert (session 0) when the profiler was off at open, or when the session it opened
/// in has since ended — so stopping mid-span never corrupts the next session.
#[must_use]
pub(crate) struct Span {
    session: u32,
    depth: usize,
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.session != 0 {
            close_to(self.session, self.depth, None);
        }
    }
}

const INERT: Span = Span { session: 0, depth: 0 };

/// Open a span charged to `owner` (a plugin id, [`FRAMEWORK`] or [`GC`]).
#[inline]
pub(crate) fn span(owner: &str, name: &str) -> Span {
    if !active() {
        return INERT;
    }
    open(owner, name, Kind::Normal)
}

/// [`span`] whose name is built only while profiling (it allocates or touches V8).
#[inline]
pub(crate) fn span_lazy(owner: &str, name: impl FnOnce() -> String) -> Span {
    if !active() {
        return INERT;
    }
    open(owner, &name(), Kind::Normal)
}

/// A span charged to the runtime itself.
#[inline]
pub(crate) fn framework(name: &str) -> Span {
    span(FRAMEWORK, name)
}

fn now_ns(base: Instant) -> u64 {
    base.elapsed().as_nanos() as u64
}

fn open(owner: &str, name: &str, kind: Kind) -> Span {
    REC.with(|r| {
        let Ok(mut r) = r.try_borrow_mut() else { return INERT };
        let Some(rec) = r.as_mut() else { return INERT };
        let owner = rec.owners.id(owner);
        let name = rec.names.id(name);
        let start = now_ns(rec.base);
        rec.stack.push(Open { owner, name, start, child: 0, kind });
        Span { session: rec.session, depth: rec.stack.len() }
    })
}

/// Close every open span at `depth` and above (a guard closes its own span, plus any a hook left
/// open above it). `kind` closes back to the most recent open span of that kind instead.
fn close_to(session: u32, depth: usize, kind: Option<Kind>) {
    REC.with(|r| {
        let Ok(mut r) = r.try_borrow_mut() else { return };
        let Some(rec) = r.as_mut() else { return };
        if rec.session != session {
            return;
        }
        let depth = match kind {
            Some(k) => match rec.stack.iter().rposition(|o| o.kind == k) {
                Some(i) => i + 1,
                None => return,
            },
            None => depth,
        };
        if depth == 0 {
            return;
        }
        let now = now_ns(rec.base);
        while rec.stack.len() >= depth {
            let open = rec.stack.pop().expect("len >= depth >= 1");
            rec.finish(open, now);
        }
    })
}

impl Recorder {
    fn new(session: u32, options: Options) -> Self {
        Self {
            session,
            base: Instant::now(),
            started_at: SystemTime::now(),
            options,
            names: Interner::default(),
            owners: Interner::default(),
            stack: Vec::new(),
            stats: HashMap::new(),
            timeline: Vec::new(),
            dropped: 0,
            window: Window::default(),
            seq: 0,
            frames: Vec::new(),
            long_frames: 0,
            worst_interval: Vec::new(),
            worst_s2s: Vec::new(),
            top_level: 0,
        }
    }

    fn finish(&mut self, open: Open, now: u64) {
        let dur = now.saturating_sub(open.start);
        let self_ns = dur.saturating_sub(open.child);
        if let Some(parent) = self.stack.last_mut() {
            parent.child += dur;
        } else {
            self.window.s2s += dur;
            self.top_level += dur;
        }
        let stat = self.stats.entry((open.owner, open.name)).or_default();
        stat.calls += 1;
        stat.total += dur;
        stat.self_ns += self_ns;
        stat.max = stat.max.max(dur);
        *self.window.by_owner.entry(open.owner).or_default() += self_ns;
        // A window that never ends (no frames: a hibernating server) must not grow without bound.
        if self.window.events.len() >= self.options.timeline_cap {
            self.dropped += 1;
            return;
        }
        self.window.events.push(Event {
            start: open.start,
            dur,
            owner: open.owner,
            name: open.name,
            depth: self.stack.len().min(u16::MAX as usize) as u16,
        });
    }

    /// End the current window at `now`. A real frame (opened by a PRE) is counted; the lead-in
    /// before the first PRE and the tail after the last are not frames, only timeline.
    fn close_window(&mut self, now: u64, complete: bool) {
        let w = std::mem::take(&mut self.window);
        let in_timeline = self.timeline.len() + w.events.len() <= self.options.timeline_cap;
        if !(complete && w.pre.is_some()) {
            self.keep_events(w.events, in_timeline);
            return;
        }
        self.seq += 1;
        let interval = now.saturating_sub(w.start);
        if interval >= u64::from(self.options.long_frame_ms) * 1_000_000 {
            self.long_frames += 1;
        }
        if self.frames.len() < FRAME_CAP {
            self.frames.push((w.start, interval, w.s2s));
        }
        let mut top: Vec<(u32, u64)> = w.by_owner.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        top.truncate(5);
        let record = FrameRecord {
            seq: self.seq,
            start: w.start,
            interval,
            game_frame: w.game_frame,
            s2s: w.s2s,
            top: top.into_iter().map(|(o, ns)| (self.owners.names[o as usize].clone(), ns)).collect(),
            events: Vec::new(),
        };
        let qualifies = |list: &[FrameRecord], key: fn(&FrameRecord) -> u64| {
            list.len() < WORST_FRAMES || list.iter().any(|f| key(f) < key(&record))
        };
        let by_interval = qualifies(&self.worst_interval, |f| f.interval);
        let by_s2s = record.s2s > 0 && qualifies(&self.worst_s2s, |f| f.s2s);
        let detail = if in_timeline || !(by_interval || by_s2s) {
            Vec::new()
        } else {
            w.events.iter().take(WORST_FRAME_EVENTS).copied().collect()
        };
        if by_interval {
            insert_worst(&mut self.worst_interval, FrameRecord { events: detail.clone(), ..record.clone() }, |f| f.interval);
        }
        if by_s2s {
            insert_worst(&mut self.worst_s2s, FrameRecord { events: detail, ..record }, |f| f.s2s);
        }
        self.keep_events(w.events, in_timeline);
    }

    fn keep_events(&mut self, events: Vec<Event>, in_timeline: bool) {
        if in_timeline {
            self.timeline.extend(events);
        } else {
            self.dropped += events.len() as u64;
        }
    }
}

fn insert_worst(list: &mut Vec<FrameRecord>, record: FrameRecord, key: fn(&FrameRecord) -> u64) {
    list.push(record);
    list.sort_by_key(|f| std::cmp::Reverse(key(f)));
    list.truncate(WORST_FRAMES);
}

/// `GameFrame` PRE, before anything else in the frame: closes the previous frame's window.
pub(crate) fn frame_pre() {
    if !active() {
        return;
    }
    REC.with(|r| {
        let Ok(mut r) = r.try_borrow_mut() else { return };
        let Some(rec) = r.as_mut() else { return };
        let now = now_ns(rec.base);
        rec.close_window(now, true);
        rec.window.start = now;
        rec.window.pre = Some(now);
    });
}

/// `GameFrame` POST, after the runtime's post-frame work: the server's own frame time.
pub(crate) fn frame_post() {
    if !active() {
        return;
    }
    REC.with(|r| {
        let Ok(mut r) = r.try_borrow_mut() else { return };
        let Some(rec) = r.as_mut() else { return };
        let now = now_ns(rec.base);
        if let Some(pre) = rec.window.pre {
            rec.window.game_frame = Some(now.saturating_sub(pre));
        }
    });
    if let Some(at) = due_stop() {
        crate::v8host::log_warn(&format!("[S2S] Profiler: {at} elapsed — stopping."));
        for line in stop_and_report() {
            crate::v8host::log_warn(&line);
        }
    }
}

fn due_stop() -> Option<String> {
    REC.with(|r| {
        let r = r.try_borrow().ok()?;
        let rec = r.as_ref()?;
        let limit = rec.options.duration?;
        (rec.base.elapsed() >= limit).then(|| format_duration(limit))
    })
}

/// GC prologue (V8 callback).
pub(crate) fn gc_begin(kind: &str) {
    if active() {
        std::mem::forget(open(GC, kind, Kind::Gc));
    }
}

/// GC epilogue (V8 callback). Closes back to the matching prologue; one seen without its prologue
/// (the session started mid-GC) is ignored.
pub(crate) fn gc_end() {
    if active() {
        close_to(SESSION.with(Cell::get), 0, Some(Kind::Gc));
    }
}

/// Promise hook BEFORE: an async continuation of `owner` starts.
pub(crate) fn async_begin(owner: &str) {
    if active() {
        std::mem::forget(open(owner, "async continuation (await / .then)", Kind::Async));
    }
}

/// Promise hook AFTER.
pub(crate) fn async_end() {
    if active() {
        close_to(SESSION.with(Cell::get), 0, Some(Kind::Async));
    }
}

/// Begin a session now. Recording state only; the caller (`request`) installs the V8 hooks.
fn begin(options: Options) -> u32 {
    let session = SESSION.with(|s| {
        let n = s.get().wrapping_add(1).max(1);
        s.set(n);
        n
    });
    REC.with(|r| *r.borrow_mut() = Some(Recorder::new(session, options)));
    ACTIVE.with(|a| a.set(true));
    session
}

/// The finished session's data, moved out of the recorder.
pub(crate) struct Finished {
    pub summary: Summary,
    pub trace: report::TraceData,
}

/// End the session (if any) and hand back everything it recorded. Spans still open (the command
/// that asked to stop is one) are discarded; their guards are inert from here on.
fn end() -> Option<Finished> {
    ACTIVE.with(|a| a.set(false));
    let mut rec = REC.with(|r| r.borrow_mut().take())?;
    let now = now_ns(rec.base);
    rec.stack.clear();
    rec.close_window(now, false);
    Some(report::finish(rec, now))
}

// ---------------------------------------------------------------------------
// Control: `sm prof …`
// ---------------------------------------------------------------------------

/// Ask for a start or stop. Applied now when V8 is free (the console command path), otherwise at
/// the top of the next frame. Returns the console lines to print.
pub(crate) fn request(req: Request) -> Vec<String> {
    match req {
        Request::Start(options) => {
            if active() {
                return vec!["[S2S] Profiler is already running. Use \"sm prof stop\" first.".to_string()];
            }
            if crate::v8host::with_host_isolate(|_| ()).is_ok() {
                start_now(options)
            } else {
                // No `refresh_detour` here: V8 is busy, so a dispatch is on the stack and may hold the
                // frame/timer tables it reads. Something is running, so frames are coming anyway.
                PENDING.with(|p| *p.borrow_mut() = Some(Request::Start(options)));
                vec!["[S2S] Profiler will start on the next frame.".to_string()]
            }
        }
        Request::Stop => {
            if !active() && PENDING.with(|p| p.borrow_mut().take()).is_some() {
                return vec!["[S2S] Profiler start cancelled.".to_string()];
            }
            if !active() {
                return vec!["[S2S] Profiler is not running. Use \"sm prof start\".".to_string()];
            }
            if crate::v8host::with_host_isolate(|_| ()).is_ok() {
                stop_and_report()
            } else {
                PENDING.with(|p| *p.borrow_mut() = Some(Request::Stop));
                vec!["[S2S] Profiler will stop on the next frame.".to_string()]
            }
        }
    }
}

/// Whether the frame hook must stay installed for the profiler (a session, or one waiting to start).
pub(crate) fn wants_frames() -> bool {
    active() || PENDING.with(|p| p.borrow().is_some())
}

/// Top of every `GameFrame` PRE, V8 free: apply a deferred start/stop.
pub(crate) fn apply_pending() {
    let Some(req) = PENDING.with(|p| p.borrow_mut().take()) else { return };
    let lines = match req {
        Request::Start(options) => start_now(options),
        Request::Stop => stop_and_report(),
    };
    for line in lines {
        crate::v8host::log_warn(&line);
    }
}

fn start_now(options: Options) -> Vec<String> {
    let js = options.js;
    let duration = options.duration;
    begin(options);
    crate::v8host::profiler_hooks(true);
    crate::v8host::refresh_detour();
    let mut lines = vec![format!(
        "[S2S] Profiler started{}{}.",
        duration.map(|d| format!(" for {}", format_duration(d))).unwrap_or_default(),
        if js { " with JS sampling" } else { "" },
    )];
    if js {
        if let Err(e) = js::start() {
            lines.push(format!("[S2S] JS sampling unavailable: {e}. Recording spans only."));
        }
    }
    if duration.is_none() {
        lines.push("[S2S] Run \"sm prof stop\" to finish and write the report.".to_string());
    }
    lines
}

/// Stop now (V8 must be free), print the report and write the files off-thread.
pub(crate) fn stop_and_report() -> Vec<String> {
    let cpuprofile = js::stop();
    crate::v8host::profiler_hooks(false);
    let Some(finished) = end() else {
        return vec!["[S2S] Profiler is not running.".to_string()];
    };
    crate::v8host::refresh_detour();
    let dir = profiles_dir();
    let stamp = report::stamp(finished.summary.started_at);
    let mut summary = finished.summary;
    let paths = dir.as_ref().map(|d| report::Paths {
        text: d.join(format!("prof-{stamp}.txt")),
        trace: d.join(format!("prof-{stamp}.trace.json")),
        cpuprofile: cpuprofile.as_ref().ok().map(|_| d.join(format!("prof-{stamp}.cpuprofile"))),
    });
    summary.files = paths.clone();
    if let Err(e) = &cpuprofile {
        if !e.is_empty() {
            summary.notes.push(format!("JS sampling: {e}"));
        }
    }
    let lines = report::console_lines(&summary, report::TOP_ROWS);
    match paths {
        Some(paths) => report::write_files(paths, lines.clone(), finished.trace, cpuprofile.ok()),
        None => crate::v8host::log_warn("[S2S] Profiler: no data directory (plugin loader not running) — report not written to disk."),
    }
    LAST.with(|l| *l.borrow_mut() = Some(summary));
    lines
}

/// `sm prof status`.
pub(crate) fn status() -> Vec<String> {
    REC.with(|r| {
        let r = r.borrow();
        match r.as_ref() {
            Some(rec) => vec![format!(
                "[S2S] Profiler running for {} ({} frames, {} spans on the timeline{}){}.",
                format_duration(rec.base.elapsed()),
                rec.seq,
                rec.timeline.len(),
                if rec.dropped > 0 { ", timeline full" } else { "" },
                rec.options.duration.map(|d| format!(", stops at {}", format_duration(d))).unwrap_or_default(),
            )],
            None if PENDING.with(|p| p.borrow().is_some()) => vec!["[S2S] Profiler starts on the next frame.".to_string()],
            None => vec!["[S2S] Profiler is not running.".to_string()],
        }
    })
}

/// `sm prof dump [rows]`: the last finished session's report again.
pub(crate) fn dump(rows: usize) -> Vec<String> {
    LAST.with(|l| match l.borrow().as_ref() {
        Some(summary) => report::console_lines(summary, rows),
        None => vec!["[S2S] No finished profile yet. Use \"sm prof start\", then \"sm prof stop\".".to_string()],
    })
}

/// The last finished session's summary.
pub(crate) fn last_summary() -> Option<Summary> {
    LAST.with(|l| l.borrow().clone())
}

/// `sm prof clear`.
pub(crate) fn clear() -> Vec<String> {
    LAST.with(|l| *l.borrow_mut() = None);
    vec!["[S2S] Profiler report cleared.".to_string()]
}

/// Shutdown: drop a running session without writing anything (V8 is about to go away).
pub(crate) fn shutdown() {
    js::abandon();
    ACTIVE.with(|a| a.set(false));
    REC.with(|r| *r.borrow_mut() = None);
    PENDING.with(|p| *p.borrow_mut() = None);
}

/// `addons/s2script/data/profiles/` — next to the SQLite files, which operators already keep
/// writable.
fn profiles_dir() -> Option<PathBuf> {
    let plugins = crate::loader::plugins_dir()?;
    Some(plugins.parent()?.join("data").join("profiles"))
}

pub(crate) fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

/// Start or end a bare recording session (no V8 hooks), for tests outside this module.
#[cfg(test)]
pub(crate) fn test_set_active(on: bool) {
    if on {
        begin(Options::default());
    } else {
        let _ = end();
    }
}

#[cfg(test)]
mod tests;
