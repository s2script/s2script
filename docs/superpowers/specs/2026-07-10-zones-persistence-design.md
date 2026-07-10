# Zones sub-slice 2 — persistence + per-map lifecycle + operator CRUD — design

**Date:** 2026-07-10
**Status:** approved (sub-slice 2 of the 3-slice zone system)

## Goal

Turn the sub-slice-1 hardcoded test box into **real, named, persisted, per-map zones** that an operator/mapper creates and manages. Zones live in a SQLite DB (source of truth) with JSON export/import, load automatically per map on `Server.onMapStart`, and drive the sub-slice-1 origin-polling detection (`OnZoneEnter`/`OnZoneLeave <name>`). The plugin-developer interface that exposes those events to other plugins is **sub-slice 3** — this slice logs them (the console output is the live-gate proof).

## Architecture

**A first-party plugin `plugins/zones` (`@s2script/zones`)** — the zone manager. All game-layer: `@s2script/db` (persistence), `@s2script/server` (`onMapStart`/`mapName`), `@s2script/config` (JSON files), `@s2script/commands`+`@s2script/admin` (operator CRUD), `@s2script/cs2` (`Player`/`Pawn`/`origin`), `@s2script/frame` (the poll). **No core/shim/engine change — no sniper, hot-reloadable.** The sub-slice-1 `examples/zones-spike` is retired (its polling logic moves here; the spike's value lives in its spec + the memory note).

### Data model

`Zone = { name: string, min: {x,y,z}, max: {x,y,z} }`, scoped to a map. In-memory registry `Map<name, { min, max, inside: Set<number> }>` (per-zone `inside` = slots currently in the zone) drives the poll; the DB is the source of truth. Names are unique per map (case-sensitive; `sm_zone_add` on an existing name updates it — upsert).

### Persistence — DB primary + JSON import/export

- **DB** (`Database.open("zones")`, the SQLite primitive): `CREATE TABLE IF NOT EXISTS zones (map TEXT, name TEXT, minX REAL, minY REAL, minZ REAL, maxX REAL, maxY REAL, maxZ REAL, PRIMARY KEY (map, name))`. `sm_zone_add` = `INSERT OR REPLACE`; `sm_zone_delete` = `DELETE … WHERE map=? AND name=?`. Every mutation updates the DB **and** the in-memory registry (the registry is the hot path; the DB is durability).
- **JSON** (`config.writeFile`/`readFile`, plain-text sibling used by nominations): `sm_zone_export` writes `config.writeFile("zones-" + sanitize(map), JSON.stringify({ "<name>": { min: [x,y,z], max: [x,y,z] }, … }, null, 2))`; `sm_zone_import` reads it, validates, and upserts each into the DB + registry. Human-editable, git-diffable, shippable alongside a map. (`sanitize` = the `[A-Za-z0-9._-]` filename guard `config.writeFile` already applies; the map name is clean but guard anyway.)

### Per-map lifecycle (first real consumer of `Server.onMapStart`)

- On `onLoad`: `await Database.open("zones")`, `CREATE TABLE`, then `loadMap(Server.mapName)` (boot-loaded plugins may miss the very first `onMapStart` fire — the documented sub-slice-1 caveat — so load the current map explicitly at `onLoad`).
- On `Server.onMapStart(map)`: `loadMap(map)`.
- `loadMap(map)`: clear the registry, `SELECT … WHERE map=?`, rebuild the registry (fresh `inside` sets). The DB is published only after `CREATE TABLE` resolves; a `loadMap` before the DB is ready no-ops (guard).
- Zones persist across a `changelevel` (re-loaded for the new map) and across a full restart (the DB is on disk).

### Detection loop (sub-slice-1 poll, generalized to N zones)

Each `OnGameFrame` (throttled ~8 Hz): for every `Player.all()` with a live `pawn.origin`, AABB-test against **every** registered zone; per zone, diff its `inside` set → `OnZoneEnter <name>` / `OnZoneLeave <name>` (logged with the player name + slot). Cost is trivial (players × zones × 6 comparisons). `OnZoneStay` (per-tick while inside) is the API but unlogged here.

### Operator commands (`registerAdmin(ADMFLAG.GENERIC)`; rcon/server-console = root → bots-provable)

- `sm_zone_add <name> <x1 y1 z1 x2 y2 z2>` — explicit world coords (mapper/rcon-friendly; corners in any order, normalized to min/max). **Or** `sm_zone_add <name> [size]` from an in-game caller — a ±`size` (default 128) box around the caller's `pawn.origin` (the server console with no coords → usage error). Upserts.
- `sm_zone_delete <name>` — remove from the DB + registry.
- `sm_zone_list` — the current map's zones (name + bounds + current occupant count).
- `sm_zone_export` — write the current map's zones to the JSON file; reply the path/count.
- `sm_zone_import` — read the JSON file, upsert each zone; reply the count (or "no file").

Validation: `name` non-empty + sanitized (`[A-Za-z0-9_-]`, so it's a clean DB/JSON key); coords finite; a degenerate (zero-volume) box is rejected with a usage message.

## Boundary

Entirely game-layer (the `plugins/zones` plugin). No CS2 names leak to core (there are none — it uses `@s2script/cs2`'s `Player`/`pawn.origin`). Both boundary gates trivially green (no core/shim touch). No new engine primitive.

## Testing

**In-isolate:** none (no core change).

**Typecheck:** `plugins/zones` passes full-strict; `check-plugins-typecheck.sh` green.

**Live gate (de_inferno, `bot_quota 4`, rcon) — bots-provable:**
- Boot: `[zones] onLoad — DB ready, N zones for <map>`; `GAMEDATA` unchanged (no core change); `RestartCount=0`.
- `sm_zone_add zoneA <coords around bot 0>` → `sm_zone_list` shows `zoneA`; a bot inside/entering → `[zones] ENTER zoneA: <name> (slot N)`; moving bots crossing out → `LEAVE zoneA`.
- **Persistence:** restart the container → boot log shows `N zones for <map>` (loaded from the DB) and the bot re-fires `ENTER zoneA` → the zone survived restart with no re-add. (Needs the `data/` RW mount, like clientprefs/db-demo.)
- `sm_zone_export` → the `zones-<map>` JSON file exists with `zoneA`'s bounds; `sm_zone_delete zoneA` + `sm_zone_import` → `zoneA` restored.
- `changelevel de_dust2` → `[zones] onMapStart de_dust2` + `0 zones for de_dust2` (per-map isolation); back to the original map → `zoneA` reloads.

**Human-client deferral:** the in-game `sm_zone_add <name>` (box-around-me) from a real player's position (bots can't type in-game; rcon uses explicit coords) — the explicit-coords path proves the mechanism.

## Deferred (sub-slice 3 / later)

- `publishInterface("@s2script/zones", { onEnter, onLeave, onStay, createZone, deleteZone, getZones, isInZone })` — the plugin-developer consumer API + a consumer demo — **sub-slice 3**.
- `OnZoneStay` delivery; zone tags/types/metadata; beam visualization (`sm_zone_show`); the in-game two-step corner-marking editor.
- Non-box shapes; the real trigger backend (the parked collision-partition op).
- Per-zone enable/disable; zone groups; a global cross-plugin zone registry.

## Slice shape

One plugin, no engine change → **no sniper**, JS-only, hot-reloadable. Built controller-authored or via a small workflow (it's a single cohesive plugin); live gate; merge; push; document. Deploy needs the `data/` RW mount (the DB), same as clientprefs.
