# `@s2script/net` — raw TCP + UDP sockets — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** raw TCP + UDP client sockets (binary `Uint8Array`) over the shared tokio runtime, off the game thread — the async-network category's last primitive.

**Architecture:** `core/src/net.rs` is a near-copy of `core/src/ws.rs` (one owner-scoped registry, a per-connection tokio task, a signal channel the frame drain polls, the connect-resolve + post-drain event fan-out + ledgered teardown spine). The one net-new mechanism is `Uint8Array ↔ Vec<u8>` marshalling (no prior art in core). `@s2script/net` is the prelude wrapper; `packages/net` is a new types package → PR + changeset.

**Tech Stack:** Rust (`tokio::net` — the `net` feature is already enabled), JavaScript (the `@s2script/net` prelude in `core/src/v8host.rs`), TypeScript (`packages/net/index.d.ts`).

**Design:** `docs/superpowers/specs/2026-07-12-net-sockets-design.md`

## Global Constraints

- **Mirror `ws.rs` / the ws V8 wiring faithfully** — `core/src/ws.rs` (the engine) and, in `core/src/v8host.rs`: `s2_ws_connect`, `resolve_ws_connect` (2372), `s2_ws_send`/`close`/`on` (2409/2421/2438), `dispatch_pending_ws_events` (2460), the `frame_async_drain` ws-signal routing (~7476), the `ffi.rs` post-drain call (`ffi.rs:61`), the ws prelude wrapper (1834), `Resource::WsConn` (plugin.rs), the shutdown reset (7298) and teardown `remove_by_owner` (7594). Every load-bearing invariant (owner-scoping, `PENDING_JOBS` balance, the owner-liveness DROP guard, the `Close` vs `Shutdown` split, the terminal-close mux prune) carries over.
- **Binary:** payloads are `Uint8Array` (`send`/`sendTo` also accept a `string` → UTF-8). A **64 KB per-read chunk cap** (no unbounded buffering).
- **Boundary:** `net.rs` + natives are engine-generic (host/port/bytes; no CS2/game symbol); `@s2script/net` is an engine-generic prelude module. **No shim change, no new `S2EngineOps` op** (tokio in core; natives `set_native`'d). One sniper rebuild.
- **Safety:** owner-scoped opaque integer handles (no raw socket/fd to JS); client-only (no inbound listener); ledgered `Resource::NetConn` → teardown closes the socket; a signal for an unloaded conn drops (owner-liveness guard).
- **`packages/*` changes** (new `packages/net`) → **branch → PR → include a Changesets changeset** (`@s2script/net`).
- Core tests run serial (`RUST_TEST_THREADS=1`).

---

## File Structure

- **Create** `core/src/net.rs` — the TCP+UDP engine (mirror `ws.rs`).
- **Modify** `core/src/lib.rs` — `mod net;`.
- **Modify** `core/src/plugin.rs` — `Resource::NetConn(u64)` + `record_net_conn`.
- **Modify** `core/src/v8host.rs` — 6 natives + `resolve_net_connect` + `dispatch_pending_net_events` + `NET_EVENT_MUX`/`NET_EVENT_PENDING` + the `frame_async_drain` routing + native registration + the `NetConn` teardown arm + the shutdown reset + the `@s2script/net` prelude + the `Uint8Array` marshalling helpers.
- **Modify** `core/src/ffi.rs` — call `dispatch_pending_net_events()` after `frame_async_drain()` (beside the ws call).
- **Create** `packages/net/{package.json,index.d.ts}` + `.changeset/net-sockets.md`.
- **Create** `examples/net-demo/{package.json,tsconfig.json,src/plugin.ts}`.

---

## Task 1: `core/src/net.rs` — the TCP + UDP engine

**Files:**
- Create: `core/src/net.rs`; Modify: `core/src/lib.rs` (`mod net;`)

**Interfaces:**
- Consumes: `crate::http::spawn` (shared runtime).
- Produces: `NetSignal { conn_id, kind }`, `NetSignalKind` (`Connected | Bound | ConnectFailed(String) | Data(Vec<u8>) | Datagram{from:String,data:Vec<u8>} | Closed | Errored(String)`); `connect_tcp(id, host, port, owner)`, `bind_udp(id, owner)`, `send(id, owner, Vec<u8>) -> bool`, `send_to(id, owner, host, port, Vec<u8>) -> bool`, `close(id, owner) -> bool`, `is_owner(id, owner) -> bool`, `drop_conn(id)`, `try_recv_signal() -> Option<NetSignal>`.

- [ ] **Step 1: Write the failing test** (create `core/src/net.rs` with the test + the fn signatures stubbed):

```rust
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
```

- [ ] **Step 2: Run — expect FAIL** (`connect_tcp` etc. undefined).

Run: `cd core && cargo test --lib net::tests`
Expected: FAIL (compile).

- [ ] **Step 3: Write `core/src/net.rs`.** Copy `ws.rs`'s `Engine`/`Conn`/`engine()`/`send`/`close`/`is_owner`/`drop_conn`/`try_recv_signal` shape verbatim (substituting `NetSignal`/`NetCommand`), then the two spawn fns. Use `tokio::io::{AsyncReadExt, AsyncWriteExt}` for TCP and `tokio::net::{TcpStream, UdpSocket}`.

```rust
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
```

- [ ] **Step 4: Add `mod net;`** to `core/src/lib.rs` (beside `mod ws;`).

- [ ] **Step 5: Run tests — expect PASS.**

Run: `cd core && cargo test --lib net::tests`
Expected: PASS (3 tests). Then `cargo build` clean.

- [ ] **Step 6: Commit.**

```bash
git add core/src/net.rs core/src/lib.rs
git commit -m "feat(net): core tcp+udp socket engine (mirrors ws.rs, binary payloads)"
```

---

## Task 2: V8 natives + `Uint8Array` marshalling + resolve/dispatch/drain/ledger

**Files:**
- Modify: `core/src/plugin.rs` (`Resource::NetConn` + `record_net_conn`), `core/src/v8host.rs`, `core/src/ffi.rs`

**Interfaces:**
- Consumes: Task 1's `net::*`; the ws V8 spine (`RESOLVERS`, `record_job`, `PENDING_JOBS`, `resolver_owner_tag`, `next_async_id`, `refresh_detour`, `current_plugin`, `event_mux::EventMux`, `WS_EVENT_MUX`/`WS_EVENT_PENDING` as templates).
- Produces (JS natives): `__s2_net_tcp_connect(host, port) -> Promise<id>`; `__s2_net_udp_bind() -> Promise<id>`; `__s2_net_send(id, data)`; `__s2_net_send_to(id, host, port, data)`; `__s2_net_close(id)`; `__s2_net_on(id, event, handler)`.

- [ ] **Step 1: Ledger resource.** In `core/src/plugin.rs`, beside `WsConn(u64)`: add `NetConn(u64)` + `pub fn record_net_conn(&mut self, id: u64) { self.order.push(Resource::NetConn(id)); }`.

- [ ] **Step 2: The `Uint8Array` marshalling helpers** (NO prior art in core — use the rusty_v8 API exactly). Add to `v8host.rs`:

```rust
/// Read a native arg as bytes: a `Uint8Array`/any TypedArray/DataView (copied) OR a `string` (UTF-8).
/// Anything else → empty. Copies (never hands a raw backing store to Rust across the boundary).
fn js_bytes_arg(scope: &mut v8::PinScope, val: v8::Local<v8::Value>) -> Vec<u8> {
    if val.is_string() { return val.to_rust_string_lossy(scope).into_bytes(); }
    if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(val) {
        let len = view.byte_length();
        let mut buf = vec![0u8; len];
        let n = view.copy_contents(&mut buf);   // copies min(len, view) bytes
        buf.truncate(n);
        return buf;
    }
    Vec::new()
}

/// Build a JS `Uint8Array` from bytes (a fresh copy → a standalone ArrayBuffer owned by V8).
fn bytes_to_uint8array<'s>(scope: &mut v8::HandleScope<'s>, bytes: &[u8]) -> v8::Local<'s, v8::Value> {
    let store = v8::ArrayBuffer::new_backing_store_from_bytes(bytes.to_vec()).make_shared();
    let ab = v8::ArrayBuffer::with_backing_store(scope, &store);
    let len = bytes.len();
    match v8::Uint8Array::new(scope, ab, 0, len) { Some(u) => u.into(), None => v8::null(scope).into() }
}
```
(IMPLEMENTER NOTE: `ArrayBufferView::copy_contents(&self, &mut [u8]) -> usize` and `byte_length()` are the read API; `ArrayBuffer::new_backing_store_from_bytes(impl Allocated<[u8]>)` → `.make_shared()` → `ArrayBuffer::with_backing_store` → `Uint8Array::new(scope, ab, offset, length) -> Option` is the build API, for the pinned v8 = "149.4.0". If a signature differs, `cargo build` names the exact type — adjust; the semantics [copy in, copy out] must hold.)

- [ ] **Step 3: The two connect natives** (mirror `s2_ws_connect` at v8host.rs:2336 — the resolver/`resolver_owner_tag`/`record_job`+`record_net_conn`/`RESOLVERS`/`PENDING_JOBS++`/`refresh_detour`/return-promise block VERBATIM), substituting the hand-off:

```rust
// __s2_net_tcp_connect(host, port) -> Promise<id>. Mirrors s2_ws_connect; ledgers Job + NetConn.
fn s2_net_tcp_connect(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    // [same resolver/owner/ledger(record_job + record_net_conn)/RESOLVERS/PENDING_JOBS/refresh_detour block as s2_ws_connect]
    // hand-off: let host = args.get(0).to_rust_string_lossy(scope); let port = args.get(1).number_value(scope).unwrap_or(0.0) as u16;
    //           crate::net::connect_tcp(id, host, port, owner_string);
}
// __s2_net_udp_bind() -> Promise<id>. Same block; hand-off: crate::net::bind_udp(id, owner_string).
```
(As with `s2_ws_connect`, use ONE fresh async id as both the RESOLVERS key AND the `conn_id`; ledger the connection as `NetConn` via `record_net_conn` so an unclosed socket is dropped at teardown.)

- [ ] **Step 4: The send/close/on natives** (mirror `s2_ws_send`/`close`/`on` at 2409/2421/2438 exactly, but bytes-typed):

```rust
fn s2_net_send(scope, args, _rv) {           // __s2_net_send(id, data)
    // id = arg0 as u64; bytes = js_bytes_arg(scope, arg1); owner = current_plugin; crate::net::send(id, &owner, bytes);
}
fn s2_net_send_to(scope, args, _rv) {         // __s2_net_send_to(id, host, port, data)
    // id, host = arg1 string, port = arg2 as u16, bytes = js_bytes_arg(scope, arg3); crate::net::send_to(id, &owner, host, port, bytes);
}
fn s2_net_close(scope, args, _rv) { /* mirror s2_ws_close -> crate::net::close(id, &owner) */ }
fn s2_net_on(scope, args, _rv) {  /* mirror s2_ws_on EXACTLY: is_owner gate via crate::net::is_owner; NET_EVENT_MUX subscribe key "<id>:<event>" */ }
```

- [ ] **Step 5: The mux + pending queue.** Add thread_locals beside `WS_EVENT_MUX`/`WS_EVENT_PENDING` (~499):

```rust
    static NET_EVENT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
    static NET_EVENT_PENDING: std::cell::RefCell<Vec<(u64, PendingNetEvent)>> = std::cell::RefCell::new(Vec::new());
```
and the enum (top-level):
```rust
enum PendingNetEvent { Data(Vec<u8>), Datagram { from: String, data: Vec<u8> }, Closed, Errored(String) }
```

- [ ] **Step 6: `resolve_net_connect`** — copy `resolve_ws_connect` (v8host.rs:2372) verbatim (resolves with the conn-id `Number` on Ok, rejects on Err; the owner-liveness DROP preamble is identical).

- [ ] **Step 7: `dispatch_pending_net_events`** — copy `dispatch_pending_ws_events` (2460) structure (snapshot / `try_borrow_mut` / per-sub liveness + context clone + TryCatch + WARN + the terminal-`close` prune), but build args per `PendingNetEvent`:
  - `Data(b)` → key `"<id>:data"`, args `[bytes_to_uint8array(tc, &b)]`.
  - `Datagram{from,data}` → key `"<id>:message"`, args `[from_obj, bytes_to_uint8array(...)]` where `from_obj` = `{host, port}` parsed from the `"host:port"` string (split on the last `:`).
  - `Errored(e)` → key `"<id>:error"`, args `[String(e)]`.
  - `Closed` → key `"<id>:close"`, args `[]`; **and prune** `"<id>:{data,message,error,close}"` after fan-out (the ws terminal-close prune).

- [ ] **Step 8: The `frame_async_drain` routing** — after the ws `while let` loop (~7500), add (mirrors it):

```rust
        while let Some(sig) = crate::net::try_recv_signal() {
            match sig.kind {
                crate::net::NetSignalKind::Connected | crate::net::NetSignalKind::Bound => {
                    if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                        PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                        resolve_net_connect(host, &entry, sig.conn_id, Ok(()));
                    }
                }
                crate::net::NetSignalKind::ConnectFailed(e) => {
                    if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                        PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                        resolve_net_connect(host, &entry, sig.conn_id, Err(e));
                    }
                    crate::net::drop_conn(sig.conn_id);
                }
                crate::net::NetSignalKind::Data(b)      => NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Data(b)))),
                crate::net::NetSignalKind::Datagram{from,data} => NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Datagram{from,data}))),
                crate::net::NetSignalKind::Errored(e)   => NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Errored(e)))),
                crate::net::NetSignalKind::Closed => {
                    NET_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, PendingNetEvent::Closed)));
                    crate::net::drop_conn(sig.conn_id);
                }
            }
        }
```

- [ ] **Step 9: Wire the rest** — register the 6 natives (beside `__s2_ws_*` ~5559); the `Resource::NetConn(h)` teardown arm (beside `WsConn` — `crate::net::drop_conn(h)`); the shutdown reset (`NET_EVENT_MUX`→new, `NET_EVENT_PENDING`→clear, beside 7298); the teardown `NET_EVENT_MUX.remove_by_owner(id)` (beside 7594); and in `core/src/ffi.rs` after `frame_async_drain()`: `v8host::dispatch_pending_net_events();` (beside `dispatch_pending_ws_events()`).

- [ ] **Step 10: Build + full suite.**

Run: `cd core && cargo test && bash scripts/check-core-boundary.sh`
Expected: PASS (existing suite + Task 1's net tests; the natives compile). Boundary green.

- [ ] **Step 11: Commit.**

```bash
git add core/src/plugin.rs core/src/v8host.rs core/src/ffi.rs
git commit -m "feat(net): V8 natives + Uint8Array marshalling + resolve/dispatch + NetConn ledger"
```

---

## Task 3: `@s2script/net` prelude + `packages/net` + changeset

**Files:**
- Modify: `core/src/v8host.rs` (the `@s2script/net` prelude, beside `__s2pkg_ws` ~1834)
- Create: `packages/net/package.json`, `packages/net/index.d.ts`, `.changeset/net-sockets.md`

**Interfaces:**
- Consumes: Task 2's `__s2_net_*` natives.
- Produces (JS): `globalThis.__s2pkg_net = { Net: {...} }`.

- [ ] **Step 1: Add the `@s2script/net` prelude** (mirror the `__s2pkg_ws` wrapper at 1834; note `send`/`sendTo` pass `data` UNCHANGED — the native handles Uint8Array|string — do NOT `String(data)`):

```javascript
  globalThis.__s2pkg_net = {
    Net: {
      connectTcp: function (host, port) {
        return __s2_net_tcp_connect(String(host), port | 0).then(function (id) {
          return {
            onData:  function (h) { __s2_net_on(id, "data", function (b) { h(b); }); },
            onClose: function (h) { __s2_net_on(id, "close", function () { h(); }); },
            onError: function (h) { __s2_net_on(id, "error", function (e) { h(e); }); },
            send:    function (data) { __s2_net_send(id, data); },
            close:   function () { __s2_net_close(id); },
          };
        });
      },
      udp: function () {
        return __s2_net_udp_bind().then(function (id) {
          return {
            onMessage: function (h) { __s2_net_on(id, "message", function (from, b) { h(from, b); }); },
            sendTo:    function (host, port, data) { __s2_net_send_to(id, String(host), port | 0, data); },
            close:     function () { __s2_net_close(id); },
          };
        });
      },
    },
  };
```

- [ ] **Step 2: Create `packages/net/package.json`** (mirror `packages/ws/package.json`; version `0.1.0` — a new package):

```json
{
  "name": "@s2script/net",
  "version": "0.1.0",
  "types": "index.d.ts",
  "publishConfig": { "access": "public" },
  "files": ["index.d.ts"],
  "repository": { "type": "git", "url": "https://github.com/GabeHirakawa/s2script.git" }
}
```

- [ ] **Step 3: Create `packages/net/index.d.ts`:**

```typescript
/** @s2script/net — engine-generic raw TCP + UDP sockets (binary), off the game thread. */
export interface TcpSocket {
  send(data: Uint8Array | string): void;
  onData(handler: (bytes: Uint8Array) => void): void;
  onClose(handler: () => void): void;
  onError(handler: (err: string) => void): void;
  close(): void;
}
export interface UdpSocket {
  sendTo(host: string, port: number, data: Uint8Array | string): void;
  onMessage(handler: (from: { host: string; port: number }, bytes: Uint8Array) => void): void;
  close(): void;
}
export declare const Net: {
  /** Connect a TCP client. Rejects on connect failure. */
  connectTcp(host: string, port: number): Promise<TcpSocket>;
  /** Bind a UDP socket on an ephemeral local port. */
  udp(): Promise<UdpSocket>;
};
```

- [ ] **Step 4: Create the changeset** `.changeset/net-sockets.md`:

```markdown
---
"@s2script/net": patch
---

New `@s2script/net` package: raw TCP + UDP client sockets (binary `Uint8Array`), off the game thread over the shared async runtime. `Net.connectTcp(host, port)` (send/onData/onClose/onError) and `Net.udp()` (sendTo/onMessage) — unblocks A2S server queries, IRC, custom protocols.
```

- [ ] **Step 5: Verify.**

Run: `bash scripts/check-plugins-typecheck.sh && cd core && cargo test`
Expected: typecheck green (the new package resolves; a demo added in Task 4 will consume it); core PASS.

- [ ] **Step 6: Commit.**

```bash
git add core/src/v8host.rs packages/net .changeset/net-sockets.md
git commit -m "feat(net): @s2script/net prelude + packages/net types + changeset"
```

---

## Task 4: demo + live gate

**Files:**
- Create: `examples/net-demo/{package.json,tsconfig.json,src/plugin.ts}`

**Interfaces:**
- Consumes: `@s2script/net`, `@s2script/frame` (the non-blocking proof).

- [ ] **Step 1: Write the demo** (`examples/net-demo/src/plugin.ts`; mirror `examples/clients-demo` for package.json/tsconfig — `@demo/net-demo`, `s2script.apiVersion "1.x"`). It (a) TCP-connects to a public HTTP server and reads the raw response, and (b) UDP-A2S-queries our own CS2 server (handling the challenge), proving both binary round-trips + non-blocking:

```typescript
// net-demo — proves @s2script/net end-to-end: a TCP round-trip to a public HTTP server + a UDP A2S
// query to our own CS2 server (challenge handshake), and that the game frame advances during both.
import { Net } from "@s2script/net";
import { OnGameFrame } from "@s2script/frame";

let frames = 0;
OnGameFrame.subscribe(() => { frames++; });
const dec = (b: Uint8Array) => Array.from(b).map((c) => String.fromCharCode(c)).join("");

async function tcp(): Promise<void> {
  try {
    const before = frames;
    const s = await Net.connectTcp("example.com", 80);
    s.onData((b) => {
      const line = dec(b).split("\r\n")[0];
      console.log(`[net-demo] TCP example.com:80 -> "${line}" frames+=${frames - before}`);
      s.close();
    });
    s.onError((e) => console.log(`[net-demo] TCP error: ${e}`));
    s.send("GET / HTTP/1.0\r\nHost: example.com\r\n\r\n");
  } catch (e) { console.log(`[net-demo] TCP connect failed: ${e}`); }
}

// A2S_INFO query: 0xFFFFFFFF 'T' "Source Engine Query\0"
const A2S_INFO = new Uint8Array([0xff,0xff,0xff,0xff,0x54, ...Array.from("Source Engine Query").map(c=>c.charCodeAt(0)), 0x00]);
async function a2s(): Promise<void> {
  try {
    const before = frames;
    const u = await Net.udp();
    u.onMessage((_from, b) => {
      const header = b[4];
      if (header === 0x41) {                     // S2C_CHALLENGE — resend with the 4-byte challenge
        const q = new Uint8Array(A2S_INFO.length + 4);
        q.set(A2S_INFO, 0); q.set(b.slice(5, 9), A2S_INFO.length);
        u.sendTo("127.0.0.1", 27015, q);
      } else if (header === 0x49) {              // A2S_INFO reply — name is the null-terminated string after byte 6
        let i = 6, name = "";
        while (i < b.length && b[i] !== 0) { name += String.fromCharCode(b[i]); i++; }
        console.log(`[net-demo] UDP A2S self-query -> server="${name}" frames+=${frames - before}`);
        u.close();
      }
    });
    u.sendTo("127.0.0.1", 27015, A2S_INFO);
  } catch (e) { console.log(`[net-demo] UDP failed: ${e}`); }
}

export function onLoad(): void {
  console.log("[net-demo] onLoad — TCP + UDP");
  tcp();
  a2s();
}
```

- [ ] **Step 2: Typecheck + core + sniper build.**

```bash
bash scripts/check-plugins-typecheck.sh && (cd core && cargo test) && \
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: typecheck green; core PASS; sniper OK (GLIBC floors unchanged; `net.rs`/marshalling add no deps).

- [ ] **Step 3: Deploy + live gate.** (build-sniper wipes `dist/addons/s2script`.)

```bash
bash scripts/build-base-plugins.sh
node packages/cli/dist/cli.js build examples/net-demo
mkdir -p dist/addons/s2script/configs dist/addons/s2script/data && chmod 777 dist/addons/s2script/configs dist/addons/s2script/data
cp plugins/*/dist/_s2script_*.s2sp examples/net-demo/dist/*.s2sp dist/addons/s2script/plugins/
rm -f dist/addons/s2script/plugins/_s2script_zones-lib.s2sp
(cd docker && docker compose restart cs2)
```

- [ ] **Step 4: Verify the live gate.** In `docker logs s2script-cs2` (after the boot window):
  - `[net-demo] TCP example.com:80 -> "HTTP/1.0 200 OK" frames+=<N>` (N > 0 = non-blocking; a real TCP binary round-trip over the internet).
  - `[net-demo] UDP A2S self-query -> server="s2script-slice0" frames+=<N>` (the UDP binary round-trip incl. the challenge handshake, against our own server).
  - `GAMEDATA n/0`, `RestartCount=0`, no panic.

- [ ] **Step 5: Commit.**

```bash
git add examples/net-demo docs/superpowers/plans/2026-07-12-net-sockets.md
git commit -m "feat(net): net-demo (TCP round-trip + UDP A2S self-query) + live gate"
```

---

## Deferred (do NOT build ahead)

- A listener/server (inbound accept); a host/port allowlist / permissions gate; TLS-over-raw-TCP (use `fetch`/`ws`); per-socket backpressure / bounded send queues; connect/send timeouts; Unix sockets; multicast; `SO_*` options.
