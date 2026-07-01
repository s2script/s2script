# Slice 2 — Tick-Integrated Async Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Own the V8 microtask checkpoint on a per-frame boundary so `await` resolves at frame edges (never mid-tick), provide `Delay`/`NextTick`/`NextFrame`, and prove the one off-main-thread → main-thread marshal with a demo `threadSleep` — all without blocking the tick.

**Architecture:** A V8-free async runtime (`core/src/async_rt.rs` — a minimal threadpool + a timer queue, mirroring the `multiplexer.rs` split) plus the V8 promise glue in `v8host.rs`. The isolate switches to explicit microtask policy; a per-frame drain (run on the Post `dispatch_game_frame`) resolves due timers + completed jobs, then runs the microtask checkpoint. The lazy `GameFrame` detour stays installed while `onGameFrame` subscribers OR pending async exist. Entirely `core/` Rust — the shim and C ABI are untouched.

**Tech Stack:** Rust (`v8 = 149.4.0`, `std::thread` + `std::sync::mpsc`, `std::time::Instant`), the sniper build (`scripts/build-sniper.sh`), Docker live gate (`joedwards32/cs2`) + `scripts/rcon.py`.

## Global Constraints

- **Entirely `core/` Rust — do NOT touch `shim/` or `shim/include/s2script_core.h`.** The shim already calls `s2script_core_dispatch_game_frame(Pre/Post)` each frame and installs/removes the detour via `request_hook`; Slice 2 adds behavior inside the core's Post path only.
- **`async_rt.rs` is V8-free** — it deals in `u64` async ids + plain `JobResult` data, never a V8 handle (the `id → Global<PromiseResolver>` map lives in `v8host`). This is the unit-testability seam.
- **Explicit microtask policy.** The isolate is switched to `MicrotasksPolicy::Explicit`; microtasks (await/`.then` continuations) run ONLY at `frame_async_drain`'s `perform_microtask_checkpoint`, once per frame.
- **The per-frame drain runs on Post, always** (independent of whether any `onGameFrame` handler is subscribed).
- **Combined lazy-detour:** the `GameFrame` detour is desired while `(onGameFrame enabled subscriptions > 0) OR (pending async > 0)`; request install/remove only on a transition.
- **The threadpool is created once per process** (persists across `shutdown`/re-init on the resident cdylib) with a fixed worker count (`N = 4`). Jobs are `Box<dyn FnOnce() -> JobResult + Send>`; workers send `(u64 id, JobResult)` over an mpsc completion channel read on the main thread.
- **Primitive semantics:** `Delay(ms)` resolves at the first drain where `elapsed_since_call ≥ ms`; `NextTick()` at the next drain; `NextFrame()` at the drain of the following frame (frame-counter + 1); `threadSleep(ms)` runs a blocking `std::thread::sleep` on a worker and resolves on the first drain after it finishes. All return real Promises. These are provisional globals (the typed `@s2script/std` API is Slice 5), like Slice 1's `onGameFrame`.
- **No panic crosses the FFI/V8-callback boundary** — every new `extern "C"` entry and V8 `FunctionCallback` stays `catch_unwind`-guarded (match the existing `console_log`/`s2_subscribe` pattern).
- **Build target = Steam Runtime sniper (glibc ≤ 2.31)** — verify loadable binaries via `scripts/build-sniper.sh`; the host `make` build is dev-only.
- **rusty_v8 API note:** confirm the exact `v8 = 149.4.0` signatures (`Isolate::set_microtasks_policy` / `MicrotasksPolicy::Explicit`, `PromiseResolver::new`/`get_promise`/`resolve`, `perform_microtask_checkpoint` on scope-or-isolate) against the installed crate; the existing `v8host.rs` (`eval`, `s2_subscribe`, the invoker) is the reference for the pinned scope/callback API.
- **Commits are signed and frequent.**

---

## File Structure

```
core/src/async_rt.rs   # NEW — V8-free: the threadpool (Pool) + the timer queue (TimerQueue) + unit tests
core/src/lib.rs        # MODIFY — `mod async_rt;`
core/src/v8host.rs     # MODIFY — kExplicit; Delay/NextTick/NextFrame/threadSleep primitives; the
                       #          id→Global<PromiseResolver> map; frame_async_drain; combined lazy-detour
                       #          (refresh_detour); shutdown async reset; out-of-range HookResult warning
core/src/ffi.rs        # MODIFY — dispatch_game_frame on Post also calls v8host::frame_async_drain
shim/, s2script_core.h # UNCHANGED
README.md              # MODIFY — async demo in the live runbook + Slice 2 acceptance
```

Task map: **T1** the Pool, **T2** the TimerQueue (both V8-free, TDD). **T3** kExplicit + the drain skeleton (microtask checkpoint) + ffi Post wiring + the out-of-range warning. **T4** `Delay`/`NextTick`/`NextFrame` + timer resolution in the drain + the combined lazy-detour. **T5** `threadSleep` + job resolution in the drain. **T6** the live gate + README. T1–T5 are `cargo test`-verifiable; T6 is operator-run.

---

## Task 1: The threadpool `Pool` (V8-free, TDD)

**Files:**
- Create: `core/src/async_rt.rs`
- Modify: `core/src/lib.rs` (add `mod async_rt;`)
- Test: inline `#[cfg(test)]` in `core/src/async_rt.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub type JobResult = Result<(), String>;         // Slice 2: sleep never errors → Ok(())
  pub type Job = Box<dyn FnOnce() -> JobResult + Send + 'static>;
  pub struct Pool { /* job_tx, completion_rx, workers */ }
  impl Pool {
      pub fn new(workers: usize) -> Self;
      pub fn submit(&self, id: u64, job: Job);              // runs `job` on a worker
      pub fn try_recv_completed(&self) -> Option<(u64, JobResult)>; // non-blocking
  }
  ```

- [ ] **Step 1: Write the failing test**

In `core/src/async_rt.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn pool_runs_job_off_thread_and_reports_completion() {
        let pool = Pool::new(2);
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        pool.submit(42, Box::new(move || { r2.store(true, Ordering::SeqCst); Ok(()) }));
        // Poll for completion (worker runs on another thread).
        let mut got = None;
        for _ in 0..1000 {
            if let Some(c) = pool.try_recv_completed() { got = Some(c); break; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (id, res) = got.expect("job never completed");
        assert_eq!(id, 42);
        assert!(res.is_ok());
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn try_recv_completed_is_nonblocking_when_empty() {
        let pool = Pool::new(1);
        assert!(pool.try_recv_completed().is_none()); // nothing submitted → immediate None
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core async_rt -- --test-threads=1`
Expected: FAIL — `Pool` doesn't exist (compile error; add `mod async_rt;` to `lib.rs` first).

- [ ] **Step 3: Implement `Pool`**

At the top of `core/src/async_rt.rs`:
```rust
//! Engine-generic, V8-free async runtime primitives: a fixed-size threadpool and a timer queue.
//! Holds NO V8 handles — jobs/timers carry a `u64` id that `v8host` maps to a PromiseResolver.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

pub type JobResult = Result<(), String>;
pub type Job = Box<dyn FnOnce() -> JobResult + Send + 'static>;

pub struct Pool {
    job_tx: Sender<(u64, Job)>,
    completion_rx: Receiver<(u64, JobResult)>,
    _workers: Vec<JoinHandle<()>>,
}

impl Pool {
    pub fn new(workers: usize) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<(u64, Job)>();
        let (done_tx, completion_rx) = mpsc::channel::<(u64, JobResult)>();
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let mut handles = Vec::new();
        for _ in 0..workers.max(1) {
            let job_rx = job_rx.clone();
            let done_tx = done_tx.clone();
            handles.push(std::thread::spawn(move || loop {
                // Lock only to dequeue; release before running the (possibly long) job.
                let next = { job_rx.lock().unwrap().recv() };
                match next {
                    Ok((id, job)) => { let res = job(); let _ = done_tx.send((id, res)); }
                    Err(_) => break, // all senders dropped → pool shutting down
                }
            }));
        }
        Pool { job_tx, completion_rx, _workers: handles }
    }

    pub fn submit(&self, id: u64, job: Job) {
        let _ = self.job_tx.send((id, job));
    }

    pub fn try_recv_completed(&self) -> Option<(u64, JobResult)> {
        self.completion_rx.try_recv().ok()
    }
}
```
Add to `core/src/lib.rs`: `mod async_rt;`

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core async_rt -- --test-threads=1`
Expected: PASS — both tests green.

- [ ] **Step 5: Commit**
```bash
git add core/src/async_rt.rs core/src/lib.rs
git commit -m "feat(core): V8-free threadpool (submit + completion channel) (TDD)"
```

---

## Task 2: The `TimerQueue` (V8-free, TDD)

**Files:**
- Modify: `core/src/async_rt.rs`
- Test: extend the inline `#[cfg(test)]`

**Interfaces:**
- Consumes: nothing from Task 1 (independent structure in the same file).
- Produces:
  ```rust
  pub enum TimerKind { Deadline(std::time::Instant), Frame(u64) } // Frame(target_frame_count)
  pub struct TimerQueue { /* entries: Vec<(u64 id, TimerKind)> */ }
  impl TimerQueue {
      pub fn new() -> Self;
      pub fn push(&mut self, id: u64, kind: TimerKind);
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
      // returns the ids whose deadline/frame has been reached (now/frame), removing them.
      pub fn due(&mut self, now: std::time::Instant, frame: u64) -> Vec<u64>;
  }
  ```

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests`:
```rust
    use std::time::{Duration, Instant};

    #[test]
    fn deadline_timer_is_due_only_after_its_instant() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(1, TimerKind::Deadline(now + Duration::from_millis(50)));
        assert_eq!(q.due(now, 0), Vec::<u64>::new());              // not yet
        assert_eq!(q.len(), 1);
        assert_eq!(q.due(now + Duration::from_millis(60), 0), vec![1]); // now due, removed
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn frame_timer_is_due_at_or_after_target_frame() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(7, TimerKind::Frame(5));
        assert_eq!(q.due(now, 4), Vec::<u64>::new()); // frame 4 < 5
        assert_eq!(q.due(now, 5), vec![7]);           // frame 5 reached
        assert!(q.is_empty());
    }

    #[test]
    fn multiple_due_timers_all_returned_and_removed() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(1, TimerKind::Deadline(now));            // already due
        q.push(2, TimerKind::Frame(1));                 // due at frame 1
        q.push(3, TimerKind::Deadline(now + Duration::from_secs(10))); // not due
        let mut due = q.due(now, 1);
        due.sort();
        assert_eq!(due, vec![1, 2]);
        assert_eq!(q.len(), 1); // only #3 remains
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core async_rt -- --test-threads=1`
Expected: FAIL — `TimerQueue`/`TimerKind` don't exist.

- [ ] **Step 3: Implement `TimerQueue`**

Add to `core/src/async_rt.rs` (above the tests):
```rust
#[derive(Clone, Copy, Debug)]
pub enum TimerKind {
    Deadline(std::time::Instant),
    Frame(u64), // resolve when the frame counter reaches this target
}

pub struct TimerQueue {
    entries: Vec<(u64, TimerKind)>,
}

impl TimerQueue {
    pub fn new() -> Self { TimerQueue { entries: Vec::new() } }
    pub fn push(&mut self, id: u64, kind: TimerKind) { self.entries.push((id, kind)); }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    pub fn due(&mut self, now: std::time::Instant, frame: u64) -> Vec<u64> {
        let mut ready = Vec::new();
        self.entries.retain(|(id, kind)| {
            let is_due = match kind {
                TimerKind::Deadline(t) => now >= *t,
                TimerKind::Frame(target) => frame >= *target,
            };
            if is_due { ready.push(*id); false } else { true }
        });
        ready
    }
}

impl Default for TimerQueue { fn default() -> Self { Self::new() } }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core async_rt -- --test-threads=1`
Expected: PASS — all `async_rt` tests (Pool + TimerQueue) green.

- [ ] **Step 5: Commit**
```bash
git add core/src/async_rt.rs
git commit -m "feat(core): V8-free timer queue (deadline + frame timers) (TDD)"
```

---

## Task 3: Explicit microtask policy + the frame drain + ffi Post wiring + out-of-range warning (integration TDD)

**Files:**
- Modify: `core/src/v8host.rs`, `core/src/ffi.rs`
- Test: extend `v8host.rs` and `ffi.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: the existing `v8host` `init`/`eval`/`shutdown`/`dispatch_onframe`; `ffi::s2script_core_dispatch_game_frame`.
- Produces:
  ```rust
  // v8host.rs
  pub(crate) fn frame_async_drain();   // resolve completed jobs + due timers (added T4/T5), then microtask checkpoint
  ```
  and the isolate now uses `MicrotasksPolicy::Explicit`.

- [ ] **Step 1: Write the failing tests**

Add to `core/src/v8host.rs`'s test module (reuse a capturing logger like `frame_tests`):
```rust
    #[test]
    fn microtasks_do_not_run_until_frame_drain() {
        init(dummy_logger()).unwrap();
        // With kExplicit, a resolved-promise continuation must NOT run during eval.
        eval("globalThis.__ran = false; Promise.resolve().then(() => { globalThis.__ran = true; });").unwrap();
        assert_eq!(read_bool_global("__ran"), false, "microtask ran before the drain");
        frame_async_drain(); // runs the checkpoint
        assert_eq!(read_bool_global("__ran"), true, "microtask did not run at the drain");
        shutdown();
    }

    #[test]
    fn onframe_handler_out_of_range_result_warns_and_continues() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();  // the frame_tests capturing logger
        eval("onGameFrame(() => 99);").unwrap();  // 99 is out of range for HookResult
        let out = dispatch_onframe(crate::multiplexer::Phase::Pre, true, false, false);
        assert_eq!(out.result, crate::multiplexer::HookResult::Continue); // out-of-range → Continue
        let got = LOG.lock().unwrap().clone();
        assert!(got.iter().any(|m| m.to_lowercase().contains("out-of-range") || m.contains("99")),
                "expected an out-of-range warning, got: {:?}", got);
        shutdown();
    }
```
> Add small test helpers `dummy_logger()` (a no-op `extern "C"` logger) and `read_bool_global(name)` (open a scope, read `globalThis[name]` as a bool) if not already present — or reuse the `frame_tests` `logger` + a scope read.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: FAIL — `frame_async_drain` missing; the out-of-range warning isn't emitted yet; without kExplicit the first test fails (the microtask ran early).

- [ ] **Step 3a: Switch the isolate to explicit microtask policy**

In `v8host.rs` `init`, right after `let mut isolate = v8::Isolate::new(v8::CreateParams::default());`, add:
```rust
    // We own the microtask checkpoint: with Explicit policy, await/.then continuations run ONLY
    // when we call perform_microtask_checkpoint() in frame_async_drain (once per frame).
    isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
```
> Confirm the exact method/enum against `v8 = 149.4.0` (docs.rs). If `set_microtasks_policy` isn't on `Isolate`, it may be set via `CreateParams`; use whichever the pinned crate exposes.

- [ ] **Step 3b: Add `frame_async_drain` (checkpoint + frame counter)**

Add a thread-local frame counter next to `HOST`/`FRAME`:
```rust
thread_local! { static FRAME_COUNTER: std::cell::Cell<u64> = std::cell::Cell::new(0); }
```
And the drain (Task 4/5 will insert timer/job resolution before the checkpoint):
```rust
pub(crate) fn frame_async_drain() {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };
        // (Task 4 inserts: resolve due timers here.)
        // (Task 5 inserts: resolve completed jobs here.)
        FRAME_COUNTER.with(|c| c.set(c.get().wrapping_add(1)));
        // Run the one microtask checkpoint for this frame.
        host.isolate.perform_microtask_checkpoint();
    });
}
```
> Confirm `perform_microtask_checkpoint` is available on `OwnedIsolate`/`Isolate` (or must be called via a scope) in the pinned crate; adjust to whichever the API provides.

- [ ] **Step 3c: Wire the drain into the ffi Post path**

In `core/src/ffi.rs` `s2script_core_dispatch_game_frame`, after the existing `dispatch_onframe` call, run the drain on Post only:
```rust
        let out = v8host::dispatch_onframe(phase, simulating != 0, first != 0, last != 0);
        if phase == 1 { v8host::frame_async_drain(); } // Post: resolve async + microtask checkpoint
        out.result as c_int
```
(Keep the whole body inside the existing `catch_unwind`.)

- [ ] **Step 3d: Fold in the Slice-1 out-of-range warning**

In the `onGameFrame` invoker (where the JS return maps to `HookResult` — the `if ret.is_undefined() { Continue } else { match ret.uint32_value(tc) { 1=>Changed, 2=>Handled, 3=>Stop, _=>Continue } }` block), change the mapping so a value that is neither `0..=3` logs a warning:
```rust
                    if ret.is_undefined() {
                        Ok(HookResult::Continue)
                    } else {
                        Ok(match ret.uint32_value(tc).unwrap_or(0) {
                            0 => HookResult::Continue,
                            1 => HookResult::Changed,
                            2 => HookResult::Handled,
                            3 => HookResult::Stop,
                            n => {
                                if let Some(f) = LOGGER.with(|l| l.get()) {
                                    if let Ok(c) = std::ffi::CString::new(
                                        format!("WARN: onGameFrame handler returned out-of-range HookResult {n}; treating as Continue")) {
                                        f(0, c.as_ptr());
                                    }
                                }
                                HookResult::Continue
                            }
                        })
                    }
```
> Match the exact surrounding code/variable names in `v8host.rs`; only the `match` arms + the warning change.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: PASS — both new tests + all Slice 0/1 tests green. Then `cargo build -p s2script-core 2>&1 | tail -1` → `Finished`.

- [ ] **Step 5: Commit**
```bash
git add core/src/v8host.rs core/src/ffi.rs
git commit -m "feat(core): explicit microtask policy + per-frame drain on Post; out-of-range HookResult warning"
```

---

## Task 4: `Delay`/`NextTick`/`NextFrame` + timer resolution + combined lazy-detour (integration TDD)

**Files:**
- Modify: `core/src/v8host.rs`
- Test: extend `v8host.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `async_rt::{TimerQueue, TimerKind}`; the T3 `frame_async_drain` + `FRAME_COUNTER`; the existing `apply_detour`/`HOOK_REQUEST`/`FRAME` (multiplexer) + `s2_subscribe`/`s2_unsubscribe`.
- Produces: JS globals `Delay(ms)`, `NextTick()`, `NextFrame()` (via natives `__s2_delay`/`__s2_next_tick`/`__s2_next_frame`) returning Promises; the `id → Global<PromiseResolver>` map; `refresh_detour()` (the combined predicate) replacing the direct `apply_detour` calls.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn delay_resolves_only_after_its_deadline() {
        init(dummy_logger()).unwrap();
        eval("globalThis.__d = false; Delay(30).then(() => { globalThis.__d = true; });").unwrap();
        frame_async_drain();                       // well before 30ms
        assert_eq!(read_bool_global("__d"), false);
        std::thread::sleep(std::time::Duration::from_millis(40));
        frame_async_drain();                       // now past the deadline
        assert_eq!(read_bool_global("__d"), true);
        shutdown();
    }

    #[test]
    fn next_frame_resolves_one_frame_later() {
        init(dummy_logger()).unwrap();
        eval("globalThis.__n = 0; NextFrame().then(() => { globalThis.__n = 1; });").unwrap();
        frame_async_drain(); // frame that schedules resolution for the NEXT frame → not yet
        // NextFrame targets FRAME_COUNTER+1 measured at call time; the drain that reaches it resolves it.
        assert_eq!(read_i32_global("__n"), 0);
        frame_async_drain();
        assert_eq!(read_i32_global("__n"), 1);
        shutdown();
    }

    #[test]
    fn delay_with_no_onframe_subscriber_still_requests_detour_install() {
        // Wire a recording request_hook (the ffi mock pattern) via set_hook_request BEFORE init.
        HOOKS.lock().unwrap().clear();
        set_hook_request(Some(record_hook));
        init(dummy_logger()).unwrap();
        eval("Delay(1000);").unwrap();  // pending async, zero onGameFrame subscribers
        assert!(HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 1),
                "Delay() should request the detour install");
        shutdown();
        set_hook_request(None);
    }
```
> Provide `read_i32_global` (mirror `read_bool_global`), and `record_hook`/`HOOKS` (a recording `extern "C" fn(*const c_char, c_int)` + a static `Mutex<Vec<(String,i32)>>`) if not already shared from earlier tests.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: FAIL — `Delay`/`NextFrame` undefined; `refresh_detour` missing; the detour-on-Delay isn't requested.

- [ ] **Step 3a: Add the async id → resolver map + timer queue + pending state**

Add thread-locals next to `FRAME`/`HOST`:
```rust
use crate::async_rt::{TimerQueue, TimerKind};
thread_local! {
    static TIMERS: std::cell::RefCell<TimerQueue> = std::cell::RefCell::new(TimerQueue::new());
    static RESOLVERS: std::cell::RefCell<std::collections::HashMap<u64, v8::Global<v8::PromiseResolver>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static NEXT_ASYNC_ID: std::cell::Cell<u64> = std::cell::Cell::new(1);
    static PENDING_JOBS: std::cell::Cell<usize> = std::cell::Cell::new(0); // Task 5 uses this
    static DETOUR_INSTALLED: std::cell::Cell<bool> = std::cell::Cell::new(false);
}
fn next_async_id() -> u64 { NEXT_ASYNC_ID.with(|c| { let v = c.get(); c.set(v + 1); v }) }
fn async_pending() -> usize { TIMERS.with(|t| t.borrow().len()) + PENDING_JOBS.with(|c| c.get()) }
```

- [ ] **Step 3b: The combined lazy-detour `refresh_detour()`**

Replace the direct `apply_detour(change)` calls in `s2_subscribe`/`s2_unsubscribe` with a call to `refresh_detour()` (ignore the `DetourChange` the multiplexer returns — the combined predicate supersedes it):
```rust
/// Desired = any onGameFrame subscriber OR any pending async. Requests install/remove on a transition.
fn refresh_detour() {
    let desired = FRAME.with(|f| f.borrow().enabled_count() > 0) || async_pending() > 0;
    let installed = DETOUR_INSTALLED.with(|c| c.get());
    if desired == installed { return; }
    DETOUR_INSTALLED.with(|c| c.set(desired));
    HOOK_REQUEST.with(|c| if let Some(req) = c.get() {
        let name = std::ffi::CString::new("OnGameFrame").unwrap();
        req(name.as_ptr(), desired as i32);
    });
}
```
Add `pub fn enabled_count(&self) -> usize { self.enabled_count }` to `Descriptor` in `multiplexer.rs` (accessor for the private field). In `s2_subscribe`/`s2_unsubscribe`, after mutating `FRAME`, call `refresh_detour()` instead of `apply_detour(change)`. (You may delete the now-unused `apply_detour`, or keep it unused — prefer deleting to avoid dead code.)

- [ ] **Step 3c: The `Delay`/`NextTick`/`NextFrame` natives + prelude**

Install three natives in the context (next to `__s2_subscribe`), each: create a `PromiseResolver`, store its `Global` in `RESOLVERS[id]`, push the timer, `refresh_detour()`, return the promise. A shared helper:
```rust
fn make_timer_promise<'s>(scope: &mut v8::PinScope<'s, '_>, kind: TimerKind) -> v8::Local<'s, v8::Value> {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let id = next_async_id();
    RESOLVERS.with(|m| m.borrow_mut().insert(id, v8::Global::new(scope, resolver)));
    TIMERS.with(|t| t.borrow_mut().push(id, kind));
    refresh_detour();
    promise.into()
}
```
- `__s2_delay(ms)`: `kind = TimerKind::Deadline(Instant::now() + Duration::from_millis(ms as u64))`.
- `__s2_next_tick()`: `kind = TimerKind::Frame(FRAME_COUNTER.get())` (next drain resolves `frame >= current`).
- `__s2_next_frame()`: `kind = TimerKind::Frame(FRAME_COUNTER.get() + 1)`.
Each native is a `catch_unwind(AssertUnwindSafe(..))`-guarded `FunctionCallback` that reads its arg, calls `make_timer_promise`, and sets the return value to the promise. Extend `PRELUDE` with:
```js
globalThis.Delay = (ms) => __s2_delay(ms || 0);
globalThis.NextTick = () => __s2_next_tick();
globalThis.NextFrame = () => __s2_next_frame();
```

- [ ] **Step 3d: Resolve due timers in the drain**

In `frame_async_drain`, BEFORE the `perform_microtask_checkpoint`, open a scope and resolve due timers:
```rust
        // (inside frame_async_drain, host in scope)
        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &host.context);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        let frame = FRAME_COUNTER.with(|c| c.get().wrapping_add(1));
        let due = TIMERS.with(|t| t.borrow_mut().due(std::time::Instant::now(), frame));
        for id in due {
            if let Some(g) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) {
                let resolver = v8::Local::new(scope, &g);
                let undef = v8::undefined(scope);
                resolver.resolve(scope, undef.into());
            }
        }
        // (Task 5 inserts job resolution here, using the same scope.)
        FRAME_COUNTER.with(|c| c.set(frame));
        scope.perform_microtask_checkpoint();
        // Any async that just completed may have made the detour undesired:
        drop(scope); // release the scope borrow before refresh_detour touches FRAME/HOOK_REQUEST? no FRAME borrow held
```
> Reconcile the borrow structure with the T3 skeleton (the scope replaces the bare `host.isolate.perform_microtask_checkpoint()`). Call `refresh_detour()` at the END of `frame_async_drain` (after the scope is dropped) so completing the last timer removes the detour. Confirm the v8-149.4 `PromiseResolver::resolve`/`v8::undefined`/`perform_microtask_checkpoint(scope)` signatures against the crate; mirror `eval`'s scope construction.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: PASS — the three new tests + all prior. `cargo build -p s2script-core` → `Finished`.

- [ ] **Step 5: Commit**
```bash
git add core/src/v8host.rs core/src/multiplexer.rs
git commit -m "feat(core): Delay/NextTick/NextFrame timers + combined lazy-detour (async keeps GameFrame installed)"
```

---

## Task 5: `threadSleep` demo threaded op + job resolution in the drain (integration TDD)

**Files:**
- Modify: `core/src/v8host.rs`
- Test: extend `v8host.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `async_rt::Pool`; the T4 `RESOLVERS`/`next_async_id`/`refresh_detour`/`frame_async_drain`; `PENDING_JOBS`.
- Produces: the JS global `threadSleep(ms)` (native `__s2_thread_sleep`) returning a Promise resolved from a worker thread; the process-global `Pool` + its use in `frame_async_drain`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn thread_sleep_runs_off_thread_and_resolves_on_a_drain() {
        init(dummy_logger()).unwrap();
        eval("globalThis.__t = false; threadSleep(20).then(() => { globalThis.__t = true; });").unwrap();
        // Drive frames until the worker completes (bounded).
        let mut resolved = false;
        for _ in 0..500 {
            frame_async_drain();
            if read_bool_global("__t") { resolved = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(resolved, "threadSleep promise never resolved on a drain");
        shutdown();
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: FAIL — `threadSleep` undefined.

- [ ] **Step 3a: The process-global pool + a main-thread accessor**

Add a `Once`-guarded process-global pool created at first init (persists across re-init like the platform), plus the `__s2_thread_sleep` native:
```rust
use crate::async_rt::Pool;
use std::sync::OnceLock;
static POOL: OnceLock<Pool> = OnceLock::new();
fn pool() -> &'static Pool { POOL.get_or_init(|| Pool::new(4)) }
```
`__s2_thread_sleep(ms)` (a `catch_unwind`-guarded `FunctionCallback`): create a `PromiseResolver`, store its `Global` in `RESOLVERS[id]`, `PENDING_JOBS += 1`, submit the blocking job, `refresh_detour()`, return the promise:
```rust
    // inside the callback, with `scope` and `ms: u64`:
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let id = next_async_id();
    RESOLVERS.with(|m| m.borrow_mut().insert(id, v8::Global::new(scope, resolver)));
    PENDING_JOBS.with(|c| c.set(c.get() + 1));
    pool().submit(id, Box::new(move || { std::thread::sleep(std::time::Duration::from_millis(ms)); Ok(()) }));
    refresh_detour();
    rv.set(promise.into());
```
Extend `PRELUDE`: `globalThis.threadSleep = (ms) => __s2_thread_sleep(ms || 0);`

- [ ] **Step 3b: Resolve completed jobs in the drain**

In `frame_async_drain`, in the same scope as the timer resolution (Task 4 §3d), BEFORE the microtask checkpoint, drain the pool:
```rust
        // resolve completed threadpool jobs
        while let Some((id, _res)) = pool().try_recv_completed() {
            PENDING_JOBS.with(|c| c.set(c.get().saturating_sub(1)));
            if let Some(g) = RESOLVERS.with(|m| m.borrow_mut().remove(&id)) {
                let resolver = v8::Local::new(scope, &g);
                let undef = v8::undefined(scope);
                resolver.resolve(scope, undef.into());
            }
        }
```
(The final `refresh_detour()` at the end of `frame_async_drain` already removes the detour when both timers and jobs hit zero.)

- [ ] **Step 3c: shutdown resets the async state**

In `shutdown`, alongside the existing `FRAME`/`HOST` reset, clear the per-isolate async state (leave the process-global `POOL` alone):
```rust
    TIMERS.with(|t| *t.borrow_mut() = TimerQueue::new());
    RESOLVERS.with(|m| m.borrow_mut().clear());
    PENDING_JOBS.with(|c| c.set(0));
    DETOUR_INSTALLED.with(|c| c.set(false));
    FRAME_COUNTER.with(|c| c.set(0));
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: PASS — the threaded-op test + all prior green. `cargo build -p s2script-core` → `Finished`.

- [ ] **Step 5: Commit**
```bash
git add core/src/v8host.rs
git commit -m "feat(core): threadSleep demo op — off-thread work resolves a Promise on the frame drain"
```

---

## Task 6: Live verification + README (operator-run gate)

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (extend the baked-in Load demo with an async demonstration), `README.md`

**Interfaces:**
- Consumes: everything from T1–T5.
- Produces: a documented, operator-run demonstration that `await Delay(1000)` doesn't block the tick and the threaded op resumes on the main thread, on a real CS2 server.

> The CS2 Docker harness + the 64 GB `docker/cs2-data` copy are already in place. Build with `scripts/build-sniper.sh`. Claude drives the container (build → package → recreate → observe), as in Slices 0/1.

- [ ] **Step 1: Extend the Load demo with an async demonstration**

In `shim/src/s2script_mm.cpp`, extend the existing baked-in demo `eval` (the Slice-1 `onGameFrame` demo) to also exercise async — an `async` IIFE that logs, awaits `Delay(1000)`, logs the elapsed frames, then awaits `threadSleep`:
```js
        // ... (existing onGameFrame demo) ...
        var __startFrame = 0;
        onGameFrame(() => { __startFrame++; }, { priority: "monitor" });
        (async () => {
            console.log('[async] before Delay(1000)');
            await Delay(1000);
            console.log('[async] after Delay(1000); frames elapsed ~' + __startFrame + ' (tick was NOT blocked)');
            await threadSleep(50);
            console.log('[async] after threadSleep(50) — resumed on the main thread');
        })();
```
> This proves: the frame loop kept running during the await (`__startFrame` advanced to ~64+ over the 1s), and the off-thread `threadSleep` continuation resumed on the main thread. Keep it clearly marked as the Slice-2 demo (removed when real plugin loading lands in Slice 4).

- [ ] **Step 2: Sniper build, package, recreate the container**
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
docker compose -f docker/docker-compose.yml up -d --force-recreate cs2
```
Wait for server-up (reuse the Slice-0/1 watcher: poll `docker logs` for `GC Connection established`).

- [ ] **Step 3: Observe the async demo + record acceptance**
```bash
docker logs s2script-cs2 2>&1 | grep -E "\[async\]|\[s2script\]|interface OK" | tail -20
```
**Acceptance to record (spec §8):**
- `[async] before Delay(1000)` appears at load; ~1s later `[async] after Delay(1000); frames elapsed ~N` with N large (≈ tickrate) — proving the tick advanced throughout the await (not blocked).
- `[async] after threadSleep(50) — resumed on the main thread` appears shortly after — the off-thread op marshalled back.
- The server never crashes; the frame loop keeps running.
- (Regression) the Slice-1 `[demo] HIGH`/`low` composition + `interface OK` lines still appear.

- [ ] **Step 4: Update the README + acceptance table**

Add a "Tick-integrated async (Slice 2)" subsection to the README's live runbook with the async demo + expected output, and a Slice-2 acceptance table covering spec §8 (with the live evidence, like Slice 1). Note the primitives are provisional globals (the typed `@s2script/std` async API is Slice 5).

- [ ] **Step 5: Commit + stop the container (keep the copy)**
```bash
git add shim/src/s2script_mm.cpp README.md
git commit -m "docs+demo: Slice 2 tick-async live demo (await Delay doesn't block the tick) + acceptance"
docker stop s2script-cs2 && docker rm s2script-cs2
```

---

## Self-Review (completed during planning)

- **Spec coverage:** §1 thesis → T1–T6. §2 kExplicit → T3; threadpool → T1; Post drain → T3; combined lazy-detour → T4. §3 async_rt (threadpool+timers, V8-free) → T1+T2. §4 v8host (policy/primitives/resolver map/drain/lazy-detour/shutdown reset/out-of-range warning) → T3+T4+T5. §5 primitive semantics → T4 (Delay/NextTick/NextFrame) + T5 (threadSleep). §6 no shim/ABI change → honored (only T6 touches the shim, and only the *baked-in demo* string, not the hook wiring/ABI). §7 testing (unit/integration/live) → T1–T2 unit, T3–T5 integration, T6 live. §8 acceptance → T6. §9 out-of-scope honored (no db/http framework, no per-plugin async/ledger, no setTimeout, fixed pool size). §10 files → matches. §11 open items (rusty_v8 APIs, boot ordering, kExplicit regression) → flagged in Global Constraints + T3/T4 confirm notes. No spec section unmapped.
- **Placeholder scan:** No "TBD/TODO" gaps. The "confirm against the crate" notes (kExplicit, PromiseResolver, perform_microtask_checkpoint) are external-binding guidance naming the exact API to check — not deferred work. Test helpers (`read_bool_global`/`read_i32_global`/`dummy_logger`/`record_hook`) are described concretely where first used.
- **Type consistency:** `Pool` (`new`/`submit(u64, Job)`/`try_recv_completed() -> Option<(u64, JobResult)>`), `TimerQueue`/`TimerKind` (`Deadline(Instant)`/`Frame(u64)`, `push(u64, kind)`/`due(now, frame) -> Vec<u64>`), `JobResult = Result<(),String>` identical across T1/T2/T4/T5. The `u64` async-id space is shared by timers and jobs → one `RESOLVERS: HashMap<u64, Global<PromiseResolver>>`. `frame_async_drain()` defined in T3, extended (not re-signed) in T4/T5. `refresh_detour()`/`enabled_count()`/`async_pending()` consistent across T4/T5. `dispatch_game_frame` phase `1 == Post` matches the ffi + Slice-1 convention.
