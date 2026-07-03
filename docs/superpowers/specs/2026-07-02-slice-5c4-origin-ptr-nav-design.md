# Slice 5C.4 — Pointer-chain field navigation → `pawn.origin` / `pawn.angles`

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5c4-origin-ptr-nav` (off `main`: Slices 0–5A + entref-wire + 5B + 5C.1 + 5C.2 + 5B.4 + 5C.3 merged).
**Family:** 5C.4 — the pointer/embedded field-navigation capability (a deferred 5B follow), applied hand-written to
the most-requested field, `origin`. Continues the field-access line (5B → 5C.3 Vector → this).

---

## 1. Goal

Build a generic, engine-generic **pointer-chain read primitive** in core, and use it (hand-written in the game
layer) to ship `pawn.origin` and `pawn.angles` — the player's world position + body rotation. These live behind
a two-pointer chain the 5B/5C.3 direct-field path can't reach; this slice adds the capability to follow a
pointer chain **entirely in-core** (the raw pointers never cross to JS) and read a copied value at the end.

## 2. What we build on (merged)

- **5A entity safety:** `EntityRef` = `{index, serial}`; `entity_resolve_ptr(index, serial)` serial-gates the
  root entity; `entity::read_ptr(base, off) -> *const u8` (null/negative-offset guarded) derefs a pointer field;
  `entity::read_f32` reads a float. Guardrail: **never expose a raw pointer across time**; raw-live views are
  block-scoped and cannot cross `await`.
- **5C.3 Vector:** the `@s2script/math` `Vector`/`QAngle` value types (core prelude) + `EntityRef.readFloats` +
  the `__s2_ent_ref_read_floats` native (serial-gated, copied `number[]`).
- **5C.2 player model:** `Pawn`/`Player` are `EntityRef`-backed; hand-written nav accessors (`player.pawn`,
  `pawn.controller`) live in `games/cs2/js/pawn.js`, resolving offsets **live** via `__s2_schema_offset`. This
  slice's `pawn.origin`/`pawn.angles` follow the same hand-written pattern.

## 3. The navigation path (catalog-confirmed, spike-verified)

`origin` is reached by a two-pointer chain, every hop schema-resolvable:

```
pawn (entity, serial-gated)
  └─ m_CBodyComponent  @48  {ptr → CBodyComponent}    ── deref
       └─ m_pSceneNode  @8   {ptr → CGameSceneNode}    ── deref
            └─ m_vecAbsOrigin @200 {atomic VectorWS}   ── read 3 floats → Vector
            └─ m_angAbsRotation @212 {atomic QAngle}   ── read 3 floats → QAngle (same chain)
```

(`m_CBodyComponent` is on `CBaseEntity`, inherited by the pawn; `m_pSceneNode` on `CBodyComponent`;
`m_vecAbsOrigin`/`m_angAbsRotation` on `CGameSceneNode`.) The final value is a **plain 3-float `VectorWS`/
`QAngle`** — the 5C.3 `Vector`/`QAngle` types read it directly; the quantized `m_vecOrigin` wrapper is NOT
needed. `pawn.angles` (body world rotation) is distinct from `pawn.eyeAngles` (5C.3, the view/aim direction).

## 4. Decisions locked during brainstorming

1. **Hand-written accessors, defer codegen.** Build the generic pointer-chain native + HAND-WRITE
   `pawn.origin`/`pawn.angles` in `pawn.js` (mirrors the 5C.2 player nav). Teaching the codegen to auto-generate
   embedded/ptr accessors across the whole schema graph (path computation, embedded sub-accessors, cycles) is a
   large deferred feature.
2. **Scope = `origin` + `angles`** (same chain, different final offset — angles nearly free).
3. **The pointer chain is followed ENTIRELY in-core.** The intermediate `CBodyComponent*`/`CGameSceneNode*`
   never cross to JS; the native derefs + reads + returns a copied `{x,y,z}`. This is a block-scoped raw-live
   view within one synchronous native — guardrail-compliant.
4. **Core stays engine-generic.** The native follows a *generic* list of pointer offsets; the CS2-specific chain
   (which offsets = the scene-node path) lives in `pawn.js`. No CS2 names in core.
5. **Offsets resolve live** (`__s2_schema_offset`); nothing baked.

## 5. Architecture — the pointer-chain native (core, engine-generic)

- **`core/src/v8host.rs`:** a NEW native
  `__s2_ent_ref_read_floats_chain(index, serial, ptrOffs, finalOff, count) → number[] | null`:
  - `catch_unwind`; `rv.set_null()` first.
  - Read `index`/`serial`/`finalOff`/`count`; guard `count` ∈ 1..=4 and `finalOff >= 0`.
  - `entity_resolve_ptr(index, serial)` → the root entity pointer (serial-gated); null → return (null).
  - `ptrOffs` is a JS array (read via `v8::Local::<v8::Array>::try_from`, as v8host.rs:1366 does). For each
    element (an i32 offset): guard `off >= 0`, `p = entity::read_ptr(p, off)`; if `p.is_null()` → return (null).
    (A null hop → null result: the chain is broken, e.g. a mid-construction entity.)
  - After the chain, read `count` floats at `p + finalOff` via `entity::read_f32` into a `v8::Array` (a copy) →
    `rv.set`.
  - Registered in `install_natives`.
- **The `EntityRef` prelude + `packages/entity/index.d.ts`:** add
  `readFloatsChain(ptrOffs: number[], finalOff: number, count: number) → number[] | null` (via the native).

**Safety.** The root entity is serial-gated (a destroyed entity → serial mismatch → null before any deref). The
intermediate pointers are NOT entities (no serial) but are OWNED by the entity (freed with it), so an alive
entity ⇒ live component/node pointers; each deref is null-checked (catching construction transients). The native
is synchronous (no `await`/yield mid-chain), so no TOCTOU. The raw pointers never leave core.

## 6. Architecture — the hand-written accessors (game layer, `pawn.js`)

- `pawn.js` requires `@s2script/math` (`var Vector = __s2require("@s2script/math").Vector; var QAngle = …`).
- `pawn.origin` (a getter on `Pawn.prototype`): resolve `m_CBodyComponent` (`CBaseEntity`), `m_pSceneNode`
  (`CBodyComponent`), `m_vecAbsOrigin` (`CGameSceneNode`) live; if any `< 0` → `null`; else
  `var a = this.ref.readFloatsChain([bodyOff, sceneOff], originOff, 3); return a === null ? null : new Vector(a[0], a[1], a[2]);`.
- `pawn.angles`: identical chain, final offset `m_angAbsRotation`, → `new QAngle(...)`.
- **`packages/cs2/index.d.ts`:** `Pawn` gains `readonly origin: Vector | null;` + `readonly angles: QAngle | null;`
  (import `Vector`/`QAngle` from `@s2script/math`). All CS2 field names stay in `pawn.js`.

## 7. Data flow

`pawn.origin` → the getter resolves the 3 offsets → `readFloatsChain([48, 8], 200, 3)` →
`__s2_ent_ref_read_floats_chain(idx, serial, [48,8], 200, 3)` → `entity_resolve_ptr` (serial-gate) →
`read_ptr(p,48)` (→ CBodyComponent) → `read_ptr(p,8)` (→ CGameSceneNode) → 3× `read_f32(p, 200+i*4)` →
`[x,y,z]` → `new Vector(...)`. Stale entity or a null hop → `null`.

## 8. Testing & acceptance

- **In-isolate (`frame_tests`):** `__s2_ent_ref_read_floats_chain` degrades → `null` without engine ops
  (`entity_resolve_ptr` null); `EntityRef.readFloatsChain` → `null` degraded; a guard test (empty chain reads at
  the entity root; a negative `finalOff` → null).
- **Game-layer vm-compose (like `schema-runtime.test.mjs`):** with a stub `EntityRef.readFloatsChain` returning a
  fixed triple + a stub `@s2script/math`, `pawn.origin` is a `Vector {x,y,z}`; a null `readFloatsChain` → `null`;
  a missing offset (`__s2_schema_offset` → −1) → `null`.
- **Spike (live, front-loaded in the live task):** confirm the chain `entity+m_CBodyComponent → +m_pSceneNode →
  m_vecAbsOrigin` reads a **sane** world origin on de_inferno (a bot at a spawn point has plausible map coords,
  not garbage/zero) via a raw `readFloatsChain` before trusting the accessors. Findings → a dated doc.
- **Live gate (sniper-rebuilt for the new native):** `pawn.origin` reads a plausible `{x,y,z}` map position +
  `pawn.angles` a `{x,y,z}` rotation on a bot; both `null` on `bot_kick`; server ticking, no crash.

**Acceptance:** `cargo test -p s2script-core` green (new in-isolate tests); the CLI `node:test` suite green;
both boundary gates + `check-schema-generated.sh` green; the sniper build clean; the live gate passes; README +
CLAUDE updated.

## 9. Scope & deferrals

**Scope:** the `__s2_ent_ref_read_floats_chain` native + `EntityRef.readFloatsChain`; the hand-written
`pawn.origin`/`pawn.angles` + their `.d.ts` types; the spike; the live gate.

**Deferred — do NOT build:** codegen auto-generation of embedded/ptr accessors across the schema graph (this
slice is the capability + a hand-written application); the quantized `m_vecOrigin`
(`CNetworkOriginCellCoordQuantizedVector`) wrapper; Vector/origin **writes** (`origin`-write = an engine
`Teleport()` call, not a poke); a generic scalar-behind-pointer read (this native reads floats only — extend
later if needed); the engine-identity follow (`userId`/pawnless-enum); the game-event system; the `tsc` gate;
the registry (5.5); the base suite (6).

## 10. Global constraints (bind every task)

- **Core stays engine-generic.** The pointer-chain native follows a *generic* offset list; CS2 chain knowledge
  (`m_CBodyComponent`/`m_pSceneNode`/`m_vecAbsOrigin`/`m_angAbsRotation`) lives ONLY in `pawn.js` +
  `packages/cs2`. NO CS2 identifiers in `core/src`. Both boundary gates green.
- **Never expose a raw pointer across time.** The intermediate `CBodyComponent*`/`CGameSceneNode*` are followed
  and read within ONE synchronous native and never cross to JS; only the copied `{x,y,z}` value returns. Each
  hop null-checked; the root entity serial-gated → `T | null`.
- **Layout is data.** Every offset (the chain + the final field) resolves live via `__s2_schema_offset`; nothing
  baked.
- **cdylib:** core in-isolate tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase types (`Vector`, `QAngle`), camelCase methods/props (`readFloatsChain`, `origin`,
  `angles`).
- **Commit trailer** on every commit; commit only on `slice-5c4-origin-ptr-nav`; do NOT push.
