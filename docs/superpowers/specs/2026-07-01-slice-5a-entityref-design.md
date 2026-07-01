# Slice 5A — Handle / EntityRef System (entity-safety spine)

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5a-entityref` (off `main`, which has Slices 0–4.5 merged).
**Parent:** Slice 5, decomposed into 5A (this — handle/EntityRef), 5B (schema codegen), 5C (`@s2script/std` breadth). 5A is built first as the load-bearing spine.
**Prior art in-repo:** the Slice-3 schema/entity work in `core/src/v8host.rs` + `games/cs2/js/pawn.js`.

---

## 1. Goal — the closing thread

Make the "**never expose a raw pointer or raw cross-plugin reference across time**" guardrail real for
entities. A plugin holds an `EntityRef` (`{index, serial}` — plain data), never a raw pointer; every
entity field access validates the serial against the engine's entity system and returns `T | null`
(null once the entity is destroyed or its slot reused by a *different* entity). A stashed `Pawn` whose
entity has died reads `null`, never freed memory. Proven on the live CS2 server: read/write health,
then the pawn dies → the stashed `Pawn.health` reads `null`, the server keeps ticking.

This is the SourceMod entity-reference / Handle model, done type-safe.

## 2. What we build on (Slice 3, merged) — and the hazard it left

Slice 3 shipped `Pawn.health` (`games/cs2/js/pawn.js`) and the entity/schema natives in
`core/src/v8host.rs`:
- `__s2_schema_offset(class, field)` → offset (live SchemaSystem resolve, base-class walk, OffsetCache).
- `__s2_entity_by_index(index)` → **raw `CEntityInstance` pointer**.
- `__s2_deref_handle(handle)` → **raw pointer**.
- `__s2_ent_read_i32(ptr, offset)` / `__s2_ent_write_i32(ptr, offset, v)` / `__s2_ent_state_changed(ptr, offset)`.

`Pawn` holds `this.ent` — a **raw pointer** captured at `forSlot` time. Reading `pawn.health` after
that entity is destroyed is a **use-after-free**; Slice 3 only avoided it by reading within a single
tick. Known engine facts (from the repo's memory + Slice 3): CS2 exports no `GetEntityIdentity`; the
entity system lives at `IGameResourceService + 0x50`; schema fields are inherited (walk base classes);
offsets resolve at map-live, not Load.

## 3. Decisions locked during brainstorming

1. **Scope = safe accessors only.** Every access validates the serial and returns `T | null` (the
   SourceMod `GetEntProp` model — re-validate every call). The block-scoped **raw-live fast-path**
   (validate once → raw pointer → many plain reads, epoch-stamped so it can't cross `await`) is the
   guardrail's *eventual* form but is **deferred to 5A.1** — 5A proves the safe substrate first.
2. **Inter-plugin `EntityRef`-on-the-wire = deferred to a fast-follow.** 5A is single-plugin. Because
   an `EntityRef` is already serializable `{index, serial}`, it will round-trip over the existing 4.5
   structured-copy wire cheaply later (tag + re-wrap); no wire work in 5A.
3. **Validation via the engine's own serial (Approach A).** An `EntityRef` is `{index, serial}`; every
   access reads the engine's `CEntityIdentity[index]` serial and compares. Rejected alternatives: a
   core-owned generational handle table (duplicates a serial the engine already maintains, needs
   entity-lifecycle hooks); index-only re-lookup (can't distinguish "gone" from "different entity
   reused the index" → silent wrong-entity reads).

## 4. Architecture — Approach A: engine-serial validation, no raw pointer on the safe path

**The safety property that drives everything:** a raw pointer *never leaves core*. JS holds only an
`EntityRef` = `{index, serial}`. The read/write natives take `(index, serial, offset)` and do
validate → deref → read/write entirely inside core, returning a value or `null`. There is no
"get me the pointer" native on the safe path — that is what makes a stashed ref impossible to turn
into a use-after-free.

Core stays **engine-generic**: it reads a `CEntityIdentity` *layout* (offsets) and does
`CEntityHandle` bit-math — no game-class name. The CS2 class/field names (`CCSPlayerController`,
`m_hPlayerPawn`, `CCSPlayerPawn`, `m_iHealth`) live only in `games/cs2/js/pawn.js`.

## 5. Core native surface (engine-generic)

New `(index, serial)`-based natives, each `catch_unwind`-wrapped and degrade-never-crash:
- `__s2_ent_current_serial(index)` → serial (`-1` if the slot is empty) — used to *capture* a fresh `EntityRef`.
- `__s2_ent_ref_valid(index, serial)` → bool (serial matches the live identity).
- `__s2_ent_ref_read_i32(index, serial, offset)` → number | null.
- `__s2_ent_ref_write_i32(index, serial, offset, value)` → bool (false if invalid).
- `__s2_ent_ref_state_changed(index, serial, offset)` → no-op if invalid.
- `__s2_handle_decode(handleValue)` → `{index, serial}` — decodes a `CEntityHandle` uint32 (e.g. read
  from `m_hPlayerPawn`) into an `EntityRef`. Pure bit-math.

The Slice-3 **raw-pointer-returning natives** (`__s2_entity_by_index` → bare pointer,
`__s2_deref_handle`, `__s2_ent_read_i32(ptr,…)`, `__s2_ent_write_i32(ptr,…)`,
`__s2_ent_state_changed(ptr,…)`) are **deleted from the JS surface** — per CLAUDE.md, raw pointers live
only behind the explicit `unsafe` module (not built in 5A). Their underlying Rust logic (the
entity-system chunk-walk that turns an index into a live `CEntityInstance` pointer) is **reused
internally** by the new `(index, serial)` natives: each does index→pointer + serial-validate + deref +
read/write within a single native call and discards the pointer — it never crosses to JS.

The engine-struct offsets (entity-system `+0x50` (existing), the `CEntityIdentity` serial field, the
`CEntityHandle` index/serial bit-split) are named engine-generic Source 2 constants in core, with a
`TODO(gamedata)` to migrate to a regenerable gamedata file when the treadmill tooling lands ("layout
is data, semantics are code").

## 6. `EntityRef` (`@s2script/std`) + `Pawn` (`@s2script/cs2`) + `T | null` semantics

**`@s2script/std` (engine-generic)** — the `EntityRef` primitive holds `{index, serial}` and exposes:
- `isValid()` → boolean
- `readInt32(offset)` → number | null
- `writeInt32(offset, value)` → boolean
- `notifyStateChanged(offset)` → void

It knows entities have index+serial (true of any Source 2 game), nothing about CS2.

**`@s2script/cs2` (game)** — `Pawn` refactored to hold an `EntityRef` (not a raw `ent`). The schema
offsets + the CS2 controller→pawn convention stay here.

**`T | null` semantics (author-facing):**
```js
const p = Pawn.forSlot(0);   // Pawn | null   (null if no such player / schema not ready)
p.health;                     // number | null (null once the pawn entity is destroyed/reused)
p.health = 50;                // no-op if the entity is gone — never a UAF
p.ref.isValid();              // boolean, explicit check
```

## 7. Data flow (`Pawn.forSlot(0)` then a later `p.health`)

1. `forSlot(slot)`: capture `controllerRef = { slot+1, __s2_ent_current_serial(slot+1) }`; if invalid
   → null. `controllerRef.readInt32(m_hPlayerPawn)` → a handle uint32 → `__s2_handle_decode` →
   `pawnRef`. If `pawnRef.isValid()` → `new Pawn(pawnRef, HEALTH)`, else null. (CS2 convention: player
   controllers occupy entity index `slot+1`, confirmed in the Slice-3/4 live gates.)
2. Later `p.health`: `pawnRef.readInt32(HEALTH)` → core validates `identity[index].serial === serial`;
   match → deref → read → number; mismatch (pawn died/respawned) → **null**. No pointer ever surfaces
   to JS.

## 8. Front-loaded spike (the engine-touchpoint unknown)

Validate on a live server before the load-bearing work; findings to a dated
`docs/superpowers/specs/…-slice-5a-spike-findings.md`:
1. From the entity system (`IGameResourceService + 0x50` → `CGameEntitySystem`), locate the
   `CEntityIdentity` array, index it by entity index, and read the current **serial**. Confirm against
   a live entity.
2. The `CEntityHandle` **bit-split** for CS2 (index bits vs serial bits) — needed to decode
   `m_hPlayerPawn`.
3. **Validity detection:** capture a serial, let the entity die/respawn, confirm the serial changes
   (identity invalidates) so a stale `EntityRef` resolves to `null`.

If any of these don't hold as expected, revise before the load-bearing tasks.

## 9. Error handling — degrade-never-crash

Per "degrade per-descriptor, never crash globally": every native is `catch_unwind`-wrapped (no panic
crosses the FFI boundary). Invalid index, null identity, serial mismatch, or schema-not-ready → read
returns `null`, write returns `false` (no-op), `state_changed` no-ops. Core **never dereferences a
stale/freed pointer** — validation gates every deref. `Pawn.forSlot` returns `null` when the schema
isn't loaded yet or the player/pawn doesn't exist.

## 10. Testing & acceptance

**Cargo-unit-testable** (new `core/src/entity.rs`, pure + engine-generic, inline `#[cfg(test)] mod`):
- `CEntityHandle` **decode** (bit-math): a packed uint32 → `{index, serial}`, round-trips.
- The serial-**compare** decision: `resolve(identity_serial, ref_serial) → valid?` incl. the
  empty-slot / mismatch cases.

**Live-only (the acceptance thread** — the memory reads need a real entity system, as in Slice 3):
the **host-invalidation live gate** — `Pawn.forSlot(0)` reads `health` (e.g. 100) and writes it; then
the bot dies (natural round death or forced via rcon) → the **stashed** `Pawn`'s `health` reads
**`null`** (not garbage, not a crash), the server keeps ticking; on respawn a fresh `forSlot` works
again with the new serial.

**Acceptance criteria:**
1. `cargo test -p s2script-core` green (existing + the new `entity.rs` unit tests); both boundary gates
   green (`check-core-boundary.sh`, `test-boundary-nameleak.sh`); sniper build OK.
2. `s2script build` produces a loadable demo `.s2sp` using the EntityRef-backed `Pawn`.
3. The host-invalidation live gate passes on the Docker CS2 server (read/write health; entity death →
   stashed `Pawn.health` → `null`, no crash; respawn → fresh `forSlot` works).
4. README documents the runbook + acceptance; CLAUDE.md "Current state" updated (5A done; the raw-ent
   UAF hazard closed; focus → 5B).

## 11. File structure

- **New:** `core/src/entity.rs` (pure decode + resolve logic, unit-tested, no CS2 ids); the
  spike-findings doc.
- **Modify:** `core/src/v8host.rs` (the six `(index, serial)` natives; retire the raw-pointer natives
  from the plugin surface; wire `entity.rs`; new engine-struct offset constants with `TODO(gamedata)`),
  `core/src/lib.rs` (add `mod entity;`), `packages/std/index.d.ts` + the `@s2script/std` prelude
  (`EntityRef`), `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts` (`Pawn` EntityRef-backed),
  `README.md`, `CLAUDE.md`.
- The Slice-4 demo (`examples/demo-plugin`) or a small dedicated demo exercises the EntityRef-backed
  `Pawn` for the live gate.

## 12. Scope & deferrals

**Scope:** one coherent slice — the single-plugin entity-safety spine.

**Deferred (do NOT build ahead):** the raw-live block-scoped fast-path (5A.1); the inter-plugin
`EntityRef` wire integration (fast-follow); non-`i32` field types (5B codegen); migrating engine
offsets into a regenerable gamedata file (treadmill tooling); a first-class `unsafe` module (until a
plugin genuinely needs raw escape); and everything already deferred at the Slice-5 level (tsc
typecheck gate, 5B schema codegen, 5C `@s2script/std` breadth, config/permissions/reload-state-handoff,
the registry/platform 5.5, the base-plugin suite 6).

## 13. Global constraints (bind every task)

- **Core stays engine-generic.** No CS2 identifiers, no `include_str!`/`include_bytes!`, no `games/`
  references in `core/src`. `entity.rs` and every new native are engine-generic (they read a
  `CEntityIdentity` layout + do handle bit-math — no game-class knowledge). Both boundary gates
  (`check-core-boundary.sh`, `test-boundary-nameleak.sh`) must stay green. The `EntityRef` primitive
  lives in `@s2script/std`; CS2 class/field names live only in `games/cs2/js/pawn.js`.
- **Never expose a raw pointer across time.** A raw pointer never crosses to JS on the safe path; JS
  holds only `{index, serial}`; every deref is serial-gated. Raw escape is `unsafe`-only (deferred).
- **Degrade-never-crash.** Every native `catch_unwind`-wrapped; every failure is a `null`/`false`
  no-op, never a global crash or a stale deref. No panic crosses the FFI boundary.
- **Layout is data, semantics are code.** Engine-struct offsets are named constants now with a
  `TODO(gamedata)` migration path; behavioral facts + name mappings stay in reviewed code.
- **Naming convention (locked Slice 4):** PascalCase events + types (`EntityRef`, `Pawn`), camelCase
  functions + properties (`isValid`, `readInt32`, `forSlot`, `health`).
- **cdylib test constraint:** unit tests are inline `#[cfg(test)] mod` in the source file (no
  `core/tests/` — can't link).
