# nominations (map nomination + the mapvote foundation) — Design

**Status:** Approved (brainstorm — sub-slice 1 of nominations+rockthevote; maplist.txt/workshop, shared SQLite, cooldown decided), ready for the plan.
**Slice:** the SourceMod `nominations` plugin + the shared map-vote foundation (a SQLite map-history/nomination store + a `maplist.txt` pool + a configurable cooldown) that `rockthevote` (sub-slice 2) builds on. Shipped opt-in (`disabled/`).

## Goal

Let players nominate maps for the next map vote: `sm_nominate <map>` (or a menu) adds a map to a shared nomination list, restricted to a curated `maplist.txt` pool and excluding recently-played maps (a configurable cooldown). The nomination list + play history live in a shared SQLite DB that `rockthevote` reads. Both plugins ship in a `disabled/` folder — operators opt in.

## Motivation & context

Map voting (nominations → rockthevote → the vote) is a staple SourceMod feature set. This sub-slice builds the **shared foundation** — the `maplist.txt` pool, the SQLite `map_history`/`nominations` tables, and the cooldown — plus the `nominations` plugin itself. It reuses `@s2script/db` (SQLite), `@s2script/menu` (the nomination menu), `@s2script/server` (`mapName`/`isMapValid`), and `@s2script/commands`, and adds one small engine-generic capability: a plugin-accessible raw config-file read/write (`config.readFile`/`writeFile`), needed because a plain-text `maplist.txt` can't go through the existing `.json`-suffixed config bridge.

## Scope

**In scope:** the `config.readFile`/`config.writeFile` capability (a shim op pair + `@s2script/config` methods); the `maplist.txt` format + parser (stock + colon-separated workshop entries); the shared `mapvote` SQLite DB (`map_history` + `nominations` tables) + the cooldown query + play-history recording on `onLoad`; the `nominations` plugin (`sm_nominate <map>` + a menu; `map_cooldown` config); shipped in `plugins/disabled/`.

**Deferred (to sub-slice 2 / follow-ons):** `rockthevote` (`rtv`/the turnout threshold/the map vote/the winner change — including the workshop `host_workshop_map` change); a hard cap on total nominations (one-per-player for the MVP; the pool bounds it); `sm_nominate` tab-completion; the end-of-map auto-vote (mapchooser); admin-forced nominations.

## Approach (decided)

- **Shared state via one SQLite DB, no inter-plugin interface.** `Database.open("mapvote")` resolves to the same file for any plugin (separate owner-scoped connections, shared data), so `nominations` writes and `rockthevote` reads the same `map_history`/`nominations` tables. `CREATE TABLE IF NOT EXISTS` on each `onLoad` (idempotent — whichever plugin loads first creates them).
- **`onLoad` = "map started."** Plugins reload on a `changelevel`, so `onLoad` fires at each new map; recording is deduped against the last history row so a same-map dev reload doesn't double-record (and doesn't clear nominations).
- **Opt-in via a `disabled/` subfolder.** The loader's directory scan (`loader.rs` `read_dir`) is top-level `.s2sp`-only and non-recursive, so `plugins/disabled/*.s2sp` is never loaded. No loader change.

## Architecture

One-way deps (game → core). The `config.readFile`/`writeFile` capability is engine-generic; the `nominations` plugin is CS2 (uses `Server`/`Player`).

### New capability — `config.readFile` / `config.writeFile` (engine-generic)

`@s2script/config` gains `readFile(name: string): string | null` and `writeFile(name: string, content: string): void` — raw read/write of `addons/s2script/configs/<sanitized name>` where `name` includes its extension (e.g. `"maplist.txt"`). Backed by a new shim op pair (`config_read_file`/`config_write_file`) that resolves the path like the existing `ConfigPath` **but without the `.json` append** (the same `dladdr` walk to the addon root; the name is sanitized to a safe basename to prevent traversal — reject `/`, `..`). `readFile` returns `null` when the file is absent; `writeFile` creates/overwrites. Exposed on the `config` object beside the typed getters. Degrade-safe (no ops → `null`/no-op).

### `maplist.txt` format + parser

One entry per line; blank lines and lines starting with `//` or `#` are ignored. Each entry is `<name>` (stock) or `<name>:<workshopId>` (workshop — colon-separated, the ID numeric):
```
de_dust2
de_inferno
awp_lego_2:3070284539
```
Parsed into `MapEntry[] = { name: string, workshopId: string | null }` (split each non-comment line on the first `:`). The **name** is the map's identity everywhere (nomination, cooldown, history); the **workshopId** is retained for sub-slice 2's `host_workshop_map <id>` change (stock → `changelevel <name>`). If `maplist.txt` is absent on first run, `nominations` writes a template (a few stock maps + a `//` comment documenting the `name:workshopId` workshop form).

### Shared `mapvote` SQLite DB + cooldown

`Database.open("mapvote")`, `CREATE TABLE IF NOT EXISTS`:
- `map_history(id INTEGER PRIMARY KEY AUTOINCREMENT, map TEXT NOT NULL, played_at INTEGER NOT NULL)` — the play log.
- `nominations(map TEXT PRIMARY KEY, nominator INTEGER NOT NULL)` — the current nomination list.

**Recording (on `onLoad`):** `SELECT map FROM map_history ORDER BY id DESC LIMIT 1`; if it differs from `Server.mapName` (or is empty), it's a new map → `INSERT INTO map_history(map, played_at)` with a JS-supplied unix timestamp, then `DELETE FROM nominations` (fresh nominations for the new map); if it matches, no-op (a reload, keep nominations).

**Cooldown:** a map is in cooldown if it is among the last `map_cooldown` distinct maps: `SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT ?`. The current map is always in that set (just recorded), so it is never nominatable.

### `nominations` plugin (`plugins/disabled/nominations`, CS2)

- **`sm_nominate <map>`** — read + parse `maplist.txt` → the pool; reject if `<map>` isn't in the pool (by name), or is in cooldown, or is already nominated; else **one-nomination-per-player** (`DELETE FROM nominations WHERE nominator = <slot>` then `INSERT`), and `Chat.toAll("[nominations] <player> nominated <map>")`.
- **`sm_nominate`** (no arg) — a **chat menu** (`@s2script/menu`, non-freezing, paginated) of nominatable maps (pool − cooldown − already-nominated) → on pick, nominate that map.
- **Config:** `map_cooldown` (int, default 5 — distinct maps that must play before a map is nominatable again).
- **`onLoad`:** open the DB, create tables, record the play history (above), and register the commands. Async DB calls are fire-and-forget with per-call error logging.

## Testing & gate

- **Core unit tests** (the `config_read_file`/`config_write_file` op + `config.readFile`/`writeFile`, in-isolate like the existing config natives): degrade to `null`/no-op with no ops; a write-then-read round-trip against a stub op; a `..`/`/` name is rejected (no traversal).
- **Live gate (bots-provable):** build `nominations` into `plugins/disabled/`; confirm it does NOT load (no `[nominations] onLoad`). Move the `.s2sp` up into `plugins/`; confirm it loads, auto-generates `maplist.txt`, and records the current map in `map_history`. `sm_nominate de_dust2` (rcon) → the `nominations` row appears (verify via a log / a follow-up query) + the announce; `sm_nominate` with a cooldown'd map (the current map) → rejected; `sm_map de_dust2` → a new `map_history` row + nominations cleared. `RestartCount=0`, no crash.
- **Gates:** core-boundary (the `config.readFile` capability is engine-generic — a file name + bytes), name-leak, typecheck, full `cargo test`. One sniper (the shim op pair + the `@s2script/config` prelude method).

## Boundary & safety summary

`config.readFile`/`writeFile` (a sanitized configs-dir file name → raw bytes) is engine-generic (core prelude + shim op). The `nominations` plugin (`Server.mapName`/`isMapValid`, `Player`, the maplist/cooldown/nomination logic) is CS2/game-layer. The shared DB is owner-scoped per connection (the `@s2script/db` model) and ledgered. `maplist.txt` names are sanitized to a safe basename (no traversal); the workshop ID is treated as opaque text (validated numeric before use in sub-slice 2's change command). Both boundary gates stay green.
