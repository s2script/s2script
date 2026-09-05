//! Fan-out invocation policy — the one place a snapshot of JS handlers is called.
//!
//! Feature modules snapshot their mux and hand the list here. This module owns the
//! re-entrancy / nest / isolate-borrow decision, the per-subscriber liveness + context
//! walk, TryCatch isolation, HookResult collapse, and `Delivery`. It does **not** own
//! hook views, `ACTIVE_HOOK`, acquire sessions, argument construction, `dispatch_onframe`,
//! TopMenu, or inter-plugin emit — those stay in `v8host`.
//!
//! Isolate facts arrive through a narrow adapter (`HostAccess`, `with_host_isolate`,
//! `owner_is_live`, `clone_plugin_context`). `HOST` / `PLUGINS` / `REGISTRY` are never
//! named here.
//!
//! `nest::top()` is consulted **before** any host-isolate borrow. A non-null nest token
//! uses `CallbackScope` and never takes `HOST`. `#63` (Busy) still applies when the
//! stack is empty.
//!
//! `AFTER_HANDLER` is host-path-only: the pickup-gate acquire fold installs a setter
//! around `fan_out_inner`, and only the host-borrow subscriber walk invokes it. The
//! nested CallbackScope walk does not.

use crate::multiplexer::HookResult;
use crate::v8host::{clone_plugin_context, log_warn, owner_is_live, with_host_isolate, HostAccess};

thread_local! {
    /// Called after each collapsing handler with that handler's HookResult. Used by the
    /// return-value pickup gate to collect per-handler votes (Continue is not a vote).
    ///
    /// Invoked only on the host-borrow walk. Nested CallbackScope fan-out never calls it.
    static AFTER_HANDLER: std::cell::Cell<Option<fn(HookResult)>> =
        const { std::cell::Cell::new(None) };
}

thread_local! {
    // One epoch for an outer fan-out and all nested callbacks. Pool resources released by an
    // early subscriber cannot be reassigned to a later subscriber of the same input event.
    static DISPATCH_EPOCH: std::cell::Cell<(u64, usize)> = const { std::cell::Cell::new((0, 0)) };
}

pub(crate) fn current_epoch() -> Option<u64> {
    DISPATCH_EPOCH.with(|state| { let (epoch, depth) = state.get(); (depth > 0).then_some(epoch) })
}

pub(crate) struct DispatchScope;
impl DispatchScope {
    pub(crate) fn enter() -> Self {
        DISPATCH_EPOCH.with(|state| {
            let (epoch, depth) = state.get();
            state.set((if depth == 0 { epoch.wrapping_add(1) } else { epoch }, depth + 1));
        });
        Self
    }
}
impl Drop for DispatchScope {
    fn drop(&mut self) {
        DISPATCH_EPOCH.with(|state| {
            let (epoch, depth) = state.get();
            state.set((epoch, depth - 1));
        });
    }
}

/// Crate-private setter used by acquire folding. Returns the previous handler so the
/// caller can save/restore around a nested dispatch.
pub(crate) fn set_after_handler(f: Option<fn(HookResult)>) -> Option<fn(HookResult)> {
    AFTER_HANDLER.with(|c| c.replace(f))
}

/// THE dispatch preamble, owned once.
///
/// Every engine→JS notification used to hand-roll this same six-part sequence, and the copies had
/// drifted: only some reported through `report_js_error`, only some left a crash breadcrumb, and
/// `apply_errors` (auto-disable for a handler that keeps throwing) reached exactly one of them.
/// Each part is load-bearing and none may be dropped when a call site is converted:
///
/// 1. **Snapshot before invoke** — the caller passes an already-taken snapshot, so the mux borrow is
///    released before any JS runs. A handler that subscribes or unsubscribes mid-dispatch therefore
///    cannot invalidate the list being walked.
/// 2. **`try_borrow_mut` re-entrancy guard** — core holds `HOST.borrow_mut()` across ALL JS, so a
///    handler that re-enters dispatch with no nest token (`#63` C-ABI inbound) would double-borrow.
///    Graceful skip, never a panic.
/// 3. **Per-subscriber liveness** — an owner unloaded earlier in THIS fan-out must not be called;
///    the registry borrow is released before entering the context.
/// 4. **Context clone out of the plugin table** — the borrow is released before the call so the
///    handler may re-enter the plugin table.
/// 5. **Per-subscriber `HandleScope`/`ContextScope`** — handles are freed per subscriber rather than
///    accumulating across the whole fan-out.
/// 6. **Per-handler `TryCatch`** — one throwing handler must not deny the others their dispatch.
///
/// `build_args` runs inside the per-subscriber `TryCatch`, which is why it takes the scope: the
/// argument `Local`s must be created in the scope they are passed to, and they cannot outlive it.
/// Per-dispatch crash instrumentation, opt-in per call site.
///
/// Both of these were ad-hoc in the hand-rolled copies, and the coverage gaps are invisible once the
/// preamble is shared: `enter_dispatch` (the signal-safe breadcrumb naming the culprit plugin if the
/// process faults inside a handler) reached 4 of ~20 paths, and `report_js_error` (filing the throw
/// with the crash reporter) reached 2. Making them explicit arguments preserves each path's current
/// behaviour exactly while turning "which paths are instrumented?" into something you can read off
/// the call sites — and turning "instrument this one too" into a one-word change.
#[derive(Clone, Copy)]
pub(crate) struct Instrument<'a> {
    /// Breadcrumb dispatch tag held across the handler call, e.g. `"event:player_death"`.
    pub breadcrumb: Option<&'a str>,
    /// Crash-reporter context tag for a throwing handler. `None` = WARN only.
    pub report_as: Option<&'a str>,
}

impl<'a> Instrument<'a> {
    /// WARN on throw, no breadcrumb, no crash report — what most notify paths do today.
    pub(crate) fn none() -> Self {
        Self {
            breadcrumb: None,
            report_as: None,
        }
    }
    /// Breadcrumb only (today: the damage and frame paths).
    pub(crate) fn breadcrumb(tag: &'a str) -> Self {
        Self {
            breadcrumb: Some(tag),
            report_as: None,
        }
    }
    /// Breadcrumb + crash report under the same tag (today: the game-event and concommand paths).
    pub(crate) fn full(tag: &'a str) -> Self {
        Self {
            breadcrumb: Some(tag),
            report_as: Some(tag),
        }
    }
}

/// How far a fan-out lets a handler's return value truncate the chain.
///
/// Three policies because the hand-rolled copies genuinely had three, and TWO of them are correct:
///
/// * `Never` — notify paths. The return value is not read at all, so a handler that happens to
///   return a number cannot silently veto later subscribers.
/// * `Stop` — the standard collapse, matching `multiplexer::run_chain`: `Handled` is a return value,
///   NOT a veto over other plugins' observers; only `Stop` truncates. `multiplexer`'s own
///   `handled_does_not_short_circuit` test is the authority.
/// * `Handled` — the DOCUMENTED exception, used only by the chat path. A chat line is consumed
///   ONCE: the menu model's "one active menu per slot" is per-ISOLATE, so a shop menu, a nominate
///   menu and a live vote each believed they were the only one, and typing "2" picked a shop item
///   AND a map AND cast a ballot in one keystroke. Claiming the line means nothing after sees it.
///
/// The distinction matters: `dispatch_damage` truncated at `Handled` too, but as a BUG (one
/// plugin's `Handled` silently disabling every other plugin's damage observer), not as this
/// deliberate consumed-once semantic. Converting both to one policy would have fixed damage and
/// broken chat.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopAt {
    Never,
    Stop,
    Handled,
}

/// Did a NOTIFY dispatch actually reach JS, or must the caller replay it one drain later?
///
/// Core holds `HOST.borrow_mut()` across ALL JS, so a handler that causes the engine to
/// synchronously dispatch back into core hits a failed `try_borrow_mut` on the inner dispatch.
/// Historically that dispatch was **silently dropped** — no error, no log, presenting only as "my
/// plugin stopped getting events". `Deferred` is that condition made visible: core delivered
/// NOTHING and the caller (the shim, which still owns the arguments) must queue a replay for the
/// next `GameFrame`.
///
/// Only **notify-only** dispatches can carry this. A pre-hook returns a `HookResult` the engine
/// consumes synchronously; there is no answer to give a frame later, so pre-hooks keep today's
/// graceful skip and `fan_out_collapsing` never reports `Deferred` to anyone.
///
/// `#[must_use]`: dropping a `Delivery` is exactly the silent drop this type exists to end.
#[must_use]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Delivery {
    /// The fan-out ran (or there was nobody to run it for). Nothing to replay.
    Delivered,
    /// `HOST` was already borrowed and the snapshot was non-empty: nothing ran. Replay next frame.
    Deferred,
}

pub(crate) fn fan_out<F>(
    snap: &[(String, u64, v8::Global<v8::Function>)],
    label: &str,
    instrument: Instrument<'_>,
    build_args: F,
) -> Delivery
where
    F: for<'s> Fn(&mut v8::PinScope<'s, '_>) -> Option<Vec<v8::Local<'s, v8::Value>>>,
{
    fan_out_inner(snap, label, instrument, StopAt::Never, build_args).1
}

/// `fan_out` with the handlers' return values collapsed into a single `HookResult`.
///
/// Collapse rule is `multiplexer::run_chain`'s, reproduced here because these paths snapshot from an
/// `EventMux` (no per-subscription id or priority) rather than a `Descriptor`: take the MAX of the
/// returned results, and truncate the remainder once one reaches `stop_at`. A non-number return (or
/// `undefined`) is `Continue`; a handler that throws is `Continue` and does NOT truncate.
///
/// NOTE: no `Priority::Monitor` handling, because these paths have no priority — every subscriber is
/// effectively Normal and runs in subscription order. That is today's behaviour (the callers
/// synthesised `Priority::Normal` for every row before passing to `run_chain`), and it is why
/// `events.d.ts` currently promises ordering semantics the runtime cannot deliver. Wiring real
/// priority through is a separate, contract-affecting change.
pub(crate) fn fan_out_collapsing<F>(
    snap: &[(String, u64, v8::Global<v8::Function>)],
    label: &str,
    instrument: Instrument<'_>,
    stop_at: StopAt,
    build_args: F,
) -> HookResult
where
    F: for<'s> Fn(&mut v8::PinScope<'s, '_>) -> Option<Vec<v8::Local<'s, v8::Value>>>,
{
    // The collapsing paths are the pre/collapsing entries: the engine consumes their answer
    // synchronously, so a re-entrant one has nobody to answer a frame later. Their `Delivery` is
    // deliberately DISCARDED here and never surfaces — that is the documented, permanent limitation
    // (spec §2), not an oversight.
    fan_out_inner(snap, label, instrument, stop_at, build_args).0
}

/// Inbound fan-out while a plugin native is paused in engine FFI. Uses the published
/// `FunctionCallbackInfo` — never `HOST`, never a second `&mut Isolate`.
fn fan_out_nested<F>(
    info: *const v8::FunctionCallbackInfo,
    snap: &[(String, u64, v8::Global<v8::Function>)],
    label: &str,
    instrument: Instrument<'_>,
    stop_at: StopAt,
    build_args: F,
) -> (HookResult, Delivery)
where
    F: for<'s> Fn(&mut v8::PinScope<'s, '_>) -> Option<Vec<v8::Local<'s, v8::Value>>>,
{
    let info = unsafe { &*info };
    let mut storage = unsafe { v8::CallbackScope::new(info) };
    let mut cs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let cs = &mut cs;
    fan_out_call_subscribers(cs, snap, label, instrument, stop_at, build_args)
}

fn fan_out_call_subscribers<F>(
    parent: &mut v8::PinScope,
    snap: &[(String, u64, v8::Global<v8::Function>)],
    label: &str,
    instrument: Instrument<'_>,
    stop_at: StopAt,
    build_args: F,
) -> (HookResult, Delivery)
where
    F: for<'s> Fn(&mut v8::PinScope<'s, '_>) -> Option<Vec<v8::Local<'s, v8::Value>>>,
{
    let mut result = HookResult::Continue;
    for (owner, generation, handler_g) in snap {
        if stop_at != StopAt::Never {
            let truncated = match stop_at {
                StopAt::Stop => result == HookResult::Stop,
                StopAt::Handled => result >= HookResult::Handled,
                StopAt::Never => false,
            };
            if truncated {
                break;
            }
        }
        if !owner_is_live(owner, *generation) {
            continue;
        }
        let Some(g_ctx) = clone_plugin_context(owner) else {
            continue;
        };

        let _crash_guard = instrument
            .breadcrumb
            .map(|tag| crate::crash::breadcrumb::enter_dispatch(owner, tag));

        let ctx_local = v8::Local::new(parent, &g_ctx);
        let scope = &mut v8::ContextScope::new(parent, ctx_local);

        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

        let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
        let Some(args) = build_args(tc) else { continue };
        let func = v8::Local::new(tc, handler_g);
        match func.call(tc, recv, &args) {
            None => {
                let msg = tc
                    .exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: {}: handler '{}': {}", label, owner, msg));
                if let Some(context) = instrument.report_as {
                    let stack = tc
                        .stack_trace()
                        .map(|s| s.to_rust_string_lossy(&*tc))
                        .unwrap_or_default();
                    crate::crash::report_js_error(owner, context, &msg, &stack);
                }
            }
            Some(ret) if stop_at != StopAt::Never && ret.is_number() => {
                let r = match ret.uint32_value(tc).unwrap_or(0) {
                    1 => HookResult::Changed,
                    2 => HookResult::Handled,
                    3 => HookResult::Stop,
                    _ => HookResult::Continue,
                };
                if r > result {
                    result = r;
                }
            }
            Some(_) => {}
        }
    }
    (result, Delivery::Delivered)
}

/// The one fan-out body. Returns BOTH the collapsed `HookResult` (for `fan_out_collapsing`) and
/// whether anything ran at all (for `fan_out`). Keeping them in one function is the point: the two
/// public wrappers must not drift in re-entrancy discipline, and the borrow-failure signal exists in
/// exactly one place.
///
/// `dispatch_hook` / `dispatch_hook_post` call this directly so they can inspect `Delivery` (those
/// pre-hooks cannot be replayed; a `Deferred` is a named skip).
thread_local! {
    static DEFER_CALLBACKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Owner-store engine follow-ups must not re-enter JavaScript while the store registry is borrowed.
/// Notify events use the usual deferred replay; synchronous pre-hooks follow its normal skip policy.
pub(crate) fn defer_while<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) { DEFER_CALLBACKS.with(|v| v.set(self.0)); }
    }
    let _restore = Restore(DEFER_CALLBACKS.with(|v| v.replace(true)));
    f()
}

pub(crate) fn fan_out_inner<F>(
    snap: &[(String, u64, v8::Global<v8::Function>)],
    label: &str,
    instrument: Instrument<'_>,
    stop_at: StopAt,
    build_args: F,
) -> (HookResult, Delivery)
where
    F: for<'s> Fn(&mut v8::PinScope<'s, '_>) -> Option<Vec<v8::Local<'s, v8::Value>>>,
{
    let _dispatch = DispatchScope::enter();
    if snap.is_empty() {
        // Nobody subscribed → nothing to replay. Reporting `Deferred` here would make the shim
        // `DuplicateEvent` every event on the server with no one listening.
        return (HookResult::Continue, Delivery::Delivered);
    }
    if DEFER_CALLBACKS.with(std::cell::Cell::get) {
        return (HookResult::Continue, Delivery::Deferred);
    }
    // Plugin-originated outbound: a FunctionCallbackInfo is on the stack. Nested CallbackScope
    // does not take HOST (promise_reject_cb). #63 still applies when the stack is empty.
    if let Some(info) = crate::nest::top().filter(|p| !p.is_null()) {
        return fan_out_nested(info, snap, label, instrument, stop_at, build_args);
    }
    match with_host_isolate(|isolate| {
        let mut result = HookResult::Continue;
        for (owner, generation, handler_g) in snap {
            if stop_at != StopAt::Never {
                let truncated = match stop_at {
                    StopAt::Stop => result == HookResult::Stop,
                    StopAt::Handled => result >= HookResult::Handled,
                    StopAt::Never => false,
                };
                if truncated {
                    break;
                }
            }
            if !owner_is_live(owner, *generation) {
                continue;
            }
            let Some(g_ctx) = clone_plugin_context(owner) else {
                continue;
            };

            // Held across the handler call: if the process faults inside JS, the breadcrumb names
            // the culprit plugin and this dispatch.
            let _crash_guard = instrument
                .breadcrumb
                .map(|tag| crate::crash::breadcrumb::enter_dispatch(owner, tag));

            let mut hs_storage = v8::HandleScope::new(isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            // `None` = skip THIS subscriber, preserving the per-call-site `continue` used when a
            // `v8::String::new` allocation fails. Returning an empty Vec instead would call the
            // handler with its arguments silently missing.
            let Some(args) = build_args(tc) else { continue };
            let func = v8::Local::new(tc, handler_g);
            let hr = match func.call(tc, recv, &args) {
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "handler threw".into());
                    log_warn(&format!("WARN: {}: handler '{}': {}", label, owner, msg));
                    if let Some(context) = instrument.report_as {
                        let stack = tc
                            .stack_trace()
                            .map(|s| s.to_rust_string_lossy(&*tc))
                            .unwrap_or_default();
                        crate::crash::report_js_error(owner, context, &msg, &stack);
                    }
                    HookResult::Continue
                }
                Some(ret) if stop_at != StopAt::Never && ret.is_number() => {
                    // Out-of-range => Continue, NOT Stop. A handler returning a garbage number
                    // must not be able to truncate every other plugin's dispatch — that is the same
                    // composition failure as the damage `Handled` short-circuit. This mapping is
                    // what the run_chain paths already documented.
                    let r = match ret.uint32_value(tc).unwrap_or(0) {
                        1 => HookResult::Changed,
                        2 => HookResult::Handled,
                        3 => HookResult::Stop,
                        _ => HookResult::Continue,
                    };
                    if r > result {
                        result = r;
                    }
                    r
                }
                Some(_) => HookResult::Continue,
            };
            AFTER_HANDLER.with(|c| {
                if let Some(f) = c.get() {
                    f(hr);
                }
            });
        }
        (result, Delivery::Delivered)
    }) {
        Ok(pair) => pair,
        Err(HostAccess::Busy) => (HookResult::Continue, Delivery::Deferred),
        Err(HostAccess::Absent) => (HookResult::Continue, Delivery::Delivered),
    }
}
