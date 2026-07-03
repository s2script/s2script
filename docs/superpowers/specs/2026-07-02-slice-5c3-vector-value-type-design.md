# Slice 5C.3 — Vector value type + direct Vector/QAngle field codegen

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5c3-vector-value-type` (off `main`: Slices 0–5A + entref-wire + 5B + 5C.1 + 5C.2 + 5B.4 merged).
**Family:** 5C.3 — the first `@s2script/std`-breadth slice (introduces the engine-generic `@s2script/math`
module), and simultaneously the next *field kind* after 5B.4 (closes one of 5B's `skip-with-logged-reason`
deferrals: the `Vector`/`QAngle` atomic types).

---

## 1. Goal

Introduce a **`Vector` / `QAngle` value type** (a new engine-generic module `@s2script/math`) and extend the
schema codegen to emit accessors for **direct atomic `Vector`/`QAngle` fields**. This unblocks
`pawn.eyeAngles` (`m_angEyeAngles`, the aim direction) and `pawn.absVelocity` (`m_vecAbsVelocity`, speed) — plus
every other direct `Vector`/`QAngle` field (~515 Vector + 77 QAngle across all classes). Reads only; a read
returns a **copied** value object `{x, y, z}`, never a live pointer; serial-gated `T | null`.

## 2. What we build on (merged)

- **Slice 5B.2/5B.4** — `EntityRef` typed reads via kind-dispatch natives (`__s2_ent_ref_read`, `read_string`)
  over pure `entity.rs` helpers (incl. `read_f32`, null/negative-offset guarded); all `entity_resolve_ptr`
  serial-gated → `T | null`.
- **Slice 5B.3/5B.4** — the codegen (`packages/cli/src/schemagen/{model,emit-dts,emit-js}.ts`, pure + node:test):
  `classifyField` maps atomic type-names → `AccessorKind`; `ATOMIC`/`READ`/`TSTYPE` tables; `idiomaticName`
  (`KNOWN_TAGS`); the emitters produce the committed `games/cs2/js/schema.generated.js` +
  `packages/cs2/schema.generated.d.ts`, freshness-gated by `scripts/check-schema-generated.sh`.
- **Slice 5C.1** — the engine-generic module taxonomy: the core prelude (`INJECTED_STD_PRELUDE`, `v8host.rs`)
  sets `globalThis.__s2pkg_{entity,frame,timers,console,interfaces}`; `s2require` maps `@s2script/<name>` →
  `globalThis.__s2pkg_<name>`; each module is a types-only package `packages/<mod>/{package.json,index.d.ts}`;
  the CLI externalizes `@s2script/*` by wildcard.
- **The catalog** records vector types as `atomic`: `Vector` (×515), `QAngle` (×77), `Color` (×69),
  `Vector2D` (×23), `Quaternion` (×21), `Vector4D` (×5) — the *name* but no byte layout (like enum lacked
  byte-width). The layouts are fixed and known: `Vector`/`QAngle` = 3× `float32` (12 B), `Vector2D` = 2×,
  `Vector4D` = 4×, `Quaternion` = 4× `float32`, `Color` = 4× `uint8` (RGBA).

## 3. Decisions locked during brainstorming

1. **Scope = the `Vector`/`QAngle` value type + codegen for DIRECT atomic `Vector`/`QAngle` field reads.**
   `pawn.eyeAngles` (`m_angEyeAngles` QAngle @5648 on `CCSPlayerPawn`) and `pawn.absVelocity`
   (`m_vecAbsVelocity` Vector @1644 on `CBaseEntity`, inherited) are the headline targets — both direct atomic
   fields on already-curated classes.
2. **`origin` is a deferred follow.** It lives on `CGameSceneNode` (`m_vecAbsOrigin` @200), *not* on the entity,
   reached via a scene-node pointer; the primary `m_vecOrigin` is a quantized wrapper
   (`CNetworkOriginCellCoordQuantizedVector`). Reaching it needs the **embedded/ptr capability** (deferred since
   5B.4). Not this slice.
3. **`Vector` is engine-generic → the CORE prelude.** A `Vector`/`QAngle` value type is true on any Source 2
   game, so it joins `entity`/`frame`/… in `INJECTED_STD_PRELUDE` as `__s2pkg_math`, NOT in `@s2script/cs2`.
   This means the slice **touches core → needs a sniper rebuild** (like 5B.4; unlike the JS-only 5C.2).
4. **A dedicated `readFloats` native** (not 3× `readFloat32` in JS): one `entity_resolve_ptr` lookup, atomic
   (one serial check per vector), extensible to `count` 2/4.
5. **`QAngle` exposed as `{x, y, z}`** (x=pitch, y=yaw, z=roll — the CSSharp convention), uniform with `Vector`.
6. **Value types are copied snapshots** (never a live pointer), consistent with string/number reads; a `Vector`
   is a plain object so it crosses the inter-plugin structured-copy wire cleanly (no wire concern).
7. **Reads only.** Vector **writes** are deferred (velocity/angle networking + `origin`-write = an engine
   `Teleport()` call, not a field poke). `Vector2D`/`Vector4D`/`Color`/`Quaternion` codegen deferred.

## 4. Architecture — the value type + the read primitive (core, engine-generic)

- **`@s2script/math` module** — a new types-only package `packages/math/{package.json,index.d.ts}`. The runtime
  is added to `INJECTED_STD_PRELUDE` (`v8host.rs`), which sets `globalThis.__s2pkg_math = { Vector, QAngle }`:
  - `Vector` — `function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }`; `Vector.prototype.length`
    = `Math.sqrt(x*x + y*y + z*z)`; `Vector.prototype.toString`. (No arithmetic — YAGNI for a read slice.)
  - `QAngle` — `function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }`; `QAngle.prototype.toString`.
- **`core/src/v8host.rs`:** a NEW native `__s2_ent_ref_read_floats(index, serial, offset, count) → number[] | null`:
  `catch_unwind`; `rv.set_null()` first; `entity_resolve_ptr` (invalid → null); read `count` contiguous `f32`s
  via `crate::entity::read_f32(p, offset + i*4)` into a `v8::Array` of doubles; `rv.set(array)`. Registered in
  `install_natives`. (Reuses the existing pure `entity::read_f32`; no new `entity.rs` primitive.)
- **The `EntityRef` prelude + `packages/entity/index.d.ts`:** add `readFloats(off, count) → number[] | null`
  (via the new native) — the low-level primitive. `@s2script/entity` does NOT depend on `@s2script/math`; the
  `Vector`/`QAngle` construction happens in the **generated game-layer getter** (§5), which requires
  `@s2script/math` itself. So the split is clean: core provides the float-triple primitive; the math module
  provides the value classes; the generated code (game layer) joins them.

## 5. Architecture — codegen (extends 5B.4)

- **`model.ts`:** `AccessorKind` gains `"vector" | "qangle"` (per-type; count is always 3 this slice). A shared
  `VEC` table maps the atomic type-name → `{ k, cls, count }`: `Vector → {k:"vector", cls:"Vector", count:3}`,
  `QAngle → {k:"qangle", cls:"QAngle", count:3}`. `classifyField`: for `kind:"atomic"` whose name is in `VEC`,
  return `{accessorKind: VEC[name].k, writable:false}` (before the "not a scalar" skip). `READ` maps both →
  `"readFloats"`; `TSTYPE` maps `vector → "Vector | null"`, `qangle → "QAngle | null"`.
- **`emit-js.ts`:** a `vector`/`qangle` field emits a getter that reads the float triple and constructs the
  value type: `get: function () { var a = this.ref.readFloats(off("<cls>","<raw>"), 3); return a === null ? null
  : new Vector(a[0], a[1], a[2]); }` (`Vector`/`QAngle` per the kind). The generated `schema.generated.js`
  gains, at the top (next to the existing requires), `var Vector = __s2require("@s2script/math").Vector;` +
  `var QAngle = __s2require("@s2script/math").QAngle;` — emitted only when a vector/qangle accessor is present.
- **`emit-dts.ts`:** uses `TSTYPE` (`Vector | null` / `QAngle | null`); adds `import { Vector, QAngle } from
  "@s2script/math";` to the generated `.d.ts` header when a vector/qangle field is present.
- **Regenerate** the committed `games/cs2/js/schema.generated.js` + `packages/cs2/schema.generated.d.ts`;
  `check-schema-generated.sh` stays green. Result: `CCSPlayerPawn` gains `eyeAngles` (`QAngle | null`) and
  (via `CBaseEntity`) `absVelocity` (`Vector | null`).

## 6. Data flow

`pawn.eyeAngles` → generated getter → `this.ref.readFloats(off("CCSPlayerPawn","m_angEyeAngles"), 3)` →
`__s2_ent_ref_read_floats(idx, serial, off, 3)` → `entity_resolve_ptr` serial-gates → 3× `read_f32` →
`[x,y,z]` → the getter `new QAngle(a[0],a[1],a[2])` → a copied value object. A stale ref → the native returns
`null` → the getter returns `null`.

## 7. Testing & acceptance

- **In-isolate (`frame_tests`):** `__s2_ent_ref_read_floats` degrades → `null` (no engine ops); `count` reads
  the right number of floats when wired (or degrades). `EntityRef.readFloats` → `null` degraded.
- **Value-type unit (node:test):** `new Vector(3,4,0).length() === 5`; `Vector`/`QAngle` construct + `toString`.
- **Generated-accessor vm-compose (like `schema-runtime.test.mjs`):** eval `schema.generated.js` + `pawn.js` with
  a stub `EntityRef` whose `readFloats` returns a fixed triple → `pawn.eyeAngles` is a `QAngle {x,y,z}`; when the
  stub `readFloats` returns `null` → the generated accessor returns `null`.
- **Codegen (node:test):** `classifyField` — `Vector`/`QAngle` atomics → `vector`/`qangle`; a non-vector atomic
  still classifies as before; emit — a `vector` field emits `readFloats(off, 3)` + `new Vector(...)`, the TS type
  `Vector | null`, and the `@s2script/math` import appears in both generated files; determinism holds.
- **Freshness gate:** regenerate → `check-schema-generated.sh` green.
- **Live gate (sniper-rebuilt for the new native):** a plugin reads `pawn.eyeAngles` + `pawn.absVelocity` on a
  bot live — a `{x,y,z}` QAngle (the bot's view angles) + a `{x,y,z}` Vector (velocity, ~0 for a standing bot,
  nonzero when moving); both `null` on `bot_kick`; server ticking, no crash.

**Acceptance:** `cargo test -p s2script-core` green (new in-isolate tests); the CLI `node:test` suite green;
both boundary gates + `check-schema-generated.sh` green; the sniper build clean; the live gate passes; README +
CLAUDE updated.

## 8. Scope & deferrals

**Scope:** the `@s2script/math` module (`Vector`/`QAngle` value types + the types-only package); the
`__s2_ent_ref_read_floats` native + `EntityRef.readFloats`; the codegen for direct atomic `Vector`/`QAngle`
fields; regenerating the committed schema files; the live gate.

**Deferred — do NOT build:** `origin` + any field behind `CGameSceneNode`/a pointer/embedded class (the
embedded/ptr capability — its own follow, needs a scene-node spike); Vector **writes** (velocity/angle
`notifyStateChanged` + `origin`-write = an engine `Teleport()`); `Vector2D`/`Vector4D`/`Color`/`Quaternion`
codegen + value types; Vector arithmetic (`add`/`sub`/`dot`/`cross`); the quantized wrapper types
(`CNetworkOriginCellCoordQuantizedVector`, `CNetworkVelocityVector`); the `enum` codegen; the engine-identity
follow (`userId`/pawnless-enumeration); the `tsc` gate; the registry (5.5); the base suite (6).

## 9. Global constraints (bind every task)

- **Core stays engine-generic.** The `Vector`/`QAngle` value types + `readFloats` native + `__s2pkg_math` are
  engine-generic (Source 2 math types); CS2 field names appear only in the regenerated `games/cs2`/`packages/cs2`
  files. NO CS2 identifiers in `core/src`. Both boundary gates green.
- **Never expose a raw pointer / copy values.** A vector read returns a fresh `{x,y,z}` value object COPIED from
  the floats — the pointer never crosses to JS. Every read serial-gated → `T | null`.
- **Layout is data (with a known-shape exception).** Field offsets resolve live from the catalog; the vector
  *shape* (3 floats) is a fixed, well-known layout hardcoded in the `VEC` table (the catalog records the type
  name but no byte layout — same posture as the `ATOMIC` scalar table).
- **Deterministic codegen + freshness gate.** Same catalog+list → byte-identical generated files.
- **cdylib:** core in-isolate tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase types (`Vector`, `QAngle`), camelCase methods/props (`readFloats`, `eyeAngles`,
  `absVelocity`, `length`).
- **Commit trailer** on every commit; commit only on `slice-5c3-vector-value-type`; do NOT push.
