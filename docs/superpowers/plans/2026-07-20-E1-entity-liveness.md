# E1 Entity Liveness — Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: **superpowers:subagent-driven-development**. Execute tasks in the Parallelization-map order. Every task is strict TDD: write the failing test → run it and OBSERVE the failure → minimal implementation → run it green → commit. An executor sees ONLY its own task — the **Interfaces: Consumes/Produces** blocks are the contract between tasks; do not rename anything listed under Produces.

**Goal:** Make the host's books — not the entity's own (possibly freed) memory — the sole authority for entity liveness, so a stale `EntityRef` after a changelevel resolves to a deterministic `null` instead of a deterministic SEGV.

**Architecture:** A shared `core/src/liveness.rs` primitive (host-minted monotonic ids + keyed `is_live` tables) backs two separate instances: the existing plugin `Registry` (refactored, behavior-preserved) and a new entity books table `LIVE: index → (host-id, engine-serial)` fed unconditionally by the `IEntityListener` FFI entry and cleared at map start (the implicit epoch). `EntityRef` becomes `{index, id}` (id = host-minted u64, f64-safe on the JS wire as `{__s2ref:[index,id]}`); resolution is books-first, then a new shim op `ent_resolve(index, engine_serial)` validates in the system-owned identity CHUNK (the `s2_deref_handle` idiom) and returns the instance pointer — instance memory is only ever dereferenced last, and never to decide liveness.

**Tech Stack:** Rust (core cdylib, embedded V8 via the `v8` crate, in-isolate `#[test]` suite), C++ (Metamod shim, hl2sdk headers), JS (the injected std prelude inside `core/src/v8host.rs` + the `games/cs2/js/*.js` game package), TypeScript `.d.ts` (packages/sdk, packages/cs2), shell gate scripts.

**Authoritative design:** `docs/superpowers/specs/2026-07-20-safety-by-construction-north-star-design.md` §1–§3, §6, §8 (Candidate D locked). Read §3.1 (resolution order) and §3.4 (coverage matrix) before Task 8.

---

## Global Constraints

- **Repo root (work here):** `/home/gkh/projects/s2script/.claude/worktrees/rearch+north-star` — a git worktree on branch `rearch/north-star`. All paths below are relative to this root. Use absolute paths in tool calls.
- **Commits:** plain `git add` + `git commit` on the current branch, one commit per task (messages given per task). Do NOT push, do NOT `gt submit`, do NOT open PRs — the human cuts the Graphite stack from these commits afterwards (mapping in the Parallelization map).
- **Build & test commands (from CLAUDE.md — copy exactly):**
  ```bash
  cargo test -p s2script-core       # core unit + in-isolate suite. FORCED SINGLE-THREADED via
                                    # .cargo/config.toml — do NOT pass --test-threads.
  make core                         # cargo build --release (first run downloads ~130MB prebuilt V8)
  make shim                         # cmake build of the Metamod C++ shim (links core)
  make package                      # scripts/package-addon.sh → dist/addons/s2script
  ```
- **Gate suite (run per PR-boundary task, as marked):**
  ```bash
  make check-boundary                     # core must NOT import games/*
  ./scripts/check-plugins-typecheck.sh    # every plugin + example typechecks vs the shipped .d.ts
  ./scripts/check-schema-generated.sh
  ./scripts/check-nav-generated.sh
  ./scripts/check-events-generated.sh
  ./scripts/check-csitem-generated.sh
  ./scripts/test-boundary-nameleak.sh
  ```
- **Host builds are dev/CI only.** The only deployable binaries come from the sniper (Docker `rust:bullseye`) build — that is Task 12 (human). Never claim live behavior from a host build.
- **Single-threaded tests ⇒ thread_local state leaks between tests.** Every test touching the entity books MUST start by resetting them (`crate::entity_live::reset_for_tests()`).
- **S2EngineOps is an append-only C ABI.** New fields go AFTER `crash_test_native`, in the same order in `core/src/v8host.rs` AND `shim/include/s2script_core.h`. Never reorder existing fields.
- **Core is engine-generic.** No CS2 identifier (class/field/flag names) in `core/` or `shim/` op signatures. `EF_IN_STAGING_LIST`'s bit value stays in `games/cs2/js/pawn.js`.
- Do not touch `gamedata/`, do not run docker, do not deploy. Do not edit generated files (`*.generated.js`, `*.generated.d.ts`).
- Verified file/line anchors below are against `rearch/north-star` @ `90b166a` (code identical to `main` @ `427a2ae`). If a line has drifted, re-locate by the quoted code, not the number.

---

## Resolved decisions (2026-07-20 — these OVERRIDE the "Open questions for human review" section)

1. **Changeset bump = `minor`** (packages are 0.x, pre-beta, no external consumers of prior versions). NOT `major`; stay pre-1.0.
2. **`onDelete` books the delete AFTER the synchronous handler dispatch** (as planned): a handler may read the dying, slot-validated entity; the ref dies when the FFI entry returns.
3. **Repair-sweep ships default-ON** (as planned): armed per map-start, firing at the first simulating frame. The live gate (E0/V4) confirms whether it is load-bearing or a safety net; leave it in either way.
4. **REMOVE the public `EntityRef` constructor from the typed surface** (this SUPERSEDES Open Question 4's "keep it + future lint"). Fold into **Task 9**: the runtime prelude ctor stays (the framework mints refs), but `EntityRef` in the `packages/sdk` / `packages/cs2` `.d.ts` is declared WITHOUT a public constructor (an interface, or a class with a `private constructor`), so `new EntityRef(...)` does not typecheck in plugin code. Verify the prelude + `games/cs2` runtime still mint refs (they use the JS ctor, unaffected). Rationale: a hand-built ref is the "raw ref across time" footgun; nothing in-repo constructs refs outside the prelude/game package; make mis-minting unrepresentable.
5. **`ent_snapshot` CAP:** verify the constant against the SDK's `MAX_TOTAL_ENTITIES` during Task 5 review; keep fail-closed on overflow.

---

## Parallelization map

Numbers are tasks; `→` is a hard dependency (Consumes the upstream's Produces). **SPINE** = must be sequential. Tasks marked ∥ are logically independent, BUT all tasks editing `core/src/v8host.rs` share one file in one worktree — respect the "serialize" note when fanning out.

```
                 ┌─ Task 2 (plugin.rs refactor)          [∥ lane A — plugin.rs only]
Task 1 (liveness.rs)                                      [SPINE root]
                 └─ Task 3 (entity_live.rs books)         [SPINE]
                        └─ Task 4 (ffi.rs feed + clear)   [SPINE — ffi.rs only]

Task 5 (shim ops + ABI append + always-on listener)       [∥ lane B — shim/* + v8host.rs struct;
                                                           no dep on 1–4]
Task 6 (repair sweep)        deps: 3, 4, 5                [v8host.rs + ffi.rs]
Task 7 (minting natives)     deps: 3                      [v8host.rs]
Task 8 (atomic ref flip)     deps: 4, 5, 7                [v8host.rs — THE big one]
Task 9 (game JS + d.ts + plugins migration)  deps: 8      [games/, packages/, plugins/, examples/]
Task 10 (identity-flags isValid)             deps: 5, 8, 9
Task 11 (docs)                               deps: 10
Task 12 (LIVE GATE — human)                  deps: 9, 10 (11 recommended)
```

**File-contention serialization for the Workflow:** run `{2} ∥ {3→4} ∥ {5}` after 1; then `6 → 7 → 8` strictly sequentially (all touch `v8host.rs`); then `9 → 10 → 11 → 12`.

**Graphite stack mapping (human, after all tasks green):**
| PR | Branch suggestion | Tasks | Gate to run per PR |
|---|---|---|---|
| 1 | `entity-liveness/books` | 1, 2, 3, 4 | `cargo test -p s2script-core` + full gate suite |
| 2 | `entity-liveness/slot-ops` | 5, 6 | + `make shim` |
| 3 | `entity-liveness/ref-id` | 7, 8, 9 | + `./scripts/check-plugins-typecheck.sh` |
| 4 | `entity-liveness/identity-reads` | 10, 11 | full gate suite |
| 5 | `entity-liveness/live-gate` | 12 | live Docker CS2 gate (human) |

---

### Task 1: `core/src/liveness.rs` — the shared LiveTable primitive

**Files:**
- Create: `core/src/liveness.rs`
- Modify: `core/src/lib.rs` (add `pub(crate) mod liveness;` after line 10, `pub(crate) mod entity;`)

**Interfaces:**
- **Consumes:** nothing (pure logic, no V8, no engine).
- **Produces (Tasks 2, 3 depend on these exact signatures):**
  ```rust
  pub struct LiveTable<K: Eq + std::hash::Hash, M> { /* private */ }
  impl<K: Eq + std::hash::Hash, M> LiveTable<K, M> {
      pub fn new(first_id: u64) -> Self;
      pub fn insert(&mut self, key: K, meta: M) -> u64;      // mints; replaces existing entry
      pub fn remove(&mut self, key: &K) -> Option<(u64, M)>;
      pub fn is_live(&self, key: &K, id: u64) -> bool;
      pub fn get(&self, key: &K) -> Option<(u64, &M)>;
      pub fn get_mut(&mut self, key: &K) -> Option<(u64, &mut M)>;
      pub fn keys(&self) -> Vec<K> where K: Clone;
      pub fn clear(&mut self);                               // entries only; the allocator NEVER resets
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
  }
  ```

**Steps:**

1. **Write the failing test.** Create `core/src/liveness.rs` containing ONLY the doc header and the test module (no impl yet):

   ```rust
   //! The shared liveness primitive: host-minted monotonic ids + keyed `is_live` tables.
   //! Doctrine (north-star §1): liveness is decided by the HOST'S BOOKS — populated by
   //! notifications, cleared by transitions — never by reading the resource's own memory.
   //! Two instances share this module and stay SEPARATE tables: plugins (`plugin::Registry`)
   //! and entities (`entity_live`). A plugin reload must not invalidate entities; a map
   //! change must not invalidate plugins.

   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn mint_is_monotonic_and_replace_invalidates_the_old_id() {
           let mut t: LiveTable<i32, &str> = LiveTable::new(1);
           let a = t.insert(7, "first");
           assert_eq!(a, 1, "first_id honored");
           assert!(t.is_live(&7, a));
           let b = t.insert(7, "second");           // same key: replace = invalidation
           assert!(b > a, "ids are strictly monotonic");
           assert!(!t.is_live(&7, a), "the replaced holder's id can never match again");
           assert!(t.is_live(&7, b));
           assert_eq!(t.get(&7), Some((b, &"second")));
       }

       #[test]
       fn clear_drops_entries_but_never_resets_the_allocator() {
           let mut t: LiveTable<i32, ()> = LiveTable::new(1);
           let a = t.insert(1, ());
           t.clear();
           assert!(t.is_empty());
           assert!(!t.is_live(&1, a), "cleared entry is dead");
           let b = t.insert(1, ());
           assert!(b > a, "an id from before a clear can NEVER alias one minted after");
       }

       #[test]
       fn remove_get_mut_keys_len() {
           let mut t: LiveTable<String, i32> = LiveTable::new(0);
           let g0 = t.insert("a".into(), 10);
           assert_eq!(g0, 0, "plugin-compat: first_id 0 mints 0");
           t.insert("b".into(), 20);
           assert_eq!(t.len(), 2);
           if let Some((_, m)) = t.get_mut(&"a".to_string()) { *m = 11; }
           assert_eq!(t.get(&"a".to_string()).map(|(_, m)| *m), Some(11));
           let mut ks = t.keys(); ks.sort();
           assert_eq!(ks, vec!["a".to_string(), "b".to_string()]);
           let (gid, meta) = t.remove(&"a".to_string()).unwrap();
           assert_eq!((gid, meta), (g0, 11));
           assert!(t.remove(&"a".to_string()).is_none());
       }
   }
   ```
   Add `pub(crate) mod liveness;` to `core/src/lib.rs`.

2. **Run it — must fail to compile:**
   ```bash
   cargo test -p s2script-core liveness
   ```
   Expected: `error[E0412]: cannot find type 'LiveTable' in this scope` (or E0433). This is the observed failure.

3. **Minimal impl** — insert above the test module:

   ```rust
   use std::collections::HashMap;
   use std::hash::Hash;

   /// A host-owned liveness table: `key → (host-minted id, meta)`. The id allocator is
   /// monotonic for the table's lifetime and NEVER resets (not on remove, not on clear) —
   /// that monotonicity IS the anti-aliasing guarantee.
   pub struct LiveTable<K: Eq + Hash, M> {
       entries: HashMap<K, (u64, M)>,
       next_id: u64,
   }

   impl<K: Eq + Hash, M> LiveTable<K, M> {
       /// `first_id`: plugins use 0 (exact `Registry` behavior today); entities use 1 so
       /// id 0 is a never-live sentinel on the JS wire.
       pub fn new(first_id: u64) -> Self {
           Self { entries: HashMap::new(), next_id: first_id }
       }
       /// Mint a fresh id for `key`, replacing any existing entry. Replacement IS the
       /// invalidation of the previous holder — its captured id can never match again.
       pub fn insert(&mut self, key: K, meta: M) -> u64 {
           let id = self.next_id;
           self.next_id += 1;
           self.entries.insert(key, (id, meta));
           id
       }
       pub fn remove(&mut self, key: &K) -> Option<(u64, M)> { self.entries.remove(key) }
       pub fn is_live(&self, key: &K, id: u64) -> bool {
           self.entries.get(key).map_or(false, |(cur, _)| *cur == id)
       }
       pub fn get(&self, key: &K) -> Option<(u64, &M)> {
           self.entries.get(key).map(|(id, m)| (*id, m))
       }
       pub fn get_mut(&mut self, key: &K) -> Option<(u64, &mut M)> {
           self.entries.get_mut(key).map(|(id, m)| (*id, m))
       }
       pub fn keys(&self) -> Vec<K> where K: Clone { self.entries.keys().cloned().collect() }
       /// Drop every entry. The allocator NEVER resets.
       pub fn clear(&mut self) { self.entries.clear(); }
       pub fn len(&self) -> usize { self.entries.len() }
       pub fn is_empty(&self) -> bool { self.entries.is_empty() }
   }
   ```

4. **Run it green:** `cargo test -p s2script-core liveness` → 3 passed. Then the full crate: `cargo test -p s2script-core` → all green.

5. **Commit:**
   ```bash
   git add core/src/liveness.rs core/src/lib.rs
   git commit -m "feat(core): liveness.rs — shared monotonic-id LiveTable primitive (E1)"
   ```

---

### Task 2: refactor `plugin::Registry` onto `LiveTable` (behavior-preserving)

**Files:**
- Modify: `core/src/plugin.rs` (the `Registry` struct + impl, lines 156-220; `PluginEntry` at 144-150 and `PluginLedger` stay UNCHANGED)

**Interfaces:**
- **Consumes (Task 1):** `crate::liveness::LiveTable` — `new(0)`, `insert`, `remove`, `is_live`, `get`, `get_mut`, `keys`.
- **Produces:** `Registry`'s public API is UNCHANGED (`new/insert/remove/is_live/ledger_mut/ids/generation_of` with identical signatures and identical semantics — first generation minted is 0, shared counter across ids). Nothing downstream changes.

**Steps:**

1. **Write the failing test** — append to `plugin.rs`'s test module (proves the shared-counter semantics the refactor must preserve; passes today, must STILL pass after):
   ```rust
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
   ```
   Run `cargo test -p s2script-core plugin` — it must PASS against the old impl (this is the parity pin). Now do the refactor and keep it green.

2. **Replace the `Registry` internals** (delete the `entries: HashMap<String, PluginEntry>` + `next_gen` fields and the old method bodies):
   ```rust
   /// Maps plugin id strings to their current entry. Backed by the shared liveness
   /// primitive (E1): one instance of the SAME mechanism the entity books use —
   /// separate table, separate axis (a map change must never invalidate plugins).
   pub struct Registry {
       table: crate::liveness::LiveTable<String, PluginLedger>,
   }

   impl Registry {
       pub fn new() -> Self { Self { table: crate::liveness::LiveTable::new(0) } }

       /// Insert (or re-insert on reload) a plugin. Returns the assigned generation.
       /// A re-insert of an existing id mints a fresh generation — that IS reload.
       pub fn insert(&mut self, id: impl Into<String>) -> u64 {
           self.table.insert(id.into(), PluginLedger::new())
       }
       pub fn remove(&mut self, id: &str) -> Option<PluginEntry> {
           self.table.remove(&id.to_string())
               .map(|(generation, ledger)| PluginEntry { generation, ledger })
       }
       pub fn is_live(&self, id: &str, generation: u64) -> bool {
           self.table.is_live(&id.to_string(), generation)
       }
       pub fn ledger_mut(&mut self, id: &str) -> Option<&mut PluginLedger> {
           self.table.get_mut(&id.to_string()).map(|(_, m)| m)
       }
       pub fn ids(&self) -> Vec<String> { self.table.keys() }
       pub fn generation_of(&self, id: &str) -> Option<u64> {
           self.table.get(&id.to_string()).map(|(g, _)| g)
       }
   }
   ```
   (Keep `impl Default for Registry` as-is.)

3. **Run green:** `cargo test -p s2script-core` — the ENTIRE suite (including all v8host in-isolate tests that exercise reload liveness) must pass untouched. Any failure = a behavior change = fix the refactor, never the test.

4. **Commit:**
   ```bash
   git add core/src/plugin.rs
   git commit -m "refactor(core): plugin::Registry onto liveness::LiveTable (behavior-preserving)"
   ```

---

### Task 3: `core/src/entity_live.rs` — the entity books

**Files:**
- Create: `core/src/entity_live.rs`
- Modify: `core/src/lib.rs` (add `pub(crate) mod entity_live;` after the `liveness` line)

**Interfaces:**
- **Consumes (Task 1):** `crate::liveness::LiveTable::new(1)` + its methods.
- **Produces (Tasks 4, 6, 7, 8, 10 depend on these EXACT names/signatures):**
  ```rust
  pub fn on_created(index: i32, engine_serial: i32) -> u64;
  pub fn on_spawned(index: i32, engine_serial: i32);
  pub fn on_deleted(index: i32, engine_serial: i32);
  pub fn lookup(index: i32) -> Option<(u64, i32)>;              // (host-id, engine_serial)
  pub fn adopt(index: i32, engine_serial: i32) -> Option<u64>;  // serial match → the table's id
  pub fn engine_serial_for(index: i32, id: u64) -> Option<i32>; // id match → stored engine serial
  pub fn clear_for_map_transition();                            // clear + arm the repair sweep
  pub fn take_repair_armed() -> bool;
  pub fn repair_reconcile(live_slots: &[(i32, i32)]);
  pub fn len() -> usize;
  #[cfg(test)] pub fn reset_for_tests();
  ```

**Steps:**

1. **Write the failing tests.** Create the file with header + tests only:

   ```rust
   //! The entity books — the ONLY liveness authority for entities (north-star §3.1,
   //! Candidate D). `LIVE: index → (host-id, engine_serial)`, fed by the shim's
   //! IEntityListener through the ffi entry (UNCONDITIONALLY — before/independent of the
   //! JS mux dispatch), cleared at map start (the implicit epoch — no counter to stamp).
   //! Engine memory is NEVER read to answer "is this entity alive". Host ids are u64,
   //! monotonic, never reset across maps; JS-safe as f64 up to 2^53 mints. Game-thread
   //! only (thread_local), like every other v8host-adjacent table.

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
   ```
   Add the `lib.rs` line.

2. **Run it fails:** `cargo test -p s2script-core entity_live` → `error[E0425]: cannot find function 'on_created'` etc.

3. **Minimal impl** (above the tests):

   ```rust
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
   ```

4. **Run green:** `cargo test -p s2script-core entity_live` → 5 passed; then full `cargo test -p s2script-core`.

5. **Commit:**
   ```bash
   git add core/src/entity_live.rs core/src/lib.rs
   git commit -m "feat(core): entity_live books — index → (host-id, engine-serial), listener-fed"
   ```

---

### Task 4: unconditional books feed + map-start epoch clear in `core/src/ffi.rs`

**Files:**
- Modify: `core/src/ffi.rs` — `s2script_core_dispatch_entity_event` (lines 133-141) and `s2script_core_dispatch_map_start` (lines 108-116) + its own `#[cfg(test)] mod tests`.

**Interfaces:**
- **Consumes (Task 3):** `entity_live::{on_created, on_spawned, on_deleted, clear_for_map_transition, lookup, take_repair_armed}`; `crate::entity::{decode_handle, HANDLE_ENTRY_BITS}` (already `pub`).
- **Produces:** the FFI entries now maintain the books UNCONDITIONALLY. Contract for Task 8's dispatch code and tests: **create/spawn are booked BEFORE the JS dispatch; delete is booked (removed) AFTER it** — an `onDelete` handler can still resolve the dying entity within its synchronous dispatch; the entry is gone the moment the FFI call returns.

**Steps:**

1. **Write the failing tests** — append to `ffi.rs`'s existing test module:

   ```rust
   /// E1: the books feed lives in THIS ffi entry, unconditionally — with ZERO JS
   /// subscribers (dispatch_entity_event early-returns) the books must still update.
   #[test]
   fn entity_event_feed_updates_books_with_no_subscribers() {
       crate::entity_live::reset_for_tests();
       assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
       let create = std::ffi::CString::new("create").unwrap();
       let spawn  = std::ffi::CString::new("spawn").unwrap();
       let delete = std::ffi::CString::new("delete").unwrap();
       let cls    = std::ffi::CString::new("prop_physics").unwrap();
       let handle = ((7u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32; // (index 42, serial 7)

       s2script_core_dispatch_entity_event(create.as_ptr(), cls.as_ptr(), handle as c_int);
       let (id, ser) = crate::entity_live::lookup(42).expect("create fed the books");
       assert!(id >= 1); assert_eq!(ser, 7);

       s2script_core_dispatch_entity_event(spawn.as_ptr(), cls.as_ptr(), handle as c_int);
       assert_eq!(crate::entity_live::lookup(42), Some((id, 7)), "matching spawn keeps the id");

       // a stale delete (wrong serial) must NOT evict:
       let stale = ((9u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32;
       s2script_core_dispatch_entity_event(delete.as_ptr(), cls.as_ptr(), stale as c_int);
       assert!(crate::entity_live::lookup(42).is_some());

       s2script_core_dispatch_entity_event(delete.as_ptr(), cls.as_ptr(), handle as c_int);
       assert_eq!(crate::entity_live::lookup(42), None, "matching delete removed the entry");

       // the -1 no-entity sentinel must not touch the books (and must not panic):
       s2script_core_dispatch_entity_event(create.as_ptr(), cls.as_ptr(), -1);
       assert_eq!(crate::entity_live::len(), 0);
       s2script_core_shutdown();
   }

   /// E1: map start clears the whole table (the implicit epoch) + arms the repair sweep,
   /// unconditionally — before/independent of the Server.onMapStart JS dispatch.
   #[test]
   fn map_start_clears_books_and_arms_repair_sweep() {
       crate::entity_live::reset_for_tests();
       assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
       crate::entity_live::on_created(3, 5);
       let map = std::ffi::CString::new("de_vertigo").unwrap();
       s2script_core_dispatch_map_start(map.as_ptr());
       assert_eq!(crate::entity_live::len(), 0, "the epoch: books cleared at map start");
       assert!(crate::entity_live::take_repair_armed(), "map start arms the repair sweep");
       s2script_core_shutdown();
   }
   ```

2. **Run it fails:** `cargo test -p s2script-core ffi::tests::entity_event_feed` → assertion failure `create fed the books` (the feed doesn't exist yet).

3. **Minimal impl.** Replace the body of `s2script_core_dispatch_entity_event`:

   ```rust
   #[no_mangle]
   pub extern "C" fn s2script_core_dispatch_entity_event(kind: *const c_char, class_name: *const c_char, handle: c_int) {
       let _ = catch_unwind(|| {
           if kind.is_null() || class_name.is_null() { return; }
           let Ok(kind_str) = (unsafe { CStr::from_ptr(kind) }).to_str() else { return };
           let Ok(class_str) = (unsafe { CStr::from_ptr(class_name) }).to_str() else { return };
           // THE BOOKS FEED (north-star §3.1, critical): unconditional, BEFORE and
           // independent of the JS mux dispatch below — dispatch_entity_event early-returns
           // when no subscribers exist and skips under the HOST try_borrow_mut re-entrancy
           // guard, but a create/delete witnessed while JS is on-stack must still update
           // the books (e.g. a plugin's own synchronous createEntity).
           let decoded = if handle == -1 { None } else {
               let (idx, ser) = crate::entity::decode_handle(handle as u32);
               if idx >= 0 && ser >= 0 { Some((idx, ser)) } else { None }
           };
           if let Some((idx, ser)) = decoded {
               match kind_str {
                   "create" => { crate::entity_live::on_created(idx, ser); }
                   "spawn"  => { crate::entity_live::on_spawned(idx, ser); }
                   _ => {}
               }
           }
           v8host::dispatch_entity_event(kind_str, class_str, handle as i32);
           // Delete is booked AFTER the dispatch: an onDelete handler may still resolve
           // the dying entity (slot-validated stage 2 stays the guard); the moment this
           // FFI entry returns, the books say dead — fail-closed for any stashed ref.
           if let Some((idx, ser)) = decoded {
               if kind_str == "delete" { crate::entity_live::on_deleted(idx, ser); }
           }
       });
   }
   ```

   And in `s2script_core_dispatch_map_start`, insert immediately before `v8host::dispatch_map_start(map_str);`:
   ```rust
           // E1: the implicit entity epoch — clear the books UNCONDITIONALLY before the JS
           // dispatch (which early-returns when no Server.onMapStart subscribers exist),
           // and arm the one-shot repair sweep (consumed at the next simulating frame).
           crate::entity_live::clear_for_map_transition();
   ```

4. **Run green:** `cargo test -p s2script-core` (full suite — the v8host entity-event tests at v8host.rs:11596/11616 pass unchanged: they call `v8host::dispatch_entity_event` directly, below this feed).

5. **Commit:**
   ```bash
   git add core/src/ffi.rs
   git commit -m "feat(core): unconditional entity-event books feed + map-start epoch clear in ffi"
   ```

---

### Task 5: shim slot-side ops (`ent_resolve` / `ent_identity_flags` / `ent_snapshot`) + always-on listener

**Files:**
- Modify: `shim/src/s2script_mm.cpp` — new ops next to `s2_entity_name` (after line 363); ops-table wiring after `ops.crash_test_native = ...` (line 4211); always-on listener at the `AddListenerEntity` resolve site (lines 3944-3954).
- Modify: `shim/include/s2script_core.h` — typedefs near line 303, struct members before `} S2EngineOps;` (line 467).
- Modify: `core/src/v8host.rs` — fn-pointer types after `CrashTestNativeFn` (line 276), struct fields after `crash_test_native` (line 424), and EVERY exhaustive `S2EngineOps { ... }` literal in the test module (find them: `grep -n "S2EngineOps {" core/src/v8host.rs` → the constructors at ~11470, ~11949, ~12330 — add the three new fields as `None`; literals using `..mock_event_ops()` need no change).

**Interfaces:**
- **Consumes:** nothing from Tasks 1-4 (pure ABI addition; parallel lane).
- **Produces (Tasks 6, 8, 10 depend on these EXACT names):**
  ```rust
  // core/src/v8host.rs
  pub type EntResolveFn       = extern "C" fn(c_int, c_int) -> *mut c_void; // (index, engine_serial) -> CEntityInstance* | null
  pub type EntIdentityFlagsFn = extern "C" fn(c_int, c_int) -> i64;         // -> m_flags (>=0) | -1 stale/absent
  pub type EntSnapshotFn      = extern "C" fn(*mut c_int, *mut c_int, c_int) -> c_int; // fills live (index,serial); returns total
  // S2EngineOps fields (in order): ent_resolve, ent_identity_flags, ent_snapshot
  ```
  Plus: the shim's `IEntityListener` is now registered UNCONDITIONALLY (the books are load-bearing), not only after a JS `Entity.on*` subscribe.

**Steps:**

1. **Failing check first (core side).** Add the three types + fields to `core/src/v8host.rs`:
   ```rust
   // --- E1 entity-liveness slice (APPENDED after crash_test_native; order is the ABI). ENGINE-GENERIC:
   // slot-side identity-CHUNK validation — (index, engine_serial) in; instance ptr / identity flags /
   // a live-(index,serial) snapshot out. No game names cross the ABI. `ent_resolve` is the ONLY
   // pointer-yielding resolver core may use after E1 (the s2_deref_handle idiom: liveness decided in
   // system-owned chunk memory, never through the instance).
   pub type EntResolveFn       = extern "C" fn(c_int, c_int) -> *mut c_void;
   pub type EntIdentityFlagsFn = extern "C" fn(c_int, c_int) -> i64;
   pub type EntSnapshotFn      = extern "C" fn(*mut c_int, *mut c_int, c_int) -> c_int;
   ```
   ```rust
       // --- E1 entity-liveness slice (APPENDED after crash_test_native; order is the ABI; do not reorder above) ---
       pub ent_resolve:        Option<EntResolveFn>,
       pub ent_identity_flags: Option<EntIdentityFlagsFn>,
       pub ent_snapshot:       Option<EntSnapshotFn>,
   ```
   Run `cargo test -p s2script-core` → **expected failure:** `error[E0063]: missing fields ent_resolve, ent_identity_flags, ent_snapshot` at every exhaustive test literal. That is the observed failure; fix each listed literal by appending `ent_resolve: None, ent_identity_flags: None, ent_snapshot: None,`. Re-run → green.

2. **C header mirror** (`shim/include/s2script_core.h`). Typedefs near the other fn typedefs (~line 303):
   ```c
   /* E1 entity-liveness slice — slot-side identity-chunk validation (engine-generic). */
   typedef void*     (*s2_ent_resolve_fn)(int index, int serial);
   typedef long long (*s2_ent_identity_flags_fn)(int index, int serial);
   typedef int       (*s2_ent_snapshot_fn)(int* out_indices, int* out_serials, int cap);
   ```
   Struct members immediately before `} S2EngineOps;` (after `crash_test_native`):
   ```c
       /* E1 entity-liveness slice — MUST remain in this order; mirrors S2EngineOps in core/src/v8host.rs */
       s2_ent_resolve_fn        ent_resolve;        /* (index, engine_serial) -> CEntityInstance* | NULL — identity-CHUNK validated */
       s2_ent_identity_flags_fn ent_identity_flags; /* (index, engine_serial) -> m_flags (>=0) | -1 stale/absent */
       s2_ent_snapshot_fn       ent_snapshot;       /* fill live (index, serial) pairs; returns TOTAL found (may exceed cap) */
   ```

3. **Shim impls** — insert after `s2_entity_name` (s2script_mm.cpp, after line 363; same chunk-walk idiom, mirrors `s2_deref_handle`):
   ```cpp
   // ---------------------------------------------------------------------------
   // E1 engine-op: resolve (index, engine_serial) -> CEntityInstance*, validating ENTIRELY
   // in the system-owned identity chunk (the s2_deref_handle idiom, by pair instead of by
   // packed handle). Instance memory is NEVER read to decide liveness — the exact inversion
   // of the retired core-side entity_resolve_ptr (which read the serial through the
   // instance it was about to return: a use-after-free deciding UAF-safety).
   // ---------------------------------------------------------------------------
   static void* s2_ent_resolve(int index, int serial) {
       CGameEntitySystem* es = GetEntitySystem();
       if (!es) return nullptr;
       if (index < 0 || index >= MAX_TOTAL_ENTITIES) return nullptr;
       CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[index / MAX_ENTITIES_IN_LIST];
       if (!chunk_base) return nullptr;
       CEntityIdentity* id = &chunk_base[index % MAX_ENTITIES_IN_LIST];
       if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;
       if (id->GetRefEHandle().GetSerialNumber() != serial) return nullptr;  // stale slot reuse
       return id->m_pInstance;   // may be null (removal in progress) — caller treats null as not-live
   }

   // E1 engine-op: identity m_flags read from the SLOT (never instance+0x10). -1 = stale/absent.
   // Backs pawn.isValid's EF_IN_STAGING_LIST check without touching instance memory; the flag's
   // bit value stays in the game package (engine-generic: raw flags cross the ABI).
   static long long s2_ent_identity_flags(int index, int serial) {
       CGameEntitySystem* es = GetEntitySystem();
       if (!es) return -1;
       if (index < 0 || index >= MAX_TOTAL_ENTITIES) return -1;
       CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[index / MAX_ENTITIES_IN_LIST];
       if (!chunk_base) return -1;
       CEntityIdentity* id = &chunk_base[index % MAX_ENTITIES_IN_LIST];
       if (id->m_flags & EF_IS_INVALID_EHANDLE) return -1;
       if (id->GetRefEHandle().GetSerialNumber() != serial) return -1;
       return (long long)(unsigned int)id->m_flags;
   }

   // E1 engine-op: books repair sweep — every live identity slot's (index, serial); the
   // s2_entity_find_by_class walk minus the class filter. Returns the TOTAL found (the
   // caller detects truncation when total > cap). System-owned chunk memory only.
   static int s2_ent_snapshot(int* outIndices, int* outSerials, int cap) {
       if (!outIndices || !outSerials || cap <= 0) return 0;
       CGameEntitySystem* es = GetEntitySystem();
       if (!es) return 0;
       int found = 0;
       for (int idx = 0; idx < MAX_TOTAL_ENTITIES; ++idx) {
           CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[idx / MAX_ENTITIES_IN_LIST];
           if (!chunk_base) continue;
           CEntityIdentity* id = &chunk_base[idx % MAX_ENTITIES_IN_LIST];
           if (id->m_flags & EF_IS_INVALID_EHANDLE) continue;
           if (!id->m_pInstance) continue;
           if (found < cap) {
               CEntityHandle h = id->GetRefEHandle();
               outIndices[found] = h.GetEntryIndex();
               outSerials[found] = h.GetSerialNumber();
           }
           ++found;
       }
       return found;
   }
   ```
   Wire them in `Load()` after `ops.crash_test_native = &s2_crash_test_native;` (line 4211):
   ```cpp
       // E1 entity-liveness slice (APPENDED after crash_test_native; order is the ABI).
       ops.ent_resolve        = &s2_ent_resolve;
       ops.ent_identity_flags = &s2_ent_identity_flags;
       ops.ent_snapshot       = &s2_ent_snapshot;
   ```

4. **Always-on listener.** At the `AddListenerEntity` sig-resolve success branch (s2script_mm.cpp ~line 3951, right after `s_pAddListenerEntity = ...` is set), add:
   ```cpp
                   // E1: the entity books are load-bearing for ALL entity access now — the
                   // listener is wanted from boot, not only after a JS Entity.on* subscribe.
                   // (Registration still happens at StartupServer POST via
                   // EnsureEntityListenerRegistered — the entity system doesn't exist yet here.)
                   s_wantEntityListener = true;
   ```
   Leave `Shim_EntityListenerInstall` (line 2928) unchanged — it stays a harmless idempotent re-assert.

5. **Verify builds:**
   ```bash
   cargo test -p s2script-core     # green (fields added everywhere)
   make shim                       # C++ compiles + links
   ```

6. **Commit:**
   ```bash
   git add shim/src/s2script_mm.cpp shim/include/s2script_core.h core/src/v8host.rs
   git commit -m "feat(shim+core): slot-side ent_resolve/ent_identity_flags/ent_snapshot ops + always-on entity listener"
   ```

---

### Task 6: repair-sweep executor (books reconcile at the first simulating frame)

**Files:**
- Modify: `core/src/v8host.rs` — new `pub(crate) fn entity_repair_sweep_if_armed(simulating: bool)` (place it next to `dispatch_map_start`, ~line 4126) + a test.
- Modify: `core/src/ffi.rs` — call it in `s2script_core_dispatch_game_frame` (line 56-69).

**Interfaces:**
- **Consumes:** Task 3 (`entity_live::{take_repair_armed, repair_reconcile, clear_for_map_transition, lookup}`), Task 5 (`ops.ent_snapshot`), Task 4 (arming happens in the map-start ffi entry).
- **Produces:** `pub(crate) fn v8host::entity_repair_sweep_if_armed(simulating: bool)` — called once per Pre-phase frame dispatch; consumes the armed flag only on a simulating frame; no-ops (flag consumed, books untouched) when the op is absent.

**Steps:**

1. **Failing test** (v8host.rs test module — near the entity-listener tests ~line 11555):
   ```rust
   thread_local! { static SNAPSHOT_PAIRS: std::cell::RefCell<Vec<(i32, i32)>> = std::cell::RefCell::new(Vec::new()); }
   extern "C" fn fake_ent_snapshot(oi: *mut c_int, os: *mut c_int, cap: c_int) -> c_int {
       SNAPSHOT_PAIRS.with(|p| {
           let p = p.borrow();
           let n = p.len().min(cap as usize);
           for i in 0..n { unsafe { *oi.add(i) = p[i].0; *os.add(i) = p[i].1; } }
           p.len() as c_int
       })
   }

   /// E1: the map-start-armed repair sweep reconciles the books from the identity-chunk
   /// snapshot at the FIRST SIMULATING frame — and only then (a non-simulating frame
   /// leaves it armed).
   #[test]
   fn repair_sweep_runs_once_on_first_simulating_frame() {
       crate::entity_live::reset_for_tests();
       let _ = init(dummy_logger());
       set_engine_ops(Some(S2EngineOps { ent_snapshot: Some(fake_ent_snapshot), ..mock_event_ops() }));
       SNAPSHOT_PAIRS.with(|p| *p.borrow_mut() = vec![(1, 11), (64, 3)]);
       crate::entity_live::clear_for_map_transition();          // arm (what map start does)
       entity_repair_sweep_if_armed(false);                     // NOT simulating → stays armed
       assert_eq!(crate::entity_live::len(), 0);
       entity_repair_sweep_if_armed(true);                      // simulating → reconcile
       assert!(crate::entity_live::lookup(1).is_some() && crate::entity_live::lookup(64).is_some());
       SNAPSHOT_PAIRS.with(|p| p.borrow_mut().clear());
       entity_repair_sweep_if_armed(true);                      // disarmed → no second sweep
       assert_eq!(crate::entity_live::len(), 2, "sweep is one-shot per arming");
       shutdown();
   }
   ```
   Run: `cargo test -p s2script-core repair_sweep` → `error[E0425]: cannot find function entity_repair_sweep_if_armed`.

2. **Impl** (v8host.rs, after `dispatch_map_start`):
   ```rust
   /// E1 repair sweep (north-star §7, the E0-V4 contingency): armed by the map-start books
   /// clear, runs ONCE at the next SIMULATING frame — reconciles the books against a
   /// chunk-walk snapshot of live identity slots (system-owned memory only; the shim's
   /// ent_snapshot op). Covers entities created before StartupServer POST / before the
   /// listener attached (first boot map, preallocated controllers). Fail-closed: with no
   /// op the books stay purely listener-fed and an unseen entity reads null.
   /// ASSUMPTION TO CONFIRM AT THE LIVE GATE (E0-V4): the first simulating frame is a
   /// verified-clean moment — the new map's entity system is live and populated.
   pub(crate) fn entity_repair_sweep_if_armed(simulating: bool) {
       if !simulating { return; }
       if !crate::entity_live::take_repair_armed() { return; }
       let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
       let Some(snapshot) = ops.ent_snapshot else { return };
       const CAP: usize = 32768;   // MAX_TOTAL_ENTITIES ceiling (CS2: 16k entries + headroom)
       let mut idxs = vec![0i32; CAP];
       let mut sers = vec![0i32; CAP];
       let n = snapshot(idxs.as_mut_ptr(), sers.as_mut_ptr(), CAP as i32);
       let n = (n.max(0) as usize).min(CAP);
       let pairs: Vec<(i32, i32)> = (0..n).map(|i| (idxs[i], sers[i])).collect();
       crate::entity_live::repair_reconcile(&pairs);
   }
   ```
   In `ffi.rs::s2script_core_dispatch_game_frame`, immediately BEFORE `let out = v8host::dispatch_onframe(...)`:
   ```rust
           // E1: reconcile the entity books before any JS runs this frame (one-shot,
           // armed at map start, fires on the first simulating frame).
           if phase == Phase::Pre { v8host::entity_repair_sweep_if_armed(simulating != 0); }
   ```

3. **Run green:** `cargo test -p s2script-core` (full).

4. **Commit:**
   ```bash
   git add core/src/v8host.rs core/src/ffi.rs
   git commit -m "feat(core): map-start repair sweep — books reconcile from identity-chunk snapshot"
   ```

---

### Task 7: minting natives — `__s2_ent_id_for_index` + `__s2_handle_adopt` (+ `js_ent_id`)

**Files:**
- Modify: `core/src/v8host.rs` — new helper + two natives next to `s2_handle_decode` (line 3789); register both in the `set_native` block (after line 7469, `__s2_handle_decode`); tests.

**Interfaces:**
- **Consumes:** Task 3 (`entity_live::{lookup, adopt}`), `crate::entity::decode_handle`.
- **Produces (Tasks 8, 9, 10 depend on these EXACT names):**
  - Rust helper: `fn js_ent_id(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> u64` — 0 = invalid/never-live.
  - JS native `__s2_ent_id_for_index(index) -> number` — the books id for a slot-derived index; `0` when not live. THE index-minting path.
  - JS native `__s2_handle_adopt(handleU32) -> [index, id] | null` — decode (pure bit-math) → books adoption. THE raw-handle minting path. **After E1, no JS code may construct a live ref from `__s2_handle_decode` output.**

**Steps:**

1. **Failing test** (v8host.rs test module):
   ```rust
   /// E1: the two minting natives — index-minting via the books id, handle-minting via
   /// adoption. A dangling/mismatched handle can never mint; an absent index mints 0.
   #[test]
   fn minting_natives_are_books_backed() {
       crate::entity_live::reset_for_tests();
       let _ = init(dummy_logger());
       set_engine_ops(None);                                   // books-only paths: no ops needed
       let id = crate::entity_live::on_created(42, 7);
       create_plugin_context("mint");
       assert_eq!(eval_in_context_string("mint", "String(__s2_ent_id_for_index(42))"), id.to_string());
       assert_eq!(eval_in_context_string("mint", "String(__s2_ent_id_for_index(43))"), "0");
       let good = ((7u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32;
       let stale = ((9u32) << crate::entity::HANDLE_ENTRY_BITS) | 42u32;
       assert_eq!(
           eval_in_context_string("mint", &format!("var a=__s2_handle_adopt({good}); a ? a[0]+','+a[1] : 'null'")),
           format!("42,{id}"));
       assert_eq!(
           eval_in_context_string("mint", &format!("String(__s2_handle_adopt({stale}))")),
           "null", "a stale handle field can never mint a live ref");
       shutdown();
   }
   ```
   Run: fails with a JS `ReferenceError: __s2_ent_id_for_index is not defined` surfaced by `eval_in_context_string` (or its error-string convention — observe it).

2. **Impl** (next to `s2_handle_decode`):
   ```rust
   /// Parse a JS EntityRef id (f64 on the wire; host-minted u64). 0 = invalid/never-live.
   /// Integral, ≥1, ≤2^53 (exact-f64 range) — anything else fails closed.
   fn js_ent_id(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> u64 {
       let n = v.number_value(scope).unwrap_or(0.0);
       if !n.is_finite() || n < 1.0 || n > 9_007_199_254_740_992.0 || n.fract() != 0.0 { return 0; }
       n as u64
   }

   /// Native `__s2_ent_id_for_index(index) -> number`. The books id for a slot-derived
   /// index (Player.fromSlot / Pawn.forSlot), or 0 when the books say not-live. Books
   /// only — no engine memory. Replaces the retired `__s2_ent_current_serial` idiom.
   fn s2_ent_id_for_index(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
       let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
           rv.set_double(0.0);
           let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
           if let Some((id, _)) = crate::entity_live::lookup(index) { rv.set_double(id as f64); }
       }));
   }

   /// Native `__s2_handle_adopt(handleU32) -> [index, id] | null`. THE raw-handle minting
   /// path: decode (pure bit-math) then adopt from the books — engine-serial match yields
   /// the table's host id; mismatch/absent → null. A dangling handle field can never mint
   /// a live ref (north-star §3.1).
   fn s2_handle_adopt(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
       let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
           rv.set_null();
           let handle = args.get(0).integer_value(scope).unwrap_or(0) as u32;
           let (index, serial) = crate::entity::decode_handle(handle);
           let Some(id) = crate::entity_live::adopt(index, serial) else { return };
           let arr = v8::Array::new(scope, 2);
           let iv = v8::Integer::new(scope, index);
           let dv = v8::Number::new(scope, id as f64);
           arr.set_index(scope, 0, iv.into());
           arr.set_index(scope, 1, dv.into());
           rv.set(arr.into());
       }));
   }
   ```
   Registration (after the `__s2_handle_decode` line):
   ```rust
       set_native(scope, global_obj, "__s2_ent_id_for_index", s2_ent_id_for_index);
       set_native(scope, global_obj, "__s2_handle_adopt", s2_handle_adopt);
   ```

3. **Run green:** `cargo test -p s2script-core minting_natives` then full suite.

4. **Commit:**
   ```bash
   git add core/src/v8host.rs
   git commit -m "feat(core): __s2_ent_id_for_index + __s2_handle_adopt minting natives"
   ```

---

### Task 8: THE atomic flip — `EntityRef = {index, id}`, books-first resolution

This is the load-bearing task. It is one atomic unit because the native signatures, the prelude, and the in-isolate tests can only be green together. Work in the sub-step order below; the crate will not compile between sub-steps — that is expected; the gate is green at the END of the task.

**Files:**
- Modify: `core/src/v8host.rs` only. Regions:
  - `entity_current_serial` (:3433) + `s2_ent_current_serial` (:3473) + its registration (:7458) — DELETE.
  - `entity_resolve_ptr` (:3452) — rewrite; SAFETY comments :3422-3428 / :3445-3451 — rewrite.
  - Every `__s2_ent_ref_*` native (:3487-3782) — id-parse via `js_ent_id`.
  - Op-forwarding natives (list below) — translate id → engine serial.
  - Mint sites (:4202, :5797, :5867, :5899, s2_give_named_item return :6447, readHandleVector elements :6512, dispatch_output :7252-7263) — adopt from books.
  - `build_entity_ref` (:5727) — id-typed.
  - Prelude JS: EntityRef block (:977-1027), spawn/teleport/remove/EKV block (:1066-1119), replacer/reviver (:1140-1147), transmit (:1433-1444), damage refs (:1586-1609), trace ignore (:1672-1694), sound entity (:2146-2147).
  - Test module: fake plumbing (:11562-11590) + every test asserting `.serial` / calling `__s2_ent_current_serial`.

**Interfaces:**
- **Consumes:** Task 3 (`entity_live::{engine_serial_for, adopt, on_created, reset_for_tests}`), Task 5 (`ops.ent_resolve`), Task 7 (`js_ent_id`, `__s2_ent_id_for_index`, `__s2_handle_adopt`).
- **Produces (Task 9/10 depend on):**
  - JS: `EntityRef` instances carry `{index, id}`; **`ref.serial` no longer exists.** Ctor: `new EntityRef(index, id)`.
  - Wire format: `{__s2ref: [index, id]}` (replacer/reviver). Old `{__entref__:[...]}` blobs revive as inert plain objects (the stale-data contract).
  - Rust: `fn entity_resolve_ptr(index: i32, id: u64) -> *mut u8`; `fn build_entity_ref(scope, index: i32, id: u64) -> Local<Value>`; helper `fn ent_op_serial(scope, idx_arg, id_arg) -> Option<(i32, i32)>`.
  - Every `__s2_*` native that previously took `(index, serial)` from JS now takes `(index, id)`.

**Steps:**

1. **Write the two acceptance tests FIRST** (they encode the whole point of E1; they will not compile/pass until the end of the task):

   ```rust
   // --- E1 fake slot-side plumbing: a fake ent_resolve op + a books seed. Replaces the
   // retired FakeEnt/FakeIdent instance-identity fakes (nothing reads instance identity
   // anymore — resolution is books + slot op).
   thread_local! {
       static FAKE_RESOLVE_KEY: std::cell::Cell<(i32, i32)> = std::cell::Cell::new((-1, -1));
       static FAKE_RESOLVE_PTR: std::cell::Cell<*mut std::os::raw::c_void> =
           std::cell::Cell::new(std::ptr::null_mut());
   }
   extern "C" fn fake_ent_resolve(idx: c_int, serial: c_int) -> *mut std::os::raw::c_void {
       if (idx, serial) == FAKE_RESOLVE_KEY.with(|c| c.get()) { FAKE_RESOLVE_PTR.with(|c| c.get()) }
       else { std::ptr::null_mut() }
   }
   /// Seed the books + arm the fake slot resolver for (index, serial). Returns the minted
   /// host id. The backing buffer is a leaked 4KB zeroed block (writable, long-lived).
   fn arm_fake_entity(index: i32, serial: i32) -> u64 {
       let id = crate::entity_live::on_created(index, serial);
       let buf: &'static mut [u8; 4096] = Box::leak(Box::new([0u8; 4096]));
       FAKE_RESOLVE_KEY.with(|c| c.set((index, serial)));
       FAKE_RESOLVE_PTR.with(|c| c.set(buf.as_mut_ptr() as *mut std::os::raw::c_void));
       id
   }

   /// E1 ACCEPTANCE (unit form of the changelevel repro): a ref held across a map start
   /// resolves null/false/dead — even though the ENGINE-side slot still reads live (the
   /// fake resolver still answers). Stage 1 (the books) wins; the old design green-lit
   /// exactly this case into a UAF.
   #[test]
   fn stale_ref_after_map_start_is_null_not_uaf() {
       crate::entity_live::reset_for_tests();
       let _ = init(dummy_logger());
       let _id = arm_fake_entity(42, 7);
       set_engine_ops(Some(S2EngineOps { ent_resolve: Some(fake_ent_resolve), ..mock_event_ops() }));
       create_plugin_context("cl");
       eval_in_context_string("cl", r#"
           var E = __s2require("@s2script/sdk/entity").EntityRef;
           globalThis.__ref = new E(42, __s2_ent_id_for_index(42));
           globalThis.__before = __ref.isValid() + ":" + (__ref.readInt32(8) !== null);
           "ok"
       "#);
       assert_eq!(eval_in_context_string("cl", "globalThis.__before"), "true:true",
                  "live before the transition (books + fake slot agree)");
       dispatch_map_start("de_vertigo");            // the implicit epoch: books clear
       // The fake engine slot STILL resolves (simulating freed-but-unchanged memory) —
       // the books alone must kill the ref:
       assert_eq!(eval_in_context_string("cl", "String(__ref.isValid())"), "false");
       assert_eq!(eval_in_context_string("cl", "String(__ref.readInt32(8))"), "null");
       assert_eq!(eval_in_context_string("cl", "String(__ref.writeInt32(8, 5))"), "false");
       shutdown();
   }

   /// E1 ACCEPTANCE: cross-map (index, serial) aliasing is impossible — the SAME engine
   /// pair re-created after a clear gets a FRESH host id; a ref captured before the
   /// transition stays dead (Candidate D's win over the bare (index,serial) table).
   #[test]
   fn same_index_serial_on_new_map_does_not_revive_old_refs() {
       crate::entity_live::reset_for_tests();
       let _ = init(dummy_logger());
       let old_id = arm_fake_entity(64, 3);
       set_engine_ops(Some(S2EngineOps { ent_resolve: Some(fake_ent_resolve), ..mock_event_ops() }));
       dispatch_map_start("de_inferno");
       let new_id = crate::entity_live::on_created(64, 3);      // same pair, new map
       assert!(new_id > old_id);
       create_plugin_context("alias");
       eval_in_context_string("alias", &format!(r#"
           var E = __s2require("@s2script/sdk/entity").EntityRef;
           globalThis.__old = String(new E(64, {old_id}).isValid());
           globalThis.__new = String(new E(64, {new_id}).isValid());
           "ok"
       "#));
       assert_eq!(eval_in_context_string("alias", "__old"), "false", "old id never revives");
       assert_eq!(eval_in_context_string("alias", "__new"), "true");
       shutdown();
   }
   ```
   `cargo test -p s2script-core stale_ref` → fails (first as a compile error against the old fakes/signatures — that is the observed red).

2. **Rewrite the resolver core** (delete `fn entity_current_serial` at :3433, `fn s2_ent_current_serial` at :3473, and its `set_native` at :7458):
   ```rust
   /// Resolve (index, host-id) to a live entity pointer, or null. Resolution order
   /// (north-star §3.1 — cheapest & safest first):
   ///   1. THE BOOKS: LIVE[index].id == id, else null. No engine memory touched.
   ///   2. Defense-in-depth: the shim validates the stored engine serial in the
   ///      system-owned identity CHUNK (`ent_resolve`, the s2_deref_handle idiom) and
   ///      returns m_pInstance — instance memory is never read to decide liveness.
   ///   3. Only the CALLER derefs the instance, block-scoped within one native.
   /// The raw pointer stays in Rust — it never crosses to JS. Errors fall toward null,
   /// never toward a deref.
   fn entity_resolve_ptr(index: i32, id: u64) -> *mut u8 {
       let Some(engine_serial) = crate::entity_live::engine_serial_for(index, id) else {
           return std::ptr::null_mut();
       };
       let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return std::ptr::null_mut() };
       let Some(resolve) = ops.ent_resolve else { return std::ptr::null_mut() };
       resolve(index, engine_serial) as *mut u8
   }

   /// (index-arg, id-arg) → (index, stored engine serial), for shim ops that serial-gate
   /// internally. None = the books say not-live — the op is never called (fail-closed).
   fn ent_op_serial(scope: &mut v8::PinScope, idx_arg: v8::Local<v8::Value>, id_arg: v8::Local<v8::Value>) -> Option<(i32, i32)> {
       let index = idx_arg.integer_value(scope).unwrap_or(-1) as i32;
       let id = js_ent_id(scope, id_arg);
       let serial = crate::entity_live::engine_serial_for(index, id)?;
       Some((index, serial))
   }
   ```
   Rewrite the block comment at :3422-3428 to the new doctrine (books-first, slot-validated, instance-last). Update `s2_ent_ref_valid` (:3487) to:
   ```rust
           let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
           let id = js_ent_id(scope, args.get(1));
           rv.set_bool(!entity_resolve_ptr(index, id).is_null());
   ```

3. **Mechanical native conversion — Group A (resolve-through; arg1 = id).** In each of `s2_ent_ref_read` (:3514), `s2_ent_ref_write` (:3548), `s2_ent_ref_read_string` (:3579), `s2_ent_ref_write_string` (:3601), `s2_ent_ref_read_floats` (:3623), `s2_ent_ref_read_floats_chain` (:3653), `s2_ent_ref_read_chain` (:3686), `s2_ent_ref_write_chain` (:3729), `s2_ent_ref_state_changed` (:3767), replace
   `let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;` with
   `let id = js_ent_id(scope, args.get(1));` and `entity_resolve_ptr(index, serial)` with `entity_resolve_ptr(index, id)`.

4. **Mechanical native conversion — Group B (op-forwarding; translate via `ent_op_serial`).** Each native parses `(index, id)` from the SAME arg positions that held `(index, serial)`, translates, and passes the ENGINE serial to the unchanged shim op. Convert (verified anchors): `s2_pawn_commit_suicide` :5365, `s2_player_change_team` :5380, `s2_gamerules_terminate_round` :5398, `s2_player_switch_team` :5421, `s2_player_respawn` :5440, `s2_transmit_set` :5460, `s2_transmit_reset` :5507, `s2_trace` :5762 (args 6/7 — ignore pair; translation failure → pass `(-1, -1)`, "no ignore entity"), `s2_entity_spawn` :6177, `s2_collision_activate` :6190, `s2_ent_set_model` :6204, `s2_sound_emit` :6223 (entity source pair), `s2_entity_teleport` :6268, `s2_entity_remove` :6288, `s2_entity_fire_input` :6302 (THREE pairs — self mandatory; activator/caller optional: JS `-1`/id-0/translation-miss all → `(-1, -1)` to the op), `s2_entity_spawn_kv` :6330, `s2_give_named_item` :6447 (owner pair), `s2_entity_subobj_vcall` :6472, `s2_remove_player_item` :6492 (two pairs), `s2_entity_read_handle_vector` :6512 (root pair), `s2_entity_name` :6707. Worked example (`s2_player_change_team`):
   ```rust
   fn s2_player_change_team(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
       let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
           let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
           let team = args.get(2).integer_value(scope).unwrap_or(0) as c_int;
           let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
           let Some(func) = ops.player_change_team else { return };
           func(index, serial, team);
       }));
   }
   ```
   (Preserve each native's existing degrade value and breadcrumb `note_engine_op` calls exactly.)

5. **Mint sites — Group C (adopt from the books).** `build_entity_ref` becomes:
   ```rust
   /// Construct `new EntityRef(index, id)` — id is the HOST-MINTED books id (f64 on the
   /// wire), never a raw engine serial. Falls back to null if @s2script/entity isn't
   /// installed on this context.
   fn build_entity_ref<'s>(scope: &mut v8::PinScope<'s, '_>, index: i32, id: u64) -> v8::Local<'s, v8::Value> {
       // ... identical lookup of __s2pkg_entity.EntityRef ...
       let idx_v = v8::Integer::new(scope, index);
       let id_v = v8::Number::new(scope, id as f64);
       ctor.new_instance(scope, &[idx_v.into(), id_v.into()]).map(...)
   }
   ```
   Then at every mint site replace the `decode → entity_resolve_ptr(idx, ser) → build_entity_ref(idx, ser)` idiom with adoption:
   ```rust
   let (idx, ser) = crate::entity::decode_handle(handle as u32);
   match crate::entity_live::adopt(idx, ser) {
       Some(id) => build_entity_ref(scope, idx, id),
       None => v8::null(scope).into(),   // absent/mismatched books → null (fail-closed)
   }
   ```
   Sites: `dispatch_entity_event` :4202-4207 (delete dispatches still adopt — the ffi entry removes AFTER dispatch, Task 4 contract); `s2_trace` hit :5797-5806; `s2_entity_create` :5867-5874 (the create listener fed the books synchronously via the ffi entry — adoption is the proof); `s2_entity_find_by_class` :5899-5906 (per element); `s2_give_named_item` returned handle; `s2_entity_read_handle_vector` per element; `dispatch_output` activator/caller :7252-7263 (keep the exact `== -1` sentinel handling).

6. **Prelude JS flip.** In `INJECTED_STD_PRELUDE`:
   - Ctor (:977): `function EntityRef(index, id) { this.index = index; this.id = id; }`
   - Replace-all `this.index, this.serial` → `this.index, this.id` (covers all ~40 methods incl. spawn/teleport/remove/activateCollision/setModel/name/readHandleVector/acceptInput self-pair).
   - `acceptInput` activator/caller (:1116-1117): `activator ? activator.id : -1`, `caller ? caller.id : -1`.
   - `readHandle` / `readHandleVia` (:1016-1025): adopt, don't decode:
     ```js
     readHandleVia: function (c, o) { var h = __s2_ent_ref_read_chain(this.index, this.id, c, o, K.U32);
       if (h === null) return null; var d = __s2_handle_adopt(h >>> 0);
       return d ? new EntityRef(d[0], d[1]) : null; },   // books-adopted; a dangling handle mints null
     readHandle: function (o) {
       var h = __s2_ent_ref_read(this.index, this.id, o, K.U32);
       if (h === null) return null;
       var d = __s2_handle_adopt(h >>> 0);
       return d ? new EntityRef(d[0], d[1]) : null;
     },
     ```
   - Damage refs `__s2_dmg_ref` (:1586-1587) and `DamageInfo.victim` (:1608-1609): same `__s2_handle_adopt` pattern (null when no adoption; drop the now-redundant `ref.isValid()` mint-gate or keep it — keep, it adds the slot check).
   - Transmit (:1433-1444): `typeof entity.id === "number"` + pass `entity.id`.
   - Trace ignore (:1672-1673): `{ idx: e.index, serial: e.serial }` → `{ idx: e.index, id: e.id }` and pass `ig.id` (the native's arg 7 is now the id).
   - Sound emit (:2146-2147): `serial = e.serial | 0` → pass `e.id` (note: `| 0` would truncate an id > 2^31 — pass the raw number, no coercion: `id = (typeof e.id === "number") ? e.id : 0`).
   - Replacer/reviver (:1140-1147):
     ```js
     // Inter-plugin wire tagging: `__s2ref` is the E1 wire key — [index, HOST-id]. Old
     // `__entref__` (engine-serial) blobs deliberately revive as inert plain data.
     globalThis.__s2_entref_replacer = function (key, value) {
       return (value instanceof EntityRef) ? { __s2ref: [value.index, value.id] } : value;
     };
     globalThis.__s2_entref_reviver = function (key, value) {
       return (value && typeof value === "object" && Array.isArray(value.__s2ref))
         ? new EntityRef(value.__s2ref[0], value.__s2ref[1])
         : value;
     };
     ```
7. **Test-suite rework** (same file): delete `FakeIdent`/`FakeEnt`/`fake_ent_by_index`/old `arm_fake_entity` (:11562-11590) in favor of step 1's plumbing. Update: `entity_event_dispatch_delivers_live_entityref` (:11653 — seed via `crate::ffi::s2script_core_dispatch_entity_event` with the encoded handle, wire `ent_resolve: Some(fake_ent_resolve)`, assert `e.index + ":" + e.id`); the handle-decode test (:11366 — `__s2_ent_current_serial` is GONE; assert `__s2_ent_id_for_index(1) === 0` instead); wire/handoff tests (:10752, :11870, :11900 — `.serial` → `.id`, and seed the books so revived refs can be live where the test needs it); read/write roundtrip tests — seed books + fake resolver, exercise `readInt32/writeInt32` against the leaked buffer. Every degrade-without-ops test keeps passing as-is conceptually (`new EntityRef(1, 7)` with empty books → stage 1 already fails) — update only constructor comments. Add a books-hygiene line `crate::entity_live::reset_for_tests();` at the TOP of every test that mints or seeds (single-threaded suite: state leaks).

8. **Sweep for stragglers:**
   ```bash
   grep -n "__s2_ent_current_serial\|__entref__" core/src/v8host.rs        # → 0 hits
   grep -n "\.serial" core/src/v8host.rs                                   # → ONLY Rust-side engine-serial
                                                                           #   internals (TransmitRule etc.)
   ```
9. **Run green:** `cargo test -p s2script-core` (FULL suite) + `make check-boundary`.

10. **Commit:**
    ```bash
    git add core/src/v8host.rs
    git commit -m "feat(core)!: EntityRef = {index, host-id} — books-first resolution, slot-validated stage 2"
    ```

---

### Task 9: game-package + typings + plugin migration to `{index, id}`

**Files:**
- Modify: `games/cs2/js/pawn.js` (lines 45, 73, 110, 114, 123, 149, 230, 255, 422, 426-427, 856), `games/cs2/js/weapon.js` (line 50).
- Modify: `packages/sdk/entity.d.ts` (lines 8-19 + prose), `packages/cs2/weapon.d.ts` / `packages/cs2/index.d.ts` (prose only — no `serial` members exist).
- Modify: `plugins/basecommands/src/plugin.ts` (:87-88), `examples/demo-plugin/src/plugin.ts` (:19), `examples/transmit-demo/src/plugin.ts` (:41), `examples/switchteam-demo/src/plugin.ts` (:26, :38).
- Create: `.changeset/e1-entity-ref-host-id.md`.
- Do NOT touch `*.generated.js` / `*.generated.d.ts` (verified: no serial/EntityRef references).

**Interfaces:**
- **Consumes (Tasks 7, 8):** `__s2_ent_id_for_index`, `__s2_handle_adopt`, JS `EntityRef {index, id}`.
- **Produces:** the public TS contract — `EntityRef.id: number` (host-minted liveness id), ctor `(index, id)`; `serial` gone. Task 10 edits `pawn.js` after this task.

**Steps:**

1. **Failing gate first:** run `./scripts/check-plugins-typecheck.sh` — it currently PASSES (old d.ts). Make the d.ts change FIRST so the gate turns red, then fix consumers:
   In `packages/sdk/entity.d.ts` (:8-19):
   ```ts
   /**
    * A host-liveness-gated handle to a live entity. `id` is a HOST-MINTED monotonic
    * liveness id — liveness is decided by the host's books (fed by engine create/delete
    * notifications, cleared at map transition), NEVER by reading the entity's own memory.
    * Every access re-resolves: books first, then identity-slot validation, instance last.
    * A stale ref degrades to null/false — including across a changelevel.
    */
   export declare class EntityRef {
     readonly index: number;
     readonly id: number;
     constructor(index: number, id: number);
     /** True iff the host's books say live AND the identity slot still matches. */
     isValid(): boolean;
     ...
   ```
   (Keep every method signature; update `serial`-wording prose lines 19/71/73/112/159/170/174 to "liveness-gated".)
   Run `./scripts/check-plugins-typecheck.sh` → **observed failure:** `Property 'serial' does not exist on type 'EntityRef'` in basecommands/demo-plugin/transmit-demo/switchteam-demo.

2. **Fix the four TS consumers** — mechanical `.serial` → `.id` at the listed lines (log strings like `vic.index + "/" + vic.id`).

3. **Migrate `games/cs2/js/pawn.js`:**
   - :45, :73, :422 — index-minting:
     ```js
     var ref = new EntityRef(idx, __s2_ent_id_for_index(idx));
     ```
   - :426-427 (`Pawn.forSlot` handle path):
     ```js
     var d = __s2_handle_adopt(handle >>> 0);
     if (!d) return null;                       // dangling m_hPlayerPawn can never mint
     var pawn = new EntityRef(d[0], d[1]);
     ```
   - :110, :114, :123, :149, :230, :255, :856 — `this.ref.serial` → `this.ref.id`.
   - `games/cs2/js/weapon.js` :50 — both pairs → `.id`.
   - Sweep: `grep -n "\.serial\|__s2_handle_decode\|__s2_ent_current_serial" games/cs2/js/*.js` → 0 hits (generated files excluded — they have none).

4. **Changeset** — `.changeset/e1-entity-ref-host-id.md`:
   ```md
   ---
   "@s2script/sdk": minor
   "@s2script/cs2": minor
   ---

   BREAKING (pre-1.0 minor): `EntityRef` is now `{index, id}` — `id` is a host-minted
   liveness id replacing the raw engine `serial` on the public surface. Liveness is
   decided by the host's books (listener-fed, cleared per map), never by entity memory;
   stale refs — including across a changelevel — deterministically resolve to
   `null`/`false`. The inter-plugin/handoff wire format is `{__s2ref: [index, id]}`;
   pre-E1 `{__entref__}` blobs revive as inert data.
   ```

5. **Run the full gate suite green** (all commands in Global Constraints, including `cargo test -p s2script-core` and `./scripts/check-plugins-typecheck.sh`).

6. **Commit:**
   ```bash
   git add games/cs2/js packages/sdk/entity.d.ts packages/cs2 plugins examples .changeset/e1-entity-ref-host-id.md
   git commit -m "feat(cs2+sdk)!: migrate game package, typings, plugins to EntityRef {index, id}"
   ```

---

### Task 10: identity-derived reads — `pawn.isValid` drops the `[16]→48` instance chain

**Files:**
- Modify: `core/src/v8host.rs` — new native `__s2_ent_identity_flags` (next to `s2_entity_name` :6707) + registration + prelude method `EntityRef.prototype.identityFlags` + delete `ENT_IDENTITY_PTR_OFFSET`/`ENT_IDENTITY_HANDLE_OFFSET` (:429-431, now unreferenced) + mark `ent_by_index`/`deref_handle` ops fields `// unused since E1 (kept for ABI order)`.
- Modify: `games/cs2/js/pawn.js` — the `isValid` getter (:219-226).
- Modify: `packages/sdk/entity.d.ts` — add `identityFlags(): number | null;`.

**Interfaces:**
- **Consumes:** Task 5 (`ops.ent_identity_flags`), Task 8 (`ent_op_serial`), Task 9 (id-world pawn.js).
- **Produces:** JS `EntityRef.prototype.identityFlags() -> number | null` — raw `CEntityIdentity::m_flags` read from the SLOT (books-translated, chunk-validated); null = stale/absent/op-missing. Flag bit VALUES stay game-side.

**Steps:**

1. **Failing test:**
   ```rust
   thread_local! { static FAKE_FLAGS: std::cell::Cell<i64> = std::cell::Cell::new(-1); }
   extern "C" fn fake_ent_identity_flags(idx: c_int, serial: c_int) -> i64 {
       if (idx, serial) == FAKE_RESOLVE_KEY.with(|c| c.get()) { FAKE_FLAGS.with(|c| c.get()) } else { -1 }
   }

   /// E1: identityFlags reads the SLOT (books-translated) — live flags cross; a stale ref
   /// or missing op degrades to null. This is the primitive pawn.isValid's staging check
   /// rides on, with the [16]→48 instance chain gone.
   #[test]
   fn identity_flags_is_slot_side_and_books_gated() {
       crate::entity_live::reset_for_tests();
       let _ = init(dummy_logger());
       let id = arm_fake_entity(9, 4);
       FAKE_FLAGS.with(|c| c.set(0x104));            // arbitrary flags incl. bit 2 (staging)
       set_engine_ops(Some(S2EngineOps {
           ent_identity_flags: Some(fake_ent_identity_flags), ..mock_event_ops() }));
       create_plugin_context("fl");
       eval_in_context_string("fl", &format!(r#"
           var E = __s2require("@s2script/sdk/entity").EntityRef;
           globalThis.__live  = String(new E(9, {id}).identityFlags());
           globalThis.__stale = String(new E(9, {id} + 1).identityFlags());
           "ok"
       "#));
       assert_eq!(eval_in_context_string("fl", "__live"), "260");
       assert_eq!(eval_in_context_string("fl", "__stale"), "null", "books gate the flags read");
       shutdown();
   }
   ```
   Run → fails (`identityFlags is not a function`).

2. **Impl.** Native:
   ```rust
   /// Native `__s2_ent_identity_flags(index, id) -> number | null`. CEntityIdentity::m_flags
   /// read from the identity SLOT via the ent_identity_flags op (books-translated id →
   /// engine serial; chunk-validated shim-side) — NEVER via instance+0x10. The E1
   /// replacement for the retired readInt32Via([16], 48) staging-flag chain.
   fn s2_ent_identity_flags(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
       let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
           rv.set_null();
           let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
           let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
           let Some(func) = ops.ent_identity_flags else { return };
           let flags = func(index, serial);
           if flags >= 0 { rv.set_double(flags as f64); }
       }));
   }
   ```
   + `set_native(scope, global_obj, "__s2_ent_identity_flags", s2_ent_identity_flags);`
   Prelude (after `notifyStateChanged` in the EntityRef prototype):
   ```js
   // E1: raw CEntityIdentity::m_flags from the identity SLOT (books-gated; never
   // instance memory). null = stale/unavailable. Flag bit meanings are game facts —
   // interpret them in the game package, not here.
   identityFlags: function () { return __s2_ent_identity_flags(this.index, this.id); },
   ```
   `games/cs2/js/pawn.js` isValid getter (:219-226):
   ```js
   // pawn.isValid — SourceMod/CSSharp-sense validity: live per the HOST'S BOOKS (+ slot
   // check) AND fully spawned (out of the engine EF_IN_STAGING_LIST). The staging bit is
   // read from the identity SLOT via ref.identityFlags() — the pre-E1 readInt32Via([16],48)
   // instance chain (the changelevel-UAF crash site) is gone. If the flags read is
   // unavailable (null), fall back to liveness (do not over-block a live pawn).
   Object.defineProperty(Pawn.prototype, "isValid", {
     get: function () {
       if (!this.ref.isValid()) return false;
       var flags = this.ref.identityFlags();
       return flags === null ? true : (flags & 4) === 0;   // EF_IN_STAGING_LIST = 1<<2 (CS2 fact)
     },
     enumerable: true, configurable: true,
   });
   ```
   d.ts: `/** Raw identity-slot flags (engine m_flags), or null when stale/unavailable. Bit meanings are game-specific. */ identityFlags(): number | null;`
   Delete the two `ENT_IDENTITY_*` consts (:429-431); annotate `ent_by_index`/`deref_handle` fields as unused-since-E1. Verify: `grep -n "ENT_IDENTITY\|readInt32Via(\[16\], 48)\|readInt32Via(\[16\],48)" core/src/v8host.rs games/cs2/js/*.js` → 0 hits.

3. **Run green:** `cargo test -p s2script-core` + `./scripts/check-plugins-typecheck.sh` + `make check-boundary`.

4. **Commit:**
   ```bash
   git add core/src/v8host.rs games/cs2/js/pawn.js packages/sdk/entity.d.ts
   git commit -m "feat(core+cs2): pawn.isValid via slot-side identity flags — retire the [16]->48 instance chain"
   ```

---

### Task 11: documentation — the doctrine lands where the next agent reads it

**Files:**
- Modify: `docs/ARCHITECTURE.md` — the entity-safety section (find it: `grep -n "EntityRef\|serial" docs/ARCHITECTURE.md`).
- Modify: `CLAUDE.md` — ONLY the `**Entity safety (locked in Slice 5A)**` paragraph in `## Current state`.

**Interfaces:** Consumes the shipped Task 8/10 reality; produces no code contract.

**Steps:**

1. Rewrite the ARCHITECTURE.md entity-safety section to describe: the doctrine sentence (host's books, populated by notifications, cleared by transitions, never the referent's memory); `liveness.rs` + its two instances; `EntityRef = {index, id}`; the 3-stage resolution order; the unconditional ffi feed (create/spawn-before, delete-after) and why it must not live in the mux dispatch; the map-start clear as the implicit epoch + the one-shot repair sweep; the `{__s2ref:[index,id]}` wire form; the minting rules (`__s2_ent_id_for_index`, `__s2_handle_adopt` — a dangling handle can never mint).
2. Replace the CLAUDE.md Slice-5A entity-safety paragraph with an E1 version (same length discipline — one paragraph): books authority, `{index, id}`, `ent_resolve` slot validation, errors fall toward null, the two liveness axes are separate instances of `liveness.rs`.
3. No test cycle (docs); verify `make check-boundary` still green (no code touched).
4. **Commit:**
   ```bash
   git add docs/ARCHITECTURE.md CLAUDE.md
   git commit -m "docs: entity liveness core-authority — books doctrine, EntityRef {index,id} (E1)"
   ```

---

### Task 12: LIVE GATE (HUMAN) — the changelevel repro goes SEGV → null

**This task is executed by the human operator, not a Workflow agent.** Agents may prepare the gate plugin (step 1) in a prior commit; everything from step 2 on is manual.

**Files:**
- Create: `examples/liveness-gate/package.json`, `examples/liveness-gate/tsconfig.json`, `examples/liveness-gate/src/plugin.ts` (copy scaffold shape from `examples/demo-plugin/`).

**Interfaces:** Consumes the full E1 surface. Produces the acceptance evidence (north-star §3.6) + a `docs/PROGRESS.md` entry.

**Steps:**

1. **Gate plugin** (`examples/liveness-gate/src/plugin.ts`) — holds a pawn ref across the transition and pokes it every frame (the exact crash shape: `player_spawn` handler ref + per-frame `pawn.isValid`/field read):
   ```ts
   import { Events } from "@s2script/sdk/events";
   import { OnGameFrame } from "@s2script/sdk/frame";
   import { Server } from "@s2script/sdk/server";
   import { Player } from "@s2script/cs2";

   let heldPawn: any = null;
   let lastState = "";

   export function onLoad() {
     Events.on("player_spawn", (ev) => {
       const slot = ev.getPlayerSlot("userid");
       const p = Player.fromSlot(slot);
       if (p && p.pawn) {
         heldPawn = p.pawn;
         console.log(`[gate] holding pawn ref idx=${heldPawn.ref.index} id=${heldPawn.ref.id}`);
       }
     });
     Server.onMapStart((map) => console.log(`[gate] map start: ${map} — held ref should now be dead`));
     OnGameFrame.subscribe(() => {
       if (!heldPawn) return;
       // The pre-E1 crash site: isValid (staging flags) + a schema read on a stale ref.
       const state = `valid=${heldPawn.isValid} health=${heldPawn.health}`;
       if (state !== lastState) { lastState = state; console.log(`[gate] ${state}`); }
     });
   }
   ```
   Build it: `cd examples/liveness-gate && npx s2script build`. Commit with `git add examples/liveness-gate && git commit -m "test(examples): liveness-gate plugin — held-ref changelevel repro"`. Deploy ONLY this `.s2sp` (never `examples/*` wholesale — stale-bundle onLoad failures spam frames).

2. **Sniper build + deploy (the ONLY deployable binaries):**
   ```bash
   docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
     rust:bullseye bash /repo/scripts/build-sniper.sh
   make package
   make docker-test
   docker exec s2script-cs2 /patch-gameinfo.sh
   docker compose -f docker/docker-compose.yml stop cs2 && docker compose -f docker/docker-compose.yml start cs2
   # verify the NEW .so actually loaded: docker inspect StartedAt + md5 in /proc/<pid>/maps (respawn-slice lesson)
   ```
3. **Repro (deterministic):** connect/add a bot so a pawn ref is held (`python3 scripts/rcon.py "bot_add"`), confirm `[gate] holding pawn ref` + `valid=true`, then force the transition directly:
   ```bash
   python3 scripts/rcon.py "changelevel de_vertigo"
   ```
   (The original incident path — rapid round-cycling to match-end — also works: `mp_maxrounds 2; mp_roundtime 1; mp_warmup_end`; the direct changelevel is the deterministic form.)
4. **Acceptance checks (north-star §3.6) — ALL must hold:**
   - Server survives the changelevel; the gate plugin logs `valid=false health=null` (deterministic null) — pre-E1 this was the deterministic SEGV.
   - Crash spool EMPTY: `docker exec s2script-cs2 sh -c 'ls addons/s2script/data/crash 2>/dev/null'` (adjust to the configured spool dir) → no new envelopes/minidumps.
   - **Fresh-map sanity (repair sweep / listener timing — the E0-V4 assumption):** on the new map, `Player.fromSlot` works (e.g. `sm_who`/basecommands respond), `Entity.findByClass("cs_gamerules")` returns a live ref, `createEntity` + `ref.isValid()` works, a new `player_spawn` mints a live held ref (`valid=true` resumes for the new pawn).
   - Cross-plugin wire: any interface passing an EntityRef still serial... id-gates correctly (e.g. zones/menu demos), and a pre-E1 handoff blob (if any) revives inert without crashing.
5. **Record:** append the E1 entry to `docs/PROGRESS.md` (what shipped, the gate evidence, the E0 items observed: did `OnEntityDeleted` fire on the changelevel teardown? was the repair sweep needed — check whether fromSlot worked BEFORE the first sweep by log ordering). Commit: `git commit -m "docs(progress): E1 entity liveness — live-gate results"`.
6. **If the gate fails:** do NOT patch live; capture logs + spool, file the failure against the specific stage (books feed / clear / sweep / adoption), and fix under a new task with a unit repro first.

---

## Open questions for human review

1. **Changeset bump level.** CLAUDE.md says breaking = major, but the packages are 0.x under changesets (major would go 1.0.0). The plan uses `minor` with a BREAKING note (0.x convention). Confirm or switch to `major` before release.
2. **`onDelete` ref semantics.** The design says a missed delete degrades safely but doesn't pin whether `onDelete` handlers should receive a resolvable ref. The plan books the delete AFTER the synchronous dispatch (handler can read the dying entity, slot-validated; ref dies when the FFI entry returns). The `.d.ts` already allows null there, so this is the permissive choice — confirm it matches intent (the strict alternative: remove BEFORE dispatch, `onDelete` always gets data-only `className`).
3. **Repair-sweep default-ON.** The design leaves sweep timing to E0. The plan ships it armed-per-map-start, firing at the first simulating frame, because on the FIRST boot map the listener attaches at StartupServer POST and CS2 pre-allocates controller entities whose creates may precede it — without the sweep, `Player.fromSlot` could be permanently null on boot map (a fail-closed but game-breaking degrade). E0/live-gate step 4 verifies; if V4 proves creates always follow StartupServer POST, the sweep can be demoted to a no-op-in-practice safety net (leave it in).
4. **`__s2_handle_decode` retention.** Kept (pure bit-math; used for diagnostics/packing). It can no longer mint a live ref through any in-repo path, but a plugin calling `new EntityRef(idx, decodedSerial)` would fail-closed only probabilistically (serial vs id collision is possible for small ids). Acceptable now; the L1/B2 lint layer (`no-raw-entityref-ctor`) is the principled fix — consider removing the ctor from the public `.d.ts` in E1 instead (breaking, but nothing in-repo constructs refs manually outside the prelude/game package).
5. **`ent_snapshot` CAP = 32768.** MAX_TOTAL_ENTITIES headroom hardcoded core-side; if the walk can ever exceed it the tail is silently dropped (still fail-closed). Confirm the constant against the SDK's MAX_TOTAL_ENTITIES at review.

## Self-Review

- **Spec coverage vs north-star §3:** §3.1 books (Task 3), listener feed unconditional-in-FFI incl. the re-entrancy/no-subscriber trap (Task 4, quoted in the code comment), map-clear-as-epoch (Task 4), `{index, id}` ref + wire `{__s2ref:[index,id]}` (Task 8), resolution order 1-books/2-slot/3-instance (Task 8 `entity_resolve_ptr` doc), minting-from-handles adoption incl. `readHandle`/`__s2_handle_decode` retirement from minting paths (Tasks 7/8/9), identity-derived reads off the slot + `[16]→48` death (Tasks 5/10), §3.4 coverage rows encoded as unit tests (stale-after-map-start, cross-map aliasing — Task 8 step 1), §3.6 acceptance as the live gate (Task 12), §6.1 shared `liveness.rs` + Registry refactor (Tasks 1-2), §6.2 ~4-PR stack (5-PR mapping given), §7 repair-sweep + u64-on-wire items resolved inline with E0 assumptions flagged.
- **Beyond-spec necessities found in code and covered:** lazy listener install made unconditional (Task 5 — the spec silently assumes an always-fed listener); the ~20 op-forwarding natives + transmit/trace/sound prelude sites that also carry serials (Task 8 Groups B/D, all with verified line anchors); the 4 TS consumers of `ref.serial` that would break the 5E.1 gate (Task 9); exhaustive `S2EngineOps` test literals that break on field append (Task 5 step 1 makes that the observed red).
- **Placeholder scan:** no TODO/elided bodies in any test or impl shown; the only `...` appearances are inside `build_entity_ref` (explicitly "identical lookup" of unchanged lines) and d.ts continuation markers — both are edits to existing verified code, with the changed lines given in full.
- **Type consistency across Consumes/Produces:** `entity_live` signatures identical in Task 3 Produces and Tasks 4/6/7/8/10 Consumes; `EntResolveFn`/`ent_op_serial`/`js_ent_id`/`build_entity_ref(index: i32, id: u64)` names match everywhere they appear; id is u64 in Rust, f64 (Number) on the wire, `number` in `.d.ts` — the 2^53 bound enforced in exactly one place (`js_ent_id`).
- **Fail-closed audit:** every unknown resolves toward null — id 0 sentinel, adoption misses, missing ops, stale deletes, sweep-absent, old wire blobs. No path derefs instance memory to decide liveness anywhere after Task 8; the one deliberate read-during-delete window is slot-validated.
