//! Inter-plugin interface registry (Slice 4.5) — PURE bookkeeping.
//! Engine-generic: NO CS2 identifiers, NO V8. The V8 `Global<Function>` handles (methods +
//! subscriber callbacks) live in `v8host`, keyed by the same interface-name / sub-id strings this
//! module tracks. This module is the source of truth for what is published, who imports what, and
//! the reverse-dependency unload order.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Hard, Optional }

/// What `v8host::iface_call` should do for a (consumer, interface, method) triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTarget { Ok, Unavailable, VersionMismatch }

#[derive(Debug, Clone)]
pub struct Subscriber {
    pub sub_id: u64,
    pub consumer_id: String,
    pub consumer_gen: u64,
    pub event: String,
}

#[derive(Debug, Clone)]
pub struct InterfaceEntry {
    pub version: String,
    pub producer_id: String,
    pub producer_gen: u64,
    pub method_names: Vec<String>,
    pub subscribers: Vec<Subscriber>,
}

struct ImportDecl { range: String, kind: Kind }

pub struct InterfaceRegistry {
    ifaces: HashMap<String, InterfaceEntry>,
    imports: HashMap<String, HashMap<String, ImportDecl>>, // plugin_id → (iface_name → decl)
}

/// Parse the leading semver major from a version or a range operator ("^1.2.3","1.x","~1.0" → 1).
/// Returns None when there is no leading integer.
fn leading_major(s: &str) -> Option<u32> {
    let after_op = s.trim_start_matches(|c: char| !c.is_ascii_digit());
    let digits: String = after_op.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

/// Minimal caret/major/x/star satisfaction (matches the Slice-4 apiVersion style). A concrete
/// `version` satisfies `range` iff `range` is `*`, OR both have a leading major and the majors match.
pub fn version_satisfies(range: &str, version: &str) -> bool {
    if range.trim() == "*" { return true; }
    match (leading_major(range), leading_major(version)) {
        (Some(ra), Some(va)) => ra == va,
        _ => false,
    }
}

impl InterfaceRegistry {
    pub fn new() -> Self {
        Self { ifaces: HashMap::new(), imports: HashMap::new() }
    }

    pub fn publish(&mut self, name: &str, version: &str, producer_id: &str, producer_gen: u64, method_names: Vec<String>) {
        self.ifaces.insert(name.to_string(), InterfaceEntry {
            version: version.to_string(),
            producer_id: producer_id.to_string(),
            producer_gen,
            method_names,
            subscribers: Vec::new(),
        });
    }

    pub fn lookup(&self, name: &str) -> Option<&InterfaceEntry> {
        self.ifaces.get(name)
    }

    pub fn remove_by_producer(&mut self, producer_id: &str) -> Vec<String> {
        let names: Vec<String> = self.ifaces.iter()
            .filter(|(_, e)| e.producer_id == producer_id)
            .map(|(n, _)| n.clone())
            .collect();
        for n in &names { self.ifaces.remove(n); }
        names
    }

    pub fn set_imports(&mut self, plugin_id: &str, decls: Vec<(String, String, Kind)>) {
        let map = decls.into_iter()
            .map(|(name, range, kind)| (name, ImportDecl { range, kind }))
            .collect();
        self.imports.insert(plugin_id.to_string(), map);
    }

    pub fn clear_imports(&mut self, plugin_id: &str) {
        self.imports.remove(plugin_id);
    }

    pub fn dep_kind(&self, plugin_id: &str, name: &str) -> Option<Kind> {
        self.imports.get(plugin_id).and_then(|m| m.get(name)).map(|d| d.kind)
    }

    fn import_range(&self, plugin_id: &str, name: &str) -> Option<&str> {
        self.imports.get(plugin_id).and_then(|m| m.get(name)).map(|d| d.range.as_str())
    }

    pub fn is_available(&self, plugin_id: &str, name: &str) -> bool {
        matches!(self.call_target_inner(plugin_id, name, None), CallTarget::Ok)
    }

    pub fn call_target(&self, plugin_id: &str, name: &str, method: &str) -> CallTarget {
        self.call_target_inner(plugin_id, name, Some(method))
    }

    fn call_target_inner(&self, plugin_id: &str, name: &str, method: Option<&str>) -> CallTarget {
        let Some(range) = self.import_range(plugin_id, name) else { return CallTarget::Unavailable };
        let Some(entry) = self.ifaces.get(name) else { return CallTarget::Unavailable };
        if !version_satisfies(range, &entry.version) { return CallTarget::VersionMismatch; }
        if let Some(m) = method {
            if !entry.method_names.iter().any(|n| n == m) { return CallTarget::Unavailable; }
        }
        CallTarget::Ok
    }

    pub fn add_subscriber(&mut self, name: &str, sub: Subscriber) -> bool {
        match self.ifaces.get_mut(name) {
            Some(e) => { e.subscribers.push(sub); true }
            None => false,
        }
    }

    pub fn remove_subscribers_by_consumer(&mut self, consumer_id: &str) -> Vec<(String, u64)> {
        let mut removed = Vec::new();
        for (name, e) in self.ifaces.iter_mut() {
            e.subscribers.retain(|s| {
                if s.consumer_id == consumer_id { removed.push((name.clone(), s.sub_id)); false }
                else { true }
            });
        }
        removed
    }

    pub fn live_subscriber_ids(&self, name: &str, event: &str, is_live: &dyn Fn(&str, u64) -> bool) -> Vec<u64> {
        let Some(e) = self.ifaces.get(name) else { return Vec::new() };
        e.subscribers.iter()
            .filter(|s| s.event == event && is_live(&s.consumer_id, s.consumer_gen))
            .map(|s| s.sub_id)
            .collect()
    }

    /// Producer id for `name`, if published (used by v8host to enter the right context).
    pub fn producer_of(&self, name: &str) -> Option<(String, u64)> {
        self.ifaces.get(name).map(|e| (e.producer_id.clone(), e.producer_gen))
    }

    fn imports_from(&self, consumer: &str, producer: &str) -> bool {
        let Some(decls) = self.imports.get(consumer) else { return false };
        decls.keys().any(|name| self.ifaces.get(name).map_or(false, |e| e.producer_id == producer))
    }

    /// Reverse-dependency unload order: a consumer (importer) is emitted BEFORE the producer it
    /// imports from, so its onUnload can still call producer methods. Cycles degrade to arbitrary
    /// order (appended as-is).
    pub fn unload_order(&self, ids: &[String]) -> Vec<String> {
        let mut remaining: Vec<String> = ids.to_vec();
        let mut order = Vec::new();
        while !remaining.is_empty() {
            // pick a plugin nobody-remaining imports from → safe to unload first (a top consumer).
            let pick = remaining.iter().position(|p| {
                !remaining.iter().any(|q| q != p && self.imports_from(q, p))
            });
            match pick {
                Some(i) => { order.push(remaining.remove(i)); }
                None => { order.append(&mut remaining); } // cycle
            }
        }
        order
    }

    pub fn clear(&mut self) {
        self.ifaces.clear();
        self.imports.clear();
    }
}

impl Default for InterfaceRegistry {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> InterfaceRegistry { InterfaceRegistry::new() }

    #[test]
    fn version_satisfies_caret_and_x_and_star() {
        assert!(version_satisfies("^1.0.0", "1.4.2"));
        assert!(version_satisfies("1.x", "1.0.0"));
        assert!(version_satisfies("*", "9.9.9"));
        assert!(!version_satisfies("^1.0.0", "2.0.0"));
        assert!(!version_satisfies("^2.0.0", "1.9.9"));
        assert!(!version_satisfies("1.x", "0.9.0"));
        assert!(!version_satisfies("garbage", "1.0.0"));
    }

    #[test]
    fn publish_then_lookup() {
        let mut r = reg();
        r.publish("@x/if", "1.2.0", "prod", 0, vec!["greet".into()]);
        let e = r.lookup("@x/if").expect("published");
        assert_eq!(e.version, "1.2.0");
        assert_eq!(e.producer_id, "prod");
        assert_eq!(e.method_names, vec!["greet".to_string()]);
        assert!(r.lookup("@nope").is_none());
    }

    #[test]
    fn remove_by_producer_drops_all_its_interfaces() {
        let mut r = reg();
        r.publish("@a", "1.0.0", "prod", 0, vec![]);
        r.publish("@b", "1.0.0", "prod", 0, vec![]);
        r.publish("@c", "1.0.0", "other", 0, vec![]);
        let mut removed = r.remove_by_producer("prod");
        removed.sort();
        assert_eq!(removed, vec!["@a".to_string(), "@b".to_string()]);
        assert!(r.lookup("@a").is_none());
        assert!(r.lookup("@c").is_some());
    }

    #[test]
    fn dep_kind_and_availability() {
        let mut r = reg();
        r.set_imports("cons", vec![
            ("@hard".into(), "^1.0.0".into(), Kind::Hard),
            ("@opt".into(), "^1.0.0".into(), Kind::Optional),
        ]);
        assert_eq!(r.dep_kind("cons", "@hard"), Some(Kind::Hard));
        assert_eq!(r.dep_kind("cons", "@opt"), Some(Kind::Optional));
        assert_eq!(r.dep_kind("cons", "@undeclared"), None);
        // not published yet → not available
        assert!(!r.is_available("cons", "@hard"));
        r.publish("@hard", "1.5.0", "prod", 0, vec![]);
        assert!(r.is_available("cons", "@hard"));
        // published but incompatible version → not available
        r.publish("@opt", "2.0.0", "prod2", 0, vec![]);
        assert!(!r.is_available("cons", "@opt"));
    }

    #[test]
    fn call_target_reports_unavailable_mismatch_ok() {
        let mut r = reg();
        r.set_imports("cons", vec![("@x".into(), "^1.0.0".into(), Kind::Hard)]);
        assert_eq!(r.call_target("cons", "@x", "greet"), CallTarget::Unavailable);
        r.publish("@x", "1.2.0", "prod", 0, vec!["greet".into()]);
        assert_eq!(r.call_target("cons", "@x", "greet"), CallTarget::Ok);
        assert_eq!(r.call_target("cons", "@x", "missingMethod"), CallTarget::Unavailable);
        r.publish("@x", "3.0.0", "prod", 1, vec!["greet".into()]);   // republished incompatible
        assert_eq!(r.call_target("cons", "@x", "greet"), CallTarget::VersionMismatch);
    }

    #[test]
    fn subscribers_add_and_remove_by_consumer() {
        let mut r = reg();
        r.publish("@x", "1.0.0", "prod", 0, vec![]);
        assert!(r.add_subscriber("@x", Subscriber { sub_id: 1, consumer_id: "cons".into(), consumer_gen: 0, event: "greeted".into() }));
        assert!(r.add_subscriber("@x", Subscriber { sub_id: 2, consumer_id: "cons".into(), consumer_gen: 0, event: "greeted".into() }));
        // adding to a missing interface → false
        assert!(!r.add_subscriber("@nope", Subscriber { sub_id: 3, consumer_id: "c".into(), consumer_gen: 0, event: "e".into() }));
        let mut removed = r.remove_subscribers_by_consumer("cons");
        removed.sort();
        assert_eq!(removed, vec![("@x".to_string(), 1), ("@x".to_string(), 2)]);
        assert!(r.lookup("@x").unwrap().subscribers.is_empty());
    }

    #[test]
    fn live_subscriber_ids_filters_by_event_and_liveness() {
        let mut r = reg();
        r.publish("@x", "1.0.0", "prod", 0, vec![]);
        r.add_subscriber("@x", Subscriber { sub_id: 1, consumer_id: "live".into(), consumer_gen: 0, event: "greeted".into() });
        r.add_subscriber("@x", Subscriber { sub_id: 2, consumer_id: "dead".into(), consumer_gen: 0, event: "greeted".into() });
        r.add_subscriber("@x", Subscriber { sub_id: 3, consumer_id: "live".into(), consumer_gen: 0, event: "other".into() });
        let is_live = |id: &str, _g: u64| id == "live";
        let ids = r.live_subscriber_ids("@x", "greeted", &is_live);
        assert_eq!(ids, vec![1]);   // 2 not live, 3 wrong event
    }

    #[test]
    fn unload_order_puts_consumers_before_producers() {
        let mut r = reg();
        r.publish("@x", "1.0.0", "prod", 0, vec![]);
        r.set_imports("cons", vec![("@x".into(), "^1.0.0".into(), Kind::Hard)]);
        // input order must not matter:
        assert_eq!(r.unload_order(&["prod".into(), "cons".into()]), vec!["cons".to_string(), "prod".to_string()]);
        assert_eq!(r.unload_order(&["cons".into(), "prod".into()]), vec!["cons".to_string(), "prod".to_string()]);
    }
}
