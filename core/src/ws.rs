//! Engine-generic WebSocket client engine. Per connection, a tokio task (on the SHARED http runtime)
//! connects + select!s read/write and emits WsSignals down a channel the frame drain polls. Holds NO
//! V8 handles. Registry maps a conn id -> the outgoing command sender + the owning plugin.
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use tokio_tungstenite::tungstenite::Message;

pub enum WsSignalKind {
    Connected,
    ConnectFailed(String),
    Message(String),
    Closed(u16, String),
    Errored(String),
}
pub struct WsSignal {
    pub conn_id: u64,
    pub kind: WsSignalKind,
}
enum WsCommand {
    Send(String),
    Close,
}

struct Conn {
    cmd_tx: tokio::sync::mpsc::UnboundedSender<WsCommand>,
    owner: String,
}
struct Engine {
    sig_tx: Sender<WsSignal>,
    sig_rx: Mutex<Receiver<WsSignal>>,
    conns: Mutex<HashMap<u64, Conn>>,
}
static ENGINE: OnceLock<Engine> = OnceLock::new();
fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let (sig_tx, sig_rx) = channel();
        Engine {
            sig_tx,
            sig_rx: Mutex::new(sig_rx),
            conns: Mutex::new(HashMap::new()),
        }
    })
}

pub fn connect(conn_id: u64, url: String, owner: String) {
    let e = engine();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<WsCommand>();
    e.conns.lock().unwrap().insert(conn_id, Conn { cmd_tx, owner });
    let sig_tx = e.sig_tx.clone();
    crate::http::spawn(async move {
        let stream = match tokio_tungstenite::connect_async(&url).await {
            Ok((s, _resp)) => s,
            Err(err) => {
                let _ = sig_tx.send(WsSignal {
                    conn_id,
                    kind: WsSignalKind::ConnectFailed(err.to_string()),
                });
                return;
            }
        };
        let _ = sig_tx.send(WsSignal {
            conn_id,
            kind: WsSignalKind::Connected,
        });
        let (mut write, mut read) = stream.split();
        loop {
            tokio::select! {
                incoming = read.next() => match incoming {
                    Some(Ok(Message::Text(t))) => { let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Message(t.to_string()) }); }
                    Some(Ok(Message::Binary(_))) => { /* binary deferred — ignore */ }
                    Some(Ok(Message::Close(cf))) => {
                        let (code, reason) = cf.map(|c| (u16::from(c.code), c.reason.to_string())).unwrap_or((1005, String::new()));
                        let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Closed(code, reason) }); break;
                    }
                    Some(Ok(_)) => { /* Ping/Pong handled by tungstenite */ }
                    Some(Err(err)) => { let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Errored(err.to_string()) }); break; }
                    None => { let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Closed(1006, "stream ended".into()) }); break; }
                },
                cmd = cmd_rx.recv() => match cmd {
                    Some(WsCommand::Send(t)) => { if write.send(Message::text(t)).await.is_err() { break; } }
                    Some(WsCommand::Close) | None => { let _ = write.send(Message::Close(None)).await; break; }
                }
            }
        }
    });
}

pub fn send(conn_id: u64, owner: &str, text: String) -> bool {
    let e = engine();
    let map = e.conns.lock().unwrap();
    match map.get(&conn_id) {
        Some(c) if c.owner == owner => c.cmd_tx.send(WsCommand::Send(text)).is_ok(),
        _ => false,
    }
}
pub fn close(conn_id: u64, owner: &str) -> bool {
    let e = engine();
    let map = e.conns.lock().unwrap();
    match map.get(&conn_id) {
        Some(c) if c.owner == owner => {
            let _ = c.cmd_tx.send(WsCommand::Close);
            true
        }
        _ => false,
    }
}
/// Ownership check for `__s2_ws_on` (mirrors the `owner == owner` guard baked into `send`/`close`) —
/// a subscribe attempt on a conn this plugin doesn't own must no-op, exactly like a send/close would.
pub fn is_owner(conn_id: u64, owner: &str) -> bool {
    let e = engine();
    let map = e.conns.lock().unwrap();
    matches!(map.get(&conn_id), Some(c) if c.owner == owner)
}
/// Teardown / post-close deregister — closes regardless of owner (the ledger owns the id).
pub fn drop_conn(conn_id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&conn_id) {
        let _ = c.cmd_tx.send(WsCommand::Close);
    }
}
pub fn try_recv_signal() -> Option<WsSignal> {
    engine().sig_rx.lock().ok()?.try_recv().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Uses a local echo server on the http runtime. Requires http::init() for the shared runtime.
    fn echo_server_port() -> u16 {
        crate::http::init();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let std_listener = listener;
        crate::http::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    let (mut w, mut r) = ws.split();
                    while let Some(Ok(m)) = r.next().await {
                        if m.is_close() {
                            break;
                        }
                        if w.send(m).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        port
    }
    fn drain_for(kinds: usize) -> Vec<WsSignal> {
        let mut out = Vec::new();
        for _ in 0..500 {
            while let Some(s) = try_recv_signal() {
                out.push(s);
            }
            if out.len() >= kinds {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        out
    }
    #[test]
    fn connect_send_echo_close() {
        let port = echo_server_port();
        connect(1, format!("ws://127.0.0.1:{port}/"), "p".into());
        // wait for Connected, then send, then expect the echo
        let mut got_connected = false;
        let mut echo = None;
        for _ in 0..500 {
            while let Some(s) = try_recv_signal() {
                match s.kind {
                    WsSignalKind::Connected => {
                        got_connected = true;
                        send(1, "p", "hi".into());
                    }
                    WsSignalKind::Message(t) => {
                        echo = Some(t);
                    }
                    _ => {}
                }
            }
            if echo.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(got_connected);
        assert_eq!(echo.as_deref(), Some("hi"));
        close(1, "p");
    }
    #[test]
    fn connect_bad_port_fails() {
        crate::http::init();
        connect(2, "ws://127.0.0.1:1/".into(), "p".into());
        let sigs = drain_for(1);
        assert!(sigs.iter().any(|s| matches!(s.kind, WsSignalKind::ConnectFailed(_))));
    }
    #[test]
    fn send_wrong_owner_denied() {
        let port = echo_server_port();
        connect(3, format!("ws://127.0.0.1:{port}/"), "pA".into());
        // wait for connect
        for _ in 0..200 {
            if try_recv_signal().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!send(3, "pB", "x".into())); // wrong owner
        close(3, "pA");
    }
}
