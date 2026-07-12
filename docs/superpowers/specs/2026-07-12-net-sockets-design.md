# `@s2script/net` — raw TCP + UDP sockets — design

**Date:** 2026-07-12
**Status:** approved (design)
**Slice:** net-sockets (the async-network category's raw-socket primitive)

## Goal

Give plugins **raw TCP and UDP sockets** over the shared tokio runtime, off the game thread — the last primitive in the async-network set (`fetch` + WebSocket + DB already ship). Client-only (connect/bind outbound; no inbound listener). Binary payloads (`Uint8Array`). Unblocks A2S game-server queries, IRC/Discord gateways, RCON-out, and arbitrary custom protocols — and, longer-term, community remote-DB drivers.

## Background — the pattern this mirrors

`core/src/ws.rs` (the WebSocket client) is the near-exact template: a process-global signal channel + an owner-scoped `HashMap<u64, Conn>` registry, each connection a tokio task on the SHARED runtime (`http::spawn`) doing `select!(read ↔ command)` and emitting `WsSignal`s the frame drain polls; connect resolves via the async-result resolver (`resolve_ws_connect`, with the owner-liveness DROP guard), and the event stream (`onMessage`/`onClose`/`onError`) fans out **post-drain**, HOST-free (`dispatch_pending_ws_events`). Owner-scoped `send`/`close`/`is_owner`/`drop_conn`; ledgered `Resource::WsConn`. `tokio`'s `net` feature is already enabled (the ws tests use `tokio::net::TcpListener`).

The one thing `@s2script/net` adds that ws didn't: **binary payloads** — ws carries `String`; a raw socket carries `Vec<u8>`, so the natives marshal `Uint8Array ↔ Vec<u8>`.

## Scope decisions (locked)

- **TCP + UDP** both, this slice.
- **Binary** (`Uint8Array`) core, with a `string`→UTF-8 convenience on send.
- **Client-only** — connect (TCP) / bind-ephemeral (UDP) outbound. No listener/accept.
- **Deferred:** a listener/server; a host/port allowlist (permissions); TLS-over-TCP (use `fetch`/`ws` for TLS); per-socket backpressure.

## A. The API — `@s2script/net`

**TCP** (stream — the ws shape):
```ts
Net.connectTcp(host: string, port: number): Promise<TcpSocket>   // rejects on connect failure
interface TcpSocket {
  send(data: Uint8Array | string): void;   // string → UTF-8 bytes
  onData(handler: (bytes: Uint8Array) => void): void;   // one call per read chunk
  onClose(handler: () => void): void;
  onError(handler: (err: string) => void): void;
  close(): void;
}
```

**UDP** (connectionless datagrams — A2S etc.):
```ts
Net.udp(): Promise<UdpSocket>              // binds an ephemeral local port (resolves once bound)
interface UdpSocket {
  sendTo(host: string, port: number, data: Uint8Array | string): void;
  onMessage(handler: (from: { host: string; port: number }, bytes: Uint8Array) => void): void;
  close(): void;
}
```

`onData`/`onMessage`/`onClose`/`onError` follow the ws additive-subscriber convention (register after the `await` resolves). Multiple handlers allowed.

## B. Core `core/src/net.rs` (one module, both protocols)

A near-copy of `ws.rs`. A process-global `Engine { sig_tx, sig_rx, conns }` (`OnceLock`); `conns: Mutex<HashMap<u64, Conn>>` where `Conn { cmd_tx, owner }`.

- **`NetSignal { conn_id, kind }`**, `kind`:
  - TCP: `Connected | Data(Vec<u8>) | Closed | Errored(String)`
  - UDP: `Bound | Datagram { from: String /* "host:port" */, data: Vec<u8> } | Errored(String)`
- **`connect_tcp(id, host, port, owner)`**: spawn a task → `tokio::net::TcpStream::connect((host, port))` (DNS resolves off-thread; failure → `Errored`/connect-reject) → emit `Connected` → `select!`:
  - `read` half: `read.read(&mut buf[0..64KB])` → 0 bytes = EOF → `Closed` + break; N bytes → `Data(buf[..n])`; err → `Errored` + `Closed` (browser-parity, like ws's read-error arm) + break.
  - `cmd_rx`: `Send(Vec<u8>)` → `write.write_all(&bytes)`; `Close` → emit `Closed` + break; `Shutdown` → break, NO signal (teardown, mirrors ws's `Close` vs `Shutdown` split so a late signal can't misroute onto a reused id).
- **`bind_udp(id, owner)`**: spawn a task → `tokio::net::UdpSocket::bind("0.0.0.0:0")` → emit `Bound` → `select!`:
  - `recv_from(&mut buf[0..64KB])` → `Datagram { from, data }`.
  - `cmd_rx`: `SendTo(host, port, Vec<u8>)` → resolve the addr + `send_to`; `Close` → break; `Shutdown` → break.
- Owner-scoped `send(id, owner, bytes)` / `send_to(id, owner, host, port, bytes)` / `close(id, owner)` / `is_owner(id, owner)` / `drop_conn(id)` / `try_recv_signal()` — identical shape to ws.

One module + one registry covers both (a `conn_id` is a TCP conn or a UDP socket; commands differ by kind).

## C. Binary marshalling + the async spine

The ONE new mechanism vs ws (which carried `String`):
- **JS → Rust (send):** the native reads arg as a `Uint8Array` → its backing byte slice → `Vec<u8>`; OR a `string` → UTF-8 bytes. A small `js_bytes_arg(scope, val) -> Vec<u8>` helper.
- **Rust → JS (data/datagram):** the fan-out builds a `Uint8Array` from the received `Vec<u8>` (`v8::ArrayBuffer::new_backing_store_from_bytes` → `v8::ArrayBuffer::with_backing_store` → `v8::Uint8Array::new`), passed to the handler. UDP additionally passes a `{host, port}` object.

Everything else reuses the proven spine **verbatim**:
- `connectTcp`/`udp` allocate a fresh async id used as BOTH the connect-resolver id and the `conn_id`; resolve via a `resolve_net_connect` (mirrors `resolve_ws_connect` — owner-liveness DROP guard, resolve with the conn-id `Number` on `Connected`/`Bound`, reject on `Errored`/connect-fail).
- `onData`/`onMessage`/`onClose`/`onError` queue to a `NET_EVENT_PENDING` and fan out **after `frame_async_drain()` in `ffi.rs`** via `dispatch_pending_net_events` (mirrors `dispatch_pending_ws_events`, HOST-free, per-sub liveness + TryCatch).
- New `frame_async_drain` loop: `while let Some(sig) = net::try_recv_signal() { route }` — `Connected`/`Bound`/`Errored`(on connect) → `resolve_net_connect`; `Data`/`Datagram`/`Closed`/`Errored`(mid-stream) → queue to `NET_EVENT_PENDING` (+ `drop_conn` + prune the conn's mux keys on the terminal `Closed`, like ws).
- **A 64 KB per-read chunk cap** (no unbounded buffering; a large TCP payload arrives as multiple `onData` chunks).

## D. Safety + packaging

- **Owner-scoped opaque integer handles** — `send`/`sendTo`/`close`/`on` verify `current_plugin` owns the id (a wrong owner no-ops); **no raw socket/fd crosses to JS**.
- **No inbound listener** — connect/bind-ephemeral outbound only, so no port is exposed/bound for accept.
- **Ledgered `Resource::NetConn(u64)`** → teardown `drop_conn`s the socket (+ `remove_by_owner` on the mux) even if `close()` was never called; a signal for an unloaded conn finds no resolver/live sub → dropped.
- **Outbound to an arbitrary host:port is allowed** for MVP (like `fetch`'s arbitrary URLs). A host/port allowlist is a permissions-system concern — deferred.
- **`packages/net` is a NEW types-only package** → this slice changes `packages/*`, so: **branch → PR → with a Changesets changeset** (`@s2script/net`, new package at its initial version — a `patch` under the current independent-versioning config). No shim change, no new `S2EngineOps` op (tokio in core; natives `set_native`'d). One sniper rebuild.

## E. Testing

**In-isolate (Rust `net.rs` tests, mirroring the ws tests):**
- TCP: connect → send → echo → close against a local echo listener spawned on the runtime; assert `Connected`, the echoed `Data`, `Closed`.
- UDP: bind → `send_to` a local echo → assert the `Datagram` round-trip.
- Owner-scoping (`send` wrong owner denied) + bad-host/port connect → `Errored`/reject.

**Live gate (self-contained — no external dependency):**
- A demo that **UDP-A2S-queries our own CS2 server**: `Net.udp()` → `sendTo("127.0.0.1", 27015, A2S_INFO)` (the `\xFF\xFF\xFF\xFFTSource Engine Query\0` packet) → `onMessage` parses the A2S_INFO reply (server name + map). Proves a UDP binary round-trip against a real Source server with zero external setup.
- Plus a TCP connect+send+echo to a public echo service (or a second local check), and the **game-frame-advances-during-I/O** non-blocking assertion (the fetch/ws demo pattern).
- Sniper rebuild; `GAMEDATA n/0`, no panic, `RestartCount=0`.

## Boundary check

`net.rs` + the natives are engine-generic (host/port/bytes — no CS2/game symbol). `@s2script/net` is an engine-generic prelude module. No shim change; no new op (tokio in core, like ws/http/sqlx). One sniper rebuild (core `.so`).

## Out of scope (do not build ahead)

- A listener/server (inbound accept — a much larger, separate thing).
- A host/port allowlist / permissions gate on outbound connections.
- TLS-over-raw-TCP (use `@s2script/fetch` or `@s2script/ws` when you need TLS).
- Per-socket backpressure / bounded send queues; `connect`/`send` timeouts (add if a use case needs them).
- Unix domain sockets; multicast; `SO_*` socket options.
