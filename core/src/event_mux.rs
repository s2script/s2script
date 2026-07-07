//! Notify-only game-event multiplexer: name → subscribers. Re-entrancy-safe (snapshot before invoke),
//! liveness-checked, and remove_by_owner for ledgered teardown. Mirrors multiplexer.rs's discipline
//! without the priority/HookResult machinery (events don't collapse).
use std::collections::HashMap;

pub struct EventSub<H> { pub owner: String, pub generation: u64, pub handler: H }

#[derive(Default)]
pub struct EventMux<H> { by_name: HashMap<String, Vec<EventSub<H>>> }

impl<H: Clone> EventMux<H> {
    pub fn new() -> Self { Self { by_name: HashMap::new() } }
    /// Returns true iff this is the FIRST subscriber for `name` (caller then calls the engine-op event_subscribe).
    pub fn subscribe(&mut self, name: &str, owner: String, generation: u64, handler: H) -> bool {
        let list = self.by_name.entry(name.to_string()).or_default();
        let first = list.is_empty();
        list.push(EventSub { owner, generation, handler });
        first
    }
    /// A snapshot of the handlers for `name` (empty if none) — the set that runs for this fire.
    pub fn snapshot(&self, name: &str) -> Vec<(String, u64, H)> {
        self.by_name.get(name).map(|v| v.iter().map(|s| (s.owner.clone(), s.generation, s.handler.clone())).collect()).unwrap_or_default()
    }
    /// Remove all of an owner's subscriptions (teardown). Returns the names that became empty
    /// (caller then calls the engine-op event_unsubscribe for each).
    pub fn remove_by_owner(&mut self, owner: &str) -> Vec<String> {
        let mut emptied = Vec::new();
        for (name, list) in self.by_name.iter_mut() {
            let before = list.len();
            list.retain(|s| s.owner != owner);
            if before > 0 && list.is_empty() { emptied.push(name.clone()); }
        }
        emptied
    }
    /// True iff no name has any subscriber (the trigger to install/remove a GLOBAL hook,
    /// as opposed to the per-name `subscribe` "first for this name" signal).
    pub fn is_empty(&self) -> bool {
        self.by_name.values().all(|v| v.is_empty())
    }

    /// Remove a specific handler (by identity: owner + handler clone comparison) from a name.
    /// Returns true if the name is now empty (caller calls engine-op event_unsubscribe).
    /// Since V8 Globals can't be compared by identity, this removes ALL of owner's subs for `name`
    /// (mirrors the iface_off "best-effort" approach — consumers rarely double-subscribe).
    pub fn remove_by_owner_on(&mut self, name: &str, owner: &str) -> bool {
        if let Some(list) = self.by_name.get_mut(name) {
            list.retain(|s| s.owner != owner);
            list.is_empty()
        } else {
            false
        }
    }

    /// Drop an entire name key and all its subscribers (releasing any retained handler clones).
    /// Unlike `remove_by_owner*`, this is keyed only by `name` — used to prune a terminated
    /// resource's keys (e.g. a closed WebSocket conn id's `<id>:message`/`close`/`error`) whose
    /// name is never re-subscribed, so the entry would otherwise accumulate for the plugin's uptime.
    pub fn remove_by_name(&mut self, name: &str) {
        self.by_name.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscribe_first_then_snapshot_then_remove_by_owner() {
        let mut m: EventMux<&'static str> = EventMux::new();
        assert!(m.subscribe("player_death", "p".into(), 1, "h1"));   // first for the name
        assert!(!m.subscribe("player_death", "q".into(), 1, "h2"));  // not first
        assert_eq!(m.snapshot("player_death").len(), 2);
        assert_eq!(m.snapshot("round_start").len(), 0);
        let emptied = m.remove_by_owner("p");
        assert!(emptied.is_empty(), "still q for player_death");
        assert_eq!(m.snapshot("player_death").len(), 1);
        let emptied = m.remove_by_owner("q");
        assert_eq!(emptied, vec!["player_death".to_string()]);       // now empty → event_unsubscribe
    }

    #[test]
    fn is_empty_tracks_any_subscriber() {
        let mut m: EventMux<&str> = EventMux::new();
        assert!(m.is_empty());
        m.subscribe("player_hurt", "p".into(), 1, "h");
        assert!(!m.is_empty());
        m.remove_by_owner("p");
        assert!(m.is_empty());
    }

    /// Slice 5D.1: `remove_by_owner_on` returns false while another owner remains,
    /// and true only when the last subscriber for that name is removed.
    /// This guards the caller's "fire engine-op event_unsubscribe" condition: a premature
    /// true would cause a spurious engine-level deregister while entries still exist.
    #[test]
    fn remove_by_owner_on_partial_then_empty() {
        let mut m: EventMux<&'static str> = EventMux::new();
        m.subscribe("test_event", "owner_a".into(), 1, "h_a");
        m.subscribe("test_event", "owner_b".into(), 1, "h_b");

        // Remove owner_a: owner_b still present → name is NOT empty → must return false.
        let became_empty = m.remove_by_owner_on("test_event", "owner_a");
        assert!(!became_empty, "name must not be reported empty while owner_b remains");
        assert_eq!(m.snapshot("test_event").len(), 1, "exactly one subscriber remains");
        assert_eq!(m.snapshot("test_event")[0].0, "owner_b",
            "the surviving subscriber must be owner_b");

        // Remove the last (owner_b): name becomes empty → must return true.
        let became_empty = m.remove_by_owner_on("test_event", "owner_b");
        assert!(became_empty, "name must become empty after removing the last subscriber");
        assert_eq!(m.snapshot("test_event").len(), 0, "no subscribers must remain");
    }

    /// Slice 5D.1: `remove_by_owner_on` on a name that was never subscribed returns false,
    /// not true — removing a non-existent entry must not signal "became empty" (which would
    /// cause a spurious engine-op `event_unsubscribe` call in the caller).
    #[test]
    fn remove_by_owner_on_absent_name_returns_false() {
        let mut m: EventMux<&'static str> = EventMux::new();
        let became_empty = m.remove_by_owner_on("test_event", "owner_a");
        assert!(!became_empty,
            "absent name must return false, not 'became empty' — prevents a spurious event_unsubscribe call");
    }

    /// WebSocket terminal-close prune: `remove_by_name` drops the whole key regardless of owner,
    /// and dropping an absent key is a harmless no-op. This is what keeps a reconnect-on-close
    /// loop (fresh monotonic conn ids) from accumulating dead per-conn subscriber entries.
    #[test]
    fn remove_by_name_drops_whole_key_and_absent_is_noop() {
        let mut m: EventMux<&'static str> = EventMux::new();
        m.subscribe("7:message", "owner_a".into(), 1, "h_a");
        m.subscribe("7:message", "owner_b".into(), 1, "h_b");
        m.subscribe("7:close", "owner_a".into(), 1, "h_c");
        assert_eq!(m.snapshot("7:message").len(), 2);

        m.remove_by_name("7:message");
        assert_eq!(m.snapshot("7:message").len(), 0, "the whole key is gone, both owners");
        assert_eq!(m.snapshot("7:close").len(), 1, "an unrelated key is untouched");

        m.remove_by_name("7:close");
        assert!(m.is_empty());
        m.remove_by_name("nonexistent");  // no panic
    }
}
