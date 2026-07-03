# Slice 5C.5 — Curated pointer-chain field navigation (codegen)

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5c5-ptr-nav-codegen` (off `main`: Slices 0–5A + entref-wire + 5B + 5C.1/5C.2/5B.4/5C.3/5C.4 + 5D.1 merged).
**Family:** 5C.5 — generalizes 5C.4's hand-written `pawn.origin`/`angles` (pointer-chain nav) into codegen over a
**curated** set of navigation targets. The first of the two remaining "continue with both" slices (this one =
pure JS/codegen + a small runtime primitive, no engine RE; the next = the engine-RE bundle).

---

## 1. Goal

Generalize 5C.4's pointer-chain field access from hand-written to **codegen'd**, over a **curated** list of
navigation targets — so `pawn.sceneNode` (`origin`/`angles`/`scale`/…), `pawn.weaponServices` (`activeWeapon`),
`pawn.movementServices` (`ducked`/`ladderNormal`/…), and `pawn.aimPunchServices` (recoil angles) become typed,
pointer-backed wrappers whose fields read through the pointer chain, serial-gated at the root entity. NOT a graph
traversal — the schema graph has 1918 embedded + 113 ptr fields across 2429 classes, is cyclic, and is mostly
engine noise; we curate the few useful targets (exactly like `codegen-classes.json` curates schema classes).

## 2. What we build on (merged)

- **5C.4** — `EntityRef.readFloatsChain(ptrOffs, finalOff, count)` + the native `__s2_ent_ref_read_floats_chain`
  (follows a pointer chain **entirely in-core**, serial-gated at the root, each hop null-checked, returns a
  copied float triple; the raw intermediate pointers never cross to JS). The safety model for this slice.
- **5B.2/5B.4** — the kind-dispatch scalar read native `__s2_ent_ref_read(idx,serial,off,kind)` over `KIND_*`
  (I32/F32/BOOL/I8/I16/U8/U16/U32/U64/I64/F64); `__s2_handle_decode` (a `CEntityHandle` u32 → a serial-gated
  `EntityRef`).
- **5B.3/5C.3** — the schema codegen (`packages/cli/src/schemagen/{model,emit-dts,emit-js,gen}.ts`):
  `classifyField`/`idiomaticName`/`ATOMIC`/`VEC`/`READ`/`TSTYPE`, the curated `games/cs2/codegen-classes.json`,
  the freshness gate. `Vector`/`QAngle` value types (`@s2script/math`).
- **The catalog** records ptr fields as `{kind:"ptr", inner:"<Class>"}` and embedded as `{kind:"class",
  name:"<Class>"}`; `__s2_schema_offset(cls, field)` resolves an offset walking the inheritance chain.

## 3. Decisions locked during brainstorming

1. **Curated navigation paths, NOT graph traversal.** A hand-maintained `games/cs2/nav-targets.json` lists
   `{ wrapperProp, source class, path (ordered ptr/embedded field names), target class }`. Bounded, cycle-free,
   noise-free.
2. **Pointer-backed wrappers.** Each nav target → a typed wrapper exposing the target class's readable fields
   (generated from the schema, walking inheritance like the schema codegen). `pawn.sceneNode` → a `SceneNode`
   wrapper, `pawn.weaponServices` → a `WeaponServices` wrapper, etc. This replaces 5C.4's hand-written
   `pawn.origin`/`angles` (now `pawn.sceneNode.origin`/`angles` — see §10 for the compat note) and auto-exposes
   the rest.
3. **The runtime generalizes the chain read to all needed kinds — minimally.** ONE new native
   `__s2_ent_ref_read_chain(idx, serial, pathOffs[], finalOff, kind)` (the scalar kind-dispatch, but with a
   pointer-chain deref first); `readFloatsChain` (5C.4, exists) covers vectors; `__s2_handle_decode` covers
   handles. `EntityRef` gains per-kind `…Via(pathOffs, finalOff)` methods.
4. **Four curated targets** (all four chosen): `CGameSceneNode`, `CCSPlayer_WeaponServices`,
   `CCSPlayer_MovementServices`, `CCSPlayer_AimPunchServices` (§6).
5. **`VectorWS` → a vector accessor** (a world-space 3-float Vector; add it to the `VEC` map alongside
   `Vector`/`QAngle`).
6. **Safety = re-resolve per access.** A wrapper holds `(rootEntityRef, pathOffsets)` — NEVER a cached pointer.
   Every field access re-resolves the chain in-core, serial-gated at the root; a stale entity reads `null`.

## 4. Architecture — the runtime (core, engine-generic)

- **`core/src/v8host.rs`:** a NEW native `__s2_ent_ref_read_chain(index, serial, pathOffs, finalOff, kind) →
  value | null`: `catch_unwind`; `rv.set_null()`; guards (`finalOff >= 0`, `pathOffs` is an array); resolve the
  root via `entity_resolve_ptr` (serial-gated); follow each `pathOffs` element via `entity::read_ptr`
  (null-check each hop); then read at `finalOff` per `kind` using the SAME `match kind` arms as
  `__s2_ent_ref_read` (I32/F32/BOOL/… → number/bigint/bool). Reuses the existing pure `entity::read_*` helpers.
  (Structurally = `__s2_ent_ref_read_floats_chain` for the deref loop + `__s2_ent_ref_read`'s `match kind` for
  the final read — factor the shared deref if clean.)
- **The `EntityRef` prelude + `packages/entity/index.d.ts`:** add the SCALAR + handle chain-read methods, thin
  wrappers over the new native: `readInt32Via`/`readInt8Via`/`readInt16Via`/`readUInt8Via`/`readUInt16Via`/
  `readUInt32Via` → `number | null`; `readFloat32Via` → `number | null`; `readBoolVia` → `boolean | null`;
  `readUInt64Via`/`readInt64Via` → `bigint | null`; `readHandleVia(pathOffs, off)` → `EntityRef | null`
  (= `readUInt32Via` → `__s2_handle_decode`). **Vectors reuse 5C.4's existing `readFloatsChain(pathOffs, off, 3)`
  directly** (no new via-method — it already reads a float triple through a chain); the wrapper's `Vector`/
  `QAngle` getter wraps its `number[]` result.

## 5. Architecture — the nav-targets config + the `navgen` codegen

- **`games/cs2/nav-targets.json`** (committed, hand-maintained): an array of
  `{ prop, source, path: [fieldNames], target, wrapper }`, e.g.
  `{ "prop": "sceneNode", "source": "CCSPlayerPawn", "path": ["m_CBodyComponent","m_pSceneNode"], "target":
  "CGameSceneNode", "wrapper": "SceneNode" }`.
- **`navgen`** (`packages/cli/src/navgen/{model,emit-js,emit-dts,gen}.ts`, pure + node:test; `s2script
  gen-nav`): a pure transform over `nav-targets.json` + the committed `schema-catalog.json`. For each target,
  build the wrapper's field list from the target class's readable fields (walk inheritance; reuse
  `classifyField`/`idiomaticName`; collision→raw). Emit:
  - **Runtime** (`games/cs2/js/nav.generated.js`, injected ahead of `pawn.js` like `schema.generated.js`): per
    wrapper, a constructor `function SceneNode(root, path) { this.root = root; this.path = path; }` +
    accessors reading VIA the chain — `get scale() { return this.root.readFloat32Via(this.path,
    off("CGameSceneNode","m_flScale")); }`, `get origin() { var a = this.root.readFloatsChain(this.path,
    off(...), 3); return a === null ? null : new Vector(a[0],a[1],a[2]); }`, `get activeWeapon() { return
    this.root.readHandleVia(this.path, off(...)); }` (→ `EntityRef`). Plus, for each nav target, a nav accessor
    installed on the SOURCE prototype: `get sceneNode() { var p = resolvePath(["m_CBodyComponent","m_pSceneNode"]);
    return p === null ? null : new SceneNode(this.ref, p); }` where `resolvePath` maps the path field-names to
    live offsets via `__s2_schema_offset` (null if any missing). Exposes `globalThis.__s2pkg_cs2_nav`.
  - **Types** (`packages/cs2/nav.generated.d.ts`): `export interface SceneNode { readonly scale: number | null;
    readonly origin: Vector | null; … }` (using the same `TSTYPE` map + `Vector`/`QAngle`/`EntityRef` imports) +
    the nav accessors are declared on `Pawn` in `packages/cs2/index.d.ts` (`readonly sceneNode: SceneNode |
    null; …`).
- **`pawn.js`** installs the nav accessors: `globalThis.__s2pkg_cs2_nav.applyNav(Pawn.prototype, "CCSPlayerPawn")`
  (mirrors `schema.applyAccessors`). The wrappers themselves come from `nav.generated.js`.
- **Freshness-gated** (`scripts/check-nav-generated.sh`, mirror `check-schema-generated.sh`); deterministic.

## 6. The four curated targets (catalog-confirmed)

| prop | source → path | target (runtime type) | sample readable fields |
|---|---|---|---|
| `sceneNode` | `CCSPlayerPawn` → `m_CBodyComponent`,`m_pSceneNode` | `CGameSceneNode` | `origin` (`m_vecAbsOrigin` VectorWS), `angles` (`m_angAbsRotation` QAngle), `scale` (`m_flScale`), `dormant` (`m_bDormant`), `absScale` (`m_flAbsScale`) |
| `weaponServices` | `CCSPlayerPawn` → `m_pWeaponServices` | `CCSPlayer_WeaponServices` | `activeWeapon` (`m_hActiveWeapon` handle → `EntityRef`), `savedWeapon`, the `nTimeTo*` ints, the pickup bools |
| `movementServices` | `CCSPlayerPawn` → `m_pMovementServices` | `CCSPlayer_MovementServices` | `ducked` (`m_bDucked`), `duckAmount` (`m_flDuckAmount`), `ladderNormal` (`m_vecLadderNormal` Vector), … |
| `aimPunchServices` | `CCSPlayerPawn` → `m_pAimPunchServices` | `CCSPlayer_AimPunchServices` | the `predictableBaseAngle`/`unpredictableBaseAngle` QAngles |

(`m_pWeaponServices`/`m_pMovementServices` live on `CBasePlayerPawn` and are declared as the base `CPlayer_*`
types, but the runtime object is the CS2-derived `CCSPlayer_*` — curate the **derived** class as the target for
the full field set; `__s2_schema_offset` resolves the path field on the ancestor + the target fields on the
derived class, both via inheritance.) Handle fields → `EntityRef` (`m_hActiveWeapon`); `CUtlVector` fields
(`m_hMyWeapons`) are deferred (arrays). A field whose kind isn't chain-supported (embedded/ptr/`CUtl*`/enum)
is skipped-with-logged-reason, exactly as the schema codegen does.

## 7. Data flow

`pawn.weaponServices.activeWeapon` → `pawn.weaponServices` getter resolves the path `[m_pWeaponServices]` live →
`new WeaponServices(pawn.ref, [wsOff])` → `.activeWeapon` getter → `this.root.readHandleVia([wsOff],
off("CCSPlayer_WeaponServices","m_hActiveWeapon"))` → `readUInt32Via` → `__s2_ent_ref_read_chain(idx, serial,
[wsOff], off, KIND_U32)` → `entity_resolve_ptr` (serial-gate) → `read_ptr(p, wsOff)` (→ the services object) →
`read_u32(p, off)` (the handle) → `__s2_handle_decode` → a serial-gated `EntityRef` (the active weapon). Stale
pawn / null hop → `null`.

## 8. Safety

- **The raw intermediate pointers never cross to JS** — the chain is followed + read within one synchronous
  native (5C.4's model); only copied values / a re-derived `EntityRef` (the decoded handle) return.
- **No cached pointer.** A wrapper holds `(rootEntityRef, pathOffsets)` — a value, not a live handle. Every
  access re-resolves the chain, serial-gated at the root; a destroyed pawn → `null` (never garbage). A stashed
  wrapper is safe (it re-resolves + degrades).
- **Layout is data.** Every offset (the path + the field) resolves live via `__s2_schema_offset`; nothing baked.
- **Degrade per field.** A null hop, a missing offset, or a stale root → the field's `null`.

## 9. Testing & acceptance

- **In-isolate (core):** `__s2_ent_ref_read_chain` degrades → `null` without ops; the kind arms return the right
  types when wired (or degrade); guards (non-array path, negative finalOff, bad kind) → `null` before the resolve.
- **navgen (node:test):** the pure transform over a fixture `nav-targets.json` + a fixture catalog emits the
  wrapper interfaces (right fields/types via `TSTYPE`), the chain-read getters (`readFloat32Via`/`readHandleVia`/
  `readVectorVia`), the nav accessors on the source, and the `@s2script/math` imports; determinism holds; the
  committed `nav.generated.{js,d.ts}` are freshness-gated.
- **vm-compose (like `schema-runtime.test.mjs`):** with a stub `EntityRef` whose `read*Via` return fixed values,
  `pawn.sceneNode` is a `SceneNode` with `scale`/`origin`; `pawn.weaponServices.activeWeapon` is an `EntityRef`;
  a null path/read → `null`.
- **Live gate (sniper-rebuilt for the new native):** on Docker CS2 (`bot_quota 2`), a plugin reads
  `pawn.sceneNode.origin` (a plausible map coord — matches 5C.4), `pawn.sceneNode.scale` (≈1.0),
  `pawn.weaponServices.activeWeapon` (an `EntityRef`; log its validity / a field), `pawn.movementServices.ducked`
  (bool). All `null` on `bot_kick`; server ticking, no crash.

**Acceptance:** `cargo test -p s2script-core` green (the new native); the CLI `node:test` suite green (navgen +
vm-compose); both boundary gates + all freshness gates (`schema`, `events`, `nav`) green; the sniper build clean;
the live gate passes; README + CLAUDE updated.

## 10. Scope & deferrals

**Scope:** the `__s2_ent_ref_read_chain` native + `EntityRef.*Via` methods; `games/cs2/nav-targets.json`; the
`navgen` codegen + `s2script gen-nav` + `check-nav-generated.sh`; the 4 curated targets → generated wrappers +
nav accessors; `VectorWS`→vector; the live gate.

**Compat note:** 5C.4's hand-written `pawn.origin`/`pawn.angles` are SUPERSEDED by `pawn.sceneNode.origin`/
`angles`. Keep the two `pawn.origin`/`pawn.angles` hand-written accessors as thin aliases (`get origin() { var s =
this.sceneNode; return s ? s.origin : null; }`) so existing code doesn't break, OR remove them and update the
demo — decide in the plan (default: keep as aliases; note in README).

**Deferred — do NOT build:** graph auto-traversal / non-curated targets; chains deeper than the curated paths;
cyclic navigation (`CGameSceneNode.m_pParent`/`m_pChild`); `CUtlVector`/array fields (`m_hMyWeapons`);
pointer-chain **writes**; string-via (no curated target needs a `char[N]` through a chain — add if one does);
typed wrappers recursively navigating to further ptr targets (one hop of wrappers only); the engine-RE bundle
(the next slice); the `tsc` gate; the registry (5.5); the base suite (6).

## 11. Global constraints (bind every task)

- **Core stays engine-generic.** The `__s2_ent_ref_read_chain` native + `EntityRef.*Via` are engine-generic; the
  nav-targets config + wrappers + CS2 field names live ONLY in `games/cs2` + `packages/cs2`. NO CS2 identifiers
  in `core/src`. Both boundary gates green.
- **Never expose a raw pointer across time.** The chain is followed + read in-core; wrappers hold a
  `(rootEntityRef, pathOffsets)` VALUE, not a pointer; every access re-resolves serial-gated → `T | null`.
- **Layout is data.** Offsets (path + field) resolve live; nothing baked.
- **Deterministic codegen + freshness gate.** Same config+catalog → byte-identical `nav.generated.{js,d.ts}`.
- **cdylib:** core in-isolate tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase wrappers/types (`SceneNode`, `WeaponServices`), camelCase props/methods (`sceneNode`,
  `activeWeapon`, `readHandleVia`).
- **Commit trailer** on every commit; commit only on `slice-5c5-ptr-nav-codegen`; do NOT push.
