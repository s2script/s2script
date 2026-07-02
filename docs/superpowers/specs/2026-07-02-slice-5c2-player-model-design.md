# Slice 5C.2 — The Player Model

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5c2-player-model` (off `main`, which has Slices 0–5A + entref-wire + 5B + 5C.1 merged).
**Parent:** Slice 5C. 5C.1 = the module split (done); 5C.2 = this (the player model); 5C.3+ = std breadth.

---

## 1. Goal

Give CS2 an honest **player** abstraction. CS2 splits SourceMod's single "client" into two entities — the
**controller** (`CCSPlayerController`: the persistent player — team/score/ping; survives death; ≈ SM's
"client") and the **pawn** (`CCSPlayerPawn`: the in-world body — health/position; respawns). Today only
`Pawn.forSlot(slot)` exists (pawn-first, hiding the controller — the "pawn is the client" conflation).
5C.2 lands a `Player` (the controller) with `.pawn` navigation, iteration, and slot-based entry, consolidating
on **`Player`** as the primary abstraction (the cross-Source2 concept). Structural only — no core change.

## 2. What exists (merged)

- **The 5B.3 generated accessors** (`games/cs2/js/schema.generated.js` runtime + `packages/cs2/schema.generated.d.ts`
  types): `CCSPlayerController` exposes `team`(m_iTeamNum-ish)/`score`/`ping`/… and handle fields
  `playerPawn`(m_hPlayerPawn)/`pawn`(m_hPawn)/`observerPawn`; `CCSPlayerPawn` exposes `health`/`friction`/…
  and `controller`(m_hController). The generated `globalThis.__s2pkg_cs2_schema` exposes
  `applyAccessors(proto, className)` (defines a class's accessors on a prototype, `configurable:true`) and
  `wrap(className, ref)`.
- **`games/cs2/js/pawn.js`** (hand-written cs2 runtime, concatenated after `schema.generated.js`): `function
  Pawn(ref)` + `applyAccessors(Pawn.prototype, "CCSPlayerPawn")` + `Pawn.forSlot(slot)` (controller entity
  index `slot+1` → resolve `m_hPlayerPawn` → pawn `EntityRef` → `new Pawn`) + `globalThis.__s2pkg_cs2 = { Pawn }`.
- **`packages/cs2/index.d.ts`**: `interface Pawn extends CCSPlayerPawn { readonly ref: EntityRef }` + the
  `Pawn` const (`forSlot`).
- **`@s2script/entity` `EntityRef`** — serial-gated, `T | null`; `readHandle(off) → EntityRef | null` (decodes
  a `CHandle`, validates, returns a live ref); `__s2_ent_current_serial(index)`; `__s2_schema_offset(cls,field)`.

## 3. Decisions locked during brainstorming

1. **Structural, JS-only.** The Player model is built purely in the `@s2script/cs2` JS + types layer — NO
   core/shim/`package-addon` change, NO sniper rebuild (like 5B.3). Add it to the existing `games/cs2/js/pawn.js`
   (Player + Pawn coexist, one concatenated file — keeps the shim's single-file load unchanged).
2. **`Player` is the primary abstraction** (the cross-Source2 player concept), wrapping the **controller**
   `EntityRef`. Referenced by **slot** now, safely (serial-gating already gives non-dangling stored refs —
   the reason CSSharp re-looks-up by slot/userid; a stored `Player` degrades to `null` on reuse, no UAF).
3. **Engine-backed identity deferred to the follow (5C.2b).** `player.userId` + `Player.fromUserId`,
   `player.name`, `player.steamId` are all engine-only (no schema field; `m_steamID` is `uint64`, `m_iszPlayerName`
   is `char[128]`, `m_iConnected` is an enum — all deferred kinds; userId isn't in the schema at all). They
   need S2EngineOps natives (core/shim) and land together as the engine-identity follow. Also deferred:
   `fromClient` (1-based bridge — userId is the intended stable key). The 5C.2 API is *shaped to accommodate*
   these later.
4. **Slot is 0-based** (`CPlayerSlot`); the controller entity index is `slot+1` (matching `Pawn.forSlot`).

## 4. Architecture

Everything is added to `games/cs2/js/pawn.js` (+ `packages/cs2/index.d.ts` types). `Player` mirrors the
`Pawn` construction pattern: a constructor over the controller `EntityRef`, the generated `CCSPlayerController`
accessors applied to its prototype, then hand-written navigation that **shadows** the raw generated handle
accessors with typed ones (`configurable:true` makes the override clean).

- **`Player`** — `function Player(ref) { this.ref = ref; }` where `ref` is the **controller** `EntityRef`.
  `applyAccessors(Player.prototype, "CCSPlayerController")` gives `team`/`score`/`ping`/… (and raw
  `playerPawn`/`pawn`/`controller` handle fields). Then the hand-written members below are defined on the
  prototype, overriding where they collide.
- **`Pawn`** — unchanged construction (`applyAccessors(Pawn.prototype, "CCSPlayerPawn")`), plus a hand-written
  typed `controller` navigation that shadows the generated raw `controller`.
- Cross-references (`player.pawn → Pawn`, `pawn.controller → Player`) resolve within the one IIFE (both
  constructors defined before the getters run).

## 5. API surface

**`Player`:**
- `player.ref` → the serial-gated controller `EntityRef`.
- `player.slot` → `number` (0-based) — stored at construction (`player.ref.index - 1`).
- generated `CCSPlayerController` accessors: `player.team`, `player.score`, `player.ping`, … (all `T | null`).
- `player.pawn` → **`Pawn | null`** — `this.ref.readHandle(off("CCSPlayerController","m_hPlayerPawn"))`, wrapped
  as a `Pawn` (or `null`). Shadows the raw generated `pawn`(m_hPawn).
- `Player.fromSlot(slot)` → **`Player | null`** — `new EntityRef(slot+1, __s2_ent_current_serial(slot+1))`;
  `null` if the controller isn't valid OR the slot is unoccupied. **Occupancy filter (live-verified):** CS2
  pre-allocates all 64 controller entities, so `isValid()` (entity-exists) does NOT distinguish an occupied
  slot from an empty one — and `m_iConnected` reads `0` for both (unusable here). The clean, schema-readable
  occupancy signal is that an occupied controller has a **live player pawn** (`m_hPlayerPawn` → `readHandle`
  ≠ null). So `fromSlot` returns a `Player` only when the controller has a pawn (in-game/spawned players);
  connected-but-pawnless (dead/spectating) is deferred to the engine-identity/connection follow.
- `Player.all()` → **`Player[]`** — iterate `slot` in `0..MAX_PLAYERS-1` (`MAX_PLAYERS = 64`), `fromSlot`, keep
  the non-null (occupied) results = the in-game players.

**`Pawn`** (additions):
- `pawn.controller` → **`Player | null`** — `this.ref.readHandle(off("CBasePlayerPawn","m_hController"))`,
  wrapped as a `Player`. Shadows the raw generated `controller`(m_hController).
- `Pawn.forSlot` — unchanged (back-compat shortcut for "the pawn at slot N").

**`globalThis.__s2pkg_cs2 = { Pawn, Player }`.** Types in `packages/cs2/index.d.ts`. NOTE: the typed nav
properties **shadow** generated handle fields whose generated type is `EntityRef | null` (`CCSPlayerController.pawn`
= m_hPawn; `CCSPlayerPawn.controller` = m_hController). A derived interface cannot re-type an inherited property
incompatibly (`Pawn | null` isn't assignable to `EntityRef | null`), so the interfaces must **`Omit` the
shadowed field** before re-adding the typed one:
- `export interface Player extends Omit<CCSPlayerController, "pawn"> { readonly ref: EntityRef; readonly slot: number; readonly pawn: Pawn | null; }`
- `export interface Pawn extends Omit<CCSPlayerPawn, "controller"> { readonly ref: EntityRef; readonly controller: Player | null; }`

plus the `Player` const (`fromSlot(slot): Player | null; all(): Player[]`) and the existing `Pawn` const
(`forSlot`). (`player.playerPawn` — the raw generated `m_hPlayerPawn` `EntityRef` — remains available; `player.pawn`
is the ergonomic typed one.)

## 6. Data flow

`Player.fromSlot(0)` → controller `EntityRef(1, serial)` → `isValid()` → `Player`. `player.team` → generated
getter → `readInt32(off(...))` → serial-gated → `number | null`. `player.pawn` → `readHandle(m_hPlayerPawn)`
→ pawn `EntityRef | null` → `Pawn | null`. `pawn.controller` → `readHandle(m_hController)` → `Player | null`.
`Player.all()` → 64× `fromSlot` (each an `isValid()`), filtered.

## 7. Error handling / degrade

Unchanged posture — everything is `EntityRef`-backed and serial-gated. An empty slot / stale controller →
`fromSlot` returns `null`; a dead/absent pawn → `player.pawn` is `null`; a stored `Player` whose controller
entity is destroyed reads `null`/degrades (no dangling). `readHandle` returns `null` on an invalid handle.
No raw pointer crosses to JS. No new failure modes.

## 8. Testing & acceptance

**In-isolate (`frame_tests`, core — reuses the injected-cs2 test harness):** with a registered cs2 prelude and
`set_engine_ops(None)`, `Player.fromSlot(0)` degrades (`null`), a constructed `Player`'s generated accessor +
`.pawn` degrade to `null`, `Player.all()` is `[]`, `pawn.controller` is `null` — proving the wiring (a plain
object couldn't) without a live entity. (If the existing cs2-prelude in-isolate harness can't host the
generated `schema.generated.js`, this coverage moves to the live gate and the in-isolate test asserts the
pure `fromSlot`/`slot` arithmetic + null-degrade via a stub.)

**Live-only (Docker CS2):** a plugin uses `Player.all()` to iterate the connected players, reads a
`player.team` (and `player.score`), hops `player.pawn` → `health`, and the reverse `pawn.controller` → the
`Player`; after a `bot_kick`/disconnect the reads go `null` and iteration drops the slot, server ticking, no
crash. **No sniper rebuild** (no core/shim change) — only the demo `.s2sp` + the repackaged addon JS.

**Acceptance:**
1. `cargo test -p s2script-core` green (+ any in-isolate wiring test); the CLI `node:test` suite green; both
   boundary gates green; `check-schema-generated.sh` green (unchanged).
2. `s2script build` produces the demo `.s2sp` using `Player`.
3. Live gate: `Player.all()` iterates players, `player.team`/`player.pawn.health` read, `pawn.controller`
   round-trips, all `null` on death/disconnect, no crash.
4. README documents the `Player` model + the controller/pawn split; CLAUDE.md "Current state" updated (5C.2
   done; focus → the engine-identity follow / 5C.3).

## 9. File structure

- **Modify** `games/cs2/js/pawn.js` — add the `Player` constructor + `applyAccessors(Player.prototype,
  "CCSPlayerController")` + `player.slot`/`player.pawn` + `Player.fromSlot`/`Player.all` + the typed
  `Pawn.prototype.controller`; export `{ Pawn, Player }`.
- **Modify** `packages/cs2/index.d.ts` — the `Player` interface + const + `Pawn.controller`.
- **Modify** a demo (`examples/demo-plugin`) to iterate `Player.all()` + navigate; `README.md`, `CLAUDE.md`.

No core, shim, `package-addon.sh`, or `@s2script/std`/module change. CS2 identifiers stay in `games/cs2` +
`packages/cs2` (both boundary gates stay green).

## 10. Scope & deferrals

**Scope:** the `Player` abstraction (controller wrapper + generated accessors + `.slot`/`.pawn` + `fromSlot`/
`all`), the `Pawn.controller` reverse nav, the types, the demo + live gate.

**Deferred — do NOT build:** the engine-identity follow (`player.userId` + `Player.fromUserId`, `player.name`,
`player.steamId` — need S2EngineOps natives); `fromClient` (1-based bridge); the full SM `GetClient*` parity
surface (incremental → Slice 6); the `@s2script/cs2` internal module split; a `maxplayers`/`connected` engine
op (iterate a fixed range for now); the base-plugin suite (6); the registry (5.5); config/permissions; the
`tsc` gate; `enum`/`Vector`/string codegen; the 5B.3 codegen post-merge TODOs.

## 11. Global constraints (bind every task)

- **Core stays engine-generic.** All Player/Pawn code + CS2 identifiers live in `games/cs2` + `packages/cs2`;
  NOTHING enters `core/src`. Both gates green (`check-core-boundary.sh`, `test-boundary-nameleak.sh`).
- **Never expose a raw pointer across time.** `Player`/`Pawn` are `EntityRef`-backed; every access is
  serial-gated → `T | null`; `.pawn`/`.controller` return typed wrappers over `readHandle` (an `EntityRef`),
  never a raw pointer; a stored `Player` degrades to `null` on reuse.
- **Layout is data.** Fields resolve live via the generated accessors + `__s2_schema_offset`; no offsets baked.
- **Deterministic codegen stays green.** 5C.2 touches no generated file; `check-schema-generated.sh` stays green.
- **Naming:** PascalCase types (`Player`), camelCase props/fns (`player.pawn`, `Player.fromSlot`, `player.slot`).
- **Commit trailer** on every commit; commit only on `slice-5c2-player-model`; do NOT push.
