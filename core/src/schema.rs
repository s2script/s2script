//! Engine-generic SchemaSystem offset resolution + cache (V8-free logic).
//! The live SchemaSystem query lives in v8host (needs the raw pointer); this module
//! owns the cache + miss-once-WARN policy so it is unit-testable without an engine.
use std::collections::HashMap;

pub struct OffsetCache {
    map: HashMap<(String, String), i32>,
}

impl OffsetCache {
    pub fn new() -> Self { OffsetCache { map: HashMap::new() } }

    /// Resolve `(class, field)` to a byte offset, caching the result (including a `-1` miss).
    /// `raw` performs the live SchemaSystem lookup (returns `-1` if not found); `log` receives a
    /// one-time WARN message on a miss. `raw`/`log` are called at most once per distinct key.
    pub fn resolve(
        &mut self,
        class: &str,
        field: &str,
        raw: impl Fn(&str, &str) -> i32,
        log: impl Fn(&str),
    ) -> i32 {
        let key = (class.to_string(), field.to_string());
        if let Some(&off) = self.map.get(&key) {
            return off;
        }
        let off = raw(class, field);
        if off < 0 {
            log(&format!(
                "WARN: schema offset not found for {class}::{field}; accessor disabled"
            ));
        }
        self.map.insert(key, off);
        off
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn resolves_once_and_caches_hits() {
        let mut cache = OffsetCache::new();
        let calls = Cell::new(0);
        let raw = |_c: &str, _f: &str| { calls.set(calls.get() + 1); 320 };
        let noop = |_s: &str| {};
        // Engine-generic placeholder names: core must contain NO game identifiers, and the
        // boundary gate (scripts/check-core-boundary.sh) greps ALL of core/src including tests.
        assert_eq!(cache.resolve("ExampleClass", "m_value", &raw, &noop), 320);
        assert_eq!(cache.resolve("ExampleClass", "m_value", &raw, &noop), 320);
        assert_eq!(calls.get(), 1, "second lookup must hit the cache, not re-query");
    }

    #[test]
    fn caches_and_warns_once_on_miss() {
        let mut cache = OffsetCache::new();
        let raw = |_c: &str, _f: &str| -1;
        let warns = Cell::new(0);
        let log = |_s: &str| warns.set(warns.get() + 1);
        assert_eq!(cache.resolve("X", "y", &raw, &log), -1);
        assert_eq!(cache.resolve("X", "y", &raw, &log), -1);
        assert_eq!(warns.get(), 1, "a missing field must WARN at most once (cached miss)");
    }
}
