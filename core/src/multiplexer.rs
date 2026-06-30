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

pub const MAX_HANDLER_ERRORS: u32 = 10;

#[derive(Clone, Copy, Debug)]
pub struct DispatchOutcome { pub result: HookResult, pub detour: DetourChange }

struct Subscription<H> {
    id: SubId,
    priority: Priority,
    phase: Phase,
    handler: H,
    enabled: bool,
    error_count: u32,
}

pub struct Descriptor<H: Clone> {
    #[allow(dead_code)]
    name: String,
    subs: Vec<Subscription<H>>,
    next_id: SubId,
    enabled_count: usize,
}

#[derive(Debug)]
pub struct ChainOutcome { pub result: HookResult, pub errored: Vec<SubId> }

/// Phase 2: run the snapshot with collapse rules. FREE fn — holds NO Descriptor borrow,
/// so the `invoke` closure (JS in the V8 path) may safely re-subscribe/unsubscribe.
pub fn run_chain<H>(
    snapshot: &[(SubId, Priority, H)],
    mut invoke: impl FnMut(&H) -> Result<HookResult, ()>,
) -> ChainOutcome {
    let mut result = HookResult::Continue;
    let mut stopped = false;
    let mut errored = Vec::new();
    for (id, prio, h) in snapshot {
        let is_monitor = *prio == Priority::Monitor;
        if !is_monitor && stopped { continue; }
        match invoke(h) {
            Ok(r) if !is_monitor => {
                if r > result { result = r; }
                if r == HookResult::Stop { stopped = true; }
            }
            Ok(_) => { /* monitor: return ignored */ }
            Err(()) => errored.push(*id),
        }
    }
    ChainOutcome { result, errored }
}

impl<H: Clone> Descriptor<H> {
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

    /// Phase 3: bump error_count for each errored id; auto-disable at the threshold; if that
    /// dropped the enabled count to 0, return Remove.
    pub fn apply_errors(&mut self, errored: &[SubId]) -> DetourChange {
        let mut disabled = 0usize;
        for id in errored {
            if let Some(s) = self.subs.iter_mut().find(|s| s.id == *id) {
                if !s.enabled { continue; }
                s.error_count += 1;
                if s.error_count >= MAX_HANDLER_ERRORS { s.enabled = false; disabled += 1; }
            }
        }
        let mut detour = DetourChange::None;
        for _ in 0..disabled {
            if let DetourChange::Remove = self.dec_enabled() { detour = DetourChange::Remove; }
        }
        detour
    }

    /// Phase 1: clone the ordered, enabled handlers for `phase`. `subs` is kept priority-sorted
    /// (Monitor last), so the snapshot is already in dispatch order.
    pub fn snapshot(&self, phase: Phase) -> Vec<(SubId, Priority, H)> {
        self.subs.iter()
            .filter(|s| s.enabled && s.phase == phase)
            .map(|s| (s.id, s.priority, s.handler.clone()))
            .collect()
    }

    /// Recomposed convenience: snapshot + run_chain + apply_errors. Replaces Task 1's dispatch.
    pub fn dispatch(&mut self, phase: Phase, invoke: impl FnMut(&H) -> Result<HookResult, ()>) -> DispatchOutcome {
        let snap = self.snapshot(phase);
        let out = run_chain(&snap, invoke);
        let detour = self.apply_errors(&out.errored);
        DispatchOutcome { result: out.result, detour }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A mock handler: records that it ran (via the shared log) and returns a scripted result.
    #[derive(Clone)]
    struct Mock { tag: &'static str, ret: HookResult }

    fn run(d: &mut Descriptor<Mock>, phase: Phase, log: &std::cell::RefCell<Vec<&'static str>>) -> HookResult {
        d.dispatch(phase, |h| { log.borrow_mut().push(h.tag); Ok(h.ret) }).result
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

    #[test]
    fn snapshot_excludes_subs_added_after_it() {
        // The snapshot taken at dispatch start is the set that runs this frame; a sub added
        // afterward (the re-entrancy case) is NOT in it — it runs next frame.
        let mut d = Descriptor::new("d");
        d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "a", ret: HookResult::Continue });
        let snap = d.snapshot(Phase::Pre);
        d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "b", ret: HookResult::Continue });
        assert_eq!(snap.len(), 1);            // "b" not in the earlier snapshot
        assert_eq!(d.snapshot(Phase::Pre).len(), 2); // but a fresh snapshot includes it
    }

    #[test]
    fn unsubscribe_excludes_from_future_snapshots_and_lazy_removes() {
        let mut d: Descriptor<Mock> = Descriptor::new("d");
        let (a, _) = d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "a", ret: HookResult::Continue });
        let (b, _) = d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "b", ret: HookResult::Continue });
        assert!(matches!(d.unsubscribe(a), DetourChange::None));   // one still enabled
        assert_eq!(d.snapshot(Phase::Pre).len(), 1);              // "a" gone from snapshots
        assert!(matches!(d.unsubscribe(b), DetourChange::Remove)); // last one gone → remove detour
        assert!(matches!(d.unsubscribe(b), DetourChange::None));   // already gone, idempotent
        assert_eq!(d.snapshot(Phase::Pre).len(), 0);
    }

    #[test]
    fn handler_error_is_continue_and_counts_then_auto_disables() {
        let mut d = Descriptor::new("d");
        d.subscribe(Priority::Normal, Phase::Pre, Mock { tag: "bad", ret: HookResult::Stop });
        // invoke always errors; error must be treated as Continue (NOT the handler's scripted Stop),
        // and after MAX_HANDLER_ERRORS the sub auto-disables (being last → requests Remove).
        let mut last = DispatchOutcome { result: HookResult::Continue, detour: DetourChange::None };
        for _ in 0..MAX_HANDLER_ERRORS {
            last = d.dispatch(Phase::Pre, |_h| Err(()));
            assert_eq!(last.result, HookResult::Continue);
        }
        assert!(matches!(last.detour, DetourChange::Remove));
        assert_eq!(d.snapshot(Phase::Pre).len(), 0); // auto-disabled → excluded from snapshots
    }
}
