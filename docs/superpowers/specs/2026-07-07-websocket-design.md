# WebSocket Primitive — Design

**Status:** Approved (brainstorm — the `connect` API fork decided), ready for the plan.
**Slice:** a client WebSocket primitive — the first real-time capability, composing the tokio engine (fetch) with the cross-context event spine (`onCached`).

## Goal

Give plugins persistent, bidirectional WebSocket **client** connections — `WebSocket.connect(url) → Promise<WebSocket>` with `send`/`close` + `onMessage`/`onClose`/`onError` — running fully off the game thread. Unlocks real-time integrations: Discord gateway bots, live dashboards, push services.

## Motivation & context

fetch is request/response; a WebSocket is persistent, bidirectional, and streaming — the "push, not poll" capability where a game-server framework shines (a Discord bot reacting to gateway events in real time). It's the second consumer of the async-network foundation and the proof it *composes*: a WebSocket is literally **fetch's one-shot resolver (for the open handshake) + `onCached`'s post-drain mux (for the message stream)** — the two hardest pieces already exist. Only the protocol (`tokio-tungstenite`) and the per-connection glue are new.

## Scope

**In scope:** client `connect` (Promise, resolve-on-open) + `send` (text) + `close` + `onMessage`/`onClose`/`onError`; owner-scoped opaque handles; ledgered teardown; `wss://` (TLS); the `@s2script/ws` module; a bots-provable echo live-gate demo.

**Deferred (named follow-ons):** a WS **server** (listen for inbound — a much larger, separate thing); **binary** messages (text/UTF-8 MVP, like fetch's body); backpressure / bounded channels (unbounded for the MVP); a **per-connection early-message buffer** (the MVP requires subscribing `onMessage` synchronously after `await connect` — see below); subprotocols/extensions; automatic reconnection (the plugin's job); per-message send acknowledgement.

## Architecture

One-way deps (game → core). Engine-generic → a core native + `@s2script/ws`; **no shim change** (`tokio-tungstenite` in core). It reuses everything:

- **The shared tokio runtime** (the one `http.rs` built) — `http.rs` exposes a spawn accessor; ws tasks run on the same runtime (one async-network engine, not two).
- **`tokio-tungstenite`** (rustls/ring TLS → `wss://`) for the protocol.
- **Per connection, a tokio task:** connects (`connect_async`), then `select!`s between **reading** incoming frames and **writing** queued outgoing messages. Everything it produces is a **signal** on one channel the frame drain polls: `WsSignal { conn_id, kind: Connected | ConnectFailed(err) | Message(text) | Closed(code, reason) | Errored(err) }`.
- **A connection registry** `id → { outgoing: Sender<WsCommand>, owner }` (the `send`/`close` channel to the task + the owning plugin).

**The drain routes each signal to one of the two existing spines:**
- **`Connected` / `ConnectFailed`** → resolve/reject the **connect Promise** (the fetch async-result spine — a one-shot resolver keyed by conn id; `Connected` resolves with the `WebSocket` handle, `ConnectFailed` rejects). Reuses `resolve_fetch`/`RESOLVERS`/`record_job`/`PENDING_JOBS` discipline.
- **`Message` / `Closed` / `Errored`** → fan out to that connection's handlers (the `onCached` event spine — a `WS_EVENT_MUX` keyed by `(conn_id, event)`, drained + fanned out **after `frame_async_drain()`, HOST free**), carrying the payload (the text, or `code`+`reason`). Mirrors `dispatch_pending_cookie_cached`.

## The API

```ts
const ws = await WebSocket.connect("wss://gateway.discord.gg/?v=10&encoding=json"); // rejects on connect failure
ws.onMessage((data: string) => { /* each incoming text frame */ });   // additive
ws.onClose((code: number, reason: string) => { /* … */ });            // additive
ws.onError((err: string) => { /* … */ });                             // additive
ws.send("…");   // fire-and-forget text send (no ack Promise)
ws.close();     // initiate a clean close
```
- `WebSocket.connect(url) → Promise<WebSocket>` — resolves on the open handshake, **rejects on connect failure** (bad host, TLS, handshake error). Chosen for consistency with the Promise-based async surface (fetch/db/timers) + clean failure handling; the early-message gap is covered by core buffering (below).
- `onMessage`/`onClose`/`onError` are **additive** subscriptions (s2script's event style — multiple handlers, like `Clients.on*`), each firing in the plugin's context on the frame drain.
- `send(text)` is **fire-and-forget** (queued to the connection's tokio task; WS has no per-message ack).
- **Text/UTF-8** messages (binary deferred).
- Module: `import { WebSocket } from "@s2script/ws"`. The runtime wraps the connection id in a `WebSocket` handle object whose methods call the owner-scoped natives.

**Early-message ordering (why the common pattern is gap-free):** the guarantee comes from the drain *order*, not a buffer. Within a frame drain: (1) the `Connected` signal resolves the connect Promise in the fetch-completion loop; (2) the microtask checkpoint runs the connect `.then`, in which the plugin subscribes `onMessage` **synchronously**; (3) the `Message` fan-out runs **after** the checkpoint (post-drain, HOST free). So `onMessage` is subscribed before any message is fanned out, and a message can't precede its own `Connected` signal — no message is ever delivered to an unsubscribed connection **when the plugin subscribes synchronously right after `await connect`** (the documented, expected pattern — e.g. handling a Discord gateway `HELLO`). **Known limitation:** if a plugin instead `await`s something else *between* `connect` and `onMessage`, messages arriving in that gap are dropped (fanned out to zero subscribers). A per-connection buffer that holds messages until the first `onMessage` subscription is a named follow-up; the MVP documents "subscribe synchronously."

## Natives

All owner-scoped (`current_plugin` must own the conn — a guessable integer can't touch another plugin's socket):
- `__s2_ws_connect(url) → Promise<id>` — spawn the task; the `Connected` signal resolves with the id, `ConnectFailed` rejects. Ledgers a `Resource::WsConn(id)`.
- `__s2_ws_send(id, text)` — queue an outgoing text message to the conn's task.
- `__s2_ws_close(id)` — queue a close.
- `__s2_ws_on(id, event, handler)` — subscribe a handler to `(id, event)` in `WS_EVENT_MUX` (event ∈ `"message"`/`"close"`/`"error"`); ledgered as an event-sub.

## Safety, teardown, lifecycle

- **Owner-scoped:** `send`/`close`/`on` verify `current_plugin` owns the conn id (mirrors the DB owner-scoping); a wrong/unknown owner is a no-op.
- **Ledgered teardown:** `Resource::WsConn(id)` → on plugin unload, tell the task to close, remove the `(id,*)` mux subscribers + the connect resolver, and deregister the conn — **before the context disposes** (the drop-before-dispose discipline). The tokio task exits on the close command or when its outgoing channel drops.
- **Liveness:** a signal arriving for an unloaded/reloaded plugin's conn finds no resolver / no live subscribers → dropped (never resolved into a dead context), exactly the fetch/timer discipline.
- **Shutdown:** the connection registry + `WS_EVENT_MUX` + the connect-resolver entries clear on `shutdown()`; the shared runtime (process-global `OnceLock`) is not dropped; in-flight tasks finish and their signals are drained-and-dropped.
- **Backpressure:** unbounded channels for the MVP (a flood queues until the drain delivers) — bounded channels + a drop/close-on-overflow policy is a named follow-up.

## Dependencies

New core dep: **`tokio-tungstenite`** with a rustls feature (reuses our `tokio` + `ring`; no new TLS stack). Trial-built locally before the workflow (like `reqwest`). One sniper rebuild.

## Testing & live gate

- **Core tests (`ws.rs`):** spin a **tiny local WebSocket echo server** in-test (a `tokio-tungstenite` server task on an ephemeral port) → `connect` + `send("hi")` + receive the echoed `Message("hi")` + `close`; verify the signal flow (Connected → Message → Closed) and a connect-failure (`ConnectFailed` → reject) against a dead port.
- **In-isolate:** the `__s2_ws_*` natives + the `@s2script/ws` module driven against the local echo server (connect → `onMessage` → `send` → assert the echo arrives on a drain); owner-scoping (a second plugin can't `send` the conn).
- **Live gate (bots-provable):** the demo connects to a public WS echo service, `send`s a message, logs the echoed reply via `onMessage`, and logs the frame counter to prove the tick advanced while connected (non-blocking).
- **Gates:** core-boundary (`ws.rs` + `@s2script/ws` engine-generic — no game names), typecheck, full `cargo test`.

## Boundary & safety summary

`ws.rs`, the natives, and `@s2script/ws` are engine-generic (URLs + text frames — Source2-generic). No shim change / no `S2EngineOps` op. The tokio tasks run on the shared background runtime; the game loop stays decoupled (connect/send/close hand off; messages fan out on the drain). Both gates stay green.
