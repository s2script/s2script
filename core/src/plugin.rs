//! Plugin registry, per-plugin ledger, generation counter, and reverse-teardown order.
//! Pure logic — no V8, no CS2 identifiers. The V8 context lives in v8host, keyed by the
//! same plugin id string. This module is the teardown authority and async-liveness guard.

// ---------------------------------------------------------------------------
// Phase
// ---------------------------------------------------------------------------

/// The plugin lifecycle phase (design spec §5). Stored on the v8host `PluginInstance`;
/// pure data here (no V8). A plugin starts `Loading` at context creation, reaches `Active`
/// once its awaited factory has settled and its buffered registrations have been armed, moves
/// to `Unloading` during teardown, and a never-Active load that fails ends as `Failed` (its
/// context is disposed and the reason retained for `sm plugins list`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Active,
    Unloading,
    Failed,
}

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

/// A ledgered resource that must be torn down when a plugin unloads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Resource {
    Hook(u64),
    Timer(u64),
    Job(u64),
    /// A published interface name (producer-owned). Teardown removes the registry entry +
    /// method Globals + subscriber list.
    Interface(String),
    /// A consumer's event-subscription id. Teardown removes it from the producer's subscriber
    /// list + drops the handler Global.
    EventSub(u64),
    /// A consumer→producer import edge (interface name). Teardown drops the edge (no Global).
    Import(String),
    /// An open DB connection handle (opaque, from `db::open`). Teardown closes it even if the
    /// plugin never calls `close()` itself.
    DbConn(u64),
    /// An open WebSocket connection id (opaque, from `ws::connect`). Teardown closes it (regardless
    /// of owner — the ledger owns the id) even if the plugin never calls `close()` itself.
    WsConn(u64),
    /// An open raw-socket (TCP/UDP) connection id (opaque, from `net::connect_tcp`/`net::bind_udp`).
    /// Teardown drops it (regardless of owner — the ledger owns the id) even if the plugin never
    /// calls `close()` itself.
    NetConn(u64),
    /// An open remote-SQL (MySQL/Postgres) pool handle (opaque, from `sqldb::connect`). Teardown
    /// drops the pool even if the plugin never calls `close()` itself.
    RemoteDbConn(u64),
}

// ---------------------------------------------------------------------------
// PluginLedger
// ---------------------------------------------------------------------------

/// Records the resources a plugin currently owns, in acquisition order.
///
/// `active` is the teardown authority. `by_resource` is a removal index whose vectors preserve
/// acquisition multiplicity: releasing an indistinguishable duplicate removes its most recent
/// acquisition. Both structures contain active entries only; completion leaves no logical
/// tombstone, though their backing allocations may retain peak concurrent capacity.
pub struct PluginLedger {
    next_sequence: u64,
    active: std::collections::BTreeMap<u64, Resource>,
    by_resource: std::collections::HashMap<Resource, Vec<u64>>,
}

impl PluginLedger {
    pub fn new() -> Self {
        Self {
            next_sequence: 0,
            active: std::collections::BTreeMap::new(),
            by_resource: std::collections::HashMap::new(),
        }
    }

    pub fn record(&mut self, resource: Resource) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.active.insert(sequence, resource.clone());
        self.by_resource.entry(resource).or_default().push(sequence);
    }

    /// Release one acquisition of `resource`. Duplicate acquisitions are intentionally distinct.
    pub fn release(&mut self, resource: &Resource) -> bool {
        let (sequence, empty) = match self.by_resource.get_mut(resource) {
            Some(sequences) => {
                let Some(sequence) = sequences.pop() else { return false };
                (sequence, sequences.is_empty())
            }
            None => return false,
        };
        if empty { self.by_resource.remove(resource); }
        self.active.remove(&sequence).is_some()
    }

    pub fn len(&self) -> usize { self.active.len() }

    pub fn record_hook(&mut self, id: u64) {
        self.record(Resource::Hook(id));
    }

    pub fn record_timer(&mut self, id: u64) {
        self.record(Resource::Timer(id));
    }

    pub fn record_job(&mut self, id: u64) {
        self.record(Resource::Job(id));
    }

    pub fn record_interface(&mut self, name: String) {
        self.record(Resource::Interface(name));
    }

    pub fn record_event_sub(&mut self, id: u64) {
        self.record(Resource::EventSub(id));
    }

    pub fn record_import(&mut self, name: String) {
        self.record(Resource::Import(name));
    }

    /// Record an open DB connection handle against this plugin (teardown authority for Task 3).
    pub fn record_db_conn(&mut self, handle: u64) {
        self.record(Resource::DbConn(handle));
    }

    /// Record an open WebSocket connection id against this plugin (teardown authority, ws Task 2).
    pub fn record_ws_conn(&mut self, id: u64) {
        self.record(Resource::WsConn(id));
    }

    /// Record an open raw-socket (TCP/UDP) connection id against this plugin (teardown authority,
    /// net Task 2).
    pub fn record_net_conn(&mut self, id: u64) {
        self.record(Resource::NetConn(id));
    }

    /// Record an open remote-SQL pool handle against this plugin (teardown authority, remote-db Task 2).
    pub fn record_remote_db_conn(&mut self, handle: u64) {
        self.record(Resource::RemoteDbConn(handle));
    }

    /// Resources in REVERSE acquisition order — last-acquired torn down first.
    pub fn teardown_order(&self) -> Vec<Resource> {
        self.active.values().rev().cloned().collect()
    }
}

impl Default for PluginLedger {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// PluginEntry
// ---------------------------------------------------------------------------

/// The registry entry for a single loaded (or reloaded) plugin instance.
pub struct PluginEntry {
    /// Monotonically increasing generation counter. A reload bumps this,
    /// making the old generation stale for `is_live` checks.
    pub generation: u64,
    /// The resource ledger for this plugin instance.
    pub ledger: PluginLedger,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Maps plugin id strings to their current entry. Backed by the shared liveness
/// primitive (E1): one instance of the SAME mechanism the entity books use —
/// separate table, separate axis (a map change must never invalidate plugins).
pub struct Registry {
    table: crate::liveness::LiveTable<String, PluginLedger>,
}

impl Registry {
    pub fn new() -> Self {
        // first_id 1, same as the entity books: generation 0 is a NEVER-LIVE sentinel. Every
        // subscribe-time stamp falls back to 0 for an unregistered owner (`unwrap_or(0)`), and
        // when the registry also minted 0 the process's FIRST plugin aliased that fallback — its
        // pre-registration subscriptions accidentally fired while every other plugin's were
        // silently dropped (the P0-1 defect). With 1 as the first real generation, a 0-stamped
        // row can never match any plugin.
        Self { table: crate::liveness::LiveTable::new(1) }
    }

    /// Insert (or re-insert on reload) a plugin. Returns the assigned generation.
    /// A re-insert of an existing id mints a fresh generation — that IS reload.
    pub fn insert(&mut self, id: impl Into<String>) -> u64 {
        self.table.insert(id.into(), PluginLedger::new())
    }

    /// Remove a plugin. Returns the `PluginEntry` so the caller can walk the
    /// ledger for teardown. Returns `None` if not present.
    pub fn remove(&mut self, id: &str) -> Option<PluginEntry> {
        self.table
            .remove(&id.to_string())
            .map(|(generation, ledger)| PluginEntry { generation, ledger })
    }

    /// Returns `true` iff the plugin is present AND its generation matches.
    pub fn is_live(&self, id: &str, generation: u64) -> bool {
        self.table.is_live(&id.to_string(), generation)
    }

    /// Mutable access to a plugin's ledger (for recording resources).
    pub fn ledger_mut(&mut self, id: &str) -> Option<&mut PluginLedger> {
        self.table.get_mut(&id.to_string()).map(|(_, m)| m)
    }

    /// Record only against the exact live owner generation that acquired the resource.
    pub fn record(&mut self, id: &str, generation: u64, resource: Resource) -> bool {
        let Some((live_generation, ledger)) = self.table.get_mut(&id.to_string()) else { return false };
        if live_generation != generation { return false; }
        ledger.record(resource);
        true
    }

    /// Release one matching acquisition only when the owner generation is still current.
    pub fn release(&mut self, id: &str, generation: u64, resource: &Resource) -> bool {
        let Some((live_generation, ledger)) = self.table.get_mut(&id.to_string()) else { return false };
        live_generation == generation && ledger.release(resource)
    }

    pub fn active_resource_count(&self, id: &str, generation: u64) -> Option<usize> {
        let (live_generation, ledger) = self.table.get(&id.to_string())?;
        (live_generation == generation).then(|| ledger.len())
    }

    /// All currently registered plugin ids.
    pub fn ids(&self) -> Vec<String> {
        self.table.keys()
    }

    /// The current generation for `id`, if present.
    pub fn generation_of(&self, id: &str) -> Option<u64> {
        self.table.get(&id.to_string()).map(|(g, _)| g)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_is_copy_eq() {
        assert_eq!(Phase::Loading, Phase::Loading);
        assert_ne!(Phase::Active, Phase::Failed);
        // Copy: assigning does not move.
        let p = Phase::Active;
        let q = p;
        assert_eq!(p, q);
    }

    #[test]
    fn teardown_is_reverse_acquisition_order() {
        let mut l = PluginLedger::new();
        l.record_hook(1); l.record_timer(2); l.record_job(3); l.record_hook(4);
        // reverse of [Hook(1),Timer(2),Job(3),Hook(4)]:
        assert_eq!(l.teardown_order(),
            vec![Resource::Hook(4), Resource::Job(3), Resource::Timer(2), Resource::Hook(1)]);
    }

    #[test]
    fn insert_assigns_and_reload_bumps_generation() {
        let mut r = Registry::new();
        let g1 = r.insert("a");
        assert!(r.is_live("a", g1));
        let g2 = r.insert("a");                 // reload
        assert_ne!(g1, g2);
        assert!(!r.is_live("a", g1), "old generation is stale after reload");
        assert!(r.is_live("a", g2));
    }

    #[test]
    fn remove_makes_it_not_live_and_returns_ledger() {
        let mut r = Registry::new();
        let g = r.insert("a");
        r.ledger_mut("a").unwrap().record_timer(7);
        let entry = r.remove("a").expect("present");
        assert_eq!(entry.ledger.teardown_order(), vec![Resource::Timer(7)]);
        assert!(!r.is_live("a", g), "removed plugin is not live");
        assert!(r.remove("a").is_none());
    }

    #[test]
    fn generations_come_from_one_shared_monotonic_counter_and_zero_is_never_live() {
        let mut r = Registry::new();
        let a = r.insert("a");
        let b = r.insert("b");
        let a2 = r.insert("a");                  // reload of a
        assert_eq!(a, 1, "first generation is 1 — 0 is reserved as the never-live sentinel");
        assert!(b > a && a2 > b, "one shared counter across ids: {a} {b} {a2}");
        assert_eq!(r.generation_of("b"), Some(b));
        assert_eq!(r.ids().len(), 2);
        // The P0-1 invariant: 0 is exactly what every subscribe-time stamp falls back to for an
        // unregistered owner (`unwrap_or(0)`), so no plugin — not even the process's first — may
        // ever be live for it. Otherwise a subscription made before registration accidentally
        // fires for whichever plugin holds generation 0.
        assert!(!r.is_live("a", 0), "generation 0 must never match a real plugin");
        assert!(!r.is_live("b", 0), "generation 0 must never match a real plugin");
    }

    #[test]
    fn teardown_includes_iface_resources_in_reverse_order() {
        let mut l = PluginLedger::new();
        l.record_interface("@x/if".into());
        l.record_import("@y/dep".into());
        l.record_event_sub(9);
        // reverse of [Interface("@x/if"), Import("@y/dep"), EventSub(9)]:
        assert_eq!(
            l.teardown_order(),
            vec![Resource::EventSub(9), Resource::Import("@y/dep".into()), Resource::Interface("@x/if".into())]
        );
    }

    #[test]
    fn record_iface_resources_preserve_acquisition_order() {
        let mut l = PluginLedger::new();
        l.record_interface("@x/if".into());
        l.record_event_sub(3);
        l.record_import("@y/dep".into());
        assert_eq!(l.teardown_order(), vec![
            Resource::Import("@y/dep".to_string()),
            Resource::EventSub(3),
            Resource::Interface("@x/if".to_string()),
        ]);
    }

    #[test]
    fn one_hundred_thousand_completed_timers_leave_no_active_resources() {
        let mut r = Registry::new();
        let generation = r.insert("timer-owner");
        for id in 1..=100_000 { assert!(r.record("timer-owner", generation, Resource::Timer(id))); }
        assert_eq!(r.active_resource_count("timer-owner", generation), Some(100_000));
        for id in 1..=100_000 { assert!(r.release("timer-owner", generation, &Resource::Timer(id))); }
        assert_eq!(r.active_resource_count("timer-owner", generation), Some(0));
        assert!(r.remove("timer-owner").unwrap().ledger.teardown_order().is_empty());
    }

    #[test]
    fn one_hundred_thousand_completed_jobs_preserve_only_active_multiplicity() {
        let mut r = Registry::new();
        let generation = r.insert("job-owner");
        for id in 1..=100_000 { assert!(r.record("job-owner", generation, Resource::Job(id))); }
        assert!(r.record("job-owner", generation, Resource::Job(100_000)));
        for id in 1..=100_000 { assert!(r.release("job-owner", generation, &Resource::Job(id))); }
        assert_eq!(r.active_resource_count("job-owner", generation), Some(1));
        assert_eq!(r.remove("job-owner").unwrap().ledger.teardown_order(), vec![Resource::Job(100_000)]);
    }

    #[test]
    fn late_release_after_owner_reload_cannot_touch_new_generation() {
        let mut r = Registry::new();
        let old_generation = r.insert("owner");
        assert!(r.record("owner", old_generation, Resource::Job(7)));
        let new_generation = r.insert("owner");
        assert!(r.record("owner", new_generation, Resource::Job(7)));
        assert!(!r.release("owner", old_generation, &Resource::Job(7)));
        assert_eq!(r.active_resource_count("owner", new_generation), Some(1));
        assert!(r.release("owner", new_generation, &Resource::Job(7)));
        assert_eq!(r.active_resource_count("owner", new_generation), Some(0));
    }

    #[test]
    fn explicit_disposal_then_unload_preserves_reverse_order_exactly_once() {
        let mut r = Registry::new();
        let generation = r.insert("owner");
        assert!(r.record("owner", generation, Resource::Hook(1)));
        assert!(r.record("owner", generation, Resource::Timer(2)));
        assert!(r.record("owner", generation, Resource::DbConn(3)));
        assert!(r.release("owner", generation, &Resource::Timer(2)));
        assert!(!r.release("owner", generation, &Resource::Timer(2)));
        assert_eq!(r.remove("owner").unwrap().ledger.teardown_order(),
            vec![Resource::DbConn(3), Resource::Hook(1)]);
    }
}
