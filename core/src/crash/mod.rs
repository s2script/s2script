//! Crash-reporter capture client (engine-generic). Sub-project 1 of the crash-reporter spec.
//! No V8 types cross into this module; no game names ever appear here.
pub mod breadcrumb;
pub mod config;
pub mod dedup;
pub mod envelope;
pub mod panic_hook;
pub mod spool;
pub mod uploader;

use std::path::PathBuf;
use std::sync::Mutex;

static SPOOL_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Record the crash-spool directory (pushed by the shim with the identity block). Empty → None.
pub fn set_spool_dir(path: &str) {
    let mut g = match SPOOL_DIR.lock() { Ok(g) => g, Err(p) => p.into_inner() };
    *g = if path.is_empty() { None } else { Some(PathBuf::from(path)) };
}

pub fn spool_dir() -> Option<PathBuf> {
    match SPOOL_DIR.lock() { Ok(g) => g.clone(), Err(p) => p.into_inner().clone() }
}

/// Best-effort "file:line" from a V8 stack's first frame:
///   "    at fn (plugin.js:12:5)"  /  "    at plugin.js:12:5"
pub(crate) fn parse_top_frame(stack: &str) -> (String, u32) {
    for line in stack.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("at ") else { continue };
        let loc = rest.rsplit_once('(').map(|(_, l)| l.trim_end_matches(')')).unwrap_or(rest);
        // loc = "file:line:col" — split from the right.
        let mut it = loc.rsplitn(3, ':');
        let _col = it.next();
        let line_no = it.next().and_then(|l| l.parse::<u32>().ok()).unwrap_or(0);
        let file = it.next().unwrap_or("").to_string();
        if !file.is_empty() { return (file, line_no); }
    }
    (String::new(), 0)
}

use std::sync::Mutex as StdMutex;
static JS_LIMITER: StdMutex<Option<dedup::RateLimiter>> = StdMutex::new(None);

/// The fatal-JS capture entry (D-2): dedup by (plugin, message, top frame), then envelope
/// (kind=js) → spool. Called from the per-handler TryCatch sites + the promise-reject drain.
pub fn report_js_error(plugin: &str, dispatch: &str, message: &str, stack: &str) {
    let Some(dir) = spool_dir() else { return };
    let (file, line) = parse_top_frame(stack);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sig = dedup::fnv1a64(&[plugin, message, &format!("{}:{}", file, line)]);
    {
        let mut g = match JS_LIMITER.lock() { Ok(g) => g, Err(p) => p.into_inner() };
        let rl = g.get_or_insert_with(dedup::RateLimiter::new);
        if !rl.should_report(sig, now) { return; }
    }
    let mut bc = breadcrumb::snapshot();
    // The current stamp may already have unwound (guard dropped) — restamp the culprit.
    breadcrumb::copy_cstr(&mut bc.plugin, plugin);
    breadcrumb::copy_cstr(&mut bc.dispatch, dispatch);
    let cfg = config::load();
    let env = envelope::render(
        &bc,
        "js",
        envelope::Detail::Js {
            stack: stack.to_string(),
            message: message.to_string(),
            file,
            line,
        },
        Some(envelope::iso8601_utc(now as i64)),
        "", // patched by the uploader (D-1)
        &config::scrub(&cfg),
    );
    if let Ok(json) = serde_json::to_string(&env) {
        let _ = spool::write_incident(&dir, &json);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_top_frame_variants() {
        assert_eq!(
            super::parse_top_frame("Error: x\n    at doThing (myplugin.js:12:5)\n    at top (a.js:1:1)"),
            ("myplugin.js".to_string(), 12)
        );
        assert_eq!(
            super::parse_top_frame("Error: x\n    at myplugin.js:7:3"),
            ("myplugin.js".to_string(), 7)
        );
        assert_eq!(super::parse_top_frame("no frames here"), (String::new(), 0));
    }
}
