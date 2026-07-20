//! Plugin registry, per-plugin ledger, generation counter, and reverse-teardown order.
//! Pure logic — no V8, no CS2 identifiers. The V8 context lives in v8host, keyed by the
//! same plugin id string. This module is the teardown authority and async-liveness guard.

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

/// A ledgered resource that must be torn down when a plugin unloads.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Records every resource a plugin acquires, in acquisition order.
/// `teardown_order()` returns them reversed (last-acquired torn down first).
pub struct PluginLedger {
    /// All resources in acquisition order.
    order: Vec<Resource>,
    /// Convenience: hook subscription ids in acquisition order.
    pub hook_subs: Vec<u64>,
    /// Convenience: timer ids in acquisition order.
    pub timers: Vec<u64>,
    /// Convenience: job ids in acquisition order.
    pub jobs: Vec<u64>,
    /// Convenience: published interface names in acquisition order.
    pub interfaces: Vec<String>,
    /// Convenience: event subscription ids in acquisition order.
    pub event_subs: Vec<u64>,
    /// Convenience: import edges (interface names) in acquisition order.
    pub imports: Vec<String>,
}

impl PluginLedger {
    pub fn new() -> Self {
        Self {
            order: Vec::new(),
            hook_subs: Vec::new(),
            timers: Vec::new(),
            jobs: Vec::new(),
            interfaces: Vec::new(),
            event_subs: Vec::new(),
            imports: Vec::new(),
        }
    }

    pub fn record_hook(&mut self, id: u64) {
        self.order.push(Resource::Hook(id));
        self.hook_subs.push(id);
    }

    pub fn record_timer(&mut self, id: u64) {
        self.order.push(Resource::Timer(id));
        self.timers.push(id);
    }

    pub fn record_job(&mut self, id: u64) {
        self.order.push(Resource::Job(id));
        self.jobs.push(id);
    }

    pub fn record_interface(&mut self, name: String) {
        self.order.push(Resource::Interface(name.clone()));
        self.interfaces.push(name);
    }

    pub fn record_event_sub(&mut self, id: u64) {
        self.order.push(Resource::EventSub(id));
        self.event_subs.push(id);
    }

    pub fn record_import(&mut self, name: String) {
        self.order.push(Resource::Import(name.clone()));
        self.imports.push(name);
    }

    /// Record an open DB connection handle against this plugin (teardown authority for Task 3).
    pub fn record_db_conn(&mut self, handle: u64) {
        self.order.push(Resource::DbConn(handle));
    }

    /// Record an open WebSocket connection id against this plugin (teardown authority, ws Task 2).
    pub fn record_ws_conn(&mut self, id: u64) {
        self.order.push(Resource::WsConn(id));
    }

    /// Record an open raw-socket (TCP/UDP) connection id against this plugin (teardown authority,
    /// net Task 2).
    pub fn record_net_conn(&mut self, id: u64) {
        self.order.push(Resource::NetConn(id));
    }

    /// Record an open remote-SQL pool handle against this plugin (teardown authority, remote-db Task 2).
    pub fn record_remote_db_conn(&mut self, handle: u64) {
        self.order.push(Resource::RemoteDbConn(handle));
    }

    /// Resources in REVERSE acquisition order — last-acquired torn down first.
    pub fn teardown_order(&self) -> Vec<Resource> {
        self.order.iter().rev().cloned().collect()
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
        Self { table: crate::liveness::LiveTable::new(0) }
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
        assert_eq!(entry.ledger.timers, vec![7]);
        assert!(!r.is_live("a", g), "removed plugin is not live");
        assert!(r.remove("a").is_none());
    }

    #[test]
    fn generations_come_from_one_shared_monotonic_counter_starting_at_zero() {
        let mut r = Registry::new();
        let a = r.insert("a");
        let b = r.insert("b");
        let a2 = r.insert("a");                  // reload of a
        assert_eq!(a, 0, "first generation is 0 (async-resolver unwrap_or(0) compat)");
        assert!(b > a && a2 > b, "one shared counter across ids: {a} {b} {a2}");
        assert_eq!(r.generation_of("b"), Some(b));
        assert_eq!(r.ids().len(), 2);
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
    fn record_iface_resources_populate_convenience_vecs() {
        let mut l = PluginLedger::new();
        l.record_interface("@x/if".into());
        l.record_event_sub(3);
        l.record_import("@y/dep".into());
        assert_eq!(l.interfaces, vec!["@x/if".to_string()]);
        assert_eq!(l.event_subs, vec![3]);
        assert_eq!(l.imports, vec!["@y/dep".to_string()]);
    }
}
