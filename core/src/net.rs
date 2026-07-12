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
pub fn is_owner(conn_id: u64, owner: &str) -> bool {
    matches!(engine().conns.lock().unwrap().get(&conn_id), Some(c) if c.owner == owner)
}
pub fn drop_conn(conn_id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&conn_id) { let _ = c.cmd_tx.send(NetCommand::Shutdown); }
}
pub fn try_recv_signal() -> Option<NetSignal> { engine().sig_rx.lock().ok()?.try_recv().ok() }

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
}
