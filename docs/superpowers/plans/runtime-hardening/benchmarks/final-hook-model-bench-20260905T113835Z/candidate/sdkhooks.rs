//! Native-only model of Task 7's indexed SDK hook lookup.
//!
//! This mirrors the candidate's nested entity/kind maps and detached snapshot clones. It has no
//! V8, SourceHook, shim, engine, or process-startup work.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

struct HookHasher(u64);

impl Default for HookHasher {
    fn default() -> Self { Self(0xcbf29ce484222325) }
}

impl Hasher for HookHasher {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn write_u64(&mut self, value: u64) {
        let mut value = value;
        value ^= value >> 30;
        value = value.wrapping_mul(0xbf58476d1ce4e5b9);
        value ^= value >> 27;
        value = value.wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        self.0 ^= value;
        self.0 = self.0.wrapping_mul(0x100000001b3);
    }
}

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<HookHasher>>;

#[derive(Clone, Debug)]
struct Entry {
    owner: String,
    generation: u64,
    handler: u64,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub owner: String,
    pub generation: u64,
    pub handler: u64,
}

#[derive(Default)]
pub struct HookStore {
    buckets: FastMap<u64, FastMap<String, Vec<Entry>>>,
    kind_entities: FastMap<String, Vec<EntityAddress>>,
    next_sub_id: u64,
}

struct EntityAddress {
    entity_id: u64,
    entity_index: i32,
    engine_serial: i32,
    first_sub_id: u64,
}

impl HookStore {
    pub fn with_total(total: usize, matching: usize, target_entity: u64) -> Self {
        assert!(matching <= total, "matching hooks must fit in total hooks");
        let mut store = Self::default();
        for i in 0..(total - matching) {
            store.insert(
                target_entity.wrapping_add(i as u64 + 1),
                "OnTakeDamage",
                Entry {
                    owner: format!("distractor-{i}"),
                    generation: i as u64,
                    handler: i as u64,
                },
            );
        }
        for i in 0..matching {
            store.insert(
                target_entity,
                "OnTakeDamage",
                Entry {
                    owner: format!("target-{i}"),
                    generation: i as u64,
                    handler: (total - matching + i) as u64,
                },
            );
        }
        store
    }

    pub fn with_distinct_entities(total: usize, kind: &str) -> Self {
        let mut store = Self::default();
        for i in 0..total {
            store.insert(
                TARGET_BASE.wrapping_add(i as u64),
                kind,
                Entry {
                    owner: format!("entity-{i}"),
                    generation: i as u64,
                    handler: i as u64,
                },
            );
        }
        store
    }

    fn insert(&mut self, entity_id: u64, kind: &str, entry: Entry) {
        let sub_id = self.next_sub_id;
        self.next_sub_id += 1;
        let kinds = self.buckets.entry(entity_id).or_default();
        let first = !kinds.contains_key(kind);
        kinds.entry(kind.to_string()).or_default().push(entry);
        if first {
            self.kind_entities.entry(kind.to_string()).or_default().push(EntityAddress {
                entity_id,
                entity_index: entity_id as i32,
                engine_serial: entity_id as i32,
                first_sub_id: sub_id,
            });
        }
    }

    pub fn snapshot_kind(&self, entity_id: u64, kind: &str) -> Vec<Snapshot> {
        self.snapshot_kind_with_visitor(entity_id, kind, || {})
    }

    fn snapshot_kind_with_visitor(
        &self,
        entity_id: u64,
        kind: &str,
        mut visited: impl FnMut(),
    ) -> Vec<Snapshot> {
        self.buckets
            .get(&entity_id)
            .and_then(|kinds| kinds.get(kind))
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        visited();
                        Snapshot {
                            owner: entry.owner.clone(),
                            generation: entry.generation,
                            handler: entry.handler,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn snapshot_kind_with_visit_count(
        &self,
        entity_id: u64,
        kind: &str,
    ) -> (Vec<Snapshot>, usize) {
        let mut visited = 0;
        let snapshot = self.snapshot_kind_with_visitor(entity_id, kind, || visited += 1);
        (snapshot, visited)
    }

    pub fn snapshot_kind_entities(&self, kind: &str) -> Vec<(i32, i32)> {
        self.snapshot_kind_entities_with_visitor(kind, || {})
    }

    fn snapshot_kind_entities_with_visitor(
        &self,
        kind: &str,
        mut visited: impl FnMut(),
    ) -> Vec<(i32, i32)> {
        self.kind_entities.get(kind).map(|entities| {
            entities.iter().map(|entity| {
                visited();
                (entity.entity_index, entity.engine_serial)
            }).collect()
        }).unwrap_or_default()
    }

    pub fn snapshot_kind_entities_with_visit_count(
        &self,
        kind: &str,
    ) -> (Vec<(i32, i32)>, usize) {
        let mut visited = 0;
        let entities = self.snapshot_kind_entities_with_visitor(kind, || visited += 1);
        (entities, visited)
    }

    pub fn kind_membership_metadata(&self, kind: &str) -> Vec<(u64, u64)> {
        self.kind_entities.get(kind).map(|entities| {
            entities.iter().map(|entity| (entity.entity_id, entity.first_sub_id)).collect()
        }).unwrap_or_default()
    }
}

const TARGET_BASE: u64 = 1;
