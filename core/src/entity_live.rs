//! The entity books — the ONLY liveness authority for entities (north-star §3.1,
//! Candidate D). `LIVE: index → (host-id, engine_serial)`, fed by the shim's
//! IEntityListener through the ffi entry (UNCONDITIONALLY — before/independent of the
//! JS mux dispatch), cleared at map start (the implicit epoch — no counter to stamp).
//! Engine memory is NEVER read to answer "is this entity alive". Host ids are u64,
//! monotonic, never reset across maps; JS-safe as f64 up to 2^53 mints. Game-thread
//! only (thread_local), like every other v8host-adjacent table.

use std::cell::{Cell, RefCell};
use crate::liveness::LiveTable;

thread_local! {
    static LIVE: RefCell<LiveTable<i32, i32>> = RefCell::new(LiveTable::new(1));
    /// Armed by `clear_for_map_transition`; consumed by the first simulating frame's
    /// repair sweep (north-star §7 / E0-V4 contingency: entities created before
    /// StartupServer POST or before the listener attached).
    static REPAIR_ARMED: Cell<bool> = Cell::new(false);
}

/// OnEntityCreated: mint a fresh host id (upsert — a same-index create replaces a
/// stale entry, which is itself an invalidation of any holder of the old id).
pub fn on_created(index: i32, engine_serial: i32) -> u64 {
    LIVE.with(|t| t.borrow_mut().insert(index, engine_serial))
}

/// OnEntitySpawned: repair-upsert. Present-and-matching keeps the create-minted id
/// (refs minted at create stay valid); absent or serial-mismatched mints fresh —
/// a create this table provably missed.
pub fn on_spawned(index: i32, engine_serial: i32) {
    LIVE.with(|t| {
        let mut t = t.borrow_mut();
        match t.get(&index) {
            Some((_, s)) if *s == engine_serial => {}
            _ => { t.insert(index, engine_serial); }
        }
    });
}

/// OnEntityDeleted: remove ONLY when the stored serial matches — a stale delete must
/// not evict a newer same-index entity. (A wrongly-kept entry still fails closed at
/// the slot-validation stage.)
pub fn on_deleted(index: i32, engine_serial: i32) {
    LIVE.with(|t| {
        let mut t = t.borrow_mut();
        let matches = t.get(&index).map_or(false, |(_, s)| *s == engine_serial);
        if matches { t.remove(&index); }
    });
}

pub fn lookup(index: i32) -> Option<(u64, i32)> {
    LIVE.with(|t| t.borrow().get(&index).map(|(id, s)| (id, *s)))
}

/// Adopt a decoded raw engine handle into the books: serial match → the table's id;
/// mismatch/absent → None. A dangling handle field can never mint a live ref.
pub fn adopt(index: i32, engine_serial: i32) -> Option<u64> {
    LIVE.with(|t| t.borrow().get(&index)
        .and_then(|(id, s)| if *s == engine_serial { Some(id) } else { None }))
}

/// (index, host-id) → the stored engine serial, for slot-side shim ops. None = the
/// books say not-live (fail-closed: the engine is never asked).
pub fn engine_serial_for(index: i32, id: u64) -> Option<i32> {
    if id == 0 { return None; }
    LIVE.with(|t| t.borrow().get(&index)
        .and_then(|(cur, s)| if cur == id { Some(*s) } else { None }))
}

/// Map transition: clear the whole table (this IS the epoch, implicit) + arm the sweep.
pub fn clear_for_map_transition() {
    LIVE.with(|t| t.borrow_mut().clear());
    REPAIR_ARMED.with(|c| c.set(true));
}

pub fn take_repair_armed() -> bool { REPAIR_ARMED.with(|c| c.replace(false)) }

/// Reconcile against a chunk-walk snapshot of live identity slots: upsert
/// absent/mismatched, evict entries whose slot is gone. Minting here is safe by
/// construction — the snapshot is read from system-owned identity chunks (the shim's
/// `ent_snapshot` op), never from instance memory.
pub fn repair_reconcile(live_slots: &[(i32, i32)]) {
    LIVE.with(|t| {
        let mut t = t.borrow_mut();
        let mut seen = std::collections::HashSet::new();
        for &(index, serial) in live_slots {
            seen.insert(index);
            match t.get(&index) {
                Some((_, s)) if *s == serial => {}
                _ => { t.insert(index, serial); }
            }
        }
        for k in t.keys() {
            if !seen.contains(&k) { t.remove(&k); }
        }
    });
}

pub fn len() -> usize { LIVE.with(|t| t.borrow().len()) }

#[cfg(test)]
pub fn reset_for_tests() {
    LIVE.with(|t| t.borrow_mut().clear());
    REPAIR_ARMED.with(|c| c.set(false));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() { reset_for_tests(); }

    #[test]
    fn create_lookup_adopt_and_id_translation() {
        fresh();
        let id = on_created(42, 7);
        assert!(id >= 1, "entity ids start at 1 (0 = never-live JS sentinel)");
        assert_eq!(lookup(42), Some((id, 7)));
        assert_eq!(adopt(42, 7), Some(id), "serial match adopts the table's id");
        assert_eq!(adopt(42, 8), None, "serial mismatch can never mint a live ref");
        assert_eq!(adopt(43, 7), None, "absent index can never mint a live ref");
        assert_eq!(engine_serial_for(42, id), Some(7));
        assert_eq!(engine_serial_for(42, id + 1), None);
        assert_eq!(engine_serial_for(42, 0), None, "id 0 is never live");
    }

    #[test]
    fn delete_removes_only_on_serial_match() {
        fresh();
        let id = on_created(5, 3);
        on_deleted(5, 9);                       // stale delete for a replaced slot
        assert_eq!(lookup(5), Some((id, 3)), "stale delete must not evict a newer entity");
        on_deleted(5, 3);
        assert_eq!(lookup(5), None);
    }

    #[test]
    fn spawn_repairs_a_missed_create_but_keeps_a_matching_id() {
        fresh();
        let id = on_created(6, 2);
        on_spawned(6, 2);                       // normal create→spawn: id survives
        assert_eq!(lookup(6), Some((id, 2)), "matching spawn keeps the create-minted id");
        on_spawned(7, 4);                       // spawn witnessed with NO create (missed feed)
        let (id7, s7) = lookup(7).expect("spawn upserts a missed create");
        assert!(id7 > id); assert_eq!(s7, 4);
        on_spawned(6, 9);                       // spawn with a DIFFERENT serial = slot reused unseen
        let (id6b, s6b) = lookup(6).unwrap();
        assert!(id6b > id7, "mismatched spawn mints fresh (old refs die)"); assert_eq!(s6b, 9);
    }

    #[test]
    fn map_transition_clears_arms_and_never_reuses_ids() {
        fresh();
        let id = on_created(10, 1);
        assert!(!take_repair_armed(), "not armed before any transition");
        clear_for_map_transition();
        assert_eq!(lookup(10), None, "the epoch: the whole table clears");
        assert!(take_repair_armed(), "transition arms the repair sweep");
        assert!(!take_repair_armed(), "take consumes");
        let id2 = on_created(10, 1);            // SAME (index, serial) on the new map
        assert!(id2 > id, "cross-map (index,serial) aliasing is impossible: fresh id");
    }

    #[test]
    fn repair_reconcile_upserts_and_evicts() {
        fresh();
        let kept = on_created(1, 11);           // present + matching → kept
        on_created(2, 22);                      // present, serial drifted → re-minted
        on_created(3, 33);                      // absent from snapshot → evicted
        repair_reconcile(&[(1, 11), (2, 99), (4, 44)]);
        assert_eq!(lookup(1), Some((kept, 11)), "matching entry keeps its id");
        let (_, s2) = lookup(2).unwrap(); assert_eq!(s2, 99, "drifted serial re-minted");
        assert_eq!(lookup(3), None, "gone-from-engine entry evicted (fail-closed)");
        assert!(lookup(4).is_some(), "never-seen live slot adopted");
        assert_eq!(len(), 3);
    }
}
