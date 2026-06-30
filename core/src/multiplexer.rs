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

    /// Phase 1: clone the ordered, enabled handlers for `phase`. `subs` is kept priority-sorted
    /// (Monitor last), so the snapshot is already in dispatch order.
    pub fn snapshot(&self, phase: Phase) -> Vec<(SubId, Priority, H)> {
        self.subs.iter()
            .filter(|s| s.enabled && s.phase == phase)
            .map(|s| (s.id, s.priority, s.handler.clone()))
            .collect()
    }

    /// Convenience composing snapshot + run_chain (Task 2 adds apply_errors). Used by unit tests
    /// for NON-re-entrant invokers; the V8 glue (Task 3) calls the three phases separately so it
    /// does not hold a borrow across invocation.
    pub fn dispatch(&mut self, phase: Phase, invoke: impl FnMut(&H) -> Result<HookResult, ()>) -> HookResult {
        let snap = self.snapshot(phase);
        run_chain(&snap, invoke).result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A mock handler: records that it ran (via the shared log) and returns a scripted result.
    #[derive(Clone)]
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
