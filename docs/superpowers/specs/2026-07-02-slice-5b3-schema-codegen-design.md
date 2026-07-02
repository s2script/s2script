# Slice 5B.3 — Schema Codegen

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5b3-schema-codegen` (off `main`, which has Slices 0–5A + entref-wire + 5B.1 + 5B.2 merged).
**Parent:** Slice 5B (schema codegen), sub-project 3 of 3: 5B.1 (catalog dump, done), 5B.2 (typed field access, done), 5B.3 (this — the codegen).

---

## 1. Goal

Consume the committed `games/cs2/gamedata/schema-catalog.json` and **generate** typed field accessors for a
curated set of CS2 entity classes, so plugin authors write idiomatic typed properties —
`pawn.health`, `pawn.friction`, `pawn.controller` — with autocomplete and typechecking, instead of the
raw plumbing `p.ref.readFloat32(__s2_schema_offset("CBaseEntity", "m_flFriction"))`. The raw
`__s2_schema_offset` + `EntityRef.read*` primitives become **internal** — the generated code calls them
under the hood; authors never touch them. Acceptance: a plugin reads a *generated* field (e.g.
`pawn.friction`) on a live CS2 server, correct while the entity lives, `null` on entity death.

## 2. What we build on (merged)

- **Slice 3** hand-wrote the proof-of-shape accessor: `games/cs2/js/pawn.js` `Pawn` has
  `get health()`/`set health()` (`readInt32`/`writeInt32` + `notifyStateChanged`) and `Pawn.forSlot(slot)`
  (controller entity `slot+1` → `m_hPlayerPawn` handle → pawn `EntityRef`); `healthOff` resolved live via
  the `__s2_schema_offset` native, cached by the core `OffsetCache`. 5B.3 **generates** accessors of this
  shape instead of hand-writing them.
- **Slice 5B.1** — `games/cs2/gamedata/schema-catalog.json` (committed, regenerable per CS2 update):
  2429 classes, each `{parent, fields:[{name, offset, type:{kind, name?, inner?}}]}`; own-fields per class,
  inheritance via the `parent` chain.
- **Slice 5B.2** — `EntityRef` (from `@s2script/std`) typed methods: `readFloat32`/`writeFloat32`,
  `readBool`/`writeBool`, `readInt8`/`readInt16`/`readUInt8`/`readUInt16`/`readUInt32` (+ `readInt32`/
  `writeInt32`), and `readHandle(off) → EntityRef | null`. These are the primitives the generated
  accessors emit calls to. `Pawn` exposes `this.ref` (its `EntityRef`).
- **The CLI** — `packages/cli` (`s2script build <dir>`; `cli.ts`/`build.ts`; `build.mjs` → `dist/cli.js`).
  5B.3 adds a `gen-schema` subcommand.
- **The packaging split** — `games/cs2/js/pawn.js` is the **runtime** (copied to `dist/s2script/js/` by
  `scripts/package-addon.sh`, injected per-context by the shim → `register_injected_package`, sets
  `globalThis.__s2pkg_cs2`). `packages/cs2/index.d.ts` is the **author-time type surface** (`import { Pawn }
  from "@s2script/cs2"`; `package.json` `"types"` only, no runtime). 5B.3 generates into BOTH.

## 3. Decisions locked during brainstorming

Grounded in prior art (CounterStrikeSharp / Swiftly / Plugify-Source2) — see §10.

1. **Model = per-class codegen, curated→growing** (the CounterStrikeSharp model, scoped for a first
   slice). The generator handles any class; a checked-in **curated class list** bounds *this* slice's
   output; the list grows in Slice 6 as the base-plugin suite needs classes. (Not a generic string
   accessor — we already have that raw; not full-catalog — that's noise + forces the deferred kinds.)
2. **Naming = idiomatic property + raw resolution key.** The generated property is idiomatic (strip the
   `m_` + Hungarian type tag, camelCase: `m_iHealth`→`health`, `m_flFriction`→`friction`,
   `m_hController`→`controller`, `m_bClientSideRagdoll`→`clientSideRagdoll`). The generated code
   **hardcodes the raw schema name + its declaring class** in the resolve call
   (`__s2_schema_offset("CBaseEntity", "m_iHealth")`) — idiomatic is cosmetic, raw is the stable key.
   **Idiomatic-name collisions within a class are detected at generation time → fall back to the raw name
   for the loser (logged).** Nothing reverses names at runtime; both sides are generated together.
3. **Field kinds in scope = only what 5B.2 can safely express AND the catalog can safely type:**
   `float32`, `bool`, the integer widths (`int8`/`int16`/`int32`, `uint8`/`uint16`/`uint32`), and
   `handle`→`EntityRef`. **Every other field is SKIPPED with a logged per-class reason** (not silently
   dropped): `ptr` (raw-pointer guardrail); embedded `class` **including `Vector`/`QAngle`** (the `Vector`
   value type is 5C); strings (`CUtlSymbolLarge`/`CUtlString`); the compound "atomic" types
   (`CUtlVector<…>`/`CNetworkUtlVectorBase<…>`/`CStrongHandle<…>`/`CEntityOutputTemplate<…>`/
   `CTransform`/`matrix3x4_t`/`Quaternion`); `int64`/`uint64`/`float64` (JS f64 precision — BigInt later);
   `unknown`; and **`enum`** — deferred because the 5B.1 catalog records enum fields as `{kind:"enum",
   name}` with **no byte width**, and CS2 enums are 1/2/4 bytes; defaulting to `int32` would misread narrow
   enums (adjacent-byte garbage). `enum` returns to scope once the catalog carries enum size (a future 5B.1
   dumper extension) → then mapped to the matching `readUInt8`/`readUInt16`/`readInt32`.
4. **Delivery = committed generated artifacts, CI verifies freshness.** `gen-schema` is a
   **maintainer/treadmill** step (never run by the plugin developer), a **pure offline transform** over the
   committed catalog (no live server — unlike the 5B.1 *dump*). It writes two **committed** files; a gate
   regenerates + `git diff --exit-code` fails the build if the catalog changed but the output wasn't
   regenerated.
5. **Access model** (confirmed by prior art): offsets resolved **live** from the SchemaSystem on first
   touch, cached (never baked as literals); resolution keyed on the **declaring** class with the
   inheritance chain **flattened at build time** (the generator walks the catalog `parent` chain — no
   runtime base-walk); setters exist only for writable scalar kinds and **auto-call `notifyStateChanged`**
   (the stricter CS2Fixes behavior); handle fields return a bare `EntityRef` (typed wrappers for the
   referenced class deferred).
6. **Player model unchanged this slice.** `Pawn.forSlot(slot)` stays exactly as-is (pawn-first). The
   controller-as-"client" / pawn-as-body rework, the `Player`/`fromClient` naming, and the SourceMod-parity
   entry-point surface are DEFERRED to 5C / Slice 6 (the base-plugin suite is the parity driver). 5B.3
   generates `CCSPlayerController` + `CCSWeaponBase` *types/accessors* (proving the multi-hierarchy chain),
   reachable via a generated `EntityRef`-wrapping factory / handle reads — not via new entry points.

## 4. Architecture

`gen-schema` is a pipeline of three units with clean boundaries; the two emitters share one model:

```
schema-catalog.json ──▶ [model] ──▶ [.d.ts emitter] ──▶ packages/cs2/schema.generated.d.ts   (committed, types)
      +                    │
curated class list ────────┘   └────▶ [.js emitter]  ──▶ games/cs2/js/schema.generated.js     (committed, runtime)
```

- **The model (pure).** Input: the parsed catalog + the curated class list. For each requested class,
  transitively include its ancestor chain (so `extends`/prototype chains are complete). For each class
  produce a normalized descriptor: `{ className, parent, fields: [{ propName, rawName, declaringClass,
  offset(ref-only), accessorKind, writable }] }` — where `accessorKind` ∈ `{f32, bool, i8, i16, i32, u8,
  u16, u32, handle}` mapped from the catalog `type`, and skipped fields are collected into a
  `{ className, skipped: [{rawName, reason}] }` report. Name mapping + collision→raw fallback + type→kind
  mapping + the skip decision all live here, pure and unit-tested. (Offsets are recorded for reference
  only; the emitted code resolves live — see §5.)
- **The `.d.ts` emitter (pure).** Model → a TypeScript declaration string: `export interface <Class>
  extends <Parent> { <prop>: <tsType>; … }` (own fields only per class; `extends` gives inheritance),
  `tsType` = `number | null` / `boolean | null` / `EntityRef | null`; writable scalars stay mutable, others
  `readonly`. Deterministic ordering (stable, catalog order) so output is byte-reproducible.
- **The `.js` emitter (pure).** Model → the runtime accessor source: for each concrete class, an object of
  getter/setter property descriptors (own + flattened-inherited, since JS needs them all on the usable
  prototype) that read/write via `this.ref.<method>(__s2_schema_offset("<declaringClass>", "<rawName>"))`;
  setters (scalars only) write then `this.ref.notifyStateChanged(off)`; a small `wrap(className, ref)`
  factory to construct a typed accessor object over an existing `EntityRef`. Deterministic ordering.
- **The CLI wiring.** `s2script gen-schema` reads the catalog + list, runs the model + both emitters, writes
  the two files, and prints the per-class skip summary + counts.
- **Integration with `pawn.js`.** `Pawn.forSlot` (the behavioral entry point) stays hand-written; the
  hand-written `get/set health` is **deleted** (now generated from `m_iHealth`); `pawn.js` applies the
  generated `CCSPlayerPawn` accessor descriptors to its `Pawn` prototype and keeps the `forSlot` wiring.
  The generated accessors assume `this.ref` is an `EntityRef` — which `Pawn` provides.

## 5. Data flow (a generated getter, at runtime)

`pawn.friction` → generated getter → `this.ref.readFloat32(__s2_schema_offset("CBaseEntity",
"m_flFriction"))` → core resolves the offset live from SchemaSystem (cached by `OffsetCache` after first
touch) → `EntityRef` serial-gates the read → `number` alive / `null` on a stale ref. `pawn.health = 100` →
generated setter → `writeInt32(off)` → on success `notifyStateChanged(off)`. `pawn.controller` →
`readHandle(off)` → `EntityRef | null`.

## 6. Error handling / degrade

- **Codegen (build time):** an unmappable field → skipped + logged (never emitted as a broken accessor);
  a requested class absent from the catalog → hard error (fail the gen, name it); a name collision →
  raw-name fallback + logged. The generator never emits a field it can't safely back.
- **Runtime:** unchanged from 5B.2 — every read is serial-gated `T | null`, every access `catch_unwind`ed
  in core, offsets resolved live (a missing field → `__s2_schema_offset` returns `< 0` → the generated
  accessor returns `null`, no crash). Degrade-never-crash holds; the generated layer adds no new failure
  modes.

## 7. Testing & acceptance

**Pure / unit-testable (no server):**
- The model: name transform (prefix strip + camelCase) over representative fields; collision→raw fallback;
  type→`accessorKind` mapping for each in-scope kind; each out-of-scope kind → skipped-with-reason; parent
  chain flattened correctly (a field on `CBaseEntity` appears for `CCSPlayerPawn`); ancestor auto-inclusion.
- The emitters: **determinism** — same catalog+list → byte-identical `.d.ts` and `.js` (this is what makes
  the CI freshness gate meaningful); spot-assert the emitted getter/setter shape for a float, a bool, an
  int, and a handle field, and that a skipped field is absent from both outputs.

**Gate:** `scripts/check-schema-generated.sh` — regenerate to a temp dir, `git diff --exit-code` against the
committed `schema.generated.{js,d.ts}`; fail (with the drift) if the catalog was updated but the output
wasn't regenerated. Runs alongside the existing boundary gates.

**Live-only (Docker CS2):** a plugin imports the generated `@s2script/cs2` surface and reads **generated**
fields on the pawn — e.g. `pawn.friction` (float), `pawn.health` (int, still works after the cutover), and
a bool field — correct values while alive; then the pawn dies (`bot_kick`) → the generated getters return
`null`, server keeps ticking, no crash.

**Acceptance criteria:**
1. `s2script gen-schema` regenerates `games/cs2/js/schema.generated.js` + `packages/cs2/schema.generated.d.ts`
   deterministically; `scripts/check-schema-generated.sh` green; the CLI prints per-class coverage +
   skip counts.
2. Unit tests (model + emitters) green; both boundary gates green; sniper build clean.
3. The live gate passes: a generated field reads correctly, `null` on entity death, no crash.
4. README documents `gen-schema` + the treadmill step + the author-facing typed-accessor usage; CLAUDE.md
   "Current state" updated (5B done; Slice 5B.3 complete → next focus is 5C).

## 8. File structure

- **Add** to `packages/cli/src/`: the model + `.d.ts` emitter + `.js` emitter + the `gen-schema` command
  (small focused modules; e.g. `schemagen/model.ts`, `schemagen/emit-dts.ts`, `schemagen/emit-js.ts`, wired
  from `cli.ts`), the curated class list — a **hand-maintained** authoring input (NOT regenerable gamedata),
  so it lives under the game package, e.g. `games/cs2/codegen-classes.json`, not in `gamedata/`.
- **Generate (committed):** `games/cs2/js/schema.generated.js`, `packages/cs2/schema.generated.d.ts`.
- **Modify:** `games/cs2/js/pawn.js` (delete hand-written `health`; apply generated `CCSPlayerPawn`
  accessors; keep `forSlot`), `packages/cs2/index.d.ts` (re-export / reference the generated `.d.ts`),
  `packages/cs2/package.json` if the types entry needs to include the generated file, `scripts/package-addon.sh`
  (ship `schema.generated.js` into `dist/s2script/js/` alongside `pawn.js`).
- **Add gate:** `scripts/check-schema-generated.sh`.
- **Modify:** a demo (read a generated field), `README.md`, `CLAUDE.md`.

The codegen + generated CS2 accessors live entirely in the **game package** layer (`packages/cli` tooling +
`games/cs2` + `packages/cs2`) — **core never sees any of it** (engine-generic; the boundary gates stay green).

## 9. Scope & deferrals

**Scope:** `gen-schema` (model + two emitters + CLI + freshness gate), the curated class list
(`CCSPlayerPawn`, `CCSPlayerController`, `CCSWeaponBase` + ancestors), scalar+handle+enum accessors on them,
committed generated outputs, `pawn.js` integration, the live gate.

**Deferred — do NOT build:** `enum` accessors (the catalog lacks enum byte-width — skip-with-reason until a
future dumper extension records it), the `Vector`/`QAngle` value type + Vector fields, strings,
embedded/`ptr`/nested accessors, typed wrappers for handle-referenced classes (handle → bare `EntityRef`
only), `i64`/`u64` (BigInt), the player-model / `Player`/`fromClient` / controller-as-client rework +
SourceMod-parity entry points (5C / Slice 6), the `tsc` typecheck **gate** (5B.3 GENERATES the `.d.ts`; the
enforcement gate is later), the `@s2script/std` module split + breadth (5C), config/permissions, the
registry (5.5), the base-plugin suite (Slice 6).

## 10. Prior-art grounding (why these choices)

Research across CounterStrikeSharp, Swiftly/SwiftlyS2, and Plugify-Source2 (+ CS2Fixes, cross-checked
against hl2sdk):
- **Offset resolution is universally live + cached, never baked**; keyed on the *declaring* class with
  inheritance flattened at codegen/authoring time (CSS bakes `[SchemaMember("CBaseEntity","m_iHealth")]`;
  CS2Fixes bakes `DECLARE_SCHEMA_CLASS`). → our build-time parent-flatten + live-resolve.
- **Two ecosystem models:** generic string accessor over the whole schema (Swiftly, Plugify) vs broad
  per-class codegen (CSS). We pick per-class codegen (best ergonomics/typing), scoped to a curated list for
  a first slice. Nobody ships a *tiny* curated codegen set — hence "curated→growing."
- **The raw schema name is the universal stable key**; idiomatic names, where present (CSS), are a cosmetic
  layer over the raw name. → idiomatic property + raw resolution key.
- **`NetworkStateChanged` on write** is manual in CSS/Swiftly/Plugify, automatic in CS2Fixes. → we
  auto-call it (matching our Slice-3 `health` setter).

## 11. Global constraints (bind every task)

- **Core stays engine-generic.** The codegen, the curated list, and all generated CS2 accessors live in the
  game-package layer (`packages/cli` + `games/cs2` + `packages/cs2`); NOTHING CS2 enters `core/src`. Both
  gates green (`scripts/check-core-boundary.sh`, `scripts/test-boundary-nameleak.sh`).
- **Layout is data.** Generated code resolves offsets **live** via `__s2_schema_offset` at runtime — it
  **never** embeds an offset number. A field-offset change is absorbed by regenerating from a fresh catalog,
  not by a code change.
- **Never expose a raw pointer across time.** Generated getters return `T | null` via the serial-gated
  `EntityRef`; handle fields return `EntityRef | null`; no pointer/offset escapes to author code.
- **Degrade-never-crash.** Missing field → `null`; the generator never emits an accessor it can't back;
  build-time skips are logged, not silent.
- **Deterministic output.** Same catalog + list → byte-identical generated files (required by the freshness
  gate).
- **Naming:** PascalCase types (`CCSPlayerPawn` interfaces), camelCase props (`friction`, `health`).
- **Commit trailer** on every commit; commit only on `slice-5b3-schema-codegen`; do NOT push.
