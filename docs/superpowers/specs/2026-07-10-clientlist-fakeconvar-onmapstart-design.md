# Client-list refactor + FakeConVar + OnMapStart — design

**Date:** 2026-07-10
**Status:** approved (combined slice, three independent capabilities "in one go")

## Goal

Close three gaps in one slice:

1. **Client-list offset refactor (a treadmill fix)** — `Player.allConnected()` / `sm_who` / `Player.target` return EMPTY on build 2000870 because the shim's engine-identity resolution reads 6 hand-committed gamedata offsets that moved on the update — and offsets, unlike signatures, are never re-scanned, so the gamedata-validation gate can't catch them. Replace the whole offset path with **typed SDK virtuals** (compiler-resolved against the pinned, per-update-bumped hl2sdk) → self-healing on every future update.
2. **FakeConVar** — a plugin registers its OWN ConVar (CSSharp `FakeConVar` / SM `CreateConVar`). We have get/set on existing cvars (6.7); creation is the gap. One new op + `Server.registerCvar`.
3. **OnMapStart** — a real framework map-start event (CSSharp `OnMapStart`), replacing the documented per-plugin `Server.mapName` OnGameFrame poll ([[plugin-lifecycle-map-changes]]). A shim SourceHook → core mux → `Server.onMapStart(handler)`.

## Background / feasibility (confirmed against OUR pinned SDK + the CSSharp reference)

### Client-list

- **The offsets being retired** (`gamedata/core.gamedata.jsonc` `.offsets`): `NetworkServerService.gameServer`=336, `NetworkGameServer.clientCount`=592, `NetworkGameServer.clientElems`=600, `ServerSideClient.name`=64, `ServerSideClient.signon`=100, `ServerSideClient.userId`=168. Consumed only by `S2_ClientAt` + the five `s2_client_*` ops (`shim/src/s2script_mm.cpp:505-596`) and the `S2_GameServer()` helper (`:944`).
- **The typed replacements exist and are already partially proven:**
  - `INetworkServerService::GetIGameServer()` (`third_party/hl2sdk/public/iserver.h:218`) returns `CNetworkGameServerBase*` (IS-A `INetworkGameServer`) — a compiler-resolved virtual replaces the `gameServer`=336 deref. The `INetworkGameServer` typed virtuals (`GetMaxClients`/`GetMapName`/`GetGlobals`) are ALREADY what the server-info ops call on that pointer, live-proven — only the pointer *acquisition* was an offset.
  - `IVEngineServer2::GetPlayerUserId(CPlayerSlot)` (`public/eiface.h:217`, returns `CPlayerUserId`, `.Get()` = -1 when no player at the slot) replaces the `userId` offset AND is the validity signal. **CSSharp-cross-validated:** their `player_manager.cpp` reads userids exclusively via `globals::engine->GetPlayerUserId(slot).Get()` — they never touch `CServerSideClient` at all.
  - `IVEngineServer2::GetClientConVarValue(CPlayerSlot, "name")` (`eiface.h:247`) — the userinfo name (what SM's `GetClientName` reads) — replaces the `name` offset.
  - **Signon has NO engine virtual** (nothing on `IVEngineServer2`/`INetworkGameServer` exposes per-slot signon; reaching the `CServerSideClientBase` object would need the client-array offset — the thing we're removing; `CNetworkGameServerBase` has no `GetClient(slot)` virtual). Replacement: **shim-tracked per-slot state driven by the six ALREADY-INSTALLED `ISource2GameClients` lifecycle hooks** (`OnClientConnected`→2/CONNECTED, `ClientPutInServer`→5/SPAWN, `ClientActive`+`ClientFullyConnect`→6/FULL, `ClientDisconnect`→0). This is also CSSharp's model (their PlayerManager is lifecycle-driven state, not struct reads). The two observable JS gates are preserved: `>= 2` = connected (`kSignonConnected`), `>= 4` = in-game (the `kickWithReason` deliver-now gate; real clients previously read 6=FULL there, tracked clients read 6 too).
- **The SDK header IS the treadmill artifact:** vtable positions come from the pinned `third_party/hl2sdk` (cs2 branch), which the update treadmill already bumps per patch (the 2000870 recovery) — so a future layout change is fixed by the SDK bump we do anyway, with zero shim/gamedata edits. That is the self-healing claim.
- **Zero core change, zero ABI change** for this feature — the 5 op signatures/semantics are preserved; only the shim implementations and gamedata change.

### FakeConVar

- `ICvar::RegisterConVar(const ConVarCreation_t& setup, uint64 nAdditionalFlags, ConVarRef* pCvarRef, ConVarData** pCvarData)` (`public/icvar.h:127`) — a vtable call on the already-held `s_pCvar` (VEngineCvar007), the exact sibling of the PROVEN `RegisterConCommand` path (6.1).
- `ConVarCreation_t : CVarCreationBase_t` (`public/tier1/convar.h:734`) = `{ m_pszName, m_pszHelpString, m_nFlags }` + `ConVarValueInfo_t m_valueInfo` = `{ m_Version(=0 from ctor), m_bHasDefault/Min/Max, raw CVValue_t-sized default/min/max bytes (set via the inline template `SetDefaultValue<T>`/`SetMinValue<T>`/`SetMaxValue<T>`), callback fn-ptrs (all null), EConVarType m_eVarType }`. **Verified against `tier1/convar.cpp`:** the SDK's own path (`CConVar<T>::Register` → `SetupConVar` → queued → `g_pCVar->RegisterConVar(info, flags, &ref, &data)`) passes the struct through untouched (`m_Version` stays 0) — so filling it ourselves + calling the vtable directly is byte-equivalent, and it AVOIDS the non-inline tier1 `SetupConVar`/`SanitiseConVarFlags` (the 5D.1 dlopen cascade).
- **String defaults without CUtlString methods:** `CVValue_t.m_StringValue` is a `CUtlString` = exactly one `char*` member; every `CUtlString` mutator is `DLL_CLASS_IMPORT`. We set the default via `SetDefaultValue<const char*>(persistedBuffer)` — writes the pointer bytes directly into the value slot (guarded by a `static_assert(sizeof(CUtlString) == sizeof(const char*))`), pointing into the shim's persistent name-keyed registry (same name-lifetime-anchor pattern as `s_concommandRefs`). The engine copies the value into its own `ConVarData` at registration (its own `CvarTypeTrait` copy), so our buffer only has to outlive the call — but we persist it anyway (registry entry) for safety.
- **Flags:** the shim ORs `FCVAR_RELEASE` (1<<19, "the only cvars available to customers") unconditionally — without it a retail CS2 hides the cvar. Caller flags are additive.
- **Type surface this slice:** `bool` / `int32` / `float32` / `string` (CSSharp's `FakeConVar<T>` set), + optional numeric min/max. Reads reuse the 6.7 `cvar_get` (already formats all `EConVarType`s); writes reuse `Server.setCvar` (console). Value-change callbacks are DEFERRED.
- **Treadmill note:** `ConVarCreation_t`'s size is ABI-sensitive (it GREW on 2000870 — the Metamod segfault) — we are on the matched cs2 SDK, and a future growth is covered by the same SDK bump that fixes Metamod.

### OnMapStart

- **SOLVED by the CSSharp reference:** `SH_DECL_HOOK3_void(INetworkServerService, StartupServer, SH_NOATTRIB, 0, const GameSessionConfiguration_t&, ISource2WorldSession*, const char*)` + a POST `SH_ADD_HOOK` on the held service — verbatim their `mm_plugin.cpp:82/171-177/210-220`; their `Hook_StartupServer` fires their `OnMapStart` with the map name read from the game server's globals (i.e. the server object is live and named at POST time). **The signature matches OUR `iserver.h:221`** (`StartupServer(const GameSessionConfiguration_t&, ISource2WorldSession*, const char*)`); both param classes are forward-declared in the same header (a reference param needs no definition).
- We already hold `s_pNetworkServerService` (5D.2) and already run six SourceHooks on this exact pattern (`ISource2GameClients` lifecycle → `s2script_core_dispatch_client_event` → `CLIENT_MUX`). OnMapStart is a seventh: hook → FFI export `s2script_core_dispatch_map_start(mapName)` → core `MAP_MUX` (an `event_mux::EventMux` with the fixed `""` key, like `CHAT_MSG_SUBS`) → JS subscribers.
- **Dispatch mode: direct (not post-drain).** `StartupServer` fires from the engine main thread OUTSIDE the frame drain / isolate borrow — exactly like the client-lifecycle hooks, which dispatch directly; the `try_borrow_mut` re-entrancy guard covers the pathological case. Notify-only (no `HookResult`).
- The map name comes from `S2_GameServer()->GetMapName()` in the POST hook (feature 1's typed path; CSSharp-equivalent).

## Architecture

### Feature 1 — client-list refactor (shim + gamedata only; NO op/ABI/core change)

- `S2_GameServer()` → `static_cast<INetworkServerService*>(s_pNetworkServerService)->GetIGameServer()` (null-guarded). Also transparently fixes `server_max_clients`/`server_map_name`/`server_game_time` acquisition.
- `s2_client_userid(slot)` → `s_pEngine->GetPlayerUserId(CPlayerSlot(slot)).Get()` (slot bounds 0..63; `-1` on null engine / empty slot).
- `s2_client_valid(slot)` → `userid != -1` (engine-authoritative; hook-independent).
- `s2_client_name(slot)` → `s_pEngine->GetClientConVarValue(CPlayerSlot(slot), "name")` when valid, else null (core copies, as today).
- `s2_client_signon(slot)` → `s_trackedSignon[slot]` — a `static int[64]` written by the six existing lifecycle `Hook_*` bodies (state set BEFORE the core dispatch so handlers observe it; disconnect clears AFTER dispatch). `-1` for out-of-bounds slot; `0` = never-connected/disconnected.
- `s2_client_find_by_userid(id)` → loop `0..GetMaxClients()` (fallback 64) comparing `s2_client_userid`.
- DELETE: `S2_ClientAt`, the `s_off*` statics, the gamedata `pick()` block + its `GamedataResult` entries + the 6 `.offsets` entries. (`S2_ClientAt` has no other callers — verified; the `s2_event_fire_to_client` comment referencing it gets reworded.)
- **GAMEDATA VALIDATION count drops by 6** (the removed offset presence-checks); still `0 FAILED`.
- Known semantic notes: bots' tracked signon depends on which lifecycle hooks the engine fires for fake clients (`connect` is proven; SPAWN/FULL expected — either reading is fine since nothing gates a bot on `>= 4`); a shim hot-reload mid-map would zero tracked signon for connected clients (not a supported flow — the shim is VDF-loaded at boot); clients persist across changelevel so tracked state persists correctly.

### Feature 2 — FakeConVar (one op, ABI-appended after `user_message_send`, the current last op)

The ABI-append discipline is mandatory: byte-identical across the C header typedef+field, the Rust type+`Option` field, BOTH in-isolate test op-structs, and the shim `ops.` assignment.

1. `int convar_register(const char* name, const char* help, uint64_t flags, int type, const char* defaultValue, const char* minValue, const char* maxValue)` — `type`: 0=bool, 1=int32, 2=float32, 3=string (an s2script enum, mapped shim-side to `EConVarType_Bool/Int32/Float32/String`); `defaultValue` always as a string (shim parses); `minValue`/`maxValue` nullable, numeric types only. Returns 1 on success (or already-registered — idempotent, reload-safe, name-keyed persistent registry mirroring `s_concommandRefs`), 0 on failure (`ref.IsValidRef()` false / null ICvar / bad type). The registered cvar persists for the process lifetime (SM parity — values survive plugin reload; no unregistration, and unlike ConCommands there is no callback into plugin code, so no UAF surface).

JS (in `@s2script/server`, beside the existing `getCvar`/`setCvar`):

```ts
Server.registerCvar("s2_demo_mode", { type: "int", default: 42, help: "…", min: 0, max: 100 }): boolean
```

`opts = { type: "bool"|"int"|"float"|"string", default, help?, flags?, min?, max? }`. Reading back = `Server.getCvar(name)` (6.7); setting = `Server.setCvar` / the console / `sm_cvar`.

### Feature 3 — OnMapStart (no op; a hook + FFI export + core mux, like the client-lifecycle events)

- **Shim:** `SH_DECL_HOOK3_void(INetworkServerService, StartupServer, …)` + POST `SH_ADD_HOOK` at Load (guarded on `s_pNetworkServerService`), `SH_REMOVE_HOOK` at Unload (installed-flag). The hook body reads `S2_GameServer()->GetMapName()`, logs one sanity line (`[s2script] map start: <map> (maxClients=N)` — doubling as feature 1's live vtable sanity check), and calls the new FFI export.
- **Core:** `s2script_core_dispatch_map_start(const char* map)` (ffi.rs, `catch_unwind`, null→`""`) → `dispatch_map_start(&str)` in v8host.rs — mirrors `dispatch_client_event` verbatim (snapshot-release, `try_borrow_mut`, per-sub `is_live` + TryCatch), handler arg = the map-name `v8::String`. `MAP_MUX` (EventMux, fixed key `""`), reset at shutdown + `remove_by_owner` at plugin unload beside `CLIENT_MUX`. Native `__s2_map_start_subscribe(handler)` (mirrors `s2_chat_on_message`).
- **JS:** `Server.onMapStart((mapName: string) => void)` in `@s2script/server` — map start is a Source2 concept → engine-generic, and it lives beside `Server.mapName` (the thing it replaces polling of).
- **Timing contract:** boot-loaded plugins receive the first map's fire (plugins load during shim Load, before the first `StartupServer`); a hot-loaded plugin mid-map does NOT get a synthetic fire for the current map — read `Server.mapName` at load (documented; SM's late-load synthetic `OnMapStart` call is a deferral).
- Existing pollers (nominations / nextmap / rockthevote) are NOT migrated this slice (they work; do-not-build-ahead).

## Boundary (both gates must stay green)

- **Everything in this slice's core/shim is engine-generic** — client identity, convar registration, and map-start are Source2 concepts; `INetworkServerService`/`IVEngineServer2`/`ICvar`/`ConVarCreation_t` are Source2 engine types → shim-only. Expect ZERO CS2 names in core/shim diffs.
- **CS2 layer:** nothing new — `Player.allConnected()`/`Player.target` (pawn.js) just start working again through the unchanged client ops; the demo plugin is the only new consumer.
- Litmus: every line would be true on any Source 2 game.

## Testing

**In-isolate (core, `RUST_TEST_THREADS=1`):**
- Feature 1: none (no core change; existing `client_*` degrade tests still pass).
- Feature 2: `registerCvar` degrades to `false` with no op (never throws); both test op-structs gain `convar_register: None`.
- Feature 3: subscribe via `Server.onMapStart` + call `dispatch_map_start("de_test")` in-isolate → the handler received `"de_test"` (mirrors the `dispatch_client_event` in-isolate test); unsubscribed dispatch is a no-op.

**Live gate (de_inferno → de_dust2, `bot_quota 2`, rcon) — ALL THREE ARE BOTS-PROVABLE:**
- **Client-list:** boot shows NO `identity offsets:` line; `GAMEDATA VALIDATION: (N−6) ok, 0 FAILED`. `sm_clients` (demo) → the 2 bots with real `name`/`userId`/`signonState`(5 or 6)/`steamid "0"`, `fromUserId` round-trips to the same slot; `sm_who` (basecommands, untouched) lists the 2 bots — **the consumer-level proof the 2000870 regression is fixed**; `sm_slap <botname>` resolves by name (Player.target works again).
- **FakeConVar:** boot logs `registerCvar s2_demo_mode -> true value=42`; rcon `s2_demo_mode` → the engine shows value + help text; rcon `s2_demo_mode 77` then `sm_cvar s2_demo_mode` → `77` (the 6.7 read path sees the plugin-registered cvar); re-load (plugin hot-reload) → still `true` (idempotent), value PERSISTS (SM parity).
- **OnMapStart:** boot log `[s2script] map start: de_inferno (maxClients=N)` (the shim hook + typed-vtable sanity in one line); rcon `changelevel de_dust2` → `[cl-demo] onMapStart: de_dust2` and `sm_clients` still valid on the new map (client list survives changelevel); `RestartCount=0` throughout.

**Human-client deferrals (documented):** `kickWithReason`'s deliver-now path over the TRACKED signon (`>= 4`) on a real client (the ban-reason flow — mechanism preserved, gates identical, but the human e2e re-run is deferred like the other human tests); a human client's `name`/`ip` through the new virtuals.

## Deferred (do NOT build ahead)

- ConVar value-CHANGE callbacks (`FnGenericChangeCallback` trampoline → a core dispatch, the config.onChange analog) + typed `getCvarInt/Float`; a synchronous engine-side cvar SET (the 6.7 deferral).
- `onMapEnd` / `StartChangeLevel` hooks; a synthetic `onMapStart` fire for late-loaded plugins.
- Migrating nominations/nextmap/rockthevote off the `Server.mapName` poll onto `Server.onMapStart`.
- A userid-GATED `Client` safe handle (the stored-Client slot-reuse charter gap — unchanged by this slice).
- Vector/Color/etc. convar types; FCVAR flag constants exported to JS.

## Slice shape

One combined slice, one sniper rebuild (shim refactor + one new op + the hook; core gains one op mirror + one native + one mux + one FFI export). Feature 1 is shim+gamedata only; features 2/3 are the only core changes. Built via Workflow (implement → adversarial-review → fix per task, sequential — all tasks touch `s2script_mm.cpp`), opus final review, live gate, merge, push, document.
