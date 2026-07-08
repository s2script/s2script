# rockthevote Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the SourceMod `rockthevote` plugin — chat `rtv` + `sm_forcertv` start a map vote at a turnout threshold; the winner changes at the end of the round.

**Architecture:** A single CS2/game-layer plugin in `disabled/rockthevote/` (opt-in). JS-only — no core/shim change, no new engine op, no sniper. Composes `@s2script/votes` (the vote), the shared `mapvote` SQLite DB + `maplist.txt` (from sub-slice 1 `nominations`), `@s2script/chat`/`clients`/`server`/`commands`/`admin`, `@s2script/frame` (per-map reset poll), and `Events.on("round_end")` (apply).

**Tech Stack:** TypeScript (pure ESM), esbuild via `s2script build`, the `@s2script/*` first-party modules (types resolved from `packages/*/index.d.ts`).

## Global Constraints

- **Ships opt-in in `disabled/rockthevote/`** — the loader's top-level non-recursive `.s2sp` scan never loads `plugins/disabled/`. Build output is `disabled/rockthevote/dist/_s2script_rockthevote.s2sp`.
- **Pure ESM** — named imports only (`import { X } from "@s2script/y"`); no `require`, no `import = require`, no `import * as` on the interface proxies. The 5E.1 typecheck gate (`module: ESNext`, full `strict`) must pass.
- **No core/shim/gamedata change, no sniper.** If a task believes it needs one, STOP — it's out of scope.
- **Plugins persist across a changelevel** — do NOT rely on `onLoad` firing per-map; per-map state resets via a `Server.mapName` poll on `OnGameFrame` (the `nominations` pattern).
- **Injection guard:** any `maplist.txt`-derived value used to build a `Server.command` console string is validated `^[A-Za-z0-9_]+$` (map name) / `^[0-9]+$` (workshop id) before use.
- **Bots** (`steamId === "0"`) are skipped everywhere (trigger, player count).
- **Config keys** (`s2script.config`, exact names + defaults): `rtv_threshold` (float 0.6), `rtv_min_players` (int 0), `rtv_map_count` (int 5), `rtv_cooldown` (int 5), `rtv_vote_duration` (int 20), `rtv_show_tally` (bool true).
- **Commit each task** with the session trailer `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`; no backticks in `git commit -m` (use `-F -`).

---

### Task 1: Plugin scaffold + RTV trigger + threshold

**Files:**
- Create: `disabled/rockthevote/package.json` (id `@s2script/rockthevote`, the 6 config keys, `main` → `dist/...`)
- Create: `disabled/rockthevote/tsconfig.json` (extends the root `tsconfig.base.json`, like the other plugins)
- Create: `disabled/rockthevote/src/plugin.ts`

**Interfaces:**
- Consumes: `@s2script/commands` (`Commands.registerAdmin(name, flags, handler)` + `ctx.callerSlot`/`ctx.reply(msg)`), `@s2script/admin` (`ADMFLAG.CHANGEMAP`), `@s2script/chat` (`Chat.onMessage(handler)` where **handler is positional `(slot: number, text: string, teamonly: boolean) => HookResultValue | void`** — see `plugins/basetriggers/src/plugin.ts` for the exact pattern; `Chat.toAll`/`toSlot`), `@s2script/events` (`HookResult` — `.Continue`/`.Handled`), `@s2script/clients` (`Clients.allConnected()` → `Client[]` with `.slot`/`.steamId`/`.isBot`; `Clients.onDisconnect(cb)` → `cb(client)`, only `.slot` guaranteed live), `@s2script/frame` (`OnGameFrame.subscribe(fn)`), `@s2script/server` (`Server.mapName`), `@s2script/config` (`config.getInt`/`getFloat`/`getBool`). Verify each name/shape against `packages/<mod>/index.d.ts` before use.
- Produces: module-level state `rtvVoters: Set<number>`, `voteRunning: boolean`, `votedThisMap: boolean`, `pendingMap: MapEntry | null`, `currentMap: string`; a `startVote(force: boolean): void` function (Task 1 leaves the body a stub that logs `"[rockthevote] threshold reached — starting vote"` and sets `voteRunning = true`; Task 2 fills it); a `playerCount(): number` helper (connected non-bots); a `pollMapChange()` frame handler that resets per-map state on a `Server.mapName` change.

- [ ] **Step 1: Scaffold** — copy the shape of an existing `disabled/nominations/package.json` + `tsconfig.json`. `package.json` declares the 6 config keys under `s2script.config` with the exact types/defaults from Global Constraints. `id`: `@s2script/rockthevote`.

- [ ] **Step 2: Trigger + threshold in `plugin.ts`.** Implement:
  - `Chat.onMessage` handler: trim + lowercase `text`; if it is one of `rtv`/`!rtv`/`rockthevote`/`!rockthevote` and the sender is not a bot → `requestRtv(slot)`. Return `HookResult.Handled` for the `!`-prefixed forms (suppress the broadcast); return `HookResult.Continue` for the bare forms (SM shows them). (Import `HookResult` from `@s2script/events`.)
  - `requestRtv(slot)`: if `voteRunning || votedThisMap` → reply "a vote is already running / already happened this map" and return. Add `slot` to `rtvVoters` (if already present → reply "you already RTV'd (need N)"). Compute `need = Math.ceil(config.getFloat("rtv_threshold") * playerCount())`; if `playerCount() < config.getInt("rtv_min_players")` → reply "not enough players". If `rtvVoters.size >= need` → `startVote(false)`; else `Chat.toAll` "Player wants RTV (N more needed)".
  - `Commands.registerAdmin("sm_forcertv", ADMFLAG.CHANGEMAP, ctx => { startVote(true); ctx.reply("RTV forced."); })`.
  - `Clients.onDisconnect(c => rtvVoters.delete(c.slot))`.
  - `playerCount()`: `Clients.allConnected().filter(c => !c.isBot).length`.
  - `pollMapChange()`: throttle a frame counter ~once/sec; on `Server.mapName !== currentMap` set `currentMap = m` and reset `rtvVoters.clear()`, `voteRunning = false`, `votedThisMap = false`, `pendingMap = null`.
  - `startVote(force)`: **stub** for Task 1 — `console.log("[rockthevote] startVote force=" + force)` + `voteRunning = true`.
  - `onLoad()`: subscribe `pollMapChange` to `OnGameFrame`, register the command + the chat/disconnect subs, `console.log("[rockthevote] onLoad — sm_forcertv + rtv registered")`. `onUnload()`: a log line.

- [ ] **Step 3: Build + typecheck.** Run `node packages/cli/dist/cli.js build disabled/rockthevote` — the 5E.1 gate must pass (prints the `.s2sp` path). Fix any type errors against the real `.d.ts`.

- [ ] **Step 4: Commit.**
```bash
git add disabled/rockthevote
git commit -F - <<'EOF'
feat(rockthevote): scaffold + chat rtv / sm_forcertv trigger + turnout threshold (disabled/)
...
Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 2: The ballot, the vote, and applying the winner at round_end

**Files:**
- Modify: `disabled/rockthevote/src/plugin.ts`

**Interfaces:**
- Consumes (from Task 1): the module state (`rtvVoters`/`voteRunning`/`votedThisMap`/`pendingMap`/`currentMap`) + `playerCount()`; replaces the `startVote` stub body. Adds `@s2script/db` (`Database.open("mapvote")` → `db.query(sql, params)` returning `Record<string, SqlValue>[]`), `@s2script/votes` (`Vote.start(config): boolean` — returns false if a vote is already active; `Vote.isActive()` is a **method**; `config.onEnd: (result: VoteResult) => void` where **`VoteResult = { winner: number | null; counts: number[]; total: number }` and `winner` is an INDEX into `options` (or null on a tie/no-votes)** — NOT the display string), `@s2script/events` (`Events.on("round_end", handler)` → `handler(ev)`), `config.getBool`. A local `MapEntry = { name: string; workshopId: string | null }` + a duplicated `parseMaplist` (colon split, `//`/`#`/blank skip, skip empty-name) + `loadPool()` (via `config.readFile("maplist.txt")`; do NOT auto-generate — nominations owns that; if absent, treat as empty pool + log).
- Produces: the full RTV flow. No new exports.

- [ ] **Step 1: DB + maplist helpers.** In `onLoad`, `Database.open("mapvote").then(d => { db = d; })` (no CREATE TABLE — nominations owns the schema; guard reads with `if (!db)`). Add `parseMaplist`/`loadPool` (duplicated from nominations, `config.readFile` only — no write). Add `cooldownSet()`: `SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT ?` with `Math.max(0, config.getInt("rtv_cooldown"))`. Add `nominationList()`: `SELECT map FROM nominations ORDER BY rowid` → `string[]`.

- [ ] **Step 2: `buildBallot()` (async → `{ options: string[]; entries: Map<string, MapEntry> } | null`).** Let `cap = Math.min(Math.max(1, config.getInt("rtv_map_count")), 8)` — the ballot is `2..9` options (`VoteConfig`) and "Don't Change" takes one, so **at most 8 map options**. options = the nominations (in order, truncated to `cap`), then random pool-fill: `pool = loadPool()`; exclude `cooldownSet()` + the already-listed nominations; shuffle (`Math.random`); take until `options.length === cap`. Build an `entries` map from display-name → `MapEntry` (for workshopId lookup; nominations not in the pool map to `{ name, workshopId: null }`). Append the literal `"Don't Change"` (not in `entries`). If the map-option count (excluding "Don't Change") is 0 → return null (caller aborts — a 1-option "Don't Change"-only vote is pointless).

- [ ] **Step 3: Fill `startVote(force)`.** Guard `if (voteRunning || Vote.isActive()) return;` (`isActive` is a **method**). Run `buildBallot()`; if null → `Chat.toAll("No maps available to vote on")`, reset RTV (`voteRunning=false; votedThisMap=true`), return. Set `voteRunning = true`, `rtvVoters.clear()`. `Vote.start({ question: "RockTheVote", options, duration: config.getInt("rtv_vote_duration"), showLiveTally: config.getBool("rtv_show_tally"), onEnd: (result) => finishVote(result, options, entries) })`. `startVote` stays `void` — do the `buildBallot().then(...).catch(logErr)` internally (it awaits the DB).

- [ ] **Step 4: `finishVote(result, options, entries)`.** `voteRunning = false; votedThisMap = true;`. Map the index to the display string: `const chosen = result.winner === null ? null : options[result.winner];`. If `chosen === null || chosen === "Don't Change"` → `Chat.toAll(chosen === null ? "Vote tied — map stays" : "Don't Change won — map stays")`; return. Else `const entry = entries.get(chosen) ?? { name: chosen, workshopId: null };` validate `entry.name` matches `^[A-Za-z0-9_]+$` and, if present, `entry.workshopId` matches `^[0-9]+$` — on a validation miss, log + `Chat.toAll("winner invalid — map unchanged")` + return. Set `pendingMap = entry`; `Chat.toAll(chosen + " won — changing at the end of the round")`.

- [ ] **Step 5: `Events.on("round_end", ...)` apply.** If `pendingMap` → `Server.command(pendingMap.workshopId ? "host_workshop_map " + pendingMap.workshopId : "changelevel " + pendingMap.name)`; `pendingMap = null`. (Guard: only when set; the `pollMapChange` reset also clears a stale `pendingMap` if the map changed by other means.)

- [ ] **Step 6: Build + typecheck + commit.** `node packages/cli/dist/cli.js build disabled/rockthevote` (gate passes) + `bash scripts/check-plugins-typecheck.sh`. Commit `feat(rockthevote): ballot + @s2script/votes vote + round_end map change (workshop/stock)`.

## Self-Review

- **Spec coverage:** trigger (Task 1) · threshold (Task 1) · `sm_forcertv` (Task 1) · disconnect drop (Task 1) · per-map reset (Task 1) · ballot nominations+pool−cooldown+DontChange (Task 2) · the vote (Task 2) · Don't-Change/tie → stay (Task 2) · winner → pending → round_end change workshop/stock (Task 2) · zero-options abort (Task 2) · injection guard (Task 2). All covered.
- **Type consistency:** `MapEntry`, `startVote(force)`, `finishVote(winner, entries)`, `buildBallot`, `pollMapChange`, the state names are used identically across tasks.
- **No placeholders:** the Task-1 `startVote` stub is an explicit, intentional deliverable boundary (logs + sets `voteRunning`), replaced in Task 2 Step 3.
