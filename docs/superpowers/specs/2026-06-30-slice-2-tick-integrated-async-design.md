# Slice 2 — Tick-Integrated Async — Design Spec

- **Project:** s2script (TypeScript plugin framework for Source 2; SourceMod's spiritual successor)
- **Date:** 2026-06-30
- **Status:** Approved design, ready for implementation planning
- **Builds on:** Slice 0 (V8 in CS2) + Slice 1 (multiplexer + OnGameFrame detour), both merged to `main`.
- **Scope:** Slice 2 only — tick-integrated async on the single shared context. See `docs/ARCHITECTURE.md` §2.1.

---

## 1. Purpose & what it proves

Own the microtask checkpoint so `await` resolves at controlled **frame boundaries** and never preempts mid-tick; provide the primitives `Delay(ms)` / `NextTick()` / `NextFrame()`; and prove the one place s2script crosses threads — genuinely blocking work runs off the main thread and marshals back as a resolved Promise on the next frame drain — with a demo threaded op. Prove `await Delay(1000)` does not block the tick.

All engine-generic → `core` (SourceMod model). Single shared V8 context (context-per-plugin is Slice 4). A notable consequence of grounding in the code: **the C++ shim and the C ABI header do not change** — the entire slice is `core/` Rust, and almost all of it is `cargo test`-verifiable.

## 2. Decided directions

1. **Explicit microtask policy.** The isolate is currently created with `CreateParams::default()` (policy `kAuto` — V8 auto-runs microtasks after each callback). Slice 2 switches it to **`kExplicit`**: microtasks (the `await`/`.then` continuations) run **only** when *we* call the checkpoint, once per frame. Existing synchronous tests are unaffected.
2. **Minimal real threadpool** (confirmed over per-op spawn): a fixed-size worker pool + a job channel + an mpsc completion channel. It's the durable design Slice 5's SQL/HTTP/file ops reuse verbatim, and barely more code than per-op spawn.
3. **The per-frame drain runs on the Post `dispatch_game_frame`**, independent of whether any `onGameFrame` handler is subscribed.
4. **The lazy-detour predicate extends to async:** the `GameFrame` detour stays installed while `(onGameFrame subscribers > 0) OR (async pending > 0)`.

## 3. The async runtime — `core/src/async_rt.rs` (new; mostly V8-free → unit-testable)

Mirrors the `multiplexer.rs`/`v8host.rs` split: pure logic here, V8 promise glue in `v8host`.

- **Threadpool (fully V8-free).** A fixed-size pool (`N` workers, default 4) created **once per process** via a `Once`-guarded process-global — it persists across `shutdown`/re-init on the resident cdylib, exactly like the V8 platform. Jobs are `Box<dyn FnOnce() -> JobResult + Send>`; each worker loops receiving jobs, runs them, and sends `(JobId, JobResult)` over an **mpsc completion channel**. The channel is created at first init: the `Sender` goes into the process-global pool (cloned to workers), the `Receiver` lives on the main thread. Unit-testable with no V8: submit a job, drain the completion channel, assert the result.
- **Timer queue (V8-free).** Pending entries keyed by an `Instant` deadline (`Delay`) or a frame-count target (`NextTick`/`NextFrame`), each carrying an opaque `TimerId`. `due(now: Instant, frame: u64) -> Vec<TimerId>` returns the entries ready to fire and removes them. Clock = `std::time::Instant` sampled on the main thread each frame. A monotonic `frame` counter is incremented once per drain.
- **Pending-async count** = timers-in-flight + jobs-in-flight; drives the lazy-detour predicate (§4).

**Testability seam:** `async_rt` never holds a V8 handle — it deals in `JobId`/`TimerId` and plain `JobResult` data. The `TimerId`/`JobId` → `Global<PromiseResolver>` mapping lives in `v8host` (§4). This keeps the threadpool + timer logic unit-testable without a running V8/engine.

## 4. The V8 layer — `core/src/v8host.rs` (promise glue + the drain)

- **`init`:** `isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit)`.
- **Primitives installed at context creation** (alongside `console`/`onGameFrame`). Each: creates a `v8::PromiseResolver`, stores the `Global<PromiseResolver>` in a main-thread map keyed by the `TimerId`/`JobId`, registers the timer/job with `async_rt`, increments pending-async, updates the lazy-detour (§ below), and returns the Promise (`resolver.get_promise`).
- **The frame drain** — `frame_async_drain()`, called by the ffi `dispatch_game_frame` on **Post**, **always** (independent of the handler snapshot), under the HOST context/scope:
  1. `try_recv` all completed jobs from the completion `Receiver` → resolve each mapped promise with its result;
  2. resolve all `due()` timers → resolve each mapped promise (`undefined`);
  3. increment the frame counter;
  4. **`perform_microtask_checkpoint()`** — the single point where the `await`/`.then` continuations run.
  Any promise resolved in steps 1–2 has its continuation run in step 4.
- **Combined lazy-detour.** A single predicate `desired = (multiplexer enabled subscriptions > 0) || (async pending > 0)`, tracked in `v8host`. On any change — `onGameFrame` subscribe/unsubscribe (the Slice-1 `DetourChange` path) **or** an async submit/completion — recompute `desired`; if it flipped, call `request_hook("OnGameFrame", 1|0)`. This layers over Slice 1's per-descriptor lazy-detour so `Delay(1000)` with no `onGameFrame` handler installs the detour, and it's removed only when **both** the subscriber count and the pending-async count reach zero.
- **`shutdown`** resets the per-isolate async state (the resolver maps, the timer queue, the pending count) alongside the existing `FRAME`/`HOST` reset. The process-global threadpool + completion channel persist (like the platform). Pending resolvers are dropped (their promises simply never resolve — the isolate is being torn down).
- **Folded-in Slice-1 Minor:** the `onGameFrame` invoker logs a named warning when a JS handler returns an out-of-range `HookResult` (previously silently mapped to `Continue`).

## 5. Primitive semantics (provisional globals, like Slice 1's `onGameFrame`)

- **`Delay(ms)`** → Promise resolving at the first frame drain where `elapsed_since_call ≥ ms`. A **cooperative timer** — never blocks a thread.
- **`NextTick()`** → resolves at the next frame drain (soonest resume); ≈ `Delay(0)`.
- **`NextFrame()`** → resolves at the drain of the *following* frame (frame-counter target = current + 1); distinct from `NextTick` when queued before the current frame's drain.
- **`threadSleep(ms)`** (the demo threaded op) → a **genuinely blocking** `std::thread::sleep(ms)` submitted to the **worker pool**, resolving on the first frame drain after the worker finishes. This is the Slice-2 stand-in proving the cross-thread marshal; the real off-thread ops (SQL/HTTP/file) and their general framework are Slice 5. Named to make explicit that it blocks a *worker* thread, unlike `Delay`.

All four return real Promises; `await` works because the continuation runs at the frame checkpoint. These globals are provisional (the typed `@s2script/std` async API is Slice 5), consistent with Slice 1's provisional `onGameFrame`.

## 6. No shim / no C ABI change

The shim already calls `s2script_core_dispatch_game_frame(Pre/Post)` each frame and installs/removes the detour via `request_hook`. Slice 2 adds the drain **inside** the core's Post handling and extends the **core-side** lazy-detour predicate. Therefore `shim/src/*`, `shim/CMakeLists.txt`, and `shim/include/s2script_core.h` are **untouched**. The entire slice is `core/` Rust. (This is why Slice 2 carries even lower engine-coupling risk than Slice 1.)

## 7. Testing strategy

- **Unit (`cargo test`, no V8):** the threadpool (submit a job → receive its result on the completion channel; N-worker concurrency); the timer queue (`due()` returns entries in deadline/frame order and removes them; a `Delay` deadline not yet reached is not due).
- **Integration (`cargo test` + V8, `--test-threads=1`):**
  - `kExplicit`: a `Promise.resolve().then(set flag)` does **not** set the flag until a `frame_async_drain` runs.
  - `Delay(ms)`: a drain before the deadline leaves the promise pending; a drain after resolves it (drive the clock via real elapsed time or a small sleep).
  - `NextTick`/`NextFrame` resolve at the expected drain (frame-counter based).
  - Threaded-op marshal: `threadSleep(x).then(set flag)` → drive frame drains in a loop until the worker completes → the flag is set (the continuation ran on the main thread).
  - **Non-blocking:** repeated `frame_async_drain` calls before a `Delay` deadline return promptly and the awaiting JS has not resumed; after the deadline it resumes — proving `await Delay(...)` doesn't block the main thread.
  - Lazy-detour: an async submit with no `onGameFrame` subscribers requests `install`; the last completion requests `remove`; a submit while a subscriber exists requests nothing.
- **Live (sniper build + Docker, operator-run by Claude):** a boot demo evaluating `await Delay(1000)` (log before/after) + a `threadSleep`, on a real CS2 server: the frame counter keeps advancing while awaiting (the tick is not blocked), the `Delay` continuation fires ~1s later, the `threadSleep` continuation resumes on the main thread, no crash. Reuses `scripts/build-sniper.sh`, the Docker harness (the 64 GB `cs2-data` copy is in place), and `scripts/rcon.py`.

## 8. Acceptance criteria

1. `cargo test -p s2script-core -- --test-threads=1` passes (the new `async_rt` unit tests + the V8-integration async tests + all Slice 0/1 tests); `make check-boundary` stays green; sniper build produces loadable binaries.
2. The isolate uses explicit microtask policy; Promise continuations run only at `frame_async_drain`.
3. `Delay(ms)` resolves at/after its deadline; `NextTick`/`NextFrame` at the expected drain.
4. The demo threaded op runs off the main thread and its Promise resolves on a subsequent frame drain (cross-thread marshal proven).
5. `await Delay(1000)` does not block: the frame drain is non-blocking while the promise is pending; on a live server the frame counter advances throughout the await.
6. The lazy-detour keeps `GameFrame` installed while async is pending (even with no `onGameFrame` subscribers) and removes it when both counts reach zero.
7. Reproduces from the README (sniper build + Docker runbook + the async demo).

## 9. Out of scope (Slice 2)

Real off-thread ops (SQL/HTTP/file) and the general native-async-op registration framework — Slice 5; per-plugin async accounting and cancel-pending-async-on-unload with per-continuation liveness guards — Slice 4 (this slice's async is process-global, no plugin identity); `setTimeout`/`setInterval` and the typed `@s2script/std` async API — Slice 5; threadpool backpressure / dynamic resizing (fixed size this slice); context-per-plugin. Note later needs as TODOs and stop.

## 10. File structure / deliverables

- `core/src/async_rt.rs` (new) — the V8-free threadpool + timer queue + pending-count, with unit tests.
- `core/src/lib.rs` (modify) — `mod async_rt;`.
- `core/src/v8host.rs` (modify) — `kExplicit` policy; install `Delay`/`NextTick`/`NextFrame`/`threadSleep`; the `TimerId`/`JobId` → `Global<PromiseResolver>` maps; `frame_async_drain`; the combined lazy-detour; the out-of-range `HookResult` warning; the `shutdown` async reset.
- `core/src/ffi.rs` (modify) — `dispatch_game_frame` on Post also calls `frame_async_drain`.
- `shim/`, `shim/include/s2script_core.h`: **unchanged**.
- README (modify) — the async demo in the live runbook + Slice 2 acceptance.
- Sniper build + Docker live gate + `scripts/rcon.py` reused.

## 11. Open items to validate during implementation

- The exact rusty_v8 (v8 149.4.0) APIs: `Isolate::set_microtasks_policy` / `MicrotasksPolicy::Explicit`, `PromiseResolver::new`/`get_promise`/`resolve`, and `perform_microtask_checkpoint` (on the scope or isolate) — confirm against the installed crate; the existing `v8host.rs` scope construction is the reference.
- Whether the boot demo's `await` needs the detour installed *before* the first frame — the async submit during Load requests the install synchronously, and the shim installs on `request_hook`, so the first Post frame after Load runs the drain. Confirm the ordering (Load runs the demo eval → `Delay` submit → `request_hook(install)` → first `GameFrame` Post → drain) on the live server.
- That switching to `kExplicit` doesn't strand any microtasks the existing Slice-0/1 synchronous paths implicitly relied on (they are synchronous; expected to be unaffected — verify the full suite stays green).
