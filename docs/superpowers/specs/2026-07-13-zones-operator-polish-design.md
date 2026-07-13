# Zones operator polish — design

**Date:** 2026-07-13
**Status:** approved
**Slice:** the deferred UX + queryability layer for the shipped `@s2script/zones` platform (follows the 3-sub-slice zone system: detection → persistence → interface).

## Goal

Close the zone system's deferred "operator polish" list with three cohesive, **game-layer-only** additions to `plugins/zones`, plus a coherence fix to the plugin's version declarations:

1. **Beam wireframe visualization** — `sm_zone_show` / `sm_zone_hide` draw a zone's box as edge-beams.
2. **In-game E-to-mark editor** — `sm_zone_edit` lets an admin press **E** at two corners (with a live rubber-band box preview) instead of typing coordinates.
3. **Tags** — a `tags` column + `sm_zone_tag` + `getZonesByTag` on the published interface, so zones are queryable by tag.

Plus: **`created`/`deleted` zone-lifecycle events** on the interface (so other plugins can hook zones as operators make them), and a **versioning fix** (drop the `1.0.x` anti-pattern from the zones manifest).

Everything is `plugins/zones` + `packages/zones` + the consumer demo + a changeset. **No core/shim/gamedata/op/ABI change → no sniper, hot-reloadable, cleanly parallel with any concurrent core slice.** It composes the existing `Beam`/`pawn.origin`/`pawn.buttons`/`TriggerZone` (`@s2script/cs2`), `Vector` (`@s2script/math`), and the `@s2script/zones` interface as-is.

## Background

The current `plugins/zones` (real-trigger backend, merged) publishes `@s2script/zones@1.0.0` with `createZone`/`deleteZone`/`getZones`/`isInZone`/`zonesFor` + emits `enter`/`leave`/`stay`. Zones are per-map rows in a SQLite `zones` table (`Database.open("zones")`, `PRIMARY KEY(map,name)`), loaded on `Server.onMapStart`, detected via engine `trigger_multiple` touch outputs. Operator CRUD: `sm_zone_add`/`delete`/`list`/`export`/`import`. This slice adds the polish that sub-slice 3 explicitly deferred.

## Versioning fix (the coherence correction)

**Finding:** interface/plugin version enforcement is **deferred** (`core/src/loader.rs` — the manifest `version` is "not yet validated"; only `apiVersion` *major* is checked at load), so these version strings are declarative. Meanwhile everything real is `0.x`: `packages/*` are `0.1.1`–`0.3.0` (independent, Changesets), the release tag is `v0.1.2`, and `build-base-plugins.sh` stamps every plugin's `version` to the release tag. `plugins/zones` is the **only** plugin declaring `1.0.x` — `publishes: {"@s2script/zones": "1.0.0"}` and `pluginDependencies: {…: "^1.0.0"}` — while it depends on packages that are actually `0.1.2`/`0.2.0`/`0.3.0`. The consumer demo mirrors the `^1.0.0`.

**Model (restated):** plugins (and their published interfaces) are **version-bound to the release tag**; only `packages/*` get independent versions.

**Fix:**
- `plugins/zones/package.json` `publishes["@s2script/zones"]`: `1.0.0` → **`0.1.0`** (matches the release-bound `version`). `plugins/zones/src/plugin.ts` `publishInterface("@s2script/zones", "0.1.0", …)` to match. **No independent interface bump** — the additive changes ship in the next release; the interface version tracks the release, it is not bumped on its own axis.
- `plugins/zones/package.json` `pluginDependencies`: `^1.0.0` → the real per-package `0.x` carets — `@s2script/admin ^0.2.0`, `@s2script/cs2 ^0.3.0`, the rest (`commands`/`db`/`server`/`config`/`frame`/`interfaces`) `^0.1.0` — plus add `@s2script/math ^0.1.0`.
- `examples/zones-consumer-demo/package.json` `pluginDependencies`: `@s2script/cs2 ^0.3.0`, `@s2script/zones ^0.1.0`.
- **`packages/zones` (types package) stays independent** and gets the Changesets **minor** bump (`0.1.1` → `0.2.0`) for the `.d.ts` additions. That axis *is* supposed to be independent — the changeset is correct.
- **`build-base-plugins.sh` is NOT changed.** The `publishInterface(…, version, …)` arg lives in code (can't be stamped without build-time codegen), so stamping only the manifest would split code vs. manifest. Both are hand-maintained at `0.1.0` in the 0.x scheme. (A future option: teach the stamp script to own `publishes` — out of scope here.)

## Architecture

Single file, `plugins/zones/src/plugin.ts`, organized into clearly-commented sections (matches the existing single-file plugin pattern; the shared state — `zones` registry, `db`, `iface` — is tightly coupled, so one file is the right unit). New helper state and functions are added alongside the existing trigger-lifecycle code.

### Unit 1 — Beam viz (`sm_zone_show` / `sm_zone_hide`)

- **State:** `shown = Map<zoneName, { beams: BeamHandle[]; expiresAt: number }>` (`expiresAt` = wall-clock ms via `Date.now()`; `0` = persistent).
- **`box12(min, max) → { a: Vec3; b: Vec3 }[]`** — a pure helper returning the 12 edges of the AABB (8 corners → 4 bottom + 4 top + 4 vertical). Reused by viz and the editor preview.
- **`showZone(z, seconds)`** — remove any existing beams for `z.name`, then for each of the 12 edges `Beam.draw(new Vector(a.x,a.y,a.z), new Vector(b.x,b.y,b.z), { color, width })`; store the handles + `expiresAt = seconds > 0 ? Date.now() + seconds*1000 : 0`.
- **`hideZone(name)` / `clearAllBeams()`** — `remove()` each beam handle, delete the entry.
- **`sm_zone_show <name|all> [seconds]`** (`ADMFLAG.GENERIC`) — default `seconds = 30`; `0` = persistent. `all` iterates `zones` (bounded operator use — a handful of zones × 12 beams).
- **`sm_zone_hide [name|all]`** — hide one or all.
- **Expiry:** the existing per-frame `OnGameFrame` handler checks each `shown` entry; `expiresAt > 0 && Date.now() >= expiresAt` → `hideZone`.
- **Cleanup:** `clearAllBeams()` runs in `loadMap` (map change) and `onUnload` (beams are game-world-owned, not auto-ledgered).

### Unit 2 — E-to-mark editor (`sm_zone_edit`)

- **State:** `edits = Map<slot, { name: string; cornerA: Vec3 | null; prevMask: number; expiresAt: number; preview: BeamHandle[] }>`.
- **`sm_zone_edit <name>`** (`ADMFLAG.GENERIC`, in-game only — `callerSlot >= 0`) starts a session for the caller: seed `prevMask` with the pawn's **current** `buttons` mask (so holding E on entry doesn't fire — the menu system's proven fix), `cornerA = null`, `expiresAt = Date.now() + 60_000`. Reply: "Walk to a corner and press E; press E again at the opposite corner." `sm_zone_edit` (no arg) or `sm_zone_edit cancel` cancels the caller's session (removes preview beams).
- **A dedicated `OnGameFrame` poll** (runs only while `edits.size > 0`): for each editing slot —
  - Timeout: `Date.now() >= expiresAt` → cancel + reply once + clear preview.
  - Read `Pawn.forSlot(slot)?.buttons`; rising-edge `pressed = mask & ~prevMask`; update `prevMask = mask`.
  - **Live preview:** if `cornerA` is set and the second corner isn't yet marked, recompute the box `normBox(cornerA, currentOrigin)` and **`update()` each of the 12 preview beams** (created once when `cornerA` was set) to the new edges — the wireframe rubber-bands as the operator walks. No per-frame entity create/remove.
  - On `pressed & IN_USE`:
    - `origin = Pawn.forSlot(slot)?.origin`; null → reply "no position, try again" (don't consume).
    - **1st press:** `cornerA = origin`; create the 12 preview beams (start collapsed at A; the per-frame preview then tracks the box). Reply "Corner 1 set."
    - **2nd press:** `box = normBox(cornerA, origin)`; reject zero-volume; `upsertZone(name, box)` (persist + emit `created`); remove preview beams; `showZone` the saved box (timed preview); reply "Zone '<name>' saved."; end the session.
- **Documented caveat:** an E press also fires the game's own `+use` (door/pickup) — inherent to no-detour button polling; acceptable for an editor.

### Unit 3 — Tags (`sm_zone_tag` + interface queryability)

- **DB migration (additive, idempotent):** the fresh `CREATE TABLE IF NOT EXISTS zones (…, tags TEXT, …)` includes the column; for existing per-map DBs a guarded `ALTER TABLE zones ADD COLUMN tags TEXT` is wrapped in try/catch (SQLite throws "duplicate column name" if it exists → ignore). Run once in `onLoad` after the `CREATE TABLE`.
- **Representation:** `tags` stored comma-separated; each tag sanitized `[a-z0-9_-]`, lowercased (consistent matching). In-memory `Zone` gains `tags: string[]`.
- **`loadMap`** selects `tags` and parses (`split(",")`, filter empties). **`upsertZone(name, box, tags?)`** — when `tags` is omitted it **preserves** the existing zone's tags (a coords-only re-save from `sm_zone_add`/editor/`createZone` does not wipe tags); when provided it replaces. Persists tags in the `INSERT OR REPLACE`.
- **`zonesByTag(tag) → Zone[]`** — a shared internal filter (lowercased tag; `z.tags.includes(tag)`), used by both the interface method and the `sm_zone_list <tag>` filter → the query path is **rcon/bots-provable**.
- **`sm_zone_tag <name> [tag...]`** (`ADMFLAG.GENERIC`) — set/replace the zone's tags (empty = clear); persist. `sm_zone_list [tag]` prints tags and optionally filters by tag.
- **Export/import** JSON round-trips tags: `out[name] = { min, max, tags }`; import reads `tags` if present (old files → `[]`, backward-compatible).

### Interface additions — `@s2script/zones` (version stays `0.1.0`)

All additive; consumers ignoring the new surface are unaffected.

- `Zone` gains `tags: string[]`; `getZones()` returns them.
- **`getZonesByTag(tag: string): Zone[]`** — the current map's zones carrying `tag`.
- **`setZoneTags(name: string, tags: string[]): boolean`** — set/replace tags programmatically (registry now + fire-and-forget DB), `true` if the zone exists. (`createZone` stays a 3-arg boolean — tags set via this or `sm_zone_tag`.)
- **`created` / `deleted` events** (the "hook into zones as they're created" ask):
  - `on("created", (z: ZoneCreatedEvent) => void)` — `ZoneCreatedEvent = { zone: string; min: Vec3; max: Vec3; tags: string[] }`. Emitted on `createZone`/`sm_zone_add`/the editor save, and for each zone loaded on a map's DB load.
  - `on("deleted", (z: ZoneDeletedEvent) => void)` — `ZoneDeletedEvent = { zone: string }`. Emitted on `deleteZone`/`sm_zone_delete`, and for each zone cleared on a map change.
  - Payloads are wire-safe plain data (`zone` key consistent with the enter/leave/stay events). Consumers use `getZones()` for the initial snapshot + `created`/`deleted` for deltas (the standard *list + subscribe* pattern; the existing load-order caveat — probe a method before subscribing — still applies).

## Boundary

Entirely game-layer. `plugins/zones` + `examples/zones-consumer-demo` + `packages/zones` (types) + a changeset. The interface carries only plain data + slots across the structured-copy wire. No CS2 name reaches core (there is no core touch). Both boundary gates trivially green. No new engine primitive, no sniper.

## Files

- `plugins/zones/src/plugin.ts` — viz + editor + tags + migration + created/deleted emits; `publishInterface` version → `0.1.0`; add `import { Vector } from "@s2script/math"`.
- `plugins/zones/package.json` — `publishes` → `0.1.0`; `pluginDependencies` → real `0.x` carets + add `@s2script/math`.
- `packages/zones/index.d.ts` — `Zone.tags`; `getZonesByTag`; `setZoneTags`; `created`/`deleted` `on` overloads + `ZoneCreatedEvent`/`ZoneDeletedEvent`.
- `examples/zones-consumer-demo/package.json` — dep carets → `0.x`. (Optionally exercise a tag/created hook in its `.ts` — bots-provable; not required.)
- `.changeset/*.md` — **minor** for `@s2script/zones` (the types package `.d.ts` grew).

## Testing

**Offline (all before any deploy — the live server is shared):**
- `scripts/build-base-plugins.sh` builds `plugins/zones` (typecheck gate = full strict) + `scripts/check-plugins-typecheck.sh` green across all plugins (incl. the consumer demo).
- A DB tags round-trip: create a zone, `setZoneTags`/`sm_zone_tag`, reload → tags parse back; the guarded `ALTER TABLE` is idempotent on an existing (tagless) DB.
- Pure-helper sanity: `box12` returns 12 distinct edges for a known box.

**Live gate (de_inferno/de_dust2, `bot_quota 4`, rcon — bots-provable; coordinate before deploying to the shared server):**
- Boot: `[zones] publishing @s2script/zones@0.1.0`; `RestartCount=0`; base suite loads.
- Tags: `sm_zone_add t <coords>` → `sm_zone_tag t heal vip` → `sm_zone_list heal` shows `t` → **restart → tags persist** (the guarded migration + round-trip) → `sm_zone_list` shows tags.
- Viz: `sm_zone_show t 0` (persistent) and `sm_zone_show all 5` run with no crash; `sm_zone_hide all` clears; the frame-expiry removes the timed set. (Seeing the beams is a deferred human-client visual test.)
- Editor: `sm_zone_edit e` arms a session; the button-poll runs each frame with no crash; `sm_zone_edit cancel` clears. (A real player pressing E at two corners + seeing the rubber-band preview is a deferred human-client test.)
- Created/deleted hooks: `sm_zone_add`/`sm_zone_delete` fire the interface `created`/`deleted` events — provable by extending the consumer demo to log them (or via an internal debug log).

**Human-client deferrals** (per the standing convention — same ceiling as SayText2/menus): visually seeing the beam wireframe; walking corners as a real player and watching the live rubber-band preview.

## Deferred (not this slice)

- Ephemeral (non-persisted) zones (`createZone(…, { persist: false })`); an `OnZoneStay` throttle config knob; a `createZone` that returns the created zone's data.
- Zone types/priorities beyond flat tags; `getZonesByTag` across maps; per-zone colors from tags.
- The interface `.d.ts` codegen (producer-emitted consumer types) — a framework-wide deferral.
- Teaching `build-base-plugins.sh` to stamp `publishes` (the systemic version-binding option).

## Slice shape

One plugin edit + interface `.d.ts` + consumer dep fix + a changeset; game-layer only → **no sniper**, hot-reloadable. Subagent-driven execution; offline build + typecheck + DB round-trip; then (coordinated) live rcon smoke on the shared server; PR with the changeset.
