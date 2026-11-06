//! The JS multiplexer (performance plan, Stage 1): one Rust→JS crossing per event.
//!
//! `dispatch::fan_out` enters every subscriber's context from Rust: a context clone, a
//! `HandleScope`, a `ContextScope`, a `TryCatch` and a freshly built payload per subscriber. Most of
//! that is fixed cost, and it dominated: about 730 ns per subscriber for a game event. Here Rust
//! crosses once, into a dedicated context running `core/js/mux.js`, and the subscriber loop runs in
//! JS, where calling a function from another context is an ordinary call. Each of `fan_out`'s
//! guarantees keeps a home:
//!
//! * **Snapshot before invoke.** Rust's stores (`Channels`, and the `OnGameFrame` `Descriptor`)
//!   stay the one source of truth. JS caches each channel's array and gets a fresh one whenever the
//!   store's version has moved; a dispatch walks the array it started with.
//! * **Re-entrancy, nest tokens, `#63` Busy → `Delivery::Deferred`, `defer_while`.** Unchanged, and
//!   decided here before any JS runs, exactly as `fan_out_inner` decides them.
//! * **Owner liveness.** `enter(slot, generation)` checks the registry by slot (`Registry::live_at`).
//! * **Crash breadcrumb.** Rust names the dispatch once; `enter` stamps each plugin and its ring
//!   entry, so the breadcrumb reads as it does for a Rust fan-out.
//! * **The calling plugin.** A handler runs with this context as the *entered* context. `enter`
//!   records the dispatch owner, and `function_adapter::entered_owner` answers with it, so checks
//!   that ask "which plugin entered?" (interop record views, package tokens) see the subscriber.
//! * **Per-handler isolation.** A `try/catch` per handler in JS; `thrown` WARNs and reports.
//! * **Profiler spans.** While `sm prof` records, dispatch takes the Rust fan-out instead, so a
//!   session attributes every callback exactly as it always has. With profiling off this costs a
//!   thread-local load.
//!
//! `OnGameFrame` keeps its own rules on the same machinery ([`dispatch_frame`]): priorities with
//! `Monitor` (runs after a `Stop`, return ignored), errored ids handed back for `apply_errors`
//! auto-disable, no WARN on a throw, the out-of-range WARN, the handler's source line in the
//! breadcrumb, and no `DispatchScope` epoch.
//!
//! `ParentBusy::enter` is not carried over: its untargeted entries are invisible to `parent_busy`,
//! which only matches an entry carrying the function target being dispatched.
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::thread::LocalKey;

use crate::channels::Channels;
use crate::dispatch::{callbacks_deferred, Delivery, DispatchScope, StopAt};
use crate::multiplexer::HookResult;
use crate::v8host::{clone_plugin_context, log_warn, owner_is_live, owner_slot, slot_owner, with_host_isolate, with_live_slot, HostAccess};

/// The channel stores routed through the multiplexer. The discriminant is the store's index on
/// the JS side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// Post game events: notify only, breadcrumb and crash report under `event:<name>`.
    Event = 0,
    /// Pre game events (the FireEvent hook): collapsing, WARN only.
    EventPre = 1,
    /// `Chat.onMessage` / `onSay` lines (one channel, `""`): consumed once, WARN only.
    ChatMessage = 2,
    /// `Commands.onClientCommand(name, …)` listeners: collapsing, WARN only.
    ClientCommand = 3,
    /// `OnGameFrame`, channels `"pre"` and `"post"` (see [`dispatch_frame`]).
    Frame = 4,
}
const KINDS: usize = 5;

/// What each subscriber is handed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Payload {
    /// `new GameEvent(name)`, built by the subscriber's own realm.
    GameEvent,
    /// The same plain arguments for every subscriber.
    Args,
    /// A `{ simulating, firstTick, lastTick, phase }` object built in the subscriber's own realm.
    FrameCtx,
}

impl Kind {
    fn label(self, name: &str) -> String {
        match self {
            Kind::Event => format!("dispatch_game_event('{}')", name),
            Kind::EventPre => format!("dispatch_game_event_pre('{}')", name),
            Kind::ChatMessage => "dispatch_chat: onSay".to_string(),
            Kind::ClientCommand => format!("dispatch_client_command on '{}'", name),
            Kind::Frame => format!("OnGameFrame ({})", name),
        }
    }
    /// The breadcrumb and crash-report tag (`Instrument::full`), or `None` (`Instrument::none`).
    fn tag(self, name: &str) -> Option<String> {
        match self {
            Kind::Event => Some(format!("event:{}", name)),
            Kind::Frame => Some(format!("OnGameFrame:{}", name)),
            Kind::EventPre | Kind::ChatMessage | Kind::ClientCommand => None,
        }
    }
    fn payload(self) -> Payload {
        match self {
            Kind::Event | Kind::EventPre => Payload::GameEvent,
            Kind::ChatMessage | Kind::ClientCommand => Payload::Args,
            Kind::Frame => Payload::FrameCtx,
        }
    }
}

/// One subscriber as a store reports it.
pub(crate) struct Row {
    pub id: u64,
    pub owner: String,
    pub generation: u64,
    /// `Priority::Monitor`: runs even after a `Stop`; its return is ignored. Frame channels only.
    pub monitor: bool,
    pub handler: v8::Global<v8::Function>,
}

/// A subscriber store the multiplexer can mirror.
pub(crate) trait Source {
    /// Whether `name` has a subscriber, and the store's version, which moves whenever `rows` could
    /// change (for any name).
    fn probe(&self, name: &str) -> (bool, u64);
    /// The subscribers of `name`, in dispatch order.
    fn rows(&self, name: &str) -> Vec<Row>;
}

/// A `Channels` store as a [`Source`].
pub(crate) struct ChannelStore(pub &'static LocalKey<RefCell<Channels<v8::Global<v8::Function>>>>);

impl Source for ChannelStore {
    fn probe(&self, name: &str) -> (bool, u64) {
        self.0.with(|m| {
            let m = m.borrow();
            (m.has_subscribers(name), m.version())
        })
    }
    fn rows(&self, name: &str) -> Vec<Row> {
        self.0.with(|m| m.borrow().snapshot(name))
            .into_iter()
            .map(|(owner, generation, handler)| Row { id: 0, owner, generation, monitor: false, handler })
            .collect()
    }
}

struct State {
    context: v8::Global<v8::Context>,
    set: v8::Global<v8::Function>,
    drop: v8::Global<v8::Function>,
    dispatch: v8::Global<v8::Function>,
    frame: v8::Global<v8::Function>,
}

/// One per crossing in progress (crossings nest when a handler fires another event).
struct Frame {
    kind: Kind,
    name: String,
    breadcrumb: bool,
    owner: Option<(u32, u64)>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Per kind: the store version each channel's JS array was built at.
    static PUSHED: RefCell<[HashMap<String, (u64, bool)>; KINDS]> = RefCell::new(Default::default());
    /// Per kind: the version at which stale arrays were last dropped from JS.
    static PRUNED: RefCell<[u64; KINDS]> = const { RefCell::new([0; KINDS]) };
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    /// Benchmarks only: take the Rust fallback without a profiler session's span costs.
    pub(crate) static FORCE_FALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Take the Rust fan-out instead: while the profiler records (and, in tests, when forced).
fn use_fallback() -> bool {
    #[cfg(test)]
    if FORCE_FALLBACK.with(std::cell::Cell::get) {
        return true;
    }
    crate::profiler::active()
}

/// Drop every handle. Registered as a process singleton (`BeforeIsolateDrop`).
pub(crate) fn reset() {
    STATE.with(|s| s.borrow_mut().take());
    PUSHED.with(|p| p.borrow_mut().iter_mut().for_each(HashMap::clear));
    PRUNED.with(|p| *p.borrow_mut() = [0; KINDS]);
    FRAMES.with(|f| f.borrow_mut().clear());
}

#[cfg(test)]
thread_local! {
    static HIDDEN_CTOR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Act as if `owner`'s prelude had not published its payload class yet.
#[cfg(test)]
pub(crate) fn test_hide_ctor(owner: Option<&str>) {
    HIDDEN_CTOR.with(|h| *h.borrow_mut() = owner.map(str::to_string));
}

/// How many channels of `kind` have an array in JS.
#[cfg(test)]
pub(crate) fn test_pushed(kind: Kind) -> usize {
    PUSHED.with(|p| p.borrow()[kind as usize].len())
}

/// The plugin a handler is running for, when the entered context is the multiplexer's.
pub(crate) fn dispatch_owner() -> Option<(String, u64)> {
    let (slot, generation) = FRAMES.with(|f| f.borrow().last().and_then(|fr| fr.owner))?;
    with_live_slot(slot, generation, |id| (id.to_string(), generation))
}

/// Is `ctx` the multiplexer's context?
///
/// Compared by identity rather than marked with a context slot: slots need retiring before the
/// isolate drops, and this context is not in `Host::contexts`.
pub(crate) fn is_mux_context(ctx: v8::Local<v8::Context>) -> bool {
    STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|st| st.context == ctx)).unwrap_or(false))
}

/// What a crossing produced: the collapsed result and, for frame channels, the ids of handlers
/// that threw.
struct Outcome {
    result: HookResult,
    errored: Vec<u64>,
}

const CONTINUE: Outcome = Outcome { result: HookResult::Continue, errored: Vec::new() };

/// Dispatch `name` on `source` through the multiplexer. `args` builds the arguments every
/// subscriber receives (at most three; unused for game events, whose payload is per subscriber).
/// `fallback` is the Rust fan-out for the same dispatch, taken while the profiler records.
pub(crate) fn dispatch<A>(
    source: &dyn Source,
    kind: Kind,
    name: &str,
    stop_at: StopAt,
    args: A,
    fallback: impl FnOnce() -> (HookResult, Delivery),
) -> (HookResult, Delivery)
where
    A: for<'s> Fn(&mut v8::PinScope<'s, '_>) -> Vec<v8::Local<'s, v8::Value>>,
{
    debug_assert!(kind != Kind::Frame, "frames go through dispatch_frame");
    if use_fallback() {
        return fallback();
    }
    let _dispatch = DispatchScope::enter();
    let (outcome, delivery) = cross(source, kind, name, |tc, kind_v, name_v| {
        let stop = match stop_at {
            StopAt::Never => 0,
            StopAt::Stop => 1,
            StopAt::Handled => 2,
        };
        let payload = args(tc);
        debug_assert!(payload.len() <= 3, "the multiplexer passes at most three shared arguments");
        let mut argv: Vec<v8::Local<v8::Value>> = vec![
            kind_v,
            name_v,
            v8::Integer::new(tc, stop).into(),
            v8::Integer::new(tc, payload.len().min(3) as i32).into(),
        ];
        argv.extend(payload.into_iter().take(3));
        (|st: &State| st.dispatch.clone(), argv)
    });
    (outcome.result, delivery)
}

/// `OnGameFrame` through the multiplexer: the collapsed result and the ids of handlers that threw
/// (for `Descriptor::apply_errors`). `fallback` is the Rust chain, taken while the profiler records.
pub(crate) fn dispatch_frame(
    source: &dyn Source,
    pre: bool,
    simulating: bool,
    first: bool,
    last: bool,
    fallback: impl FnOnce() -> crate::multiplexer::ChainOutcome,
) -> crate::multiplexer::ChainOutcome {
    if use_fallback() {
        return fallback();
    }
    let phase = if pre { "pre" } else { "post" };
    let (outcome, _) = cross(source, Kind::Frame, phase, |tc, kind_v, name_v| {
        let argv: Vec<v8::Local<v8::Value>> = vec![
            kind_v,
            name_v,
            v8::Boolean::new(tc, simulating).into(),
            v8::Boolean::new(tc, first).into(),
            v8::Boolean::new(tc, last).into(),
        ];
        (|st: &State| st.frame.clone(), argv)
    });
    crate::multiplexer::ChainOutcome { result: outcome.result, errored: outcome.errored }
}

/// A frame phase with no enabled subscriber skips dispatch before it reaches the multiplexer; this
/// still drops the arrays its last subscribers left in JS.
pub(crate) fn release_idle_frame(source: &dyn Source, pre: bool) {
    let name = if pre { "pre" } else { "post" };
    let (any, version) = source.probe(name);
    if !any {
        prune_idle(&Job { source, kind: Kind::Frame, name, version });
    }
}

/// With nobody subscribed, drop stale arrays if the store moved since the last prune: once a store
/// empties no dispatch would reach `sync`, and those arrays pin the unloaded plugins' contexts.
/// Best effort; a busy host retries on the next dispatch.
fn prune_idle(job: &Job) {
    if job.prune_due() && !callbacks_deferred() {
        let _ = enter(|hs| {
            if let Some(context) = ensure_state(hs) {
                let cs = &mut v8::ContextScope::new(hs, context);
                job.prune(cs);
            }
        });
    }
}

type Entry = fn(&State) -> v8::Global<v8::Function>;

/// The one crossing: the re-entrancy decision, then the multiplexer call. `call` picks the JS
/// entry point and builds its arguments after the kind and channel name.
fn cross<C>(source: &dyn Source, kind: Kind, name: &str, call: C) -> (Outcome, Delivery)
where
    C: for<'s> FnOnce(
        &mut v8::PinScope<'s, '_>,
        v8::Local<'s, v8::Value>,
        v8::Local<'s, v8::Value>,
    ) -> (Entry, Vec<v8::Local<'s, v8::Value>>),
{
    let (any, version) = source.probe(name);
    let job = Job { source, kind, name, version };
    if !any {
        // Nobody subscribed → nothing to replay (see `fan_out_inner`).
        prune_idle(&job);
        return (CONTINUE, Delivery::Delivered);
    }
    if callbacks_deferred() {
        return (CONTINUE, Delivery::Deferred);
    }
    match enter(|hs| job.run(hs, call)) {
        Ok(outcome) => (outcome, Delivery::Delivered),
        Err(HostAccess::Busy) => (CONTINUE, Delivery::Deferred),
        Err(HostAccess::Absent) => (CONTINUE, Delivery::Delivered),
    }
}

/// Enter V8: nested under the paused native's callback info when the dispatch is
/// plugin-originated, else through `HOST`.
fn enter<R>(f: impl FnOnce(&mut v8::PinScope<'_, '_, ()>) -> R) -> Result<R, HostAccess> {
    if let Some(info) = crate::nest::top().filter(|p| !p.is_null()) {
        // Plugin-originated: a native is paused in engine FFI. Nest under its callback info,
        // never `HOST`.
        let info = unsafe { &*info };
        let mut storage = unsafe { v8::CallbackScope::new(info) };
        let mut cs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let mut hs_storage = v8::HandleScope::new(&mut cs);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        return Ok(f(&mut hs));
    }
    with_host_isolate(|isolate| {
        let mut hs_storage = v8::HandleScope::new(isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        f(&mut hs)
    })
}

struct Job<'a> {
    source: &'a dyn Source,
    kind: Kind,
    name: &'a str,
    version: u64,
}

struct FrameGuard;
impl Drop for FrameGuard {
    fn drop(&mut self) {
        FRAMES.with(|f| {
            f.borrow_mut().pop();
        });
    }
}

fn hook_result(n: i32) -> HookResult {
    match n {
        1 => HookResult::Changed,
        2 => HookResult::Handled,
        3 => HookResult::Stop,
        _ => HookResult::Continue,
    }
}

impl Job<'_> {
    fn run<'s, C>(&self, hs: &mut v8::PinScope<'s, '_, ()>, call: C) -> Outcome
    where
        C: for<'t> FnOnce(
            &mut v8::PinScope<'t, '_>,
            v8::Local<'t, v8::Value>,
            v8::Local<'t, v8::Value>,
        ) -> (Entry, Vec<v8::Local<'t, v8::Value>>),
    {
        let Some(context) = ensure_state(hs) else { return CONTINUE };
        let cs = &mut v8::ContextScope::new(hs, context);
        let Some(name) = v8::String::new(cs, self.name) else { return CONTINUE };
        self.sync(cs, name);

        let tag = self.kind.tag(self.name);
        let _crumb = tag.as_deref().map(crate::crash::breadcrumb::enter_dispatch_tag);
        FRAMES.with(|f| {
            f.borrow_mut().push(Frame {
                kind: self.kind,
                name: self.name.to_string(),
                breadcrumb: tag.is_some(),
                owner: None,
            })
        });
        let _frame = FrameGuard;

        let mut tc_storage = v8::TryCatch::new(cs);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;
        let kind_v = v8::Integer::new(tc, self.kind as i32).into();
        let (entry, argv) = call(tc, kind_v, name.into());
        let Some(func) = STATE.with(|s| s.borrow().as_ref().map(entry)) else { return CONTINUE };
        let func = v8::Local::new(tc, &func);
        let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
        let Some(ret) = func.call(tc, recv, &argv) else {
            // Only the multiplexer itself can land here (handlers are caught in JS).
            let msg = tc.exception().map(|e| e.to_rust_string_lossy(&*tc)).unwrap_or_default();
            log_warn(&format!("WARN: {}: multiplexer: {}", self.kind.label(self.name), msg));
            return CONTINUE;
        };
        // A number, or (frame channels, when a handler threw) `[result, ...erroredIds]`.
        if let Ok(arr) = v8::Local::<v8::Array>::try_from(ret) {
            let n = arr.length();
            let result = arr.get_index(tc, 0).and_then(|v| v.int32_value(tc)).unwrap_or(0);
            let errored = (1..n)
                .filter_map(|i| arr.get_index(tc, i).and_then(|v| v.number_value(tc)))
                .map(|id| id as u64)
                .collect();
            return Outcome { result: hook_result(result), errored };
        }
        Outcome { result: hook_result(ret.int32_value(tc).unwrap_or(0)), errored: Vec::new() }
    }

    /// Push this channel's subscriber array to JS if it changed since it was last pushed, and drop
    /// the arrays of every channel that changed, so an unloaded plugin's handlers (and with them
    /// its context) are not kept alive by a channel that rarely fires.
    fn sync(&self, cs: &mut v8::PinScope, name: v8::Local<v8::String>) {
        let k = self.kind as usize;
        // An incomplete array is re-pushed every dispatch until the payload classes appear.
        let current = PUSHED.with(|p| p.borrow()[k].get(self.name) == Some(&(self.version, true)));
        if !current {
            let rows = self.source.rows(self.name);
            let payload = self.kind.payload();
            let mut subs = Vec::with_capacity(rows.len());
            let mut complete = true;
            for row in &rows {
                let make = match payload {
                    Payload::GameEvent => game_event_ctor(cs, &row.owner),
                    Payload::FrameCtx => frame_ctx_factory(cs, &row.owner),
                    Payload::Args => None,
                };
                if payload != Payload::Args && make.is_none() && owner_is_live(&row.owner, row.generation) {
                    // The prelude has not published the payload class yet: deliver `undefined`
                    // as the Rust fan-out did, and look again next dispatch.
                    complete = false;
                }
                let handler = v8::Local::new(cs, &row.handler);
                let line = if self.kind == Kind::Frame {
                    handler.get_script_line_number().map_or(0, |l| l + 1)
                } else {
                    0
                };
                let fields: [v8::Local<v8::Value>; 7] = [
                    v8::Number::new(cs, owner_slot(&row.owner).map_or(-1.0, f64::from)).into(),
                    v8::Number::new(cs, row.generation as f64).into(),
                    handler.into(),
                    make.unwrap_or_else(|| v8::null(cs).into()),
                    v8::Number::new(cs, row.id as f64).into(),
                    v8::Boolean::new(cs, row.monitor).into(),
                    v8::Integer::new_from_unsigned(cs, line as u32).into(),
                ];
                subs.push(v8::Array::new_with_elements(cs, &fields).into());
            }
            let set = STATE.with(|s| s.borrow().as_ref().map(|st| st.set.clone()));
            if let Some(set) = set {
                let set = v8::Local::new(cs, &set);
                let args = [
                    v8::Integer::new(cs, k as i32).into(),
                    name.into(),
                    v8::Array::new_with_elements(cs, &subs).into(),
                ];
                let recv = v8::undefined(cs).into();
                if set.call(cs, recv, &args).is_some() {
                    // Recorded even when incomplete, so the prune below keeps the array just set.
                    PUSHED.with(|p| p.borrow_mut()[k].insert(self.name.to_string(), (self.version, complete)));
                }
            }
        }
        self.prune(cs);
    }

    /// Is there a JS array that may be stale and has not been checked at this version?
    fn prune_due(&self) -> bool {
        let k = self.kind as usize;
        PRUNED.with(|p| p.borrow()[k]) != self.version && PUSHED.with(|p| !p.borrow()[k].is_empty())
    }

    /// Drop the JS arrays of every channel that changed since it was pushed.
    fn prune(&self, cs: &mut v8::PinScope) {
        let k = self.kind as usize;

        if PRUNED.with(|p| p.borrow()[k]) == self.version {
            return;
        }
        let stale: Vec<String> = PUSHED.with(|p| {
            let mut p = p.borrow_mut();
            let stale = p[k].iter().filter(|(_, v)| v.0 != self.version).map(|(n, _)| n.clone()).collect::<Vec<_>>();
            for n in &stale {
                p[k].remove(n);
            }
            stale
        });
        PRUNED.with(|p| p.borrow_mut()[k] = self.version);
        if stale.is_empty() {
            return;
        }
        let drop = STATE.with(|s| s.borrow().as_ref().map(|st| st.drop.clone()));
        let Some(drop) = drop else { return };
        let names: Vec<v8::Local<v8::Value>> =
            stale.iter().filter_map(|n| v8::String::new(cs, n)).map(Into::into).collect();
        let drop = v8::Local::new(cs, &drop);
        let args = [v8::Integer::new(cs, k as i32).into(), v8::Array::new_with_elements(cs, &names).into()];
        let recv = v8::undefined(cs).into();
        let _ = drop.call(cs, recv, &args);
    }
}

/// The owner's `__s2pkg_events.GameEvent`, read in its own context — the class the Rust fan-out
/// constructed per subscriber.
fn game_event_ctor<'s>(cs: &mut v8::PinScope<'s, '_>, owner: &str) -> Option<v8::Local<'s, v8::Value>> {
    #[cfg(test)]
    if HIDDEN_CTOR.with(|h| h.borrow().as_deref() == Some(owner)) {
        return None;
    }
    let g_ctx = clone_plugin_context(owner)?;
    let ctx = v8::Local::new(cs, &g_ctx);
    let scope = &mut v8::ContextScope::new(cs, ctx);
    let global = ctx.global(scope);
    let pkg_key = v8::String::new(scope, "__s2pkg_events")?;
    let pkg = v8::Local::<v8::Object>::try_from(global.get(scope, pkg_key.into())?).ok()?;
    let ctor_key = v8::String::new(scope, "GameEvent")?;
    let ctor = v8::Local::<v8::Function>::try_from(pkg.get(scope, ctor_key.into())?).ok()?;
    Some(ctor.into())
}

/// A function compiled in the owner's context that builds the per-frame `ctx` object there, with
/// the same keys, in the same order, that `dispatch_onframe` set them.
fn frame_ctx_factory<'s>(cs: &mut v8::PinScope<'s, '_>, owner: &str) -> Option<v8::Local<'s, v8::Value>> {
    let g_ctx = clone_plugin_context(owner)?;
    let ctx = v8::Local::new(cs, &g_ctx);
    let scope = &mut v8::ContextScope::new(cs, ctx);
    let src = v8::String::new(
        scope,
        "(function (s, f, l, p) { return { simulating: s, firstTick: f, lastTick: l, phase: p }; })",
    )?;
    let f = v8::Script::compile(scope, src, None)?.run(scope)?;
    v8::Local::<v8::Function>::try_from(f).ok().map(Into::into)
}

const SRC: &str = include_str!("../js/mux.js");

/// Build the multiplexer context on first use; returns it.
fn ensure_state<'s>(hs: &mut v8::PinScope<'s, '_, ()>) -> Option<v8::Local<'s, v8::Context>> {
    if let Some(ctx) = STATE.with(|s| s.borrow().as_ref().map(|st| st.context.clone())) {
        return Some(v8::Local::new(hs, &ctx));
    }
    let context = v8::Context::new(hs, Default::default());
    let cs = &mut v8::ContextScope::new(hs, context);
    let enter = v8::FunctionTemplate::builder(slow_enter)
        .build_fast(cs, std::slice::from_ref(&FAST_ENTER))
        .get_function(cs)?;
    let thrown = v8::Function::new(cs, slow_thrown)?;
    let range = v8::Function::new(cs, slow_range)?;
    let src = v8::String::new(cs, SRC)?;
    let factory = v8::Script::compile(cs, src, None)?.run(cs)?;
    let factory = v8::Local::<v8::Function>::try_from(factory).ok()?;
    let recv = v8::undefined(cs).into();
    let api = factory.call(cs, recv, &[enter.into(), thrown.into(), range.into()])?;
    let api = v8::Local::<v8::Object>::try_from(api).ok()?;
    let get = |key: &str| -> Option<v8::Global<v8::Function>> {
        let k = v8::String::new(cs, key)?;
        let f = v8::Local::<v8::Function>::try_from(api.get(cs, k.into())?).ok()?;
        Some(v8::Global::new(cs, f))
    };
    let (set, drop, dispatch, frame) = (get("set")?, get("drop")?, get("dispatch")?, get("frame")?);
    let state = State { context: v8::Global::new(cs, context), set, drop, dispatch, frame };
    STATE.with(|s| *s.borrow_mut() = Some(state));
    Some(context)
}

fn arg_slot(n: f64) -> Option<u32> {
    (n.is_finite() && n >= 0.0 && n < u32::MAX as f64 && n.fract() == 0.0).then_some(n as u32)
}

fn arg_generation(n: f64) -> u64 {
    if !n.is_finite() || n < 1.0 || n > 9_007_199_254_740_992.0 || n.fract() != 0.0 { return 0; }
    n as u64
}

/// `enter(slot, generation, line)`: 1 iff the subscriber's plugin is loaded at that generation,
/// after naming it in the breadcrumb (and, when `line` > 0, its handler's source line) and as the
/// dispatch owner. A V8 fast call: no heap, no V8, no panic.
fn fast_enter(_recv: v8::Local<v8::Object>, slot: f64, generation: f64, line: f64) -> i32 {
    let (Some(slot), generation) = (arg_slot(slot), arg_generation(generation)) else { return 0 };
    let Ok(Some(breadcrumb)) =
        FRAMES.with(|f| f.try_borrow().map(|f| f.last().map(|fr| fr.breadcrumb)))
    else {
        return 0;
    };
    let live = with_live_slot(slot, generation, |id| {
        if breadcrumb {
            crate::crash::breadcrumb::stamp_plugin(id);
        }
        if line >= 1.0 && line < u32::MAX as f64 {
            crate::crash::breadcrumb::note_js_location(id, line as u32);
        }
    });
    if live.is_none() {
        return 0;
    }
    FRAMES.with(|f| {
        if let Ok(mut f) = f.try_borrow_mut() {
            if let Some(top) = f.last_mut() {
                top.owner = Some((slot, generation));
            }
        }
    });
    1
}

fn slow_enter(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    rv.set_int32(0);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let num = |i: i32, scope: &mut v8::PinScope| args.get(i).number_value(scope).unwrap_or(f64::NAN);
        let (slot, generation, line) = (num(0, scope), num(1, scope), num(2, scope));
        let recv = v8::Object::new(scope);
        rv.set_int32(fast_enter(recv, slot, generation, line));
    }));
}

/// `thrown(slot, message, stack)`: what the Rust fan-out does for a throwing handler — WARN (not
/// for frames) and, where the channel reports, a crash report.
fn slow_thrown(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let slot = arg_slot(args.get(0).number_value(scope).unwrap_or(f64::NAN));
        let msg = args.get(1).to_rust_string_lossy(scope);
        let stack = args.get(2).to_rust_string_lossy(scope);
        let owner = slot.and_then(slot_owner).unwrap_or_else(|| "?".to_string());
        let Some((kind, name)) = FRAMES.with(|f| f.borrow().last().map(|fr| (fr.kind, fr.name.clone()))) else {
            return;
        };
        if kind != Kind::Frame {
            log_warn(&format!("WARN: {}: handler '{}': {}", kind.label(&name), owner, msg));
        }
        if let Some(tag) = kind.tag(&name) {
            crate::crash::report_js_error(&owner, &tag, &msg, &stack);
        }
    }));
}

/// `range(n)`: an `OnGameFrame` handler returned a number outside 0..=3.
fn slow_range(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = args.get(0).uint32_value(scope).unwrap_or(0);
        log_warn(&format!("WARN: onGameFrame handler returned out-of-range HookResult {n}; treating as Continue"));
    }));
}

use v8::fast_api::{CFunction, CFunctionInfo, Int64Representation, Type as FastType};

const FAST_ENTER: CFunction = CFunction::new(
    fast_enter as *const c_void,
    &CFunctionInfo::new(
        FastType::Int32.as_info(),
        &[FastType::V8Value.as_info(), FastType::Float64.as_info(), FastType::Float64.as_info(),
          FastType::Float64.as_info()],
        Int64Representation::Number,
    ),
);
