# EntityRef on the Inter-Plugin Wire (the 5A fast-follow)

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-entref-wire` (off `main`, which has Slices 0–5A merged).
**Name:** the EntityRef-wire fast-follow (distinct from the 5A spec's deferred "5A.1" raw-live fast-path).
**Prior art in-repo:** the Slice-4.5 inter-plugin marshalling (`iface_to_json`/`iface_from_json`,
`iface_call`/`iface_emit`) + the Slice-5A `EntityRef` primitive.

---

## 1. Goal — the closing thread

Close the deferral both Slice 4.5 and Slice 5A punted: *"Entity refs on the inter-plugin wire use the
same `EntityRef`/`T | null` type as the entity system"* (CLAUDE.md). A producer plugin hands an
`EntityRef` across a published-interface method return or event payload; the consumer receives a
**live `EntityRef`** (not plain data) that validates against its own view of the shared entity system.
So a producer passes a live pawn's ref → the consumer reads through it → the entity dies → the
consumer's ref goes `null` too. **Host-invalidation across the plugin boundary.** Proven live: two
plugins, a pawn ref crosses, the consumer reads `100`, the pawn is destroyed, the consumer reads
`null` — no crash.

## 2. What we build on (merged)

- **Slice 4.5 marshalling** (`core/src/v8host.rs`): the inter-plugin structured-copy wire — a JSON
  string carrier. `iface_to_json(scope, value)` calls the current context's `JSON.stringify(value)`
  (TryCatch-guarded) → owned Rust `String`; `iface_from_json(scope, json)` calls `JSON.parse(json)`
  (TryCatch-guarded) → a fresh `Local` in the target context. `iface_call`/`iface_emit` use these to
  copy method args/returns + event payloads across the boundary (no shared object identity crosses).
- **Slice 5A `EntityRef`** (`@s2script/std` prelude): `EntityRef(index, serial)` with `isValid()`,
  `readInt32(offset) → number|null`, `writeInt32(offset, value) → boolean`, `notifyStateChanged`. The
  raw pointer never crosses to JS; every access is serial-gated against the engine's `CEntityIdentity`.

**Today's gap:** an `EntityRef` is `{index, serial}` data, so it *already* crosses the JSON carrier —
but it lands on the consumer as a **plain object**, not a live `EntityRef`. The consumer has no safe
accessors and no host-invalidation. This slice adds the tag + rehydration so it lands as a live
`EntityRef`.

## 3. Decision — replacer/reviver in the existing marshalling (Approach A)

The surgical fix: make the marshalling **tag** an `EntityRef` on the way out and **rehydrate** it on
the way in, by passing an `EntityRef`-aware replacer/reviver to the existing `JSON.stringify`/`parse`
calls. Rejected alternatives: (B) an `EntityRef.toJSON()` + manual consumer rehydration — not
transparent, the consumer wouldn't automatically get a live ref; (C) a full JS marshalling module
(`__s2_marshal`/`__s2_unmarshal`) that `iface_call`/`iface_emit` call instead of the Rust helpers —
more refactor than needed. A is two extra args to the two existing `Function::call`s.

## 4. Architecture

**The replacer/reviver are prelude JS** (engine-generic, in `@s2script/std`'s `INJECTED_STD_PRELUDE`),
using the **context's own** `EntityRef`:
- `__s2_entref_replacer(key, value)` → `value instanceof EntityRef ? { __entref__: [value.index, value.serial] } : value`.
- `__s2_entref_reviver(key, value)` → `(value && Array.isArray(value.__entref__)) ? new EntityRef(value.__entref__[0], value.__entref__[1]) : value`.

Because the reviver runs in the **target** (consumer) context, `new EntityRef(...)` binds to *that*
context's natives — a live ref that validates against the shared entity system.

**The Rust marshalling passes them:**
- `iface_to_json` fetches `globalThis.__s2_entref_replacer` (a `Function`) and calls
  `stringify.call(recv, [value, replacer])`.
- `iface_from_json` fetches `globalThis.__s2_entref_reviver` and calls `parse.call(recv, [text, reviver])`.
- **Fallback:** if the global isn't present (e.g. the shared `HOST` context, which has no `@s2script/std`
  prelude), fall back to the current no-replacer/no-reviver call — nothing breaks. Both fetches are
  best-effort (a missing/non-function global → plain stringify/parse).

Still **structured-copy**: only `{index, serial}` numbers cross the boundary as the tag; no shared
object identity. The `EntityRef` the consumer holds is a fresh copy bound to its own context.

## 5. Data flow

Producer returns/emits `ref` (an `EntityRef`) → `iface_to_json` replacer tags it
`{"__entref__":[1,5]}` inside the JSON string → Rust `String` carrier → consumer context
`iface_from_json` reviver → `new EntityRef(1,5)` (live, consumer-context) → the consumer's
`ref.readInt32(HEALTH)` validates `identity[1].serial === 5` against the shared entity system →
`number | null`. Entity dies → serial mismatch → the consumer's ref reads `null`. Both `iface_call`
returns and `iface_emit` payloads get this automatically (both route through the two helpers).

## 6. Error handling — unchanged degrade-never-crash

The tag is plain JSON data, so nothing new can throw: the replacer/reviver are pure; a missing global
falls back to plain stringify/parse; the existing TryCatch guards remain. The reviver's
`Array.isArray(value.__entref__)` guard means any non-tag object passes through untouched.
**Caveat (documented, reserved key):** a plain object literally shaped `{__entref__:[a,b]}` sent as
data would be revived as an `EntityRef` — `__entref__` is a reserved wire key. Acceptable for the
slice. **Access:** the consumer gets a full `EntityRef` (read *and* write) — write-permission gating
is deferred (permissions are a later slice); a producer that shares a ref shares full access.

## 7. Testing & acceptance

**Cargo-unit-testable (in-isolate, like the Slice-4.5 `frame_tests`):**
- A producer publishes an interface whose method returns an `EntityRef`; the consumer calls it and the
  returned value **is a real `EntityRef`** (has `isValid`/`readInt32`), not a plain object — provable
  on the degrade path (null ops → the revived ref's `isValid()` is `false` and `readInt32` is `null`,
  which a plain `{index,serial}` object could not do).
- The same via an event payload (`emit` → the consumer's handler receives a live `EntityRef`).
- A non-`EntityRef` value (plain data, incl. an object with an unrelated `__entref__`-free shape)
  round-trips unchanged (the replacer/reviver don't corrupt ordinary payloads).

**Live-only (the acceptance thread):** two plugins on Docker CS2 — a producer publishes an interface
(or emits an event) carrying `Pawn.forSlot(0).ref`; the consumer reads the pawn's health through the
received `EntityRef` (100), then the pawn is `bot_kick`'d → the consumer's ref reads `null` (no crash,
server keeps ticking). Cross-plugin host-invalidation proven.

**Acceptance criteria:**
1. `cargo test -p s2script-core` green (existing 82 + the new in-isolate tests); both boundary gates
   green; sniper build OK.
2. `s2script build` produces the two demo `.s2sp`s (producer + consumer) passing an `EntityRef`.
3. The live gate passes: the consumer reads a producer-passed pawn's health, and it goes `null` when
   the pawn is destroyed (no crash).
4. README documents the runbook + acceptance; CLAUDE.md "Current state" notes the wire deferral closed.

## 8. File structure

- **Modify** `core/src/v8host.rs`: `iface_to_json`/`iface_from_json` pass the fetched
  replacer/reviver (best-effort) to `stringify`/`parse`; add the `__s2_entref_replacer`/
  `__s2_entref_reviver` functions to `INJECTED_STD_PRELUDE`; the in-isolate tests in `frame_tests`.
- **Modify** `packages/std/index.d.ts` — no new *public* type is required (`EntityRef` already exists);
  the replacer/reviver are internal `__s2_*` globals. (No `.d.ts` change unless a doc note is wanted.)
- **Create** the two demos: `examples/entref-producer/` + `examples/entref-consumer/` (or extend the
  existing greeter demos) for the live gate.
- **Modify** `README.md`, `CLAUDE.md`.

No new C-ABI, no new natives, no game-package change — the whole slice is the two-line marshalling
wiring + two prelude functions + tests + demos.

## 9. Scope & deferrals

**Scope:** one small, surgical slice — the marshalling recognizes `EntityRef` and round-trips it live.

**Deferred (do NOT build):** the raw-live block-scoped fast-path (the 5A spec's "5A.1"); non-`i32`
field access over a wired ref (5B codegen); write-permission gating on a shared ref (permissions, a
later slice); a general typed-token wire for OTHER handle types beyond `EntityRef` (only `EntityRef`
is special-cased now); splitting `@s2script/std` into SourceMod-style modules (Slice 5C, decided
2026-07-01); everything already deferred (tsc gate, 5B/5C, registry 5.5, base-plugin suite 6).

## 10. Global constraints (bind every task)

- **Core stays engine-generic.** No CS2 identifiers, no `include_str!`/`games/` in `core/src`. The
  replacer/reviver live in `@s2script/std` (engine-generic; `EntityRef` is engine-generic). Both
  boundary gates (`check-core-boundary.sh`, `test-boundary-nameleak.sh`) stay green.
- **Structured-copy at the boundary.** Only `{index, serial}` numbers cross (as the tag); no shared
  object identity. The consumer's `EntityRef` is a fresh copy bound to its own context.
- **Degrade-never-crash.** A missing replacer/reviver global falls back to plain stringify/parse; the
  existing TryCatch guards remain; nothing new panics or throws across the FFI boundary.
- **`T | null` / host-invalidation preserved.** The wired `EntityRef` is a full `EntityRef` — every
  access is serial-gated; a dead entity reads `null`, never a stale deref.
- **Naming convention (locked Slice 4):** PascalCase types (`EntityRef`), camelCase fns/props.
- **cdylib test constraint:** unit tests inline `#[cfg(test)] mod` in the source file.
