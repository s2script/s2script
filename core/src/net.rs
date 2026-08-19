//! Engine-generic raw TCP + UDP socket engine. Per connection a tokio task (on the SHARED http
//! runtime) drives the socket and emits NetSignals down a channel the frame drain polls. Holds NO V8
//! handles. Mirrors ws.rs (registry, owner-scoping, Close-vs-Shutdown split); adds raw binary payloads.
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const READ_CAP: usize = 64 * 1024;

pub enum NetSignalKind {
    Connected,                                   // TCP connected
    Bound,                                       // UDP bound
    ConnectFailed(String),                       // connect/bind failed -> reject the Promise
    Data(Vec<u8>),                               // TCP inbound chunk
    Datagram { from: String, data: Vec<u8> },    // UDP inbound datagram ("host:port")
    Closed,                                      // terminal
    Errored(String),                             // mid-stream error (queues an "error" event)
}
pub struct NetSignal { pub conn_id: u64, pub kind: NetSignalKind }

enum NetCommand {
    Send(Vec<u8>),                               // TCP write
    SendTo(String, u16, Vec<u8>),                // UDP datagram
    Close,                                        // JS close() -> emit Closed
    Shutdown,                                     // teardown -> NO signal (mirrors ws)
}

struct Conn { cmd_tx: tokio::sync::mpsc::UnboundedSender<NetCommand>, owner: String }
struct Engine { sig_tx: Sender<NetSignal>, sig_rx: Mutex<Receiver<NetSignal>>, conns: Mutex<HashMap<u64, Conn>> }
static ENGINE: OnceLock<Engine> = OnceLock::new();
fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| { let (sig_tx, sig_rx) = channel(); Engine { sig_tx, sig_rx: Mutex::new(sig_rx), conns: Mutex::new(HashMap::new()) } })
}

pub fn connect_tcp(conn_id: u64, host: String, port: u16, owner: String) {
    let e = engine();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<NetCommand>();
    e.conns.lock().unwrap().insert(conn_id, Conn { cmd_tx, owner });
    let sig_tx = e.sig_tx.clone();
    crate::http::spawn(async move {
        let stream = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
            Ok(s) => s,
            Err(err) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::ConnectFailed(err.to_string()) }); return; }
        };
        let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Connected });
        let (mut read, mut write) = stream.into_split();
        let mut buf = vec![0u8; READ_CAP];
        loop {
            tokio::select! {
                r = read.read(&mut buf) => match r {
                    Ok(0) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Closed }); break; }
                    Ok(n) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Data(buf[..n].to_vec()) }); }
                    Err(err) => {
                        // mid-stream read error is terminal: Errored THEN Closed (ws-parity — Closed
                        // is what drives drop_conn + the mux prune in the drain).
                        let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Errored(err.to_string()) });
                        let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Closed }); break;
                    }
                },
                cmd = cmd_rx.recv() => match cmd {
                    Some(NetCommand::Send(bytes)) => { if write.write_all(&bytes).await.is_err() { break; } }
                    Some(NetCommand::Close) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Closed }); break; }
                    Some(NetCommand::Shutdown) | None => { break; } // teardown: no signal
                    Some(NetCommand::SendTo(..)) => { /* wrong socket type — ignore */ }
                }
            }
        }
    });
}

pub fn bind_udp(conn_id: u64, owner: String) {
    let e = engine();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<NetCommand>();
    e.conns.lock().unwrap().insert(conn_id, Conn { cmd_tx, owner });
    let sig_tx = e.sig_tx.clone();
    crate::http::spawn(async move {
        let sock = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(err) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::ConnectFailed(err.to_string()) }); return; }
        };
        let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Bound });
        let mut buf = vec![0u8; READ_CAP];
        loop {
            tokio::select! {
                r = sock.recv_from(&mut buf) => match r {
                    Ok((n, from)) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Datagram { from: from.to_string(), data: buf[..n].to_vec() } }); }
                    Err(err) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Errored(err.to_string()) }); }
                },
                cmd = cmd_rx.recv() => match cmd {
                    Some(NetCommand::SendTo(host, port, bytes)) => { let _ = sock.send_to(&bytes, (host.as_str(), port)).await; }
                    Some(NetCommand::Close) => { let _ = sig_tx.send(NetSignal { conn_id, kind: NetSignalKind::Closed }); break; }
                    Some(NetCommand::Shutdown) | None => { break; }
                    Some(NetCommand::Send(_)) => { /* wrong socket type — ignore */ }
                }
            }
        }
    });
}

pub fn send(conn_id: u64, owner: &str, bytes: Vec<u8>) -> bool {
    let map = engine().conns.lock().unwrap();
    match map.get(&conn_id) { Some(c) if c.owner == owner => c.cmd_tx.send(NetCommand::Send(bytes)).is_ok(), _ => false }
}
pub fn send_to(conn_id: u64, owner: &str, host: String, port: u16, bytes: Vec<u8>) -> bool {
    let map = engine().conns.lock().unwrap();
    match map.get(&conn_id) { Some(c) if c.owner == owner => c.cmd_tx.send(NetCommand::SendTo(host, port, bytes)).is_ok(), _ => false }
}
pub fn close(conn_id: u64, owner: &str) -> bool {
    let map = engine().conns.lock().unwrap();
    match map.get(&conn_id) { Some(c) if c.owner == owner => c.cmd_tx.send(NetCommand::Close).is_ok(), _ => false }
}
/// Ownership check for `__s2_net_on` (mirrors the owner guard in `send`/`close`) — a subscribe on a
/// conn this plugin doesn't own must no-op.
pub fn is_owner(conn_id: u64, owner: &str) -> bool {
    matches!(engine().conns.lock().unwrap().get(&conn_id), Some(c) if c.owner == owner)
}
pub fn drop_conn(conn_id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&conn_id) { let _ = c.cmd_tx.send(NetCommand::Shutdown); }
}
pub fn try_recv_signal() -> Option<NetSignal> { engine().sig_rx.lock().ok()?.try_recv().ok() }

// ---------------------------------------------------------------------------
// V8 adapter — natives, mux, pending queue, post-drain dispatch, teardown.
// The tokio engine above holds no V8 handles. This half owns the isolate-facing
// surface and routes through `fan_out` so the host isolate stays in `v8host`.
// ---------------------------------------------------------------------------

use crate::dispatch::{fan_out, Instrument};
use crate::v8host::{current_plugin, log_warn, set_native, subscribe_into};

enum PendingNetEvent {
    Data(Vec<u8>),
    Datagram { from: String, data: Vec<u8> },
    Closed,
    Errored(String),
}

thread_local! {
    static NET_EVENT_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    static NET_EVENT_PENDING: std::cell::RefCell<Vec<(u64, PendingNetEvent)>>
        = std::cell::RefCell::new(Vec::new());
}

fn queue_data(conn_id: u64, b: Vec<u8>) {
    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((conn_id, PendingNetEvent::Data(b))));
}
fn queue_datagram(conn_id: u64, from: String, data: Vec<u8>) {
    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((conn_id, PendingNetEvent::Datagram { from, data })));
}
fn queue_error(conn_id: u64, e: String) {
    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((conn_id, PendingNetEvent::Errored(e))));
}
fn queue_close(conn_id: u64) {
    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((conn_id, PendingNetEvent::Closed)));
}

/// One drain's worth of net signals. Connect/bind results go back to the host so it can resolve
/// the Promise (needs the Jobs resolver map). Events are queued here. Terminal drops are returned
/// so the host can `drop_conn` AFTER the microtask checkpoint.
pub(crate) struct SignalPoll {
    pub connects: Vec<(u64, Result<(), String>)>,
    pub drops: Vec<u64>,
}

/// Drain the engine channel. The tick only polls — it does not match Data/Datagram/Errored/Closed.
pub(crate) fn poll_signals() -> SignalPoll {
    let mut connects = Vec::new();
    let mut drops = Vec::new();
    while let Some(sig) = try_recv_signal() {
        match sig.kind {
            NetSignalKind::Connected | NetSignalKind::Bound => {
                connects.push((sig.conn_id, Ok(())));
            }
            NetSignalKind::ConnectFailed(e) => {
                connects.push((sig.conn_id, Err(e)));
                drops.push(sig.conn_id);
            }
            NetSignalKind::Data(b) => queue_data(sig.conn_id, b),
            NetSignalKind::Datagram { from, data } => queue_datagram(sig.conn_id, from, data),
            NetSignalKind::Errored(e) => queue_error(sig.conn_id, e),
            NetSignalKind::Closed => {
                queue_close(sig.conn_id);
                drops.push(sig.conn_id);
            }
        }
    }
    SignalPoll { connects, drops }
}

fn net_owner(scope: &mut v8::PinScope) -> String {
    current_plugin(scope).unwrap_or_default()
}

/// Read a native arg as bytes: a TypedArray/DataView (copied) or a string (UTF-8). Never hands a
/// raw backing store to Rust.
fn js_bytes_arg(scope: &mut v8::PinScope, val: v8::Local<v8::Value>) -> Vec<u8> {
    if val.is_string() {
        return val.to_rust_string_lossy(scope).into_bytes();
    }
    if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(val) {
        let len = view.byte_length();
        let mut buf = vec![0u8; len];
        let n = view.copy_contents(&mut buf);
        buf.truncate(n);
        return buf;
    }
    Vec::new()
}

/// Build a JS `Uint8Array` from bytes — a fresh copy into a V8-owned ArrayBuffer.
fn bytes_to_uint8array<'s>(scope: &mut v8::PinScope<'s, '_>, bytes: &[u8]) -> v8::Local<'s, v8::Value> {
    if bytes.is_empty() {
        let ab = v8::ArrayBuffer::new(scope, 0);
        return match v8::Uint8Array::new(scope, ab, 0, 0) {
            Some(u) => u.into(),
            None => v8::null(scope).into(),
        };
    }
    let store = v8::ArrayBuffer::new_backing_store_from_bytes(bytes.to_vec()).make_shared();
    let ab = v8::ArrayBuffer::with_backing_store(scope, &store);
    let len = bytes.len();
    match v8::Uint8Array::new(scope, ab, 0, len) {
        Some(u) => u.into(),
        None => v8::null(scope).into(),
    }
}

fn s2_net_send(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let bytes = js_bytes_arg(scope, args.get(1));
        let owner = net_owner(scope);
        send(id, &owner, bytes);
    }));
}

fn s2_net_send_to(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let dhost = args.get(1).to_rust_string_lossy(scope);
        let port = args.get(2).number_value(scope).unwrap_or(0.0) as u16;
        let bytes = js_bytes_arg(scope, args.get(3));
        let owner = net_owner(scope);
        send_to(id, &owner, dhost, port, bytes);
    }));
}

fn s2_net_close(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let owner = net_owner(scope);
        close(id, &owner);
    }));
}

fn s2_net_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let id = args.get(0).number_value(scope).unwrap_or(0.0) as u64;
        let event = args.get(1).to_rust_string_lossy(scope);
        let owner = net_owner(scope);
        if !is_owner(id, &owner) {
            log_warn(&format!(
                "WARN: __s2_net_on: '{owner}' does not own net conn {id} — '{event}' handler NOT \
                 subscribed and will never fire"
            ));
            return;
        }
        let key = format!("{id}:{event}");
        let _ = subscribe_into(scope, &args, &NET_EVENT_MUX, &key, 2);
    }));
}

pub(crate) fn dispatch_pending_events() {
    let pending: Vec<(u64, PendingNetEvent)> =
        NET_EVENT_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() { return; }

    for (conn_id, ev) in pending {
        let event: &str = match &ev {
            PendingNetEvent::Data(_) => "data",
            PendingNetEvent::Datagram { .. } => "message",
            PendingNetEvent::Closed => "close",
            PendingNetEvent::Errored(_) => "error",
        };
        let key = format!("{conn_id}:{event}");
        let snap = NET_EVENT_MUX.with(|m| m.borrow().snapshot(&key));
        if !snap.is_empty() {
            let _ = fan_out(&snap, &format!("dispatch_pending_net_events('{key}')"), Instrument::none(), |tc| {
                match &ev {
                    PendingNetEvent::Data(b) => Some(vec![bytes_to_uint8array(tc, b)]),
                    PendingNetEvent::Datagram { from, data } => {
                        let (fhost, fport): (&str, u16) = match from.rsplit_once(':') {
                            Some((h, p)) => (h, p.parse::<u16>().unwrap_or(0)),
                            None => (from.as_str(), 0),
                        };
                        let from_obj = v8::Object::new(tc);
                        if let Some(k) = v8::String::new(tc, "host") {
                            let v: v8::Local<v8::Value> =
                                v8::String::new(tc, fhost).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                            from_obj.set(tc, k.into(), v);
                        }
                        if let Some(k) = v8::String::new(tc, "port") {
                            let v: v8::Local<v8::Value> = v8::Number::new(tc, fport as f64).into();
                            from_obj.set(tc, k.into(), v);
                        }
                        Some(vec![from_obj.into(), bytes_to_uint8array(tc, data)])
                    }
                    PendingNetEvent::Errored(e) => {
                        let s_val: v8::Local<v8::Value> =
                            v8::String::new(tc, e).unwrap_or_else(|| v8::String::new(tc, "").unwrap()).into();
                        Some(vec![s_val])
                    }
                    PendingNetEvent::Closed => Some(vec![]),
                }
            });
        }
        if matches!(ev, PendingNetEvent::Closed) {
            NET_EVENT_MUX.with(|m| {
                let mut mux = m.borrow_mut();
                for evn in ["data", "message", "error", "close"] {
                    mux.remove_by_name(&format!("{conn_id}:{evn}"));
                }
            });
        }
    }
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_net_send", s2_net_send);
    set_native(scope, global_obj, "__s2_net_send_to", s2_net_send_to);
    set_native(scope, global_obj, "__s2_net_close", s2_net_close);
    set_native(scope, global_obj, "__s2_net_on", s2_net_on);
}

pub(crate) fn register_store() {
    crate::owner_stores::register(
        "NET_EVENT_MUX",
        Box::new(|owner| { NET_EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { NET_EVENT_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            NET_EVENT_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );
}

pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    crate::process_singletons::register(
        "NET_EVENT_PENDING", AfterIsolateDrop,
        Box::new(|| NET_EVENT_PENDING.with(|q| q.borrow_mut().clear())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    // A local TCP echo listener on the shared runtime (mirrors ws.rs's echo_server_port).
    fn tcp_echo_port() -> u16 {
        crate::http::init();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 64];
                if let Ok(n) = s.read(&mut buf) { let _ = s.write_all(&buf[..n]); }
            }
        });
        port
    }
    fn drain_until<F: Fn(&NetSignal) -> bool>(f: F) -> NetSignal {
        for _ in 0..500 {
            while let Some(s) = try_recv_signal() { if f(&s) { return s; } }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no matching signal");
    }
    #[test]
    fn tcp_connect_send_echo() {
        let port = tcp_echo_port();
        connect_tcp(1, "127.0.0.1".into(), port, "p".into());
        drain_until(|s| matches!(s.kind, NetSignalKind::Connected));
        assert!(send(1, "p", b"hello".to_vec()));
        let sig = drain_until(|s| matches!(s.kind, NetSignalKind::Data(_)));
        match sig.kind { NetSignalKind::Data(b) => assert_eq!(b, b"hello"), _ => unreachable!() }
        assert!(!send(1, "pB", b"x".to_vec())); // wrong owner denied
        close(1, "p");
        drain_until(|s| matches!(s.kind, NetSignalKind::Closed));
        drop_conn(1);
        assert!(!is_owner(1, "p"));
    }
    #[test]
    fn tcp_bad_port_fails() {
        crate::http::init();
        connect_tcp(2, "127.0.0.1".into(), 1, "p".into());
        drain_until(|s| matches!(s.kind, NetSignalKind::ConnectFailed(_)));
    }
    #[test]
    fn udp_bind_send_recv() {
        crate::http::init();
        // A local UDP echo on a std socket + thread.
        let echo = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            if let Ok((n, from)) = echo.recv_from(&mut buf) { let _ = echo.send_to(&buf[..n], from); }
        });
        bind_udp(3, "p".into());
        drain_until(|s| matches!(s.kind, NetSignalKind::Bound));
        assert!(send_to(3, "p", "127.0.0.1".into(), echo_port, b"ping".to_vec()));
        let sig = drain_until(|s| matches!(s.kind, NetSignalKind::Datagram{..}));
        match sig.kind { NetSignalKind::Datagram{data,..} => assert_eq!(data, b"ping"), _ => unreachable!() }
        drop_conn(3);
    }

    /// The tick only polls. Data/Closed are queued here, not handed back as connect results;
    /// Closed is also a deferred drop so the host can deregister after the checkpoint.
    #[test]
    fn poll_signals_returns_connects_and_queues_events() {
        while try_recv_signal().is_some() {}
        NET_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 8801));
        let tx = &engine().sig_tx;
        let _ = tx.send(NetSignal { conn_id: 8801, kind: NetSignalKind::Connected });
        let _ = tx.send(NetSignal { conn_id: 8801, kind: NetSignalKind::Data(b"hi".to_vec()) });
        let _ = tx.send(NetSignal { conn_id: 8801, kind: NetSignalKind::Closed });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(8801, Ok(()))]);
        assert_eq!(p.drops, vec![8801]);
        let pending = NET_EVENT_PENDING.with(|q| q.borrow().len());
        let ours = NET_EVENT_PENDING.with(|q| {
            q.borrow().iter().filter(|e| e.0 == 8801).count()
        });
        assert_eq!(ours, 2, "expected data + close queued, pending={pending}");
        NET_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 8801));
    }

    #[test]
    fn poll_signals_failed_connect_is_a_drop_without_an_event() {
        while try_recv_signal().is_some() {}
        NET_EVENT_PENDING.with(|q| q.borrow_mut().retain(|e| e.0 != 8802));
        let _ = engine().sig_tx.send(NetSignal {
            conn_id: 8802,
            kind: NetSignalKind::ConnectFailed("nope".into()),
        });
        let p = poll_signals();
        assert_eq!(p.connects, vec![(8802, Err("nope".into()))]);
        assert_eq!(p.drops, vec![8802]);
        assert!(NET_EVENT_PENDING.with(|q| q.borrow().iter().all(|e| e.0 != 8802)));
    }
}
