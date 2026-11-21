//! `sm prof start js`: V8's own sampling CPU profiler, driven through the inspector protocol's
//! `Profiler` domain (rusty_v8 binds the inspector, not `v8::CpuProfiler`). Spans say which plugin
//! callback was slow; the `.cpuprofile` says which function and line inside it.
//!
//! The same session runs V8's sampling heap profiler (the `HeapProfiler` domain), keeping the samples
//! of objects that have since been collected: the `.heapprofile` says which function allocated how
//! much while the session ran, which is what decides how often the garbage collector has to run.
//!
//! The inspector exists only while a session samples: created at start, dropped at stop (V8 is free
//! at both — the console command path or the top of a frame), so a server that never profiles never
//! carries one. The profile JSON is extracted and written on the report writer thread, never parsed
//! on the game thread.

use std::cell::RefCell;
use std::rc::Rc;
use v8::inspector::{
    Channel, ChannelImpl, StringBuffer, StringView, V8Inspector, V8InspectorClient, V8InspectorClientImpl,
    V8InspectorClientTrustLevel, V8InspectorSession,
};

/// Sampling interval, µs. V8's default is 1000; finer resolves sub-millisecond handlers.
const SAMPLING_INTERVAL_US: u32 = 250;
/// Average bytes allocated between two allocation samples. V8's default is 32768; finer names the
/// smaller allocators of a 30-second session without a measurable cost per allocation.
const ALLOC_SAMPLING_INTERVAL_BYTES: u32 = 8192;

/// What a session sampled: the raw protocol responses, parsed on the writer thread.
pub(crate) struct Sampled {
    /// `Profiler.stop`.
    pub cpu: String,
    /// `HeapProfiler.stopSampling`; `None` when allocation sampling did not start.
    pub alloc: Option<String>,
}

type Responses = Rc<RefCell<Vec<(i32, String)>>>;

struct Collector(Responses);

impl ChannelImpl for Collector {
    fn send_response(&self, call_id: i32, mut message: v8::UniquePtr<StringBuffer>) {
        if let Some(buf) = message.as_mut() {
            self.0.borrow_mut().push((call_id, buf.string().to_string()));
        }
    }
    fn send_notification(&self, _message: v8::UniquePtr<StringBuffer>) {}
    fn flush_protocol_notifications(&self) {}
}

struct Client;
impl V8InspectorClientImpl for Client {}

struct Sampler {
    // Field order is drop order: the session before the inspector that owns it.
    session: V8InspectorSession,
    _inspector: V8Inspector,
    responses: Responses,
    next_id: i32,
    /// Whether allocation sampling started (it is extra: CPU sampling goes on without it).
    alloc: bool,
}

thread_local! {
    static SAMPLER: RefCell<Option<Sampler>> = const { RefCell::new(None) };
}

impl Sampler {
    /// Dispatch one protocol call and return its response (the inspector answers synchronously).
    fn call(&mut self, method: &str, params: &str) -> Result<String, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = format!("{{\"id\":{id},\"method\":\"{method}\",\"params\":{params}}}");
        self.session.dispatch_protocol_message(StringView::from(msg.as_bytes()));
        let mut responses = self.responses.borrow_mut();
        let at = responses.iter().position(|(i, _)| *i == id).ok_or_else(|| format!("{method}: no response"))?;
        let (_, body) = responses.remove(at);
        if body.contains("\"error\":") && !body.contains("\"result\":") {
            return Err(format!("{method}: {body}"));
        }
        Ok(body)
    }
}

/// Start sampling. V8 must be free.
pub(crate) fn start() -> Result<(), String> {
    abandon();
    let sampler = crate::v8host::with_host_isolate(|isolate| -> Result<Sampler, String> {
        let responses: Responses = Rc::new(RefCell::new(Vec::new()));
        let inspector = V8Inspector::create(isolate, V8InspectorClient::new(Box::new(Client)));
        let session = inspector.connect(
            1,
            Channel::new(Box::new(Collector(responses.clone()))),
            StringView::empty(),
            V8InspectorClientTrustLevel::FullyTrusted,
        );
        let mut sampler = Sampler { session, _inspector: inspector, responses, next_id: 0, alloc: false };
        sampler.call("Profiler.enable", "{}")?;
        sampler.call("Profiler.setSamplingInterval", &format!("{{\"interval\":{SAMPLING_INTERVAL_US}}}"))?;
        sampler.call("Profiler.start", "{}")?;
        // Collected objects stay in the profile: the question is what allocated, not what is live.
        sampler.alloc = sampler.call("HeapProfiler.enable", "{}").is_ok()
            && sampler
                .call(
                    "HeapProfiler.startSampling",
                    &format!(
                        "{{\"samplingInterval\":{ALLOC_SAMPLING_INTERVAL_BYTES},\"includeObjectsCollectedByMajorGC\":true,\"includeObjectsCollectedByMinorGC\":true}}"
                    ),
                )
                .is_ok();
        Ok(sampler)
    })
    .map_err(|_| "the JavaScript engine is busy".to_string())??;
    SAMPLER.with(|s| *s.borrow_mut() = Some(sampler));
    Ok(())
}

/// Stop sampling and return the raw responses (the writer thread extracts the profiles).
/// `Err("")` when no sampler was running — not an error worth printing.
pub(crate) fn stop() -> Result<Sampled, String> {
    let Some(mut sampler) = SAMPLER.with(|s| s.borrow_mut().take()) else {
        return Err(String::new());
    };
    let result = crate::v8host::with_host_isolate(|_isolate| {
        let alloc = if sampler.alloc { sampler.call("HeapProfiler.stopSampling", "{}").ok() } else { None };
        let _ = sampler.call("HeapProfiler.disable", "{}");
        let cpu = sampler.call("Profiler.stop", "{}");
        let _ = sampler.call("Profiler.disable", "{}");
        drop(sampler);
        cpu.map(|cpu| Sampled { cpu, alloc })
    });
    match result {
        Ok(sampled) => sampled,
        Err(_) => Err("the JavaScript engine was busy at stop".to_string()),
    }
}

/// Drop a sampler without collecting (shutdown, or a fresh start).
pub(crate) fn abandon() {
    if let Some(sampler) = SAMPLER.with(|s| s.borrow_mut().take()) {
        drop(sampler);
    }
}

/// From a raw `Profiler.stop` response, the `.cpuprofile` body (the `result.profile` object).
pub(crate) fn extract_profile(response: &str) -> Option<serde_json::Value> {
    let mut v: serde_json::Value = serde_json::from_str(response).ok()?;
    Some(v.get_mut("result")?.get_mut("profile")?.take())
}

/// Self time per JS function from a `.cpuprofile`, heaviest first: `(self ns, function, url:line)`.
/// Samples are spread evenly over the profile's span, as DevTools does.
pub(crate) fn hot_functions(profile: &serde_json::Value, limit: usize) -> Vec<(u64, String, String)> {
    let Some(nodes) = profile.get("nodes").and_then(|n| n.as_array()) else { return Vec::new() };
    let samples = profile.get("samples").and_then(|s| s.as_array()).map(|s| s.len()).unwrap_or(0);
    let span_us = profile.get("endTime").and_then(|v| v.as_u64()).unwrap_or(0)
        .saturating_sub(profile.get("startTime").and_then(|v| v.as_u64()).unwrap_or(0));
    if samples == 0 {
        return Vec::new();
    }
    let per_sample_ns = span_us * 1000 / samples as u64;
    let mut hits: std::collections::HashMap<(String, String), u64> = std::collections::HashMap::new();
    for node in nodes {
        let count = node.get("hitCount").and_then(|h| h.as_u64()).unwrap_or(0);
        if count == 0 {
            continue;
        }
        let frame = &node["callFrame"];
        let func = frame["functionName"].as_str().unwrap_or("");
        let func = if func.is_empty() { "(anonymous)" } else { func };
        let url = frame["url"].as_str().unwrap_or("");
        let line = frame["lineNumber"].as_i64().unwrap_or(-1) + 1;
        let place = if url.is_empty() { String::new() } else { format!("{url}:{line}") };
        *hits.entry((func.to_string(), place)).or_default() += count;
    }
    let mut rows: Vec<(u64, String, String)> =
        hits.into_iter().map(|((f, p), c)| (c * per_sample_ns, f, p)).collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    rows.truncate(limit);
    rows
}

/// Bytes allocated per JS function from a `.heapprofile` (V8's sampling heap profile: a call tree
/// whose nodes carry the sampled bytes they allocated themselves), heaviest first:
/// `(bytes, function, url:line)`. Sampled, so small allocators are estimates.
pub(crate) fn top_allocators(profile: &serde_json::Value, limit: usize) -> (u64, Vec<(u64, String, String)>) {
    let mut bytes: std::collections::HashMap<(String, String), u64> = std::collections::HashMap::new();
    let mut total = 0u64;
    let mut stack: Vec<&serde_json::Value> = profile.get("head").into_iter().collect();
    while let Some(node) = stack.pop() {
        if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
            stack.extend(children.iter());
        }
        let own = node.get("selfSize").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
        if own == 0 {
            continue;
        }
        total += own;
        let frame = &node["callFrame"];
        let func = frame["functionName"].as_str().unwrap_or("");
        let func = if func.is_empty() { "(anonymous)" } else { func };
        let url = frame["url"].as_str().unwrap_or("");
        let line = frame["lineNumber"].as_i64().unwrap_or(-1) + 1;
        let place = if url.is_empty() { String::new() } else { format!("{url}:{line}") };
        *bytes.entry((func.to_string(), place)).or_default() += own;
    }
    let mut rows: Vec<(u64, String, String)> = bytes.into_iter().map(|((f, p), b)| (b, f, p)).collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    rows.truncate(limit);
    (total, rows)
}
