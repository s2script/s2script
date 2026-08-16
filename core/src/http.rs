//! Engine-generic async HTTP: a process-global tokio runtime + a shared reqwest Client + a
//! completion channel. The engine half holds NO V8 handles; the main thread only submits (`fetch`)
//! and polls (`try_recv_completed`) — the runtime does all network I/O off-thread. Mirrors
//! async_rt's POOL: a OnceLock, built once, never dropped (survives a Metamod re-init).
//!
//! The adapter half (`__s2_fetch`) lives below. Promise resolve stays in `v8host` (`resolve_fetch`)
//! because it needs RESOLVERS + the host isolate.
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub struct FetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout_ms: u64,
}
pub struct FetchResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
pub struct FetchCompletion {
    pub id: u64,
    pub result: Result<FetchResponse, String>,
}

const MAX_BODY: usize = 10 * 1024 * 1024; // 10 MB cap

struct Engine {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    tx: Sender<FetchCompletion>,
    rx: Mutex<Receiver<FetchCompletion>>,
}
static ENGINE: OnceLock<Engine> = OnceLock::new();

pub fn init() {
    ENGINE.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let client = reqwest::Client::builder().build().expect("reqwest client");
        let (tx, rx) = channel();
        Engine { runtime, client, tx, rx: Mutex::new(rx) }
    });
}

pub fn fetch(id: u64, req: FetchRequest) {
    let Some(e) = ENGINE.get() else { return }; // degrade: not initialized
    let client = e.client.clone();
    let tx = e.tx.clone();
    e.runtime.spawn(async move {
        let result = do_fetch(client, req).await;
        let _ = tx.send(FetchCompletion { id, result });
    });
}

pub fn try_recv_completed() -> Option<FetchCompletion> {
    ENGINE.get()?.rx.lock().ok()?.try_recv().ok()
}

/// Spawn a future on the shared tokio runtime (used by ws.rs to reuse the one runtime). No-op if
/// the engine hasn't been initialized yet (degrade, never panic).
pub fn spawn<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Some(e) = ENGINE.get() {
        e.runtime.spawn(future);
    }
}

/// Enter the shared runtime's context (an RAII guard) so runtime-requiring constructors (e.g. a sqlx
/// `connect_lazy_with`) can be called from the main thread without blocking. None if uninitialized.
pub fn enter() -> Option<tokio::runtime::EnterGuard<'static>> {
    ENGINE.get().map(|e| e.runtime.enter())
}

async fn do_fetch(client: reqwest::Client, req: FetchRequest) -> Result<FetchResponse, String> {
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut rb = client.request(method, &req.url).timeout(Duration::from_millis(req.timeout_ms));
    for (k, v) in &req.headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if let Some(b) = req.body {
        rb = rb.body(b);
    }
    let mut resp = rb.send().await.map_err(|e| e.to_string())?; // network/timeout → Err
    let status = resp.status().as_u16();
    let status_text = resp.status().canonical_reason().unwrap_or("").to_string();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Fast reject on a declared oversized body...
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BODY {
            return Err("response body too large".into());
        }
    }
    // ...but a chunked / no-Content-Length response can lie, so STREAM the body and abort the moment
    // the accumulated size exceeds MAX_BODY — never buffer an unbounded (hostile) response into memory.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if buf.len() + chunk.len() > MAX_BODY {
            return Err("response body too large".into());
        }
        buf.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&buf).into_owned();
    Ok(FetchResponse { status, status_text, headers, body })
}

// ---------------------------------------------------------------------------
// V8 adapter — `__s2_fetch`. The tokio+reqwest engine above holds no V8 handles.
// Promise create goes through `begin_job_promise` so RESOLVERS stay in v8host;
// Promise resolve (`resolve_fetch`) stays there too.
// ---------------------------------------------------------------------------

use crate::v8host::{begin_job_promise, set_native};

fn get_str_prop(scope: &mut v8::PinScope, obj: v8::Local<v8::Object>, name: &str) -> Option<String> {
    let key = v8::String::new(scope, name)?;
    let val = obj.get(scope, key.into())?;
    if val.is_null_or_undefined() {
        return None;
    }
    Some(val.to_rust_string_lossy(scope))
}

/// Native `__s2_fetch(url, options) -> Promise<rawResponse>` where `rawResponse =
/// {status, ok, statusText, headers, body}`. MIRRORS `s2_thread_sleep`'s resolver/ledger/pending
/// block (via `begin_job_promise` — a `Job` resource) but hands off to `fetch` so the calling
/// (main/game) thread never blocks on I/O. The Promise resolves on a LATER `frame_async_drain`
/// via `v8host::resolve_fetch`.
///
/// `options` (all optional): `method` (default `"GET"`), `headers` (a plain string→string object),
/// `body` (a string), `timeoutMs` (default 30000). Degrade-never-crash: the whole body runs under
/// `catch_unwind`; a malformed/absent `options` degrades to the defaults (never throws
/// synchronously) — the actual network outcome (incl. a 4xx/5xx, which RESOLVES with `ok:false`,
/// vs. a network/timeout error, which REJECTS) is decided later by `resolve_fetch`.
fn s2_fetch(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let url = args.get(0).to_rust_string_lossy(scope);
        let mut method = "GET".to_string();
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut body: Option<String> = None;
        let mut timeout_ms = 30_000u64;
        if let Ok(opts) = v8::Local::<v8::Object>::try_from(args.get(1)) {
            if let Some(v) = get_str_prop(scope, opts, "method") {
                method = v;
            }
            if let Some(v) = get_str_prop(scope, opts, "body") {
                body = Some(v);
            }
            if let Some(k) = v8::String::new(scope, "timeoutMs") {
                if let Some(v) = opts.get(scope, k.into()) {
                    if v.is_number() {
                        timeout_ms = v.integer_value(scope).unwrap_or(30_000).max(0) as u64;
                    }
                }
            }
            if let Some(k) = v8::String::new(scope, "headers") {
                if let Some(hv) = opts.get(scope, k.into()) {
                    if let Ok(ho) = v8::Local::<v8::Object>::try_from(hv) {
                        if let Some(names) = ho.get_own_property_names(scope, Default::default()) {
                            for i in 0..names.length() {
                                let Some(key) = names.get_index(scope, i) else { continue };
                                let Some(val) = ho.get(scope, key) else { continue };
                                headers.push((
                                    key.to_rust_string_lossy(scope),
                                    val.to_rust_string_lossy(scope),
                                ));
                            }
                        }
                    }
                }
            }
        }
        let (id, promise) = begin_job_promise(scope);
        fetch(id, FetchRequest { method, url, headers, body, timeout_ms });
        rv.set(promise);
    }));
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_fetch", s2_fetch);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    // A tiny local HTTP/1.1 server on an ephemeral port; returns one canned response then exits.
    fn spawn_server(response: &'static str) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(response.as_bytes());
            }
        });
        port
    }
    fn drain_blocking(id: u64) -> FetchCompletion {
        for _ in 0..500 {
            if let Some(c) = try_recv_completed() {
                if c.id == id {
                    return c;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no completion");
    }
    #[test]
    fn fetch_local_server_ok() {
        init();
        let port = spawn_server("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        fetch(
            1,
            FetchRequest {
                method: "GET".into(),
                url: format!("http://127.0.0.1:{port}/"),
                headers: vec![],
                body: None,
                timeout_ms: 5000,
            },
        );
        let c = drain_blocking(1);
        let r = c.result.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "hello");
    }
    #[test]
    fn fetch_404_resolves_not_rejects() {
        init();
        let port = spawn_server("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        fetch(
            2,
            FetchRequest {
                method: "GET".into(),
                url: format!("http://127.0.0.1:{port}/"),
                headers: vec![],
                body: None,
                timeout_ms: 5000,
            },
        );
        let r = drain_blocking(2).result.unwrap(); // Ok, not Err
        assert_eq!(r.status, 404);
    }
    #[test]
    fn fetch_bad_host_rejects() {
        init();
        fetch(
            3,
            FetchRequest {
                method: "GET".into(),
                url: "http://127.0.0.1:1/".into(),
                headers: vec![],
                body: None,
                timeout_ms: 1000,
            },
        );
        assert!(drain_blocking(3).result.is_err()); // connection refused / timeout → Err
    }
}
