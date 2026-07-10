# Zones sub-slice 1 — trigger-zone spine + feasibility spike — design

**Date:** 2026-07-10
**Status:** approved (sub-slice 1 of the 3-slice zone system; a feasibility SPIKE)

## Goal

Prove — end-to-end, live on a bot — that a **runtime-created `trigger_multiple` with a programmatic axis-aligned bounding box** detects a player and drives an `OnZoneEnter`/`OnZoneLeave` event. A single **hardcoded / command-placed** zone; **no persistence, no inter-plugin interface, no operator CRUD** (those are sub-slices 2 and 3). This is the risk-retiring slice: if runtime bbox triggers don't fire touch in CS2, we discover it here and learn exactly what's needed (a spawnflags/collision recipe, or a new engine primitive) before building persistence and the consumer API on top.

## Why a spike

The whole 3-slice zone system rests on one unproven assumption: that we can spawn a `trigger_multiple` at runtime, give it a bbox from coordinates, and have the engine fire touch for players. CSSharp and ModSharp only ever hook **existing map-authored** triggers (`HookEntityOutput("*", "OnStartTouch", …)`) — neither creates a runtime touchable trigger. So the recipe is genuinely unknown and must be established empirically. This slice's deliverable is **knowledge + a working minimal mechanism**, not a shipped feature.

## What we already have (no new primitive assumed at the outset)

- `createEntity(className, keyvalues?)` + `EntityRef.spawn()` + `teleport(origin)` (entity-creation + EKV slices).
- `Entity.onOutput(classname, output, handler)` — hooks I/O outputs; the handler gets `activator`/`caller` as serial-gated `EntityRef`s (entity-I/O slice). `trigger_multiple`'s `OnStartTouch`/`OnEndTouch` are exactly such outputs; `activator` = the touching pawn.
- `EntityRef.acceptInput(input, value?, …)` — fire inputs (e.g. `Enable`).
- Schema field writes: `EntityRef.writeFloat32(off, v)` / `writeUInt8` / `writeBool` + `notifyStateChanged(off)`; `__s2_schema_offset(class, field)`. A `Vector` write = 3× `writeFloat32` (the `pawn.setVelocity` pattern).
- `EntityRef.readHandleVector(ptrOffs, vectorOff, maxCount)` — reads a `CUtlVector<CHandle>` (items slice). `CBaseTrigger.m_hTouchingEntities` is such a field.
- `pawn.origin` (world position) + `Player`/`Pawn.forSlot` + `Vector` math + `OnGameFrame`.

## The recipe to test (the hypothesis)

Schema facts (from the catalog): `CTriggerMultiple : CBaseTrigger : CBaseToggle`; the bbox lives on the embedded `CBaseModelEntity.m_Collision` (a `CCollisionProperty`) with `m_vecMins`/`m_vecMaxs` (Vector), `m_nSolidType` (`SOLID_BBOX = 0x2`), `m_usSolidFlags`, `m_CollisionGroup`, `m_triggerBloat`. `CBaseTrigger` has `m_bDisabled` (bool), `m_OnStartTouch`/`m_OnEndTouch` (outputs), `m_hTouchingEntities` (`CUtlVector<CHandle>`), `m_spawnflags` (on `CBaseEntity`).

The primary spawn recipe (a zone defined by world-space `min`/`max` corners → center `c = (min+max)/2`, half-extents `h = (max−min)/2`):

1. `createEntity("trigger_multiple", { "spawnflags": "<player-touch flags>", "wait": "0", "StartDisabled": "0" })` (EKV — the entity's own `Spawn()` parses keyvalues).
2. Configure the collision (post-create, pre- or post-spawn — the spike determines which works): `m_nSolidType = SOLID_BBOX`; `m_Collision.m_vecMins = -h`; `m_Collision.m_vecMaxs = +h` (3 float writes each at `schemaOffset("CBaseModelEntity","m_Collision") + schemaOffset("CCollisionProperty","m_vecMins")` etc.); `m_bDisabled = false`; solid flags as needed. `notifyStateChanged` each.
3. `spawn()` (`DispatchSpawn`).
4. `teleport(center)` — place the trigger; mins/maxs are origin-relative.
5. Hook detection:
   - **Primary (event-driven):** `Entity.onOutput("trigger_multiple", "OnStartTouch", h)` + `"OnEndTouch"` → resolve `activator` → `OnZoneEnter(activator, "test")` / `OnZoneLeave`.
   - **Fallback (engine-collision, poll-derived):** each `OnGameFrame` (throttled), read `m_hTouchingEntities` (`readHandleVector`); diff against the previous set → enter/leave. This still proves the ENGINE detects the runtime trigger (it maintains the touch list); we just derive the events framework-side.

## Iteration ladder (what the spike tries if touch does NOT fire)

Front-load these in the live loop; each is cheap to try because it's a JS-only redeploy (the trigger recipe is all plugin-side unless a new op is needed):

1. **Spawnflags** — try the known trigger spawnflags for "everything / clients / players" (enumerate a few values; the map-trigger convention is a small set).
2. **Enable input** — `acceptInput("Enable")` after spawn (some triggers spawn disabled regardless of `StartDisabled`).
3. **Set bbox before vs after `DispatchSpawn`** — Spawn() may recompute collision bounds; try both orders.
4. **`m_hTouchingEntities` poll** — if the output never fires but the list populates, the engine IS detecting touch → ship the poll-derived path (still "real trigger" collision) and note the output-hook gap.
5. **Collision-rules refresh** — if the bbox writes don't take effect until the entity re-registers with the spatial partition, we need a `CollisionRulesChanged()` / `CBaseEntity::SetSize` / partition-reinsert call — **this is the one place a NEW engine op (a `collision_rules_changed(index,serial)` or `entity_set_size(index,serial,mins,maxs)` shim call) would be introduced.** The spike identifies precisely which, grounded in the SDK, and only then.

**Stop rule:** if after the ladder neither the output hook nor `m_hTouchingEntities` reflects a player inside the box, STOP and report — the runtime-trigger approach is not viable without deeper RE (e.g. a full `CBaseTrigger::Spawn`/`Enable` game-function call chain), and we reconsider (a game-function-call op, or fall back to origin-polling for the zone system). Do not thrash.

## Architecture / boundary

- **A plugin** (`plugins/zones-spike` in `examples/`, or the nascent `@s2script/zones` — name TBD in the plan) depending on `@s2script/entity`, `@s2script/cs2`, `@s2script/commands`. All zone logic is game-layer.
- The bbox-write helper uses CS2/Source2 schema field names (`m_Collision`, `m_vecMins`, `SOLID_BBOX`) → stays in the plugin (which depends on `@s2script/cs2`) or a small `pawn.js` CS2 helper; **never core**.
- If the ladder forces a new op (collision-rules/set-size), it is **engine-generic** (`CBaseEntity`/`CCollisionProperty` are Source2 types) → core/shim, both boundary gates green. Expect NO new op in the happy path.

## Testing

**In-isolate (core):** none expected (no core change in the happy path). If the ladder introduces a shim op, it gets the standard degrade test + ABI-append.

**Live gate (de_dust2 / de_inferno, `bot_quota 2+`, rcon) — fully bots-provable:**
- `sm_zonetest` spawns a hardcoded box (placed to overlap or sit in a bot's path — e.g. centered on slot 0's `pawn.origin` with ±96u half-extents, or a large box over a spawn) + installs the hooks; boot/GAMEDATA unchanged (`N ok, 0 FAILED`) in the happy path.
- **PASS:** `[zonestest] OnZoneEnter: player=<name> zone=test` logs when a bot is inside / walks in, and `OnZoneLeave` when it exits (via the output hook OR the `m_hTouchingEntities` poll — the spike reports WHICH worked). The `activator` resolves to a valid `EntityRef`/`Player`.
- Server keeps ticking, `RestartCount=0`, no crash across repeated create/enter/leave/remove.
- `sm_zonetest_clear` removes the trigger (`EntityRef.remove()`), no dangling hook / no crash.

The spike REPORTS: which detection path fired (output vs poll), the exact working recipe (spawnflags, bbox order, any Enable/collision call), and whether a new engine op is needed for sub-slice 2.

## Deferred (later sub-slices / not this spike)

- Persistence (DB + JSON import/export), per-map `onMapStart` load, operator CRUD (`sm_zone_add`/`delete`/`list`) — **sub-slice 2**.
- The `publishInterface("@s2script/zones", …)` consumer API, `OnZoneStay`, tags/types, beam visualization, the editing UX — **sub-slice 3**.
- Non-box shapes (cylinder/sphere), non-player entity detection, filters (`m_hFilter`).

## Slice shape

Exploratory. Likely a JS-only plugin (no sniper) in the happy path; a sniper rebuild only if the ladder forces a collision op. **Executed interactively (controller-driven) with live iteration** — a spike needs the live touch/no-touch feedback to walk the recipe ladder, which a fire-and-forget workflow can't see. Live gate → report the working recipe → feeds sub-slice 2's plan.
