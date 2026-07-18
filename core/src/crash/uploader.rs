//! Upload-on-next-boot sweep. The handler side only ever WRITES files; this module (normal
//! context, shared tokio runtime) renders + uploads them with retry, marking each sent.
//! Fail-off throughout: any error leaves the file in place for the next sweep.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::crash::config::CrashConfig;
use crate::crash::spool::{self, SpoolItem};

/// Files currently being uploaded (guards the periodic sweep double-starting one file).
static INFLIGHT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static BOOT_SWEEP_DONE: AtomicBool = AtomicBool::new(false);
/// Unix seconds of the last periodic sweep (0 = never).
static LAST_SWEEP: Mutex<u64> = Mutex::new(0);
const SWEEP_INTERVAL_SECS: u64 = 300;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// D-1: random 128-bit hex persisted at <dir>/server_id; "unknown" if unreadable+unwritable.
pub fn server_id(dir: &Path) -> String {
    let path = dir.join("server_id");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let t = s.trim().to_string();
        if t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()) { return t; }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    match std::fs::write(&path, &id) {
        Ok(()) => id,
        Err(_) => "unknown".to_string(),
    }
}

/// Patch host.server_id into a spooled envelope (envelopes are spooled with it empty).
pub fn finalize(envelope_json: &str, server_id: &str) -> Option<String> {
    let mut env: crate::crash::envelope::Envelope = serde_json::from_str(envelope_json).ok()?;
    env.host.server_id = server_id.to_string();
    serde_json::to_string(&env).ok()
}

/// Boot-time sweep: triggered by the FIRST identity push (the spool dir arrives there), so it
/// runs once per process, after the ops table + http engine exist.
pub fn boot_sweep() {
    if BOOT_SWEEP_DONE.swap(true, Ordering::SeqCst) { return; }
    let Some(dir) = crate::crash::spool_dir() else { return };
    let cfg = crate::crash::config::load();
    sweep_now(&dir, &cfg);
}

/// Periodic sweep for the still-alive kinds (js/panic): every SWEEP_INTERVAL_SECS, from
/// frame_async_drain. Cheap early-outs; never blocks the frame.
pub fn periodic_sweep() {
    let now = now_secs();
    {
        let mut last = match LAST_SWEEP.lock() { Ok(g) => g, Err(p) => p.into_inner() };
        if now.saturating_sub(*last) < SWEEP_INTERVAL_SECS { return; }
        *last = now;
    }
    let Some(dir) = crate::crash::spool_dir() else { return };
    let cfg = crate::crash::config::load();
    sweep_now(&dir, &cfg);
}

/// Scan the spool and spawn one upload task per pending incident. Synchronous scan (test seam);
/// the network work runs on the shared tokio runtime (http::spawn — never a second runtime).
pub fn sweep_now(dir: &Path, cfg: &CrashConfig) {
    if !cfg.enabled { return; }
    let sid = server_id(dir);
    for item in spool::scan(dir) {
        let files: Vec<PathBuf> = match &item {
            SpoolItem::Envelope(p) => vec![p.clone()],
            SpoolItem::Native { meta, dump } => vec![dump.clone(), meta.clone()],
        };
        {
            let mut inflight = match INFLIGHT.lock() { Ok(g) => g, Err(p) => p.into_inner() };
            if files.iter().any(|f| inflight.contains(f)) { continue; }
            inflight.extend(files.iter().cloned());
        }
        let dir = dir.to_path_buf();
        let cfg = cfg.clone();
        let sid = sid.clone();
        crate::http::spawn(async move {
            let ok = upload_item(&dir, &item, &cfg, &sid).await;
            if ok { spool::mark_sent(&dir, &files); }
            let mut inflight = match INFLIGHT.lock() { Ok(g) => g, Err(p) => p.into_inner() };
            inflight.retain(|f| !files.contains(f));
        });
    }
}

/// Render (native) / finalize (envelope) + POST with 3 attempts (1s/2s/4s backoff).
async fn upload_item(dir: &Path, item: &SpoolItem, cfg: &CrashConfig, sid: &str) -> bool {
    // Build the envelope JSON + optional minidump bytes OUTSIDE the retry loop.
    let (json, dump_bytes): (String, Option<Vec<u8>>) = match item {
        SpoolItem::Envelope(p) => {
            let Ok(raw) = std::fs::read_to_string(p) else { return false };
            let Some(fin) = finalize(&raw, sid) else {
                // Unparseable spool file: consume it (move to sent) rather than retry forever.
                spool::mark_sent(dir, &[p.clone()]);
                return false;
            };
            (fin, None)
        }
        SpoolItem::Native { meta, dump } => {
            let bc = read_meta(meta); // zeroed breadcrumb when the sidecar is missing/short
            let occurred = std::fs::metadata(dump)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| crate::crash::envelope::iso8601_utc(d.as_secs() as i64));
            let dump_name = dump.file_name().and_then(|n| n.to_str()).unwrap_or("crash.dmp").to_string();
            let env = crate::crash::envelope::render(
                &bc,
                "native",
                crate::crash::envelope::Detail::Native { minidump_ref: dump_name },
                occurred,
                sid,
                &crate::crash::config::scrub(cfg),
            );
            let Ok(json) = serde_json::to_string(&env) else { return false };
            let bytes = if cfg.include_minidump { std::fs::read(dump).ok() } else { None };
            (json, bytes)
        }
    };

    let client = reqwest::Client::new();
    for attempt in 0u32..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(1 << (attempt - 1))).await;
        }
        let mut req = client
            .post(&cfg.endpoint)
            .header("authorization", format!("Bearer {}", cfg.api_key))
            .timeout(std::time::Duration::from_secs(30));
        req = match &dump_bytes {
            Some(bytes) => {
                let form = reqwest::multipart::Form::new()
                    .part("envelope", reqwest::multipart::Part::text(json.clone())
                        .mime_str("application/json").unwrap_or_else(|_| reqwest::multipart::Part::text(json.clone())))
                    .part("minidump", reqwest::multipart::Part::bytes(bytes.clone()).file_name("crash.dmp"));
                req.multipart(form)
            }
            None => req.header("content-type", "application/json").body(json.clone()),
        };
        match req.send().await {
            Ok(resp) if resp.status().is_success() => return true,
            _ => continue,
        }
    }
    false // all attempts failed: leave the file for the next sweep
}

/// Read a .s2meta sidecar back into a CrashBreadcrumb (validate size + magic; else zeroed).
fn read_meta(meta: &Path) -> crate::crash::breadcrumb::CrashBreadcrumb {
    use crate::crash::breadcrumb::{CrashBreadcrumb, BREADCRUMB_MAGIC};
    let zeroed = || unsafe { std::mem::MaybeUninit::<CrashBreadcrumb>::zeroed().assume_init() };
    let Ok(bytes) = std::fs::read(meta) else { return zeroed() };
    if bytes.len() != std::mem::size_of::<CrashBreadcrumb>() { return zeroed(); }
    let bc: CrashBreadcrumb = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const CrashBreadcrumb) };
    if bc.magic != BREADCRUMB_MAGIC { return zeroed(); }
    bc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("s2crash-up-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// One-shot mock ingest endpoint: accepts one HTTP request, returns 200, hands back the body.
    fn spawn_ingest() -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read until the socket would block long enough — headers+small body fit easily.
                s.set_read_timeout(Some(std::time::Duration::from_millis(300))).unwrap();
                while let Ok(n) = s.read(&mut chunk) {
                    if n == 0 { break; }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > 64 * 1024 { break; }
                }
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
            }
        });
        (port, rx)
    }

    #[test]
    fn server_id_is_created_once_and_stable() {
        let d = tmpdir("sid");
        let a = server_id(&d);
        let b = server_id(&d);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32, "uuid v4 hex, hyphens stripped");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn finalize_patches_server_id() {
        let json = r#"{"schema_version":1,"incident_id":"i","kind":"panic","s2script":{"version":"0.0.0","api_version":"1"},"gamedata":{"fingerprint":"","generated_at":"","hl2sdk":"","schema_build":"","stale":false},"game":{"name":"","build_number":0,"map":"","players":0,"uptime":0},"host":{"server_id":"","os":"linux"},"breadcrumb":{"plugin":"","dispatch":"","engine_op":"","js_location":"","ring":[]},"plugins":[],"detail":{"message":"m","backtrace":"b"}}"#;
        let out = finalize(json, "srv-42").unwrap();
        let env: crate::crash::envelope::Envelope = serde_json::from_str(&out).unwrap();
        assert_eq!(env.host.server_id, "srv-42");
        assert!(finalize("{ not json", "x").is_none());
    }

    #[test]
    fn sweep_uploads_envelope_and_marks_sent() {
        crate::http::init();
        let d = tmpdir("sweep");
        let (port, rx) = spawn_ingest();
        let cfg = crate::crash::config::CrashConfig {
            enabled: true,
            endpoint: format!("http://127.0.0.1:{}/ingest", port),
            api_key: "key-abc".into(),
            ..Default::default()
        };
        let bc = crate::crash::breadcrumb::snapshot();
        let env = crate::crash::envelope::render(
            &bc, "panic",
            crate::crash::envelope::Detail::Panic { message: "m".into(), backtrace: "b".into() },
            None, "", &crate::crash::envelope::Scrub { map: false, players: false });
        let json = serde_json::to_string(&env).unwrap();
        crate::crash::spool::write_incident(&d, &json).unwrap();

        sweep_now(&d, &cfg);

        // The upload runs on the shared tokio runtime; poll for the sent/ move.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if crate::crash::spool::scan(&d).is_empty() { break; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(crate::crash::spool::scan(&d).is_empty(), "incident must be marked sent");
        let body = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(body.contains("authorization: Bearer key-abc") || body.contains("Authorization: Bearer key-abc"));
        assert!(body.contains("schema_version"));
        assert!(body.contains(&server_id(&d)), "server_id patched into the uploaded envelope");
    }

    #[test]
    fn sweep_disabled_uploads_nothing() {
        crate::http::init();
        let d = tmpdir("disabled");
        crate::crash::spool::write_incident(&d, r#"{"x":1}"#).unwrap();
        let cfg = crate::crash::config::CrashConfig::default(); // enabled=false
        sweep_now(&d, &cfg);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(crate::crash::spool::scan(&d).len(), 1, "opt-out must not upload or consume");
    }
}
