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
    /// JS-initiated close (`__s2_ws_close` -> `ws::close`, owner-checked): the task emits its own
    /// `Closed` WsSignal so `onClose` fires and the drain's Closed-routing deregisters the conn
    /// (see the signal-routing step in v8host.rs's `frame_async_drain`).
    Close,
    /// Ledger-teardown close (`ws::drop_conn`, unconditional — plugin unload / process shutdown):
    /// closes the socket WITHOUT emitting a signal. The registry entry is already removed
    /// synchronously by `drop_conn` before this is even sent, and the owning plugin's WS_EVENT_MUX
    /// subscribers are torn down in the same teardown pass — nothing is left to route a signal to.
    /// Kept distinct from `Close` so a late-arriving teardown signal can never be misrouted onto an
    /// unrelated LATER connection that happens to reuse this same numeric conn id.
    Shutdown,
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
                    Some(Err(err)) => {
                        // A mid-stream read error is terminal. Emit BOTH signals, browser-parity:
                        // `onError(err)` fires, then `onClose(1006, ...)` — 1006 = Abnormal Closure
                        // (no clean close frame). The following Closed is what makes the drain call
                        // `drop_conn` (the Errored arm alone does not) and prune the conn's mux keys,
                        // so the error path cleans up exactly like every other terminal path.
                        let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Errored(err.to_string()) });
                        let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Closed(1006, "connection error".into()) });
                        break;
                    }
                    None => { let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Closed(1006, "stream ended".into()) }); break; }
                },
                cmd = cmd_rx.recv() => match cmd {
                    Some(WsCommand::Send(t)) => { if write.send(Message::text(t)).await.is_err() { break; } }
                    Some(WsCommand::Close) => {
                        // Self-initiated close (JS called ws.close()). We don't block waiting on the
                        // peer's close-frame acknowledgment (the peer may never send one) — emit our
                        // own Closed signal (1000 = Normal Closure, per RFC 6455) so the drain's
                        // Closed routing fires onClose AND ws::drop_conn deregisters this conn_id
                        // from the registry, exactly like a peer-initiated close already does above.
                        let _ = write.send(Message::Close(None)).await;
                        let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Closed(1000, String::new()) });
                        break;
                    }
                    Some(WsCommand::Shutdown) | None => {
                        // Ledger-teardown close (plugin unload / shutdown) or the sender vanished
                        // unexpectedly: close the socket but emit NO signal. `drop_conn` already
                        // removed the registry entry synchronously before sending this, and the
                        // owner's WS_EVENT_MUX subs are torn down in the same pass, so there is
                        // nothing left to route a signal to (and, unlike `Close`, never risking a
                        // late signal landing on a future connection that reuses this conn id).
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
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
        // Report the actual send outcome (mirrors `send`): `false` if the task is already gone,
        // rather than an unconditional `true` that would mislead a future caller.
        Some(c) if c.owner == owner => c.cmd_tx.send(WsCommand::Close).is_ok(),
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
/// Sends `Shutdown` (not `Close`): this path never emits a `WsSignal::Closed` (see `WsCommand`'s
/// doc) since there is no live owner left to route a signal to, and doing so would risk a
/// late-arriving signal misrouting onto a future, unrelated connection that reuses this conn id.
pub fn drop_conn(conn_id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&conn_id) {
        let _ = c.cmd_tx.send(WsCommand::Shutdown);
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
        // Drive the full signal flow the design doc calls for: Connected -> Message -> Closed.
        // On the echo, self-initiate a close and verify it actually produces a Closed signal
        // (the regression this test used to miss: it called close() with no follow-up assertion).
        let mut got_connected = false;
        let mut echo = None;
        let mut closed = None;
        for _ in 0..500 {
            while let Some(s) = try_recv_signal() {
                match s.kind {
                    WsSignalKind::Connected => {
                        got_connected = true;
                        send(1, "p", "hi".into());
                    }
                    WsSignalKind::Message(t) => {
                        echo = Some(t);
                        close(1, "p");
                    }
                    WsSignalKind::Closed(code, reason) => {
                        closed = Some((code, reason));
                    }
                    _ => {}
                }
            }
            if closed.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(got_connected);
        assert_eq!(echo.as_deref(), Some("hi"));
        // A self-initiated close signals Closed(1000, "") (RFC 6455 Normal Closure) even though
        // the peer never echoes a close frame back (this test's echo_server_port helper just
        // drops the connection on receiving a close, per its `m.is_close() => break`).
        assert_eq!(closed, Some((1000, String::new())));
        // The Closed signal is what drives ws::drop_conn in the real drain (v8host.rs); here we
        // call it directly to verify a self-close leaves no leaked registry entry.
        drop_conn(1);
        assert!(!is_owner(1, "p"));
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
