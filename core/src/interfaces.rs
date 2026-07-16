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

    /// Register (or re-register) an interface. Returns Err when a DIFFERENT producer already
    /// holds a live entry for `name` — implementations are alternatives (you run mapchooser OR
    /// mapchooser_extended, never both), spec §4.8. Re-publish by the SAME producer is a
    /// hot-reload and preserves subscribers.
    pub fn publish(
        &mut self,
        name: &str,
        version: &str,
        producer_id: &str,
        producer_gen: u64,
        method_names: Vec<String>,
    ) -> Result<(), String> {
        if let Some(existing) = self.ifaces.get(name) {
            if existing.producer_id != producer_id {
                return Err(format!(
                    "interface '{}' is already published by '{}' — '{}' cannot also publish it \
                     (implementations are alternatives; load only one)",
                    name, existing.producer_id, producer_id
                ));
            }
        }
        // Preserve any existing subscribers on republish of the same name: a producer updating its
        // interface in place keeps its consumers subscribed. (A fresh producer's entry starts empty
        // because `remove_by_producer` cleared the prior one on unload.)
        let subscribers = self.ifaces.get(name).map(|e| e.subscribers.clone()).unwrap_or_default();
        self.ifaces.insert(name.to_string(), InterfaceEntry {
            version: version.to_string(),
            producer_id: producer_id.to_string(),
            producer_gen,
            method_names,
            subscribers,
        });
        Ok(())
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

    /// Drop (and return the ids of) the given consumer's subs on a specific (name, event).
    pub fn remove_subscribers_by_consumer_on(&mut self, consumer_id: &str, name: &str, event: &str) -> Vec<u64> {
        let mut dropped = Vec::new();
        if let Some(e) = self.ifaces.get_mut(name) {
            e.subscribers.retain(|s| {
                if s.consumer_id == consumer_id && s.event == event {
                    dropped.push(s.sub_id);
                    false
                } else {
                    true
                }
            });
        }
        dropped
    }

    /// The consumer id owning `sub_id` on interface `name`, if present.
    pub fn consumer_of_sub(&self, name: &str, sub_id: u64) -> Option<String> {
        self.ifaces.get(name)?.subscribers.iter().find(|s| s.sub_id == sub_id).map(|s| s.consumer_id.clone())
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
    /// Call this BEFORE any `remove_by_producer` / entry removal — edges are derived
    /// from currently-published interfaces, so a removed producer's edges become invisible. (Task 7's
    /// `unload_all` computes the order once up front, then unloads.)
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

    #[test]
    fn producer_of_returns_id_and_generation() {
        let mut r = reg();
        assert!(r.producer_of("@x").is_none());
        r.publish("@x", "1.0.0", "prod", 3, vec![]);
        assert_eq!(r.producer_of("@x"), Some(("prod".to_string(), 3)));
    }

    #[test]
    fn clear_empties_ifaces_and_imports() {
        let mut r = reg();
        r.publish("@x", "1.0.0", "prod", 0, vec![]);
        r.set_imports("cons", vec![("@x".into(), "^1.0.0".into(), Kind::Hard)]);
        r.clear();
        assert!(r.lookup("@x").is_none());
        assert_eq!(r.dep_kind("cons", "@x"), None);
    }

    #[test]
    fn remove_subscribers_by_consumer_on_drops_only_matching_name_event() {
        let mut r = reg();
        r.publish("@x", "1.0.0", "prod", 0, vec![]);
        r.add_subscriber("@x", Subscriber { sub_id: 1, consumer_id: "cons".into(), consumer_gen: 0, event: "greeted".into() });
        r.add_subscriber("@x", Subscriber { sub_id: 2, consumer_id: "cons".into(), consumer_gen: 0, event: "greeted".into() });
        r.add_subscriber("@x", Subscriber { sub_id: 3, consumer_id: "cons".into(), consumer_gen: 0, event: "other".into() });
        r.add_subscriber("@x", Subscriber { sub_id: 4, consumer_id: "other_cons".into(), consumer_gen: 0, event: "greeted".into() });
        // Only drop cons's subs on (name=@x, event=greeted); leave other and other_cons alone.
        let mut dropped = r.remove_subscribers_by_consumer_on("cons", "@x", "greeted");
        dropped.sort();
        assert_eq!(dropped, vec![1u64, 2]);
        let remaining: Vec<u64> = r.lookup("@x").unwrap().subscribers.iter().map(|s| s.sub_id).collect();
        assert!(remaining.contains(&3), "cons's 'other' event sub must survive");
        assert!(remaining.contains(&4), "other_cons's sub must survive");
        assert!(!remaining.contains(&1) && !remaining.contains(&2));
    }

    #[test]
    fn consumer_of_sub_returns_owner() {
        let mut r = reg();
        r.publish("@x", "1.0.0", "prod", 0, vec![]);
        r.add_subscriber("@x", Subscriber { sub_id: 42, consumer_id: "cons".into(), consumer_gen: 0, event: "greeted".into() });
        assert_eq!(r.consumer_of_sub("@x", 42), Some("cons".to_string()));
        assert_eq!(r.consumer_of_sub("@x", 99), None);  // sub_id not found
        assert_eq!(r.consumer_of_sub("@nope", 42), None); // interface not found
    }

    #[test]
    fn republish_preserves_existing_subscribers() {
        let mut r = reg();
        r.publish("@x", "1.0.0", "prod", 0, vec!["greet".into()]);
        r.add_subscriber("@x", Subscriber { sub_id: 5, consumer_id: "cons".into(), consumer_gen: 0, event: "greeted".into() });
        r.publish("@x", "1.1.0", "prod", 0, vec!["greet".into(), "wave".into()]); // in-place update
        let e = r.lookup("@x").unwrap();
        assert_eq!(e.version, "1.1.0");
        assert_eq!(e.method_names, vec!["greet".to_string(), "wave".to_string()]);
        assert_eq!(e.subscribers.len(), 1, "existing subscriber preserved across republish");
    }

    #[test]
    fn republish_by_the_same_producer_succeeds_and_keeps_subscribers() {
        let mut r = InterfaceRegistry::new();
        r.publish("@c/mapchooser", "1.2.0", "@edge/mce", 1, vec!["pick".into()]).expect("first");
        r.add_subscriber("@c/mapchooser", Subscriber {
            sub_id: 7, consumer_id: "@x/rtv".into(), consumer_gen: 1, event: "changed".into(),
        });
        // Same producer republishing (hot-reload) is allowed and preserves subscribers.
        r.publish("@c/mapchooser", "1.3.0", "@edge/mce", 2, vec!["pick".into()]).expect("republish");
        let e = r.lookup("@c/mapchooser").expect("entry");
        assert_eq!(e.version, "1.3.0");
        assert_eq!(e.subscribers.len(), 1, "republish must keep subscribers");
    }

    #[test]
    fn a_second_live_producer_of_the_same_interface_is_rejected() {
        let mut r = InterfaceRegistry::new();
        r.publish("@c/mapchooser", "1.2.0", "@edge/mce", 1, vec!["pick".into()]).expect("first");
        // A DIFFERENT producer claiming the same live name: implementations are alternatives.
        let err = r.publish("@c/mapchooser", "1.2.0", "@stock/mapchooser", 1, vec!["pick".into()])
            .expect_err("second producer must be rejected");
        assert!(err.contains("@c/mapchooser"), "error names the interface: {}", err);
        assert!(err.contains("@edge/mce"), "error names the incumbent producer: {}", err);
        // The incumbent is untouched.
        assert_eq!(r.lookup("@c/mapchooser").expect("entry").producer_id, "@edge/mce");
    }

    #[test]
    fn a_new_producer_may_claim_a_name_after_the_incumbent_unloads() {
        let mut r = InterfaceRegistry::new();
        r.publish("@c/mapchooser", "1.2.0", "@edge/mce", 1, vec!["pick".into()]).expect("first");
        r.remove_by_producer("@edge/mce");
        r.publish("@c/mapchooser", "1.2.0", "@stock/mapchooser", 1, vec!["pick".into()])
            .expect("free after unload");
        assert_eq!(r.lookup("@c/mapchooser").expect("entry").producer_id, "@stock/mapchooser");
    }

    // --- Characterization: version_satisfies is MAJOR-ONLY (design spec §10). ---
    // These document the hole this slice does NOT fix. The semver-unification spec
    // inverts every assertion below; until then they lock in what we know is wrong.

    #[test]
    fn characterize_major_only_matching_accepts_wrong_minors_pre_1_0() {
        // npm semantics: ^0.1.0 pins the minor, so 0.2.0 must NOT satisfy it.
        // We accept it. Pre-1.0, every range matches every version.
        assert!(version_satisfies("^0.1.0", "0.2.0"), "KNOWN WRONG: 0.x caret ignores the minor");
        assert!(version_satisfies("^0.1.0", "0.99.0"), "KNOWN WRONG");
        assert!(version_satisfies("^0.2.0", "0.1.0"), "KNOWN WRONG: this is the zones drift");
    }

    #[test]
    fn characterize_major_only_matching_accepts_older_minors_post_1_0() {
        // A consumer that typechecked against a 1.2.0 contract binds a 1.0.0 producer
        // that lacks the methods 1.2.0 promised → the proxy throws at call time.
        assert!(version_satisfies("^1.2.0", "1.0.0"), "KNOWN WRONG: older minor satisfies a caret");
    }

    #[test]
    fn characterize_major_mismatch_is_correctly_refused() {
        // The one thing major-only DOES get right.
        assert!(!version_satisfies("^1.0.0", "2.0.0"));
        assert!(!version_satisfies("^2.0.0", "1.0.0"));
    }
}
