//! Engine-generic async HTTP engine: a process-global tokio runtime + a shared reqwest Client + a
//! completion channel. Holds NO V8 handles; the main thread only submits (`fetch`) and polls
//! (`try_recv_completed`) — the runtime does all network I/O off-thread. Mirrors async_rt's POOL:
//! a OnceLock, built once, never dropped (survives a Metamod re-init).
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
