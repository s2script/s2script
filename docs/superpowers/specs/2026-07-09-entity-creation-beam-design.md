# Slice: entity-creation lifecycle primitive + beam drawing

**Date:** 2026-07-09
**Status:** design approved — proceeding to plan
**Reference:** [edgegamers/TTT](https://github.com/edgegamers/TTT) (a CounterStrikeSharp CS2 plugin) — its
`env_beam` usage in `PropMover.cs` (a hold-E prop-grab whose beam follows the player's aim each tick) and
`Items/Tripwire/TripwireItem.cs` is the exact CS2 beam mechanism we mirror.

## Motivation

`@s2script/trace` (the prior slice) computes *where* a player is looking (`pawn.aimTrace()` → a start eye
point + a hit `endPos`), but s2script has **no way to render anything in the world** — no beam, particle,
overlay, or **entity-creation** primitive at all. We can read and write *existing* entities but have never
**spawned** one.

The user's concrete ask — "draw a visible beam when a player presses E" — is really two things:
1. a foundational **entity-creation lifecycle primitive** (spawning a new entity is a charter-level gap that
   unlocks props, particles, markers, ragdolls — everything spawn-based), and
2. a **beam** helper as its first consumer + the live-gate proof.

## The proven CS2 beam pattern (from the reference)

```
beam = CreateEntityByName("env_beam")          // create — THE missing primitive
beam.m_nRenderMode = kRenderTransAlpha          // uint8 (RenderMode_t)
beam.m_flWidth     = 2.0                         // float32
beam.m_clrRender   = <RGBA>                      // Color (4-byte RGBA)
beam.m_vecEndPos   = <end>                       // Vector + SetStateChanged("CBeam","m_vecEndPos")
beam.Teleport(<start>)                           // sets the start point (entity origin)
beam.DispatchSpawn()                             // register/activate
// each tick to follow aim:  Teleport(newStart) + rewrite m_vecEndPos + SetStateChanged
// cleanup:                   UTIL_Remove(beam)
```

`env_beam` self-renders a colored point-to-point beam with only those fields (no sprite/model set in the
reference). Start = the entity's own origin (set via `Teleport`); end = `m_vecEndPos`.

## Scope

**In:**
- An engine-generic entity-lifecycle primitive: `createEntity(className) → EntityRef | null`, and on
  `EntityRef`: `.spawn()`, `.teleport(origin, angles?, velocity?)`, `.remove()`.
- A CS2 `Beam` helper: `Beam.draw(start, end, opts?) → BeamHandle`, `handle.update(start, end)`,
  `handle.remove()`.
- A hold-E laser-sight demo plugin (the live gate).

**Out (YAGNI / deferred, do NOT build ahead):**
- Temp-entity beams via usermessage; `IVDebugOverlay` lines; particle-system beams (rejected in
  brainstorming — `env_beam` is the real multiplayer-visible, updatable route).
- Ledgered / auto-removed created entities (see Lifecycle policy — SM doesn't auto-remove either).
- Spawning with `CEntityKeyValues` (keyvalue-configured spawn); non-beam entity types beyond what the demo
  needs; `AcceptInput`/entity-IO; a generic (non-CS2) beam module.
- Promoting `Beam` to an engine-generic `@s2script/beam` (possible later if a second Source 2 game needs it).

## Architecture

### 1. Core ops (engine-generic) — `S2EngineOps`

Four ops, **ABI-appended after the current last op (`trace_shape`)**, kept consistent across the C header
(`shim/include/s2script_core.h`), the Rust mirror (`core/src/v8host.rs`), **both** in-isolate test op-structs,
and the shim wiring:

| op | signature | returns |
|----|-----------|---------|
| `entity_create` | `(const char* className)` | a packed `CEntityHandle` (index+serial), or 0 on failure |
| `entity_dispatch_spawn` | `(int index, int serial)` | bool (serial-gated) |
| `entity_teleport` | `(int index, int serial, const float* origin/*3, nullable*/, const float* angles/*3, nullable*/, const float* velocity/*3, nullable*/)` | bool |
| `entity_remove` | `(int index, int serial)` | bool |

All degrade (return 0 / false / no-op) when their signature is unresolved or the serial is stale — never a
crash (the universal op contract).

### 2. Shim — sig-scanned engine functions

Four functions resolved by **self-validated byte signatures** (a new `.signatures` gamedata block in
`gamedata/core.gamedata.jsonc`; borrowed from CSSharp/ModSharp as *hints*, re-scanned + validated UNIQUE in
`.text` of our pinned `libserver.so` by the gate — the "good", loud-on-break kind of RE, unlike the silent
client-list offsets):

- **`UTIL_CreateEntityByName(const char* name, int forceEdictIndex=-1) → CBaseEntity*`** — seed
  `48 8D 05 ? ? ? ? 55 48 89 FA` (CSSharp `UTIL_CreateEntityByName`, lib `server`). The shim calls it with
  `(className, -1)`, then converts the returned `CBaseEntity*` to a serial-gated ref via
  `GetRefEHandle().ToInt()` → the existing `decode_handle`/`build_entity_ref` path (the DamageInfo.victim
  pattern). **The raw pointer never crosses to JS** (charter).
- **`CBaseEntity::DispatchSpawn(CEntityKeyValues* = nullptr)`** — seed
  `48 85 FF 74 ? 55 48 89 E5 41 55 41 54 49 89 FC` (CSSharp `CBaseEntity_DispatchSpawn`). Resolve the entity
  ptr from `(index, serial)` serial-gated, then call with a null keyvalues arg.
- **`CBaseEntity::Teleport(const Vector* origin, const QAngle* angles, const Vector* velocity)`** — **no
  ready CSSharp seed** (its key is empty). The offline spike resolves it from the ModSharp gamedata or by
  self-RE on the pinned binary (a well-known teleport function). Any of the three args may be null (we pass
  origin, null, null for the beam start).
- **`UTIL_Remove(CBaseEntity*)`** — seed `48 89 FE 48 85 FF 74 ? 48 8D 05 ? ? ? ? 48` (CSSharp `UTIL_Remove`).

`SetStateChanged` needs no new function — it is the existing `notifyStateChanged` /
`__s2_ent_ref_state_changed` native (`NetworkStateChanged(offset)`, from Slice 5A).

### 3. `@s2script/entity` (engine-generic module)

- `createEntity(className: string): EntityRef | null` — over `entity_create`; null on failure. Returns a
  live, serial-gated `EntityRef` bound to the created entity.
- `EntityRef.spawn(): boolean` — `entity_dispatch_spawn`.
- `EntityRef.teleport(origin: Vec3, angles?: Vec3, velocity?: Vec3): boolean` — `entity_teleport`
  (`{x,y,z}` copied to a float triple; a nullable arg maps to a null pointer).
- `EntityRef.remove(): boolean` — `entity_remove`.

No hardcoded class or field names — `className` is a parameter. Boundary-clean.

### 4. CS2 `Beam` (game layer — `games/cs2/js/beam.js` + `packages/cs2`)

The only code that knows the CS2 schema facts (`env_beam`, `CBeam.m_vecEndPos`, `m_flWidth`,
`CBaseModelEntity.m_clrRender`, `m_nRenderMode`), resolved **live** via `__s2_schema_offset(...)` + existing
raw `EntityRef` writes — **no schema-codegen regeneration** (the self-contained pawn.js pattern):

- `Beam.draw(start: Vec3, end: Vec3, opts?: { color?: [r,g,b,a], width?: number }): BeamHandle | null`
  1. `ref = createEntity("env_beam")` (null → return null)
  2. `ref.writeUInt8(off(m_nRenderMode), kRenderTransAlpha)` — value from `RenderMode_t` (pin in impl)
  3. `ref.writeFloat32(off(m_flWidth), width ?? 2.0)`
  4. `ref.writeUInt32(off(m_clrRender), packRGBA(color ?? [255,0,0,255]))` — packed little-endian `r|g<<8|b<<16|a<<24`
  5. write `m_vecEndPos` as 3 float32 + `notifyStateChanged(off(m_vecEndPos))`
  6. `ref.teleport(start)`  →  `ref.spawn()`  (reference order: fields → Teleport → DispatchSpawn)
  7. return `{ ref, update, remove }`
- `handle.update(start, end)` — `ref.teleport(start)` + rewrite `m_vecEndPos` + `notifyStateChanged` (the
  per-tick follow path).
- `handle.remove()` — `ref.remove()`.

`writeUInt8`/`writeFloat32`/`writeUInt32`/`writeFloats` and `notifyStateChanged` all already exist.

### 5. Demo — hold-E laser sight (`plugins/beam-demo`, CS2)

Reuses the existing button-mask poll (the menu renderer's `m_nButtons.m_pButtonStates` chain,
`IN_USE = 32`, rising/falling-edge detection):
- On a lazy `OnGameFrame` poll, per live pawn: read the button mask, edge-detect E.
- **E held:** `hit = pawn.aimTrace()`; if a beam exists → `handle.update(eyeOf(pawn), hit.endPos)`, else
  `handle = Beam.draw(eye, hit.endPos, { color:[255,0,0,255], width:2 })`. (Eye = the beam start; the aim
  hit-point = the end.)
- **E released / player disconnects / plugin unloads:** `handle.remove()`; clear the per-player entry.

One beam per player, tracked in a slot→handle map, cleaned up by the plugin (see Lifecycle policy).

## Boundary analysis (both CI gates stay green)

- **Core / `@s2script/entity`:** the 4 ops are `className`-parameterized or take `(index, serial)` — **zero**
  hardcoded schema class/field names. `createEntity` returns a serial-gated `EntityRef` (raw `CBaseEntity*`
  converted shim-side, never crossing to JS). `CBaseEntity`/`UTIL_*`/`Teleport` are Source 2 engine
  primitives, not CS2 game identifiers — engine-generic. Passes the `check-core-boundary` gate.
- **CS2 layer:** `Beam` (the sole code naming `env_beam`/`CBeam` fields) lives in `games/cs2` + `packages/cs2`
  alongside `pawn.js`. Passes `test-boundary-nameleak`.

## Lifecycle policy (SourceMod parity)

Created entities are **game-world-owned and NOT auto-ledgered**. Spawning an entity that outlives the plugin
is legitimate (SM does not auto-remove entities on plugin unload). The **plugin** owns cleanup: the demo
tracks its per-player beam and removes it on release-E, `Clients.onDisconnect`, and `onUnload`. (An opt-in
*ledgered* entity that auto-removes on teardown is a deferred YAGNI.)

## Testing

- **In-isolate (core):** each op degrades with no live engine — `createEntity → null`,
  `spawn/teleport/remove → false/no-op` — asserted like every prior op. Assert the `Beam` field-write math
  (packed RGBA; the `m_vecEndPos` float triple) as a pure unit where possible.
- **Live gate — split by what bots can prove:**
  - *Bot-provable (the mechanism, the real deliverable):* create an `env_beam` at fixed coords →
    `createEntity` returns a **valid** `EntityRef` (`isValid()===true`), `spawn`/`teleport`/`remove` all
    succeed, `Player.forSlot`/server keeps ticking, `RestartCount=0`, no crash. This proves the entire
    entity-lifecycle primitive live.
  - *Human-provable (the visual):* a human client joins, holds E, and **sees** a red laser from their gun
    tracking their crosshair; release → gone. Same human-client ceiling as SayText2 / menus / damage → a
    **deferred human-client live test**, but the primitive is already bot-proven.

## Risks

1. **`Teleport` signature** (the one without a ready seed) — resolved in the offline spike; if it resists, the
   beam start can fall back to an origin-field write, but `Teleport` is the clean networked path.
2. **`env_beam` rendering fields** — the reference ships with only RenderMode/Width/Render/EndPos, so those
   suffice; if a sprite/material is required to render, the human live gate surfaces it (a field add, not a
   redesign).
3. **Treadmill** — 4 more per-update signatures, but **gate-validated** (loud on break), not silent offsets.
4. **UAF discipline** — `createEntity`'s returned ref is serial-gated like every EntityRef; a stale beam
   handle reads/writes as no-op, never garbage.

## Sequencing — one slice, spike-first

One coherent thread (the demo is the live gate). Tasks:

0. **Offline sig spike** — resolve all 4 functions (create/spawn/teleport/remove) on the pinned
   `libserver.so`, validate each UNIQUE in `.text`, record patterns + resolve types in
   `gamedata/core.gamedata.jsonc`. De-risks the RE before committing (our standard pattern; `Teleport` is the
   focus).
1. **Core ops + shim** — the 4 `S2EngineOps` ops (ABI-appended after `trace_shape`), the shim sig-scan +
   call/convert logic, in-isolate degrade tests.
2. **`@s2script/entity` methods** — `createEntity` + `EntityRef.spawn/teleport/remove` + the types package.
3. **CS2 `Beam`** — `beam.js` + `packages/cs2` types.
4. **Hold-E demo + live gate** — the laser plugin; bot-prove the mechanism, document the human visual as a
   deferred live test.

Needs one sniper rebuild (the ops + shim). Related: [[re-gamedata-strategy]], [[cs2-schema-entity-access]],
[[deferred-live-tests]], the trace slice (`2026-07-08-trace-design.md`).
