# fetch / HTTP Primitive — Design

**Status:** Approved (brainstorm), ready for the plan.
**Slice:** the async HTTP `fetch` primitive — the "talk to the outside world" I/O capability, and the async-network-I/O foundation for websockets + raw sockets.

## Goal

Give plugins a web-parity `fetch(url, options) → Promise<Response>` that performs HTTP/HTTPS requests **fully off the game thread**, scaling to **arbitrary concurrency** (operator-controlled: `players × plugins-that-callout-on-connect`) without ever blocking the tick. Engine-generic core primitive + an `@s2script/http` module.

## Motivation & context

HTTP is the single biggest ecosystem unlock after the DB: permissions services, webhooks, the Steam Web API, Discord relays, remote stats/ban backends — the whole class SM plugins get from SteamWorks/RIPext. Two hard constraints shape it:

1. **It must be truly async.** Unlike the DB primitive (sync-behind-a-Promise, fine because SQLite is sub-ms), a network round-trip is 10ms–seconds; running it inline would stutter the tick. Today the threadpool resolver only ever resolves `undefined` — this slice builds the **async-result spine** (a background job carrying a typed payload back to resolve a Promise), reusable by every future async native.
2. **Concurrency is unbounded and latency-critical.** A map change on a full server reconnects ~everyone at once; each of N plugins may fire a callout on connect, and permissions fetches are on the critical path. So the engine must handle hundreds+ concurrent requests efficiently and resolve each as soon as *its own* round-trip completes — which rules out thread-per-request and calls for async I/O.

This is also the async-network foundation: **websockets** (`tokio-tungstenite`) and **raw `@s2script/net` sockets** (`tokio::net`) are named follow-ons that reuse this runtime + the cross-context event-delivery spine (`onCached`/client-events), differing only in shape (persistent/streaming vs one-shot).

## Scope

**In scope:** `core/src/http.rs` (a `tokio` runtime + `reqwest`), the async-result marshalling, `__s2_fetch`, `@s2script/http` (`fetch` + `Response`), a concurrency-proving live-gate demo.

**Deferred (named follow-ons):** websockets; raw `@s2script/net` TCP/UDP; binary request/response bodies (`ArrayBuffer`); per-plugin `maxConcurrent`/politeness; an SSRF allowlist / permissions gate; a dedicated-vs-shared runtime tuning knob.

## Architecture

One-way deps (game → core). HTTP is engine-generic → a core native + an engine-generic module; **no shim change** (`tokio`/`reqwest` live in core like `rusqlite`). Three layers:

1. **Core — `core/src/http.rs`.** A **`tokio` multi-threaded runtime** started once at core init on a small fixed set of background threads (~4 — async I/O multiplexes thousands of connections over few threads via `epoll`; **not** thread-per-request), plus **one shared `reqwest::Client`** (rustls TLS, connection pooling + keep-alive) reused across all fetches. The runtime and the client are internal — invisible to plugins; the main game thread never runs them. **Lifecycle:** the runtime is built at core init and dropped on `shutdown()` (in-flight requests are cancelled and the completion channel drained without resolving — safe on a re-init); a completion that arrives *after* its owning plugin unloaded/reloaded is dropped by the liveness guard (never resolved into a dead context), exactly like the timer/job path. The fetch-pending map is cleared on shutdown alongside `RESOLVERS`.
2. **The async-result spine.** `__s2_fetch(...)` (main thread) allocates a job id, creates a `PromiseResolver`, records it in a fetch-pending map keyed by id (the resolver `Global` + the owning plugin's `(id, generation)` tag), `spawn`s the request onto the runtime, and **returns the Promise instantly**. The tokio task runs the request, builds `Result<FetchResponse, String>` (`FetchResponse { status, statusText, headers, body }`), and **sends `(id, result)` down a thread-safe completion channel** (mpsc) — it never touches V8. The **frame drain** (Post, main thread) polls that channel; for each completion it removes the pending entry and — behind the existing **liveness guard** (drop, never resolve, if the plugin unloaded/reloaded) — builds the V8 `Response` and resolves the Promise, or rejects on the `Err`. This mirrors the timer/job resolve path (`resolve_or_drop`) but carries a payload; the pattern (background async work → completion channel → resolve-with-payload on the drain) is the reusable spine.
3. **`@s2script/http`** — the `fetch`/`Response` types + a thin prelude runtime over `__s2_fetch`.

## The API

```ts
const res = await fetch("https://api.example.com/perms", {
  method: "GET",                         // default "GET"
  headers: { authorization: "Bearer …" },
  body: "…",                             // string (JSON/form); binary deferred
  timeoutMs: 5000,                       // per-request; default 30000
});
```
- `interface FetchOptions { method?: string; headers?: Record<string,string>; body?: string; timeoutMs?: number; }`
- `interface Response { readonly status: number; readonly ok: boolean; readonly statusText: string; readonly headers: Record<string,string>; text(): string; json<T = unknown>(): T; }`
- The body is **buffered** (fully read on the tokio task) → `text()`/`json()` are **synchronous** (a deliberate, ergonomic deviation from web fetch's streaming `Promise` methods — nothing to gain over an already-fetched buffer). Body is **UTF-8 text** for the MVP (binary/`ArrayBuffer` deferred).
- Module: `import { fetch } from "@s2script/http"`.

## Error semantics (web-fetch parity — exactly right for the join critical path)

- **HTTP errors do NOT reject.** `403`/`404`/`500` **resolve** with `{ ok: false, status, … }` — a permissions API returning `403` is a *valid answer* (denied), not a failure; the plugin reads `res.status`, never lands in `catch`.
- **Only network-level failures reject** — DNS failure, connection refused, TLS error, and **timeout** — with an `Error` the plugin's `try/catch`/`.catch` handles (decide the fallback: deny / default / retry).
- **Timeout** is per-request (default 30s); on a latency-sensitive path the plugin sets it short (e.g. 5s). A hung request is cheap (an idle async task, no blocked thread) — an async-engine win.
- **Join pattern:** a plugin fetches in `Clients.onConnect`; the player connects immediately with a safe default while the request is in flight; on resolve the plugin applies the real result — the same async-apply pattern clientprefs uses for cookie loads. The framework keeps the gap tiny (tokio + pooling) and bounded (timeout).

## Concurrency & non-blocking (the core requirement)

- **The main thread never blocks on I/O** — it only does the instant `fetch()` hand-off and the instant resolve-on-drain. Every request runs on the runtime.
- **Arbitrary concurrency** — N concurrent fetches multiplex over the ~4 runtime threads via async I/O; each resolves as soon as *its own* round-trip finishes (the last player's permissions don't wait behind the first's). No framework-imposed cap.
- **Connection reuse** — the shared `reqwest::Client` pools TCP+TLS connections, so a burst of same-host fetches (a permissions API) skip repeated handshakes.

## Safety limits

- **Max response size** — the buffered body is capped (default ~10 MB); exceeding it rejects (prevents a huge/hostile response OOMing the isolate).
- **Timeout** — per-request, default 30s.
- **SSRF** — a plugin *can* reach internal URLs; for the MVP fetch is a **trusted-plugin capability** (the operator installs the plugin), with an allowlist/permissions gate deferred to the permissions system. Named, not blocked.

## Dependencies

New core deps: **`tokio`** (`rt-multi-thread`, `net`, `time`) and **`reqwest`** (`rustls-tls`, no default OpenSSL). A sizeable tree (compile time + binary size) — the honest cost of a foundational, must-be-performant I/O primitive; it's the standard Rust async-HTTP stack and is amortized across websockets + raw sockets later. Same "vendored, self-contained" posture as `rusqlite`/`hl2sdk`. One sniper rebuild.

## Testing & live gate

- **Core tests (`http.rs`):** fetch against a tiny **local test HTTP server** spun up in-test (a real round-trip through the runtime + the marshalling); a `404` resolves `ok:false`; a bad host / a short timeout **rejects**; the max-response-size cap rejects.
- **Live gate (bots-provable):** the demo fires **N concurrent fetches at a public API** (e.g. `https://httpbin.org/get`), logs each response status, **and logs the frame counter before/after** — proving N requests resolve while the tick keeps advancing (zero blocking) and concurrency works. One test proves the whole stack live: tokio + reqwest + TLS + marshalling + concurrency + non-blocking.
- **Gates:** core-boundary (`http.rs` engine-generic — no game names), typecheck (`@s2script/http`), full `cargo test`.

## Boundary & safety summary

`http.rs`, `__s2_fetch`, and `@s2script/http` are engine-generic (URLs + HTTP — Source2-generic). No shim change / no `S2EngineOps` op. The tokio runtime is an internal background engine; the game loop stays fully decoupled (submit + resolve-on-drain only). Both gates stay green.
