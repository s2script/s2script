# WebSocket Primitive — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or a Workflow). Steps use checkbox (`- [ ]`). Tasks are SEQUENTIAL and DEPENDENT — implement in order, commit each.

**Goal:** A client WebSocket (`@s2script/ws`): `connect(url) → Promise<WebSocket>` + `send`/`close` + `onMessage`/`onClose`/`onError`, over the shared tokio runtime + `tokio-tungstenite`, fully off the game thread.

**Architecture:** `core/src/ws.rs` reuses `http.rs`'s tokio runtime (a new `http::spawn` accessor). Per connection a tokio task connects (`connect_async`) and `select!`s read/write, emitting `WsSignal`s down a channel. The frame drain routes signals: `Connected`/`ConnectFailed` resolve/reject the connect Promise (the fetch resolver spine); `Message`/`Closed`/`Errored` fan out to per-connection handlers (the `onCached` mux spine, post-drain). Owner-scoped, ledgered.

**Tech Stack:** Rust (`tokio-tungstenite` rustls, `tokio::sync::mpsc`), rusty_v8, TypeScript.

## Global Constraints
- **Core engine-generic:** `core/src/ws.rs`, the natives, `@s2script/ws` — NO game/CS2 names (`scripts/check-core-boundary.sh`).
- **Main thread never blocks:** `connect`/`send`/`close` hand off to the runtime (unbounded tokio mpsc `send` for commands is non-blocking, non-async) and return; messages fan out on the frame drain. No `.await`/blocking on the game thread.
- **Reuse the async spines:** `Connected`/`ConnectFailed` → resolve/reject via the fetch resolver path (`RESOLVERS`/`record_job`/`PENDING_JOBS`; a `resolve_ws_connect` mirroring `resolve_fetch` but resolving with the conn-id `Number`). `Message`/`Closed`/`Errored` → a `WS_EVENT_MUX` (keyed `"<id>:<event>"`) + a `WS_EVENT_PENDING` queue fanned out **after `frame_async_drain()`, HOST free** (mirroring `dispatch_pending_cookie_cached`), carrying the payload.
- **Ordering (load-bearing):** the connect-Promise resolve happens INSIDE the drain (before the microtask checkpoint, so the plugin subscribes `onMessage` this frame); the message fan-out happens AFTER the drain (post-checkpoint). Never fan out a message before the checkpoint.
- **Owner-scoped + degrade:** `send`/`close`/`on` verify `current_plugin` owns the conn (no-op otherwise); `catch_unwind` every native; a signal for an unloaded plugin's conn drops (no resolver / no live subscriber).
- **No `S2EngineOps` op / no shim change.** `cargo test` serial. Commit messages end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`; `-F -` heredoc, no backticks.

## File Structure
- `core/Cargo.toml` — add `tokio-tungstenite`, `futures-util` (for `SinkExt`/`StreamExt`).
- `core/src/http.rs` — add `pub fn spawn<F>(f: F)` (share the runtime).
- `core/src/ws.rs` (NEW) — the ws engine (registry, signal channel, per-connection task) + a unit test.
- `core/src/lib.rs` — `mod ws;`.
- `core/src/v8host.rs` — the `__s2_ws_*` natives + `resolve_ws_connect` + the signal-drain step + `WS_EVENT_MUX`/`WS_EVENT_PENDING` + `dispatch_pending_ws_events` + teardown (`Resource::WsConn`) + the `__s2pkg_ws` prelude.
- `core/src/ffi.rs` — call `dispatch_pending_ws_events()` after `frame_async_drain()`.
- `core/src/plugin.rs` — `Resource::WsConn(u64)` + `record_ws_conn`.
- `packages/ws/{package.json,index.d.ts}` (NEW).
- `plugins/ws-demo/{package.json,tsconfig.json,src/plugin.ts}` (NEW).

---

### Task 1: `core/src/ws.rs` — the tokio side (connection task + signals)

**Files:** Modify `core/Cargo.toml`, `core/src/lib.rs`, `core/src/http.rs`; Create `core/src/ws.rs`.

**Interfaces — Produces:**
```rust
pub enum WsSignalKind { Connected, ConnectFailed(String), Message(String), Closed(u16, String), Errored(String) }
pub struct WsSignal { pub conn_id: u64, pub kind: WsSignalKind }
pub fn connect(conn_id: u64, url: String, owner: String);  // spawn the task; registers the conn
pub fn send(conn_id: u64, owner: &str, text: String) -> bool;  // owner-checked; false if not owned/absent
pub fn close(conn_id: u64, owner: &str) -> bool;
pub fn drop_conn(conn_id: u64);  // teardown: close + deregister regardless of owner
pub fn try_recv_signal() -> Option<WsSignal>;
```

- [ ] **Step 1: Deps.** `core/Cargo.toml`:
```toml
tokio-tungstenite = { version = "0.29", default-features = false, features = ["rustls-tls-webpki-roots", "connect"] }
futures-util = { version = "0.3", default-features = false, features = ["sink"] }
```
(Verified to build locally with our tokio+ring.)

- [ ] **Step 2: `http::spawn`.** In `core/src/http.rs`, add:
```rust
/// Spawn a future on the shared tokio runtime (used by ws.rs to reuse the one runtime). No-op if uninitialized.
pub fn spawn<F>(future: F) where F: std::future::Future<Output = ()> + Send + 'static {
    if let Some(e) = ENGINE.get() { e.runtime.spawn(future); }
}
```

- [ ] **Step 3: `core/src/ws.rs`:**
```rust
//! Engine-generic WebSocket client engine. Per connection, a tokio task (on the SHARED http runtime)
//! connects + select!s read/write and emits WsSignals down a channel the frame drain polls. Holds NO
//! V8 handles. Registry maps a conn id -> the outgoing command sender + the owning plugin.
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::sync::{Mutex, OnceLock};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

pub enum WsSignalKind { Connected, ConnectFailed(String), Message(String), Closed(u16, String), Errored(String) }
pub struct WsSignal { pub conn_id: u64, pub kind: WsSignalKind }
enum WsCommand { Send(String), Close }

struct Conn { cmd_tx: tokio::sync::mpsc::UnboundedSender<WsCommand>, owner: String }
struct Engine { sig_tx: Sender<WsSignal>, sig_rx: Mutex<Receiver<WsSignal>>, conns: Mutex<HashMap<u64, Conn>> }
static ENGINE: OnceLock<Engine> = OnceLock::new();
fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| { let (sig_tx, sig_rx) = channel(); Engine { sig_tx, sig_rx: Mutex::new(sig_rx), conns: Mutex::new(HashMap::new()) } })
}

pub fn connect(conn_id: u64, url: String, owner: String) {
    let e = engine();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<WsCommand>();
    e.conns.lock().unwrap().insert(conn_id, Conn { cmd_tx, owner });
    let sig_tx = e.sig_tx.clone();
    crate::http::spawn(async move {
        let stream = match tokio_tungstenite::connect_async(&url).await {
            Ok((s, _resp)) => s,
            Err(err) => { let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::ConnectFailed(err.to_string()) }); return; }
        };
        let _ = sig_tx.send(WsSignal { conn_id, kind: WsSignalKind::Connected });
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
    let e = engine(); let map = e.conns.lock().unwrap();
    match map.get(&conn_id) { Some(c) if c.owner == owner => c.cmd_tx.send(WsCommand::Send(text)).is_ok(), _ => false }
}
pub fn close(conn_id: u64, owner: &str) -> bool {
    let e = engine(); let map = e.conns.lock().unwrap();
    match map.get(&conn_id) { Some(c) if c.owner == owner => { let _ = c.cmd_tx.send(WsCommand::Close); true } _ => false }
}
/// Teardown / post-close deregister — closes regardless of owner (the ledger owns the id).
pub fn drop_conn(conn_id: u64) {
    if let Some(c) = engine().conns.lock().unwrap().remove(&conn_id) { let _ = c.cmd_tx.send(WsCommand::Close); }
}
pub fn try_recv_signal() -> Option<WsSignal> { engine().sig_rx.lock().ok()?.try_recv().ok() }

#[cfg(test)]
mod tests {
    use super::*;
    // Uses a local echo server on the http runtime. Requires http::init() for the shared runtime.
    fn echo_server_port() -> u16 {
        crate::http::init();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(false).unwrap();
        let std_listener = listener;
        crate::http::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                    let (mut w, mut r) = ws.split();
                    while let Some(Ok(m)) = r.next().await {
                        if m.is_close() { break; }
                        if w.send(m).await.is_err() { break; }
                    }
                }
            }
        });
        port
    }
    fn drain_for(kinds: usize) -> Vec<WsSignal> {
        let mut out = Vec::new();
        for _ in 0..500 { while let Some(s) = try_recv_signal() { out.push(s); } if out.len() >= kinds { break; } std::thread::sleep(std::time::Duration::from_millis(10)); }
        out
    }
    #[test]
    fn connect_send_echo_close() {
        let port = echo_server_port();
        connect(1, format!("ws://127.0.0.1:{port}/"), "p".into());
        // wait for Connected, then send, then expect the echo
        let mut got_connected = false; let mut echo = None;
        for _ in 0..500 {
            while let Some(s) = try_recv_signal() { match s.kind {
                WsSignalKind::Connected => { got_connected = true; send(1, "p", "hi".into()); }
                WsSignalKind::Message(t) => { echo = Some(t); }
                _ => {}
            }}
            if echo.is_some() { break; }
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
        for _ in 0..200 { if try_recv_signal().is_some() { break; } std::thread::sleep(std::time::Duration::from_millis(10)); }
        assert!(!send(3, "pB", "x".into())); // wrong owner
        close(3, "pA");
    }
}
```

- [ ] **Step 4: `mod ws;`** in `core/src/lib.rs`. Run `cargo test --manifest-path core/Cargo.toml ws::` — the 3 `ws::tests::*` green (first build compiles tokio-tungstenite). Full suite green.
- [ ] **Step 5: Commit** (`feat(ws): tokio-tungstenite connection engine (core/src/ws.rs)`).

---

### Task 2: The `__s2_ws_*` natives + signal routing + teardown

**Files:** Modify `core/src/plugin.rs`, `core/src/v8host.rs`, `core/src/ffi.rs`.

**Interfaces — Consumes:** `ws::{connect,send,close,drop_conn,try_recv_signal}` (Task 1). **Produces (JS natives):** `__s2_ws_connect(url)->Promise<id>`, `__s2_ws_send(id,text)`, `__s2_ws_close(id)`, `__s2_ws_on(id,event,handler)`.

**Mirror:** `s2_fetch` (the connect native: resolver + `next_async_id` + `record_*` + `RESOLVERS` + `PENDING_JOBS` + `refresh_detour`, then `ws::connect`); `resolve_fetch` (→ `resolve_ws_connect`, resolve with a `Number`); `s2_cookie_on_cached` (the `__s2_ws_on` subscribe, keyed `"<id>:<event>"`) + `dispatch_pending_cookie_cached` (→ `dispatch_pending_ws_events`, with a payload).

- [ ] **Step 1: Ledger.** `core/src/plugin.rs`: `Resource::WsConn(u64)` + `pub fn record_ws_conn(&mut self, id: u64) { self.order.push(Resource::WsConn(id)); }`.

- [ ] **Step 2: Statics.** In `v8host.rs` near `COOKIE_CACHED_*`:
```rust
static WS_EVENT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>> = std::cell::RefCell::new(crate::event_mux::EventMux::new());
// (conn_id, event, payload1, payload2) queued during the drain, fanned out post-drain (HOST free).
static WS_EVENT_PENDING: std::cell::RefCell<Vec<(u64, String, String, i32)>> = std::cell::RefCell::new(Vec::new());
```
(For `message`/`error` the 3rd tuple field is the text and the 4th is unused; for `close` the 3rd is the reason and the 4th is the code.)

- [ ] **Step 3: `__s2_ws_connect`.** Mirror `s2_fetch` but: `let id = next_async_id();` used as BOTH the connect resolver id (in `RESOLVERS`) AND the conn id; `record_ws_conn(id)`; `ws::connect(id, url, owner_string)`. Owner string = `current_plugin(scope).unwrap_or_default()`.

- [ ] **Step 4: `resolve_ws_connect`.** Copy `resolve_fetch`'s owner-liveness + scope preamble, but resolve with the conn id (`resolver.resolve(scope, v8::Number::new(scope, id as f64).into())`) on `Ok`, or `reject` with the error string. (Signature: `resolve_ws_connect(host, entry, id: u64, result: Result<(), String>)`.)

- [ ] **Step 5: `__s2_ws_send` / `__s2_ws_close`.** `catch_unwind`; parse `id` (Number) + (send) `text` (String); `owner = current_plugin`; call `ws::send(id, &owner, text)` / `ws::close(id, &owner)` (owner-checked in Task 1). No return value needed.

- [ ] **Step 6: `__s2_ws_on`.** Mirror `s2_cookie_on_cached`: parse `id` + `event` (String) + `handler`; owner = `current_plugin`; subscribe into `WS_EVENT_MUX` under the key `format!("{id}:{event}")` (owner-tagged, ledgered as an event-sub like `s2_cookie_on_cached` does).

- [ ] **Step 7: The signal-drain step.** In `frame_async_drain`, after the fetch-completion loop, drain `ws::try_recv_signal()` and ROUTE (HOST borrowed here — connect resolves inline; events queue):
```rust
while let Some(sig) = crate::ws::try_recv_signal() {
    match sig.kind {
        crate::ws::WsSignalKind::Connected => {
            if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                resolve_ws_connect(host, &entry, sig.conn_id, Ok(()));
            }
        }
        crate::ws::WsSignalKind::ConnectFailed(e) => {
            if let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&sig.conn_id)) {
                PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
                resolve_ws_connect(host, &entry, sig.conn_id, Err(e));
            }
            crate::ws::drop_conn(sig.conn_id);
        }
        crate::ws::WsSignalKind::Message(t) => WS_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, "message".into(), t, 0))),
        crate::ws::WsSignalKind::Errored(e) => WS_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, "error".into(), e, 0))),
        crate::ws::WsSignalKind::Closed(code, reason) => {
            WS_EVENT_PENDING.with(|q| q.borrow_mut().push((sig.conn_id, "close".into(), reason, code as i32)));
            crate::ws::drop_conn(sig.conn_id);
            // (mux subscribers for this conn are cleaned up when the plugin unloads; a closed conn's
            // stale subscribers simply never fire again — acceptable. Optionally remove_by key here.)
        }
    }
}
```

- [ ] **Step 8: `dispatch_pending_ws_events`.** Mirror `dispatch_pending_cookie_cached` but per-`(conn_id, event)` key and pass the payload. For each pending `(conn_id, event, s, n)`: snapshot `WS_EVENT_MUX` for key `format!("{conn_id}:{event}")`; fan out (HOST free, per-sub liveness + scope + TryCatch) calling `handler(...)` — for `"message"`/`"error"` a single String arg `s`; for `"close"` two args `(Number n, String s)` (code, reason). `pub(crate) fn`.

- [ ] **Step 9: `ffi.rs`.** After `frame_async_drain()` (and the cookie dispatch): `v8host::dispatch_pending_ws_events();`.

- [ ] **Step 10: Teardown + shutdown + register.** In `unload_plugin`'s teardown walk, add `plugin::Resource::WsConn(id) => { crate::ws::drop_conn(*id); }`. In `unload_plugin` also `WS_EVENT_MUX.remove_by_owner(id)` (near `COOKIE_CACHED_MUX.remove_by_owner`). In `shutdown()`: reset `WS_EVENT_MUX` + clear `WS_EVENT_PENDING` (near the cookie resets). Register the 4 natives (`set_native`).

- [ ] **Step 11: In-isolate test.** Spin the local echo server (reuse the `ws::tests` helper or inline), load a plugin that `__s2_ws_connect(...).then(id => { __s2_ws_on(id,"message",m=>globalThis.__out=m); __s2_ws_send(id,"hi"); })`, drive `frame_async_drain()` + `dispatch_pending_ws_events()` in a poll loop, assert `__out === "hi"`.
- [ ] **Step 12: Tests.** `cargo test …` green. **Step 13: Commit** (`feat(ws): __s2_ws_* natives + signal routing (connect resolver + event mux)`).

---

### Task 3: `@s2script/ws` — types + prelude runtime

**Files:** Create `packages/ws/{package.json,index.d.ts}`; Modify `core/src/v8host.rs` (the `__s2pkg_ws` prelude + an in-isolate test).

- [ ] **Step 1: `packages/ws/package.json`:** `{ "name": "@s2script/ws", "version": "0.1.0", "types": "index.d.ts" }`
- [ ] **Step 2: `packages/ws/index.d.ts`:**
```ts
/** @s2script/ws — client WebSocket. NO runtime code (injected as __s2pkg_ws). */
export interface WebSocket {
  onMessage(handler: (data: string) => void): void;
  onClose(handler: (code: number, reason: string) => void): void;
  onError(handler: (err: string) => void): void;
  send(data: string): void;
  close(): void;
}
export declare const WebSocket: {
  /** Connect to a WebSocket server (wss:// for TLS). Resolves on the open handshake; rejects on connect failure. */
  connect(url: string): Promise<WebSocket>;
};
```
- [ ] **Step 3: The prelude** (near `__s2pkg_http`):
```js
globalThis.__s2pkg_ws = {
  WebSocket: {
    connect: function (url) {
      return __s2_ws_connect(String(url)).then(function (id) {
        return {
          onMessage: function (h) { __s2_ws_on(id, "message", function (m) { h(m); }); },
          onClose:   function (h) { __s2_ws_on(id, "close", function (code, reason) { h(code, reason); }); },
          onError:   function (h) { __s2_ws_on(id, "error", function (e) { h(e); }); },
          send:      function (data) { __s2_ws_send(id, String(data)); },
          close:     function () { __s2_ws_close(id); },
        };
      });
    },
  },
};
```
- [ ] **Step 4: In-isolate test** (module-level, against the local echo server): `var { WebSocket } = require("@s2script/ws"); WebSocket.connect("ws://127.0.0.1:PORT/").then(ws => { ws.onMessage(m => globalThis.__out = m); ws.send("hi"); })`; poll the drain + `dispatch_pending_ws_events`; assert `"hi"`.
- [ ] **Step 5: Tests + commit** (`feat(ws): @s2script/ws module (WebSocket handle)`).

---

### Task 4: `ws-demo` plugin (echo live gate)

**Files:** Create `plugins/ws-demo/{package.json,tsconfig.json,src/plugin.ts}` (mirror `plugins/reservedslots/`).

- [ ] **Step 1-2:** `package.json` (`@demo/ws-demo`) + `tsconfig.json` (copy reservedslots).
- [ ] **Step 3: `src/plugin.ts`:**
```ts
// ws-demo — connects to a public WebSocket echo service, sends a message, logs the echoed reply, and
// logs the frame counter to prove the connection didn't block the tick.
import { WebSocket } from "@s2script/ws";
import { OnGameFrame } from "@s2script/frame";

let frames = 0;

export async function onLoad(): Promise<void> {
  OnGameFrame.subscribe(() => { frames++; });
  const start = frames;
  try {
    const ws = await WebSocket.connect("wss://ws.postman-echo.com/raw");
    ws.onMessage((data) => {
      console.log("[ws-demo] echo=" + data + "; tick advanced " + (frames - start) + " frames while connecting/echoing");
      ws.close();
    });
    ws.onClose((code, reason) => console.log("[ws-demo] closed code=" + code + " reason=" + reason));
    ws.onError((e) => console.log("[ws-demo] error=" + e));
    ws.send("hello-from-s2script");
    console.log("[ws-demo] connected + sent (frames=" + frames + ")");
  } catch (e) {
    console.log("[ws-demo] connect ERROR: " + String(e));
  }
}

export function onUnload(): void { console.log("[ws-demo] onUnload"); }
```
- [ ] **Step 4: Build** (`node packages/cli/dist/cli.js build plugins/ws-demo`). **Step 5: Commit** (`feat(ws): ws-demo (echo live gate)`).

---

## Post-implementation (controller / me)
1. **Sniper rebuild** (compiles tokio-tungstenite). 2. **Deploy** (mkdir dirs, copy `.s2sp`, restart). 3. **Live gate (bots-provable):** `[ws-demo] connected + sent`; then `[ws-demo] echo=hello-from-s2script; tick advanced N frames while connecting/echoing` (N>0 = non-blocking; the echo round-trip proves connect+send+onMessage over wss:// live) + `[ws-demo] closed …`. No crash, gamedata 11/0. 4. **Gates:** boundary, full `cargo test`, plugins-typecheck. 5. **Final opus review** → merge + push.

## Self-review notes
- **Spec coverage:** engine (T1), natives + signal routing + teardown (T2), module (T3), the echo live-gate demo (T4). Server/binary/backpressure/early-message-buffer/reconnection deferred (in no task). ✓
- **Type consistency:** `WsSignalKind` (Rust) ↔ the drain routing ↔ `resolve_ws_connect` (Number) / the mux payload ↔ the natives ↔ the `WebSocket` handle (`.d.ts`). Connect reuses `RESOLVERS`/`record_ws_conn`/`PENDING_JOBS`; the event fan-out mirrors `dispatch_pending_cookie_cached` with a payload. `id` is both the connect-resolver id and the conn id.
