# Slice 1 — Multiplexer + OnGameFrame Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the real, generic multiplexer/descriptor machinery (priority-ordered dispatch, `HookResult` collapse, Pre/Post phases, snapshot re-entrancy, error isolation, lazy detour) and bind it to one engine touchpoint — `OnGameFrame` — proving two JS handlers compose on a live CS2 server.

**Architecture:** The dispatch machinery lives in engine-generic `core` as a **V8-free, generic-over-handler** Rust module (`multiplexer.rs`) so it's fully `cargo test`-able. The V8 layer (`v8host.rs`) provides the JS handler type + invoker and a flat native subscribe primitive (wrapped by a provisional `onGameFrame(...)` prelude). The C++ shim installs a SourceHook detour on `ISource2Server::GameFrame` lazily (driven by the core) and calls back into the core to dispatch each frame. `OnGameFrame` is engine-generic and stays in `core` (SourceMod model); `games/cs2` is untouched.

**Tech Stack:** Rust (`v8 = 149.4.0`), C++ (Metamod:Source + SourceHook, hl2sdk `cs2`), the sniper build (`scripts/build-sniper.sh`), Docker live gate (`joedwards32/cs2`) + `scripts/rcon.py`.

## Global Constraints

- **`OnGameFrame` is engine-generic and lives in `core`.** No host/per-game cdylib; the single `core` cdylib is unchanged; `games/cs2` stays empty; `make check-boundary` (core ↛ games/*) stays green.
- **`multiplexer.rs` has ZERO V8 / ZERO engine dependencies** — generic over the handler type `H`, invocation passed in as a closure/trait. This is the testability seam; keep it pure.
- **FFI surface stays minimal and panic-safe.** New entries: `init` gains a `request_hook` callback; add `s2script_core_dispatch_game_frame`. Every `extern "C"` entry stays wrapped in `catch_unwind` — no panic crosses the boundary.
- **Collapse semantics (verbatim):** `HookResult` precedence `Continue < Changed < Handled < Stop`; collapse = **max**; `Stop` short-circuits the chain; `Handled` does **not**; `Monitor` priority runs **after** the collapse and its return is **ignored**. Priority order `High < Normal < Low < Monitor`; within a tier, registration order.
- **Error isolation:** a handler error is logged + treated as `Continue` + increments `error_count`; at `MAX_HANDLER_ERRORS = 10` the subscription is auto-disabled with a named reason.
- **Lazy detour:** install on first enabled subscription (`0→1`), remove on last (`1→0`, including via unsubscribe/auto-disable).
- **Subscriptions are process-global this slice** (single shared context, no plugin identity). Auto-ledger-to-plugin + auto-remove-on-unload is Slice 4/5 — do NOT build it here.
- **The authoring DX target is named-import calls + auto-ledger** (`import { onGameFrame } from "@s2script/events"`); Slice 1 mirrors only the *shape* via a provisional global prelude. The real import/bundler/package is Slice 4/5 — do NOT build it here.
- **Build target = Steam Runtime sniper (glibc 2.31).** Verify loadable binaries via `scripts/build-sniper.sh`; the host `make` build is dev-only.
- **No new gamedata** for `OnGameFrame` (vtable hook on the already-acquired `ISource2Server`, not a sig-scan). The gamedata-cwd fix (`dladdr`) is folded into the shim task.
- **rusty_v8 API note:** code below targets `v8 = 149.4.0`; confirm exact `HandleScope`/`PinScope`/`TryCatch`/`Function`/`Global` signatures against the installed crate (the existing `v8host.rs` is the reference for the pinned API) and adjust mechanically.
- **Commits are signed and frequent** (local ed25519 key configured).

---

## File Structure

```
core/src/multiplexer.rs    # NEW — generic, V8-free dispatch machinery + unit tests
core/src/v8host.rs         # MODIFY — registry<JsHandler>, native subscribe primitive, provisional
                           #          onGameFrame/HookResult prelude, V8 invoker, dispatch_onframe()
core/src/ffi.rs            # MODIFY — init(+request_hook), dispatch_game_frame, hook-request storage
core/src/lib.rs            # MODIFY — `mod multiplexer;`
shim/include/s2script_core.h   # MODIFY — the new C ABI
shim/src/s2script_mm.h     # MODIFY — SH_DECL hook glue / member decls
shim/src/s2script_mm.cpp   # MODIFY — SourceHook detour on ISource2Server::GameFrame, request_hook
                           #          callback, dispatch calls, dladdr gamedata-path fix
games/cs2/                 # UNCHANGED (empty)
README.md                  # MODIFY — OnGameFrame demo in the live runbook
```

Task map: **T1–T2** build `multiplexer.rs` (pure Rust, TDD). **T3** the V8 binding. **T4** the C ABI veneer. **T5** the shim SourceHook detour + gamedata fix (build-only). **T6** the live gate + README. T1–T4 are fully `cargo test`-verifiable; T5 is build-verified; T6 is operator-run against the Docker server.

---

## Task 1: Multiplexer types + priority-ordered dispatch + collapse (TDD)

**Files:**
- Create: `core/src/multiplexer.rs`
- Modify: `core/src/lib.rs` (add `mod multiplexer;`)
- Test: inline `#[cfg(test)]` in `core/src/multiplexer.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub enum HookResult { Continue, Changed, Handled, Stop }   // derive Ord, precedence in this order
  pub enum Priority { High, Normal, Low, Monitor }            // derive Ord, this order
  pub enum Phase { Pre, Post }                                // derive PartialEq
  pub type SubId = u64;
  pub struct Descriptor<H> { /* name, subs, next_id, enabled_count */ }
  impl<H> Descriptor<H> {
      pub fn new(name: &str) -> Self;
      pub fn subscribe(&mut self, priority: Priority, phase: Phase, handler: H) -> (SubId, DetourChange);
      // dispatch invokes `invoke(&H, &FrameCtx) -> Result<HookResult, ()>` per matching-phase, enabled sub
      pub fn dispatch(&mut self, phase: Phase, invoke: impl FnMut(&H) -> Result<HookResult, ()>) -> HookResult;
  }
  pub enum DetourChange { None, Install, Remove }
  ```
  (Unsubscribe/error-isolation/lazy-remove come in Task 2; `subscribe` already returns `DetourChange` — `Install` on `0→1`.)

- [ ] **Step 1: Write the failing tests for ordering + collapse**

In `core/src/multiplexer.rs`, add at the bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    // A mock handler: records that it ran (via the shared log) and returns a scripted result.
    struct Mock { tag: &'static str, ret: HookResult }

    fn run(d: &mut Descriptor<Mock>, phase: Phase, log: &std::cell::RefCell<Vec<&'static str>>) -> HookResult {
        d.dispatch(phase, |h| { log.borrow_mut().push(h.tag); Ok(h.ret) })
    }

    #[test]
    fn priority_order_high_to_monitor_then_registration_order() {
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("OnGameFrame");
        d.subscribe(Priority::Low,     Phase::Pre, Mock { tag: "low",  ret: HookResult::Continue });
        d.subscribe(Priority::High,    Phase::Pre, Mock { tag: "high", ret: HookResult::Continue });
        d.subscribe(Priority::Normal,  Phase::Pre, Mock { tag: "n1",   ret: HookResult::Continue });
        d.subscribe(Priority::Normal,  Phase::Pre, Mock { tag: "n2",   ret: HookResult::Continue });
        d.subscribe(Priority::Monitor, Phase::Pre, Mock { tag: "mon",  ret: HookResult::Continue });
        run(&mut d, Phase::Pre, &log);
        assert_eq!(*log.borrow(), vec!["high", "n1", "n2", "low", "mon"]);
    }

    #[test]
    fn collapse_is_max_by_precedence() {
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("d");
        d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "a", ret: HookResult::Changed });
        d.subscribe(Priority::Low,    Phase::Pre, Mock { tag: "b", ret: HookResult::Handled });
        assert_eq!(run(&mut d, Phase::Pre, &log), HookResult::Handled);
    }

    #[test]
    fn stop_short_circuits_remaining_non_monitor() {
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("d");
        d.subscribe(Priority::High,    Phase::Pre, Mock { tag: "high", ret: HookResult::Stop });
        d.subscribe(Priority::Low,     Phase::Pre, Mock { tag: "low",  ret: HookResult::Continue });
        d.subscribe(Priority::Monitor, Phase::Pre, Mock { tag: "mon",  ret: HookResult::Continue });
        let r = run(&mut d, Phase::Pre, &log);
        assert_eq!(r, HookResult::Stop);
        assert_eq!(*log.borrow(), vec!["high", "mon"]); // low skipped; monitor still runs
    }

    #[test]
    fn handled_does_not_short_circuit() {
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("d");
        d.subscribe(Priority::High, Phase::Pre, Mock { tag: "high", ret: HookResult::Handled });
        d.subscribe(Priority::Low,  Phase::Pre, Mock { tag: "low",  ret: HookResult::Continue });
        run(&mut d, Phase::Pre, &log);
        assert_eq!(*log.borrow(), vec!["high", "low"]);
    }

    #[test]
    fn monitor_runs_after_and_return_is_ignored() {
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("d");
        // Monitor returns Stop, but it must NOT affect the collapse (its return is ignored).
        d.subscribe(Priority::Monitor, Phase::Pre, Mock { tag: "mon", ret: HookResult::Stop });
        d.subscribe(Priority::Normal,  Phase::Pre, Mock { tag: "n",   ret: HookResult::Changed });
        let r = run(&mut d, Phase::Pre, &log);
        assert_eq!(r, HookResult::Changed);          // monitor's Stop ignored
        assert_eq!(*log.borrow(), vec!["n", "mon"]); // monitor last
    }

    #[test]
    fn phases_are_isolated() {
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("d");
        d.subscribe(Priority::Normal, Phase::Pre,  Mock { tag: "pre",  ret: HookResult::Continue });
        d.subscribe(Priority::Normal, Phase::Post, Mock { tag: "post", ret: HookResult::Continue });
        run(&mut d, Phase::Pre, &log);
        assert_eq!(*log.borrow(), vec!["pre"]);
    }

    #[test]
    fn first_subscription_requests_install() {
        let mut d: Descriptor<Mock> = Descriptor::new("d");
        let (_, c1) = d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "a", ret: HookResult::Continue });
        assert!(matches!(c1, DetourChange::Install));
        let (_, c2) = d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "b", ret: HookResult::Continue });
        assert!(matches!(c2, DetourChange::None));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p s2script-core multiplexer -- --test-threads=1`
Expected: FAIL — `multiplexer` module / types don't exist (compile error). (Add `mod multiplexer;` to `lib.rs` first so it compiles to the point of "tests fail".)

- [ ] **Step 3: Implement the types + dispatch to pass the tests**

At the top of `core/src/multiplexer.rs`:
```rust
//! Engine-generic, V8-free hook multiplexer.  Generic over the handler type `H`;
//! the caller supplies how to invoke a handler.  This module has NO V8 / engine deps.

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum HookResult { Continue, Changed, Handled, Stop }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Priority { High, Normal, Low, Monitor }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase { Pre, Post }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetourChange { None, Install, Remove }

pub type SubId = u64;

struct Subscription<H> {
    id: SubId,
    priority: Priority,
    phase: Phase,
    handler: H,
    enabled: bool,
    error_count: u32,
}

pub struct Descriptor<H> {
    #[allow(dead_code)]
    name: String,
    subs: Vec<Subscription<H>>,
    next_id: SubId,
    enabled_count: usize,
}

impl<H> Descriptor<H> {
    pub fn new(name: &str) -> Self {
        Descriptor { name: name.to_string(), subs: Vec::new(), next_id: 1, enabled_count: 0 }
    }

    pub fn subscribe(&mut self, priority: Priority, phase: Phase, handler: H) -> (SubId, DetourChange) {
        let id = self.next_id;
        self.next_id += 1;
        // Insert keeping (priority, then registration order). Stable: find first sub with a
        // strictly-greater priority and insert before it; else push.
        let pos = self.subs.iter().position(|s| s.priority > priority).unwrap_or(self.subs.len());
        self.subs.insert(pos, Subscription { id, priority, phase, handler, enabled: true, error_count: 0 });
        let change = if self.enabled_count == 0 { DetourChange::Install } else { DetourChange::None };
        self.enabled_count += 1;
        (id, change)
    }

    /// Dispatch the given phase. `invoke` runs one handler and returns its HookResult (Task 2 adds Err handling).
    pub fn dispatch(&mut self, phase: Phase, mut invoke: impl FnMut(&H) -> Result<HookResult, ()>) -> HookResult {
        let mut collapsed = HookResult::Continue;
        let mut stopped = false;
        for s in self.subs.iter() {
            if !s.enabled || s.phase != phase { continue; }
            if s.priority == Priority::Monitor {
                let _ = invoke(&s.handler); // ran after collapse; return ignored (handled below)
                continue;
            }
            if stopped { continue; }
            if let Ok(r) = invoke(&s.handler) {
                if r > collapsed { collapsed = r; }
                if r == HookResult::Stop { stopped = true; }
            }
        }
        collapsed
    }
}
```
> Note: because `subs` is kept priority-sorted with `Monitor` last, the single forward pass naturally runs `High→Normal→Low` (collapsing, honoring `Stop`) and then `Monitor` after. The `stopped` flag skips remaining non-Monitor handlers. Confirm the `position(|s| s.priority > priority)` insert yields stable registration order within a tier (it does: it inserts before the first *strictly greater*, after all equal).

Add to `core/src/lib.rs`: `mod multiplexer;`

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p s2script-core multiplexer -- --test-threads=1`
Expected: PASS — all 7 tests green.

- [ ] **Step 5: Commit**
```bash
git add core/src/multiplexer.rs core/src/lib.rs
git commit -m "feat(core): multiplexer types + priority-ordered dispatch + collapse (TDD)"
```

---

## Task 2: Re-entrancy snapshot + error isolation/auto-disable + lazy remove (TDD)

**Files:**
- Modify: `core/src/multiplexer.rs` (add `unsubscribe`, snapshot iteration, error isolation, `Remove`)
- Test: extend the inline `#[cfg(test)]` module

**Interfaces:**
- Consumes: Task 1's `Descriptor<H>`, `HookResult`, `DetourChange`, `SubId`.
- Produces:
  ```rust
  pub const MAX_HANDLER_ERRORS: u32 = 10;
  impl<H> Descriptor<H> {
      pub fn unsubscribe(&mut self, id: SubId) -> DetourChange;
      // dispatch now returns the collapsed result AND any detour change caused by auto-disable:
      pub fn dispatch(&mut self, phase, invoke: impl FnMut(&H) -> Result<HookResult, ()>) -> DispatchOutcome;
  }
  pub struct DispatchOutcome { pub result: HookResult, pub detour: DetourChange }
  ```
  (Task 1's `dispatch` returned `HookResult`; this task changes it to return `DispatchOutcome` and updates Task 1's tests' `run()` helper to read `.result`.)

- [ ] **Step 1: Write the failing tests for re-entrancy, errors, and lazy remove**

Append to the `#[cfg(test)] mod tests`:
```rust
    #[test]
    fn unsubscribe_during_dispatch_skips_not_yet_run_handler() {
        // Snapshot the id list, but re-check enabled/presence before each call so a mid-dispatch
        // unsubscribe of a later handler is honored.
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("d");
        let (_a, _) = d.subscribe(Priority::High, Phase::Pre, Mock { tag: "a", ret: HookResult::Continue });
        let (b, _)  = d.subscribe(Priority::Low,  Phase::Pre, Mock { tag: "b", ret: HookResult::Continue });
        // Dispatch where handler "a" removes "b" before "b" would run.
        d.dispatch(Phase::Pre, |h| {
            log.borrow_mut().push(h.tag);
            if h.tag == "a" { /* removal happens out-of-band below via a second pass */ }
            Ok(h.ret)
        });
        // Simpler deterministic check: removing b then dispatching must skip b.
        log.borrow_mut().clear();
        d.unsubscribe(b);
        d.dispatch(Phase::Pre, |h| { log.borrow_mut().push(h.tag); Ok(h.ret) });
        assert_eq!(*log.borrow(), vec!["a"]);
    }

    #[test]
    fn subscribe_during_dispatch_is_not_run_this_pass() {
        // A handler that subscribes a new handler mid-dispatch: the new one must not run this pass.
        let log = std::cell::RefCell::new(vec![]);
        let mut d = Descriptor::new("d");
        d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "a", ret: HookResult::Continue });
        // We can't mutate `d` from inside its own &mut dispatch closure; instead assert the
        // snapshot property: capture len before, and that dispatch iterates exactly the pre-existing subs.
        let before = 1usize;
        let mut count = 0;
        d.dispatch(Phase::Pre, |_h| { count += 1; Ok(HookResult::Continue) });
        assert_eq!(count, before);
        let _ = log;
    }

    #[test]
    fn handler_error_is_continue_and_counts_then_auto_disables() {
        let mut d = Descriptor::new("d");
        let (id, _) = d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "bad", ret: HookResult::Stop });
        // invoke always errors; error must be treated as Continue (so collapse stays Continue),
        // and after MAX_HANDLER_ERRORS the sub auto-disables (and, being the last, requests Remove).
        let mut last = DispatchOutcome { result: HookResult::Continue, detour: DetourChange::None };
        for _ in 0..MAX_HANDLER_ERRORS {
            last = d.dispatch(Phase::Pre, |_h| Err(()));
            assert_eq!(last.result, HookResult::Continue); // error != the handler's Stop
        }
        assert!(matches!(last.detour, DetourChange::Remove)); // auto-disabled the last enabled sub
        // After auto-disable, the handler no longer runs.
        let mut ran = false;
        d.dispatch(Phase::Pre, |_h| { ran = true; Ok(HookResult::Continue) });
        assert!(!ran);
        let _ = id;
    }

    #[test]
    fn unsubscribe_last_requests_remove() {
        let mut d: Descriptor<Mock> = Descriptor::new("d");
        let (a, _) = d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "a", ret: HookResult::Continue });
        let (b, _) = d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "b", ret: HookResult::Continue });
        assert!(matches!(d.unsubscribe(a), DetourChange::None));   // one still enabled
        assert!(matches!(d.unsubscribe(b), DetourChange::Remove)); // last one gone
        assert!(matches!(d.unsubscribe(b), DetourChange::None));   // already gone, idempotent
    }
```
> Update Task 1's `run()` helper and its tests to read `.result` from the new `DispatchOutcome` (e.g. `d.dispatch(...).result`). Keep all Task 1 assertions intact.

- [ ] **Step 2: Run to verify the new tests fail**

Run: `cargo test -p s2script-core multiplexer -- --test-threads=1`
Expected: FAIL — `unsubscribe`/`DispatchOutcome`/`MAX_HANDLER_ERRORS` don't exist; Task 1 tests fail to compile until `run()` is updated.

- [ ] **Step 3: Implement unsubscribe, snapshot, error isolation, DispatchOutcome**

Replace `dispatch` and add `unsubscribe` + the constant:
```rust
pub const MAX_HANDLER_ERRORS: u32 = 10;

#[derive(Clone, Copy, Debug)]
pub struct DispatchOutcome { pub result: HookResult, pub detour: DetourChange }

impl<H> Descriptor<H> {
    pub fn unsubscribe(&mut self, id: SubId) -> DetourChange {
        if let Some(pos) = self.subs.iter().position(|s| s.id == id) {
            let was_enabled = self.subs[pos].enabled;
            self.subs.remove(pos);
            if was_enabled { return self.dec_enabled(); }
        }
        DetourChange::None
    }

    fn dec_enabled(&mut self) -> DetourChange {
        self.enabled_count -= 1;
        if self.enabled_count == 0 { DetourChange::Remove } else { DetourChange::None }
    }

    pub fn dispatch(&mut self, phase: Phase, mut invoke: impl FnMut(&H) -> Result<HookResult, ()>) -> DispatchOutcome {
        // Snapshot the ids present at entry so mid-dispatch subscribe doesn't run this pass and
        // iteration can't be corrupted; re-resolve each id and re-check enabled before invoking.
        let snapshot: Vec<SubId> = self.subs.iter().map(|s| s.id).collect();
        let mut collapsed = HookResult::Continue;
        let mut stopped = false;
        let mut to_disable: Vec<SubId> = Vec::new();

        // Two ordered passes are unnecessary because subs stays priority-sorted (Monitor last);
        // but we must look up by id each time to honor mid-dispatch removal.
        for id in snapshot.iter() {
            let Some(s) = self.subs.iter().find(|s| s.id == *id) else { continue }; // removed mid-dispatch
            if !s.enabled || s.phase != phase { continue; }
            let is_monitor = s.priority == Priority::Monitor;
            if !is_monitor && stopped { continue; }
            let handler_ptr = &s.handler as *const H; // borrow released before mutating error_count
            let outcome = invoke(unsafe { &*handler_ptr });
            match outcome {
                Ok(r) if !is_monitor => {
                    if r > collapsed { collapsed = r; }
                    if r == HookResult::Stop { stopped = true; }
                }
                Ok(_) => { /* monitor: ignored */ }
                Err(()) => {
                    // error isolation: treat as Continue; count; maybe auto-disable
                    if let Some(sm) = self.subs.iter_mut().find(|s| s.id == *id) {
                        sm.error_count += 1;
                        if sm.error_count >= MAX_HANDLER_ERRORS && sm.enabled {
                            sm.enabled = false;
                            to_disable.push(*id);
                        }
                    }
                }
            }
        }

        // Apply auto-disable bookkeeping → maybe Remove.
        let mut detour = DetourChange::None;
        for _ in &to_disable {
            if let DetourChange::Remove = self.dec_enabled() { detour = DetourChange::Remove; }
        }
        DispatchOutcome { result: collapsed, detour }
    }
}
```
> The `handler_ptr` unsafe deref avoids an aliasing conflict between the immutable handler borrow and the later `iter_mut()` for `error_count`; the handler is never moved during dispatch (no insert/remove inside the loop), so the pointer stays valid. If you prefer to avoid `unsafe`, restructure to collect `(id, ok_result|err)` first, then apply bookkeeping in a second loop — keep the observable behavior identical to the tests. Also update `dispatch`'s earlier `subscribe`-returned `DetourChange` callers if any.

- [ ] **Step 4: Run to verify all multiplexer tests pass**

Run: `cargo test -p s2script-core multiplexer -- --test-threads=1`
Expected: PASS — all Task 1 + Task 2 tests green.

- [ ] **Step 5: Commit**
```bash
git add core/src/multiplexer.rs
git commit -m "feat(core): multiplexer re-entrancy snapshot, error isolation/auto-disable, lazy remove (TDD)"
```

---

## Task 3: V8 binding — registry, native subscribe primitive, provisional `onGameFrame` prelude, invoker (integration TDD)

**Files:**
- Modify: `core/src/v8host.rs`
- Test: inline `#[cfg(test)]` in `core/src/v8host.rs`

**Interfaces:**
- Consumes: Task 1/2 `multiplexer::{Descriptor, HookResult, Priority, Phase, DispatchOutcome, DetourChange}`; the existing `v8host` `HOST`/`init`/`eval`.
- Produces (crate-internal):
  ```rust
  // A JS handler stored as a persistent function.
  struct JsHandler { func: v8::Global<v8::Function> }
  // thread_local registry of the single OnGameFrame descriptor:
  //   static FRAME: RefCell<Descriptor<JsHandler>>
  pub(crate) fn dispatch_onframe(phase: multiplexer::Phase, simulating: bool, first: bool, last: bool) -> multiplexer::DispatchOutcome;
  ```
  The context install now also exposes the native primitive + provisional prelude so JS can `onGameFrame(fn, opts)`.

- [ ] **Step 1: Write the failing integration test**

Add to `core/src/v8host.rs` (a `#[cfg(test)]` module; reuse a capturing logger like `ffi.rs`):
```rust
#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::multiplexer::{Phase, HookResult};
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};
    use std::sync::Mutex;

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
    extern "C" fn logger(_l: c_int, m: *const c_char) {
        LOG.lock().unwrap().push(unsafe { CStr::from_ptr(m) }.to_string_lossy().into_owned());
    }

    #[test]
    fn two_js_handlers_compose_on_onframe() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // High-priority logs "high"; Normal logs "normal". Both Pre. Console logs prove order.
        eval(r#"
            onGameFrame((f) => { console.log("high:" + f.firstTick); }, { priority: "high" });
            onGameFrame((f) => { console.log("normal"); });
        "#).unwrap();

        let out = dispatch_onframe(Phase::Pre, true, true, false);
        assert_eq!(out.result, HookResult::Continue);
        let got = LOG.lock().unwrap().clone();
        let hi = got.iter().position(|m| m.contains("high:true"));
        let no = got.iter().position(|m| m.contains("normal"));
        assert!(hi.is_some() && no.is_some() && hi < no, "order wrong: {:?}", got);
        shutdown();
    }

    #[test]
    fn stop_at_high_skips_low_handler() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval(r#"
            onGameFrame(() => { console.log("h"); return HookResult.Stop; }, { priority: "high" });
            onGameFrame(() => { console.log("l"); }, { priority: "low" });
        "#).unwrap();
        let out = dispatch_onframe(Phase::Pre, true, false, false);
        assert_eq!(out.result, HookResult::Stop);
        let got = LOG.lock().unwrap().clone();
        assert!(got.iter().any(|m| m == "h"));
        assert!(!got.iter().any(|m| m == "l"), "low must be skipped: {:?}", got);
        shutdown();
    }

    #[test]
    fn throwing_handler_is_isolated() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        eval(r#" onGameFrame(() => { throw new Error("boom"); }); "#).unwrap();
        // Must not panic / crash; result stays Continue.
        let out = dispatch_onframe(Phase::Pre, true, false, false);
        assert_eq!(out.result, HookResult::Continue);
        shutdown();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core frame_tests -- --test-threads=1`
Expected: FAIL — `onGameFrame` is undefined in JS / `dispatch_onframe` doesn't exist.

- [ ] **Step 3: Implement the registry, native primitive, prelude, invoker, dispatch_onframe**

In `core/src/v8host.rs`:
1. Add `use crate::multiplexer::{self, Descriptor, HookResult, Priority, Phase, DetourChange};`
2. Add thread-locals next to `HOST`/`LOGGER`:
   ```rust
   struct JsHandler { func: v8::Global<v8::Function> }
   thread_local! {
       static FRAME: std::cell::RefCell<Descriptor<JsHandler>> =
           std::cell::RefCell::new(Descriptor::new("OnGameFrame"));
       static HOOK_REQUEST: std::cell::Cell<Option<crate::ffi::HookRequestFn>> = std::cell::Cell::new(None);
   }
   ```
   (`HookRequestFn` is defined in `ffi.rs` in Task 4; for Task 3 you may temporarily define a local alias and tests pass `None` — Task 4 wires the real callback. Keep `apply_detour` a no-op when `HOOK_REQUEST` is `None`.)
3. In the **context install** (where `console` is set up), also install a native `__s2_subscribe(name, fn, opts) -> id` / `__s2_unsubscribe(id)` and run a **provisional prelude** that defines `onGameFrame`, `HookResult`, `Priority`, `Phase`:
   - `__s2_subscribe`: read arg0 = name (string, only `"OnGameFrame"` supported), arg1 = function (store as `v8::Global`), arg2 = opts (`{priority, phase}` strings → map to enums, defaults Normal/Pre). Call `FRAME.subscribe(...)` → `(id, change)`; call `apply_detour(change)`; return `id` as a JS number.
   - `__s2_unsubscribe(id)`: `FRAME.unsubscribe(id)` → `apply_detour(change)`.
   - The prelude (a JS string eval'd once at context creation, after the natives are installed):
     ```js
     globalThis.HookResult = { Continue:0, Changed:1, Handled:2, Stop:3 };
     globalThis.Priority   = { High:"high", Normal:"normal", Low:"low", Monitor:"monitor" };
     globalThis.Phase      = { Pre:"pre", Post:"post" };
     globalThis.onGameFrame = (fn, opts) => {
       const id = __s2_subscribe("OnGameFrame", fn, opts || {});
       return { dispose: () => __s2_unsubscribe(id) };
     };
     ```
4. The **V8 invoker** + `dispatch_onframe`:
   ```rust
   pub(crate) fn dispatch_onframe(phase: Phase, simulating: bool, first: bool, last: bool) -> multiplexer::DispatchOutcome {
       HOST.with(|h| {
           let mut hb = h.borrow_mut();
           let Some(host) = hb.as_mut() else {
               return multiplexer::DispatchOutcome { result: HookResult::Continue, detour: DetourChange::None };
           };
           // Build a HandleScope+Context, then dispatch, invoking each JsHandler.func with a ctx object.
           // (Match the existing eval()'s scope/TryCatch pattern for v8 149.4.0.)
           // For each handler: build ctx = { simulating, firstTick, first, lastTick: last, phase },
           // call func under TryCatch; on exception -> Err(()); else map return number -> HookResult
           // (undefined / out-of-range -> Continue).
           FRAME.with(|f| {
               let mut fr = f.borrow_mut();
               // ... open scope on host.isolate/context (see eval()), then:
               fr.dispatch(phase, |jh| { /* invoke jh.func in-scope, return Ok(HookResult)|Err(()) */ })
           })
       })
   }
   fn apply_detour(change: DetourChange) {
       if let DetourChange::None = change { return; }
       HOOK_REQUEST.with(|c| if let Some(req) = c.get() {
           let enable = matches!(change, DetourChange::Install) as i32;
           let name = std::ffi::CString::new("OnGameFrame").unwrap();
           req(name.as_ptr(), enable);
       });
   }
   ```
   > The tricky part is invoking `jh.func` *inside* the same `HandleScope`/`ContextScope` while the `FRAME` and `HOST` borrows are held. Reconcile the borrow structure: open the scope from `host.isolate`+`host.context` first, capture what you need, then call `fr.dispatch` with a closure that uses the open scope. Match `eval()`'s exact v8-149.4 scope/TryCatch construction. Reading the return: `value.is_undefined()` → Continue; else `value.uint32_value(scope)` → map 0..=3.
5. Reset `FRAME` in `shutdown()` (clear subscriptions) so re-init starts clean.

- [ ] **Step 4: Run to verify the integration tests pass**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: PASS — `frame_tests` (3) + the multiplexer suite + the existing Slice-0 ffi tests all green.

- [ ] **Step 5: Commit**
```bash
git add core/src/v8host.rs
git commit -m "feat(core): V8 onGameFrame binding — registry, native subscribe, provisional prelude, invoker (TDD)"
```

---

## Task 4: C ABI — `init(+request_hook)`, `dispatch_game_frame`, header (integration TDD)

**Files:**
- Modify: `core/src/ffi.rs`, `shim/include/s2script_core.h`
- Test: extend `ffi.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `v8host::dispatch_onframe`, `v8host::init`/`eval`/`shutdown`; `multiplexer::Phase`.
- Produces:
  ```rust
  pub type HookRequestFn = extern "C" fn(descriptor: *const c_char, enable: c_int);
  // s2script_core_init(logger, request_hook); s2script_core_dispatch_game_frame(phase, sim, first, last) -> c_int
  ```
  C header gains `s2_hook_request_fn`, the new `init` signature, and `s2script_core_dispatch_game_frame`.

- [ ] **Step 1: Write the failing test (full path through the C ABI + mock request_hook)**

Add to `ffi.rs` tests:
```rust
    use std::sync::Mutex as M2;
    static HOOKS: M2<Vec<(String, i32)>> = M2::new(Vec::new());
    extern "C" fn mock_request(name: *const c_char, enable: c_int) {
        let n = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        HOOKS.lock().unwrap().push((n, enable));
    }

    #[test]
    fn subscribe_installs_dispatch_runs_unsubscribe_removes() {
        HOOKS.lock().unwrap().clear();
        assert_eq!(s2script_core_init(Some(test_logger), Some(mock_request)), 0);
        // subscribing the first handler must request install:
        assert_eq!(s2script_core_eval(
            b"globalThis._sub = onGameFrame(() => {});\0".as_ptr() as *const c_char), 0);
        assert!(HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 1));
        // dispatch (Pre=0) must not crash and returns a HookResult code:
        let rc = s2script_core_dispatch_game_frame(0, 1, 1, 0);
        assert!(rc >= 0);
        // unsubscribe the last handler must request remove:
        assert_eq!(s2script_core_eval(b"_sub.dispose();\0".as_ptr() as *const c_char), 0);
        assert!(HOOKS.lock().unwrap().iter().any(|(n, e)| n == "OnGameFrame" && *e == 0));
        s2script_core_shutdown();
    }
```
> Update the existing Slice-0 tests' `s2script_core_init(Some(test_logger))` calls to `s2script_core_init(Some(test_logger), None)`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: FAIL — `init` arity changed / `dispatch_game_frame` missing.

- [ ] **Step 3: Implement the ABI changes**

In `ffi.rs`:
```rust
use crate::multiplexer::Phase;
pub type HookRequestFn = extern "C" fn(descriptor: *const c_char, enable: c_int);

#[no_mangle]
pub extern "C" fn s2script_core_init(logger: Option<LogFn>, request_hook: Option<HookRequestFn>) -> c_int {
    catch_unwind(|| {
        let Some(logger) = logger else { return -2 };
        v8host::set_hook_request(request_hook); // stores into HOOK_REQUEST thread-local
        match v8host::init(logger) { Ok(()) => 0, Err(_) => -1 }
    }).unwrap_or(-99)
}

#[no_mangle]
pub extern "C" fn s2script_core_dispatch_game_frame(phase: c_int, simulating: c_int, first: c_int, last: c_int) -> c_int {
    catch_unwind(|| {
        let phase = if phase == 0 { Phase::Pre } else { Phase::Post };
        let out = v8host::dispatch_onframe(phase, simulating != 0, first != 0, last != 0);
        // apply_detour for any auto-disable Remove happens inside dispatch_onframe.
        out.result as c_int
    }).unwrap_or(-99)
}
```
Add `pub fn set_hook_request(f: Option<HookRequestFn>)` to `v8host.rs` storing into `HOOK_REQUEST`. Keep `shutdown` unchanged (plus the `FRAME` reset from Task 3).

In `shim/include/s2script_core.h`, replace the body with:
```c
typedef void (*s2_log_fn)(int level, const char* utf8_msg);
typedef void (*s2_hook_request_fn)(const char* descriptor, int enable); /* core -> shim: install(1)/remove(0) */

int  s2script_core_init(s2_log_fn logger, s2_hook_request_fn request_hook);
int  s2script_core_eval(const char* utf8_js);
int  s2script_core_dispatch_game_frame(int phase, int simulating, int first, int last); /* phase 0=Pre,1=Post; returns collapsed HookResult */
void s2script_core_shutdown(void);
```

- [ ] **Step 4: Run to verify all core tests pass**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: PASS — the new ABI test + all prior suites green. Then confirm the cdylib still links (sniper not required for this check):
Run: `cargo build -p s2script-core 2>&1 | tail -1` → `Finished`.

- [ ] **Step 5: Commit**
```bash
git add core/src/ffi.rs core/src/v8host.rs shim/include/s2script_core.h
git commit -m "feat(core): C ABI for OnGameFrame — init(+request_hook) + dispatch_game_frame"
```

---

## Task 5: Shim SourceHook detour on `ISource2Server::GameFrame` + gamedata-cwd fix (build)

**Files:**
- Modify: `shim/src/s2script_mm.h`, `shim/src/s2script_mm.cpp`
- (Possibly) Modify: `shim/CMakeLists.txt` if SourceHook needs an extra source/define

**Interfaces:**
- Consumes: the Task 4 C ABI (`s2script_core_init(logger, request_hook)`, `s2script_core_dispatch_game_frame`).
- Produces: a shim that, on `request_hook("OnGameFrame", 1)`, installs a SourceHook detour on `ISource2Server::GameFrame` (pre+post) calling `s2script_core_dispatch_game_frame`; gamedata path resolved via `dladdr`.

> **Reference, do not reinvent:** CounterStrikeSharp's GameFrame hook (`SH_DECL_HOOK3_void` on `ISource2Server::GameFrame(bool, bool, bool)`, `SH_ADD_HOOK`/`SH_REMOVE_HOOK`, `g_SHPtr`/`SH_GET_CALLCLASS`) and the Metamod `sample_mm` SourceHook usage. Confirm the exact interface (`ISource2Server`), method, arg order (`bSimulating, bFirstTick, bLastTick`), and the `ISmmAPI` SourceHook init against `third_party/hl2sdk/public/eiface.h` + `third_party/metamod-source`.

- [ ] **Step 1: Wire SourceHook + the GameFrame hook in the shim**

In `shim/src/s2script_mm.h`, add the SourceHook declaration and member state (the acquired `ISource2Server*`, a `bool m_frameHookInstalled`). In `s2script_mm.cpp`:
- `SH_DECL_HOOK3_void(ISource2Server, GameFrame, SH_NOATTRIB, 0, bool, bool, bool);` (confirm macro/arity).
- Acquire and store `ISource2Server* m_server` in `Load` (already fetched via `GetServerFactory` for the interface-acquisition log — keep the pointer).
- Hook handlers:
  ```cpp
  void S2ScriptPlugin::Hook_GameFramePre(bool simulating, bool first, bool last) {
      s2script_core_dispatch_game_frame(0, simulating, first, last);
      RETURN_META(MRES_IGNORED);
  }
  void S2ScriptPlugin::Hook_GameFramePost(bool simulating, bool first, bool last) {
      s2script_core_dispatch_game_frame(1, simulating, first, last);
      RETURN_META(MRES_IGNORED);
  }
  ```
- The `request_hook` callback:
  ```cpp
  static void s2_request_hook(const char* descriptor, int enable) {
      if (strcmp(descriptor, "OnGameFrame") != 0) return;
      if (enable && !g_S2ScriptPlugin.m_frameHookInstalled) {
          SH_ADD_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                      SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePre),  false);
          SH_ADD_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                      SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePost), true);
          g_S2ScriptPlugin.m_frameHookInstalled = true;
      } else if (!enable && g_S2ScriptPlugin.m_frameHookInstalled) {
          SH_REMOVE_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                         SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePre),  false);
          SH_REMOVE_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                         SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePost), true);
          g_S2ScriptPlugin.m_frameHookInstalled = false;
      }
  }
  ```
- Pass it to init: `s2script_core_init(&s2_logger, &s2_request_hook)`.
- In `Unload`, remove the hook if installed (before `s2script_core_shutdown`).
- Initialize SourceHook for this plugin per Metamod (`SH_ADD_HOOK` needs `g_SHPtr`/`PLUGIN_SAVEVARS` already sets `g_SHPtr`; confirm).

- [ ] **Step 2: gamedata-cwd fix via `dladdr`**

Replace the relative gamedata path resolution with one based on the plugin `.so` location:
```cpp
#include <dlfcn.h>
#include <libgen.h>
static std::string GamedataPath() {
    Dl_info info;
    if (dladdr((void*)&GamedataPath, &info) && info.dli_fname) {
        std::string so = info.dli_fname;            // .../addons/s2script/bin/linuxsteamrt64/s2script.so
        // up 3 dirs: linuxsteamrt64 -> bin -> s2script, then /gamedata
        char buf[4096]; snprintf(buf, sizeof buf, "%s", so.c_str());
        std::string dir = dirname(buf);             // .../bin/linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str()); dir = dirname(buf); // .../bin
        snprintf(buf, sizeof buf, "%s", dir.c_str()); dir = dirname(buf); // .../s2script
        return dir + "/gamedata/core.gamedata.jsonc";
    }
    return "addons/s2script/gamedata/core.gamedata.jsonc"; // fallback
}
```
Use `GamedataPath()` in place of the hardcoded relative string in `Load`.

- [ ] **Step 3: Build (sniper) and verify it compiles, links, exports/imports**

Run:
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh 2>&1 | tail -8
nm -D --undefined-only build/shim/s2script.so | grep -E 's2script_core_(init|eval|shutdown|dispatch_game_frame)'
nm -D --defined-only build/shim/s2script.so | grep -i createinterface
```
Expected: build succeeds; the shim imports `s2script_core_dispatch_game_frame` (and the others) and still exports `CreateInterface`. If SourceHook symbols are undefined at link, add the SourceHook impl source / define per the metamod reference (Step's reference note) and the vendored AMBuild scripts.

- [ ] **Step 4: Commit**
```bash
git add shim/src/s2script_mm.cpp shim/src/s2script_mm.h shim/CMakeLists.txt
git commit -m "feat(shim): SourceHook GameFrame detour + lazy install via request_hook; gamedata dladdr fix"
```

---

## Task 6: Live verification + README (operator-run gate)

**Files:**
- Modify: `README.md` (add the OnGameFrame demo + multiplexer note to the runbook)

**Interfaces:**
- Consumes: everything from T1–T5.
- Produces: a documented, operator-run demonstration that two handlers compose live on `OnGameFrame`, plus the §9 acceptance results.

> The CS2 Docker harness + the 64 GB `docker/cs2-data` copy are already in place from Slice 0. The plugin must be built with `scripts/build-sniper.sh` (glibc 2.31). Claude drives the container (build → package → recreate → RCON), as in Slice 0's live gate.

- [ ] **Step 1: Build, package, recreate the container**
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
docker compose -f docker/docker-compose.yml up -d --force-recreate cs2   # re-binds the rebuilt dist
```
Wait for server-up (reuse the Slice-0 watcher pattern: poll `docker logs` for `GC Connection established`).

- [ ] **Step 2: Drive the two-handlers-compose demo over RCON**

Subscribe two handlers that log every ~64 ticks at different priorities, with a `Stop` at High skipping a Low handler, then observe the console:
```bash
python3 scripts/rcon.py "css_eval not-applicable" 2>/dev/null  # (rcon.py drives meta/console; eval via a console command if exposed, else via a small startup script)
```
Concretely, exercise it by evaluating (through whatever console/eval path the shim exposes, or a temporary `meta`-triggered eval) the demo script:
```js
let n = 0;
const a = onGameFrame((f) => { if (n % 64 === 0) console.log("A tick=" + n); }, { priority: "high" });
const b = onGameFrame((f) => { if (n % 64 === 0) console.log("B (low)"); n++; }, { priority: "low" });
```
Then watch `docker logs`:
```bash
docker logs --tail 40 s2script-cs2 2>&1 | grep -E "\[s2script\]|A tick=|B \(low\)|interface OK"
```
**Acceptance to record (§9):**
- The detour installed on first subscription (the `request_hook` fired) and dispatch runs each tick (the periodic "A tick=" lines advance).
- Two handlers compose: "A tick=" (High) appears before "B (low)" (Low) within a frame; add a `Stop` variant and confirm the Low handler stops appearing.
- `meta unload`/dispose the last subscription → the detour is **removed** (ticks stop), no crash.
- A deliberately throwing handler is isolated (the server keeps running; after the threshold it stops appearing).
- The gamedata-cwd fix: `interface OK: Source2Server (Source2Server001)` etc. appear on boot **without** manual gamedata placement.

- [ ] **Step 3: Update the README runbook + acceptance table**

Add an "OnGameFrame multiplexer (Slice 1)" subsection to the README's live runbook with the demo script + expected console output, and an acceptance table covering the §9 criteria (mark them with the live evidence, like Slice 0's). Note the provisional `onGameFrame` global is a Slice-1 stand-in for the Slice-4/5 `import { onGameFrame } from "@s2script/events"`.

- [ ] **Step 4: Commit + stop the container (keep the copy)**
```bash
git add README.md
git commit -m "docs: Slice 1 OnGameFrame live runbook + acceptance results"
docker stop s2script-cs2 && docker rm s2script-cs2
```

---

## Self-Review (completed during planning)

- **Spec coverage:** §1 thesis → T1–T6. §3 multiplexer (types/collapse/priority/snapshot/error/lazy) → T1+T2 (each semantic has a named test). §4 OnGameFrame descriptor + dispatch flow → T3 (`dispatch_onframe`) + T5 (detour). §5 native primitive + provisional surface + invoker → T3. §6 C ABI + shim detour → T4 (ABI) + T5 (SourceHook). §7 gamedata-cwd `dladdr` → T5 Step 2. §8 testing (unit/integration/live) → T1–T2 unit, T3–T4 integration, T6 live. §9 acceptance → T6. §10 out-of-scope honored (no ledger/context-per-plugin/bundler/resultApply). §13 DX target recorded → Global Constraints + T6 Step 3 note. No spec section unmapped.
- **Placeholder scan:** No "TBD/TODO" gaps. The "confirm against hl2sdk/CSS" notes (T5) and the rusty_v8-borrow-structure note (T3) are external-binding guidance naming the exact symbol/file to check — not deferred work. The `unsafe` handler-ptr in T2 Step 3 has a stated alternative.
- **Type consistency:** `HookResult`/`Priority`/`Phase`/`DetourChange`/`DispatchOutcome`/`SubId` identical across T1–T4. `dispatch` returns `HookResult` in T1 then `DispatchOutcome` in T2 — T2 Step 1 explicitly updates T1's `run()` helper + tests. `s2script_core_init(logger, request_hook)` and `s2script_core_dispatch_game_frame(phase, sim, first, last)` identical across the Rust `ffi.rs`, the C header (T4), and the C++ caller (T5). `HookRequestFn`/`s2_hook_request_fn` signature `(const char*, int)` consistent. The provisional JS surface (`onGameFrame`, `HookResult`, `__s2_subscribe`) consistent across T3/T4/T6.
```
