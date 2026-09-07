//! Keyed multi-channel subscription store — the storage half of the generic dispatch path.
//!
//! Replaces `event_mux.rs`, whose charter ("events don't collapse") was falsified by its own users:
//! five of its instances DO collapse handler return values into a `HookResult`. Rather than grow a
//! second collapse/priority implementation alongside `multiplexer::Descriptor`, `Channels` is a thin
//! keyed facade OVER `Descriptor`, so priority, `enabled`, `error_count` and `apply_errors` come from
//! the one implementation that already has them.
//!
//! Two impedance mismatches with `Descriptor`, both handled here so `multiplexer.rs` stays untouched
//! (it is on the `OnGameFrame` path, and that path is the reference implementation):
//!
//! 1. **Generation.** Every subscriber carries the plugin-reload liveness token that dispatch checks
//!    via `REGISTRY.is_live(owner, generation)`. `Descriptor::Subscription` has no such field, so it
//!    is folded into the handler type: `Descriptor<(u64, H)>`. Nothing in `multiplexer.rs` needs to
//!    know, and the safety-critical liveness path keeps the exact shape dispatch already expects.
//! 2. **Caller-allocated ids.** Callers pass a subscription id from `v8host::next_sub_id()` and later
//!    dispose by that id (the `Scope` surface); `Descriptor` mints its own `SubId`. `ids` maps the
//!    caller's id to `(channel, SubId)`.
//!
//! The public API is deliberately signature-compatible with the `EventMux` it replaces, so swapping
//! the statics over is a type change rather than a rewrite of ~100 call sites.
//!
//! Priority and auto-disable are AVAILABLE through this type but not yet switched on: every
//! subscription lands at `Priority::Normal` and nothing calls `apply_errors`, which keeps this
//! migration behaviour-identical. Turning either on is a deliberate follow-up, per capability.
use crate::multiplexer::{Descriptor, Phase, Priority, SubId};
use std::collections::HashMap;

pub struct Channels<H: Clone> {
    by_name: HashMap<String, Descriptor<(u64, H)>>,
    /// Caller-allocated subscription id → the channel it lives on, its `Descriptor` id, and owner.
    ///
    /// The owner is repeated here so owner teardown can prune only the reverse mappings for the
    /// subscriptions it removed. Channel liveness alone is insufficient when another owner remains.
    ids: HashMap<u64, (String, SubId, String)>,
}

impl<H: Clone> Channels<H> {
    pub fn new() -> Self {
        Channels { by_name: HashMap::new(), ids: HashMap::new() }
    }

    /// Returns true iff this is the FIRST subscriber for `name` — the caller then performs its
    /// engine-op follow-up (`event_subscribe`, installing a detour, …). Same contract as
    /// `EventMux::subscribe`.
    pub fn subscribe(&mut self, name: &str, id: u64, owner: String, generation: u64, handler: H) -> bool {
        let indexed_owner = owner.clone();
        let desc = self
            .by_name
            .entry(name.to_string())
            .or_insert_with(|| Descriptor::new(name));
        let first = desc.enabled_count() == 0;
        let (sub_id, _) = desc.subscribe(Priority::Normal, Phase::Pre, owner, (generation, handler));
        self.ids.insert(id, (name.to_string(), sub_id, indexed_owner));
        first
    }

    /// The handlers for `name`, in dispatch order (priority, then registration order — `Descriptor`
    /// keeps `subs` sorted). Shape matches `EventMux::snapshot` so dispatch is unchanged.
    pub fn snapshot(&self, name: &str) -> Vec<(String, u64, H)> {
        match self.by_name.get(name) {
            None => Vec::new(),
            Some(d) => d
                .snapshot(Phase::Pre)
                .into_iter()
                .map(|(_id, _prio, owner, (generation, h))| (owner, generation, h))
                .collect(),
        }
    }

    /// Drop every subscription owned by `owner`; returns the channel names that became empty, which
    /// the caller uses to issue its engine-op unsubscribe.
    pub fn remove_by_owner(&mut self, owner: &str) -> Vec<String> {
        let mut emptied = Vec::new();
        for (name, desc) in self.by_name.iter_mut() {
            let before = desc.enabled_count();
            desc.remove_by_owner(owner);
            if before > 0 && desc.enabled_count() == 0 {
                emptied.push(name.clone());
            }
        }
        self.ids.retain(|_, (_, _, indexed_owner)| indexed_owner != owner);
        for name in &emptied {
            self.by_name.remove(name);
        }
        emptied
    }

    /// Drop the subscriptions with these caller-allocated ids (the `Scope` disposal path); returns
    /// the channel names that became empty.
    pub fn remove_by_ids(&mut self, ids: &[u64]) -> Vec<String> {
        let mut emptied = Vec::new();
        for id in ids {
            let Some((name, sub_id, _owner)) = self.ids.remove(id) else { continue };
            let became_empty = self.by_name.get_mut(&name).is_some_and(|desc| {
                let before = desc.enabled_count();
                desc.unsubscribe(sub_id);
                before > 0 && desc.enabled_count() == 0
            });
            if became_empty {
                emptied.push(name.clone());
                self.by_name.remove(&name);
            }
        }
        emptied
    }

    /// Drop `owner`'s subscriptions on ONE channel; true iff that channel became empty.
    pub fn remove_by_owner_on(&mut self, name: &str, owner: &str) -> bool {
        let Some(desc) = self.by_name.get_mut(name) else { return false };
        let before = desc.enabled_count();
        desc.remove_by_owner(owner);
        let now_empty = before > 0 && desc.enabled_count() == 0;
        self.ids.retain(|_, (n, _, indexed_owner)| n != name || indexed_owner != owner);
        if now_empty {
            self.by_name.remove(name);
        }
        now_empty
    }

    /// Drop a whole channel.
    pub fn remove_by_name(&mut self, name: &str) {
        self.by_name.remove(name);
        self.ids.retain(|_, (n, _, _)| n != name);
    }

    /// True iff no channel has any subscriber — used by the "is the last hook gone?" detour
    /// reconciliation.
    pub fn is_empty(&self) -> bool {
        self.by_name.values().all(|d| d.enabled_count() == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch() -> Channels<&'static str> {
        Channels::new()
    }

    #[test]
    fn subscribe_reports_only_the_first_per_channel() {
        let mut c = ch();
        assert!(c.subscribe("a", 1, "p1".into(), 1, "h1"), "first on 'a'");
        assert!(!c.subscribe("a", 2, "p2".into(), 1, "h2"), "second on 'a' is not first");
        assert!(c.subscribe("b", 3, "p1".into(), 1, "h3"), "first on 'b'");
    }

    #[test]
    fn snapshot_returns_owner_generation_handler_in_registration_order() {
        let mut c = ch();
        c.subscribe("a", 1, "p1".into(), 7, "h1");
        c.subscribe("a", 2, "p2".into(), 9, "h2");
        assert_eq!(
            c.snapshot("a"),
            vec![("p1".to_string(), 7, "h1"), ("p2".to_string(), 9, "h2")]
        );
        assert!(c.snapshot("nope").is_empty());
    }

    /// The generation is the plugin-reload liveness token dispatch checks before entering a
    /// context. It is folded into the handler tuple inside this type, so a regression that dropped
    /// or transposed it would silently make dispatch check the WRONG generation — which fails open,
    /// calling into a reloaded plugin's stale context. Pinned explicitly.
    #[test]
    fn generation_survives_the_descriptor_round_trip_per_subscriber() {
        let mut c = ch();
        c.subscribe("a", 1, "p1".into(), 42, "h1");
        c.subscribe("a", 2, "p1".into(), 43, "h2");
        let gens: Vec<u64> = c.snapshot("a").into_iter().map(|(_, g, _)| g).collect();
        assert_eq!(gens, vec![42, 43], "each subscriber keeps its OWN generation");
    }

    #[test]
    fn remove_by_owner_reports_emptied_channels_only() {
        let mut c = ch();
        c.subscribe("a", 1, "p1".into(), 1, "h1");
        c.subscribe("a", 2, "p2".into(), 1, "h2");
        c.subscribe("b", 3, "p1".into(), 1, "h3");
        let emptied = c.remove_by_owner("p1");
        assert_eq!(emptied, vec!["b".to_string()], "'a' still has p2, only 'b' emptied");
        assert_eq!(c.snapshot("a").len(), 1);
        assert!(c.snapshot("b").is_empty());
    }

    #[test]
    fn remove_by_ids_drops_exactly_those_subscriptions() {
        let mut c = ch();
        c.subscribe("a", 10, "p1".into(), 1, "h1");
        c.subscribe("a", 11, "p1".into(), 1, "h2");
        let emptied = c.remove_by_ids(&[10]);
        assert!(emptied.is_empty(), "'a' still has one subscriber");
        assert_eq!(c.snapshot("a"), vec![("p1".to_string(), 1, "h2")]);
        assert_eq!(c.remove_by_ids(&[11]), vec!["a".to_string()], "now emptied");
    }

    #[test]
    fn remove_by_ids_ignores_unknown_ids() {
        let mut c = ch();
        c.subscribe("a", 1, "p1".into(), 1, "h1");
        assert!(c.remove_by_ids(&[999]).is_empty());
        assert_eq!(c.snapshot("a").len(), 1, "an unknown id must not drop anything");
    }

    #[test]
    fn is_empty_tracks_every_channel() {
        let mut c = ch();
        assert!(c.is_empty());
        c.subscribe("a", 1, "p1".into(), 1, "h1");
        assert!(!c.is_empty());
        c.remove_by_owner("p1");
        assert!(c.is_empty());
    }

    #[test]
    fn remove_by_owner_on_scopes_to_one_channel() {
        let mut c = ch();
        c.subscribe("a", 1, "p1".into(), 1, "h1");
        c.subscribe("b", 2, "p1".into(), 1, "h2");
        assert!(c.remove_by_owner_on("a", "p1"), "'a' emptied");
        assert!(c.snapshot("a").is_empty());
        assert_eq!(c.snapshot("b").len(), 1, "'b' untouched");
    }

    #[test]
    fn shared_channel_owner_churn_retains_only_the_live_reverse_mapping() {
        let mut c = ch();
        c.subscribe("shared", 1, "a".into(), 1, "a");

        for cycle in 0..1_000 {
            let id = cycle + 2;
            assert!(!c.subscribe("shared", id, "b".into(), 1, "b"));
            assert!(c.remove_by_owner("b").is_empty(), "owner a keeps the channel live");
        }

        assert_eq!(c.snapshot("shared"), vec![("a".to_string(), 1, "a")]);
        assert_eq!(c.ids.len(), 1, "disposed owner ids must not accumulate behind owner a");
        assert_eq!(c.by_name.len(), 1, "the live shared descriptor remains");
    }

    #[test]
    fn unique_channel_churn_releases_reverse_mappings_and_empty_descriptors() {
        let mut c = ch();

        for cycle in 0..1_000 {
            let name = format!("unique-{cycle}");
            c.subscribe(&name, cycle + 1, "b".into(), 1, "b");
            assert_eq!(c.remove_by_owner("b"), vec![name]);
        }

        assert!(c.is_empty());
        assert_eq!(c.ids.len(), 0, "disposed subscriptions leave no reverse mappings");
        assert_eq!(c.by_name.len(), 0, "empty descriptors must not accumulate across churn");
    }

    #[test]
    fn remove_by_owner_on_prunes_only_that_owners_reverse_mappings() {
        let mut c = ch();
        c.subscribe("shared", 1, "a".into(), 1, "a");
        c.subscribe("shared", 2, "b".into(), 1, "b");

        assert!(!c.remove_by_owner_on("shared", "b"), "owner a keeps the channel live");

        assert_eq!(c.snapshot("shared"), vec![("a".to_string(), 1, "a")]);
        assert_eq!(c.ids.len(), 1, "only owner a's live reverse mapping remains");
        assert!(c.ids.contains_key(&1));
    }

    #[test]
    fn duplicate_and_unknown_id_disposal_is_idempotent_and_prunes_the_descriptor() {
        let mut c = ch();
        c.subscribe("a", 1, "p1".into(), 1, "h1");

        assert!(c.remove_by_ids(&[999]).is_empty());
        assert_eq!(c.remove_by_ids(&[1, 1, 999]), vec!["a".to_string()]);
        assert!(c.remove_by_ids(&[1, 999]).is_empty());
        assert_eq!(c.ids.len(), 0);
        assert_eq!(c.by_name.len(), 0, "the emptied descriptor is removed after notification");
    }

    #[test]
    fn dispatch_snapshot_survives_subscription_changes_and_future_snapshots_reflect_them() {
        let mut c = ch();
        c.subscribe("a", 1, "p1".into(), 1, "h1");
        c.subscribe("a", 2, "p2".into(), 1, "h2");

        let dispatch_snapshot = c.snapshot("a");
        assert!(c.remove_by_ids(&[1]).is_empty());
        c.subscribe("a", 3, "p3".into(), 1, "h3");

        assert_eq!(
            dispatch_snapshot,
            vec![("p1".to_string(), 1, "h1"), ("p2".to_string(), 1, "h2")],
            "the active dispatch owns its cloned snapshot",
        );
        assert_eq!(
            c.snapshot("a"),
            vec![("p2".to_string(), 1, "h2"), ("p3".to_string(), 1, "h3")],
            "the next dispatch sees the mutations",
        );
        assert_eq!(c.remove_by_ids(&[2, 3]), vec!["a".to_string()]);
        assert_eq!(c.by_name.len(), 0);
    }
}
