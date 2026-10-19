//! Turning a finished session into something to read: the console report, and the files written
//! off the game thread (the report as text, a Chrome trace, and the JS `.cpuprofile`).

use super::{Event, FrameRecord, Options, Recorder, Stat, FRAMEWORK, GC};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Rows per table in the console report (`sm prof dump <rows>` overrides).
pub(crate) const TOP_ROWS: usize = 15;

/// A finished frame's s2script share below this fraction is "not s2script" in the verdict.
const NOT_US: f64 = 0.10;

#[derive(Clone, Debug)]
pub(crate) struct Paths {
    pub text: PathBuf,
    pub trace: PathBuf,
    pub cpuprofile: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub owner: String,
    pub name: String,
    pub stat: Stat,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnerRow {
    pub owner: String,
    pub self_ns: u64,
    pub calls: u64,
}

/// Everything the console report needs; kept after the session for `sm prof dump`.
#[derive(Clone, Debug)]
pub(crate) struct Summary {
    pub started_at: SystemTime,
    pub wall: u64,
    pub options: Options,
    pub frames: u64,
    pub intervals_avg: u64,
    pub interval_p50: u64,
    pub interval_p99: u64,
    pub interval_max: u64,
    pub long_frames: u64,
    pub s2s_total: u64,
    pub s2s_frame_max: u64,
    pub owners: Vec<OwnerRow>,
    pub rows: Vec<Row>,
    pub worst_interval: Vec<FrameRecord>,
    pub worst_s2s: Vec<FrameRecord>,
    pub timeline_spans: usize,
    pub dropped: u64,
    pub notes: Vec<String>,
    pub files: Option<Paths>,
}

impl Summary {
    pub(crate) const DEFAULT_ROWS: usize = TOP_ROWS;
}

/// What the trace file is written from (moved to the writer thread).
pub(crate) struct TraceData {
    pub started_at: SystemTime,
    pub names: Vec<String>,
    pub owners: Vec<String>,
    pub timeline: Vec<Event>,
    pub frames: Vec<(u64, u64, u64)>,
    pub long_frame_ns: u64,
}

pub(super) fn finish(mut rec: Recorder, now: u64) -> super::Finished {
    let mut intervals: Vec<u64> = rec.frames.iter().map(|f| f.1).collect();
    intervals.sort_unstable();
    let pct = |p: f64| -> u64 {
        if intervals.is_empty() {
            return 0;
        }
        let i = ((intervals.len() - 1) as f64 * p).round() as usize;
        intervals[i.min(intervals.len() - 1)]
    };
    let frames = rec.frames.len() as u64;
    let name = |ids: &super::Interner, id: u32| ids.names[id as usize].clone();

    let mut rows: Vec<Row> = rec
        .stats
        .iter()
        .map(|(&(o, n), s)| Row { owner: name(&rec.owners, o), name: name(&rec.names, n), stat: *s })
        .collect();
    rows.sort_by(|a, b| b.stat.self_ns.cmp(&a.stat.self_ns).then_with(|| a.owner.cmp(&b.owner)));

    let mut owners: Vec<OwnerRow> = Vec::new();
    for row in &rows {
        match owners.iter_mut().find(|o| o.owner == row.owner) {
            Some(o) => {
                o.self_ns += row.stat.self_ns;
                o.calls += row.stat.calls;
            }
            None => owners.push(OwnerRow { owner: row.owner.clone(), self_ns: row.stat.self_ns, calls: row.stat.calls }),
        }
    }
    owners.sort_by(|a, b| b.self_ns.cmp(&a.self_ns));

    // A worst frame's spans that missed the timeline go to the trace anyway, once.
    let mut extra: Vec<Event> = Vec::new();
    for f in rec.worst_interval.iter_mut().chain(rec.worst_s2s.iter_mut()) {
        let events = std::mem::take(&mut f.events);
        if !extra.iter().any(|e| events.first() == Some(e)) {
            extra.extend(events);
        }
    }
    let mut timeline = std::mem::take(&mut rec.timeline);
    timeline.extend(extra);

    let summary = Summary {
        started_at: rec.started_at,
        wall: now,
        options: rec.options.clone(),
        frames,
        intervals_avg: if frames == 0 { 0 } else { intervals.iter().sum::<u64>() / frames },
        interval_p50: pct(0.50),
        interval_p99: pct(0.99),
        interval_max: intervals.last().copied().unwrap_or(0),
        long_frames: rec.long_frames,
        s2s_total: rec.top_level,
        s2s_frame_max: rec.frames.iter().map(|f| f.2).max().unwrap_or(0),
        owners,
        rows,
        worst_interval: rec.worst_interval,
        worst_s2s: rec.worst_s2s,
        timeline_spans: timeline.len(),
        dropped: rec.dropped,
        notes: Vec::new(),
        files: None,
    };
    let trace = TraceData {
        started_at: rec.started_at,
        names: rec.names.names,
        owners: rec.owners.names,
        timeline,
        frames: rec.frames,
        long_frame_ns: u64::from(rec.options.long_frame_ms) * 1_000_000,
    };
    super::Finished { summary, trace }
}

fn ms(ns: u64) -> String {
    format!("{:.2}ms", ns as f64 / 1e6)
}

fn pct_of(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0.0%".to_string();
    }
    format!("{:.1}%", part as f64 * 100.0 / whole as f64)
}

/// `owner` as the report prints it: plugin ids as-is, the two built-in owners described.
fn owner_label(owner: &str) -> String {
    match owner {
        FRAMEWORK => "s2script (framework)".to_string(),
        _ => owner.to_string(),
    }
}

fn frame_line(s: &Summary, f: &FrameRecord) -> String {
    let at = s.started_at + Duration::from_nanos(f.start);
    let game = f.game_frame.map(|g| format!("  GameFrame {}", ms(g))).unwrap_or_default();
    let top = f
        .top
        .iter()
        .filter(|(_, ns)| *ns > 0)
        .take(3)
        .map(|(o, ns)| format!("{} {}", owner_label(o), ms(*ns)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[S2S]   frame {:>7} at {}  interval {:>9}  s2script {:>8} ({}){}{}",
        f.seq,
        clock(at),
        ms(f.interval),
        ms(f.s2s),
        pct_of(f.s2s, f.interval),
        game,
        if top.is_empty() { String::new() } else { format!("  [{top}]") },
    )
}

/// The report: what an operator reads to decide "was it us, and who".
pub(crate) fn console_lines(s: &Summary, rows: usize) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "[S2S] ---- s2script profile: {} from {}, {} frame(s) ----",
        super::format_duration(Duration::from_nanos(s.wall)),
        clock(s.started_at),
        s.frames
    ));
    if s.frames == 0 {
        out.push("[S2S] No frames ran while profiling (is the server hibernating? sv_hibernate_when_empty).".to_string());
    } else {
        out.push(format!(
            "[S2S] Frame interval: avg {}  p50 {}  p99 {}  max {}  long (>= {}ms): {}",
            ms(s.intervals_avg),
            ms(s.interval_p50),
            ms(s.interval_p99),
            ms(s.interval_max),
            s.options.long_frame_ms,
            s.long_frames
        ));
    }
    out.push(format!(
        "[S2S] s2script used {} of {} wall time ({}); most in one frame: {}",
        ms(s.s2s_total),
        super::format_duration(Duration::from_nanos(s.wall)),
        pct_of(s.s2s_total, s.wall),
        ms(s.s2s_frame_max)
    ));
    if let Some(worst) = s.worst_interval.first() {
        let share = if worst.interval == 0 { 0.0 } else { worst.s2s as f64 / worst.interval as f64 };
        out.push(if share < NOT_US {
            format!(
                "[S2S] Verdict: the longest frame ({}) was NOT s2script — it ran {} of it. Look at the engine, the map, or the host (CPU steal, I/O).",
                ms(worst.interval),
                ms(worst.s2s)
            )
        } else {
            let who = worst.top.first().map(|(o, _)| owner_label(o)).unwrap_or_else(|| "s2script".into());
            format!(
                "[S2S] Verdict: s2script ran {} of the longest frame ({}); the biggest share was {}. See the callbacks below and the trace.",
                ms(worst.s2s),
                ms(worst.interval),
                who
            )
        });
    }
    if !s.worst_interval.is_empty() {
        out.push("[S2S] Longest frames (engine frame-to-frame time) and s2script's share:".to_string());
        out.extend(s.worst_interval.iter().take(rows.min(10)).map(|f| frame_line(s, f)));
    }
    if !s.worst_s2s.is_empty() {
        out.push("[S2S] Frames where s2script ran longest:".to_string());
        out.extend(s.worst_s2s.iter().take(rows.min(10)).map(|f| frame_line(s, f)));
    }
    if !s.owners.is_empty() {
        out.push("[S2S] By owner (self time — excludes callees, other plugins and GC):".to_string());
        out.push(format!("[S2S]   {:>10} {:>7} {:>9}  owner", "self", "% wall", "calls"));
        for o in s.owners.iter().take(rows) {
            out.push(format!(
                "[S2S]   {:>10} {:>7} {:>9}  {}",
                ms(o.self_ns),
                pct_of(o.self_ns, s.wall),
                o.calls,
                owner_label(&o.owner)
            ));
        }
    }
    if !s.rows.is_empty() {
        out.push("[S2S] Top callbacks by self time:".to_string());
        out.push(format!(
            "[S2S]   {:>10} {:>9} {:>9} {:>9}  owner :: what",
            "self", "calls", "avg", "max"
        ));
        for r in s.rows.iter().take(rows) {
            out.push(format!(
                "[S2S]   {:>10} {:>9} {:>9} {:>9}  {} :: {}",
                ms(r.stat.self_ns),
                r.stat.calls,
                ms(if r.stat.calls == 0 { 0 } else { r.stat.total / r.stat.calls }),
                ms(r.stat.max),
                owner_label(&r.owner),
                r.name
            ));
        }
    }
    if s.dropped > 0 {
        out.push(format!(
            "[S2S] Timeline full: {} later spans are in the totals but not the trace (the worst frames are kept in full).",
            s.dropped
        ));
    }
    out.extend(s.notes.iter().map(|n| format!("[S2S] {n}")));
    if let Some(js) = js_hot_for(s) {
        out.extend(js.into_iter().take(rows + 1));
    } else if s.files.as_ref().is_some_and(|f| f.cpuprofile.is_some()) {
        out.push("[S2S] JS function breakdown: being written — \"sm prof dump\" shows it in a moment.".to_string());
    }
    if let Some(files) = &s.files {
        out.push(format!("[S2S] Report: {}", files.text.display()));
        out.push(format!(
            "[S2S] Trace:  {}  ({} spans; open in https://ui.perfetto.dev or chrome://tracing)",
            files.trace.display(),
            s.timeline_spans
        ));
        if let Some(p) = &files.cpuprofile {
            out.push(format!(
                "[S2S] JS:     {}  (Chrome DevTools > Performance > Load profile, or https://www.speedscope.app)",
                p.display()
            ));
        }
    }
    if let Some(e) = take_write_error() {
        out.push(format!("[S2S] Writing the previous report failed: {e}"));
    }
    out
}

static WRITE_ERROR: Mutex<Option<String>> = Mutex::new(None);
/// The JS hot-function table of the last profile, computed on the writer thread: (session stamp,
/// lines). `sm prof dump` shows it once it is ready.
static JS_HOT: Mutex<Option<(String, Vec<String>)>> = Mutex::new(None);
const JS_ROWS: usize = 25;

fn js_lines(profile: &serde_json::Value, rows: usize) -> Vec<String> {
    let hot = super::js::hot_functions(profile, rows);
    let mut out = vec!["[S2S] Hottest JS functions (self time, sampled):".to_string()];
    if hot.is_empty() {
        out.push("[S2S]   (no JS samples — no plugin code ran while sampling)".to_string());
    }
    for (ns, func, place) in hot {
        out.push(format!("[S2S]   {:>10}  {}  {}", ms(ns), func, place));
    }
    out
}

fn js_hot_for(s: &Summary) -> Option<Vec<String>> {
    let g = match JS_HOT.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    g.as_ref().filter(|(st, _)| *st == stamp(s.started_at)).map(|(_, lines)| lines.clone())
}

fn take_write_error() -> Option<String> {
    match WRITE_ERROR.lock() {
        Ok(mut g) => g.take(),
        Err(p) => p.into_inner().take(),
    }
}

/// Write the report files on a thread of their own: a trace can be tens of MB, and serializing it on
/// the game thread would cause exactly the hitch the profiler exists to find. Failures surface on the
/// next `sm prof` report (the console belongs to the game thread).
pub(crate) fn write_files(paths: Paths, lines: Vec<String>, trace: TraceData, cpuprofile: Option<String>) {
    let spawned = std::thread::Builder::new().name("s2s-profiler-write".into()).spawn(move || {
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = paths.text.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut lines = lines;
            if let (Some(path), Some(response)) = (&paths.cpuprofile, &cpuprofile) {
                let profile = super::js::extract_profile(response)
                    .ok_or_else(|| std::io::Error::other("Profiler.stop returned no profile"))?;
                std::fs::write(path, serde_json::to_vec(&profile).map_err(std::io::Error::other)?)?;
                let hot = js_lines(&profile, JS_ROWS);
                lines.extend(hot.iter().cloned());
                let mut g = match JS_HOT.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                *g = Some((stamp(trace.started_at), hot));
            }
            let text: String = lines.iter().map(|l| format!("{}\n", l.strip_prefix("[S2S] ").unwrap_or(l))).collect();
            std::fs::write(&paths.text, text)?;
            let file = std::fs::File::create(&paths.trace)?;
            write_trace(&mut std::io::BufWriter::new(file), &trace)?;
            Ok(())
        })();
        if let Err(e) = result {
            let mut g = match WRITE_ERROR.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *g = Some(e.to_string());
        }
    });
    if let Err(e) = spawned {
        crate::v8host::log_warn(&format!("[S2S] Profiler: could not start the report writer: {e}"));
    }
}

fn json(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"?\"".to_string())
}

fn us(ns: u64) -> String {
    format!("{}.{:03}", ns / 1000, ns % 1000)
}

/// Chrome trace-event format (the JSON-object form). One process, one thread (the game thread):
/// complete (`X`) events nest by time, so Perfetto/chrome://tracing draw the call tree as a flame
/// chart. Two counter tracks carry each frame's interval and s2script's share; long frames get an
/// instant marker.
pub(crate) fn write_trace(w: &mut impl Write, t: &TraceData) -> std::io::Result<()> {
    let started_us = t.started_at.duration_since(UNIX_EPOCH).map(|d| d.as_micros()).unwrap_or(0);
    write!(
        w,
        "{{\"displayTimeUnit\":\"ms\",\"otherData\":{{\"producer\":\"s2script {}\",\"startedUnixMicros\":{}}},\"traceEvents\":[",
        env!("CARGO_PKG_VERSION"),
        started_us
    )?;
    write!(w, "{{\"name\":\"process_name\",\"ph\":\"M\",\"pid\":1,\"args\":{{\"name\":\"s2script\"}}}}")?;
    write!(w, ",\n{{\"name\":\"thread_name\",\"ph\":\"M\",\"pid\":1,\"tid\":1,\"args\":{{\"name\":\"game thread\"}}}}")?;
    for e in &t.timeline {
        let owner = &t.owners[e.owner as usize];
        let name = &t.names[e.name as usize];
        let label = if owner == FRAMEWORK || owner == GC { name.clone() } else { format!("{owner} \u{b7} {name}") };
        write!(
            w,
            ",\n{{\"name\":{},\"cat\":{},\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":1,\"tid\":1}}",
            json(&label),
            json(owner),
            us(e.start),
            us(e.dur)
        )?;
    }
    for &(start, interval, s2s) in &t.frames {
        write!(
            w,
            ",\n{{\"name\":\"frame interval (ms)\",\"ph\":\"C\",\"ts\":{},\"pid\":1,\"args\":{{\"ms\":{:.3}}}}}",
            us(start),
            interval as f64 / 1e6
        )?;
        write!(
            w,
            ",\n{{\"name\":\"s2script per frame (ms)\",\"ph\":\"C\",\"ts\":{},\"pid\":1,\"args\":{{\"ms\":{:.3}}}}}",
            us(start),
            s2s as f64 / 1e6
        )?;
        if interval >= t.long_frame_ns && t.long_frame_ns > 0 {
            write!(
                w,
                ",\n{{\"name\":{},\"ph\":\"i\",\"s\":\"g\",\"ts\":{},\"pid\":1,\"tid\":1}}",
                json(&format!("long frame {:.2}ms (s2script {:.2}ms)", interval as f64 / 1e6, s2s as f64 / 1e6)),
                us(start)
            )?;
        }
    }
    write!(w, "\n]}}\n")?;
    w.flush()
}

/// UTC civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn parts(t: SystemTime) -> (i64, u32, u32, u64, u64, u64, u32) {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (y, m, day) = civil((secs / 86_400) as i64);
    let rem = secs % 86_400;
    (y, m, day, rem / 3600, rem % 3600 / 60, rem % 60, d.subsec_millis())
}

/// `20261019-121314` (UTC), for file names.
pub(crate) fn stamp(t: SystemTime) -> String {
    let (y, m, d, hh, mm, ss, _) = parts(t);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// `12:13:14.567Z`, to line a frame up with the server log.
pub(crate) fn clock(t: SystemTime) -> String {
    let (_, _, _, hh, mm, ss, ms) = parts(t);
    format!("{hh:02}:{mm:02}:{ss:02}.{ms:03}Z")
}

#[cfg(test)]
pub(super) fn civil_for_test(days: i64) -> (i64, u32, u32) {
    civil(days)
}
