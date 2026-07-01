# Slice 3 — One Schema-Backed Typed Accessor (`pawn.health`) — Design Spec

- **Project:** s2script (TypeScript plugin framework for Source 2; SourceMod's spiritual successor)
- **Date:** 2026-06-30
- **Status:** Approved design, ready for implementation planning
- **Builds on:** Slice 0 (V8 in CS2), Slice 1 (multiplexer + `OnGameFrame`), Slice 2 (tick-integrated async) — all merged to `main`.
- **Scope:** Slice 3 only — one schema-resolved typed field accessor, state-change folded, living in the game package. See `docs/ARCHITECTURE.md` §2.0.5 and §3 (Slice 3).

---

## 1. Purpose & what it proves

Prove the **engine-generic core / per-game package boundary holds at the very first game-specific accessor.** Resolve one field — `CCSPlayerPawn::m_iHealth` — from the live Source 2 SchemaSystem in-process, and expose `pawn.health` get/set in `@s2script/cs2` (JS), with the network state-change folded into the setter so a connected client's HUD updates. Core gains only **engine-generic** Source 2 machinery (schema-offset resolution, entity access, raw field read/write, `NetworkStateChanged`, a minimal ConCommand); **zero CS2 names appear in core Rust.** All CS2 knowledge (`CCSPlayerPawn`, `m_iHealth`, `CCSPlayerController`, `m_hPlayerPawn`, the controller→pawn walk) lives in the cs2 package.

This is the first slice to cross out of engine-generic core into a game package, and the first with a **real offset dependency** — the trigger for the update-day fire drill (`docs/ARCHITECTURE.md` §3 cross-cutting (e)).

**Litmus for every line of code:** *would this be true on a different Source 2 game?* Yes → core. No → `@s2script/cs2`.

**Key property — layout is data, resolved live.** The `m_iHealth` offset is read from the in-process SchemaSystem at runtime, never hardcoded. A Valve offset shift on an update needs **no code change** (CLAUDE.md: "a field-offset change must never require a code change"); only a field *rename* touches cs2 (name mappings live in reviewed code). The gamedata carries interface/signature *strings*, not offsets.

## 2. Decided directions

1. **The accessor is JS in `@s2script/cs2`; core exposes only engine-generic natives** (confirmed over game-specific Rust). The SourceMod model: one engine-generic native binary; game-specifics are data (gamedata) + JS packages, never per-game compiled binaries. The `games/cs2` Rust crate stays the empty boundary sentinel.
2. **The `m_iHealth` offset is resolved live from SchemaSystem in-process** (confirmed over a hardcoded/gamedata offset) — realizing "layout is data" and keeping the framework green across offset-only patches.
3. **Two demo paths, both verified** (confirmed): an **automatable server-side readback gate** (frame/timer-armed, targets a bot pawn) proving get/set/state-change in logs, **and** a **manual client HUD confirmation** via an `s2_sethp` ConCommand the operator types after connecting — proving the state-change networks to a real client's HUD end-to-end.
4. **The boundary check gains a name-leak gate** (confirmed): fail the build if any CS2 class/field identifier appears in `core/`, catching a leak the crate-dependency check alone would miss.

## 3. Core — the engine-generic native bridge (`core/src/`, new module e.g. `schema.rs` / `entity.rs`)

All natives are Source-2-generic; none contain a CS2 name. Installed on the V8 global alongside `console`/`onGameFrame`/`Delay` (Slices 0–2), following the same block-scoped-scope + no-`HOST`-borrow-across-JS discipline established in Slices 1–2.

- **`__s2_schema_offset(class: string, field: string) → i32`** — in-process SchemaSystem lookup: resolve the declared class in the game's type scope, find the field, return its byte offset. Result cached by `(class, field)`. On a missing class/field, return `-1` and log a named `WARN` (degrade-per-descriptor — the accessor disables with a reason, the framework keeps running). Never panics across the FFI boundary.
- **`__s2_entity_by_index(index: i32) → ExternalPointer | null`** — resolve a `CEntityInstance*` from the engine-generic entity system by entity index; `null` if empty. The entity system itself is acquired via the most direct available path (an interface, or a single gamedata **signature** if required — validated in §11); any signature added lives in `gamedata/`, never hardcoded.
- **`__s2_deref_handle(handleValue: u32) → ExternalPointer | null`** — resolve an entity handle (a `CEntityHandle`/`CBaseHandle` u32, as read from a schema handle field) to its live `CEntityInstance*`, or `null` if stale/invalid. (Minimal raw form; the full `EntityRef`/staleness wrapper is Slice 5.)
- **`__s2_ent_read_i32(ent: ExternalPointer, offset: i32) → i32`** and **`__s2_ent_write_i32(ent: ExternalPointer, offset: i32, value: i32)`** — raw field read/write at a resolved offset relative to an entity pointer. Bounds/null-guarded (null ent or negative offset → no-op read returns 0 / write is skipped, with a named `WARN`).
- **`__s2_ent_state_changed(ent: ExternalPointer, offset: i32)`** — invoke the Source 2 `NetworkStateChanged` path so the changed field is re-sent to clients. This is the "state-change folded into the setter" that makes the client HUD update.
- **`__s2_concommand(name: string, jsFn: function)`** — register a minimal raw Source 2 ConCommand; on invocation the callback hands JS the **calling client slot** and the argument string(s). Just enough to trigger the demo — **not** the Slice-5 command framework (no filters, targeting, permissions, or reply routing).

`ExternalPointer` is a `v8::External`-wrapped raw pointer, opaque to JS, valid only for the current call chain (never stored across `await` — consistent with the raw-pointer discipline; the durable handle system is Slice 5).

## 4. `@s2script/cs2` — the JS package

A real JavaScript file under `games/cs2/` (e.g. `games/cs2/js/pawn.js`), read from disk and `eval`'d into the shared context at boot via a core "load this cs2 file" path (the shim passes the resolved path, like it passes the gamedata path). This models `@s2script/cs2` as a distributable package **without** building the Slice-4 plugin loader / `.s2sp`. Contents — all CS2-specific:

- The **names**: `"CCSPlayerPawn"`, `"m_iHealth"`, `"CCSPlayerController"`, `"m_hPlayerPawn"`.
- The **offset resolution** (once, at load): `const HEALTH = __s2_schema_offset("CCSPlayerPawn", "m_iHealth")`, `const PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn")`.
- The **`slot → controller → pawn` walk**: map a client slot to its controller entity (CS2 index convention), read the `m_hPlayerPawn` handle field off the controller, deref it to the pawn `CEntityInstance*`. All CS2 knowledge, all in cs2.
- The typed wrapper:
  ```js
  class Pawn {
    constructor(ent) { this.ent = ent; }
    get health()  { return __s2_ent_read_i32(this.ent, HEALTH); }
    set health(v) { __s2_ent_write_i32(this.ent, HEALTH, v); __s2_ent_state_changed(this.ent, HEALTH); }
  }
  function pawnForSlot(slot) { /* slot → controller → m_hPlayerPawn → deref → new Pawn(ent) | null */ }
  ```
- Provisional, like Slice 1's `onGameFrame` — the typed, codegen-backed `@s2script/cs2` API is Slice 5.

The `games/cs2` **Rust crate** remains empty (the boundary sentinel). No CS2 name enters core.

## 5. The two demo paths (share the cs2 `slot → pawn` walk)

Both are baked in for Slice 3 (removed when real plugin loading lands in Slice 4), consistent with the Slice 1/2 baked demos.

- **Auto CI gate — frame/timer-armed** (reuse the Slice-2 arm-after-live-ticking pattern — the server barely ticks during boot, so arm after a live-frame threshold): once live-ticking, scan slots for the first non-null player pawn (a bot added via `bot_add`), then log: the resolved offset, `health` get, a `health` set to a marker value, the state-change call, and a readback confirming the write. Fully log-verifiable on the headless server.
- **Manual HUD — `s2_sethp <value>`**: the ConCommand handler resolves the **calling** client's pawn via `pawnForSlot(callerSlot)` and sets `pawn.health = value`. The operator connects a CS2 client, types `s2_sethp 1234` in console, and watches their own HUD health become `1234` — proving the setter's `NetworkStateChanged` networks to the client.

## 6. Boundary discipline

- **Core stays engine-generic:** no CS2 class/field/identifier in `core/`. Enforced by extending the boundary gate (`scripts/check-core-boundary.sh` or a companion grep) to fail if patterns like `CCSPlayer`, `m_iHealth`, `m_hPlayerPawn`, `cs2`/`CS2` game names appear under `core/`. The existing crate-dependency check stays (core depends on no `games/*` crate).
- **cs2 uses only core natives + gamedata:** the cs2 JS calls the generic `__s2_*` natives and nothing engine-specific of its own.
- **gamedata carries strings, not offsets:** the SchemaSystem interface string is already in `gamedata/core.gamedata.jsonc`; any entity-system signature added is a *signature string* in gamedata. The `m_iHealth` offset is never in gamedata or code — it's resolved live.

## 7. Testing strategy

- **Unit (`cargo test`, no engine):** the schema-offset **cache** (a second lookup of the same `(class, field)` doesn't re-query; a miss returns `-1` and logs once); the memory read/write helpers against a **fake in-memory struct** (write-then-read round-trips at an offset; null-ent / negative-offset guards return safely). These isolate the pure logic from the live engine.
- **Integration (`cargo test` + V8, `--test-threads=1`):** the JS↔native bridge wiring — the natives are installed on the global and callable from JS with the documented signatures/return shapes (using a fake/stub entity pointer where a live engine isn't available).
- **Live (sniper build + Docker, operator-run):**
  - *Auto gate:* server log shows the offset resolved, `health` get, set-to-marker, state-change, and a matching readback against a bot pawn; the server never crashes; Slice 1/2 demos still fire (regression).
  - *Manual HUD (operator):* connect a CS2 client, `s2_sethp 1234`, observe the HUD health change to `1234`.
  - Reuses `scripts/build-sniper.sh`, the Docker harness (the 64 GB `cs2-data` copy is in place), and `scripts/rcon.py`; the demo arms after a live-frame threshold (the server barely ticks during boot).

## 8. Acceptance criteria

1. `cargo test -p s2script-core -- --test-threads=1` passes (new schema-cache + memory-helper unit tests + the bridge integration tests + all Slice 0/1/2 tests); the boundary gate (crate-dep **and** the new name-leak grep) stays green; sniper build produces loadable binaries.
2. Core contains **no CS2 identifier**; the `pawn.health` accessor + CS2 names live entirely in `games/cs2/` (JS).
3. The `m_iHealth` offset is resolved **live from SchemaSystem** at runtime (not hardcoded / not in gamedata); a missing class/field degrades that accessor with a named reason without crashing.
4. **Auto gate (live):** on the server, `pawn.health` reads a bot pawn's health, writes a marker value, calls the state-change, and a readback confirms the write — all in the log, no crash.
5. **Manual HUD (live):** an operator-connected client sees their HUD health change after `s2_sethp <value>` — the setter's `NetworkStateChanged` networks the change.
6. Reproduces from the README (sniper build + Docker runbook + the auto gate + the manual HUD step).

## 9. Out of scope (Slice 3)

The full schema **codegen** pipeline (Slice 5 — resolve the one field by name at runtime now, no generated `.d.ts`); the durable handle / `EntityRef` staleness-wrapper system (Slice 5 — a minimal raw entity pointer/handle here, block-scoped, never stored across `await`); the command framework — filters, targeting, permissions, reply routing (Slice 5 — a raw one-off ConCommand only); plugin loader / `.s2sp` / hot-reload (Slice 4 — the cs2 JS is eval'd at boot); any second field, entity type, or write type beyond `i32` health; per-plugin identity/ledger (Slice 4). Note later needs as TODOs and stop.

## 10. File structure / deliverables

- `core/src/` (new module(s), e.g. `schema.rs` / `entity.rs`) — the engine-generic natives (§3), with unit tests. `core/src/v8host.rs` — install the new natives + the cs2-file load path. `core/src/ffi.rs` / `shim/` — acquire SchemaSystem (currently deferred), pass the SchemaSystem pointer + the resolved cs2-JS path (and entity-system pointer/signature result if resolved shim-side) across the C ABI. `core/src/lib.rs` — `mod` the new module(s).
- `games/cs2/js/pawn.js` (new) — the `@s2script/cs2` accessor + names + walk + the two demos' cs2-side logic. `games/cs2/` Rust crate: unchanged (empty sentinel).
- `gamedata/core.gamedata.jsonc` — SchemaSystem string already present; add an entity-system signature **only if** §11 finds one is needed.
- `shim/include/s2script_core.h` — extend the C ABI for SchemaSystem + the cs2-path (and any entity-system pointer) handoff.
- `scripts/check-core-boundary.sh` (or a companion) — add the CS2-name-leak grep gate.
- README — the Slice-3 auto gate + manual HUD runbook + acceptance table.
- Sniper build + Docker live gate + `scripts/rcon.py` reused.

## 11. Open items to validate during implementation

Resolved via the hl2sdk headers + a live in-process dump/spike (like Slice 2's rusty_v8 items), each degrading with a named reason if not found:
- The exact **SchemaSystem** call sequence to resolve `(class, field) → offset` in-process (type scope acquisition, `FindDeclaredClass`, field iteration).
- The **entity-system** access path: whether `CEntityInstance*`-by-index is reachable via an existing interface/pointer or needs a single gamedata **signature** for the entity-system global; if a signature is needed, it goes in `gamedata/`.
- The **`slot → controller → pawn`** specifics: the controller-by-slot mapping (entity index convention vs a player-manager lookup) and dereferencing `m_hPlayerPawn`.
- The Source 2 **`NetworkStateChanged`** invocation (the `CEntityInstance`/chain-of-changes call that marks the field dirty for the client).
- The Source 2 **ConCommand** registration path and how the callback surfaces the calling client slot + args.
- Whether **SchemaSystem acquisition** (currently deferred in the shim) is via the engine/server factory or a module factory, and where the pointer is stored for core to query.
