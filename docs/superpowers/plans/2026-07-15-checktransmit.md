# CheckTransmit (per-client entity visibility) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-client entity visibility filtering (`@s2script/transmit`): plugins register declarative, serial-gated per-entity viewer rules; a `CheckTransmit` POST SourceHook applies them natively per snapshot with zero JS in the hot path. This is the TTT-blocking capability.

**Architecture:** A POST SourceHook on `ISource2GameEntities::CheckTransmit` (`Source2GameEntities001`, acquired exactly like the seven `Source2GameClients001` hooks) mutates each client's transmit bitvec from a shim-owned `entindex → {serial, u64 viewer-mask}` table. The core owns policy: per-plugin rule maps (ledgered — unload clears), AND-merge across plugins, pushed to the shim via three new append-only `S2EngineOps` ops. The one non-SDK layout fact (which client an info struct is for, int32 at gamedata offset `CheckTransmitInfo_clientEntityIndex`, hint +576) is validated at FIRST FIRE, fail-closed, with a named GAMEDATA-style disable on mismatch.

**Tech Stack:** C++ shim (SourceHook, hl2sdk `eiface.h`/`iservernetworkable.h`/`bitvec.h`), Rust core (`core/src/v8host.rs`, rusty_v8), gamedata jsonc, `.d.ts` types package, TS demo plugin.

## Global Constraints

- **The full spec is at `docs/superpowers/specs/2026-07-15-checktransmit-design.md`** — read it before your task.
- **Engine-generic only.** Nothing in this slice touches `games/*`; `make check-boundary` must stay green. `ISource2GameEntities`/`CCheckTransmitInfo` are Source 2 engine facts.
- **The ops ABI is append-only.** New `S2EngineOps` fields go AFTER `player_change_team` in BOTH `core/src/v8host.rs` and `shim/include/s2script_core.h`, byte-identical field order/names, with the codebase's `APPENDED after <prev>; order is the ABI; do not reorder above` comment convention. Never reorder or remove existing fields.
- **Never a bare borrowed constant.** The +576 offset lives in `gamedata/core.gamedata.jsonc` (data, not code) and is validated at first hook fire, fail-closed; failure → named `gamedata FAIL` line + descriptor disable. See `docs/re-strategy.md`.
- **Degrade, never crash.** Missing interface / missing op / stale ref / failed validation → `false`/no-op with a WARN; the framework keeps running.
- **No raw pointer crosses to JS.** Entities cross as `(index, serial)` pairs; the shim serial-gates at registration AND once per snapshot per entry.
- **Panic safety:** every new Rust native body is wrapped in `std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { ... }))`.
- **Naming:** PascalCase namespace object (`Transmit`), camelCase methods (`setVisibleTo`, `reset`, `resetAll`, `stats`).
- **Tests:** `cargo test -p s2script-core` is forced single-threaded via `.cargo/config.toml` — do NOT pass `--test-threads`.
- **Changeset required** for the new `packages/transmit` package (`npm run changeset` is interactive — write the changeset file directly instead, as specified in Task 3).
- **Commit style:** conventional commits; end every commit message with the session trailer line shown in each commit step.

## File Structure

- `core/src/v8host.rs` — new `S2EngineOps` fields + fn-pointer type aliases; `TRANSMIT_RULES` per-plugin store + AND-merge/push helpers; 4 natives (`__s2_transmit_set/reset/reset_all/stats`); prelude `Transmit` module + `__s2pkg_transmit` export; unload/shutdown wiring; in-isolate tests.
- `shim/include/s2script_core.h` — the 3 op typedefs + byte-identical struct tail fields.
- `shim/src/s2script_mm.h` — forward decls, `Hook_CheckTransmit` member, `m_gameEntities` + `m_checkTransmitHookInstalled` members.
- `shim/src/s2script_mm.cpp` — `#include <iservernetworkable.h>`; SH_DECL; rule table + stats statics; the 3 op fns; first-fire layout validation; `Hook_CheckTransmit` body; Load() acquisition/hook-install/ops-assignment; Unload() removal.
- `gamedata/core.gamedata.jsonc` — `interfaces.Source2GameEntities`, `offsets.CheckTransmitInfo_clientEntityIndex`.
- `packages/transmit/package.json`, `packages/transmit/index.d.ts` — the types package.
- `.changeset/checktransmit.md` — the changeset.
- `examples/transmit-demo/{package.json,tsconfig.json,src/plugin.ts}` — the live-gate demo plugin.

---

## Task 1: Core — ops ABI, rule store, natives, prelude module, tests

**Files:**
- Modify: `core/src/v8host.rs` (S2EngineOps struct ends ~line 335; type aliases live just above it; natives cluster ~line 5379-5579; `set_native` registrations ~line 6522; prelude `INJECTED_STD_PRELUDE` starts ~line 730, `__s2pkg_*` exports ~line 1195-1203; `unload_plugin` step-(a) mux block ~line 8611; tests + `mock_event_ops` ~line 10300+)

**Interfaces:**
- Consumes: existing internals of `core/src/v8host.rs` — `ENGINE_OPS.with(|o| o.get())`, `set_native(scope, global_obj, name, cb)`, `current_plugin(scope) -> Option<String>`, `init(dummy_logger())`/`shutdown()`/`set_engine_ops(...)`/`create_plugin_context(id)`/`eval_in_context_string(id, src)`/`unload_plugin(id)` (test harness), `mock_event_ops()`.
- Produces (later tasks rely on these exact names):
  - Rust ABI (Task 2 mirrors in C): `pub type TransmitSetFn = extern "C" fn(index: c_int, serial: c_int, mask: u64) -> c_int;` · `pub type TransmitClearFn = extern "C" fn(index: c_int) -> c_int;` · `pub type TransmitStatsFn = extern "C" fn(out: *mut u64);` and struct fields `transmit_set: Option<TransmitSetFn>, transmit_clear: Option<TransmitClearFn>, transmit_stats: Option<TransmitStatsFn>` appended after `player_change_team`.
  - JS (Tasks 3/4 rely on): `require("@s2script/transmit")` → `{ Transmit }` with `Transmit.setVisibleTo(entityRef, number[]) -> boolean`, `Transmit.reset(entityRef) -> boolean`, `Transmit.resetAll() -> void`, `Transmit.stats() -> {snapshots, entries, bitsCleared, nsLast, nsMax} | null`.

- [ ] **Step 1: Append the ops ABI fields + type aliases**

In `core/src/v8host.rs`, next to the other fn-pointer aliases (directly above the `S2EngineOps` struct), add (use the file's existing `c_int` import/alias style — the file already uses `std::os::raw::c_int` in aliases and tests):

```rust
/// checktransmit slice: upsert the merged visibility mask for a serial-gated entity.
/// Returns 1 on success, 0 on a stale ref / full table / uninstalled hook / disabled descriptor.
pub type TransmitSetFn = extern "C" fn(index: std::os::raw::c_int, serial: std::os::raw::c_int, mask: u64) -> std::os::raw::c_int;
/// checktransmit slice: drop the entity's rule entry (1 removed, 0 absent).
pub type TransmitClearFn = extern "C" fn(index: std::os::raw::c_int) -> std::os::raw::c_int;
/// checktransmit slice: copy the hot-path counters into out[5] = {snapshots, entries, bitsCleared, nsLast, nsMax}.
pub type TransmitStatsFn = extern "C" fn(out: *mut u64);
```

At the END of `pub struct S2EngineOps` (immediately after `pub player_change_team: Option<PlayerChangeTeamFn>,`):

```rust
    // --- checktransmit slice (APPENDED after player_change_team; order is the ABI; do not reorder above) ---
    pub transmit_set:   Option<TransmitSetFn>,
    pub transmit_clear: Option<TransmitClearFn>,
    pub transmit_stats: Option<TransmitStatsFn>,
}
```

Then find EVERY full `S2EngineOps { ... }` struct literal in the test module (e.g. `mock_event_ops()` and any other literal that names every field — search `S2EngineOps {`) and add to each:

```rust
        transmit_set: None, transmit_clear: None, transmit_stats: None,
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /home/gkh/projects/s2script-checktransmit && cargo build --release -p s2script-core 2>&1 | tail -5`
Expected: `Finished` release build, no errors. (If a struct literal was missed, the compiler names it — fix and re-run.)

- [ ] **Step 3: Write the failing tests**

In the test module of `core/src/v8host.rs`, next to the sound-op tests, add mocks + tests. `EntityRef` construction in eval'd JS: `new (require("@s2script/entity").EntityRef)(index, serial)` — but a plain `{index, serial}` object satisfies the `Transmit` wrapper's duck-typing check, so tests use object literals.

```rust
    // --- checktransmit slice: __s2_transmit_* natives + the per-plugin rule store ---
    static TRANSMIT_SET_CALLS: std::sync::Mutex<Vec<(i32, i32, u64)>> = std::sync::Mutex::new(Vec::new());
    static TRANSMIT_CLEAR_CALLS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
    extern "C" fn mock_transmit_set(index: c_int, serial: c_int, mask: u64) -> c_int {
        TRANSMIT_SET_CALLS.lock().unwrap().push((index, serial, mask));
        1
    }
    extern "C" fn mock_transmit_set_reject(_index: c_int, _serial: c_int, _mask: u64) -> c_int { 0 }
    extern "C" fn mock_transmit_clear(index: c_int) -> c_int {
        TRANSMIT_CLEAR_CALLS.lock().unwrap().push(index);
        1
    }
    extern "C" fn mock_transmit_stats(out: *mut u64) {
        unsafe { for i in 0..5 { *out.add(i) = (i as u64 + 1) * 10; } }
    }
    fn transmit_test_ops() -> S2EngineOps {
        S2EngineOps {
            transmit_set: Some(mock_transmit_set),
            transmit_clear: Some(mock_transmit_clear),
            transmit_stats: Some(mock_transmit_stats),
            ..mock_event_ops()
        }
    }

    /// setVisibleTo folds the viewer-slot array into a u64 mask and pushes (index, serial, mask).
    #[test]
    fn transmit_set_folds_viewer_slots_into_mask() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tm1");
        let out = eval_in_context_string("tm1",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 7, serial: 42}, [0, 5, 63]))");
        assert_eq!(out, "true");
        let calls = TRANSMIT_SET_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (7, 42, 1u64 | (1u64 << 5) | (1u64 << 63)));
        drop(calls);
        shutdown();
    }

    /// Empty viewer array = hidden from everyone = mask 0.
    #[test]
    fn transmit_set_empty_array_masks_zero() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tm2");
        let out = eval_in_context_string("tm2",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 3, serial: 1}, []))");
        assert_eq!(out, "true");
        assert_eq!(TRANSMIT_SET_CALLS.lock().unwrap()[0], (3, 1, 0u64));
        shutdown();
    }

    /// A slot outside [0,64) throws RangeError from the JS wrapper (programmer error, not staleness).
    #[test]
    fn transmit_set_out_of_range_slot_throws() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tm3");
        let out = eval_in_context_string("tm3",
            "(function(){ try { __s2pkg_transmit.Transmit.setVisibleTo({index:1,serial:1},[64]); return 'no-throw'; } catch (e) { return e.constructor.name; } })()");
        assert_eq!(out, "RangeError");
        assert_eq!(TRANSMIT_SET_CALLS.lock().unwrap().len(), 0);
        shutdown();
    }

    /// Two plugins with rules on the same (index, serial) AND-merge: the pushed mask is the intersection.
    #[test]
    fn transmit_rules_and_merge_across_plugins() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tma");
        create_plugin_context("tmb");
        eval_in_context_string("tma",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 5, serial: 9}, [0, 1]))");
        eval_in_context_string("tmb",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 5, serial: 9}, [1, 2]))");
        let calls = TRANSMIT_SET_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], (5, 9, 0b11u64));          // tma alone
        assert_eq!(calls[1], (5, 9, 0b10u64));          // tma AND tmb = bit 1 only
        drop(calls);
        shutdown();
    }

    /// reset() removes only the caller's rule; the remaining merge is re-pushed; the LAST reset clears.
    #[test]
    fn transmit_reset_recomputes_then_clears() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        TRANSMIT_CLEAR_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tra");
        create_plugin_context("trb");
        eval_in_context_string("tra", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, serial: 9}, [0, 1])");
        eval_in_context_string("trb", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, serial: 9}, [1, 2])");
        let out = eval_in_context_string("tra", "String(__s2pkg_transmit.Transmit.reset({index: 5, serial: 9}))");
        assert_eq!(out, "true");
        assert_eq!(TRANSMIT_SET_CALLS.lock().unwrap().last().copied(), Some((5, 9, 0b110u64))); // trb alone
        let out = eval_in_context_string("trb", "String(__s2pkg_transmit.Transmit.reset({index: 5, serial: 9}))");
        assert_eq!(out, "true");
        assert_eq!(TRANSMIT_CLEAR_CALLS.lock().unwrap().as_slice(), &[5]);
        shutdown();
    }

    /// reset() with a serial that doesn't match the recorded rule returns false and pushes nothing.
    #[test]
    fn transmit_reset_serial_mismatch_is_false() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        TRANSMIT_CLEAR_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("trm");
        eval_in_context_string("trm", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, serial: 9}, [0])");
        let out = eval_in_context_string("trm", "String(__s2pkg_transmit.Transmit.reset({index: 5, serial: 8}))");
        assert_eq!(out, "false");
        assert_eq!(TRANSMIT_CLEAR_CALLS.lock().unwrap().len(), 0);
        shutdown();
    }

    /// Unloading a plugin clears its rules (the ledger walk): last owner gone -> transmit_clear pushed.
    #[test]
    fn transmit_unload_clears_owner_rules() {
        let _ = init(dummy_logger());
        TRANSMIT_CLEAR_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tun");
        eval_in_context_string("tun", "__s2pkg_transmit.Transmit.setVisibleTo({index: 11, serial: 2}, [0])");
        unload_plugin("tun");
        assert_eq!(TRANSMIT_CLEAR_CALLS.lock().unwrap().as_slice(), &[11]);
        shutdown();
    }

    /// A new rule with a NEWER live serial evicts other owners' stale-serial entries on the same index
    /// (the op validated the new serial is the live one, so the old one is a dead entity's rule).
    #[test]
    fn transmit_stale_serial_evicted_on_new_set() {
        let _ = init(dummy_logger());
        TRANSMIT_SET_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tsa");
        create_plugin_context("tsb");
        eval_in_context_string("tsa", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, serial: 1}, [0])");
        eval_in_context_string("tsb", "__s2pkg_transmit.Transmit.setVisibleTo({index: 5, serial: 2}, [1])");
        let calls = TRANSMIT_SET_CALLS.lock().unwrap();
        // Second push must NOT be ANDed with tsa's stale-serial mask.
        assert_eq!(calls[1], (5, 2, 1u64 << 1));
        drop(calls);
        // And tsa's stale entry is gone: resetting it now reports false.
        let out = eval_in_context_string("tsa", "String(__s2pkg_transmit.Transmit.reset({index: 5, serial: 1}))");
        assert_eq!(out, "false");
        shutdown();
    }

    /// Missing ops (old shim) degrade to false — never a throw.
    #[test]
    fn transmit_set_missing_op_degrades_false() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(mock_event_ops()));   // no transmit ops
        create_plugin_context("tmo");
        let out = eval_in_context_string("tmo",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 1, serial: 1}, [0]))");
        assert_eq!(out, "false");
        shutdown();
    }

    /// The op rejecting (stale ref / full table / disabled) -> false, and the rule is NOT recorded.
    #[test]
    fn transmit_set_op_reject_not_recorded() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps {
            transmit_set: Some(mock_transmit_set_reject),
            transmit_clear: Some(mock_transmit_clear),
            transmit_stats: Some(mock_transmit_stats),
            ..mock_event_ops()
        }));
        create_plugin_context("trj");
        let out = eval_in_context_string("trj",
            "String(__s2pkg_transmit.Transmit.setVisibleTo({index: 1, serial: 1}, [0]))");
        assert_eq!(out, "false");
        let out = eval_in_context_string("trj", "String(__s2pkg_transmit.Transmit.reset({index: 1, serial: 1}))");
        assert_eq!(out, "false");   // nothing was recorded
        shutdown();
    }

    /// stats() surfaces the op's out[5] as a plain numbers object.
    #[test]
    fn transmit_stats_surfaces_counters() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(transmit_test_ops()));
        create_plugin_context("tst");
        let out = eval_in_context_string("tst", "JSON.stringify(__s2pkg_transmit.Transmit.stats())");
        assert_eq!(out, r#"{"snapshots":10,"entries":20,"bitsCleared":30,"nsLast":40,"nsMax":50}"#);
        shutdown();
    }

    /// stats() with no op -> null (typed TransmitStats | null).
    #[test]
    fn transmit_stats_missing_op_is_null() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(mock_event_ops()));
        create_plugin_context("tsn");
        let out = eval_in_context_string("tsn", "String(__s2pkg_transmit.Transmit.stats())");
        assert_eq!(out, "null");
        shutdown();
    }
```

Adaptation note (not a placeholder — mechanical fit): if the harness helpers are spelled slightly differently in the test module (e.g. tests call `init` with a different logger helper, or `unload_plugin` takes a scope), mirror EXACTLY what the neighboring `sound_emit_marshals_args_to_op` test does for init/eval/teardown; the assertions above are the contract.

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p s2script-core transmit 2>&1 | tail -15`
Expected: all `transmit_*` tests FAIL (eval returns a ReferenceError string — `__s2pkg_transmit` undefined — so the `assert_eq!` mismatches).

- [ ] **Step 5: Implement the rule store + natives + prelude**

(a) Thread-local store + helpers, placed near the other slice thread-locals (e.g. below `OUTPUT_MUX`):

```rust
/// checktransmit slice: per-plugin entity-visibility rules. owner -> (entindex -> rule).
/// INVARIANT: all owners' entries for one index share ONE serial (enforced in s2_transmit_set —
/// the op validates the incoming serial is the live one, so different-serial entries are stale
/// and evicted). The shim holds only the AND-merged mask per index; this map is the policy source
/// of truth so unload/reset can recompute the merge.
#[derive(Clone, Copy)]
struct TransmitRule { serial: i32, mask: u64 }
thread_local! {
    static TRANSMIT_RULES: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashMap<i32, TransmitRule>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Recompute the AND-merged mask for `index` across every owner's rule and push it to the shim
/// (transmit_set), or clear the shim entry when no rule remains (transmit_clear).
fn transmit_recompute_and_push(index: i32) {
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
    let merged = TRANSMIT_RULES.with(|r| {
        let map = r.borrow();
        let mut acc: Option<TransmitRule> = None;
        for rules in map.values() {
            if let Some(rule) = rules.get(&index) {
                acc = Some(match acc {
                    None => *rule,
                    Some(a) => TransmitRule { serial: rule.serial, mask: a.mask & rule.mask },
                });
            }
        }
        acc
    });
    match merged {
        Some(rule) => { if let Some(f) = ops.transmit_set { f(index, rule.serial, rule.mask); } }
        None => { if let Some(f) = ops.transmit_clear { f(index); } }
    }
}

/// Unload/resetAll teardown: drop every rule owned by `owner`, re-pushing each affected index.
fn transmit_remove_owner(owner: &str) {
    let indices: Vec<i32> = TRANSMIT_RULES.with(|r| {
        r.borrow_mut().remove(owner).map(|m| m.keys().copied().collect()).unwrap_or_default()
    });
    for i in indices { transmit_recompute_and_push(i); }
}
```

(b) The four natives, placed with the other entity natives:

```rust
/// Native `__s2_transmit_set(index, serial, viewerSlots[]) -> boolean` — replace the calling
/// plugin's visibility rule for the entity: transmit ONLY to the given viewer slots (empty array
/// = hidden from everyone). The u64 mask is folded core-side from the JS number array (no BigInt
/// on any boundary). The shim op serial-gates at registration; stale ref / full table / missing
/// op / disabled descriptor degrade to `false`. Other owners' entries on this index with a
/// DIFFERENT serial are evicted after the op accepts (ours is the live serial; theirs are dead).
fn s2_transmit_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        if index < 0 || serial < 0 { return; }
        let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return };
        let mut mask: u64 = 0;
        for i in 0..arr.length() {
            let Some(v) = arr.get_index(scope, i) else { return };
            let slot = v.integer_value(scope).unwrap_or(-1);
            if !(0..64).contains(&slot) { return; }   // the JS wrapper throws first; belt-and-braces
            mask |= 1u64 << (slot as u32);
        }
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        // Candidate merged mask: AND with every OTHER owner's same-serial rule on this index.
        let merged = TRANSMIT_RULES.with(|r| {
            let map = r.borrow();
            let mut acc = mask;
            for (o, rules) in map.iter() {
                if o == &owner { continue; }
                if let Some(rule) = rules.get(&index) {
                    if rule.serial == serial { acc &= rule.mask; }
                }
            }
            acc
        });
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.transmit_set) else { return };
        if f(index, serial, merged) == 0 { return; }
        TRANSMIT_RULES.with(|r| {
            let mut map = r.borrow_mut();
            // Evict stale (different-serial) entries on this index — the op just validated `serial`
            // is the live one, so any other serial in this slot belongs to a dead entity.
            for rules in map.values_mut() {
                let stale = rules.get(&index).map_or(false, |ru| ru.serial != serial);
                if stale { rules.remove(&index); }
            }
            map.entry(owner).or_default().insert(index, TransmitRule { serial, mask });
        });
        rv.set_bool(true);
    }));
}

/// Native `__s2_transmit_reset(index, serial) -> boolean` — remove the calling plugin's rule for
/// the entity (the serial must match the recorded rule), then re-push the remaining merge (or
/// clear the shim entry when this was the last rule).
fn s2_transmit_reset(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let removed = TRANSMIT_RULES.with(|r| {
            let mut map = r.borrow_mut();
            match map.get_mut(&owner) {
                Some(rules) if rules.get(&index).map_or(false, |ru| ru.serial == serial) => {
                    rules.remove(&index);
                    true
                }
                _ => false,
            }
        });
        if removed {
            transmit_recompute_and_push(index);
            rv.set_bool(true);
        }
    }));
}

/// Native `__s2_transmit_reset_all()` — remove all of the calling plugin's rules.
fn s2_transmit_reset_all(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        transmit_remove_owner(&owner);
    }));
}

/// Native `__s2_transmit_stats() -> {snapshots, entries, bitsCleared, nsLast, nsMax} | null`.
/// Null when the op is unassigned (old shim) — the capability is absent, not zero.
fn s2_transmit_stats(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.transmit_stats) else { return };
        let mut out = [0u64; 5];
        f(out.as_mut_ptr());
        let obj = v8::Object::new(scope);
        for (i, name) in ["snapshots", "entries", "bitsCleared", "nsLast", "nsMax"].iter().enumerate() {
            let k = v8::String::new(scope, name).unwrap();
            let v = v8::Number::new(scope, out[i] as f64);
            obj.set(scope, k.into(), v.into());
        }
        rv.set(obj.into());
    }));
}
```

(c) Register the natives in `install_natives` next to the other `__s2_entity_*` registrations:

```rust
    // checktransmit slice: declarative per-client entity visibility rules (@s2script/transmit).
    set_native(scope, global_obj, "__s2_transmit_set", s2_transmit_set);
    set_native(scope, global_obj, "__s2_transmit_reset", s2_transmit_reset);
    set_native(scope, global_obj, "__s2_transmit_reset_all", s2_transmit_reset_all);
    set_native(scope, global_obj, "__s2_transmit_stats", s2_transmit_stats);
```

(d) Prelude module — inside `INJECTED_STD_PRELUDE`'s IIFE (before the `globalThis.__s2pkg_*` export block), add:

```js
  // @s2script/transmit — per-client entity visibility rules (checktransmit slice). Declarative:
  // the native side evaluates rules per snapshot; NO JS runs in the CheckTransmit hot path.
  var Transmit = {
    setVisibleTo: function (entity, viewers) {
      if (!entity || typeof entity.index !== "number" || typeof entity.serial !== "number") return false;
      if (!Array.isArray(viewers)) throw new TypeError("viewers must be an array of player slots");
      for (var i = 0; i < viewers.length; i++) {
        var s = viewers[i];
        if (typeof s !== "number" || (s | 0) !== s || s < 0 || s >= 64)
          throw new RangeError("viewer slot out of range [0,64): " + s);
      }
      return __s2_transmit_set(entity.index, entity.serial, viewers);
    },
    reset: function (entity) {
      if (!entity || typeof entity.index !== "number" || typeof entity.serial !== "number") return false;
      return __s2_transmit_reset(entity.index, entity.serial);
    },
    resetAll: function () { __s2_transmit_reset_all(); },
    stats: function () { return __s2_transmit_stats(); }
  };
```

and add to the export block (with the other `__s2pkg_*` lines):

```js
globalThis.__s2pkg_transmit  = { Transmit: Transmit };
```

(`__s2require("@s2script/transmit")` resolves `__s2pkg_transmit` automatically — no module list exists to update.)

(e) Teardown wiring:
- In `unload_plugin(id)`'s step-(a) block (where `OUTPUT_MUX.with(|m| ...remove_by_owner(id))` is called), add:

```rust
    // checktransmit slice: drop the plugin's visibility rules and re-push the remaining merges
    // (last-owner-gone entries are cleared from the shim table).
    transmit_remove_owner(id);
```

- In `shutdown()` (where the other thread-local muxes are reset so a re-init starts empty), add:

```rust
    TRANSMIT_RULES.with(|r| r.borrow_mut().clear());
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p s2script-core transmit 2>&1 | tail -15`
Expected: all `transmit_*` tests PASS.

Run: `cargo test -p s2script-core 2>&1 | tail -5`
Expected: the FULL suite passes (existing count + the new tests), 0 failed.

- [ ] **Step 7: Commit**

```bash
cd /home/gkh/projects/s2script-checktransmit
git add core/src/v8host.rs
git commit -m "feat(core): @s2script/transmit — per-plugin entity-visibility rules, AND-merge, transmit_* ops ABI

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 2: Shim — ABI mirror, gamedata, CheckTransmit hook + first-fire validation

**Files:**
- Modify: `shim/include/s2script_core.h` (op typedefs near the other `s2_*_fn` typedefs; struct tail after `s2_player_change_team_fn player_change_team;` ~line 359)
- Modify: `shim/src/s2script_mm.h` (forward decls ~line 30; members ~line 79-84)
- Modify: `shim/src/s2script_mm.cpp` (include block ~line 11; SH_DECL block ~line 117; statics near the entity-system statics ~line 136; op fns near the other `s2_*` op fns; Load() new acquisition block after the Source2GameClients block ~line 2433; ops assignment tail ~line 3152; Unload() removal block ~line 3236)
- Modify: `gamedata/core.gamedata.jsonc` (`interfaces` block ~line 25; `offsets` block ~line 33)

**Interfaces:**
- Consumes (from Task 1, exact Rust-side ABI to mirror): `transmit_set(index: c_int, serial: c_int, mask: u64) -> c_int`, `transmit_clear(index: c_int) -> c_int`, `transmit_stats(out: *mut u64)` appended after `player_change_team`.
- Consumes (existing shim internals): `LoadOffsets(GamedataPath(), "linuxsteamrt64", err)`, `GamedataResult(name, ok, reason)`, `ResolveEntityBySerial(index, serial)`, `s_trackedSignon[kMaxClientSlots]` / `kSignonNone`, `g_S2ScriptPlugin`, the `versions` map + `serverFactory` in `Load()`.
- Produces: the live `Source2GameEntities001` POST hook applying the rule table; boot log lines `interface OK: Source2GameEntities (...)` + `CheckTransmit hook installed ...`; first-fire log `transmit: CheckTransmitInfo layout VALIDATED (client int @576 = slot|entindex (slot+1))`; stats counters `{snapshots, entries, bitsCleared, nsLast, nsMax}` behind the `transmit_stats` op.

- [ ] **Step 1: Mirror the ops ABI in `shim/include/s2script_core.h`**

Near the other op typedefs (before the `S2EngineOps` struct), add:

```c
/* checktransmit slice: upsert the merged visibility mask for a serial-gated entity.
 * Returns 1 on success, 0 on a stale ref / full table / uninstalled hook / disabled descriptor. */
typedef int (*s2_transmit_set_fn)(int index, int serial, unsigned long long mask);
/* checktransmit slice: drop the entity's rule entry (1 removed, 0 absent). */
typedef int (*s2_transmit_clear_fn)(int index);
/* checktransmit slice: copy the hot-path counters into out[5] = {snapshots, entries, bitsCleared, nsLast, nsMax}. */
typedef void (*s2_transmit_stats_fn)(unsigned long long* out);
```

At the END of the `S2EngineOps` struct (immediately after `s2_player_change_team_fn player_change_team;`):

```c
    /* checktransmit slice — APPENDED after player_change_team; order is the ABI; do not reorder above. */
    s2_transmit_set_fn   transmit_set;
    s2_transmit_clear_fn transmit_clear;
    s2_transmit_stats_fn transmit_stats;
} S2EngineOps;
```

Field names/order MUST be byte-identical to Task 1's Rust tail: `transmit_set`, `transmit_clear`, `transmit_stats`.

- [ ] **Step 2: gamedata entries**

In `gamedata/core.gamedata.jsonc`, add to the `interfaces` block (after `"Source2GameClients"`):

```jsonc
    // ISource2GameEntities — backs the CheckTransmit POST hook (checktransmit slice: per-client
    // entity visibility). Interface string from eiface.h INTERFACEVERSION_SERVERGAMEENTS.
    "Source2GameEntities": "Source2GameEntities001"
```

(mind the comma on the previous line), and to the `offsets` block:

```jsonc
    // CCheckTransmitInfo: byte offset of the which-client int32. hl2sdk's CCheckTransmitInfo is
    // explicitly incomplete ("TODO" at iservernetworkable.h:44), so this is a borrowed constant —
    // CSSharp calls it m_nPlayerSlot, Swiftly m_nClientEntityIndex (they DISAGREE on semantics:
    // slot vs entindex=slot+1). Both the offset AND the semantics are validated at FIRST
    // CheckTransmit fire (fail-closed: no transmit bits are touched until validation passes; a
    // persistent mismatch prints a named gamedata FAIL and disables the descriptor). Re-derive on
    // the update treadmill if transmit filtering reports FAILED; see docs/re-strategy.md.
    "CheckTransmitInfo_clientEntityIndex": { "linuxsteamrt64": 576 },
```

- [ ] **Step 3: Header declarations in `shim/src/s2script_mm.h`**

With the other forward decls at the top:

```cpp
// Forward-declared for the CheckTransmit hook (checktransmit slice); full definitions
// (eiface.h / iservernetworkable.h / bitvec.h) live in s2script_mm.cpp. CBitVec's forward decl
// matches bitvec.h:414 (`template <int NUM_BITS> class CBitVec`); by-ref params in pure
// DECLARATIONS don't need the complete type.
class ISource2GameEntities;
class CCheckTransmitInfo;
struct Entity2Networkable_t;
template <int NUM_BITS> class CBitVec;
```

In `class S2ScriptPlugin`, next to the other `Hook_*` members:

```cpp
    // CheckTransmit POST hook (checktransmit slice) — per-client entity visibility filtering.
    // Applies the core-pushed rule table to each client's transmit bitvec; notify-only for the
    // engine (MRES_IGNORED) — the mutation is in-place on the info structs. `unsigned short`
    // == the SDK's uint16 (the META_NO_HL2SDK header convention, like the uint64 params above).
    void Hook_CheckTransmit(CCheckTransmitInfo** ppInfoList, int nInfoCount,
                            CBitVec<16384>& unionTransmitEdicts, CBitVec<16384>& unionTransmitEdicts2,
                            const Entity2Networkable_t** pNetworkables,
                            const unsigned short* pEntityIndices, int nEntityIndices);
```

Next to `m_gameClients` / the install flags:

```cpp
    ISource2GameEntities* m_gameEntities = nullptr;
    bool m_checkTransmitHookInstalled = false;     // checktransmit: the CheckTransmit POST hook
```

- [ ] **Step 4: Shim implementation in `shim/src/s2script_mm.cpp`**

(a) Includes — after `#include <eiface.h>` add:

```cpp
#include <iservernetworkable.h>  // CCheckTransmitInfo (m_pTransmitEntity @0) — checktransmit slice
```

and with the C/C++ std includes add (if not already present): `#include <unordered_map>` and `#include <ctime>`.

(b) SH_DECL — after the `StartupServer` SH_DECL block:

```cpp
// ISource2GameEntities::CheckTransmit (checktransmit slice) — per-client entity visibility. POST
// hook: the game has filled each client's transmit bitvec; we clear bits per the core-pushed rule
// table. Signature verbatim from OUR eiface.h:500 (7 args; the two CBitVec<16384>& are complete
// via bitvec.h, which eiface.h includes; Entity2Networkable_t stays an incomplete pointee — fine,
// SourceHook only sizeof's the pointer). SwiftlyS2 hooks this with the identical declared
// signature (their entrypoint.cpp:74) — corroboration; the vtable index comes from OUR pinned
// hl2sdk at compile time, exactly like the seven ISource2GameClients hooks.
SH_DECL_HOOK7_void(ISource2GameEntities, CheckTransmit, SH_NOATTRIB, 0, CCheckTransmitInfo**, int,
                   CBitVec<16384>&, CBitVec<16384>&, const Entity2Networkable_t**, const uint16*, int);
```

(c) Statics — near the entity-system statics (`s_pGameResourceService` block):

```cpp
// ---------------------------------------------------------------------------
// CheckTransmit (checktransmit slice) — the per-entity visibility rule table + layout validation.
// Rules are pushed by the core (transmit_set/transmit_clear ops; AND-merged per entity across
// plugins core-side); the POST hook applies them to each client's transmit bitvec with ZERO JS in
// the hot path. The one non-SDK layout fact (which client an info is for) is a gamedata offset
// validated at FIRST FIRE (the info structs exist only inside a live snapshot build, so boot-time
// validation is impossible): fail-closed — no bit is touched until validation passes; a
// persistent failure disables the descriptor with a named gamedata FAIL (degrade, never crash).
// ---------------------------------------------------------------------------
struct TransmitEntry { int serial; uint64_t mask; };
static std::unordered_map<int, TransmitEntry> s_transmitTable;   // entindex -> merged rule
static const size_t kTransmitTableCap = 4096;
static int  s_ctiClientOff = -1;   // CCheckTransmitInfo which-client int32 (gamedata; hint +576)
// Layout state: 0 = pending (observe only), 1 = validated, -1 = FAILED (descriptor disabled).
static int  s_transmitLayoutState = 0;
static bool s_transmitClientIsEntIndex = false;  // +off semantics: false = slot, true = entindex (slot+1)
static int  s_transmitValidateAttempts = 0;
static const int kTransmitValidateMaxAttempts = 512;  // snapshots to keep trying before FAILED
// Stats out[5]: snapshots, entries (read live), bitsCleared, nsLast, nsMax.
static uint64_t s_transmitSnapshots = 0, s_transmitBitsCleared = 0;
static uint64_t s_transmitNsLast = 0, s_transmitNsMax = 0;
```

(d) The three op fns — near the other `s2_*` op functions (they need `ResolveEntityBySerial`, so place them AFTER its definition):

```cpp
// transmit_set op: upsert the AND-merged visibility mask for (index, serial). Serial-gated at
// registration — a stale ref never enters the table. Returns 0 when the entity is stale, the
// table is at cap, the hook isn't installed, or the first-fire layout validation FAILED.
static int s2_transmit_set(int index, int serial, unsigned long long mask) {
    if (!g_S2ScriptPlugin.m_checkTransmitHookInstalled || s_transmitLayoutState < 0) return 0;
    if (index < 0 || serial < 0) return 0;
    if (!ResolveEntityBySerial(index, serial)) return 0;
    auto it = s_transmitTable.find(index);
    if (it == s_transmitTable.end() && s_transmitTable.size() >= kTransmitTableCap) return 0;
    s_transmitTable[index] = TransmitEntry{serial, static_cast<uint64_t>(mask)};
    return 1;
}
static int s2_transmit_clear(int index) {
    return s_transmitTable.erase(index) > 0 ? 1 : 0;
}
static void s2_transmit_stats(unsigned long long* out) {
    if (!out) return;
    out[0] = s_transmitSnapshots;
    out[1] = static_cast<unsigned long long>(s_transmitTable.size());
    out[2] = s_transmitBitsCleared;
    out[3] = s_transmitNsLast;
    out[4] = s_transmitNsMax;
}
```

(e) First-fire layout validation + the hook body — place with the other hook handler bodies (they need `s_trackedSignon`/`kSignonNone`, defined at ~line 625):

```cpp
// First-fire layout validation (re-strategy Rule 2 for call-context-only facts). Decides the
// semantics of the which-client int32 at s_ctiClientOff (slot vs entindex=slot+1 — CSSharp and
// Swiftly disagree) by checking which interpretation maps EVERY info to a connected slot
// (s_trackedSignon, maintained by the client lifecycle hooks). With a single connected client the
// answer is unambiguous: v==0 is impossible in entindex mode (entindex 0 = worldspawn); v==1 with
// slot 1 empty is impossible in slot mode. Also sanity-checks m_pTransmitEntity (offset 0, the
// one field hl2sdk DOES guarantee): non-null and worldspawn bit 0 set. Returns 1 = validated
// (mode cached), 0 = undecidable this snapshot (stay pending), -1 = hard mismatch.
static int TransmitValidateLayout(CCheckTransmitInfo** ppInfoList, int nInfoCount) {
    if (nInfoCount <= 0) return 0;
    bool slotModeOk = true, entIndexModeOk = true;
    for (int i = 0; i < nInfoCount; i++) {
        const uint8_t* raw = reinterpret_cast<const uint8_t*>(ppInfoList[i]);
        if (!raw) return -1;
        const CBitVec<16384>* bv = ppInfoList[i]->m_pTransmitEntity;
        if (!bv || !bv->IsBitSet(0)) return -1;     // worldspawn must always transmit
        int v = *reinterpret_cast<const int32_t*>(raw + s_ctiClientOff);
        if (v < 0 || v > 64) return -1;
        bool vSlotOk = (v >= 0 && v < kMaxClientSlots && s_trackedSignon[v] != kSignonNone);
        bool vEntOk  = (v >= 1 && (v - 1) < kMaxClientSlots && s_trackedSignon[v - 1] != kSignonNone);
        slotModeOk     = slotModeOk && vSlotOk;
        entIndexModeOk = entIndexModeOk && vEntOk;
    }
    if (slotModeOk == entIndexModeOk) return 0;     // both or neither -> retry next snapshot
    s_transmitClientIsEntIndex = entIndexModeOk;
    return 1;
}

void S2ScriptPlugin::Hook_CheckTransmit(CCheckTransmitInfo** ppInfoList, int nInfoCount,
                                        CBitVec<16384>&, CBitVec<16384>&,
                                        const Entity2Networkable_t**, const uint16*, int) {
    s_transmitSnapshots++;
    if (!ppInfoList || nInfoCount <= 0) RETURN_META(MRES_IGNORED);
    if (s_transmitLayoutState == 0) {               // fail-closed gate: observe-only until validated
        int r = TransmitValidateLayout(ppInfoList, nInfoCount);
        if (r == 1) {
            s_transmitLayoutState = 1;
            META_CONPRINTF("[s2script] transmit: CheckTransmitInfo layout VALIDATED (client int @%d = %s)\n",
                           s_ctiClientOff, s_transmitClientIsEntIndex ? "entindex (slot+1)" : "slot");
        } else if (r == -1 || ++s_transmitValidateAttempts >= kTransmitValidateMaxAttempts) {
            s_transmitLayoutState = -1;
            s_transmitTable.clear();
            META_CONPRINTF("[s2script]   gamedata FAIL  CheckTransmitInfo_clientEntityIndex — first-fire "
                           "validation %s (offset %d wrong for this build? re-derive; see "
                           "docs/re-strategy.md); transmit filtering DISABLED\n",
                           (r == -1) ? "MISMATCH" : "UNDECIDABLE", s_ctiClientOff);
        }
        RETURN_META(MRES_IGNORED);                  // never mutate on the validating snapshot
    }
    if (s_transmitLayoutState != 1 || s_transmitTable.empty()) RETURN_META(MRES_IGNORED);

    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    // Entries outer, infos inner: serial-gate each entry ONCE per snapshot (a single lookup — no
    // TOCTOU window inside the snapshot), then apply to every client's bitvec. A failed resolve
    // means the entity is gone FOREVER (serials never come back), so the entry is evicted —
    // the table is self-cleaning across deaths and map changes.
    for (auto it = s_transmitTable.begin(); it != s_transmitTable.end(); ) {
        if (!ResolveEntityBySerial(it->first, it->second.serial)) {
            it = s_transmitTable.erase(it);
            continue;
        }
        const int      entIndex = it->first;
        const uint64_t mask     = it->second.mask;
        for (int i = 0; i < nInfoCount; i++) {
            uint8_t* raw = reinterpret_cast<uint8_t*>(ppInfoList[i]);
            if (!raw) continue;
            int v = *reinterpret_cast<const int32_t*>(raw + s_ctiClientOff);
            int slot = s_transmitClientIsEntIndex ? (v - 1) : v;
            if (slot < 0 || slot >= 64) continue;
            if ((mask >> slot) & 1ull) continue;    // visible to this viewer — leave the bit alone
            CBitVec<16384>* bv = ppInfoList[i]->m_pTransmitEntity;
            if (bv && bv->IsBitSet(entIndex)) { bv->Clear(entIndex); s_transmitBitsCleared++; }
        }
        ++it;
    }
    clock_gettime(CLOCK_MONOTONIC, &t1);
    uint64_t ns = (uint64_t)(t1.tv_sec - t0.tv_sec) * 1000000000ull
                + (uint64_t)t1.tv_nsec - (uint64_t)t0.tv_nsec;
    s_transmitNsLast = ns;
    if (ns > s_transmitNsMax) s_transmitNsMax = ns;
    RETURN_META(MRES_IGNORED);
}
```

(f) Load() — insert a new acquisition block immediately AFTER the `Source2GameClients` block (same `serverFactory`, same degrade discipline):

```cpp
        // Acquire ISource2GameEntities + install the CheckTransmit POST hook (checktransmit
        // slice): per-client entity visibility filtering. A sibling of the Source2GameClients
        // acquisition — same serverFactory, same degrade-never-crash. The hook only OBSERVES
        // until the first-fire layout validation passes (see Hook_CheckTransmit).
        {
            auto it = versions.find("Source2GameEntities");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : INTERFACEVERSION_SERVERGAMEENTS;
            int ret = 0;
            m_gameEntities = serverFactory
                ? reinterpret_cast<ISource2GameEntities*>(serverFactory(verStr, &ret)) : nullptr;
            std::string ctiErr;
            auto ctiOffsets = LoadOffsets(GamedataPath(), "linuxsteamrt64", ctiErr);
            auto cit = ctiOffsets.find("CheckTransmitInfo_clientEntityIndex");
            s_ctiClientOff = (ctiErr.empty() && cit != ctiOffsets.end() && cit->second >= 0)
                                 ? cit->second : -1;
            GamedataResult("CheckTransmitInfo_clientEntityIndex", s_ctiClientOff >= 0,
                           "offset key absent from gamedata");
            // Reset the per-Load hook state (a shim reload starts a fresh validation cycle).
            s_transmitTable.clear();
            s_transmitLayoutState = 0;
            s_transmitValidateAttempts = 0;
            s_transmitSnapshots = 0; s_transmitBitsCleared = 0;
            s_transmitNsLast = 0; s_transmitNsMax = 0;
            if (m_gameEntities && ret == 0 && s_ctiClientOff >= 0) {
                META_CONPRINTF("[s2script] interface OK: Source2GameEntities (%s)\n", verStr);
                SH_ADD_HOOK(ISource2GameEntities, CheckTransmit, m_gameEntities,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_CheckTransmit), true);
                m_checkTransmitHookInstalled = true;
                META_CONPRINTF("[s2script] CheckTransmit hook installed (entity visibility; "
                               "layout validates on first fire)\n");
            } else if (!m_gameEntities || ret != 0) {
                m_gameEntities = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: Source2GameEntities (%s) — "
                               "transmit filtering off\n", verStr);
            } else {
                META_CONPRINTF("[s2script] WARN: CheckTransmitInfo offset not in gamedata — "
                               "transmit filtering off\n");
            }
        }
```

Note: if `GamedataResult` / `LoadOffsets` are defined later in the file than this insertion point, they are file-statics declared above `Load()` (GamedataResult ~line 1683, LoadOffsets is in gamedata.h) — both are in scope; no forward decl needed.

(g) Ops assignment — at the END of the `ops.*` assembly block (immediately after `ops.player_change_team = &s2_player_change_team;`):

```cpp
    // checktransmit slice — APPENDED after player_change_team; order MUST match S2EngineOps.
    ops.transmit_set   = &s2_transmit_set;
    ops.transmit_clear = &s2_transmit_clear;
    ops.transmit_stats = &s2_transmit_stats;
```

(h) Unload() — next to the client lifecycle hook removal block:

```cpp
    // Remove the CheckTransmit POST hook (checktransmit slice) + drop the rule table.
    if (m_checkTransmitHookInstalled && m_gameEntities) {
        SH_REMOVE_HOOK(ISource2GameEntities, CheckTransmit, m_gameEntities,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_CheckTransmit), true);
        m_checkTransmitHookInstalled = false;
    }
    s_transmitTable.clear();
```

- [ ] **Step 5: Build and verify**

Run: `cd /home/gkh/projects/s2script-checktransmit && make core 2>&1 | tail -3 && make shim 2>&1 | tail -5`
Expected: both build clean (cargo `Finished`, cmake `[100%] ... s2script.so`). Common failure: the SH_DECL param list not matching the header's virtual EXACTLY — compare against `third_party/hl2sdk/public/eiface.h:500` verbatim.

Run: `make check-boundary`
Expected: `PASS` (nothing here touches `games/*`).

Run: `cargo test -p s2script-core 2>&1 | tail -3`
Expected: full suite still passes.

- [ ] **Step 6: Commit**

```bash
cd /home/gkh/projects/s2script-checktransmit
git add shim/include/s2script_core.h shim/src/s2script_mm.h shim/src/s2script_mm.cpp gamedata/core.gamedata.jsonc
git commit -m "feat(shim): CheckTransmit POST hook — per-client entity visibility w/ first-fire layout validation

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 3: `packages/transmit` types package + changeset

**Files:**
- Create: `packages/transmit/package.json`
- Create: `packages/transmit/index.d.ts`
- Create: `.changeset/checktransmit-transmit.md`

**Interfaces:**
- Consumes: `EntityRef` (class) from `packages/entity/index.d.ts` (version `0.3.0`); the runtime module shape from Task 1 (`{ Transmit }` with `setVisibleTo`/`reset`/`resetAll`/`stats`).
- Produces: `import { Transmit, TransmitStats } from "@s2script/transmit"` for Task 4 and future consumers (TTT).

- [ ] **Step 1: Create `packages/transmit/package.json`**

```json
{
  "name": "@s2script/transmit",
  "version": "0.0.0",
  "types": "index.d.ts",
  "description": "Type stubs for the @s2script/transmit injected API (per-client entity visibility filtering). No runtime code.",
  "publishConfig": {
    "access": "public"
  },
  "files": [
    "index.d.ts"
  ],
  "dependencies": {
    "@s2script/entity": "0.3.0"
  },
  "repository": {
    "type": "git",
    "url": "https://github.com/GabeHirakawa/s2script.git"
  }
}
```

(Version `0.0.0` + a `minor` changeset is the changesets new-package convention → first publish lands as `0.1.0`.)

- [ ] **Step 2: Create `packages/transmit/index.d.ts`**

```typescript
/**
 * @s2script/transmit — per-client entity visibility filtering (the Source 2 `CheckTransmit`
 * path), injected by the runtime; NO runtime code here. Declarative and engine-generic: a plugin
 * registers per-entity viewer rules and the native side evaluates them per snapshot — zero JS
 * runs in the hot path. Rules are per-plugin, serial-gated (`EntityRef` semantics: a stale ref
 * degrades, never crashes), AND-merged across plugins (an entity transmits to a viewer only if
 * EVERY plugin holding a rule on it allows that viewer), and ledgered (unload/reload removes the
 * plugin's rules). An entity with no rule transmits normally — "this plugin filters nothing"
 * costs nothing.
 */
import type { EntityRef } from "@s2script/entity";

export interface TransmitStats {
  /** CheckTransmit invocations observed since the shim loaded. */
  snapshots: number;
  /** Live rule entries in the native table (merged across all plugins). */
  entries: number;
  /** Total transmit bits cleared since load. */
  bitsCleared: number;
  /** Nanoseconds spent applying rules in the most recent snapshot. */
  nsLast: number;
  /** Worst-case nanoseconds for a single snapshot since load. */
  nsMax: number;
}

export declare const Transmit: {
  /**
   * Replace this plugin's visibility rule for `entity`: transmit it ONLY to the given viewer
   * slots (empty array = hidden from everyone). Returns `false` on a stale ref, a full rule
   * table, or when the capability is unavailable/disabled. Throws `RangeError` on a slot outside
   * `[0, 64)` and `TypeError` on a non-array. Scope note: this filters ENTITIES (icons, props,
   * beams) — do not transmit-hide a player's own pawn/controller from them (client-crash
   * territory; this API does not guard it).
   */
  setVisibleTo(entity: EntityRef, viewers: readonly number[]): boolean;
  /**
   * Remove this plugin's rule for `entity` (the ref's serial must match the recorded rule —
   * a stale ref returns `false`). The entity is visible to all again as far as this plugin is
   * concerned; other plugins' rules still apply.
   */
  reset(entity: EntityRef): boolean;
  /** Remove ALL of this plugin's rules. */
  resetAll(): void;
  /** Hot-path counters (measurement/debugging), or `null` when the capability is unavailable. */
  stats(): TransmitStats | null;
};
```

- [ ] **Step 3: Create `.changeset/checktransmit-transmit.md`**

```markdown
---
"@s2script/transmit": minor
---

New package: `@s2script/transmit` — per-client entity visibility filtering (`CheckTransmit`).

Declarative rules evaluated natively per snapshot (zero JS in the hot path): `Transmit.setVisibleTo(entity, viewerSlots)` transmits the entity only to the listed slots, `Transmit.reset(entity)` / `Transmit.resetAll()` remove rules, `Transmit.stats()` exposes hot-path counters. Rules are serial-gated `EntityRef`s, AND-merged across plugins, and ledgered (unload cleans up). The engine fact is a POST SourceHook on `ISource2GameEntities::CheckTransmit` with the one non-SDK layout offset validated fail-closed at first fire. This is the TTT-blocking primitive (per-role icon visibility).
```

- [ ] **Step 4: Run the typecheck gate**

Run: `cd /home/gkh/projects/s2script-checktransmit && ./scripts/check-plugins-typecheck.sh 2>&1 | tail -3`
Expected: `PASS: all plugins and examples typecheck` (the new package is inert until a consumer imports it; this proves it doesn't break resolution).

- [ ] **Step 5: Commit**

```bash
cd /home/gkh/projects/s2script-checktransmit
git add packages/transmit .changeset/checktransmit-transmit.md
git commit -m "feat(types): @s2script/transmit package + changeset

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 4: `examples/transmit-demo` live-gate plugin

**Files:**
- Create: `examples/transmit-demo/package.json`
- Create: `examples/transmit-demo/tsconfig.json`
- Create: `examples/transmit-demo/src/plugin.ts`

**Interfaces:**
- Consumes: `Transmit` from `@s2script/transmit` (Task 3), `createEntity`/`EntityRef` from `@s2script/entity`, `Commands`/`CommandContext` from `@s2script/commands` (`ctx.argInt(n, fallback)`, `ctx.reply(msg)`).
- Produces: rcon/chat commands `sm_tspawn`, `sm_thide <slot>`, `sm_tonly <slot>`, `sm_tshow`, `sm_tstats` used by the live gate.

- [ ] **Step 1: Create `examples/transmit-demo/package.json`**

```json
{
  "name": "@demo/transmit-demo",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x" }
}
```

- [ ] **Step 2: Create `examples/transmit-demo/tsconfig.json`**

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/globals/globals.d.ts"]
}
```

- [ ] **Step 3: Create `examples/transmit-demo/src/plugin.ts`**

```typescript
// Live-gate demo for @s2script/transmit (per-client entity visibility). rcon/chat-driven:
//   sm_tspawn        -> spawn a labelled point_worldtext at a fixed spot
//   sm_thide <slot>  -> hide it from that viewer slot (visible to everyone else)
//   sm_tonly <slot>  -> show it ONLY to that slot
//   sm_tshow         -> reset this plugin's rule (visible to all)
//   sm_tstats        -> print Transmit.stats()
// Server-side proof of mutation = bitsCleared grows while a hide rule is active and the entity is
// in a viewer's PVS (the game set the bit; we cleared it). Client-side visual proof (the text
// pops in/out for exactly the filtered client) is the human check.
import { Commands } from "@s2script/commands";
import { createEntity, EntityRef } from "@s2script/entity";
import { Transmit } from "@s2script/transmit";

let prop: EntityRef | null = null;
const ALL_SLOTS: number[] = [];
for (let i = 0; i < 64; i++) ALL_SLOTS.push(i);

Commands.register("sm_tspawn", (ctx) => {
  const e = createEntity("point_worldtext", {
    message: "S2 TRANSMIT TEST",
    enabled: true,
    fullbright: true,
  });
  if (!e) { ctx.reply("[transmit] createEntity failed"); return; }
  e.spawn();
  e.teleport([0, 0, 128], [0, 0, 0], null);   // fixed, findable spot
  prop = e;
  ctx.reply("[transmit] spawned point_worldtext index=" + e.index + " serial=" + e.serial);
});

Commands.register("sm_thide", (ctx) => {
  if (!prop || !prop.isValid()) { ctx.reply("[transmit] no live prop — sm_tspawn first"); return; }
  const slot = ctx.argInt(0, -1);
  if (slot < 0 || slot >= 64) { ctx.reply("[transmit] usage: sm_thide <slot 0-63>"); return; }
  const ok = Transmit.setVisibleTo(prop, ALL_SLOTS.filter((s) => s !== slot));
  ctx.reply("[transmit] hide from slot " + slot + " -> " + ok);
});

Commands.register("sm_tonly", (ctx) => {
  if (!prop || !prop.isValid()) { ctx.reply("[transmit] no live prop — sm_tspawn first"); return; }
  const slot = ctx.argInt(0, -1);
  if (slot < 0 || slot >= 64) { ctx.reply("[transmit] usage: sm_tonly <slot 0-63>"); return; }
  const ok = Transmit.setVisibleTo(prop, [slot]);
  ctx.reply("[transmit] visible ONLY to slot " + slot + " -> " + ok);
});

Commands.register("sm_tshow", (ctx) => {
  if (!prop) { ctx.reply("[transmit] no prop"); return; }
  ctx.reply("[transmit] reset -> " + Transmit.reset(prop));
});

Commands.register("sm_tstats", (ctx) => {
  const s = Transmit.stats();
  if (!s) { ctx.reply("[transmit] stats unavailable (capability off)"); return; }
  ctx.reply("[transmit] snapshots=" + s.snapshots + " entries=" + s.entries +
    " bitsCleared=" + s.bitsCleared + " nsLast=" + s.nsLast + " nsMax=" + s.nsMax);
});

export function onLoad(): void {
  console.log("[transmit-demo] onLoad — sm_tspawn/sm_thide/sm_tonly/sm_tshow/sm_tstats registered");
}
export function onUnload(): void {}
```

- [ ] **Step 4: Typecheck + build the `.s2sp`**

Run: `cd /home/gkh/projects/s2script-checktransmit && ./scripts/check-plugins-typecheck.sh 2>&1 | grep -A2 transmit-demo`
Expected: `=== typecheck examples/transmit-demo/ ...` then `  OK`.

Run: `( cd packages/cli && npm link >/dev/null 2>&1 ); npx s2script build examples/transmit-demo 2>&1 | tail -3`
Expected: a built `.s2sp` at `examples/transmit-demo/dist/` (name derived from the plugin id). If `npm link` is unavailable in this environment, `node --experimental-strip-types packages/cli/src/cli.ts build examples/transmit-demo` per the CLI's own entry (check `packages/cli/package.json` `bin`); the artifact path is what matters.

- [ ] **Step 5: Commit**

```bash
cd /home/gkh/projects/s2script-checktransmit
git add examples/transmit-demo
git commit -m "feat(examples): transmit-demo — live-gate driver for @s2script/transmit

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 5: Full gate suite

**Files:** none created — verification only (fix regressions if any gate fails, then re-run).

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: a green gate baseline the live gate builds on.

- [ ] **Step 1: Run the full gate suite**

```bash
cd /home/gkh/projects/s2script-checktransmit
make all 2>&1 | tail -3
cargo test -p s2script-core 2>&1 | tail -3
make check-boundary
./scripts/check-plugins-typecheck.sh 2>&1 | tail -1
./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh && ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh
./scripts/test-boundary-nameleak.sh
```

Expected: every command green — `make all` finishes packaging into `dist/addons/s2script`; cargo suite `0 failed`; `check-boundary` PASS; typecheck `PASS: all plugins and examples typecheck`; codegen-freshness + nameleak checks pass (this slice regenerates nothing, so they must be untouched-green).

- [ ] **Step 2: Commit anything the gates required (only if fixes were needed)**

```bash
cd /home/gkh/projects/s2script-checktransmit
git status --short   # expect clean; if fixes were made:
git add -A && git commit -m "fix(checktransmit): gate-suite fixes

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 6: Live gate (orchestrator-run — requires the Docker CS2 server)

Not dispatched to an implementer subagent; run by the orchestrator per the spec §6. Recorded here so the plan is complete.

- [ ] Sniper build: `docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`
- [ ] Deploy demo: copy `examples/transmit-demo/dist/*.s2sp` into `dist/addons/s2script/plugins/`; recreate `dist/addons/s2script/configs` as the host user if the sniper build reset ownership.
- [ ] `make docker-test` (or `docker compose -f docker/docker-compose.yml up -d`), `docker exec s2script-cs2 /patch-gameinfo.sh`, `docker compose -f docker/docker-compose.yml restart cs2` (NEVER `--force-recreate`).
- [ ] Boot evidence: console shows `interface OK: Source2GameEntities (Source2GameEntities001)`, `CheckTransmit hook installed`, gamedata banner includes `gamedata OK    CheckTransmitInfo_clientEntityIndex`.
- [ ] Validation evidence: after a client is connected, `transmit: CheckTransmitInfo layout VALIDATED (client int @576 = ...)`. Determine whether bots alone trigger CheckTransmit; if not, flag the human-client dependency in the report.
- [ ] Functional evidence via `python3 scripts/rcon.py`: `sm_tspawn` → `sm_thide 0` → `sm_tstats` shows `entries=1` and `bitsCleared` GROWING across two samples; `sm_tshow` → `bitsCleared` stops growing.
- [ ] Perf evidence: `sm_tstats` → record `nsLast`/`nsMax`; budget < 50 µs/snapshot.
- [ ] Teardown evidence: remove the demo `.s2sp` → file-watch unload → a fresh `sm_tstats` from a second load (or console) shows `entries=0`.
- [ ] Append the finished-slice entry to `docs/PROGRESS.md` and update `CLAUDE.md`'s capability inventory line — AFTER the live gate passes.

---

## Self-Review (performed per superpowers:writing-plans)

1. **Spec coverage:** §2.1 hook point/acquisition → Task 2 steps 2-4(f); §2.2 layout validation fail-closed + named disable → Task 2 step 4(c,e); §2.3 fallbacks → Task 1 step 5 (op-missing degrade) + Task 2 step 4(f) WARN paths; §3 API + mask semantics + AND-merge + ledger → Task 1; §3.2 module/package → Tasks 1(d), 3; §4 ops ABI/table/cap/stats → Tasks 1-2; §5 boundary → Task 5 (`check-boundary`); §6 live gate → Task 6; §7 tests → Task 1 step 3, Task 5. No gaps found.
2. **Placeholder scan:** no TBD/TODO/"similar to"; the two "adaptation notes" (Task 1 step 3 harness spelling, Task 4 step 4 npm-link fallback) name exact alternatives rather than deferring decisions.
3. **Type consistency:** `transmit_set(index i32/int, serial i32/int, mask u64/unsigned long long) -> c_int/int` identical across Task 1 Rust aliases, Task 2 C typedefs, and both struct tails; JS surface `setVisibleTo/reset/resetAll/stats` identical across prelude (Task 1), `.d.ts` (Task 3), demo (Task 4); stats key order `{snapshots, entries, bitsCleared, nsLast, nsMax}` identical across the op contract, the native, the test's JSON assertion, and the `.d.ts`.
