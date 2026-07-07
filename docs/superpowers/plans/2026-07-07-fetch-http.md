# fetch / HTTP Primitive — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or a Workflow). Steps use checkbox (`- [ ]`). Tasks are SEQUENTIAL and DEPENDENT — implement in order, commit each.

**Goal:** A web-parity `fetch(url, opts) → Promise<Response>` running fully off the game thread via an internal `tokio` + `reqwest` engine, scaling to arbitrary concurrency without blocking the tick.

**Architecture:** `core/src/http.rs` owns a process-global `tokio` runtime + a shared `reqwest::Client` and a completion channel. The `__s2_fetch` native reuses the `threadSleep` async plumbing (`RESOLVERS` + `record_job` + `PENDING_JOBS` + `refresh_detour`); a new frame-drain step drains `http::try_recv_completed()` and resolves each Promise with the built `Response` (the async-result spine). `@s2script/http` wraps it.

**Tech Stack:** Rust (`tokio` rt-multi-thread, `reqwest` rustls), rusty_v8, TypeScript.

## Global Constraints

- **Core engine-generic:** `core/src/http.rs`, the native, `@s2script/http` — NO game/CS2 names (`scripts/check-core-boundary.sh`).
- **The main thread never blocks on I/O.** `__s2_fetch` hands off to the runtime and returns instantly; the request runs on `tokio`; the Promise resolves on the frame drain. No blocking call on the game thread.
- **Degrade-never-crash:** the native `catch_unwind`; a request failure (network/timeout) REJECTS the Promise, an HTTP status (4xx/5xx) RESOLVES with `ok:false`; nothing panics into the engine.
- **No `S2EngineOps` op / no shim change** — `tokio`/`reqwest` live in core; the only wiring is `http::init()` at core init + one drain step + the native.
- **Async discipline (reuse threadSleep's):** an in-flight fetch increments `PENDING_JOBS` + `refresh_detour` (keeps the drain running) and is `record_job`-ledgered (teardown drops its `RESOLVERS` entry before the context disposes); a completion for an unloaded/reloaded plugin is DROPPED by the liveness guard, never resolved.
- **`cargo test` serial** (`.cargo/config.toml`) — do not change.
- Commit messages end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`; use a `-F -` heredoc (no backticks).

## File Structure
- `core/Cargo.toml` — add `tokio`, `reqwest`.
- `core/src/http.rs` (NEW) — the engine (runtime, client, channel, `fetch`, `try_recv_completed`, types) + a unit test.
- `core/src/lib.rs` (MODIFY) — `mod http;`.
- `core/src/v8host.rs` (MODIFY) — `http::init()` at core init; the `__s2_fetch` native + `resolve_fetch`; the fetch drain step in `frame_async_drain`; register the native; the `__s2pkg_http` prelude.
- `packages/http/{package.json,index.d.ts}` (NEW).
- `plugins/http-demo/{package.json,tsconfig.json,src/plugin.ts}` (NEW).

---

### Task 1: `core/src/http.rs` — the tokio + reqwest engine

**Files:** Modify `core/Cargo.toml`, `core/src/lib.rs`; Create `core/src/http.rs`.

**Interfaces — Produces:**
```rust
pub struct FetchRequest { pub method: String, pub url: String, pub headers: Vec<(String,String)>, pub body: Option<String>, pub timeout_ms: u64 }
pub struct FetchResponse { pub status: u16, pub status_text: String, pub headers: Vec<(String,String)>, pub body: String }
pub struct FetchCompletion { pub id: u64, pub result: Result<FetchResponse, String> }
pub fn init();                                   // build the process-global engine (idempotent)
pub fn fetch(id: u64, req: FetchRequest);        // spawn the request onto the runtime
pub fn try_recv_completed() -> Option<FetchCompletion>;  // the frame drain polls this
```

- [ ] **Step 1: Deps.** In `core/Cargo.toml` `[dependencies]`:
```toml
tokio = { version = "1", features = ["rt-multi-thread", "net", "time"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
```
(rustls-only — no system OpenSSL, matching `rusqlite bundled`. Pick versions that build.)

- [ ] **Step 2: Write `core/src/http.rs`:**
```rust
//! Engine-generic async HTTP engine: a process-global tokio runtime + a shared reqwest Client + a
//! completion channel. Holds NO V8 handles; the main thread only submits (`fetch`) and polls
//! (`try_recv_completed`) — the runtime does all network I/O off-thread. Mirrors async_rt's POOL:
//! a OnceLock, built once, never dropped (survives a Metamod re-init).
use std::sync::mpsc::{channel, Sender, Receiver};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub struct FetchRequest { pub method: String, pub url: String, pub headers: Vec<(String,String)>, pub body: Option<String>, pub timeout_ms: u64 }
pub struct FetchResponse { pub status: u16, pub status_text: String, pub headers: Vec<(String,String)>, pub body: String }
pub struct FetchCompletion { pub id: u64, pub result: Result<FetchResponse, String> }

const MAX_BODY: usize = 10 * 1024 * 1024; // 10 MB cap

struct Engine {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    tx: Sender<FetchCompletion>,
    rx: Mutex<Receiver<FetchCompletion>>,
}
static ENGINE: OnceLock<Engine> = OnceLock::new();

pub fn init() {
    ENGINE.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4).enable_all().build()
            .expect("tokio runtime");
        let client = reqwest::Client::builder().build().expect("reqwest client");
        let (tx, rx) = channel();
        Engine { runtime, client, tx, rx: Mutex::new(rx) }
    });
}

pub fn fetch(id: u64, req: FetchRequest) {
    let Some(e) = ENGINE.get() else { return }; // degrade: not initialized
    let client = e.client.clone();
    let tx = e.tx.clone();
    e.runtime.spawn(async move {
        let result = do_fetch(client, req).await;
        let _ = tx.send(FetchCompletion { id, result });
    });
}

pub fn try_recv_completed() -> Option<FetchCompletion> {
    ENGINE.get()?.rx.lock().ok()?.try_recv().ok()
}

async fn do_fetch(client: reqwest::Client, req: FetchRequest) -> Result<FetchResponse, String> {
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut rb = client.request(method, &req.url).timeout(Duration::from_millis(req.timeout_ms));
    for (k, v) in &req.headers { rb = rb.header(k.as_str(), v.as_str()); }
    if let Some(b) = req.body { rb = rb.body(b); }
    let resp = rb.send().await.map_err(|e| e.to_string())?; // network/timeout → Err
    let status = resp.status().as_u16();
    let status_text = resp.status().canonical_reason().unwrap_or("").to_string();
    let headers = resp.headers().iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect();
    if let Some(len) = resp.content_length() { if len as usize > MAX_BODY { return Err("response body too large".into()); } }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BODY { return Err("response body too large".into()); }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    Ok(FetchResponse { status, status_text, headers, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    // A tiny local HTTP/1.1 server on an ephemeral port; returns one canned response then exits.
    fn spawn_server(response: &'static str) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024]; let _ = s.read(&mut buf);
                let _ = s.write_all(response.as_bytes());
            }
        });
        port
    }
    fn drain_blocking(id: u64) -> FetchCompletion {
        for _ in 0..500 { if let Some(c) = try_recv_completed() { if c.id == id { return c; } } std::thread::sleep(Duration::from_millis(10)); }
        panic!("no completion");
    }
    #[test]
    fn fetch_local_server_ok() {
        init();
        let port = spawn_server("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        fetch(1, FetchRequest { method: "GET".into(), url: format!("http://127.0.0.1:{port}/"), headers: vec![], body: None, timeout_ms: 5000 });
        let c = drain_blocking(1);
        let r = c.result.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "hello");
    }
    #[test]
    fn fetch_404_resolves_not_rejects() {
        init();
        let port = spawn_server("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        fetch(2, FetchRequest { method: "GET".into(), url: format!("http://127.0.0.1:{port}/"), headers: vec![], body: None, timeout_ms: 5000 });
        let r = drain_blocking(2).result.unwrap(); // Ok, not Err
        assert_eq!(r.status, 404);
    }
    #[test]
    fn fetch_bad_host_rejects() {
        init();
        fetch(3, FetchRequest { method: "GET".into(), url: "http://127.0.0.1:1/".into(), headers: vec![], body: None, timeout_ms: 1000 });
        assert!(drain_blocking(3).result.is_err()); // connection refused / timeout → Err
    }
}
```
(NOTE: the tests use `try_recv_completed` cross-thread — fine, it's a std mpsc behind a Mutex. Adjust the `drain_blocking` timing if a slow CI needs longer.)

- [ ] **Step 3: `mod http;`** in `core/src/lib.rs`.

- [ ] **Step 4: Run tests.** `cargo test --manifest-path core/Cargo.toml http::` — the 3 `http::tests::*` green. First build compiles tokio+reqwest (slow, one-time). Full suite stays green.

- [ ] **Step 5: Commit** (`feat(http): tokio+reqwest async HTTP engine (core/src/http.rs)`).

---

### Task 2: The `__s2_fetch` native + the async-result drain step

**Files:** Modify `core/src/v8host.rs`.

**Interfaces — Consumes:** `http::{init,fetch,try_recv_completed}`, `FetchRequest`/`FetchResponse` (Task 1). **Produces (JS native):** `__s2_fetch(url, options) -> Promise<rawResponse>` where `rawResponse = { status, ok, statusText, headers, body }`.

**Mirror:** `s2_thread_sleep` (the native shape: resolver + `next_async_id` + `resolver_owner_tag` + `record_job` + `RESOLVERS` insert + `PENDING_JOBS` + `refresh_detour`, then submit); `resolve_or_drop` (the owner-context + liveness resolve); the pool-completion loop in `frame_async_drain` (the drain step); `s2_sqlite_query`'s object-building (the `Response` object).

- [ ] **Step 1: Init the engine.** At core `init()` (where the pool/other subsystems are set up — near `set_engine_ops`/the natives registration), call `crate::http::init();`.

- [ ] **Step 2: The `__s2_fetch` native.** Parse `url` (arg0) + `options` (arg1: `method`/`headers`/`body`/`timeoutMs`) into a `FetchRequest`; then mirror `s2_thread_sleep`'s resolver/ledger/pending block exactly, but call `crate::http::fetch(id, req)` instead of `pool().submit(...)`:
```rust
fn s2_fetch(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let url = args.get(0).to_rust_string_lossy(scope);
        // parse options (defaults GET / no headers / no body / 30000ms):
        let mut method = "GET".to_string(); let mut headers = Vec::new(); let mut body = None; let mut timeout_ms = 30000u64;
        if let Ok(opts) = v8::Local::<v8::Object>::try_from(args.get(1)) {
            if let Some(v) = get_str_prop(scope, opts, "method") { method = v; }
            if let Some(v) = get_str_prop(scope, opts, "body") { body = Some(v); }
            if let Some(k) = v8::String::new(scope, "timeoutMs") { if let Some(v) = opts.get(scope, k.into()) { if v.is_number() { timeout_ms = v.integer_value(scope).unwrap_or(30000).max(0) as u64; } } }
            if let Some(k) = v8::String::new(scope, "headers") { if let Some(hv) = opts.get(scope, k.into()) {
                if let Ok(ho) = v8::Local::<v8::Object>::try_from(hv) {
                    if let Some(names) = ho.get_own_property_names(scope, Default::default()) {
                        for i in 0..names.length() {
                            let key = names.get_index(scope, i).unwrap();
                            let val = ho.get(scope, key).unwrap();
                            headers.push((key.to_rust_string_lossy(scope), val.to_rust_string_lossy(scope)));
                        }
                    }
                }
            }}
        }
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let id = next_async_id();
        let owner = resolver_owner_tag(scope);
        if let Some((ref oid, _)) = owner {
            REGISTRY.with(|r| { if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_job(id); } });
        }
        RESOLVERS.with(|m| m.borrow_mut().insert(id, ResolverEntry { owner, resolver: v8::Global::new(scope.as_ref(), resolver) }));
        PENDING_JOBS.with(|c| c.set(c.get() + 1));
        crate::http::fetch(id, crate::http::FetchRequest { method, url, headers, body, timeout_ms });
        refresh_detour();
        rv.set(promise.into());
    }));
}
```
(Add a small `get_str_prop(scope, obj, name) -> Option<String>` helper if one doesn't exist. Verify the exact rusty_v8 calls — `get_own_property_names`, `to_rust_string_lossy` — against neighboring natives.)

- [ ] **Step 3: `resolve_fetch`** — mirror `resolve_or_drop` (the owner-context + liveness block) but build the raw Response object (or reject on `Err`):
```rust
fn resolve_fetch(host: &mut Host, entry: &ResolverEntry, result: Result<crate::http::FetchResponse, String>) {
    // ... COPY resolve_or_drop's owner-liveness + context-clone + HandleScope/ContextScope preamble
    //     to get `scope` + the localized `resolver` (drop on a failed liveness check) ...
    match result {
        Ok(r) => {
            let obj = v8::Object::new(scope);
            set_obj_num(scope, obj, "status", r.status as f64);
            set_obj_bool(scope, obj, "ok", (200..300).contains(&r.status));
            set_obj_str(scope, obj, "statusText", &r.status_text);
            let hobj = v8::Object::new(scope);
            for (k, v) in &r.headers { let kk = v8::String::new(scope, k).unwrap(); let vv = v8::String::new(scope, v).unwrap_or_else(|| v8::String::new(scope, "").unwrap()); hobj.set(scope, kk.into(), vv.into()); }
            let hk = v8::String::new(scope, "headers").unwrap(); obj.set(scope, hk.into(), hobj.into());
            set_obj_str(scope, obj, "body", &r.body);
            resolver.resolve(scope, obj.into());
        }
        Err(e) => { let msg = v8::String::new(scope, &e).unwrap(); let ex = v8::Exception::error(scope, msg); resolver.reject(scope, ex); }
    }
}
```
(Add tiny `set_obj_{num,bool,str}` helpers or inline. The `body`/`statusText`/header values use the `.unwrap_or_else(|| "")` fallback for oversized strings, as in `db_value_to_v8`.)

- [ ] **Step 4: The drain step.** In `frame_async_drain`, right AFTER the pool-completion loop (`while let Some((id, _res)) = pool().try_recv_completed() {...}`), add:
```rust
// Resolve completed fetch requests (payload-carrying, from the tokio engine).
while let Some(c) = crate::http::try_recv_completed() {
    let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&c.id)) else { continue };
    PENDING_JOBS.with(|c2| c2.set(c2.get().saturating_sub(1)));
    resolve_fetch(host, &entry, c.result);
}
```

- [ ] **Step 5: Register the native.** Near the timer/async native registrations: `set_native(scope, global_obj, "__s2_fetch", s2_fetch);`.

- [ ] **Step 6: In-isolate test.** In `frame_tests`, spin a local test server (reuse the `http::tests` server helper or inline one), load a plugin that does `__s2_fetch("http://127.0.0.1:PORT/", {}).then(r => { globalThis.__out = r.status + ":" + r.body; })`, then `frame_async_drain()` in a loop (a few times, with a tiny sleep) until `__out` is set, and assert `"200:hello"`. (The response arrives async — poll the drain up to ~50 times with a 10ms sleep.)

- [ ] **Step 7: Tests.** `cargo test --manifest-path core/Cargo.toml` — green.

- [ ] **Step 8: Commit** (`feat(http): __s2_fetch native + async-result drain step`).

---

### Task 3: `@s2script/http` — types + prelude runtime

**Files:** Create `packages/http/{package.json,index.d.ts}`; Modify `core/src/v8host.rs` (the `__s2pkg_http` prelude + an in-isolate test).

- [ ] **Step 1: `packages/http/package.json`:** `{ "name": "@s2script/http", "version": "0.1.0", "types": "index.d.ts" }`
- [ ] **Step 2: `packages/http/index.d.ts`:**
```ts
/** @s2script/http — async HTTP. NO runtime code (injected as __s2pkg_http). */
export interface FetchOptions { method?: string; headers?: Record<string, string>; body?: string; timeoutMs?: number; }
export interface Response {
  readonly status: number; readonly ok: boolean; readonly statusText: string;
  readonly headers: Record<string, string>;
  text(): string;
  json<T = unknown>(): T;
}
/** Perform an HTTP request off the game thread. Rejects on a network error / timeout; an HTTP
 *  status (incl. 4xx/5xx) RESOLVES with ok=false. */
export declare function fetch(url: string, options?: FetchOptions): Promise<Response>;
```
- [ ] **Step 3: The prelude.** In `core/src/v8host.rs` near `__s2pkg_db`/`__s2pkg_cookies`, add:
```js
// --- @s2script/http: fetch over __s2_fetch (adds text()/json() over the buffered body) ---
globalThis.__s2pkg_http = {
  fetch: function (url, options) {
    return __s2_fetch(String(url), options || {}).then(function (raw) {
      return {
        status: raw.status, ok: raw.ok, statusText: raw.statusText, headers: raw.headers,
        text: function () { return raw.body; },
        json: function () { return JSON.parse(raw.body); },
      };
    });
  },
};
```
(`fetch` is the named export → `import { fetch } from "@s2script/http"` resolves to `__s2pkg_http.fetch`.)
- [ ] **Step 4: In-isolate test.** A plugin `var { fetch } = require("@s2script/http")`; `fetch("http://127.0.0.1:PORT/").then(r => globalThis.__out = r.status + ":" + r.text())`; poll `frame_async_drain`; assert `"200:hello"`. (Reuse the Task-2 local-server helper.)
- [ ] **Step 5: Tests + typecheck.** `cargo test …`; a `@s2script/http` typecheck is validated by the demo (Task 4).
- [ ] **Step 6: Commit** (`feat(http): @s2script/http module (fetch + Response)`).

---

### Task 4: `http-demo` plugin (the concurrency + non-blocking live gate)

**Files:** Create `plugins/http-demo/{package.json,tsconfig.json,src/plugin.ts}`. Mirror `plugins/reservedslots/` for package.json/tsconfig.

- [ ] **Step 1: `package.json`:** `{ "name": "@demo/http-demo", "version": "0.1.0", "main": "src/plugin.ts", "s2script": { "apiVersion": "1.x" } }`
- [ ] **Step 2: `tsconfig.json`** — copy `plugins/reservedslots/tsconfig.json`.
- [ ] **Step 3: `src/plugin.ts`:**
```ts
// http-demo — proves fetch works, is concurrent, and never blocks the tick. Fires N concurrent
// requests at a public API and logs how many resolved; a frame handler proves the tick advanced
// throughout (the fetches did not stall the game).
import { fetch } from "@s2script/http";
import { OnGameFrame } from "@s2script/frame";

let frames = 0;

export async function onLoad(): Promise<void> {
  OnGameFrame.subscribe(() => { frames++; });
  const startFrames = frames;
  const N = 10;
  console.log("[http-demo] firing " + N + " concurrent fetches (frames=" + startFrames + ")");
  const results = await Promise.all(
    Array.from({ length: N }, (_unused, i) =>
      fetch("https://httpbin.org/get?i=" + i, { timeoutMs: 15000 })
        .then((r) => r.status)
        .catch((e) => "ERR:" + String(e))
    )
  );
  const ok = results.filter((s) => s === 200).length;
  // a single-request detail: status + a body snippet
  let detail = "";
  try { const r = await fetch("https://httpbin.org/get", { timeoutMs: 15000 }); detail = r.status + " len=" + r.text().length; }
  catch (e) { detail = "ERR:" + String(e); }
  console.log("[http-demo] " + ok + "/" + N + " ok; tick advanced " + (frames - startFrames) + " frames during the fetches; single=" + detail);
}

export function onUnload(): void { console.log("[http-demo] onUnload"); }
```
- [ ] **Step 4: Build.** `node packages/cli/dist/cli.js build plugins/http-demo` — typecheck passes + a `.s2sp`.
- [ ] **Step 5: Commit** (`feat(http): http-demo (concurrency + non-blocking live gate)`).

---

## Post-implementation (controller / me — NOT a workflow task)
1. **Sniper rebuild** (compiles tokio+reqwest — slow the first time). 2. **Deploy** (copy `.s2sp`, restart). 3. **Live gate (bots-provable):** `[http-demo] firing 10 concurrent fetches`; then `[http-demo] 10/10 ok; tick advanced N frames during the fetches; single=200 len=…` — N > 0 proves the tick never stalled while 10 HTTPS requests were in flight (concurrency + non-blocking + TLS + marshalling, all live). No crash, gamedata 11/0. 4. **Gates:** boundary, full `cargo test`, plugins-typecheck. 5. **Final opus review** → merge + push.

## Self-review notes
- **Spec coverage:** engine (T1), native + async-result spine (T2), module (T3), the concurrency/non-blocking live-gate demo (T4). Websockets, raw sockets, binary bodies, maxConcurrent, SSRF allowlist deferred (in no task). ✓
- **Type consistency:** `FetchRequest`/`FetchResponse`/`FetchCompletion` (Rust) ↔ `http::fetch`/`try_recv_completed` ↔ `__s2_fetch` raw `{status,ok,statusText,headers,body}` ↔ the `__s2pkg_http` wrapper adding `text()`/`json()` ↔ `Response` (`.d.ts`). `RESOLVERS`+`record_job`+`PENDING_JOBS` reused; the new drain step + `resolve_fetch` mirror the pool loop + `resolve_or_drop`.
