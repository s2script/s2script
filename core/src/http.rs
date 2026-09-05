//! Engine-generic async HTTP: a process-global tokio runtime + a shared reqwest Client + a
//! completion channel. The engine half holds NO V8 handles; the main thread only submits (`fetch`)
//! and polls (`try_recv_completed`) — the runtime does all network I/O off-thread. Mirrors
//! async_rt's POOL: a OnceLock, built once, never dropped (survives a Metamod re-init).
//!
//! The adapter half (`__s2_fetch`) lives below. Promise resolve stays in `v8host` (`resolve_fetch`)
//! because it needs the Jobs resolver map + the host isolate.
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
    pub lease: crate::async_limits::JobLease,
    queue:crate::async_limits::QueueTicket,
}



struct Engine {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    tx: Sender<FetchCompletion>,
    rx: Mutex<Receiver<FetchCompletion>>,
}
static ENGINE: OnceLock<Engine> = OnceLock::new();

pub fn init() {
    crate::async_limits::resume_delivery();
    ENGINE.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let client = reqwest::Client::builder().build().expect("reqwest client");
        let (tx, rx) = channel();
        Engine {
            runtime,
            client,
            tx,
            rx: Mutex::new(rx),
        }
    });
}

#[cfg(test)]
pub fn fetch(id: u64, req: FetchRequest) {
    fetch_reserved(id, req, crate::async_limits::domain().job(None, 0).unwrap()).unwrap();
}
pub(crate) fn fetch_reserved(
    id: u64,
    req: FetchRequest,
    mut lease: crate::async_limits::JobLease,
) -> Result<(), String> {
    let Some(e) = ENGINE.get() else {
        return Err("HTTP not initialized".into());
    };
    let client = e.client.clone();
    let tx = e.tx.clone();
    e.runtime.spawn(async move {
        let cancel=lease.cancel.clone();
        let result=tokio::select! {biased;_=cancel.wait()=>Err("AsyncCancelled".into()),r=do_fetch(client,req,&mut lease)=>r}.map_err(crate::async_limits::diagnostic);
        if result.is_err() { lease.discard_result(); }
        let _guard=crate::async_limits::delivery_guard(&lease.cancel);
        if _guard.is_some() {let _=tx.send(FetchCompletion { id,result,lease,queue:crate::async_limits::QueueTicket::new(1) });}
    });
    Ok(())
}

pub fn try_recv_completed() -> Option<FetchCompletion> {
    let mut c = ENGINE.get()?.rx.lock().ok()?.try_recv().ok()?;
    c.queue.dequeue();
    Some(c)
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

async fn do_fetch(
    client: reqwest::Client,
    req: FetchRequest,
    lease: &mut crate::async_limits::JobLease,
) -> Result<FetchResponse, String> {
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut rb = client
        .request(method, &req.url)
        .timeout(Duration::from_millis(req.timeout_ms));
    for (k, v) in &req.headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if let Some(b) = req.body {
        rb = rb.body(b);
    }
    let mut resp = rb.send().await.map_err(|e| e.to_string())?; // network/timeout → Err
    let status = resp.status().as_u16();
    let status_text = resp.status().canonical_reason().unwrap_or("").to_string();
    let mut headers = Vec::new();
    let mut header_bytes = 0usize;
    for (k, v) in resp.headers() {
        let value = v.to_str().unwrap_or("");
        let n = k
            .as_str()
            .len()
            .saturating_add(value.len())
            .saturating_add(64);
        header_bytes = header_bytes.saturating_add(n);
        if header_bytes > lease.policy.http_body_bytes {
            return Err("HttpResponseTooLarge".into());
        }
        lease.grow(n).map_err(|e| e.to_string())?;
        headers.push((k.as_str().to_owned(), value.to_owned()));
    }
    // Fast reject on a declared oversized body...
    if let Some(len) = resp.content_length() {
        if len > lease.policy.http_body_bytes as u64 {
            return Err("HttpResponseTooLarge".into());
        }
    }
    // ...but a chunked / no-Content-Length response can lie, so STREAM the body and abort the moment
    // the accumulated size exceeds MAX_BODY — never buffer an unbounded (hostile) response into memory.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if buf.len().saturating_add(chunk.len()) > lease.policy.http_body_bytes {
            return Err("HttpResponseTooLarge".into());
        }
        lease.grow(chunk.len()).map_err(|e| e.to_string())?;
        buf.reserve_exact(chunk.len());
        buf.extend_from_slice(&chunk);
    }
    let final_bytes = crate::async_limits::utf8_lossy_len(&buf);
    if final_bytes.saturating_add(header_bytes) > lease.policy.http_body_bytes {
        return Err("HttpResponseTooLarge".into());
    }
    let body = match String::from_utf8(buf) {
        Ok(body) => body,
        Err(error) => {
            // Invalid UTF-8 requires a second buffer: account conversion scratch simultaneously.
            lease.grow(final_bytes).map_err(|e| e.to_string())?;
            String::from_utf8_lossy(error.as_bytes())
                .into_owned()
                .into_boxed_str()
                .into_string()
        }
    };
    Ok(FetchResponse {
        status,
        status_text,
        headers,
        body,
    })
}

// ---------------------------------------------------------------------------
// V8 adapter — `__s2_fetch`. The tokio+reqwest engine above holds no V8 handles.
// Promise create goes through `jobs::begin_job`; Promise resolve (`resolve_fetch`) stays in v8host.
// ---------------------------------------------------------------------------

use crate::v8host::set_native;
fn s2_fetch(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let result = (|| -> Result<(), String> {
        let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
        let url =
            crate::jobs::copy_string(scope, args.get(0), &mut lease).map_err(|e| e.to_string())?;
        let mut method = "GET".to_owned();
        let mut body = None;
        let mut headers = Vec::new();
        let mut timeout_ms = 30_000;
        if let Ok(opts) = v8::Local::<v8::Object>::try_from(args.get(1)) {
            for name in ["method", "body", "timeoutMs", "headers"] {
                let key = v8::String::new(scope, name).unwrap();
                let Some(value) = opts.get(scope, key.into()) else {
                    continue;
                };
                if value.is_null_or_undefined() {
                    continue;
                }
                match name {
                    "method" => {
                        method = crate::jobs::copy_string(scope, value, &mut lease)
                            .map_err(|e| e.to_string())?
                    }
                    "body" => {
                        body = Some(
                            crate::jobs::copy_string(scope, value, &mut lease)
                                .map_err(|e| e.to_string())?,
                        )
                    }
                    "timeoutMs" => {
                        timeout_ms = value.integer_value(scope).unwrap_or(30_000).max(0) as u64
                    }
                    _ => {
                        if let Ok(ho) = v8::Local::<v8::Object>::try_from(value) {
                            if let Some(names) =
                                ho.get_own_property_names(scope, Default::default())
                            {
                                // Names array is V8-owned; native copies are charged individually before allocation.
                                for i in 0..names.length() {
                                    let Some(k) = names.get_index(scope, i) else {
                                        continue;
                                    };
                                    let Some(v) = ho.get(scope, k) else { continue };
                                    let k = crate::jobs::copy_string(scope, k, &mut lease)
                                        .map_err(|e| e.to_string())?;
                                    let v = crate::jobs::copy_string(scope, v, &mut lease)
                                        .map_err(|e| e.to_string())?;
                                    headers.push((k, v));
                                }
                            }
                        }
                    }
                }
            }
        }
        crate::jobs::check_live(&lease)?;
        let id = crate::jobs::next_id();
        let cancel = lease.cancel.clone();
        fetch_reserved(
            id,
            FetchRequest {
                method,
                url,
                headers,
                body,
                timeout_ms,
            },
            lease,
        )?;
        crate::jobs::commit_reserved(scope, id, resolver, cancel);
        Ok(())
    })();
    if let Err(e) = result {
        crate::jobs::reject(scope, resolver, &e);
    }
    rv.set(promise.into());
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_fetch", s2_fetch);
}

#[cfg(test)]
mod tests {
    fn response_bytes(response: Vec<u8>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut req = [0; 2048];
            let _ = std::io::Read::read(&mut socket, &mut req);
            std::io::Write::write_all(&mut socket, &response).unwrap();
        });
        port
    }
    fn fetch_tiny(response: Vec<u8>, max: usize) -> Result<FetchResponse, String> {
        let port = response_bytes(response);
        let d = crate::async_limits::Domain::new(crate::async_limits::AsyncPolicy {
            http_body_bytes: max,
            completion_bytes: 4096,
            failure_bytes: 64,
            ..Default::default()
        });
        let mut lease = d.job(None, 0).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(do_fetch(
            reqwest::Client::new(),
            FetchRequest {
                method: "GET".into(),
                url: format!("http://127.0.0.1:{port}/"),
                headers: vec![],
                body: None,
                timeout_ms: 1000,
            },
            &mut lease,
        ))
    }
    #[test]
    fn http_final_utf8_exact_boundary_and_declared_or_chunked_overflow() {
        let mut invalid = b"HTTP/1.0 200 OK\r\n\r\n".to_vec();
        invalid.extend_from_slice(&[0xff; 3]);
        assert_eq!(fetch_tiny(invalid.clone(), 9).unwrap().body.len(), 9);
        assert_eq!(
            fetch_tiny(invalid, 8).err().unwrap(),
            "HttpResponseTooLarge"
        );
        assert_eq!(
            fetch_tiny(
                b"HTTP/1.1 200 OK\r\nContent-Length: 129\r\n\r\n".to_vec(),
                128
            )
            .err()
            .unwrap(),
            "HttpResponseTooLarge"
        );
        let mut chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n81\r\n".to_vec();
        chunked.extend_from_slice(&[b'x'; 129]);
        chunked.extend_from_slice(b"\r\n0\r\n\r\n");
        assert_eq!(
            fetch_tiny(chunked, 128).err().unwrap(),
            "HttpResponseTooLarge"
        );
        assert_eq!(
            fetch_tiny(
                b"HTTP/1.0 200 OK\r\nVery-Long-Header-Name: value\r\n\r\n".to_vec(),
                8
            )
            .err()
            .unwrap(),
            "HttpResponseTooLarge"
        );
        assert_eq!(
            fetch_tiny(b"HTTP/1.0 200 OK\r\n\r\nok".to_vec(), 8)
                .unwrap()
                .body,
            "ok"
        );
    }

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
