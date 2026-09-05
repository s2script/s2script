//! Native-only model of the baseline SDK hook lookup in `core/src/sdkhooks.rs`.
//!
//! This deliberately retains the production baseline's shape: one ordered `Vec<Entry>`,
//! string kind matching, and detached snapshot results that clone the owner string and
//! handler token.  It has no V8 or engine dependencies; the benchmark measures the native
//! lookup/bookkeeping work that Task 7 changes.

#[derive(Clone, Debug)]
struct Entry {
    owner: String,
    generation: u64,
    entity_id: u64,
    kind: String,
    handler: u64,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub owner: String,
    pub generation: u64,
    pub handler: u64,
}

pub struct HookStore {
    entries: Vec<Entry>,
}

impl HookStore {
    /// Build a worst-case lookup layout with `matching` addressed subscribers at the end.
    /// All preceding entries use the same kind but another entity id, so the baseline scan
    /// must inspect both keys for every total-hook scale.
    pub fn with_total(total: usize, matching: usize, target_entity: u64) -> Self {
        assert!(matching <= total, "matching hooks must fit in total hooks");
        let mut entries = Vec::with_capacity(total);
        for i in 0..(total - matching) {
            entries.push(Entry {
                owner: format!("distractor-{i}"),
                generation: i as u64,
                entity_id: target_entity.wrapping_add(i as u64 + 1),
                kind: "OnTakeDamage".to_string(),
                handler: i as u64,
            });
        }
        for i in 0..matching {
            entries.push(Entry {
                owner: format!("target-{i}"),
                generation: i as u64,
                entity_id: target_entity,
                kind: "OnTakeDamage".to_string(),
                handler: (total - matching + i) as u64,
            });
        }
        Self { entries }
    }

    /// Equivalent to the baseline `snapshot_kind` filter/map/collect.
    pub fn snapshot_kind(&self, entity_id: u64, kind: &str) -> Vec<Snapshot> {
        self.entries
            .iter()
            .filter(|entry| entry.entity_id == entity_id && entry.kind == kind)
            .map(|entry| Snapshot {
                owner: entry.owner.clone(),
                generation: entry.generation,
                handler: entry.handler,
            })
            .collect()
    }
}
