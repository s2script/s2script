//! Engine-generic WebSocket client engine. One cancellable worker per connection; V8 state stays in the adapter below.
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug)]
pub struct WsTerminal {
    pub error: Option<String>,
    pub code: u16,
    pub reason: String,
}
pub enum WsSignalKind {
    Connected,
    ConnectFailed(String),
    Message(String),
    Terminal(WsTerminal),
}
pub struct WsSignal {
    pub conn_id: u64,
    pub kind: WsSignalKind,
}
enum WsCommand {
    Send(String),
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Control {
    Open,
    CloseRequested,
    Shutdown,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnPhase {
    Connecting,
    Open,
    Closing,
    TerminalPending,
}
#[derive(Clone, Copy)]
struct SocketDeadlines {
    connect: Duration,
    close_grace: Duration,
}
impl Default for SocketDeadlines {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            close_grace: Duration::from_secs(1),
        }
    }
}
struct Conn {
    data_tx: tokio::sync::mpsc::UnboundedSender<WsCommand>,
    control_tx: tokio::sync::watch::Sender<Control>,
    phase: ConnPhase,
    owner: String,
    owner_generation: u64,
}
struct Engine {
    sig_tx: Sender<WsSignal>,
    sig_rx: Mutex<Receiver<WsSignal>>,
    conns: Mutex<HashMap<u64, Conn>>,
}
static ENGINE: OnceLock<Engine> = OnceLock::new();
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct WorkerGuard;
impl WorkerGuard {
    fn new() -> Self {
        ACTIVE_WORKERS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}
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
fn publish_control(tx: &tokio::sync::watch::Sender<Control>, next: Control) {
    tx.send_if_modified(|v| {
        if *v < next {
            *v = next;
            true
        } else {
            false
        }
    });
}
fn control(rx: &tokio::sync::watch::Receiver<Control>) -> Control {
    *rx.borrow()
}

fn insert_conn(
    conn_id: u64,
    owner: String,
    owner_generation: u64,
) -> (
    tokio::sync::mpsc::UnboundedReceiver<WsCommand>,
    tokio::sync::watch::Receiver<Control>,
) {
    let (data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel();
    let (control_tx, control_rx) = tokio::sync::watch::channel(Control::Open);
    engine().conns.lock().unwrap().insert(
        conn_id,
        Conn {
            data_tx,
            control_tx,
            phase: ConnPhase::Connecting,
            owner,
            owner_generation,
        },
    );
    (data_rx, control_rx)
}

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
            return Err(format!(
                "header '{name}' is reserved by the websocket handshake"
            ));
        }
        let n = HeaderName::from_bytes(lower.as_bytes())
            .map_err(|_| format!("invalid header name: '{name}'"))?;
        let v = HeaderValue::from_str(value)
            .map_err(|_| format!("invalid value for header '{name}'"))?;
        req.headers_mut().insert(n, v);
    }
    Ok(req)
}

async fn graceful_ws<S>(
    write: &mut S,
    data_rx: &mut tokio::sync::mpsc::UnboundedReceiver<WsCommand>,
    control_rx: &mut tokio::sync::watch::Receiver<Control>,
    deadline: tokio::time::Instant,
) -> Option<Result<(), String>>
where
    S: Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    let graceful = tokio::time::timeout_at(deadline, async {
        while let Ok(WsCommand::Send(t)) = data_rx.try_recv() {
            write
                .send(Message::text(t))
                .await
                .map_err(|e| e.to_string())?;
        }
        write
            .send(Message::Close(None))
            .await
            .map_err(|e| e.to_string())?;
        write.close().await.map_err(|e| e.to_string())
    });
    tokio::pin!(graceful);
    tokio::select! { biased;
        _ = control_rx.changed() => None,
        result = &mut graceful => Some(match result {
            Ok(result) => result,
            Err(_) => Err("graceful close deadline exceeded".to_string()),
        }),
    }
}

async fn run_ws_connected<R, W, E>(
    conn_id: u64,
    mut read: R,
    mut write: W,
    mut data_rx: tokio::sync::mpsc::UnboundedReceiver<WsCommand>,
    control_rx: &mut tokio::sync::watch::Receiver<Control>,
    sig_tx: Sender<WsSignal>,
    deadlines: SocketDeadlines,
) -> Option<WsTerminal>
where
    R: Stream<Item = Result<Message, E>> + Unpin,
    W: Sink<Message> + Unpin,
    E: std::fmt::Display,
    W::Error: std::fmt::Display,
{
    loop {
        enum Ready<E> {
            Control(Result<(), tokio::sync::watch::error::RecvError>),
            Incoming(Option<Result<Message, E>>),
            Command(Option<WsCommand>),
        }
        let ready = {
            let io = async {
                tokio::select! {
                    incoming = read.next() => Ready::Incoming(incoming),
                    command = data_rx.recv() => Ready::Command(command),
                }
            };
            tokio::pin!(io);
            tokio::select! { biased;
                changed = control_rx.changed() => Ready::Control(changed),
                ready = &mut io => ready,
            }
        };
        match ready {
            Ready::Control(changed) => {
                if changed.is_err() || control(&control_rx) == Control::Shutdown {
                    return None;
                }
                if control(&control_rx) >= Control::CloseRequested {
                    let deadline = tokio::time::Instant::now() + deadlines.close_grace;
                    return graceful_ws(&mut write, &mut data_rx, control_rx, deadline)
                        .await
                        .map(|result| match result {
                            Ok(()) => WsTerminal {
                                error: None,
                                code: 1000,
                                reason: String::new(),
                            },
                            Err(error) => WsTerminal {
                                error: Some(error),
                                code: 1006,
                                reason: "connection error".into(),
                            },
                        });
                }
            }
            Ready::Incoming(incoming) => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let _ = sig_tx.send(WsSignal {
                        conn_id,
                        kind: WsSignalKind::Message(text.to_string()),
                    });
                }
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = frame
                        .map(|frame| (u16::from(frame.code), frame.reason.to_string()))
                        .unwrap_or((1005, String::new()));
                    return Some(WsTerminal {
                        error: None,
                        code,
                        reason,
                    });
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    return Some(WsTerminal {
                        error: Some(error.to_string()),
                        code: 1006,
                        reason: "connection error".into(),
                    })
                }
                None => {
                    return Some(WsTerminal {
                        error: None,
                        code: 1006,
                        reason: "stream ended".into(),
                    })
                }
            },
            Ready::Command(command) => match command {
                Some(WsCommand::Send(text)) => {
                    let send = write.send(Message::text(text));
                    tokio::pin!(send);
                    tokio::select! { biased;
                        changed = control_rx.changed() => {
                            if changed.is_err() || control(&control_rx) == Control::Shutdown {
                                return None;
                            }
                            let deadline = tokio::time::Instant::now() + deadlines.close_grace;
                            let first = tokio::time::timeout_at(deadline, &mut send);
                            tokio::pin!(first);
                            let first = tokio::select! { biased;
                                _ = control_rx.changed() => return None,
                                result = &mut first => result,
                            };
                            match first {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => return Some(WsTerminal {
                                    error: Some(error.to_string()), code: 1006, reason: "connection error".into()
                                }),
                                Err(_) => return Some(WsTerminal {
                                    error: Some("graceful close deadline exceeded".into()), code: 1006,
                                    reason: "connection error".into()
                                }),
                            }
                            drop(send);
                            return graceful_ws(&mut write, &mut data_rx, control_rx, deadline)
                                .await
                                .map(|result| match result {
                                    Ok(()) => WsTerminal { error: None, code: 1000, reason: String::new() },
                                    Err(error) => WsTerminal {
                                        error: Some(error), code: 1006, reason: "connection error".into()
                                    },
                                });
                        }
                        result = &mut send => if let Err(error) = result {
                            return Some(WsTerminal {
                                error: Some(error.to_string()), code: 1006, reason: "connection error".into()
                            });
                        }
                    }
                }
                None => return None,
            },
        }
    }
}

fn emit_connect_failed(sig_tx: &Sender<WsSignal>, conn_id: u64, error: impl Into<String>) {
    let _ = sig_tx.send(WsSignal {
        conn_id,
        kind: WsSignalKind::ConnectFailed(error.into()),
    });
}

async fn finish_connected_worker<R, W, E>(
    conn_id: u64,
    read: R,
    write: W,
    data_rx: tokio::sync::mpsc::UnboundedReceiver<WsCommand>,
    control_rx: &mut tokio::sync::watch::Receiver<Control>,
    sig_tx: &Sender<WsSignal>,
    deadlines: SocketDeadlines,
) where
    R: Stream<Item = Result<Message, E>> + Unpin,
    W: Sink<Message> + Unpin,
    E: std::fmt::Display,
    W::Error: std::fmt::Display,
{
    let _ = sig_tx.send(WsSignal {
        conn_id,
        kind: WsSignalKind::Connected,
    });
    let terminal = run_ws_connected(
        conn_id,
        read,
        write,
        data_rx,
        control_rx,
        sig_tx.clone(),
        deadlines,
    )
    .await;
    if let Some(terminal) = terminal.filter(|_| control(control_rx) != Control::Shutdown) {
        let _ = sig_tx.send(WsSignal {
            conn_id,
            kind: WsSignalKind::Terminal(terminal),
        });
    }
}

#[cfg(test)]
pub fn connect(conn_id: u64, url: String, owner: String, headers: Vec<(String, String)>) {
    connect_owned(conn_id, url, owner, 0, headers);
}
pub(crate) fn connect_owned(
    conn_id: u64,
    url: String,
    owner: String,
    owner_generation: u64,
    headers: Vec<(String, String)>,
) {
    connect_with_deadlines(
        conn_id,
        url,
        owner,
        owner_generation,
        headers,
        SocketDeadlines::default(),
    );
}
fn connect_with_deadlines(
    conn_id: u64,
    url: String,
    owner: String,
    owner_generation: u64,
    headers: Vec<(String, String)>,
    deadlines: SocketDeadlines,
) {
    let (data_rx, mut control_rx) = insert_conn(conn_id, owner, owner_generation);
    let sig_tx = engine().sig_tx.clone();
    crate::http::spawn(async move {
        let _guard = WorkerGuard::new();
        let request = match build_request(&url, &headers) {
            Ok(v) => v,
            Err(e) => {
                emit_connect_failed(&sig_tx, conn_id, e);
                return;
            }
        };
        let attempt =
            tokio::time::timeout(deadlines.connect, tokio_tungstenite::connect_async(request));
        tokio::pin!(attempt);
        let stream = tokio::select! {biased;
         _=control_rx.changed()=>return,
         r=&mut attempt=>match r { Ok(Ok((s,_)))=>s, Ok(Err(e))=>{emit_connect_failed(&sig_tx,conn_id,e.to_string());return;}, Err(_)=>{emit_connect_failed(&sig_tx,conn_id,format!("handshake did not complete within {}ms",deadlines.connect.as_millis()));return;} }
        };
        if control(&control_rx) == Control::Shutdown {
            return;
        }
        let (write, read) = stream.split();
        finish_connected_worker(
            conn_id,
            read,
            write,
            data_rx,
            &mut control_rx,
            &sig_tx,
            deadlines,
        )
        .await;
    });
}

pub fn send(conn_id: u64, owner: &str, text: String) -> bool {
    let map = engine().conns.lock().unwrap();
    matches!(map.get(&conn_id),Some(c) if c.owner==owner&&c.phase==ConnPhase::Open&&c.data_tx.send(WsCommand::Send(text)).is_ok())
}
pub fn close(conn_id: u64, owner: &str) -> bool {
    let mut map = engine().conns.lock().unwrap();
    let Some(c) = map.get_mut(&conn_id) else {
        return false;
    };
    if c.owner != owner || c.phase != ConnPhase::Open {
        return false;
    }
    c.phase = ConnPhase::Closing;
    publish_control(&c.control_tx, Control::CloseRequested);
    true
}
pub fn is_owner(conn_id: u64, owner: &str) -> bool {
    matches!(engine().conns.lock().unwrap().get(&conn_id),Some(c) if c.owner==owner)
}
pub fn shutdown_conn(conn_id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&conn_id) {
        publish_control(&c.control_tx, Control::Shutdown);
        purge_conn(conn_id);
        crate::v8host::release_resource(
            &c.owner,
            c.owner_generation,
            &crate::plugin::Resource::WsConn(conn_id),
        );
    }
}
pub fn retire_conn(conn_id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&conn_id) {
        crate::v8host::release_resource(
            &c.owner,
            c.owner_generation,
            &crate::plugin::Resource::WsConn(conn_id),
        );
    }
}
pub fn drop_conn(conn_id: u64) {
    shutdown_conn(conn_id)
}
pub fn try_recv_signal() -> Option<WsSignal> {
    engine().sig_rx.lock().ok()?.try_recv().ok()
}
#[cfg(test)]
pub(crate) fn test_insert_conn(conn_id: u64, owner: String, generation: u64) {
    let _ = insert_conn(conn_id, owner, generation);
}
#[cfg(test)]
pub(crate) fn test_inject_batch(conn_id: u64, message: &str, error: &str) {
    let tx = &engine().sig_tx;
    let _ = tx.send(WsSignal {
        conn_id,
        kind: WsSignalKind::Connected,
    });
    let _ = tx.send(WsSignal {
        conn_id,
        kind: WsSignalKind::Message(message.into()),
    });
    let terminal = WsTerminal {
        error: Some(error.into()),
        code: 1006,
        reason: "connection error".into(),
    };
    let _ = tx.send(WsSignal {
        conn_id,
        kind: WsSignalKind::Terminal(terminal.clone()),
    });
    let _ = tx.send(WsSignal {
        conn_id,
        kind: WsSignalKind::Terminal(terminal),
    });
}
#[cfg(test)]
pub(crate) fn test_spawn_terminal_worker(conn_id: u64, owner: String, generation: u64) {
    let (data_rx, mut control_rx) = insert_conn(conn_id, owner, generation);
    let sig_tx = engine().sig_tx.clone();
    crate::http::spawn(async move {
        let _guard = WorkerGuard::new();
        finish_connected_worker(
            conn_id,
            futures_util::stream::iter(vec![
                Ok::<_, std::io::Error>(Message::text("stress")),
                Err(std::io::Error::other("stress terminal")),
            ]),
            futures_util::sink::drain(),
            data_rx,
            &mut control_rx,
            &sig_tx,
            SocketDeadlines::default(),
        )
        .await;
    });
}
#[cfg(test)]
pub(crate) fn test_spawn_failed_worker(conn_id: u64, owner: String, generation: u64) {
    let _ = insert_conn(conn_id, owner, generation);
    let sig_tx = engine().sig_tx.clone();
    crate::http::spawn(async move {
        let _guard = WorkerGuard::new();
        emit_connect_failed(&sig_tx, conn_id, "stress connect failure");
    });
}
#[cfg(test)]
pub(crate) fn test_pending_count() -> usize {
    WS_EVENT_PENDING.with(|q| q.borrow().len())
}
#[cfg(test)]
pub(crate) fn test_mux_count(conn_id: u64) -> usize {
    WS_EVENT_MUX.with(|m| {
        ["message", "error", "close"]
            .iter()
            .map(|event| m.borrow().snapshot(&format!("{conn_id}:{event}")).len())
            .sum()
    })
}
#[cfg(test)]
pub(crate) fn active_conn_count() -> usize {
    engine().conns.lock().unwrap().len()
}
#[cfg(test)]
pub(crate) fn active_worker_count() -> usize {
    ACTIVE_WORKERS.load(Ordering::SeqCst)
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
fn purge_conn(conn_id: u64) {
    WS_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != conn_id));
    WS_EVENT_MUX.with(|m| {
        let mut m = m.borrow_mut();
        for ev in ["message", "close", "error"] {
            m.remove_by_name(&format!("{conn_id}:{ev}"));
        }
    });
}

/// One drain's worth of ws signals. Connect results go back to the host so it can resolve the
/// connect Promise (needs the Jobs resolver map). Events are queued here. Only failed connects are
/// returned for retirement after the microtask checkpoint.
pub(crate) struct SignalPoll {
    pub connects: Vec<(u64, Result<(), String>)>,
    pub drops: Vec<u64>,
}

/// Drain the engine channel. The tick only polls — it does not match Message/Errored/Closed.
pub(crate) fn poll_signals() -> SignalPoll {
    let mut connects = Vec::new();
    let mut drops = Vec::new();
    while let Some(sig) = try_recv_signal() {
        let live = engine().conns.lock().unwrap().contains_key(&sig.conn_id);
        if !live {
            continue;
        }
        match sig.kind {
            WsSignalKind::Connected => {
                if let Some(c) = engine().conns.lock().unwrap().get_mut(&sig.conn_id) {
                    if c.phase == ConnPhase::Connecting {
                        c.phase = ConnPhase::Open;
                        connects.push((sig.conn_id, Ok(())));
                    }
                }
            }
            WsSignalKind::ConnectFailed(e) => {
                let accepted = engine()
                    .conns
                    .lock()
                    .unwrap()
                    .get_mut(&sig.conn_id)
                    .is_some_and(|c| {
                        if c.phase == ConnPhase::Connecting {
                            c.phase = ConnPhase::TerminalPending;
                            true
                        } else {
                            false
                        }
                    });
                if accepted {
                    connects.push((sig.conn_id, Err(e)));
                    drops.push(sig.conn_id);
                }
            }
            WsSignalKind::Message(t) => {
                if matches!(
                    engine()
                        .conns
                        .lock()
                        .unwrap()
                        .get(&sig.conn_id)
                        .map(|c| c.phase),
                    Some(ConnPhase::Open | ConnPhase::Closing)
                ) {
                    queue_event(sig.conn_id, "message", t, 0);
                }
            }
            WsSignalKind::Terminal(t) => {
                let accepted = engine()
                    .conns
                    .lock()
                    .unwrap()
                    .get_mut(&sig.conn_id)
                    .is_some_and(|c| {
                        if matches!(c.phase, ConnPhase::Open | ConnPhase::Closing) {
                            c.phase = ConnPhase::TerminalPending;
                            true
                        } else {
                            false
                        }
                    });
                if accepted {
                    if let Some(e) = t.error {
                        queue_event(sig.conn_id, "error", e, 0);
                    }
                    queue_event(sig.conn_id, "close", t.reason, t.code as i32);
                }
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
        if args.length() < 2 {
            return;
        }
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

fn s2_ws_close(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 {
            return;
        }
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
        if args.length() < 3 {
            return;
        }
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
    if pending.is_empty() {
        return;
    }

    for (conn_id, event, s, n) in pending {
        let key = format!("{conn_id}:{event}");
        let snap = WS_EVENT_MUX.with(|m| m.borrow().snapshot(&key));
        if !snap.is_empty() {
            let _ = fan_out(
                &snap,
                &format!("dispatch_pending_ws_events('{key}')"),
                Instrument::none(),
                |tc| {
                    if event == "close" {
                        let code_val: v8::Local<v8::Value> = v8::Number::new(tc, n as f64).into();
                        let reason_val: v8::Local<v8::Value> = v8::String::new(tc, &s)
                            .unwrap_or_else(|| v8::String::new(tc, "").unwrap())
                            .into();
                        Some(vec![code_val, reason_val])
                    } else {
                        let s_val: v8::Local<v8::Value> = v8::String::new(tc, &s)
                            .unwrap_or_else(|| v8::String::new(tc, "").unwrap())
                            .into();
                        Some(vec![s_val])
                    }
                },
            );
        }
        if event == "close" {
            WS_EVENT_MUX.with(|m| {
                let mut mux = m.borrow_mut();
                for ev in ["message", "close", "error"] {
                    mux.remove_by_name(&format!("{conn_id}:{ev}"));
                }
            });
            retire_conn(conn_id);
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
        Box::new(|owner| {
            WS_EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(owner));
        }),
        Box::new(|ids| {
            WS_EVENT_MUX.with(|m| {
                m.borrow_mut().remove_by_ids(ids);
            });
        }),
        Box::new(|| {
            WS_EVENT_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );
}

pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    crate::process_singletons::register(
        "WS_EVENT_PENDING",
        AfterIsolateDrop,
        Box::new(|| WS_EVENT_PENDING.with(|q| q.borrow_mut().clear())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    struct FailingSink;
    impl Sink<Message> for FailingSink {
        type Error = std::io::Error;
        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(std::io::Error::other("injected ws write failure")))
        }
        fn start_send(self: Pin<&mut Self>, _item: Message) -> Result<(), Self::Error> {
            Ok(())
        }
        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    struct PendingSink(Arc<AtomicUsize>);
    impl Sink<Message> for PendingSink {
        type Error = std::io::Error;
        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Poll::Pending
        }
        fn start_send(self: Pin<&mut Self>, _item: Message) -> Result<(), Self::Error> {
            Ok(())
        }
        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }
        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Pending
        }
    }

    struct ReadySink;
    impl Sink<Message> for ReadySink {
        type Error = std::io::Error;
        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn start_send(self: Pin<&mut Self>, _item: Message) -> Result<(), Self::Error> {
            Ok(())
        }
        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FloodStream(Arc<AtomicUsize>);
    impl Stream for FloodStream {
        type Item = Result<Message, std::io::Error>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok(Message::text("inbound"))))
        }
    }

    struct RecordingSink(Arc<AtomicUsize>);
    impl Sink<Message> for RecordingSink {
        type Error = std::io::Error;
        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            if let Message::Text(text) = item {
                self.0.fetch_add(text.len(), Ordering::SeqCst);
            }
            Ok(())
        }
        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }
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
    fn wait_workers(n: usize) {
        for _ in 0..200 {
            if active_worker_count() == n {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("worker count did not reach {n}: {}", active_worker_count());
    }
    #[test]
    fn owner_shutdown_cancels_a_pending_handshake_without_terminal_signal() {
        let base = active_worker_count();
        let port = silent_server_port();
        connect_with_deadlines(
            7699,
            format!("ws://127.0.0.1:{port}/"),
            "cancel".into(),
            0,
            Vec::new(),
            SocketDeadlines {
                connect: Duration::from_secs(5),
                close_grace: Duration::from_millis(20),
            },
        );
        wait_workers(base + 1);
        shutdown_conn(7699);
        wait_workers(base);
        assert!(!is_owner(7699, "cancel"));
        while let Some(s) = try_recv_signal() {
            assert_ne!(s.conn_id, 7699, "owner shutdown is silent");
        }
    }
    #[test]
    fn injected_ws_write_failure_returns_one_error_terminal() {
        crate::http::init();
        let (data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, signal_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx.send(WsCommand::Send("fail".into())).unwrap();
        crate::http::spawn(async move {
            let terminal = run_ws_connected(
                7710,
                futures_util::stream::pending::<Result<Message, std::io::Error>>(),
                FailingSink,
                data_rx,
                &mut control_rx,
                signal_tx,
                SocketDeadlines::default(),
            )
            .await;
            done_tx.send(terminal).unwrap();
        });
        let terminal = done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(terminal.error.as_deref(), Some("injected ws write failure"));
        assert_eq!(terminal.code, 1006);
        assert!(
            signal_rx.try_recv().is_err(),
            "write failure must not emit a second envelope"
        );
    }
    #[test]
    fn owner_shutdown_interrupts_an_injected_pending_ws_write() {
        crate::http::init();
        let polls = Arc::new(AtomicUsize::new(0));
        let (data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, signal_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx.send(WsCommand::Send("stall".into())).unwrap();
        let worker_polls = polls.clone();
        crate::http::spawn(async move {
            let terminal = run_ws_connected(
                7711,
                futures_util::stream::pending::<Result<Message, std::io::Error>>(),
                PendingSink(worker_polls),
                data_rx,
                &mut control_rx,
                signal_tx,
                SocketDeadlines::default(),
            )
            .await;
            done_tx.send(terminal).unwrap();
        });
        for _ in 0..200 {
            if polls.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(polls.load(Ordering::SeqCst) > 0, "writer was never polled");
        publish_control(&control_tx, Control::Shutdown);
        assert!(done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_none());
        assert!(signal_rx.try_recv().is_err(), "owner shutdown is silent");
    }
    #[test]
    fn continuous_ws_messages_do_not_starve_a_queued_send_or_shutdown() {
        crate::http::init();
        let reads = Arc::new(AtomicUsize::new(0));
        let bytes = Arc::new(AtomicUsize::new(0));
        let (data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, _) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx.send(WsCommand::Send("progress".into())).unwrap();
        let worker_reads = reads.clone();
        let worker_bytes = bytes.clone();
        crate::http::spawn(async move {
            done_tx
                .send(
                    run_ws_connected(
                        7714,
                        FloodStream(worker_reads),
                        RecordingSink(worker_bytes),
                        data_rx,
                        &mut control_rx,
                        signal_tx,
                        SocketDeadlines::default(),
                    )
                    .await,
                )
                .unwrap();
        });
        for _ in 0..200 {
            if bytes.load(Ordering::SeqCst) == 8 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            bytes.load(Ordering::SeqCst),
            8,
            "ready messages starved the send"
        );
        assert!(
            reads.load(Ordering::SeqCst) > 0,
            "probe did not keep messages ready"
        );
        publish_control(&control_tx, Control::Shutdown);
        assert!(done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_none());
    }
    #[test]
    fn pending_ws_write_obeys_one_grace_deadline() {
        crate::http::init();
        let polls = Arc::new(AtomicUsize::new(0));
        let (data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        let (signal_tx, _) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        data_tx.send(WsCommand::Send("stall".into())).unwrap();
        let worker_polls = polls.clone();
        crate::http::spawn(async move {
            done_tx
                .send(
                    run_ws_connected(
                        7712,
                        futures_util::stream::pending::<Result<Message, std::io::Error>>(),
                        PendingSink(worker_polls),
                        data_rx,
                        &mut control_rx,
                        signal_tx,
                        SocketDeadlines {
                            connect: Duration::from_secs(10),
                            close_grace: Duration::from_millis(20),
                        },
                    )
                    .await,
                )
                .unwrap();
        });
        for _ in 0..200 {
            if polls.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        publish_control(&control_tx, Control::CloseRequested);
        let terminal = done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            terminal.error.as_deref(),
            Some("graceful close deadline exceeded")
        );
        assert_eq!(terminal.code, 1006);
    }
    #[test]
    fn simultaneous_peer_and_local_ws_close_produces_one_terminal() {
        crate::http::init();
        let (_data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel();
        let (control_tx, mut control_rx) = tokio::sync::watch::channel(Control::Open);
        publish_control(&control_tx, Control::CloseRequested);
        let (signal_tx, signal_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        crate::http::spawn(async move {
            done_tx
                .send(
                    run_ws_connected(
                        7713,
                        futures_util::stream::iter(vec![Ok::<_, std::io::Error>(Message::Close(
                            None,
                        ))]),
                        ReadySink,
                        data_rx,
                        &mut control_rx,
                        signal_tx,
                        SocketDeadlines::default(),
                    )
                    .await,
                )
                .unwrap();
        });
        let terminal = done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            terminal.code, 1000,
            "biased local close wins the simultaneous race"
        );
        assert!(
            signal_rx.try_recv().is_err(),
            "the connected loop returns one terminal only"
        );
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
                        engine().conns.lock().unwrap().get_mut(&1).unwrap().phase = ConnPhase::Open;
                        send(1, "p", "hi".into());
                    }
                    WsSignalKind::Message(t) => {
                        echo = Some(t);
                        close(1, "p");
                    }
                    WsSignalKind::Terminal(t) => {
                        closed = Some((t.code, t.reason));
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
        // Callback dispatch drives public retirement in the host; retire directly in this engine
        // test to verify a self-close leaves no registry entry.
        retire_conn(1);
        assert!(!is_owner(1, "p"));
    }
    #[test]
    fn connect_bad_port_fails() {
        crate::http::init();
        connect(2, "ws://127.0.0.1:1/".into(), "p".into(), Vec::new());
        let sigs = drain_for_conn(2, 1);
        assert!(sigs
            .iter()
            .any(|s| matches!(s.kind, WsSignalKind::ConnectFailed(_))));
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
        connect(
            920,
            format!("ws://127.0.0.1:{port}/"),
            String::new(),
            Vec::new(),
        );
        for _ in 0..200 {
            if let Some(s) = try_recv_signal() {
                if s.conn_id == 920 && matches!(s.kind, WsSignalKind::Connected) {
                    engine().conns.lock().unwrap().get_mut(&920).unwrap().phase = ConnPhase::Open;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            is_owner(920, ""),
            "the empty owner connect registered must own the conn"
        );
        assert!(
            !is_owner(920, "legacy"),
            "and a DIFFERENT fallback string must not"
        );
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
            while let Some(s) = try_recv_signal() {
                if matches!(s.kind, WsSignalKind::Connected) {
                    if let Some(c) = engine().conns.lock().unwrap().get_mut(&920) {
                        c.phase = ConnPhase::Open;
                    }
                }
            }
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
        let port = silent_server_port();
        connect_with_deadlines(
            910,
            format!("ws://127.0.0.1:{port}/"),
            "p".into(),
            0,
            Vec::new(),
            SocketDeadlines {
                connect: Duration::from_millis(300),
                close_grace: Duration::from_millis(100),
            },
        );

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
        connect(
            3,
            format!("ws://127.0.0.1:{port}/"),
            "pA".into(),
            Vec::new(),
        );
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
        for name in [
            "Host",
            "Connection",
            "Upgrade",
            "Sec-WebSocket-Key",
            "sec-websocket-version",
        ] {
            let err = build_request("ws://127.0.0.1:1/", &[(name.into(), "x".into())])
                .expect_err("reserved header must be refused");
            assert!(
                err.contains("reserved"),
                "unexpected reason for {name}: {err}"
            );
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
        assert!(
            err.contains("invalid header name"),
            "unexpected reason: {err}"
        );
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

    /// The tick only polls. Messages and the single terminal envelope are queued for dispatch.
    #[test]
    fn poll_signals_returns_connects_and_queues_events() {
        while try_recv_signal().is_some() {}
        WS_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 7701));
        let tx = &engine().sig_tx;
        let (data_tx, _data_rx) = tokio::sync::mpsc::unbounded_channel();
        let (control_tx, _) = tokio::sync::watch::channel(Control::Open);
        engine().conns.lock().unwrap().insert(
            7701,
            Conn {
                data_tx,
                control_tx,
                phase: ConnPhase::Connecting,
                owner: "test".into(),
                owner_generation: 0,
            },
        );
        let _ = tx.send(WsSignal {
            conn_id: 7701,
            kind: WsSignalKind::Connected,
        });
        let _ = tx.send(WsSignal {
            conn_id: 7701,
            kind: WsSignalKind::Message("hi".into()),
        });
        let _ = tx.send(WsSignal {
            conn_id: 7701,
            kind: WsSignalKind::Terminal(WsTerminal {
                error: Some("boom".into()),
                code: 1006,
                reason: "bye".into(),
            }),
        });
        let _ = tx.send(WsSignal {
            conn_id: 7701,
            kind: WsSignalKind::Terminal(WsTerminal {
                error: Some("duplicate".into()),
                code: 1006,
                reason: "duplicate".into(),
            }),
        });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(7701, Ok(()))]);
        assert!(
            p.drops.is_empty(),
            "public terminal retirement belongs after callback dispatch"
        );
        let pending = WS_EVENT_PENDING.with(|q| q.borrow().clone());
        let ours: Vec<_> = pending.iter().filter(|e| e.0 == 7701).collect();
        assert_eq!(ours.len(), 3, "duplicate terminal must be discarded");
        assert_eq!(ours[0].1, "message");
        assert_eq!((ours[1].1.as_str(), ours[1].2.as_str()), ("error", "boom"));
        assert_eq!(
            (ours[2].1.as_str(), ours[2].2.as_str(), ours[2].3),
            ("close", "bye", 1006)
        );
        WS_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 7701));
        shutdown_conn(7701);
    }

    #[test]
    fn poll_signals_failed_connect_is_a_drop_without_an_event() {
        while try_recv_signal().is_some() {}
        WS_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 7702));
        let (data_tx, _data_rx) = tokio::sync::mpsc::unbounded_channel();
        let (control_tx, _) = tokio::sync::watch::channel(Control::Open);
        engine().conns.lock().unwrap().insert(
            7702,
            Conn {
                data_tx,
                control_tx,
                phase: ConnPhase::Connecting,
                owner: "test".into(),
                owner_generation: 0,
            },
        );
        let _ = engine().sig_tx.send(WsSignal {
            conn_id: 7702,
            kind: WsSignalKind::ConnectFailed("nope".into()),
        });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(7702, Err("nope".into()))]);
        assert_eq!(p.drops, vec![7702]);
        assert!(WS_EVENT_PENDING.with(|q| q.borrow().iter().all(|e| e.0 != 7702)));
    }

    #[test]
    fn a_thousand_adapter_transitions_leave_no_socket_state() {
        let workers = active_worker_count();
        for i in 0..1000u64 {
            let id = 80_000 + i;
            let (data_tx, _) = tokio::sync::mpsc::unbounded_channel();
            let (control_tx, _) = tokio::sync::watch::channel(Control::Open);
            engine().conns.lock().unwrap().insert(
                id,
                Conn {
                    data_tx,
                    control_tx,
                    phase: ConnPhase::Connecting,
                    owner: "stress".into(),
                    owner_generation: 0,
                },
            );
            if i % 2 == 0 {
                let _ = engine().sig_tx.send(WsSignal {
                    conn_id: id,
                    kind: WsSignalKind::Connected,
                });
                let _ = engine().sig_tx.send(WsSignal {
                    conn_id: id,
                    kind: WsSignalKind::Terminal(WsTerminal {
                        error: None,
                        code: 1000,
                        reason: String::new(),
                    }),
                });
                let p = poll_signals();
                assert!(p.drops.is_empty());
                dispatch_pending_events();
            } else {
                let _ = engine().sig_tx.send(WsSignal {
                    conn_id: id,
                    kind: WsSignalKind::ConnectFailed("x".into()),
                });
                let p = poll_signals();
                for id in p.drops {
                    retire_conn(id);
                }
            }
        }
        assert_eq!(active_conn_count(), 0);
        assert_eq!(active_worker_count(), workers);
        assert!(WS_EVENT_PENDING.with(|q| q.borrow().is_empty()));
    }
}
