# Zones sub-slice 3 — the plugin-developer interface — design

**Date:** 2026-07-10
**Status:** approved (sub-slice 3 of the 3-slice zone system — the platform-making piece)

## Goal

Turn `@s2script/zones` from a self-contained manager into a **platform**: publish a versioned inter-plugin interface so any plugin can subscribe to zone events (`enter`/`leave`/`stay`) and query/manage zones, then prove it with a *separate* consumer plugin that reacts to those events. This is the sub-slice the whole decomposition was oriented around ("plugin-developer event API" was the chosen v1 priority). The operator-UX polish (beam visualization, in-game corner editor, tags) is deferred to a later sub-slice.

## Background

Sub-slice 1 shipped the origin-polling detection spine; sub-slice 2 shipped DB-backed per-map zones + operator CRUD. The `OnZoneEnter/Leave/Stay(player, zoneName)` API was designed backend-agnostic from the start — this slice exposes it across the inter-plugin boundary using the Slice-4.5 interface model (`publishInterface` / `require` proxy; methods = natives, events = forwards; args + payloads cross by **structured copy**; a hard dep throws `InterfaceUnavailable` on producer-unload, an optional dep resolves to `Interface | null`; all auto-ledgered).

## Architecture

### Producer — `plugins/zones` publishes `@s2script/zones@1.0.0`

At `onLoad` (after the DB opens), `handle = publishInterface("@s2script/zones", "1.0.0", impl)`. **Every method is synchronous and registry-backed** — a Promise can't cross the structured-copy wire, so mutating methods update the in-memory registry immediately and fire-and-forget the DB write (durability is async; the registry is the source of truth for detection + queries):

- `createZone(name, min, max) → boolean` — validate + sanitize; update the registry; fire-and-forget `upsertZone` (the DB write); `true` on success. (Programmatic zone creation, persisted — matching `sm_zone_add`.)
- `deleteZone(name) → boolean` — registry delete + fire-and-forget the DB delete; `true` if it existed.
- `getZones() → { name: string, min: Vec3, max: Vec3 }[]` — the current map's zones (plain data; wire-safe).
- `isInZone(slot, name) → boolean` — is that player currently inside (from the zone's `inside` set).
- `zonesFor(slot) → string[]` — the names of every zone the player is currently in.

`min`/`max` cross as plain `{x,y,z}` objects (wire-safe). The detection loop, instead of logging, **emits** on each poll:
- `handle.emit("enter", { zone, slot, userId })` when a player newly enters a zone.
- `handle.emit("leave", { zone, slot, userId })` when a player leaves.
- `handle.emit("stay", { zone, slot, userId })` for every player currently inside, each poll tick (~8 Hz; consumers throttle if they want).

**Payload is wire-safe plain data** (`{ zone: string, slot: number, userId: number }`) — never a `Player`/`Pawn` (its prototype/methods don't survive structured copy). The consumer resolves the player itself (`Player.fromSlot(slot)` / `fromUserId(userId)`) in its own V8 context. `userId` is included so a consumer can re-resolve safely if the slot churns.

The operator commands (`sm_zone_add`/`delete`/`list`/`export`/`import`) are unchanged. The producer keeps a minimal internal debug log only if useful; the events now flow to consumers.

### Consumer demo — `examples/zones-consumer-demo`

Hard-deps `@s2script/zones` (a producer-backed proxy that throws `InterfaceUnavailable` while the producer is unloaded → calls wrapped in try/catch) with a **hand-written `zones.d.ts`** ambient stub (interface `.d.ts` codegen is deferred; the consumer declares the producer's shape by hand, mirroring the greeter-consumer pattern). At `onLoad`:
- `on("enter", p => …)` / `on("leave", …)` — log the resolved player (`Player.fromSlot(p.slot)?.playerName`) + the zone name (proves events cross the boundary + the consumer resolves the player).
- `on("stay", p => …)` — **if `p.zone === "heal"`, top up the player's health** (`const pw = Player.fromSlot(p.slot)?.pawn; if (pw && pw.health < 100) pw.health = Math.min(100, pw.health + 1)`), throttled. A real, **bots-provable behavior** driven entirely by another plugin's zone events.
- Optionally call `getZones()` / `isInZone(slot, name)` (in try/catch) to show the method path.

## Boundary

Entirely game-layer. The producer (`plugins/zones`) + consumer (`examples/zones-consumer-demo`) both depend on `@s2script/cs2`; the interface itself (`@s2script/zones`) carries only plain data + slots across the wire. No CS2 name reaches core (there is no core touch). Both boundary gates trivially green. No new engine primitive, no sniper.

## Testing

**Typecheck:** the producer + consumer pass full-strict (the consumer's hand-written `zones.d.ts` types the proxy); `check-plugins-typecheck.sh` green.

**Live gate (de_inferno, `bot_quota 4`, rcon) — bots-provable:**
- Boot: `[zones] publishing @s2script/zones@1.0.0` + `[zones-consumer] onLoad — subscribed`; `RestartCount=0`.
- `sm_zone_add heal <coords bounding a bot>` → a bot inside → **the CONSUMER logs `[zones-consumer] ENTER heal: <name>`** (the event crossed the interface to a separate plugin) and **the bot's health climbs toward 100** (the consumer's `stay` heal behavior — verify via a schema read / `sm_zone_list`-adjacent check or a direct health log). `sm_slap` the bot down first to show the heal recovering it.
- `on leave` fires when the bot exits (or the zone is deleted).
- The method path: the consumer logs `getZones()` count / an `isInZone` check.
- Producer unload (rm the zones `.s2sp`) → the consumer's next method call logs the graceful `InterfaceUnavailable` degrade (hard-dep proxy throws, caught) — then re-add → recovers. (Or note this as the 4.5-proven behavior and skip if flaky.)

**Human-client deferral:** none essential — the heal behavior + the consumer's logs are bots-provable.

## Deferred (later)

- Beam visualization (`sm_zone_show` — draw the box edges with `Beam`), the in-game two-step corner-marking editor, zone tags/types/metadata (a `tags` column + `getZonesByTag`).
- `OnZoneStay` throttle config; ephemeral (non-persisted) zones (`createZone(..., { persist: false })`); a `createZone` that returns the created zone's data.
- The interface `.d.ts` codegen (producer-emitted consumer types) — a framework-wide deferral, not zone-specific.
- The real trigger backend (the parked collision-partition op).

## Slice shape

Two plugins (producer edit + a new consumer), game-layer only → **no sniper**, hot-reloadable. Controller-authored; live gate (the consumer reacting to zone events is the proof); merge; push; document.
