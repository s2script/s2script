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
    /// `Closed` WsSignal so `onClose` fires and `poll_signals` records a deferred drop
    /// (`frame_async_drain` calls `drop_conn` after the microtask checkpoint).
    Close,
    /// Ledger-teardown close (`ws::drop_conn`, unconditional — plugin unload / process shutdown):
    /// closes the socket WITHOUT emitting a signal. The registry entry is already removed
    /// synchronously by `drop_conn` before this is even sent, and the owning plugin's WS_EVENT_MUX
    /// subscribers are torn down in the same teardown pass — nothing is left to route a signal to.
    /// Kept distinct from `Close` so a late-arriving teardown signal can never be misrouted onto an
    /// unrelated LATER connection that happens to reuse this same numeric conn id.
    Shutdown,
}

/// How long the WebSocket handshake may take before `connect` rejects.
///
/// There is no such thing as "no timeout" here — there is only a timeout the plugin cannot see. A
/// peer that accepts the TCP connection and then never speaks HTTP leaves `connect_async` parked
/// forever, so the choice is between a named rejection and a Promise that never settles.
///
/// 10s is generous for a handshake (the TCP connect and one HTTP round trip) while still being far
/// inside any human's patience for "did this work?".
///
/// Mutable ONLY so the timeout path itself can be tested in reasonable time — an untested timeout is
/// a timeout that fires wrong the first time it matters. Never changed in a shipped process.
static CONNECT_TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10_000);

fn connect_timeout() -> std::time::Duration {
    std::time::Duration::from_millis(CONNECT_TIMEOUT_MS.load(std::sync::atomic::Ordering::Relaxed))
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

/// Headers the handshake owns. A plugin that sets one of these does not get a
/// custom connection — it gets a corrupt or spoofed one, so they are refused by
/// name rather than passed through and left to fail somewhere less obvious.
///
/// `Host` is included deliberately: tungstenite derives it from the URL, and
/// letting a plugin override it is request smuggling against whatever sits in
/// front of the target.
const RESERVED_HEADERS: &[&str] = &[
    "host",
    "connection",
    "upgrade",
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-accept",
    "sec-websocket-extensions",
    "sec-websocket-protocol",
    "content-length",
    "transfer-encoding",
];

/// Build the handshake request, applying caller headers over the derived one.
///
/// Returns the rejection reason as `Err` so the caller can surface it through
/// the same `ConnectFailed` path a bad URL takes — a plugin should not have to
/// distinguish "your header was refused" from "the socket did not open" by
/// where the failure arrived.
fn build_request(
    url: &str,
    headers: &[(String, String)],
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};

    let mut req = url
        .into_client_request()
        .map_err(|e| format!("invalid websocket url: {e}"))?;

    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if RESERVED_HEADERS.contains(&lower.as_str()) {
            return Err(format!("header '{name}' is reserved by the websocket handshake"));
        }
        let hname = HeaderName::from_bytes(lower.as_bytes())
            .map_err(|_| format!("invalid header name: '{name}'"))?;
        // Rejects control characters and newlines, which is what stops a value
        // from injecting additional headers into the request.
        let hvalue = HeaderValue::from_str(value)
            .map_err(|_| format!("invalid value for header '{name}'"))?;
        req.headers_mut().insert(hname, hvalue);
    }

    Ok(req)
}

pub fn connect(conn_id: u64, url: String, owner: String, headers: Vec<(String, String)>) {
    let e = engine();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<WsCommand>();
    e.conns.lock().unwrap().insert(conn_id, Conn { cmd_tx, owner });
    let sig_tx = e.sig_tx.clone();
    crate::http::spawn(async move {
        let request = match build_request(&url, &headers) {
            Ok(r) => r,
            Err(reason) => {
                let _ = sig_tx.send(WsSignal {
                    conn_id,
                    kind: WsSignalKind::ConnectFailed(reason),
                });
                return;
            }
        };
        // BOUNDED. `connect_async` waits forever if the peer completes the TCP handshake and then
        // never finishes the WebSocket one — it accepts the socket and goes quiet. Unbounded, that is
        // not a slow connect, it is a permanent one: the connect Promise NEVER settles (neither
        // `.then` nor `.catch` runs), `PENDING_JOBS` never decrements so the frame detour stays armed
        // forever, and the plugin has no way to recover or even observe it.
        //
        // This was found the hard way: it is what made the ws tests hang in CI. A test that should
        // finish in ~40ms burned a 30s poll budget and still reported its result as literally
        // "pending", which is only reachable if the promise never settled at all.
        //
        // A timeout does not paper over the underlying stall — it converts an invisible hang into the
        // named rejection the plugin already knows how to handle, through the SAME ConnectFailed path
        // a bad URL takes.
        let budget = connect_timeout();
        let stream = match tokio::time::timeout(budget, tokio_tungstenite::connect_async(request)).await {
            Ok(Ok((s, _resp))) => s,
            Ok(Err(err)) => {
                let _ = sig_tx.send(WsSignal {
                    conn_id,
                    kind: WsSignalKind::ConnectFailed(err.to_string()),
                });
                return;
            }
            Err(_elapsed) => {
                let _ = sig_tx.send(WsSignal {
                    conn_id,
                    kind: WsSignalKind::ConnectFailed(format!(
                        "handshake did not complete within {}ms",
                        budget.as_millis()
                    )),
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

// ---------------------------------------------------------------------------
// V8 adapter — natives, mux, pending queue, post-drain dispatch, teardown.
// The tokio engine above holds no V8 handles. This half owns the isolate-facing
// surface and routes through `fan_out` so the host isolate stays in `v8host`.
// ---------------------------------------------------------------------------

use crate::dispatch::{fan_out, Instrument};
use crate::v8host::{current_plugin, log_warn, set_native, subscribe_into};

thread_local! {
    static WS_EVENT_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    static WS_EVENT_PENDING: std::cell::RefCell<Vec<(u64, String, String, i32)>>
        = std::cell::RefCell::new(Vec::new());
}

/// Queue a post-drain fan-out. Called from `poll_signals` while HOST is borrowed.
fn queue_event(conn_id: u64, event: &str, s: String, n: i32) {
    WS_EVENT_PENDING.with(|q| q.borrow_mut().push((conn_id, event.to_string(), s, n)));
}

/// One drain's worth of ws signals. Connect results go back to the host so it can resolve the
/// connect Promise (needs the Jobs resolver map). Events are queued here. Terminal drops are
/// returned so the host can `drop_conn` AFTER the microtask checkpoint.
pub(crate) struct SignalPoll {
    pub connects: Vec<(u64, Result<(), String>)>,
    pub drops: Vec<u64>,
}

/// Drain the engine channel. The tick only polls — it does not match Message/Errored/Closed.
pub(crate) fn poll_signals() -> SignalPoll {
    let mut connects = Vec::new();
    let mut drops = Vec::new();
    while let Some(sig) = try_recv_signal() {
        match sig.kind {
            WsSignalKind::Connected => connects.push((sig.conn_id, Ok(()))),
            WsSignalKind::ConnectFailed(e) => {
                connects.push((sig.conn_id, Err(e)));
                drops.push(sig.conn_id);
            }
            WsSignalKind::Message(t) => queue_event(sig.conn_id, "message", t, 0),
            WsSignalKind::Errored(e) => queue_event(sig.conn_id, "error", e, 0),
            WsSignalKind::Closed(code, reason) => {
                queue_event(sig.conn_id, "close", reason, code as i32);
                drops.push(sig.conn_id);
            }
        }
    }
    SignalPoll { connects, drops }
}

fn ws_owner(scope: &mut v8::PinScope) -> String {
    current_plugin(scope).unwrap_or_default()
}

fn s2_ws_send(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let text = args.get(1).to_rust_string_lossy(scope);
        let owner = ws_owner(scope);
        if !send(id, &owner, text) {
            log_warn(&format!(
                "WARN: __s2_ws_send: '{owner}' does not own ws conn {id} (or it is already closed) \
                 — the message was NOT sent"
            ));
        }
    }));
}

fn s2_ws_close(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let owner = ws_owner(scope);
        if !close(id, &owner) {
            log_warn(&format!(
                "WARN: __s2_ws_close: '{owner}' does not own ws conn {id} (or it is already closed) \
                 — no close was sent"
            ));
        }
    }));
}

fn s2_ws_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let event = args.get(1).to_rust_string_lossy(scope);
        let owner = ws_owner(scope);
        if !is_owner(id, &owner) {
            log_warn(&format!(
                "WARN: __s2_ws_on: '{owner}' does not own ws conn {id} — '{event}' handler NOT \
                 subscribed and will never fire"
            ));
            return;
        }
        let key = format!("{id}:{event}");
        let _ = subscribe_into(scope, &args, &WS_EVENT_MUX, &key, 2);
    }));
}

/// Drain queued events after `frame_async_drain` (HOST free). Uses `fan_out` so the isolate
/// stays in the host. Terminal `close` prunes every subscriber key for that conn.
pub(crate) fn dispatch_pending_events() {
    let pending: Vec<(u64, String, String, i32)> =
        WS_EVENT_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() { return; }

    for (conn_id, event, s, n) in pending {
        let key = format!("{conn_id}:{event}");
        let snap = WS_EVENT_MUX.with(|m| m.borrow().snapshot(&key));
        if !snap.is_empty() {
            let _ = fan_out(&snap, &format!("dispatch_pending_ws_events('{key}')"), Instrument::none(), |tc| {
                if event == "close" {
                    let code_val: v8::Local<v8::Value> = v8::Number::new(tc, n as f64).into();
                    let reason_val: v8::Local<v8::Value> =
                        v8::String::new(tc, &s).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                    Some(vec![code_val, reason_val])
                } else {
                    let s_val: v8::Local<v8::Value> =
                        v8::String::new(tc, &s).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                    Some(vec![s_val])
                }
            });
        }
        if event == "close" {
            WS_EVENT_MUX.with(|m| {
                let mut mux = m.borrow_mut();
                for ev in ["message", "close", "error"] {
                    mux.remove_by_name(&format!("{conn_id}:{ev}"));
                }
            });
        }
    }
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_ws_send", s2_ws_send);
    set_native(scope, global_obj, "__s2_ws_close", s2_ws_close);
    set_native(scope, global_obj, "__s2_ws_on", s2_ws_on);
}

pub(crate) fn register_store() {
    crate::owner_stores::register(
        "WS_EVENT_MUX",
        Box::new(|owner| { WS_EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { WS_EVENT_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            WS_EVENT_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );
}

pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    crate::process_singletons::register(
        "WS_EVENT_PENDING", AfterIsolateDrop,
        Box::new(|| WS_EVENT_PENDING.with(|q| q.borrow_mut().clear())),
    );
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
    /// A listener that ACCEPTS the TCP connection and then says nothing, ever. This is the shape
    /// that hung forever before there was a timeout: the socket is up, so there is no connect error
    /// to report, and the WebSocket handshake simply never completes.
    fn silent_server_port() -> u16 {
        crate::http::init();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        crate::http::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            // Hold every accepted connection open and never write a byte.
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        port
    }

    /// Collect at least `kinds` signals FOR `conn_id`, discarding everyone else's.
    ///
    /// The signal channel is process-global and these tests share it, so the old conn-agnostic
    /// version returned whatever happened to be queued — a leftover `Connected` from an earlier test
    /// satisfied `out.len() >= kinds` immediately and the caller then asserted against a signal that
    /// had nothing to do with it. That is not hypothetical: it is a failure this suite produced in
    /// CI, and it is timing-dependent (hence CI-only) because it depends on whether the earlier
    /// test's handshake finished before this one started draining.
    ///
    /// Filtering here rather than asking every test to clean up perfectly: a test that forgets is
    /// then its own problem, not the next test's.
    fn drain_for_conn(conn_id: u64, kinds: usize) -> Vec<WsSignal> {
        let mut out = Vec::new();
        for _ in 0..500 {
            while let Some(s) = try_recv_signal() {
                if s.conn_id == conn_id {
                    out.push(s);
                }
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
        connect(1, format!("ws://127.0.0.1:{port}/"), "p".into(), Vec::new());
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
        connect(2, "ws://127.0.0.1:1/".into(), "p".into(), Vec::new());
        let sigs = drain_for_conn(2, 1);
        assert!(sigs.iter().any(|s| matches!(s.kind, WsSignalKind::ConnectFailed(_))));
    }

    /// `send`/`close`/`is_owner` must agree about who owns a conn registered under the SAME string
    /// `connect` was given — including the empty one.
    ///
    /// `__s2_ws_on` used to fall back to `"legacy"` where `__s2_ws_connect`/`__s2_ws_send` fell back
    /// to `""`. Whenever `current_plugin` could not name a plugin, that split the socket in half:
    /// `send` matched the stored owner and went out on the wire, `is_owner` did not, so the
    /// subscription was silently discarded and the handler could never fire. Nothing logged, nothing
    /// failed — the connection simply went mute one way.
    #[test]
    fn ownership_is_decided_by_the_string_connect_registered_including_the_empty_one() {
        crate::http::init();
        let port = echo_server_port();
        connect(920, format!("ws://127.0.0.1:{port}/"), String::new(), Vec::new());

        assert!(is_owner(920, ""), "the empty owner connect registered must own the conn");
        assert!(!is_owner(920, "legacy"), "and a DIFFERENT fallback string must not");
        // The half that used to disagree: whatever `is_owner` says, `send` must say the same, or a
        // socket can be writable and unsubscribable at once.
        assert_eq!(
            is_owner(920, ""),
            send(920, "", "hi".into()),
            "is_owner and send must agree for the owner"
        );
        assert_eq!(
            is_owner(920, "legacy"),
            send(920, "legacy", "hi".into()),
            "is_owner and send must agree for a non-owner too"
        );
        // Consume what this test put on the PROCESS-GLOBAL signal channel (a Connected, and possibly
        // a Message from the echo) BEFORE dropping the conn. Leaving them is not a tidiness point:
        // the next test to drain sees them first, and one that waits for "at least one signal" then
        // asserts against a signal from somewhere else entirely. drain_for_conn now filters, so this
        // is belt-and-braces — but a test should not hand the suite its litter.
        //
        // Bounded by time, not by an expected count: whether the echo comes back is a race, and
        // waiting for a signal that may never arrive would cost this test the full 5s budget.
        for _ in 0..20 {
            while try_recv_signal().is_some() {}
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop_conn(920);
    }

    /// A peer that accepts the socket and then goes silent must REJECT, not hang.
    ///
    /// Before the timeout this case produced no signal at all — ever — so the connect Promise never
    /// settled, `PENDING_JOBS` never decremented, and the caller had nothing to catch. That is the
    /// failure that made the ws tests hang in CI, and it is a plugin-visible bug in its own right:
    /// any host that accepts TCP and stalls would wedge a plugin permanently.
    #[test]
    fn a_silent_peer_times_out_instead_of_hanging_forever() {
        use std::sync::atomic::Ordering;
        let port = silent_server_port();
        let prev = CONNECT_TIMEOUT_MS.swap(300, Ordering::Relaxed);

        connect(910, format!("ws://127.0.0.1:{port}/"), "p".into(), Vec::new());

        // Generous relative to the 300ms budget: this asserts the timeout FIRES, not how promptly.
        let mut failed = None;
        for _ in 0..600 {
            if let Some(s) = try_recv_signal() {
                if s.conn_id == 910 {
                    if let WsSignalKind::ConnectFailed(e) = s.kind {
                        failed = Some(e);
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        CONNECT_TIMEOUT_MS.store(prev, Ordering::Relaxed);

        let reason = failed.expect("a silent peer must produce ConnectFailed, not silence");
        assert!(
            reason.contains("did not complete"),
            "the rejection must name the timeout so an operator can tell it from a refused \
             connection; got: {reason}"
        );
        drop_conn(910);
    }
    #[test]
    fn send_wrong_owner_denied() {
        let port = echo_server_port();
        connect(3, format!("ws://127.0.0.1:{port}/"), "pA".into(), Vec::new());
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

    #[test]
    fn build_request_applies_caller_headers() {
        let req = build_request(
            "ws://127.0.0.1:1/",
            &[("Authorization".into(), "Bearer abc123".into())],
        )
        .expect("should build");
        assert_eq!(req.headers().get("authorization").unwrap(), "Bearer abc123");
    }

    #[test]
    fn build_request_keeps_the_handshake_headers_it_derives() {
        // Caller headers must be additive — the derived handshake must survive.
        let req = build_request("ws://127.0.0.1:1/", &[("X-Trace".into(), "1".into())])
            .expect("should build");
        assert!(req.headers().get("sec-websocket-key").is_some());
        assert_eq!(req.headers().get("x-trace").unwrap(), "1");
    }

    #[test]
    fn build_request_refuses_reserved_headers() {
        for name in ["Host", "Connection", "Upgrade", "Sec-WebSocket-Key", "sec-websocket-version"] {
            let err = build_request("ws://127.0.0.1:1/", &[(name.into(), "x".into())])
                .expect_err("reserved header must be refused");
            assert!(err.contains("reserved"), "unexpected reason for {name}: {err}");
        }
    }

    /// A newline in a value is how a header injection would be attempted; the
    /// value must be rejected rather than truncated or passed through.
    #[test]
    fn build_request_refuses_values_with_control_characters() {
        let err = build_request(
            "ws://127.0.0.1:1/",
            &[("X-Evil".into(), "ok\r\nX-Injected: yes".into())],
        )
        .expect_err("control characters must be refused");
        assert!(err.contains("invalid value"), "unexpected reason: {err}");
    }

    #[test]
    fn build_request_refuses_invalid_header_names() {
        let err = build_request("ws://127.0.0.1:1/", &[("bad header".into(), "x".into())])
            .expect_err("invalid name must be refused");
        assert!(err.contains("invalid header name"), "unexpected reason: {err}");
    }

    /// A refused header has to arrive as ConnectFailed, the same channel a bad
    /// URL uses — otherwise a plugin has no way to observe it at all.
    #[test]
    fn reserved_header_surfaces_as_connect_failed() {
        crate::http::init();
        connect(
            42,
            "ws://127.0.0.1:1/".into(),
            "p".into(),
            vec![("Host".into(), "evil.example".into())],
        );
        let sigs = drain_for_conn(42, 1);
        assert!(sigs.iter().any(|s| match &s.kind {
            WsSignalKind::ConnectFailed(reason) => reason.contains("reserved"),
            _ => false,
        }));
    }

    /// The tick only polls. Message/Closed are queued here, not handed back as connect results;
    /// Closed is also a deferred drop so the host can deregister after the checkpoint.
    #[test]
    fn poll_signals_returns_connects_and_queues_events() {
        while try_recv_signal().is_some() {}
        WS_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 7701));
        let tx = &engine().sig_tx;
        let _ = tx.send(WsSignal { conn_id: 7701, kind: WsSignalKind::Connected });
        let _ = tx.send(WsSignal { conn_id: 7701, kind: WsSignalKind::Message("hi".into()) });
        let _ = tx.send(WsSignal { conn_id: 7701, kind: WsSignalKind::Closed(1000, "bye".into()) });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(7701, Ok(()))]);
        assert_eq!(p.drops, vec![7701]);
        let pending = WS_EVENT_PENDING.with(|q| q.borrow().clone());
        assert!(pending.iter().any(|e| e.0 == 7701 && e.1 == "message" && e.2 == "hi"));
        assert!(pending.iter().any(|e| e.0 == 7701 && e.1 == "close" && e.2 == "bye" && e.3 == 1000));
        WS_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 7701));
    }

    #[test]
    fn poll_signals_failed_connect_is_a_drop_without_an_event() {
        while try_recv_signal().is_some() {}
        WS_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 7702));
        let _ = engine().sig_tx.send(WsSignal {
            conn_id: 7702,
            kind: WsSignalKind::ConnectFailed("nope".into()),
        });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(7702, Err("nope".into()))]);
        assert_eq!(p.drops, vec![7702]);
        assert!(WS_EVENT_PENDING.with(|q| q.borrow().iter().all(|e| e.0 != 7702)));
    }
}
