# Slice 5B.4 — String + 64-bit Field Support

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5b4-string-64bit-fields` (off `main`, which has Slices 0–5A + entref-wire + 5B + 5C.1 + 5C.2 merged).
**Family:** Slice 5B (field types): 5B.1 catalog dump, 5B.2 typed access (scalars+handle), 5B.3 codegen, **5B.4
this — strings + 64-bit** (the next deferred kinds).

---

## 1. Goal

Extend the typed field access + codegen to two of the deferred field kinds: **`char[N]` inline strings** and
**64-bit numbers** (`uint64`/`int64`/`float64`). This unblocks `player.playerName` (`m_iszPlayerName`,
`char[128]`) and `player.steamID` (`m_steamID`, `uint64`) as generated accessors — completing the player
model's identity with **no engine natives** — and every other string/64-bit field across all classes
(steamids, names, tick counters, bit flags). Reads only. All serial-gated `T | null`; a string read returns a
**copied** JS string, never a pointer.

## 2. What we build on (merged)

- **Slice 5B.2** — `EntityRef` typed reads via TWO kind-dispatched core natives `__s2_ent_ref_read`/`write`
  over a `KIND_*` code (`I32=1,F32=2,BOOL=3,I8=4,I16=5,U8=6,U16=7,U32=8`); pure `entity.rs` helpers
  (`read_i32`/`read_f32`/`read_bool`/`read_i8`/`read_i16`/`read_u8`/`read_u16`, null/negative-offset guarded).
  Every read `entity_resolve_ptr`-serial-gated → `T | null`.
- **Slice 5B.3** — the codegen (`packages/cli/src/schemagen/{model,emit-dts,emit-js}.ts`, pure + node:test):
  `classifyField(type) → {accessorKind, writable} | {skip}`; the `ATOMIC` table maps scalar atomic names →
  kinds; `idiomaticName` de-Hungarianizes; `READ`/`WRITE`/`TSTYPE` maps; the emitters produce the committed
  `games/cs2/js/schema.generated.js` + `packages/cs2/schema.generated.d.ts`, freshness-gated by
  `scripts/check-schema-generated.sh`.
- **The catalog** records these as: `char[N]` → `{kind:"unknown", name:"char[128]"}`; 64-bit → `{kind:"atomic",
  name:"uint64"/"int64"/"float64"}`. (`CUtlSymbolLarge`/`CUtlString` are DIFFERENT string representations
  — deferred, see §9.)

## 3. Decisions locked during brainstorming

1. **Scope = `char[N]` inline strings + 64-bit ints/float.** These are direct byte reads at a resolved offset.
   `CUtlSymbolLarge` (interned symbol → string-table deref) and `CUtlString` (heap pointer) are **deferred**
   (different representations, need a spike). Reads only (string/64-bit **writes** deferred).
2. **64-bit: `BigInt` primitive, `string`-typed generated accessors.** The low-level `EntityRef.readUInt64`/
   `readInt64` primitives → `bigint | null` (the exact JS 64-bit type; for the rare author who wants numeric/
   bitmask math). But the **generated** `u64`/`i64` accessors return **decimal `string`** (the getter reads the
   bigint and `.toString()`s it, null-safe). Rationale: SourceMod-parity (`GetClientAuthId(AuthId_SteamID64)`
   is a string — SourcePawn has no 64-bit int), **wire-safe** (strings cross the inter-plugin structured-copy
   wire fine — this dissolves the `BigInt`-on-the-wire edge entirely, so no devalue is ever needed for field
   reads), exact (no precision loss), and how steamids are actually used (bans/stats/keys/logging/comparison).
   `readFloat64` → `number | null` (an `f64` fits a JS number; its generated accessor stays `number`). An author
   who needs numeric 64-bit uses the `readUInt64` primitive directly, or `BigInt(str)`.
3. **64-bit reuses the existing kind-dispatch; strings get a new native** (they carry a length).
4. **A name-transform refinement is in scope** (forced by generating `m_steamID`): today `idiomaticName`
   strips *any* leading lowercase run as a Hungarian tag, so `m_steamID` → `iD` and `m_bombSite` → `site`.
   Refine it to strip only a **known** tag set. See §6.

## 4. Architecture — read primitives (core, engine-generic)

- **`core/src/entity.rs` (pure):** add `read_u64(base,off)->u64`, `read_i64(base,off)->i64`,
  `read_f64(base,off)->f64`, and `read_string(base, off, max_len)->String` (read up to `max_len` bytes, stop
  at the first NUL, `String::from_utf8_lossy` → an owned Rust `String`). Same null/negative-offset guards;
  unit-tested with `#[repr(C)]` fixtures.
- **`core/src/v8host.rs`:**
  - **64-bit** — extend `__s2_ent_ref_read`'s `match kind` with `KIND_U64=9` (→ `rv.set(v8::BigInt::new_from_u64(scope, entity::read_u64(p,off)).into())`), `KIND_I64=10` (→ `new_from_i64`), `KIND_F64=11` (→ `rv.set_double(entity::read_f64(p,off))`). Add the three `KIND_*` consts. (Writes: the `_ => return` arm already rejects them.)
  - **Strings** — a NEW native `__s2_ent_ref_read_string(index, serial, offset, maxLen) -> string | null`:
    `catch_unwind`; `entity_resolve_ptr` (invalid → `null`); `entity::read_string(ptr, off, maxLen)` → a
    `v8::String` (`rv.set`). Registered in `install_natives`. A separate native because it needs the length
    arg + returns a string.
- **The `@s2script/entity` prelude (`EntityRef` methods, in `v8host.rs`) + `packages/entity/index.d.ts`:**
  add `K.U64/I64/F64` codes; `readUInt64(off)`/`readInt64(off)` → `bigint | null` (via the generic native),
  `readFloat64(off)` → `number | null`, `readString(off, maxLen)` → `string | null` (via the new native).

## 5. Architecture — codegen (extends 5B.3)

- **`model.ts`:** `AccessorKind` gains `"u64" | "i64" | "f64" | "str"`. `FieldDescriptor` gains
  `strLen?: number`. `ATOMIC` gains `uint64→u64`, `int64→i64`, `float64→f64` (all `writable:false`).
  `classifyField`: for `kind:"unknown"` whose `name` matches `/^char\[(\d+)\]$/`, return
  `{accessorKind:"str", writable:false, strLen:N}` (extend its return type with the optional `strLen`); still
  skip other `unknown`s. `buildModel` copies `strLen` onto the `FieldDescriptor`, so the emitters see it. `READ` gains
  `u64→"readUInt64"`, `i64→"readInt64"`, `f64→"readFloat64"`, `str→"readString"` (the primitive method names);
  `TSTYPE` gains `u64/i64→"string | null"` (generated 64-bit ints are decimal **strings** — decision 2),
  `f64→"number | null"`, `str→"string | null"`.
- **`emit-js.ts`:** a `str` field emits `this.ref.readString(off("<cls>","<raw>"), <strLen>)` (the length arg).
  A `u64`/`i64` field emits a null-safe stringify —
  `var v = this.ref.readUInt64(off("<cls>","<raw>")); return v === null ? null : v.toString();` (so the
  generated accessor is `string | null`, wire-safe). An `f64` emits its `READ` method directly (→ `number`).
  Deterministic ordering unchanged.
- **`emit-dts.ts`:** uses `TSTYPE` — `string | null` (str + 64-bit ints) / `number | null` (f64) — unchanged logic.
- **Regenerate** the committed `games/cs2/js/schema.generated.js` + `packages/cs2/schema.generated.d.ts` via
  `s2script gen-schema`; `check-schema-generated.sh` stays green.
- **Result in `@s2script/cs2`:** `CCSPlayerController` gains generated `playerName` (`string | null`) +
  `steamID` (`string | null`, a decimal steamid64); the `Player` model inherits them (its interface
  `extends CCSPlayerController`). Authors wanting the exact numeric steamid use `player.pawn`/controller
  `ref.readUInt64(off)` → `bigint`.

## 6. The name-transform refinement

`idiomaticName` today: strip `m_`, then `s.match(/^[a-z]+([A-Z].*)$/)` treats the whole leading lowercase run
as a Hungarian tag and drops it. That over-strips names with no tag: `m_steamID`→`iD`, `m_bombSite`→`site`.
Refine: extract the leading lowercase run (before the first uppercase); **strip it only if that exact run is
in a known Hungarian-tag set**, else keep the full core:

```
KNOWN_TAGS = { i, n, b, h, fl, f, u, e, p, a, v, vec, ang, q, sz, isz, ch, clr, un }
m_iHealth      → run "i"    ∈ tags → "health"        (unchanged)
m_flFriction   → run "fl"   ∈ tags → "friction"      (unchanged)
m_hController  → run "h"    ∈ tags → "controller"    (unchanged)
m_iszPlayerName→ run "isz"  ∈ tags → "playerName"
m_steamID      → run "steam" ∉ tags → "steamID"      (was "iD" — FIXED)
m_bombSite     → run "bomb"  ∉ tags → "bombSite"     (was "site" — FIXED)
m_flags        → no uppercase core → "flags"         (unchanged; regex doesn't match)
```

Common gameplay fields (real prefixes) are unchanged, so 5C.2's `Player`/`Pawn` accessors (`health`,
`teamNum`, `friction`, `pawn`, `controller`) don't move. Regenerating re-idiomaticizes any previously
over-stripped name (net-positive; the freshness gate + review catch the diff). Collision→raw fallback is
unchanged.

## 7. Data flow

`player.steamID` → generated getter → `readUInt64(off("CBasePlayerController","m_steamID"))` →
`__s2_ent_ref_read(idx, serial, off, KIND_U64)` → `entity_resolve_ptr` serial-gates → `read_u64` → `BigInt` →
the getter `.toString()`s it → a decimal `string` (`null` if the ref is stale).
`player.playerName` → `readString(off("CBasePlayerController","m_iszPlayerName"), 128)` →
`__s2_ent_ref_read_string(...)` → `read_string(ptr, off, 128)` (NUL-terminated) → a copied `string`. Stale
ref → `null`.

## 8. Testing & acceptance

- **`entity.rs` pure:** `read_u64`/`read_i64` round-trip (incl. values > 2^53 to prove no precision loss),
  `read_f64`, `read_string` (NUL-terminated truncation, `max_len` bound, non-UTF-8 lossy, null/negative-offset
  guards), via `#[repr(C)]` fixtures.
- **In-isolate (`frame_tests`):** `__s2_ent_ref_read` with `KIND_U64/I64` degrades → `null`, `KIND_F64` → `null`;
  `__s2_ent_ref_read_string` degrades → `null`; `EntityRef.readUInt64`/`readInt64`/`readString` → `null`
  degraded. (A live pointer isn't available in-isolate — the real reads are live-gated, matching 5B.2's
  posture; the `bigint` type is exercised at the live gate.)
- **Codegen (node:test):** `idiomaticName` — the §6 cases (`m_steamID`→`steamID`, `m_iszPlayerName`→`playerName`,
  the known-tag cases unchanged); `classifyField` — `uint64`/`int64`/`float64` → kinds, `char[N]` → `str` with
  `strLen=N`, non-`char[N]` `unknown` still skips; emit — a `str` field emits `readString(off, N)`, a `u64`
  field emits the null-safe `readUInt64(off)...toString()` stringify, the TS types (64-bit ints + str →
  `string|null`, f64 → `number|null`); determinism holds.
- **Freshness gate:** regenerate → `check-schema-generated.sh` green.
- **Spike (live, front-loaded):** confirm a `char[N]` inline read (`m_iszPlayerName` → a bot's name string)
  and a `uint64` read (`m_steamID`) on Docker CS2 — inline char arrays + uint64 are direct byte reads, but
  verify the byte layout live before committing the plan. Findings → a dated spike-findings doc.
- **Live gate:** a plugin reads `player.playerName` + `player.steamID` (generated) live — the name is the bot's
  name, the steamID a decimal `string`; both `null` on disconnect; server ticking, no crash.

**Acceptance:** `cargo test -p s2script-core` green (new `entity.rs` + in-isolate tests); the CLI `node:test`
suite green; both boundary gates + `check-schema-generated.sh` green; the sniper build clean; the live gate
passes; README + CLAUDE updated.

## 9. Scope & deferrals

**Scope:** `readUInt64`/`readInt64`/`readFloat64` + `readString` primitives; the 64-bit `KIND_*` arms + the
string native; the codegen for `u64`/`i64`/`f64`/`char[N]`; the name-transform refinement; regenerating the
committed schema files; the live gate.

**Deferred — do NOT build:** `CUtlSymbolLarge`/`CUtlString` string reads (pointer/table derefs — a spike/
follow); string + 64-bit **writes**; `BigInt` across the inter-plugin wire (now MOOT for generated fields —
they're strings; the `readUInt64` bigint primitive is a local read, and an author who puts a raw bigint on the
wire still hits the graceful `InterfaceValueNotSerializable`, no crash — NOT devalue); `enum` codegen (still needs
byte-width — 5B.1 follow); the `Vector` value type + Vector/QAngle codegen (5C.3); embedded/`ptr` accessors;
the `userId` engine op; the base-plugin suite (6); the registry (5.5); config/permissions; the `tsc` gate.

## 10. Global constraints (bind every task)

- **Core stays engine-generic.** The new read primitives + natives + `KIND_*` codes are engine-generic; CS2
  names appear only in the regenerated `games/cs2`/`packages/cs2` files. NO CS2 identifiers in `core/src`.
  Both boundary gates green.
- **Never expose a raw pointer / copy strings.** A string read returns a `v8::String` **copied** from the
  bytes — the pointer never crosses to JS. Every read serial-gated → `T | null`.
- **Layout is data.** Offsets (and the `char[N]` length, parsed from the catalog type name) resolve/derive
  from the catalog; no offsets baked. A field-layout change is absorbed by regenerating.
- **Deterministic codegen + freshness gate.** Same catalog+list → byte-identical generated files.
- **BigInt correctness.** `readUInt64` uses `new_from_u64` (unsigned), `readInt64` uses `new_from_i64` — never
  route a `u64 > i64::MAX` through the signed path.
- **cdylib:** core unit/in-isolate tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase types, camelCase props/methods (`readUInt64`, `readString`, `playerName`).
- **Commit trailer** on every commit; commit only on `slice-5b4-string-64bit-fields`; do NOT push.
