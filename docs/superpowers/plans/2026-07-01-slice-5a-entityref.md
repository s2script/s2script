# Slice 5A — Handle / EntityRef System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the raw entity pointer a plugin holds with an `EntityRef` (`{index, serial}`) that validates against the engine's own serial on every access and returns `T | null` — closing the Slice-3 use-after-free where a stashed `Pawn` reads freed memory.

**Architecture:** JS holds only `{index, serial}`; the read/write natives take `(index, serial, offset)` and do `ent_by_index` → serial-validate → deref → read/write entirely inside core, returning a value or `null`. No raw pointer crosses to JS. Validation compares the engine's current `CEntityIdentity` serial (read via core-held offset constants) against the ref's serial. `EntityRef` is the engine-generic `@s2script/std` primitive; `Pawn` (`@s2script/cs2`) holds one.

**Tech Stack:** Rust `cdylib` core (rusty_v8, v8 crate 149.4.0), the `S2EngineOps` C-ABI fn-pointer table (existing `ent_by_index`/`ent_state_changed` reused), `@s2script/std`/`@s2script/cs2` injected JS + `.d.ts`, Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-07-01-slice-5a-entityref-design.md`.

## Global Constraints

Every task's requirements implicitly include these (from spec §13):

- **Core stays engine-generic.** No CS2 identifiers, no `include_str!`/`include_bytes!`, no `games/` in `core/src`. Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS). `entity.rs` + the new natives read a `CEntityIdentity` *layout* + do handle bit-math — no game-class knowledge. The `EntityRef` primitive lives in `@s2script/std`; CS2 class/field names live only in `games/cs2/js/pawn.js`.
- **Never expose a raw pointer across time.** A raw pointer never crosses to JS on the safe path — JS holds only `{index, serial}`; every deref is serial-gated. Raw escape is `unsafe`-only (deferred, not built).
- **Degrade-never-crash.** Every native `catch_unwind`-wrapped; no panic crosses the FFI boundary; invalid/mismatch → read `null` / write `false` / `state_changed` no-op — **never a stale deref**.
- **Layout is data, semantics are code.** Engine-struct offsets are named constants now with a `// TODO(gamedata):` migration comment; the `CEntityHandle` bit-split lives in `entity.rs` as a named Source 2 constant.
- **Naming:** PascalCase events + types (`EntityRef`, `Pawn`), camelCase functions + properties (`isValid`, `readInt32`, `forSlot`, `health`).
- **cdylib test constraint:** unit tests are inline `#[cfg(test)] mod` in the source file — never `core/tests/`.
- **Commit trailer:** every commit ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5a-entityref`; do not push.

**Deferred — do NOT build:** the raw-live block-scoped fast-path (5A.1); the inter-plugin `EntityRef` wire integration (fast-follow); non-`i32` field types (5B codegen); migrating engine offsets to a gamedata file (treadmill tooling); a first-class `unsafe` module; the `tsc` gate; 5B/5C; config/permissions/reload-state-handoff; the registry (5.5); the base-plugin suite (6).

---

## File Structure

- **Modify `core/src/entity.rs`** — add the pure decode + resolve logic (`read_u32`, `read_ptr`, `decode_handle`, `resolve`, the `HANDLE_ENTRY_BITS` constant) next to the existing `read_i32`/`write_i32`. Unit-tested.
- **Modify `core/src/v8host.rs`** — add the six `(index, serial)` natives + install them; add the engine-struct offset constants (`ENT_IDENTITY_PTR_OFFSET`, `ENT_IDENTITY_HANDLE_OFFSET`); later delete the raw-pointer natives.
- **Modify `packages/std/index.d.ts`** + the `@s2script/std` prelude in `core/src/v8host.rs` (`INJECTED_STD_PRELUDE`) — the `EntityRef` primitive.
- **Modify `games/cs2/js/pawn.js`** + `packages/cs2/index.d.ts` — `Pawn` refactored EntityRef-backed.
- **Modify `README.md`, `CLAUDE.md`.**
- **Create** the spike-findings doc; use/extend `examples/demo-plugin` for the live gate.

The engine-touchpoint unknowns (the exact `CEntityIdentity` offsets + the `CEntityHandle` bit-split) are produced by Task 1 (spike) and consumed as named constants by Tasks 2–3.

---

## Task 1: Spike — the entity-identity serial + handle bit-split (RECON, throwaway, LIVE)

**Files:**
- Create: `docs/superpowers/specs/2026-07-01-slice-5a-spike-findings.md`
- Scratch (temporary): a throwaway native/log in `core/src/v8host.rs` + a throwaway demo, removed at the end.

**Interfaces:**
- Consumes: the existing `ops.ent_by_index(idx) -> *mut c_void` (Slice 3), the entity system reachable from it; `scripts/rcon.py`, the Docker CS2 server, `scripts/build-sniper.sh`, `docker/patch-gameinfo.sh`.
- Produces: three confirmed constants for Tasks 2–3 — `ENT_IDENTITY_PTR_OFFSET` (byte offset within `CEntityInstance` of its `CEntityIdentity*`), `ENT_IDENTITY_HANDLE_OFFSET` (byte offset within `CEntityIdentity` of the `CEntityHandle`/serial-bearing `uint32`), and `HANDLE_ENTRY_BITS` (the index/serial bit-split). Plus the invalid-handle sentinel. No production code.

This is LIVE reverse-engineering, like Slices 3/4/4.5. Escalation: if the live infra won't cooperate after reasonable attempts, report BLOCKED with the exact commands/errors so the controller can drive it.

- [ ] **Step 1: Reach the entity's `CEntityIdentity` from an entity pointer.** For a known live entity (a player controller at index `slot+1`), take `ops.ent_by_index(idx)` → pointer, and probe the first ~0x40 bytes for a pointer that leads to a struct whose `uint32` at some offset decodes to `(index==idx, plausible serial)`. Log candidate `(ENT_IDENTITY_PTR_OFFSET, ENT_IDENTITY_HANDLE_OFFSET)` pairs. (Known: the entity system is at `IGameResourceService + 0x50`; CS2 exports no `GetEntityIdentity`, so this is a memory probe.)

- [ ] **Step 2: Confirm the `CEntityHandle` bit-split.** From the `uint32` found in Step 1, determine `HANDLE_ENTRY_BITS` such that `handle & ((1<<BITS)-1) == idx` for several different indices, and the high bits are a small increasing serial. Record `HANDLE_ENTRY_BITS` and the invalid-handle sentinel (commonly `0xFFFFFFFF`).

- [ ] **Step 3: Confirm validity detection on death/respawn.** Capture a pawn's serial, then let the pawn die (`mp_restartgame 1` / a round end / forced), and confirm that reading the serial for that same entity index now yields a DIFFERENT serial (or the slot is empty), so a stale `{index, serial}` would fail the equality check. This is the whole point — record the observed before/after.

- [ ] **Step 4: Write findings.** Fill `docs/superpowers/specs/2026-07-01-slice-5a-spike-findings.md` with the three confirmed constants (exact values), the invalid-handle sentinel, the death/respawn serial-change evidence, and a **GO/NO-GO**. If NO-GO, state the blocker and stop for re-design.

- [ ] **Step 5: Remove scratch, commit the findings doc only.**

```bash
cargo test -p s2script-core -- --test-threads=1   # unchanged count (scratch removed)
git add docs/superpowers/specs/2026-07-01-slice-5a-spike-findings.md
git commit -m "docs(slice5a): spike findings — CEntityIdentity serial offsets + CEntityHandle bit-split

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: `entity.rs` — pure decode + resolve (PURE / cargo-unit)

**Files:**
- Modify: `core/src/entity.rs` (add below the existing `write_i32`, tests into the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: the `HANDLE_ENTRY_BITS` value from the Task-1 findings doc.
- Produces (used by v8host in Task 3):
  - `pub const HANDLE_ENTRY_BITS: u32` — the CS2 `CEntityHandle` index/serial bit-split (from spike).
  - `pub fn read_u32(base: *const u8, offset: i32) -> u32` — 0 on null/negative.
  - `pub fn read_ptr(base: *const u8, offset: i32) -> *const u8` — null on null/negative.
  - `pub fn decode_handle(handle: u32) -> (i32, i32)` — `(index, serial)`.
  - `pub fn resolve(current_serial: i32, ref_serial: i32) -> bool` — true iff both valid (`>= 0`) and equal.

- [ ] **Step 1: Write the failing tests** (append to `#[cfg(test)] mod tests` in `entity.rs`):

```rust
    #[test]
    fn decode_handle_is_inverse_of_encode() {
        // BITS-agnostic proof the bit-math is a correct inverse (the exact BITS value is
        // validated live in the gate; here we prove decode∘encode == identity for that split).
        let bits = HANDLE_ENTRY_BITS;
        let encode = |index: u32, serial: u32| (serial << bits) | (index & ((1 << bits) - 1));
        for &(i, s) in &[(0u32, 0u32), (1, 1), (64, 3), ((1 << bits) - 1, 7)] {
            let (di, ds) = decode_handle(encode(i, s));
            assert_eq!(di, i as i32, "index round-trips");
            assert_eq!(ds, s as i32, "serial round-trips");
        }
    }

    #[test]
    fn resolve_matches_only_equal_nonneg_serials() {
        assert!(resolve(5, 5));
        assert!(!resolve(5, 6), "mismatch (reused slot) is invalid");
        assert!(!resolve(-1, -1), "empty slot (-1) is never valid");
        assert!(!resolve(-1, 5));
        assert!(!resolve(5, -1));
    }

    #[test]
    fn read_u32_and_read_ptr_guard_null_and_negative() {
        assert_eq!(read_u32(std::ptr::null(), 4), 0);
        assert_eq!(read_u32(std::ptr::null(), -4), 0);
        assert!(read_ptr(std::ptr::null(), 8).is_null());
        assert!(read_ptr(&0u8 as *const u8, -8).is_null());
    }

    #[test]
    fn read_u32_reads_at_offset() {
        #[repr(C)]
        struct Fake { pad: [u8; 4], handle: u32 }
        let f = Fake { pad: [0; 4], handle: 0xDEAD_BEEF };
        let base = &f as *const Fake as *const u8;
        assert_eq!(read_u32(base, 4), 0xDEAD_BEEF);
    }

    #[test]
    fn read_ptr_reads_a_pointer_field() {
        let target: u8 = 42;
        #[repr(C)]
        struct Fake { pad: [u8; 8], p: *const u8 }
        let f = Fake { pad: [0; 8], p: &target as *const u8 };
        let base = &f as *const Fake as *const u8;
        let got = read_ptr(base, 8);
        assert!(!got.is_null());
        assert_eq!(unsafe { *got }, 42);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core entity:: -- --test-threads=1`
Expected: FAIL — `HANDLE_ENTRY_BITS`/`decode_handle`/`resolve`/`read_u32`/`read_ptr` not found.

- [ ] **Step 3: Implement** (add below `write_i32` in `entity.rs`; set `HANDLE_ENTRY_BITS` to the Task-1 spike value):

```rust
/// CS2 `CEntityHandle` index/serial bit-split (NUM_ENT_ENTRY_BITS). Confirmed by the Slice-5A spike;
/// see docs/superpowers/specs/2026-07-01-slice-5a-spike-findings.md.
// TODO(gamedata): migrate to a regenerable gamedata file with the other engine-struct facts.
pub const HANDLE_ENTRY_BITS: u32 = 15; // <-- SET FROM SPIKE FINDINGS

/// Read a u32 at `base + offset`. Returns 0 on a null base or negative offset (degrade-safe).
pub fn read_u32(base: *const u8, offset: i32) -> u32 {
    if base.is_null() || offset < 0 {
        return 0;
    }
    // SAFETY: caller supplies a live entity pointer + a fixed in-struct offset.
    unsafe { *(base.add(offset as usize) as *const u32) }
}

/// Read a pointer field at `base + offset`. Returns null on a null base or negative offset.
pub fn read_ptr(base: *const u8, offset: i32) -> *const u8 {
    if base.is_null() || offset < 0 {
        return std::ptr::null();
    }
    // SAFETY: caller supplies a live entity pointer + a fixed in-struct offset.
    unsafe { *(base.add(offset as usize) as *const *const u8) }
}

/// Decode a `CEntityHandle` uint32 into `(index, serial)` using the CS2 bit-split.
pub fn decode_handle(handle: u32) -> (i32, i32) {
    let index = (handle & ((1u32 << HANDLE_ENTRY_BITS) - 1)) as i32;
    let serial = (handle >> HANDLE_ENTRY_BITS) as i32;
    (index, serial)
}

/// True iff a captured `ref_serial` still matches the entity system's `current_serial` for that
/// index. Both must be valid (`>= 0`); an empty slot reports `-1` and never matches.
pub fn resolve(current_serial: i32, ref_serial: i32) -> bool {
    current_serial >= 0 && ref_serial >= 0 && current_serial == ref_serial
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p s2script-core entity:: -- --test-threads=1`
Expected: PASS (existing 2 + 5 new).

- [ ] **Step 5: Full suite + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/entity.rs
git commit -m "feat(slice5a): entity.rs — CEntityHandle decode + serial resolve + read_u32/read_ptr

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

Expected: green; both gates pass (no CS2 ids added).

---

## Task 3: v8host — the six `(index, serial)` natives (in-isolate degrade + live)

**Files:**
- Modify: `core/src/v8host.rs` (offset constants near the other engine constants; six new native fns near the Slice-3 entity natives; installs in `install_natives`)

**Interfaces:**
- Consumes: `entity::{read_i32, write_i32, read_u32, read_ptr, decode_handle, resolve, HANDLE_ENTRY_BITS}`; the existing `ENGINE_OPS` table (`ops.ent_by_index`, `ops.ent_state_changed`); `set_native`.
- Produces (called by the prelude in Task 4): natives `__s2_ent_current_serial(index) -> number`, `__s2_ent_ref_valid(index, serial) -> boolean`, `__s2_ent_ref_read_i32(index, serial, offset) -> number|null`, `__s2_ent_ref_write_i32(index, serial, offset, value) -> boolean`, `__s2_ent_ref_state_changed(index, serial, offset)`, `__s2_handle_decode(handleValue) -> [index, serial]`.

The raw-pointer natives stay in place this task (deleted in Task 4 once `pawn.js` no longer uses them).

- [ ] **Step 1: Write the failing test** (append to `#[cfg(test)] mod frame_tests` in `v8host.rs`). With no engine ops wired, the natives must DEGRADE safely (this is the only part testable without a live entity system):

```rust
    #[test]
    fn ent_ref_natives_degrade_without_engine_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops table → every entity op is a safe miss
        create_plugin_context("p");
        // current_serial → -1 ; valid → false ; read → null ; write → false ; state_changed → no-op/undefined
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_current_serial(1))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_valid(1, 7))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_i32(1, 7, 8))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_i32(1, 7, 8, 5))"), "false");
        // handle_decode is PURE (no ops needed). BITS-agnostic assertion: 64 < 2^7 <= 2^HANDLE_ENTRY_BITS,
        // so index==64, serial==0 for any real bit-split (the exact split is validated live in the gate).
        assert_eq!(eval_in_context_string("p", "var d=__s2_handle_decode(64); d[0]+','+d[1]"), "64,0");
        shutdown();
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core frame_tests::ent_ref_natives_degrade_without_engine_ops -- --test-threads=1`
Expected: FAIL — the natives don't exist yet.

- [ ] **Step 3: Add the offset constants** (near the top-of-file engine constants / `S2EngineOps`):

```rust
/// Byte offset within a `CEntityInstance` of its `CEntityIdentity*` (spike-confirmed).
// TODO(gamedata): migrate to a regenerable gamedata file.
const ENT_IDENTITY_PTR_OFFSET: i32 = 0x10; // <-- SET FROM SPIKE FINDINGS
/// Byte offset within a `CEntityIdentity` of the `CEntityHandle` uint32 (index+serial) (spike-confirmed).
const ENT_IDENTITY_HANDLE_OFFSET: i32 = 0x10; // <-- SET FROM SPIKE FINDINGS
```

- [ ] **Step 4: Add an internal serial reader + the six natives.** A private helper turns an index into its current serial via the existing `ent_by_index` op (pointer used + discarded inside — never surfaced to JS):

```rust
/// Current serial for an entity index via the engine's identity, or -1 if the slot is empty / no ops.
/// The raw pointer is used and discarded HERE — it never crosses to JS.
fn entity_current_serial(index: i32) -> i32 {
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return -1 };
    let Some(by_index) = ops.ent_by_index else { return -1 };
    let ent = by_index(index) as *const u8;
    if ent.is_null() { return -1; }
    let identity = crate::entity::read_ptr(ent, ENT_IDENTITY_PTR_OFFSET);
    if identity.is_null() { return -1; }
    let handle = crate::entity::read_u32(identity, ENT_IDENTITY_HANDLE_OFFSET);
    let (_idx, serial) = crate::entity::decode_handle(handle);
    serial
}

/// Resolve (index, serial) to a live entity pointer, or null if the serial no longer matches.
/// Raw pointer stays in Rust — callers read/write through it and discard it within the native.
fn entity_resolve_ptr(index: i32, serial: i32) -> *mut u8 {
    if !crate::entity::resolve(entity_current_serial(index), serial) {
        return std::ptr::null_mut();
    }
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return std::ptr::null_mut() };
    let Some(by_index) = ops.ent_by_index else { return std::ptr::null_mut() };
    by_index(index) as *mut u8
}

fn s2_ent_current_serial(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        rv.set_int32(entity_current_serial(index));
    }));
}

fn s2_ent_ref_valid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        rv.set_bool(crate::entity::resolve(entity_current_serial(index), serial));
    }));
}

fn s2_ent_ref_read_i32(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → null (already set)
        rv.set_int32(crate::entity::read_i32(ent as *const u8, off));
    }));
}

fn s2_ent_ref_write_i32(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let val = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → false (already set)
        crate::entity::write_i32(ent, off, val);
        rv.set_bool(true);
    }));
}

fn s2_ent_ref_state_changed(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as c_int;
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }               // invalid → no-op
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.ent_state_changed else { return };
        func(ent as *mut c_void, off);
    }));
}

fn s2_handle_decode(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(0) as u32;
        let (index, serial) = crate::entity::decode_handle(handle);
        let arr = v8::Array::new(scope, 2);
        let i = v8::Integer::new(scope, index);
        let s = v8::Integer::new(scope, serial);
        arr.set_index(scope, 0, i.into());
        arr.set_index(scope, 1, s.into());
        rv.set(arr.into());
    }));
}
```

- [ ] **Step 5: Install the six natives** (in `install_natives`, after the existing entity natives):

```rust
    set_native(scope, global_obj, "__s2_ent_current_serial", s2_ent_current_serial);
    set_native(scope, global_obj, "__s2_ent_ref_valid", s2_ent_ref_valid);
    set_native(scope, global_obj, "__s2_ent_ref_read_i32", s2_ent_ref_read_i32);
    set_native(scope, global_obj, "__s2_ent_ref_write_i32", s2_ent_ref_write_i32);
    set_native(scope, global_obj, "__s2_ent_ref_state_changed", s2_ent_ref_state_changed);
    set_native(scope, global_obj, "__s2_handle_decode", s2_handle_decode);
```

- [ ] **Step 6: Run the test + full suite + gates**

Run: `cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`
Expected: green (incl. `ent_ref_natives_degrade_without_engine_ops`; gates pass — the natives read a layout, no CS2 ids).

- [ ] **Step 7: Commit**

```bash
git add core/src/v8host.rs
git commit -m "feat(slice5a): (index,serial) entity natives — serial-gated read/write/valid/decode

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: `EntityRef` (`@s2script/std`) + `Pawn` refactor (`@s2script/cs2`) + delete raw natives (in-isolate + live)

**Files:**
- Modify: `core/src/v8host.rs` — add `EntityRef` to `INJECTED_STD_PRELUDE`; DELETE the raw-pointer natives (`s2_entity_by_index`, `s2_deref_handle`, `s2_ent_read_i32`, `s2_ent_write_i32`, `s2_ent_state_changed`) + their `install_natives` lines.
- Modify: `packages/std/index.d.ts` (EntityRef type), `games/cs2/js/pawn.js` (Pawn EntityRef-backed), `packages/cs2/index.d.ts` (Pawn.forSlot/health types).

**Interfaces:**
- Consumes: Task-3 natives.
- Produces: `EntityRef` on `@s2script/std` (`{ index, serial, isValid(), readInt32(offset), writeInt32(offset, value), notifyStateChanged(offset) }`); `Pawn` EntityRef-backed.

- [ ] **Step 1: Write the failing test** (append to `frame_tests`). With no ops, an `EntityRef` from `@s2script/std` degrades safely:

```rust
    #[test]
    fn entity_ref_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_plugin_js("er", r#"
            const { EntityRef } = require("@s2script/std");
            const ref = new EntityRef(1, 7);
            globalThis.__valid = String(ref.isValid());       // "false"
            globalThis.__read  = String(ref.readInt32(8));    // "null"
            globalThis.__write = String(ref.writeInt32(8, 5));// "false"
        "#);
        assert_eq!(read_global_string("er", "__valid"), "false");
        assert_eq!(read_global_string("er", "__read"), "null");
        assert_eq!(read_global_string("er", "__write"), "false");
        shutdown();
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core frame_tests::entity_ref_degrades_without_ops -- --test-threads=1`
Expected: FAIL — `EntityRef` not exported from `@s2script/std`.

- [ ] **Step 3: Add `EntityRef` to the `@s2script/std` prelude.** Inside `INJECTED_STD_PRELUDE`'s IIFE, define `EntityRef` and put it on `std`:

```js
  function EntityRef(index, serial) { this.index = index; this.serial = serial; }
  EntityRef.prototype = {
    isValid: function () { return __s2_ent_ref_valid(this.index, this.serial); },
    readInt32: function (offset) { return __s2_ent_ref_read_i32(this.index, this.serial, offset); },
    writeInt32: function (offset, value) { return __s2_ent_ref_write_i32(this.index, this.serial, offset, value); },
    notifyStateChanged: function (offset) { __s2_ent_ref_state_changed(this.index, this.serial, offset); },
  };
  std.EntityRef = EntityRef;
```

- [ ] **Step 4: Refactor `games/cs2/js/pawn.js` to be EntityRef-backed.** Replace the raw-`ent` `Pawn` with an EntityRef-backed one (CS2 class/field names stay here):

```javascript
(function () {
  var EntityRef = require("@s2script/std").EntityRef;

  function Pawn(ref, healthOff) { this.ref = ref; this.healthOff = healthOff; }
  Pawn.prototype = {
    get health() { return this.ref.readInt32(this.healthOff); },        // number | null
    set health(v) {
      if (this.ref.writeInt32(this.healthOff, v)) this.ref.notifyStateChanged(this.healthOff);
    },
  };

  // slot -> controller entity (index slot+1) -> m_hPlayerPawn handle -> pawn EntityRef.
  Pawn.forSlot = function (slot) {
    var HEALTH = __s2_schema_offset("CCSPlayerPawn", "m_iHealth");
    var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    if (HEALTH < 0 || PAWN_HANDLE < 0) return null;

    var ctrlIndex = slot + 1;
    var ctrl = new EntityRef(ctrlIndex, __s2_ent_current_serial(ctrlIndex));
    if (!ctrl.isValid()) return null;

    var handle = ctrl.readInt32(PAWN_HANDLE);           // the m_hPlayerPawn CEntityHandle uint32
    if (handle === null) return null;
    var decoded = __s2_handle_decode(handle >>> 0);      // [index, serial]
    var pawn = new EntityRef(decoded[0], decoded[1]);
    return pawn.isValid() ? new Pawn(pawn, HEALTH) : null;
  };

  globalThis.__s2pkg_cs2 = { Pawn: Pawn };
})();
```

- [ ] **Step 5: Update the `.d.ts` stubs.** In `packages/std/index.d.ts` add:

```ts
export class EntityRef {
  readonly index: number;
  readonly serial: number;
  constructor(index: number, serial: number);
  isValid(): boolean;
  readInt32(offset: number): number | null;
  writeInt32(offset: number, value: number): boolean;
  notifyStateChanged(offset: number): void;
}
```
In `packages/cs2/index.d.ts`, ensure `Pawn.forSlot(slot: number): Pawn | null` and `health` is `number | null` on read.

- [ ] **Step 6: Delete the raw-pointer natives.** Remove the fns `s2_entity_by_index`, `s2_deref_handle`, `s2_ent_read_i32`, `s2_ent_write_i32`, `s2_ent_state_changed` and their five `set_native(...)` lines in `install_natives`. Keep `entity::read_i32`/`write_i32` (used internally by Task-3 natives) and the `S2EngineOps` fields (`ent_by_index`, `ent_state_changed` used internally; `deref_handle` now unused — leave the field, add `// unused since 5A (EntityRef path)`; removing an ABI field is a shim-contract change, deferred).

- [ ] **Step 7: Run the test + full suite + gates**

Run: `cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`
Expected: green (incl. `entity_ref_degrades_without_ops`; no reference to a deleted native remains — grep `__s2_entity_by_index`/`__s2_deref_handle`/`__s2_ent_read_i32`/`__s2_ent_write_i32`/`__s2_ent_state_changed` finds no callers).

- [ ] **Step 8: Commit**

```bash
git add core/src/v8host.rs games/cs2/js/pawn.js packages/std/index.d.ts packages/cs2/index.d.ts
git commit -m "feat(slice5a): EntityRef in @s2script/std; Pawn EntityRef-backed; retire raw-pointer natives

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 5: Demo + host-invalidation LIVE GATE + README + CLAUDE (LIVE-ONLY)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts` (add the stash-across-death probe), `README.md`, `CLAUDE.md`.

**Interfaces:**
- Consumes: the whole 5A stack (Tasks 2–4).
- Produces: the host-invalidation acceptance proven live.

- [ ] **Step 1: Extend the demo to stash a Pawn across time.** In `examples/demo-plugin/src/plugin.ts`, on load capture a `Pawn` and, every ~256 frames, read the STASHED pawn's health (proving a held ref stays safe) plus a fresh `forSlot`:

```ts
import { OnGameFrame } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

let stashed: Pawn | null = null;
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    if (!stashed) stashed = Pawn.forSlot(0);        // stash once
    const fresh = Pawn.forSlot(0);
    console.log("[demo] tick " + ticks
      + " stashed.health=" + (stashed ? stashed.health : "none")   // null once that pawn died
      + " fresh.health=" + (fresh ? fresh.health : "none"));       // works again after respawn
    if (stashed && stashed.health === null) { stashed = null; }     // re-stash next tick
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```

- [ ] **Step 2: Build the demo `.s2sp` + the sniper runtime.**

```bash
cd /home/gkh/projects/s2script
node packages/cli/build.mjs
npx s2script build examples/demo-plugin
bash scripts/build-sniper.sh
```
Expected: a `.s2sp` + `s2script.so` (GLIBC ≤ 2.30). If a CS2 update reset `gameinfo.gi`, run `bash docker/patch-gameinfo.sh` first.

- [ ] **Step 3: Run the host-invalidation LIVE GATE on Docker CS2.** Bring up the server, drop the demo, add a bot, get the map ticking (`bot_quota 1`, `sv_hibernate_when_empty 0`), then via `scripts/rcon.py` + the container logs:
  1. Load → `[demo] tick … stashed.health=100 fresh.health=100` (a live pawn reads).
  2. Kill the stashed pawn (`mp_restartgame 1`, or a round death) → the stashed pawn's entity is destroyed → the NEXT tick logs `stashed.health=null` (the serial no longer matches — **not** garbage, **not** a crash); the server keeps ticking (`Up` in `docker ps`).
  3. Respawn → `fresh.health=100` again (a fresh `forSlot` gets the new serial); the demo re-stashes.
  Capture the excerpts. If the live infra genuinely won't cooperate after reasonable attempts, report BLOCKED with the exact commands/errors so the controller can drive it.

- [ ] **Step 4: README + CLAUDE.md.** Add a `## Safe entities — the EntityRef guardrail (Slice 5A)` section to `README.md` (the build→load→kill→respawn runbook + the captured `stashed.health=null` log + an acceptance table). Update `CLAUDE.md` "## Current state": 5A complete + merged, the Slice-3 raw-`ent` UAF closed, `EntityRef` = `{index,serial}` validated per access; "Current focus: Slice 5B next" (schema codegen). Do NOT alter the standing conventions above it.

- [ ] **Step 5: Final verification + commit** (do NOT commit build artifacts — `.s2sp`/`dist`/`.so` are gitignored):

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md
git commit -m "feat(slice5a): host-invalidation live gate — stashed Pawn goes null on entity death; README/CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §10)

1. `cargo test -p s2script-core` green (existing + new `entity.rs` unit tests + the two in-isolate degrade tests); both boundary gates green; sniper build OK.
2. `s2script build` produces a loadable demo `.s2sp` using the EntityRef-backed `Pawn`.
3. The host-invalidation live gate passes on Docker CS2: read/write health; entity death → **stashed `Pawn.health` → `null`, no crash**, server keeps ticking; respawn → fresh `forSlot` works with the new serial.
4. README documents the runbook + acceptance; CLAUDE.md "Current state" updated (5A done, raw-`ent` UAF closed, focus → 5B).
