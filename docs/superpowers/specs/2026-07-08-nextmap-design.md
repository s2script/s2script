# nextmap (map rotation + auto-change) — Design

**Status:** Approved (brainstorm — full auto-rotation decided; `nextlevel` cvar as the coordination point), ready for the plan.
**Slice:** the SourceMod `nextmap` plugin — a map rotation from `maplist.txt` with `sm_setnextmap` override, the `nextmap` chat trigger (via the existing basetriggers `nextlevel` read), and an automatic changelevel at map end (round/time limit). Ships opt-in (`disabled/`).

## Goal

Run a map rotation: at the end of each map (its `mp_maxrounds` round limit or `mp_timelimit` time limit), change to the next map — the rotation's next map in `maplist.txt` order, or an admin-forced `sm_setnextmap` override. Expose the upcoming map via the CS2 `nextlevel` cvar (which basetriggers' `nextmap` trigger already reads). Reuses `@s2script/server` (`getCvar`/`setCvar`/`command`/`gameTime`), `@s2script/events` (`round_end`), `@s2script/timers` (`delay`), and `maplist.txt` (shared with nominations/rockthevote). No new engine primitive — JS-only, no sniper.

## Scope

**In scope:** the `nextmap` plugin (`disabled/nextmap/`, CS2) — the rotation from `maplist.txt`; `sm_setnextmap <map>` (admin override, `ADMFLAG.CHANGEMAP`); setting the `nextlevel` cvar (so the basetriggers `nextmap` trigger shows the upcoming map — no basetriggers change); map-end detection via `mp_maxrounds` (round_end count) + `mp_timelimit` (elapsed `gameTime`); the delayed `changelevel`/`host_workshop_map` to the next map; config.

**Deferred (do NOT build):** `sm_maphistory`; `mp_winlimit`/`mp_fraglimit`/overtime handling in the end-detection (rounds + time only); a dedicated `mapcyclefile` distinct from `maplist.txt`; wiring rockthevote to *set* `nextlevel` instead of changing directly (they stay independent opt-in plugins — see Coordination).

## Approach (decided)

- **`nextlevel` cvar is the coordination + display point.** CS2 has a native `nextlevel` cvar; basetriggers' `nextmap` trigger already reads it (`Server.getCvar("nextlevel")`). nextmap **sets** `nextlevel` to the computed next map (on each map start) and to the override (on `sm_setnextmap`), so the trigger works with no basetriggers change.
- **Authoritative changelevel (don't trust the game's own rotation).** nextmap detects map end itself (round count vs `mp_maxrounds`, elapsed `gameTime` vs `mp_timelimit`·60) and issues the `changelevel` after a short delay (`nextmap_change_delay`, for the end-of-round screen). A `changing` guard ensures exactly one changelevel per map.
- **Rotation from `maplist.txt`, in file order.** The next map = the entry after the current map's name (wrapping); if the current map isn't in the list, start at the first entry. Reuses `config.readFile` + a duplicated `parseMaplist` (like rockthevote — two opt-in plugins, YAGNI over a shared module).
- **Plugins persist across a changelevel** ([[plugin-lifecycle-map-changes]]) — the round counter / override / `changing` flag reset on a `Server.mapName` change detected by an `OnGameFrame` poll (the nominations/rockthevote pattern), not an `onLoad`-per-map assumption.

## Architecture

Entirely CS2/game-layer (`disabled/nextmap/`). One file.

### State (module-level, persists across a changelevel)

`override: MapEntry | null` (sm_setnextmap), `roundsPlayed: number`, `currentMap: string`, `changing: boolean`, `frameCounter: number`.

### Rotation

`parseMaplist(text)` → `MapEntry[]` (name / `name:workshopId`, `//`/`#`/blank skip, empty-name skip — same as rockthevote). `rotationNext(map): MapEntry | null` = parse `maplist.txt`; find the index of the entry whose `name === map`; return `list[(i+1) % list.length]`; if not found → `list[0]`; if the list is empty → `null`.

### `sm_setnextmap <map>` (`ADMFLAG.CHANGEMAP`)

`m = ctx.arg(0)`; reject empty (usage). Prefer a `maplist.txt` entry whose `name === m` (keeps its workshopId); else if `Server.isMapValid(m)` → `{ name: m, workshopId: null }`; else `ctx.reply("'" + m + "' is not a valid map")` + return. Validate the name `^[A-Za-z0-9_]+$` (injection guard). Set `override = entry`, `Server.setCvar("nextlevel", entry.name)`, `ctx.reply("Next map set to " + entry.name)`.

### The frame poll (`pollTick`, ~once/sec)

- **Map changed** (`Server.mapName !== currentMap`): set `currentMap = m`, reset `roundsPlayed = 0`, `changing = false`, `override = null`; compute `next = rotationNext(m)`; if `next` → `Server.setCvar("nextlevel", next.name)` (seed the trigger's value for the new map).
- **Same map:** if `!changing` and `nextmap_use_timelimit` and `mp_timelimit > 0` and `gameTime >= mp_timelimit·60` → `changeToNext()`.

`gameTime` is `curtime` (seconds since the map loaded — resets each changelevel), so `gameTime >= mp_timelimit·60` is the time-limit condition.

### `round_end` handler

`Events.on("round_end", () => { if (changing) return; roundsPlayed++; const max = parseInt from Server.getCvar("mp_maxrounds"); if (max > 0 && roundsPlayed >= max) changeToNext(); })`.

### `changeToNext()`

If `changing` return; `changing = true`. `const next = override ?? rotationNext(currentMap)`; if `!next` → log "no next map" + return. Validate `next.name` `^[A-Za-z0-9_]+$` and, if present, `next.workshopId` `^[0-9]+$` — on a miss, log + return (no change). Announce `Chat.toAll("[nextmap] Changing to " + next.name + " in " + delay + "s")`; `delay(config.getInt("nextmap_change_delay") * 1000).then(() => Server.command(next.workshopId ? "host_workshop_map " + next.workshopId : "changelevel " + next.name)).catch(...)`.

### Coordination (documented, not built)

nextmap and rockthevote are independent opt-in plugins. rockthevote changes immediately at `round_end` when a vote passes (its own changelevel); nextmap changes at the map's round/time limit. If both are enabled, whichever fires first wins and the other's poll resets on the new map — acceptable for opt-in MVPs. Making rockthevote *set* `nextlevel` for nextmap to apply is a deferred unification.

### Config

- `nextmap_change_delay` (int, default 5) — seconds after the triggering round_end/limit before the changelevel (the end-of-round screen).
- `nextmap_use_timelimit` (bool, default true) — also end on `mp_timelimit` (not just `mp_maxrounds`).

## Testing & gate

- **Live gate (fully bots-provable — the round-count path needs no human):** deploy to `disabled/` → doesn't load; enable → loads. `sm_setnextmap de_dust2` (rcon) → `Server.getCvar("nextlevel")` reads `de_dust2` + "Next map set to de_dust2". `sm_setnextmap notarealmap` → rejected. **Auto-change:** `mp_maxrounds 2` (rcon) → let the bots play 2 rounds → at the 2nd `round_end` nextmap announces + (after the delay) changelevels to the next map; verify the map actually changed + `nextlevel` was honored. Also verify the rotation next (no override) is computed from `maplist.txt` order. `RestartCount=0`, no crash. (The `nextmap` chat trigger showing the value is a human test — but `getCvar("nextlevel")` via rcon proves the value is set.)
- **Gates:** core-boundary, name-leak, `scripts/check-plugins-typecheck.sh`, full `cargo test` (unchanged). No sniper.

## Boundary & safety summary

Entirely CS2/game-layer (`disabled/nextmap/`) — `Server.*`, `Events.on("round_end")`, `maplist.txt`, `@s2script/timers`. No core/shim change, no op, no sniper. The changelevel/setnextmap map name is validated `^[A-Za-z0-9_]+$` / the workshop id `^[0-9]+$` before it reaches `Server.command`/`setCvar` (the console splits on `;`). Per-map state resets on the `Server.mapName` poll; the `changing` guard prevents a double changelevel. Both boundary gates stay green.
