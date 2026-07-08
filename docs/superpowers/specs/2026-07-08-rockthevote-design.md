# rockthevote (RTV map vote) — Design

**Status:** Approved (brainstorm — sub-slice 2 of nominations+rockthevote; change-at-round-end + "Don't Change" + `sm_forcertv` decided), ready for the plan.
**Slice:** the SourceMod `rockthevote` plugin — players `rtv` to trigger a map vote at a turnout threshold; the winner changes at the end of the round. Builds on sub-slice 1 (`nominations`): reads the shared `mapvote` SQLite DB + `maplist.txt` pool. Ships opt-in (`disabled/`).

## Goal

Let players vote to change the map: typing `rtv` in chat accumulates toward a configurable turnout threshold; once reached (or an admin runs `sm_forcertv`), a map vote starts over the current nominations plus random pool maps plus a "Don't Change" option. The winning map is applied at the end of the current round. Reuses `@s2script/votes` (the vote), the shared `mapvote` DB (`nominations` + `map_history`/cooldown from sub-slice 1), and `Server.command` (the change). No new engine primitive — JS-only, no sniper.

## Motivation & context

RockTheVote is the canonical SourceMod map-vote trigger, paired with `nominations` (sub-slice 1 built the shared foundation: the `maplist.txt` pool, the `mapvote` SQLite `nominations`/`map_history` tables, and the cooldown). This sub-slice adds the player-facing RTV flow on top, exercising `@s2script/votes` (built in the basevotes slice) as its first real consumer beyond basevotes itself, and proving the two opt-in plugins share state through one SQLite file with no inter-plugin interface.

## Scope

**In scope:** the `rockthevote` plugin (`disabled/rockthevote/`, CS2) — the chat `rtv` trigger + `sm_forcertv` admin force + the turnout threshold; the ballot (nominations + random pool-fill − cooldown + "Don't Change"); the `@s2script/votes` vote with an optional live tally; applying the winner at `round_end` (workshop `host_workshop_map <id>` vs stock `changelevel <name>`); per-map RTV state reset; config keys.

**Deferred (to follow-ons):** `sm_voteban`/`sm_votemap` (basevotes-family); a mapchooser end-of-map auto-vote; RTV cooldown between votes / a per-vote extend limit; a cross-plugin global vote lock (per-context for now); the deferred framework `OnMapStart` event (rockthevote reuses the nominations `Server.mapName` poll pattern for per-map reset).

## Approach (decided)

- **Player trigger is chat-only; admin force is `sm_forcertv`.** SourceMod has no `sm_rtv` command — players type `rtv`/`!rtv`/`rockthevote` in chat (matched via `Chat.onMessage`), and `sm_forcertv` (`ADMFLAG.CHANGEMAP`) is the admin force-start. This matches SM exactly.
- **Change at end of round (SM default).** The winning map is stashed as `pendingMap` and applied on the next `round_end` game event — the current round finishes first (least disruptive).
- **Include "Don't Change" (SM default).** A non-map ballot entry; if it wins (or a tie / no votes), the map stays and RTV resets so players can rtv again.
- **Shared state, no inter-plugin interface.** `Database.open("mapvote")` reaches the same file `nominations` wrote (sub-slice 1); rockthevote reads the `nominations` table + re-reads/parses `maplist.txt` itself (`config.readFile` + a small duplicated parser — two separate opt-in plugins, YAGNI over a shared module).
- **Opt-in via `disabled/`.** Same as `nominations` — the loader's top-level non-recursive `.s2sp` scan never loads `plugins/disabled/`.
- **Plugins persist across a changelevel** ([[plugin-lifecycle-map-changes]]) — RTV per-map state (the voter set, `pendingMap`, the "vote already ran this map" flag) is reset by polling `Server.mapName` on `OnGameFrame` (the nominations pattern), not by an `onLoad`-per-map assumption.

## Architecture

One-way deps (game → core). Entirely CS2/game-layer (`disabled/rockthevote/`); no core or shim change.

### Trigger + threshold

- **`Chat.onMessage`** matches a bare `rtv`, `!rtv`, `rockthevote`, or `!rockthevote` (case-insensitive, trimmed; bots skipped by steamId `"0"`). Each requesting player is added to an `rtvVoters` set keyed by slot. After adding, if `rtvVoters.size >= Math.ceil(rtv_threshold * playerCount)` **and** `playerCount >= rtv_min_players`, start the vote. A player already in the set gets a "you already voted (N needed)" reply; a below-threshold add broadcasts "Player wants to RTV (need N more)". The command-prefixed forms (`!rtv`) are suppressed (`HookResult >= Handled`); bare `rtv` is a normal chat line that also triggers (SM shows it).
- **`sm_forcertv`** (`registerAdmin(ADMFLAG.CHANGEMAP)`) — force-start immediately, ignoring the count. Reply "RTV forced."
- **`Clients.onDisconnect`** removes the slot from `rtvVoters` (the threshold is a fraction of the current players, so a leaver must not strand it).
- **`playerCount`** = `Clients.allConnected()` minus bots (steamId `"0"`).
- A per-context guard `voteRunning` (and a per-map `votedThisMap`) prevents concurrent RTV votes and a re-trigger after a vote already ran this map.

### The ballot

Build the option list (display names):
1. The current `nominations` (from the shared DB, registration order): `SELECT map FROM nominations ORDER BY rowid`.
2. Random pool-fill: parse `maplist.txt` (`config.readFile` + the duplicated `parseMaplist`), then exclude (a) the **cooldown set** — the last `rtv_cooldown` distinct maps, `SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT rtv_cooldown` (rockthevote's own config, independent of nominations' `map_cooldown`) — and (b) the already-listed nominations; shuffle the remainder (`Math.random`) and take enough to reach `rtv_map_count` total map options.
3. Append **"Don't Change"** as the final option.

If step 1+2 yield **zero** map options (all in cooldown / empty pool), abort: announce "No maps available to vote on" and reset RTV (do not start a one-option vote).

Randomness note: workflow scripts forbid `Math.random` only in the orchestration layer; the plugin runtime is normal V8 — `Math.random()` is available for the shuffle.

### The vote + applying the winner

- **`Vote.start({ question: "RockTheVote", options, duration: rtv_vote_duration, showLiveTally: rtv_show_tally, onEnd })`** — `@s2script/votes`, the basevotes-slice module. Set `voteRunning = true` on start; clear `rtvVoters`.
- **`onEnd(winner)`** (winner = the display string, or null on tie/no-votes):
  - `winner === null || winner === "Don't Change"` → announce "Map stays" / "Vote tied — map stays"; reset RTV (`voteRunning = false`, `votedThisMap = true`), no change.
  - else → find the `MapEntry` for `winner` in the parsed `maplist.txt` (to recover `workshopId`); stash `pendingMap = entry`; announce "`<map>` won — changing at the end of the round"; set `votedThisMap = true`, `voteRunning = false`.
- **`Events.on("round_end", ...)`** — if `pendingMap` is set, apply once: `Server.command(pendingMap.workshopId ? "host_workshop_map " + pendingMap.workshopId : "changelevel " + pendingMap.name)`, then clear `pendingMap`. The subsequent changelevel is recorded by *nominations'* `Server.mapName` poll into `map_history` (shared DB → cooldown advances). `pendingMap.name`/`workshopId` are from the sanitized `maplist.txt`, but as defense the name is validated `^[A-Za-z0-9_]+$` and the id `^[0-9]+$` before building the command (we build a console string).

### Per-map reset

`pollMapChange` on `OnGameFrame` (throttled ~once/sec, the nominations pattern) compares `Server.mapName` to `currentMap`; on a change it resets `rtvVoters.clear()`, `pendingMap = null`, `votedThisMap = false`, `voteRunning = false` (a fresh map re-enables RTV). This also covers the case where `pendingMap` never fired (e.g. a manual `changelevel`) — state is cleaned on the new map.

### Config (`s2script.config`)

- `rtv_threshold` (float, default 0.6) — fraction of in-game players required.
- `rtv_min_players` (int, default 0) — minimum players before RTV is allowed.
- `rtv_map_count` (int, default 5) — number of map options on the ballot (before "Don't Change").
- `rtv_cooldown` (int, default 5) — distinct recently-played maps excluded from the ballot.
- `rtv_vote_duration` (int, default 20) — vote length in seconds.
- `rtv_show_tally` (bool, default true) — show the live center-HTML tally.

## Testing & gate

- **No core unit tests** (JS-only; the pure bits — threshold math, ballot build, `parseMaplist`, the change-command builder — are exercised by the plugin; the framework modules it composes are already core-tested).
- **Live gate (bots-provable):** deploy `rockthevote` to `plugins/disabled/` → confirm it does NOT load. Move up → confirm `[rockthevote] onLoad`. `sm_forcertv` (rcon, console = root) → a vote starts over nominations+pool+"Don't Change" (verify the ballot in chat / logs). Drive the vote to a winner (seed nominations via the shared DB if needed; the vote tally is bots-observable via the center tally / logs), then trigger a `round_end` → the map changes (test both a **stock** winner → `changelevel` and a **workshop** entry → `host_workshop_map <id>`), and `map_history` records the new map. `sm_forcertv` twice in a row → the second is refused while a vote runs / already voted this map. `RestartCount=0`, no crash.
- **Deferred (human-client):** the chat `rtv` threshold accumulation + a real player picking a ballot option (bots can't type chat / vote) — mechanism-proven (the threshold + onEnd paths are plain logic; the vote module is basevotes-proven).
- **Gates:** core-boundary (rockthevote is game-layer — no core touch), name-leak, `scripts/check-plugins-typecheck.sh`, full `cargo test` (unchanged 223). No sniper (JS-only).

## Boundary & safety summary

Entirely CS2/game-layer (`disabled/rockthevote/`) — `Server.mapName`/`Server.command`, `Player`/`Clients`, `Events.on("round_end")`, the shared `mapvote` DB, `maplist.txt` parsing. No core/shim change, no new op, no sniper. The change command is built from `maplist.txt` entries validated `^[A-Za-z0-9_]+$` (name) / `^[0-9]+$` (workshop id) before use (injection guard — we build a console string, and the console splits on `;`). The shared DB is owner-scoped per connection (the `@s2script/db` model) and ledgered. RTV state is per-context and reset per map. Both boundary gates stay green.
