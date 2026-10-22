//! The entity books — the ONLY liveness authority for entities (north-star §3.1,
//! Candidate D). `LIVE: index → (host-id, engine_serial)`, fed by the shim's
//! IEntityListener through the ffi entry (UNCONDITIONALLY — before/independent of the
//! JS mux dispatch), cleared at map start (the entity identity epoch).
//! Engine memory is NEVER read to answer "is this entity alive". Host ids are u64,
//! monotonic, never reset across maps; JS-safe as f64 up to 2^53 mints. Game-thread
//! only (thread_local), like every other v8host-adjacent table.

use std::cell::{Cell, RefCell};
use crate::liveness::LiveTable;

thread_local! {
    static MAP_EPOCH: Cell<u64> = const {Cell::new(1)};
    static LIVE: RefCell<LiveTable<i32, i32>> = RefCell::new(LiveTable::new(1));
    /// Armed by `clear_for_map_transition`; consumed by the first simulating frame's
    /// repair sweep (north-star §7 / E0-V4 contingency: entities created before
    /// StartupServer POST or before the listener attached).
    static REPAIR_ARMED: Cell<bool> = Cell::new(false);
    /// A flat mirror of `LIVE` for in-range indices: `(host id, engine serial)` per index, id 0 =
    /// absent. Every entity field access asks the books, and indexing a slice is cheaper than a
    /// hashed lookup. Written only next to a `LIVE` write (`mirror`), so the two never disagree.
    static FLAT: RefCell<Box<[(u64, i32)]>> = RefCell::new(vec![(0, 0); FLAT_LEN].into_boxed_slice());
}

/// Every index a `CEntityHandle` can carry. Indices outside it stay in `LIVE` only.
const FLAT_LEN: usize = 1 << crate::entity::HANDLE_ENTRY_BITS;

fn flat_slot(index: i32) -> Option<usize> {
    usize::try_from(index).ok().filter(|&i| i < FLAT_LEN)
}

/// Copy `LIVE`'s entry for `index` (or its absence) into `FLAT`.
fn mirror(t: &LiveTable<i32, i32>, index: i32) {
    let Some(i) = flat_slot(index) else { return };
    let entry = t.get(&index).map_or((0, 0), |(id, s)| (id, *s));
    FLAT.with(|f| f.borrow_mut()[i] = entry);
}

/// Rebuild all of `FLAT` from `LIVE` (map clear, repair sweep).
fn mirror_all(t: &LiveTable<i32, i32>) {
    FLAT.with(|f| {
        let mut f = f.borrow_mut();
        f.fill((0, 0));
        for index in t.keys() {
            if let (Some(i), Some((id, s))) = (flat_slot(index), t.get(&index)) { f[i] = (id, *s); }
        }
    });
}

/// OnEntityCreated: mint a fresh host id (upsert — a same-index create replaces a
/// stale entry, which is itself an invalidation of any holder of the old id).
pub fn on_created(index: i32, engine_serial: i32) -> u64 {
    let (id, retired) = LIVE.with(|t| {
        let mut t = t.borrow_mut();
        let retired = t.get(&index).map(|(id, _)| id);
        let id = t.insert(index, engine_serial);
        mirror(&t, index);
        (id, retired)
    });
    retire_lifecycle(index, retired);
    id
}

/// OnEntitySpawned: repair-upsert. Present-and-matching keeps the create-minted id
/// (refs minted at create stay valid); absent or serial-mismatched mints fresh —
/// a create this table provably missed.
pub fn on_spawned(index: i32, engine_serial: i32) {
    let retired = LIVE.with(|t| {
        let mut t = t.borrow_mut();
        match t.get(&index) {
            Some((_, s)) if *s == engine_serial => None,
            previous => {
                let retired = previous.map(|(id, _)| id);
                t.insert(index, engine_serial);
                mirror(&t, index);
                retired
            }
        }
    });
    retire_lifecycle(index, retired);
}

/// OnEntityDeleted: remove ONLY when the stored serial matches — a stale delete must
/// not evict a newer same-index entity. (A wrongly-kept entry still fails closed at
/// the slot-validation stage.) Returns the evicted host id so SDKHooks can unhook.
pub fn on_deleted(index: i32, engine_serial: i32) -> Option<u64> {
    let removed = LIVE.with(|t| {
        let mut t = t.borrow_mut();
        let matches = t.get(&index).map_or(false, |(_, s)| *s == engine_serial);
        if matches {
            let removed = t.remove(&index).map(|(id, _)| id);
            mirror(&t, index);
            removed
        } else {
            None
        }
    });
    retire_lifecycle(index, removed);
    removed
}

fn retire_lifecycle(index: i32, retired: Option<u64>) {
    // Only the displaced identity can have become stale. New entities, matching
    // spawn notifications and stale deletes do not require registry-wide sweeps.
    if let Some(id) = retired { crate::surface_leases::retire_entity(index, id); }
    // Even no-op lifecycle notifications still supersede an in-flight switch claim.
    crate::shared_entity_switch::on_entity_lifecycle(index, retired);
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
    if let Some(i) = flat_slot(index) {
        return FLAT.with(|f| {
            let (cur, s) = *f.try_borrow().ok()?.get(i)?;
            if cur == id { Some(s) } else { None }
        });
    }
    LIVE.with(|t| t.borrow().get(&index)
        .and_then(|(cur, s)| if cur == id { Some(*s) } else { None }))
}

/// Map transition: advance record invalidation epoch, clear entity books, and arm the sweep.
pub fn clear_for_map_transition() {
    MAP_EPOCH.with(|epoch| epoch.set(if epoch.get()==0 {0} else {epoch.get().checked_add(1).unwrap_or(0)}));
    LIVE.with(|t| { let mut t = t.borrow_mut(); t.clear(); mirror_all(&t); });
    crate::surface_leases::reset();
    crate::shared_entity_switch::reset();
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
        mirror_all(&t);
    });
    crate::surface_leases::prune_dead();
    crate::shared_entity_switch::prune_dead();
}

pub fn len() -> usize { LIVE.with(|t| t.borrow().len()) }

#[cfg(test)]
pub fn reset_for_tests() {
    LIVE.with(|t| { let mut t = t.borrow_mut(); t.clear(); mirror_all(&t); });
    crate::surface_leases::reset();
    crate::shared_entity_switch::reset();
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
    fn flat_books_track_every_write_and_out_of_range_indices_still_work() {
        fresh();
        let a = on_created(1, 10);
        let far = on_created(FLAT_LEN as i32 + 5, 3);           // outside the flat mirror
        assert_eq!(engine_serial_for(1, a), Some(10));
        assert_eq!(engine_serial_for(FLAT_LEN as i32 + 5, far), Some(3));
        on_spawned(1, 11);                                       // slot reused unseen: fresh id
        assert_eq!(engine_serial_for(1, a), None);
        let (b, _) = lookup(1).unwrap();
        assert_eq!(engine_serial_for(1, b), Some(11));
        on_deleted(1, 11);
        assert_eq!(engine_serial_for(1, b), None);
        let c = on_created(2, 5);
        repair_reconcile(&[(3, 9)]);                             // 2 evicted, 3 minted
        assert_eq!(engine_serial_for(2, c), None);
        let (d, _) = lookup(3).unwrap();
        assert_eq!(engine_serial_for(3, d), Some(9));
        clear_for_map_transition();
        assert_eq!(engine_serial_for(3, d), None);
        assert_eq!(engine_serial_for(-1, 1), None);
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
    fn borrowed_map_epoch_exhaustion_is_permanently_fail_closed() {
        let saved=map_epoch();
        struct Restore(u64);
        impl Drop for Restore { fn drop(&mut self) { MAP_EPOCH.with(|e|e.set(self.0));reset_for_tests(); } }
        let _restore=Restore(saved);
        MAP_EPOCH.with(|e|e.set(u64::MAX-1));
        clear_for_map_transition();assert_eq!(map_epoch(),u64::MAX);
        clear_for_map_transition();assert_eq!(map_epoch(),0);
        clear_for_map_transition();assert_eq!(map_epoch(),0);
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

/// Map invalidation only; this does not establish any native record lifetime.
pub(crate) fn map_epoch() -> u64 {MAP_EPOCH.with(Cell::get)}
