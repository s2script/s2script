//! Engine-generic SchemaSystem offset resolution + cache (V8-free logic).
//! The live SchemaSystem query lives in v8host (needs the raw pointer); this module
//! owns the cache + miss-once-WARN policy so it is unit-testable without an engine.
use std::collections::{HashMap, HashSet};

pub struct OffsetCache {
    /// Positive offsets that have been confirmed as valid hits (offset >= 0).
    hits: HashMap<(String, String), i32>,
    /// Keys for which we have already emitted a WARN so we don't repeat it.
    warned: HashSet<(String, String)>,
}

impl OffsetCache {
    pub fn new() -> Self {
        OffsetCache {
            hits: HashMap::new(),
            warned: HashSet::new(),
        }
    }

    /// Resolve `(class, field)` to a byte offset.
    ///
    /// * **Hit (offset >= 0):** cached on first resolution; subsequent calls return
    ///   the cached value without calling `raw` again.
    /// * **Miss (offset < 0):** NOT cached — `raw` is called on every miss so that
    ///   the lookup retries once the schema is populated (e.g. after a map loads).
    ///   `log` is called at most once per distinct key (controlled by the `warned` set).
    pub fn resolve(
        &mut self,
        class: &str,
        field: &str,
        raw: impl Fn(&str, &str) -> i32,
        log: impl Fn(&str),
    ) -> i32 {
        let key = (class.to_string(), field.to_string());

        // Fast path: confirmed hit.
        if let Some(&off) = self.hits.get(&key) {
            return off;
        }

        // Query the live SchemaSystem.
        let off = raw(class, field);

        if off >= 0 {
            // Cache the hit so future calls are free.
            self.hits.insert(key, off);
        } else {
            // Miss: do NOT cache — the schema may not be ready yet.  Warn at most once.
            if !self.warned.contains(&key) {
                log(&format!(
                    "WARN: schema offset not found for {class}::{field}; accessor disabled"
                ));
                self.warned.insert(key);
            }
        }

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

    /// Misses are NOT cached: `raw` is called on every miss so the lookup retries
    /// once the schema is populated.  The WARN fires at most once per key.
    /// When `raw` eventually returns a valid offset the hit is cached normally.
    #[test]
    fn miss_requeried_warns_once_then_caches_on_hit() {
        let mut cache = OffsetCache::new();
        let raw_calls = Cell::new(0u32);
        // First two calls return -1 (schema not ready); third call returns a real offset.
        let raw = |_c: &str, _f: &str| -> i32 {
            raw_calls.set(raw_calls.get() + 1);
            if raw_calls.get() < 3 { -1 } else { 42 }
        };
        let warns = Cell::new(0u32);
        let log = |_s: &str| warns.set(warns.get() + 1);

        // First miss: raw called, warn emitted.
        assert_eq!(cache.resolve("ExampleClass", "m_value", &raw, &log), -1);
        assert_eq!(raw_calls.get(), 1);
        assert_eq!(warns.get(), 1, "first miss must warn");

        // Second miss: raw called again (not cached), no new warn.
        assert_eq!(cache.resolve("ExampleClass", "m_value", &raw, &log), -1);
        assert_eq!(raw_calls.get(), 2, "miss must re-query raw on each call");
        assert_eq!(warns.get(), 1, "warn must fire at most once per key");

        // Third call: raw now returns a valid offset → hit is cached.
        assert_eq!(cache.resolve("ExampleClass", "m_value", &raw, &log), 42);
        assert_eq!(raw_calls.get(), 3);
        assert_eq!(warns.get(), 1, "no extra warn after a successful hit");

        // Fourth call: served from the hits cache, raw not called again.
        assert_eq!(cache.resolve("ExampleClass", "m_value", &raw, &log), 42);
        assert_eq!(raw_calls.get(), 3, "cached hit must not re-query raw");
    }
}
