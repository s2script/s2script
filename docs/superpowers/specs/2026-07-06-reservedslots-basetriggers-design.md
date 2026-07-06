# reservedslots + basetriggers — design + plan

**Goal:** Ship two more SourceMod base plugins — `@s2script/reservedslots` and `@s2script/basetriggers` — plus the small server-info op batch both need.

**Feasibility outcome:** Neither is zero-op, but both are unblocked by 3 trivial typed-method passthroughs on the already-held `INetworkGameServer*` pointer (the same object the 5D.2 client-list code dereferences; vtable indices ModSharp-cross-validated against `iserver.h`). Everything else reuses existing modules.

## Architecture

**One op batch** (shim + core + `@s2script/server`), ABI-appended after `client_address`:
- `server_max_clients() -> int` → `((INetworkGameServer*)gameServer)->GetMaxClients()` (`iserver.h:75`, idx 12) → `Server.maxPlayers`.
- `server_map_name() -> const char*` → `GetMapName()` (`iserver.h:97`, idx 25) → `Server.mapName` (`""` if unavailable; `static std::string` pattern like `s_addressBuf`).
- `server_game_time() -> float` → `GetGlobals()->curtime` (`iserver.h:65` idx 7 → `globalvars_base.h:71`) → `Server.gameTime` (map time in seconds; `0` if unavailable).

All degrade to `0`/`""` when `gameServer`/`GetGlobals` is null. Engine-generic (int/string/float out; no CS2 type crosses to core).

**`@s2script/reservedslots`** (CS2 plugin): keeps N slots free for `ADMFLAG.RESERVATION` admins by kicking a non-reserved newcomer.
- Config (`@s2script/config`): `reserved_slots` (int, default `0` = disabled).
- `Clients.onActive((c) => { ... })` — enforce at `onActive` (NOT `onConnect`: `c.steamId` is `"0"` until Steam-auth completes, so a reserved admin would be misread as non-reserved and wrongly kicked; at `onActive` the SteamID is reliable and the client is counted).
  ```
  const n = config.getInt("reserved_slots"); if (n <= 0) return;      // disabled
  if (c.isBot) return;
  const admin = Admin.forSlot(c.slot);
  if (admin && admin.hasFlags(ADMFLAG.RESERVATION)) return;            // reserved player — always allowed
  if (Player.allConnected().length > (Server.maxPlayers - n))          // this non-reserved client is over the public limit
    c.kick("[SM] This server has reserved slots — you were disconnected to keep a slot open for a reserved player.");
  ```
  (At `onActive` the client is already counted by `allConnected()`, so the check includes them. This caps non-reserved players at `maxPlayers - reserved`, leaving `reserved` slots always free — no need to kick *existing* players. The "kick an existing player to make room for a connecting reserved one" variant is deferred — it needs victim selection / a ping primitive we don't have.)

**`@s2script/basetriggers`** (CS2 plugin): answers chat phrases via `Chat.onMessage`.
- `Chat.onMessage((slot, text) => { ... })` — lowercase+trim `text`; if it matches a trigger, broadcast the answer via `Chat.toAll` and `return HookResult.Continue` (show the player's word + the answer, SM-style; do NOT suppress).
  - `timeleft` → `mp_timelimit` (`Server.getCvar("mp_timelimit")` → parseFloat, minutes) and `Server.gameTime` (elapsed sec): `left = timelimit*60 - gameTime`. `timelimit <= 0` → "no time limit"; `left <= 0` → "last round" / "0:00"; else "Time left: M:SS".
  - `thetime` → `new Date().toLocaleTimeString()` → "Current time: HH:MM:SS" (no primitive; process TZ — a deploy note).
  - `currentmap` (and `map`) → `Server.mapName` → "Current map: <name>".
  - `nextmap` → `Server.getCvar("nextlevel")`; non-empty → "Next map: <x>", empty → "Next map: Pending". (`nextmap` proper is deferred to a future `@s2script/nextmap` plugin — no authoritative engine source.)

## Build order (tasks)

- **Task 1 — the server-info op batch.** 3 ops (`server_max_clients`/`server_map_name`/`server_game_time`) ABI-appended after `client_address` (C header + Rust mirror + shim impls + the 2 test op-structs + natives) → `Server.maxPlayers`/`mapName`/`gameTime` in `@s2script/server` + `.d.ts`. Cast the held game-server pointer to `INetworkGameServer*` (mirror the client-list code's use of that pointer; `map_name` uses the `s_*Buf static std::string` pattern; all null-guarded). cargo tests (degrade → 0/""), boundary gate, sniper build.
- **Task 2 — `@s2script/reservedslots`.** `plugins/reservedslots/` — `package.json` (+ `s2script.config.reserved_slots` = `{type:"int", default:0}`), `tsconfig.json`, `src/plugin.ts` (the `Clients.onActive` enforcement above). Typecheck + build.
- **Task 3 — `@s2script/basetriggers`.** `plugins/basetriggers/` — `package.json`, `tsconfig.json`, `src/plugin.ts` (the `Chat.onMessage` triggers above). Typecheck + build.
- Then: deploy + live gate + merge.

## Testing / live gate

- **In-isolate (cargo):** the 3 natives degrade (→ 0/""/0) with no engine; the 2 test op-structs gain 3 `None` fields.
- **Boundary:** the ops return int/string/float — engine-generic; `INetworkGameServer` shim-only. Both gates green.
- **Live (bots-provable — most of it):** `Server.maxPlayers` reads the real max (e.g. 12/64, a known non-zero — validates the vtable call, like sv_gravity=800 validated cvar_get in 6.7); `Server.mapName` = the real map (e.g. `de_dust2`); `Server.gameTime` increases each frame (a plausible map-time float). `basetriggers`: typing `timeleft`/`thetime`/`currentmap` in chat broadcasts a sensible answer (map name correct, time left plausible). `reservedslots`: with `reserved_slots` set below the current bot count, a joining non-reserved client is kicked (or verify the math via logs); `RestartCount=0`, no crash.
- **Deferred human check:** the reservedslots kick of a real non-reserved player + a reserved admin being spared (needs a human + the admin flag) — same human-client bucket.

## Risks / decisions

- **Vtable-index calls (`GetMaxClients`/`GetMapName`/`GetGlobals`):** borrowed from the pinned `iserver.h` + ModSharp-cross-validated (git118: `GetGlobals`=7, `GetMapName`=25 match). Per the RE doctrine this is validate-not-blindly-borrow; the live gate confirms (a wrong index → garbage/crash, caught immediately). `map_name` uses the typed `GetMapName()` to avoid the `string_t` layout ambiguity of `GetGlobals()->mapname`. `game_time` reads the `curtime` field (a small field-offset risk; falls back to `GetTime()` idx 28 if wrong).
- **reservedslots enforce at `onActive`, not `onConnect`** (steamId reliability) — MEDIUM until the human check; the bots-provable part is the capacity math.
- **`timeleft` accuracy:** `curtime` includes warmup/freeze, so it can differ from the HUD by the warmup — approximate, acceptable for an info trigger. Exact `m_flGameStartTime` (CCSGameRules) deferred until a gamerules primitive exists.
- **`nextmap` deferred** — no authoritative engine source; basetriggers answers from `nextlevel` (honest "Pending" when unset).
